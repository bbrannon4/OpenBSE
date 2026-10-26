//! Run-time energy-conservation and sanity checks (#160).
//!
//! OpenBSE hands energy between its models by name: coil outputs are summed
//! into plant loads, supply air is passed to the envelope zone by zone, and
//! HVAC losses are deposited into zones as gains. A load can go missing at
//! any of those hand-offs while every component stays correct on its own,
//! which is how coil loads once never reached the plant (#151) and data-center
//! IT heat was counted twice (#150). These checks watch the hand-offs during
//! the run period and report to the `.err` file at the end of the run. They
//! read the solution and never change it.
//!
//! | Check | Balance | Severity |
//! |---|---|---|
//! | Zone air | surface convection + advection + gains + HVAC = storage | severe |
//! | Plant loop | supply equipment delivers no more than the loop demand; demand components report | severe |
//! | Unphysical states | NaN/inf output, negative PLR or capacity, COP ≤ 0 while running | severe |
//! | Envelope holds | surface temperatures beyond ±200 °C, NaN states replaced | severe |
//! | HVAC → zone | heat the air loops, VRF and radiant panels send = heat the zone receives | severe |
//! | Air component | ṁ·Δh across each air-side component = its `thermal_output` | warning |
//! | Plant unmet demand | capacity-limited loops (E+ "Plant Supply Side Unmet Demand Rate") | warning |
//! | HVAC convergence | timesteps whose HVAC iteration hit its limit | warning |
//!
//! The closure checks that every shipped model passes are severe. The
//! air-component check is a warning until the imbalance it found is fixed:
//! the A205 DX coil's `thermal_output` differs from its air-side enthalpy
//! change (#168). Make it severe once the shipped models pass it.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use openbse_envelope::diagnostics::{EnvelopeDiagnostics, HoldEvent, StepTime, SURFACE_TEMP_LIMIT};
use openbse_envelope::zone::ZoneAirBalance;
use openbse_io::ValidationResult;

/// A timestep's imbalance counts when it exceeds this fraction of the flows
/// being balanced (EnergyPlus's zone check uses 20% of the RSS of the terms)…
pub const STEP_TOL_FRAC: f64 = 0.01;
/// …and this floor [W], so round-off on idle equipment is not counted.
pub const STEP_TOL_W: f64 = 1.0;
/// A balance fails over the run when |Σ residual| exceeds this fraction of
/// the largest flow it balances.
pub const ANNUAL_TOL_FRAC: f64 = 0.01;
/// At most this many objects are named per check; the rest are counted.
const MAX_REPORTED: usize = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Severity {
    Warning,
    Severe,
}

/// Running totals for one balance: a zone, a loop or a component.
#[derive(Debug, Clone, Default)]
pub struct Balance {
    /// Σ residual·dt [Wh], signed.
    pub residual_wh: f64,
    /// Σ scale·dt [Wh]: the size of the flows being balanced.
    pub scale_wh: f64,
    /// Timesteps whose residual exceeded the step tolerance.
    pub steps_over: u64,
    pub steps: u64,
    /// The flagged residual with the largest magnitude [W], and its scale.
    pub worst: f64,
    pub worst_scale: f64,
    pub worst_time: StepTime,
}

impl Balance {
    /// Record one timestep. `step_scale` sets this step's tolerance and
    /// `annual_scale` is added to the run total the annual residual is
    /// judged against.
    fn record(
        &mut self,
        residual: f64,
        step_scale: f64,
        annual_scale: f64,
        dt_h: f64,
        now: StepTime,
    ) {
        self.steps += 1;
        if residual.is_nan() {
            self.steps_over += 1;
            self.worst = f64::NAN;
            self.worst_time = now;
            return;
        }
        self.residual_wh += residual * dt_h;
        self.scale_wh += annual_scale.abs() * dt_h;
        if residual.abs() > STEP_TOL_FRAC * step_scale.abs() + STEP_TOL_W {
            self.steps_over += 1;
            if !self.worst.is_nan() && residual.abs() > self.worst.abs() {
                self.worst = residual;
                self.worst_scale = step_scale;
                self.worst_time = now;
            }
        }
    }

    /// |Σ residual| over Σ scale.
    pub fn annual_fraction(&self) -> f64 {
        if self.scale_wh > 0.0 {
            self.residual_wh.abs() / self.scale_wh
        } else if self.residual_wh.abs() > 0.0 {
            f64::INFINITY
        } else {
            0.0
        }
    }

