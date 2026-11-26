//! Multi-speed packaged single-zone units under load control, as EnergyPlus
//! `AirLoopHVAC:UnitarySystem` with `UnitarySystemPerformance:Multispeed`, a
//! `Fan:SystemModel` with discrete speeds and a `Coil:Cooling:DX:MultiSpeed`.
//!
//! The unit has a few operating points, each the loop's components run at one
//! supply flow with the coils in one state:
//!
//! - off: the coils off, the fan at the no-load flow if it keeps running
//!   (none if it cycles);
//! - heating: the heating coil at its supply temperature, at the heating flow;
//! - cooling speed n: the DX coil at full output at speed n, at that speed's
//!   flow.
//!
//! Each point's sensible output to the control zone is measured against the
//! zone air at the start of the timestep, and the timestep is split between
//! two points so their time-weighted output meets the zone's load (E+
//! `UnitarySys::controlUnitarySystemtoLoad`): heating between off and
//! heating; cooling between off and speed 1, the compressor carrying speed 1's
//! PLF cycling penalty; above speed 1 between two adjacent speeds, with no
//! cycling penalty. The components' energy and the zone supply are the same
//! weighted mix of the two points.

use std::collections::HashMap;

use openbse_airloop::{CoilRole, HvacMode, LoopInfo, UnitarySpeeds};
use openbse_core::graph::{GraphComponent, SimulationGraph};
use openbse_core::ports::{AirPort, SimulationContext};
use openbse_core::simulation::ControlSignals;

use crate::{constant_fan_mode, simulate_loop_components};

/// Component outputs of one loop run.
pub type CompOutputs = HashMap<String, HashMap<String, f64>>;

/// Output keys that are rates or flows: a point's share of the timestep
/// scales them. Temperatures, ratios and efficiencies are taken from the
/// point the unit runs at longest.
const EXTENSIVE_KEYS: &[&str] = &[
    "electric_power",
    "fuel_power",
    "thermal_output",
    "mass_flow",
    "compressor_power",
    "condenser_fan_power",
    "sensible_load",
    "latent_load",
    "total_load",
    "cycling_loss",
    "sensible_cooling_rate",
    "latent_cooling_rate",
    "heat_to_air",
    "shaft_power",
    "conduction_loss",
    "leakage_loss",
    "heat_transferred",
];

/// Compressor electricity keys of a DX coil, which carry the cycling penalty.
const COMPRESSOR_KEYS: &[&str] = &["electric_power", "compressor_power", "condenser_fan_power"];

/// One operating point of the unit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Point {
    Off,
    Heat,
    Cool(usize),
}

/// What a point delivered: the components' outputs, the supply air, and the
/// sensible output to the control zone's air [W] (positive heats it).
pub struct PointRun {
    pub outputs: CompOutputs,
    pub supply: AirPort,
    pub output: f64,
}

/// The unit's loads and state this timestep.
pub struct UnitaryLoads {
    /// Mode the predictor calls for.
    pub mode: HvacMode,
    /// Positive heating load to the heating setpoint [W] (0 if none).
    pub heating_load: f64,
    /// Positive cooling load to the cooling setpoint [W] (0 if none).
    pub cooling_load: f64,
    /// Signed loads to the setpoints (E+ `QToHeatSetPt`, `QToCoolSetPt`).
    pub to_heat: f64,
    pub to_cool: f64,
    /// Control zone air at the start of the timestep [°C].
    pub zone_temp: f64,
    /// Share of the unit's air the control zone receives.
    pub control_zone_share: f64,
}

/// The unit's time-averaged result this timestep.
pub struct UnitaryResult {
    pub outputs: CompOutputs,
    pub supply: AirPort,
    /// Share of the timestep the coils are on.
    pub on_fraction: f64,
}

/// Supply flow fraction of design at a point.
fn flow_fraction(speeds: &UnitarySpeeds, point: Point) -> f64 {
    match point {
        Point::Off => speeds.no_load_flow_fraction,
        Point::Heat => speeds.heating_flow_fraction,
        Point::Cool(n) => speeds.cooling_flow_fraction(n),
    }
}