    pub fn annual_fails(&self) -> bool {
        self.annual_fraction() > ANNUAL_TOL_FRAC && self.residual_wh.abs() > 1.0
    }

    fn flagged(&self) -> bool {
        self.steps_over > 0 || self.annual_fails()
    }

    /// "in N of M timesteps … worst …; over the run …".
    fn describe(&self) -> String {
        let worst = if self.worst.is_nan() {
            format!("NaN at {}", self.worst_time)
        } else {
            format!(
                "worst {:.0} W against {:.0} W at {}",
                self.worst, self.worst_scale, self.worst_time
            )
        };
        let mut s = format!(
            "{} of {} timesteps off by more than {:.0}% + {:.0} W ({worst}); ",
            self.steps_over,
            self.steps,
            STEP_TOL_FRAC * 100.0,
            STEP_TOL_W
        );
        if self.steps_over == 0 {
            s = String::new();
        }
        s + &format!(
            "net over the run {:.1} kWh of {:.1} kWh ({:.2}%)",
            self.residual_wh / 1000.0,
            self.scale_wh / 1000.0,
            self.annual_fraction() * 100.0
        )
    }
}

/// Plant loop demand that its equipment could not meet [Wh].
#[derive(Debug, Clone, Default)]
pub struct Unmet {
    pub unmet_wh: f64,
    pub demand_wh: f64,
    pub steps: u64,
    pub worst: f64,
    pub worst_demand: f64,
    pub worst_time: StepTime,
}

/// HVAC iterations that stopped at their limit.
#[derive(Debug, Clone, Default)]
pub struct NonConvergence {
    pub steps: u64,
    pub total_steps: u64,
    pub worst_delta: f64,
    pub worst_zone: String,
    pub worst_time: StepTime,
    /// Non-converged timesteps per zone that had the largest change.
    pub by_zone: BTreeMap<String, u64>,
}

/// One unphysical output value of a component.
#[derive(Debug, Clone, Default)]
pub struct Unphysical {
    pub steps: u64,
    pub worst: f64,
    pub first_time: StepTime,
}

/// Everything the checks have seen during one run period.
#[derive(Debug, Default)]
pub struct RunChecks {
    pub zone_air: BTreeMap<String, Balance>,
    pub delivery: BTreeMap<String, Balance>,
    pub plant_excess: BTreeMap<String, Balance>,
    pub plant_unmet: BTreeMap<String, Unmet>,
    /// Plant demand components not yet seen in the outputs, by loop.
    pub plant_unseen: BTreeMap<String, BTreeSet<String>>,
    pub air_components: BTreeMap<String, Balance>,
    pub convergence: NonConvergence,
    /// Keyed by "'component' key: what is wrong".
    pub unphysical: BTreeMap<String, Unphysical>,
    pub envelope: Vec<(&'static str, EnvelopeDiagnostics)>,
}

impl RunChecks {
    /// Start a run; every component a plant loop draws on must report at
    /// least once, or its load never reached the loop.
    pub fn new(plant_demands: &HashMap<String, Vec<String>>) -> Self {
        Self {
            plant_unseen: plant_demands
                .iter()
                .map(|(l, names)| (l.clone(), names.iter().cloned().collect()))
                .collect(),
            ..Default::default()
        }
    }

    /// Zone air heat balance at the converged state.
    pub fn zone_air(&mut self, zone: &str, b: &ZoneAirBalance, dt_h: f64, now: StepTime) {
        let largest = [
            b.surface_convection,
            b.advection,
            b.internal_gains,
            b.hvac,
            b.storage,
        ]
        .iter()
        .fold(0.0_f64, |m, v| m.max(v.abs()));
        self.zone_air.entry(zone.to_string()).or_default().record(
            b.residual(),
            b.rss(),
            largest,
            dt_h,
            now,
        );
    }

    /// Heat the HVAC side sent to a zone against what the zone received [W].
    pub fn delivery(&mut self, zone: &str, sent: f64, received: f64, dt_h: f64, now: StepTime) {
        let scale = sent.abs().max(received.abs());
        self.delivery.entry(zone.to_string()).or_default().record(
            sent - received,
            scale,
            scale,
            dt_h,
            now,
        );
    }

    /// A plant loop's demand [W, signed] against what its supply equipment
    /// delivered [W, magnitude].
    pub fn plant_loop(
        &mut self,
        plant_loop: &str,
        demand: f64,
        delivered: f64,
        dt_h: f64,
        now: StepTime,
    ) {
        let demand = demand.abs();
        let excess = (delivered - demand).max(0.0);
        self.plant_excess
            .entry(plant_loop.to_string())
            .or_default()
            .record(excess, demand, demand.max(delivered), dt_h, now);
        let u = self.plant_unmet.entry(plant_loop.to_string()).or_default();
        let unmet = (demand - delivered).max(0.0);
        u.demand_wh += demand * dt_h;
        u.unmet_wh += unmet * dt_h;
        if unmet > STEP_TOL_FRAC * demand + STEP_TOL_W {
            u.steps += 1;
            if unmet > u.worst {
                u.worst = unmet;
                u.worst_demand = demand;
                u.worst_time = now;
            }
        }
    }

    /// Air-side components: ṁ·(h_out − h_in) must equal `thermal_output`
    /// (the `AirComponent` contract), and plant demand components must report.
    pub fn components(
        &mut self,
        outputs: &HashMap<String, HashMap<String, f64>>,
        dt_h: f64,
        now: StepTime,
    ) {
        for names in self.plant_unseen.values_mut() {
            names.retain(|n| {
                !outputs
                    .get(n)
                    .is_some_and(|o| o.contains_key("thermal_output"))
            });
        }
        for (name, o) in outputs {
            let (Some(&m), Some(&h_in), Some(&h_out), Some(&q)) = (
                o.get("mass_flow"),
                o.get("inlet_enthalpy"),
                o.get("outlet_enthalpy"),
                o.get("thermal_output"),
            ) else {
                continue;
            };
            let dh = m * (h_out - h_in);
            let scale = dh.abs().max(q.abs());
            if scale == 0.0 && !self.air_components.contains_key(name) {
                continue;
            }
            self.air_components.entry(name.clone()).or_default().record(
                dh - q,
                scale,
                scale,
                dt_h,
                now,
            );
        }
    }

    /// Unphysical output values: NaN or inf anywhere, a negative part-load
    /// ratio or capacity, a COP ≤ 0 while the component runs.
    pub fn states(&mut self, outputs: &HashMap<String, HashMap<String, f64>>, now: StepTime) {
        for (name, o) in outputs {
            let running = o
                .iter()
                .any(|(k, &v)| (k == "plr" || k == "part_load_ratio") && v > 0.0);
            for (key, &v) in o {
                let problem = if !v.is_finite() {
                    "not a finite number"
                } else if v >= 0.0 && !(v == 0.0 && running && key.starts_with("cop")) {
                    continue;
                } else if key.contains("plr") || key.contains("part_load_ratio") || key == "rtf" {
                    "negative part-load ratio"
                } else if key.contains("capacity") {
                    "negative capacity"
                } else if key.starts_with("cop") {
                    "COP ≤ 0"
                } else {
                    continue;
                };
                let e = self
                    .unphysical
                    .entry(format!("'{name}' {key}: {problem}"))
                    .or_default();
                if e.steps == 0 {
                    e.first_time = now;
                    e.worst = v;
                } else if v.is_nan() || v.abs() > e.worst.abs() {
                    e.worst = v;
                }
                e.steps += 1;
            }
        }
    }

    /// One timestep's HVAC iteration result; `worst` is the zone with the
    /// largest last-iteration change [K] when it did not converge.
    pub fn hvac_iteration(&mut self, converged: bool, worst: Option<(&str, f64)>, now: StepTime) {
        let c = &mut self.convergence;
        c.total_steps += 1;
        if converged {
            return;
        }
        c.steps += 1;
        if let Some((zone, delta)) = worst {
            *c.by_zone.entry(zone.to_string()).or_default() += 1;
            if delta > c.worst_delta {
                c.worst_delta = delta;
                c.worst_zone = zone.to_string();
                c.worst_time = now;
            }
        }
    }

    /// Envelope holds recorded during `phase` (sizing, warmup, run period).
    pub fn envelope(&mut self, phase: &'static str, d: EnvelopeDiagnostics) {
        if !d.is_empty() {
            self.envelope.push((phase, d));
        }
    }