/// Run the loop's components at one operating point.
#[allow(clippy::too_many_arguments)]
fn run_point(
    graph: &mut SimulationGraph,
    ctx: &SimulationContext,
    li: &LoopInfo,
    speeds: &UnitarySpeeds,
    chain: &[String],
    base: &ControlSignals,
    zone_temps: &HashMap<String, f64>,
    t_outdoor: f64,
    point: Point,
    design_flow: f64,
    min_oa_fraction: f64,
    loads: &UnitaryLoads,
) -> PointRun {
    let frac = flow_fraction(speeds, point);
    let mut s = base.clone();
    for f in s.air_mass_flows.values_mut() {
        *f = design_flow * frac;
    }
    // The minimum outdoor air is a fixed flow (E+ Controller:OutdoorAir
    // FixedMinimum), so it is a larger share of a smaller supply; an open
    // economizer holds its mixed-air share.
    let oa = if base.economizer_active {
        base.oa_fraction.unwrap_or(min_oa_fraction)
    } else {
        (min_oa_fraction / frac.max(1e-6)).min(1.0)
    };
    s.oa_fraction = Some(oa);
    let t_return = base.return_air_temp.unwrap_or(loads.zone_temp) + li.return_fan_dt;
    let t_oa = base.effective_oa_temp.unwrap_or(t_outdoor);
    s.mixed_air_temp = Some(t_return * (1.0 - oa) + t_oa * oa);
    for name in chain {
        match li.coil_role(name) {
            CoilRole::Heating => {
                let sp = if point == Point::Heat {
                    li.heating_supply_temp
                } else {
                    -99.0
                };
                s.coil_setpoints.insert(name.clone(), sp);
            }
            CoilRole::Cooling => {
                let sp = if matches!(point, Point::Cool(_)) {
                    -10.0
                } else {
                    99.0
                };
                s.coil_setpoints.insert(name.clone(), sp);
            }
            _ => {}
        }
    }
    let set_speed = |graph: &mut SimulationGraph, speed: Option<usize>| {
        if let Some((coil, _)) = &speeds.cooling_coil {
            if let Some(idx) = graph.node_by_name(coil) {
                if let GraphComponent::Air(c) = graph.component_mut(idx) {
                    c.set_speed(speed);
                }
            }
        }
    };
    if let Point::Cool(n) = point {
        set_speed(graph, Some(n));
    }
    let (outputs, supply) = simulate_loop_components(graph, ctx, chain, &s, zone_temps, t_outdoor);
    set_speed(graph, None);
    let supply = supply.unwrap_or(AirPort::new(ctx.outdoor_air, 0.0));
    let cp = openbse_psychrometrics::cp_air_fn_w(supply.state.w);
    let output =
        supply.mass_flow * loads.control_zone_share * cp * (supply.state.t_db - loads.zone_temp);
    PointRun {
        outputs,
        supply,
        output,
    }
}

/// Time-weighted mix of operating points: `(weight, run, compressor weight)`,
/// the last the weight a DX compressor's electricity takes at that point (its
/// share over the part-load fraction while it cycles).
fn blend(
    points: &[(f64, &PointRun, f64)],
    compressors: &[String],
    ctx: &SimulationContext,
) -> (CompOutputs, AirPort) {
    let mut outputs: CompOutputs = HashMap::new();
    let longest = points
        .iter()
        .max_by(|a, b| a.0.total_cmp(&b.0))
        .map(|p| p.1);
    if let Some(run) = longest {
        outputs = run.outputs.clone();
    }
    for (comp, out) in outputs.iter_mut() {
        let is_compressor = compressors.iter().any(|c| c == comp);
        // The air through the component over the timestep: its inlet and
        // outlet states mixed by mass (enthalpy and moisture), so the mixed
        // ṁ·Δh is the mixed thermal output.
        let flows: Vec<(f64, &HashMap<String, f64>)> = points
            .iter()
            .filter_map(|&(w, run, _)| {
                let o = run.outputs.get(comp)?;
                Some((w * o.get("mass_flow").copied().unwrap_or(0.0), o))
            })
            .collect();
        let mass: f64 = flows.iter().map(|(m, _)| m).sum();
        if mass > 1e-12 {
            for side in ["inlet", "outlet"] {
                let mix = |key: &str| -> Option<f64> {
                    let mut sum = 0.0;
                    for (m, o) in &flows {
                        sum += m * o.get(key).copied()?;
                    }
                    Some(sum / mass)
                };
                let (Some(h), Some(w)) = (
                    mix(&format!("{side}_enthalpy")),
                    mix(&format!("{side}_humidity_ratio")),
                ) else {
                    continue;
                };
                out.insert(format!("{side}_enthalpy"), h);
                out.insert(format!("{side}_humidity_ratio"), w);
                let t = openbse_psychrometrics::tdb_fn_h_w(h, w);
                out.insert(format!("{side}_temperature"), t);
                if side == "outlet" && out.contains_key("outlet_temp") {
                    out.insert("outlet_temp".to_string(), t);
                }
                if side == "outlet" && out.contains_key("outlet_w") {
                    out.insert("outlet_w".to_string(), w);
                }
            }
        }
        for key in EXTENSIVE_KEYS {
            if !out.contains_key(*key) {
                continue;
            }
            let value: f64 = points
                .iter()
                .map(|&(w, run, wc)| {
                    let v = run
                        .outputs
                        .get(comp)
                        .and_then(|o| o.get(*key))
                        .copied()
                        .unwrap_or(0.0);
                    let w = if is_compressor && COMPRESSOR_KEYS.contains(key) {
                        wc
                    } else {
                        w
                    };
                    w * v
                })
                .sum();
            out.insert((*key).to_string(), value);
        }
    }
    let mass: f64 = points.iter().map(|&(w, r, _)| w * r.supply.mass_flow).sum();
    let p_b = points
        .first()
        .map_or(ctx.outdoor_air.p_b, |p| p.1.supply.state.p_b);
    let supply = if mass > 1e-9 {
        let h = points
            .iter()
            .map(|&(w, r, _)| w * r.supply.mass_flow * r.supply.state.h)
            .sum::<f64>()
            / mass;
        let hum = points
            .iter()
            .map(|&(w, r, _)| w * r.supply.mass_flow * r.supply.state.w)
            .sum::<f64>()
            / mass;
        let t = openbse_psychrometrics::tdb_fn_h_w(h, hum);
        AirPort::new(
            openbse_psychrometrics::MoistAirState::new(t, hum, p_b),
            mass,
        )
    } else {
        let state = longest.map_or(ctx.outdoor_air, |r| r.supply.state);
        AirPort::new(state, 0.0)
    };
    (outputs, supply)
}