    /// Write every finding into `v` as a warning or severe error.
    pub fn report(&self, v: &mut ValidationResult) {
        let mut push = |sev: Severity, msg: String| match sev {
            Severity::Warning => v.push_warning(msg),
            Severity::Severe => v.push_severe(msg),
        };

        balances(
            &mut push,
            Severity::Severe,
            &self.zone_air,
            "Zone '{}' air heat balance does not close (surface convection + outdoor/interzone \
             air + convective gains + HVAC − storage)",
        );
        // The zones receive the supply the coils produced (corrector solve,
        // #166) and the panels' whole output (#167), so a gap is a wiring bug.
        balances(
            &mut push,
            Severity::Severe,
            &self.delivery,
            "Zone '{}' receives different heat than its HVAC sends (air loop supply, VRF and \
             radiant panels, as sent − as received)",
        );
        balances(
            &mut push,
            Severity::Severe,
            &self.plant_excess,
            "Plant loop '{}' equipment delivers more heat than the loop demand",
        );
        for (l, names) in &self.plant_unseen {
            if !names.is_empty() {
                let list: Vec<String> = names.iter().map(|n| format!("'{n}'")).collect();
                push(
                    Severity::Severe,
                    format!(
                        "Plant loop '{l}': demand component(s) {} never reported a thermal \
                         output, so their loads never reached the loop",
                        list.join(", ")
                    ),
                );
            }
        }
        let mut unmet: Vec<_> = self
            .plant_unmet
            .iter()
            .filter(|(_, u)| u.unmet_wh > ANNUAL_TOL_FRAC * u.demand_wh && u.unmet_wh > 1.0)
            .collect();
        unmet.sort_by(|a, b| b.1.unmet_wh.total_cmp(&a.1.unmet_wh));
        for (l, u) in unmet {
            push(
                Severity::Warning,
                format!(
                    "Plant loop '{l}' supply equipment could not meet its demand: {:.1} kWh of \
                     {:.1} kWh unmet ({:.2}%) in {} timesteps; worst {:.0} W of {:.0} W at {}",
                    u.unmet_wh / 1000.0,
                    u.demand_wh / 1000.0,
                    100.0 * u.unmet_wh / u.demand_wh.max(1e-9),
                    u.steps,
                    u.worst,
                    u.worst_demand,
                    u.worst_time
                ),
            );
        }
        balances(
            &mut push,
            Severity::Warning,
            &self.air_components,
            "Air component '{}': air enthalpy change ṁ·Δh differs from its thermal_output",
        );

        let c = &self.convergence;
        if c.steps > 0 {
            let mut zones: Vec<_> = c.by_zone.iter().collect();
            zones.sort_by(|a, b| b.1.cmp(a.1));
            let zones: Vec<String> = zones
                .iter()
                .take(5)
                .map(|(z, n)| format!("'{z}' ({n})"))
                .collect();
            push(
                Severity::Warning,
                format!(
                    "HVAC iteration did not converge in {} of {} timesteps; largest last \
                     change {:.2} K in zone '{}' at {}; zones most often unconverged: {}",
                    c.steps,
                    c.total_steps,
                    c.worst_delta,
                    c.worst_zone,
                    c.worst_time,
                    zones.join(", ")
                ),
            );
        }

        for (what, u) in self.unphysical.iter().take(MAX_REPORTED * 2) {
            push(
                Severity::Severe,
                format!(
                    "Component {what} in {} timesteps, first at {} (worst {})",
                    u.steps, u.first_time, u.worst
                ),
            );
        }
        if self.unphysical.len() > MAX_REPORTED * 2 {
            push(
                Severity::Severe,
                format!(
                    "… and {} more unphysical component outputs",
                    self.unphysical.len() - MAX_REPORTED * 2
                ),
            );
        }

        for (phase, d) in &self.envelope {
            holds(&mut push, phase, &d.surface_temp_holds, |s, e| {
                format!(
                    "Surface '{s}' inside temperature left ±{SURFACE_TEMP_LIMIT} °C and was held \
                     at its previous value"
                ) + &hold_detail(e)
            });
            holds(&mut push, phase, &d.nan_holds, |s, e| {
                format!("NaN {s} replaced by its previous value") + &hold_detail(e)
            });
        }
    }
}

fn hold_detail(e: &HoldEvent) -> String {
    format!(
        " in {} timesteps, first at {}, worst {:.1} at {}",
        e.timesteps, e.first_time, e.worst, e.worst_time
    )
}

fn holds(
    push: &mut impl FnMut(Severity, String),
    phase: &str,
    events: &BTreeMap<String, HoldEvent>,
    msg: impl Fn(&str, &HoldEvent) -> String,
) {
    let mut sorted: Vec<_> = events.iter().collect();
    sorted.sort_by(|a, b| b.1.timesteps.cmp(&a.1.timesteps));
    for (name, e) in sorted.iter().take(MAX_REPORTED) {
        push(Severity::Severe, format!("[{phase}] {}", msg(name, e)));
    }
    if sorted.len() > MAX_REPORTED {
        push(
            Severity::Severe,
            format!("[{phase}] … and {} more", sorted.len() - MAX_REPORTED),
        );
    }
}

/// Report the flagged balances, largest annual fraction first.
fn balances(
    push: &mut impl FnMut(Severity, String),
    sev: Severity,
    all: &BTreeMap<String, Balance>,
    what: &str,
) {
    let mut flagged: Vec<_> = all.iter().filter(|(_, b)| b.flagged()).collect();
    flagged.sort_by(|a, b| {
        b.1.annual_fraction()
            .total_cmp(&a.1.annual_fraction())
            .then(b.1.steps_over.cmp(&a.1.steps_over))
    });
    for (name, b) in flagged.iter().take(MAX_REPORTED) {
        push(
            sev,
            format!("{}: {}", what.replacen("{}", name, 1), b.describe()),
        );
    }
    if flagged.len() > MAX_REPORTED {
        let rest = what.replacen("'{}' ", "", 1);
        push(
            sev,
            format!("… and {} more: {rest}", flagged.len() - MAX_REPORTED),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> StepTime {
        StepTime {
            month: 1,
            day: 15,
            hour: 8,
            sub_hour: 1,
            timesteps_per_hour: 4,
            sizing: false,
        }
    }

    fn report(c: &RunChecks) -> ValidationResult {
        let mut v = ValidationResult::new();
        c.report(&mut v);
        v
    }

    fn closed_zone() -> ZoneAirBalance {
        ZoneAirBalance {
            surface_convection: -1500.0,
            advection: -300.0,
            internal_gains: 400.0,
            hvac: 1500.0,
            storage: 100.0,
        }
    }

    #[test]
    fn closed_balances_report_nothing() {
        let mut c = RunChecks::new(&HashMap::from([("HW".into(), vec!["Coil".into()])]));
        for _ in 0..100 {
            c.zone_air("Z", &closed_zone(), 0.25, now());
            c.delivery("Z", 1500.0, 1500.0, 0.25, now());
            c.plant_loop("HW", 2000.0, 2000.0, 0.25, now());
            c.hvac_iteration(true, None, now());
        }
        let outputs = HashMap::from([(
            "Coil".to_string(),
            HashMap::from([
                ("mass_flow".to_string(), 1.0),
                ("inlet_enthalpy".to_string(), 20_000.0),
                ("outlet_enthalpy".to_string(), 22_000.0),
                ("thermal_output".to_string(), 2000.0),
                ("plr".to_string(), 0.5),
                ("cop".to_string(), 3.0),
            ]),
        )]);
        c.components(&outputs, 0.25, now());
        c.states(&outputs, now());
        let v = report(&c);
        assert!(v.diagnostics.is_empty(), "{}", v.to_err_file());
    }

    #[test]
    fn a_dropped_zone_gain_is_severe() {
        let mut c = RunChecks::default();
        let mut z = closed_zone();
        z.internal_gains = 0.0; // a gain the solve used but the terms lost
        c.zone_air("Office", &z, 0.25, now());
        let v = report(&c);
        assert_eq!(v.error_count(), 1, "{}", v.to_err_file());
        assert!(v.to_err_file().contains("Zone 'Office' air heat balance"));
    }

    #[test]
    fn heat_sent_but_not_received_is_severe() {
        // A radiant panel delivers its convective share nowhere.
        let mut c = RunChecks::default();
        c.delivery("Room", 1000.0, 700.0, 0.25, now());
        let v = report(&c);
        assert_eq!(v.error_count(), 1, "{}", v.to_err_file());
        let err = v.to_err_file();
        assert!(err.contains("Zone 'Room' receives different heat"), "{err}");
        assert!(err.contains("worst 300 W"), "{err}");
    }

    #[test]
    fn plant_over_delivery_is_severe_and_unmet_is_a_warning() {
        let mut c = RunChecks::default();
        c.plant_loop("CHW", -5000.0, 6000.0, 1.0, now());
        c.plant_loop("HW", 5000.0, 4000.0, 1.0, now());
        let v = report(&c);
        assert_eq!(v.error_count(), 1, "{}", v.to_err_file());
        assert_eq!(v.warning_count(), 1, "{}", v.to_err_file());
        let err = v.to_err_file();
        assert!(err.contains("'CHW' equipment delivers more"), "{err}");
        assert!(
            err.contains("'HW' supply equipment could not meet"),
            "{err}"
        );
    }

    #[test]
    fn a_coil_load_that_never_reaches_the_plant_is_severe() {
        // #151: the loop looked for the coil under a name it never reported.
        let mut c = RunChecks::new(&HashMap::from([("HW".into(), vec!["HW Coil".into()])]));
        let outputs = HashMap::from([(
            "hw coil".to_string(),
            HashMap::from([("thermal_output".to_string(), 900.0)]),
        )]);
        c.components(&outputs, 0.25, now());
        let err = report(&c).to_err_file();
        assert!(err.contains("'HW Coil' never reported"), "{err}");
    }

    #[test]
    fn a_component_that_breaks_its_enthalpy_contract_is_reported() {
        let mut c = RunChecks::default();
        let outputs = HashMap::from([(
            "Humidifier".to_string(),
            HashMap::from([
                ("mass_flow".to_string(), 2.0),
                ("inlet_enthalpy".to_string(), 30_000.0),
                ("outlet_enthalpy".to_string(), 31_000.0),
                ("thermal_output".to_string(), 0.0),
            ]),
        )]);
        c.components(&outputs, 0.25, now());
        let v = report(&c);
        assert_eq!(v.warning_count(), 1, "{}", v.to_err_file());
        assert!(v.to_err_file().contains("'Humidifier'"));
    }

    #[test]
    fn unconverged_timesteps_name_the_worst_zone() {
        let mut c = RunChecks::default();
        c.hvac_iteration(true, None, now());
        c.hvac_iteration(false, Some(("Core", 0.4)), now());
        c.hvac_iteration(false, Some(("Perim", 1.2)), now());
        let err = report(&c).to_err_file();
        assert!(err.contains("did not converge in 2 of 3"), "{err}");
        assert!(err.contains("1.20 K in zone 'Perim'"), "{err}");
    }

    #[test]
    fn unphysical_states_are_severe() {
        let mut c = RunChecks::default();
        let outputs = HashMap::from([(
            "Chiller".to_string(),
            HashMap::from([
                ("plr".to_string(), -4.2),
                ("cop".to_string(), 0.0),
                ("capacity".to_string(), -99_999.0),
                ("electric_power".to_string(), f64::NAN),
                ("thermal_output".to_string(), -3000.0), // negative is fine here
            ]),
        )]);
        c.states(&outputs, now());
        let v = report(&c);
        // plr < 0, capacity < 0, NaN power; COP 0 only counts while running (plr > 0).
        assert_eq!(v.error_count(), 3, "{}", v.to_err_file());
        let running = HashMap::from([(
            "DX".to_string(),
            HashMap::from([("plr".to_string(), 0.3), ("cop".to_string(), 0.0)]),
        )]);
        c.states(&running, now());
        assert!(report(&c).to_err_file().contains("'DX' cop: COP ≤ 0"));
    }

    #[test]
    fn envelope_holds_are_severe() {
        let mut d = EnvelopeDiagnostics::default();
        d.surface_temp_held("Slab", 215.0);
        d.nan_held(|| "zone 'Z' air temperature".into());
        let mut c = RunChecks::default();
        c.envelope("warmup", d);
        c.envelope("run period", EnvelopeDiagnostics::default());
        let v = report(&c);
        assert_eq!(v.error_count(), 2, "{}", v.to_err_file());
        assert!(v.to_err_file().contains("[warmup] Surface 'Slab'"));
    }
}