/// Simulate a multi-speed unit for one HVAC iteration of a timestep: pick
/// the two operating points that bracket its load and mix them.
#[allow(clippy::too_many_arguments)]
pub fn simulate_unitary(
    graph: &mut SimulationGraph,
    ctx: &SimulationContext,
    li: &LoopInfo,
    chain: &[String],
    base: &ControlSignals,
    zone_temps: &HashMap<String, f64>,
    t_outdoor: f64,
    min_oa_fraction: f64,
    loads: &UnitaryLoads,
) -> Option<UnitaryResult> {
    let speeds = li.unitary_speeds.as_ref()?;
    let design_flow = base.air_mass_flows.values().copied().fold(0.0, f64::max);
    let fan_runs_off_cycle =
        li.fan_operating_mode == openbse_io::input::FanOperatingMode::Continuous;
    let run = |graph: &mut SimulationGraph, point: Point| {
        run_point(
            graph,
            ctx,
            li,
            speeds,
            chain,
            base,
            zone_temps,
            t_outdoor,
            point,
            design_flow,
            min_oa_fraction,
            loads,
        )
    };
    // Off: the no-load flow if the fan keeps running, else nothing moves.
    let mut off = run(graph, Point::Off);
    if !fan_runs_off_cycle {
        for out in off.outputs.values_mut() {
            for key in EXTENSIVE_KEYS {
                if let Some(v) = out.get_mut(*key) {
                    *v = 0.0;
                }
            }
        }
        off.supply.mass_flow = 0.0;
        off.output = 0.0;
    }

    // A fan that keeps running supplies its no-load air whatever the coils do,
    // so the unit meets the signed load to the setpoint of its mode, and that
    // air can push the zone past a setpoint, turning on heating or cooling for
    // the load to it (E+ `initLoadBasedControl`, "constant fan pushes zone
    // below heating set point"). A heating mode whose zone needs no heat
    // therefore adds only what keeps the zone from falling below its setpoint.
    let mut mode = loads.mode;
    let mut heating_load = loads.heating_load;
    let mut cooling_load = loads.cooling_load;
    if fan_runs_off_cycle {
        heating_load = loads.to_heat;
        cooling_load = -loads.to_cool;
        if let Some(pushed) = constant_fan_mode(off.output, loads.to_heat, loads.to_cool) {
            mode = pushed;
        }
    }
    if !li.can_condition(mode) {
        mode = HvacMode::Deadband;
    }

    let compressors: Vec<String> = li.dx_compressor_names.iter().cloned().collect();
    let (outputs, supply, on_fraction) = match mode {
        HvacMode::Deadband => {
            let (o, s) = blend(&[(1.0, &off, 1.0)], &compressors, ctx);
            (o, s, 0.0)
        }
        HvacMode::Heating => {
            let heat = run(graph, Point::Heat);
            let span = heat.output - off.output;
            let r = if span > 1.0 {
                ((heating_load - off.output) / span).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let (o, s) = blend(
                &[(1.0 - r, &off, 1.0 - r), (r, &heat, r)],
                &compressors,
                ctx,
            );
            (o, s, r)
        }
        HvacMode::Cooling => {
            let target = -cooling_load;
            let c1 = run(graph, Point::Cool(0));
            let span = c1.output - off.output;
            let cr = if span < -1.0 {
                ((target - off.output) / span).max(0.0)
            } else {
                0.0
            };
            if cr <= 1.0 || speeds.cooling_speeds() == 1 {
                // Cycle speed 1 against off: the compressor's runtime is its
                // part load over its part-load fraction (E+ RTF = PLR/PLF).
                let cr = cr.min(1.0);
                let plf = li
                    .dx_compressor_names
                    .iter()
                    .find_map(|name| {
                        let GraphComponent::Air(c) = graph.component(graph.node_by_name(name)?)
                        else {
                            return None;
                        };
                        Some(
                            c.part_load_fraction(cr)
                                .unwrap_or(1.0 - li.cooling_part_load_cd * (1.0 - cr)),
                        )
                    })
                    .unwrap_or(1.0)
                    .max(0.01);
                let (o, s) = blend(
                    &[(1.0 - cr, &off, 0.0), (cr, &c1, cr / plf)],
                    &compressors,
                    ctx,
                );
                (o, s, cr)
            } else {
                // Between two adjacent speeds, no cycling penalty (E+ "Apply
                // Part Load Fraction to Speeds Greater than 1" = No).
                let mut lower = c1;
                let mut n = 1;
                loop {
                    let upper = run(graph, Point::Cool(n));
                    let span = upper.output - lower.output;
                    let sr = if span < -1.0 {
                        ((target - lower.output) / span).clamp(0.0, 1.0)
                    } else {
                        1.0
                    };
                    if sr < 1.0 || n + 1 >= speeds.cooling_speeds() {
                        let (o, s) = blend(
                            &[(1.0 - sr, &lower, 1.0 - sr), (sr, &upper, sr)],
                            &compressors,
                            ctx,
                        );
                        break (o, s, 1.0);
                    }
                    lower = upper;
                    n += 1;
                }
            }
        }
    };
    Some(UnitaryResult {
        outputs,
        supply,
        on_fraction,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use openbse_psychrometrics::MoistAirState;

    fn run_at(t: f64, m: f64, power: f64) -> PointRun {
        let mut outputs = CompOutputs::new();
        outputs.insert(
            "dx".to_string(),
            HashMap::from([
                ("electric_power".to_string(), power),
                ("outlet_temperature".to_string(), t),
            ]),
        );
        PointRun {
            outputs,
            supply: AirPort::new(MoistAirState::new(t, 0.008, 101325.0), m),
            output: 0.0,
        }
    }

    fn ctx() -> SimulationContext {
        SimulationContext {
            timestep: openbse_core::types::TimeStep {
                month: 7,
                day: 1,
                hour: 12,
                sub_hour: 1,
                timesteps_per_hour: 4,
                sim_time_s: 0.0,
                dt: 900.0,
            },
            outdoor_air: MoistAirState::new(30.0, 0.008, 101325.0),
            day_type: openbse_core::types::DayType::WeatherDay,
            is_sizing: false,
            sizing_internal_gains: openbse_core::ports::SizingInternalGains::Full,
        }
    }

    /// Off at 0.66 of the flow and speed 1 at full flow for a quarter of the
    /// timestep each... mixed by mass and enthalpy; the compressor's
    /// electricity takes its cycling weight, the rest the time weight.
    #[test]
    fn blend_mixes_supply_by_mass_and_compressor_by_runtime() {
        let off = run_at(20.0, 0.66, 0.0);
        let on = run_at(12.0, 1.0, 1000.0);
        let (out, supply) = blend(
            &[(0.75, &off, 0.0), (0.25, &on, 0.25 / 0.8125)],
            &["dx".to_string()],
            &ctx(),
        );
        let m = 0.75 * 0.66 + 0.25;
        assert!((supply.mass_flow - m).abs() < 1e-12);
        let t = (0.75 * 0.66 * 20.0 + 0.25 * 12.0) / m;
        assert!(
            (supply.state.t_db - t).abs() < 0.01,
            "{}",
            supply.state.t_db
        );
        assert!((out["dx"]["electric_power"] - 1000.0 * 0.25 / 0.8125).abs() < 1e-9);
        // An intensive value comes from the point run longest.
        assert_eq!(out["dx"]["outlet_temperature"], 20.0);
    }
}
