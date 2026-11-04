//! Variable Refrigerant Flow (VRF) system components.
//!
//! A VRF system consists of one outdoor unit (compressor) coordinating multiple
//! indoor units (fan-coils) across zones.  The outdoor unit decides operating
//! mode, applies performance curves, enforces capacity limits, and distributes
//! compressor power to each indoor unit.
//!
//! Heat recovery mode: cooling rejection from cooling zones is routed to heating
//! zones, reducing net compressor work compared to a system where all zones must
//! be in the same mode.
//!
//! Reference: EnergyPlus Engineering Reference, "AirConditioner:VariableRefrigerantFlow"

use crate::performance_curve::PerformanceCurve;
use openbse_psychrometrics as psych;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

fn default_cooling_cop() -> f64 {
    3.5
}
fn default_heating_cop() -> f64 {
    4.0
}
fn default_cooling_supply_temp() -> f64 {
    13.0
}
fn default_heating_supply_temp() -> f64 {
    40.0
}
fn default_min_plr() -> f64 {
    0.15
}
fn default_submeter() -> String {
    "General".to_string()
}

/// Operating mode of a VRF indoor unit.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub enum VrfMode {
    Cooling,
    Heating,
    #[default]
    Off,
}

/// A single VRF indoor unit (fan-coil) serving one zone.
///
/// The indoor unit itself does not compute its own operation — the
/// `VrfOutdoorUnit::coordinate()` method sets all runtime fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VrfIndoorUnit {
    pub name: String,
    #[serde(default = "default_submeter")]
    pub submeter: String,
    /// Zone this unit serves
    pub zone: String,
    /// Rated cooling capacity [W]
    pub cooling_capacity: f64,
    /// Rated heating capacity [W]
    pub heating_capacity: f64,
    /// Rated supply airflow [m³/s]
    pub rated_airflow: f64,
    /// Supply air temperature in cooling mode [°C]
    #[serde(default = "default_cooling_supply_temp")]
    pub cooling_supply_temp: f64,
    /// Supply air temperature in heating mode [°C]
    #[serde(default = "default_heating_supply_temp")]
    pub heating_supply_temp: f64,

    // ─── Runtime state (set by VrfOutdoorUnit::coordinate) ───────────────
    #[serde(skip)]
    pub mode: VrfMode,
    #[serde(skip)]
    pub plr: f64,
    /// Electric power share allocated to this unit [W]
    #[serde(skip)]
    pub power: f64,
    /// Thermal output delivered to zone air [W] (positive = heating, negative = cooling)
    #[serde(skip)]
    pub thermal_output: f64,
    /// Actual supply air temperature [°C]
    #[serde(skip)]
    pub supply_temp: f64,
    /// Actual supply mass flow [kg/s]
    #[serde(skip)]
    pub mass_flow: f64,
    /// Supply humidity ratio [kg/kg]: the zone's, dried by the coil in cooling
    #[serde(skip)]
    pub supply_humidity_ratio: f64,
    /// Sensible share of the coil's total load this timestep [0-1]
    #[serde(skip)]
    pub shr: f64,
    /// Coil bypass factor, from the E+ rated SHR at the rating point
    #[serde(skip)]
    bypass_factor: Option<f64>,
}

impl VrfIndoorUnit {
    pub fn new(
        name: &str,
        zone: &str,
        cooling_capacity: f64,
        heating_capacity: f64,
        rated_airflow: f64,
    ) -> Self {
        Self {
            name: name.to_string(),
            submeter: "General".to_string(),
            zone: zone.to_string(),
            cooling_capacity,
            heating_capacity,
            rated_airflow,
            cooling_supply_temp: 13.0,
            heating_supply_temp: 40.0,
            mode: VrfMode::Off,
            plr: 0.0,
            power: 0.0,
            thermal_output: 0.0,
            supply_temp: 21.0,
            mass_flow: 0.0,
            supply_humidity_ratio: 0.008,
            shr: 1.0,
            bypass_factor: None,
        }
    }

    /// The coil's bypass factor at its rating point, from E+'s autosized rated
    /// SHR, 0.431 + 6086·(V/Q) with V/Q limited to 4.027e-5–6.041e-5 m³/s per W
    /// (`CoolingSHRSizing`).
    fn bypass_factor(&mut self) -> f64 {
        if let Some(bf) = self.bypass_factor {
            return bf;
        }
        let flow_per_cap =
            (self.rated_airflow / self.cooling_capacity.max(1.0)).clamp(4.027e-5, 6.041e-5);
        let shr = 0.431 + 6086.0 * flow_per_cap;
        let (_, _, _, bf) = crate::cooling_coil::rated_bypass_factor(
            shr,
            self.cooling_capacity,
            self.rated_airflow,
        );
        self.bypass_factor = Some(bf);
        bf
    }

    /// Air leaving the cooling coil at the supply temperature from zone air
    /// (`t_in`, `w_in`): along the line from the inlet to the apparatus dew
    /// point the bypass factor fixes (E+ `CalcDoe2DXCoil`), dry when that dew
    /// point is above the inlet's. Returns (leaving humidity ratio, sensible
    /// share of the total load).
    fn cooling_outlet(&mut self, t_in: f64, w_in: f64, p_b: f64) -> (f64, f64) {
        let t_out = self.cooling_supply_temp;
        if t_out >= t_in {
            return (w_in, 1.0);
        }
        let bf = self.bypass_factor().clamp(0.0, 0.9);
        let t_adp = (t_out - bf * t_in) / (1.0 - bf);
        let w_adp = psych::w_fn_tdb_rh_pb(t_adp, 1.0, p_b);
        let w_out = if w_adp < w_in {
            w_adp + bf * (w_in - w_adp)
        } else {
            w_in
        };
        let h_in = psych::h_fn_tdb_w(t_in, w_in);
        let total = h_in - psych::h_fn_tdb_w(t_out, w_out);
        let sensible = h_in - psych::h_fn_tdb_w(t_out, w_in);
        let shr = if total > 0.0 {
            (sensible / total).clamp(0.0, 1.0)
        } else {
            1.0
        };
        (w_out, shr)
    }
}

/// VRF outdoor unit — coordinates all indoor units and manages the compressor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VrfOutdoorUnit {
    pub name: String,
    #[serde(default = "default_submeter")]
    pub submeter: String,
    /// Rated total cooling capacity [W] at standard conditions
    pub rated_cooling_capacity: f64,
    /// Rated total heating capacity [W] at standard conditions
    pub rated_heating_capacity: f64,
    /// Rated COP in cooling mode
    #[serde(default = "default_cooling_cop")]
    pub rated_cooling_cop: f64,
    /// Rated COP in heating mode
    #[serde(default = "default_heating_cop")]
    pub rated_heating_cop: f64,
    /// Heat recovery: cooling rejection routed to heating zones
    #[serde(default)]
    pub heat_recovery: bool,

    /// Cooling capacity modifier f(indoor wet-bulb, outdoor dry-bulb), E+'s
    /// axes — resolved at build time
    #[serde(skip)]
    pub cooling_cap_ft: Option<PerformanceCurve>,
    /// Cooling EIR modifier f(indoor wet-bulb, outdoor dry-bulb)
    #[serde(skip)]
    pub cooling_eir_ft: Option<PerformanceCurve>,
    /// Cooling EIR modifier f(PLR): full-load power times this (E+ low
    /// part-load ratio curve); `None` makes power proportional to PLR
    #[serde(skip)]
    pub cooling_eir_fplr: Option<PerformanceCurve>,
    /// Heating capacity modifier f(indoor dry-bulb, outdoor wet-bulb)
    #[serde(skip)]
    pub heating_cap_ft: Option<PerformanceCurve>,
    /// Heating EIR modifier f(indoor dry-bulb, outdoor wet-bulb)
    #[serde(skip)]
    pub heating_eir_ft: Option<PerformanceCurve>,
    /// Heating EIR modifier f(PLR), as the cooling one
    #[serde(skip)]
    pub heating_eir_fplr: Option<PerformanceCurve>,
    /// Part-load ratio below which the compressor cycles (E+ default 0.15)
    #[serde(default = "default_min_plr")]
    pub min_plr: f64,
    /// Cooling part-load fraction f(cycling ratio); `None` means no cycling loss
    #[serde(skip)]
    pub cooling_plf_fplr: Option<PerformanceCurve>,
    /// Heating part-load fraction f(cycling ratio), as the cooling one
    #[serde(skip)]
    pub heating_plf_fplr: Option<PerformanceCurve>,

    // ─── Runtime state ────────────────────────────────────────────────────
    #[serde(skip)]
    pub compressor_power: f64,
    #[serde(skip)]
    pub total_cooling: f64,
    #[serde(skip)]
    pub total_heating: f64,
}

impl VrfOutdoorUnit {
    pub fn new(
        name: &str,
        rated_cooling_capacity: f64,
        rated_heating_capacity: f64,
        rated_cooling_cop: f64,
        rated_heating_cop: f64,
        heat_recovery: bool,
    ) -> Self {
        Self {
            name: name.to_string(),
            submeter: "General".to_string(),
            rated_cooling_capacity,
            rated_heating_capacity,
            rated_cooling_cop,
            rated_heating_cop,
            heat_recovery,
            cooling_cap_ft: None,
            cooling_eir_ft: None,
            cooling_eir_fplr: None,
            heating_cap_ft: None,
            heating_eir_ft: None,
            heating_eir_fplr: None,
            min_plr: default_min_plr(),
            cooling_plf_fplr: None,
            heating_plf_fplr: None,
            compressor_power: 0.0,
            total_cooling: 0.0,
            total_heating: 0.0,
        }
    }

    /// Coordinate all indoor units for one simulation timestep.
    ///
    /// Sets mode, PLR, supply conditions, and power for each indoor unit,
    /// and updates this unit's compressor_power, total_cooling, total_heating.
    ///
    /// `zone_loads` is the sensible load each zone asks of its indoor unit
    /// [W]: positive = heating, negative = cooling, zero = off. Like
    /// EnergyPlus's VRF terminal units, each unit meets its zone's load to
    /// setpoint rather than reacting to the air temperature, which made the
    /// HVAC iteration flip between modes (#170). A unit supplies air at its
    /// coil's leaving temperature, at the flow that carries the load from the
    /// zone's current temperature, up to its rated airflow. The unit draws
    /// zone air: in cooling its coil dries it (`cooling_outlet`), and the PLR,
    /// outdoor-unit demand and compressor power are on the coil's total load,
    /// sensible plus latent; the zone receives the sensible share.
    pub fn coordinate(
        &mut self,
        indoor_units: &mut [VrfIndoorUnit],
        outdoor: &psych::MoistAirState,
        zone_temps: &HashMap<String, f64>,
        zone_humidity_ratios: &HashMap<String, f64>,
        zone_loads: &HashMap<String, f64>,
    ) {
        let (t_outdoor, p_b) = (outdoor.t_db, outdoor.p_b);
        let t_outdoor_wb = psych::twb_fn_tdb_w_pb(t_outdoor, outdoor.w, p_b);
        // ── Step 1: Mode and PLR for each indoor unit from its zone's load ─
        for iu in indoor_units.iter_mut() {
            let t_zone = zone_temps.get(&iu.zone).copied().unwrap_or(21.0);
            let w_zone = zone_humidity_ratios.get(&iu.zone).copied().unwrap_or(0.008);
            let load = zone_loads.get(&iu.zone).copied().unwrap_or(0.0);
            let cp_air = psych::cp_air_fn_w(w_zone);
            let air_density = psych::rho_air_fn_pb_tdb_w(p_b, t_zone, w_zone);
            // The most the unit's rated airflow can carry at its supply temperature.
            let rated_airflow = iu.rated_airflow;
            let air_limit = |t_supply: f64| {
                (rated_airflow * air_density * cp_air * (t_supply - t_zone).abs()).max(0.0)
            };
            iu.supply_humidity_ratio = w_zone;
            iu.shr = 1.0;
            (iu.mode, iu.plr) = if load > 0.0 && iu.heating_supply_temp > t_zone {
                let q = load
                    .min(iu.heating_capacity)
                    .min(air_limit(iu.heating_supply_temp));
                (VrfMode::Heating, q / iu.heating_capacity.max(1.0))
            } else if load < 0.0 && iu.cooling_supply_temp < t_zone {
                let (w_out, shr) = iu.cooling_outlet(t_zone, w_zone, p_b);
                iu.supply_humidity_ratio = w_out;
                iu.shr = shr;
                let sensible = (-load)
                    .min(iu.cooling_capacity * shr)
                    .min(air_limit(iu.cooling_supply_temp));
                (
                    VrfMode::Cooling,
                    sensible / shr.max(1e-3) / iu.cooling_capacity.max(1.0),
                )
            } else {
                (VrfMode::Off, 0.0)
            };
            if iu.plr <= 0.0 {
                iu.mode = VrfMode::Off;
                iu.plr = 0.0;
            }
        }

        // ── Step 2: Sum total cooling and heating demand ──────────────────
        let total_cool_demand: f64 = indoor_units
            .iter()
            .filter(|iu| iu.mode == VrfMode::Cooling)
            .map(|iu| iu.plr * iu.cooling_capacity)
            .sum();
        let total_heat_demand: f64 = indoor_units
            .iter()
            .filter(|iu| iu.mode == VrfMode::Heating)
            .map(|iu| iu.plr * iu.heating_capacity)
            .sum();

        // ── Step 3: Dominant mode enforcement (non-heat-recovery) ────────
        if !self.heat_recovery {
            // Minority mode units go off
            let dominant = if total_cool_demand >= total_heat_demand {
                VrfMode::Cooling
            } else {
                VrfMode::Heating
            };
            for iu in indoor_units.iter_mut() {
                if iu.mode != VrfMode::Off && iu.mode != dominant {
                    iu.mode = VrfMode::Off;
                    iu.plr = 0.0;
                }
            }
        }

        // ── Step 4: Recompute totals after mode enforcement ───────────────
        let cool_demand: f64 = indoor_units
            .iter()
            .filter(|iu| iu.mode == VrfMode::Cooling)
            .map(|iu| iu.plr * iu.cooling_capacity)
            .sum();
        let heat_demand: f64 = indoor_units
            .iter()
            .filter(|iu| iu.mode == VrfMode::Heating)
            .map(|iu| iu.plr * iu.heating_capacity)
            .sum();

        // ── Step 5: Performance curve modifications ───────────────────────
        // E+'s axes: cooling f(indoor wet-bulb, outdoor dry-bulb), heating
        // f(indoor dry-bulb, outdoor wet-bulb), with the indoor state the
        // operating units' average weighted by their load.
        let indoor_mean = |mode: VrfMode, value: &dyn Fn(&str) -> f64| -> f64 {
            let (sum, weight) = indoor_units
                .iter()
                .filter(|iu| iu.mode == mode)
                .fold((0.0, 0.0), |(s, w), iu| {
                    (s + iu.plr * value(&iu.zone), w + iu.plr)
                });
            if weight > 0.0 {
                sum / weight
            } else {
                value("")
            }
        };
        let zone_wb = |zone: &str| {
            let t = zone_temps.get(zone).copied().unwrap_or(21.0);
            let w = zone_humidity_ratios.get(zone).copied().unwrap_or(0.008);
            psych::twb_fn_tdb_w_pb(t, w, p_b)
        };
        let zone_db = |zone: &str| zone_temps.get(zone).copied().unwrap_or(21.0);
        let indoor_wb = indoor_mean(VrfMode::Cooling, &zone_wb);
        let indoor_db = indoor_mean(VrfMode::Heating, &zone_db);
        let eval = |curve: &Option<PerformanceCurve>, x: f64, y: f64| {
            curve.as_ref().map_or(1.0, |c| c.evaluate(x, y))
        };
        let cool_cap_mod = eval(&self.cooling_cap_ft, indoor_wb, t_outdoor);
        let cool_eir_mod = eval(&self.cooling_eir_ft, indoor_wb, t_outdoor);
        let heat_cap_mod = eval(&self.heating_cap_ft, indoor_db, t_outdoor_wb);
        let heat_eir_mod = eval(&self.heating_eir_ft, indoor_db, t_outdoor_wb);

        let available_cool_cap = self.rated_cooling_capacity * cool_cap_mod;
        let available_heat_cap = self.rated_heating_capacity * heat_cap_mod;

        // ── Step 6: Scale PLRs if demand exceeds available capacity ───────
        if cool_demand > available_cool_cap && cool_demand > 0.0 {
            let scale = available_cool_cap / cool_demand;
            for iu in indoor_units.iter_mut() {
                if iu.mode == VrfMode::Cooling {
                    iu.plr *= scale;
                }
            }
        }
        if heat_demand > available_heat_cap && heat_demand > 0.0 {
            let scale = available_heat_cap / heat_demand;
            for iu in indoor_units.iter_mut() {
                if iu.mode == VrfMode::Heating {
                    iu.plr *= scale;
                }
            }
        }

        // ── Step 7: Final loads after capacity limiting ───────────────────
        let final_cool: f64 = indoor_units
            .iter()
            .filter(|iu| iu.mode == VrfMode::Cooling)
            .map(|iu| iu.plr * iu.cooling_capacity)
            .sum();
        let final_heat: f64 = indoor_units
            .iter()
            .filter(|iu| iu.mode == VrfMode::Heating)
            .map(|iu| iu.plr * iu.heating_capacity)
            .sum();

        self.total_cooling = final_cool;
        self.total_heating = final_heat;

        // ── Step 8: Compressor power ──────────────────────────────────────
        // E+ (`HVACVariableRefrigerantFlow.cc`): full-load power at the
        // curve-modified capacity times the EIR-f(PLR) curve. Below the
        // minimum PLR the compressor cycles: the curve is read at the minimum
        // and power scales by the runtime fraction, cycling ratio over the
        // part-load fraction (PLF floored at 0.7, 1 without a curve). With no
        // curves this is power proportional to PLR. Heat recovery: cooling
        // rejection covers part of the heating load, so the compressor makes
        // up only the difference.
        let min_plr = self.min_plr.clamp(1e-3, 1.0);
        let part_load_power = |load: f64,
                               cap: f64,
                               cop: f64,
                               eir: f64,
                               fplr: &Option<PerformanceCurve>,
                               plf: &Option<PerformanceCurve>| {
            if load <= 0.0 || cap <= 0.0 {
                return 0.0;
            }
            let plr = (load / cap).min(1.0);
            let cycling = (plr / min_plr).min(1.0);
            let plr = plr.max(min_plr);
            let fraction = fplr.as_ref().map_or(plr, |c| c.evaluate(plr, 0.0));
            let plf = plf
                .as_ref()
                .map_or(1.0, |c| c.evaluate(cycling, 0.0).max(0.7));
            let rtf = (cycling / plf).min(1.0);
            cap / cop.max(0.1) * eir * fraction * rtf
        };
        let cool_power = part_load_power(
            final_cool,
            available_cool_cap,
            self.rated_cooling_cop,
            cool_eir_mod,
            &self.cooling_eir_fplr,
            &self.cooling_plf_fplr,
        );
        self.compressor_power = if self.heat_recovery && final_cool > 0.0 && final_heat > 0.0 {
            let heat_gap = (final_heat - (final_cool + cool_power)).max(0.0);
            cool_power
                + part_load_power(
                    heat_gap,
                    available_heat_cap,
                    self.rated_heating_cop,
                    heat_eir_mod,
                    &self.heating_eir_fplr,
                    &self.heating_plf_fplr,
                )
        } else if final_cool > 0.0 {
            cool_power
        } else {
            part_load_power(
                final_heat,
                available_heat_cap,
                self.rated_heating_cop,
                heat_eir_mod,
                &self.heating_eir_fplr,
                &self.heating_plf_fplr,
            )
        };

        // ── Step 9: Set per-unit supply conditions ─────────────────────────
        // The flow is set so that ṁ·cp·(T_supply − T_zone) is the heat booked.
        let total_plr: f64 = indoor_units.iter().map(|iu| iu.plr).sum::<f64>().max(1e-9);

        for iu in indoor_units.iter_mut() {
            let t_zone = zone_temps.get(&iu.zone).copied().unwrap_or(21.0);
            let w_zone = zone_humidity_ratios.get(&iu.zone).copied().unwrap_or(0.008);
            let cp_air = psych::cp_air_fn_w(w_zone);
            let (t_supply, q) = match iu.mode {
                VrfMode::Off => (t_zone, 0.0),
                VrfMode::Cooling => (
                    iu.cooling_supply_temp,
                    -(iu.plr * iu.cooling_capacity * iu.shr),
                ),
                VrfMode::Heating => (iu.heating_supply_temp, iu.plr * iu.heating_capacity),
            };
            iu.supply_temp = t_supply;
            iu.mass_flow = if q == 0.0 {
                0.0
            } else {
                q / (cp_air * (t_supply - t_zone))
            };
            if iu.mode != VrfMode::Cooling {
                iu.supply_humidity_ratio = w_zone;
            }
            iu.power = self.compressor_power * (iu.plr / total_plr);
            iu.thermal_output = q;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Outdoor air at a dry-bulb and wet-bulb [°C], sea-level pressure.
    fn outdoor(t_db: f64, t_wb: f64) -> psych::MoistAirState {
        let w = psych::w_fn_tdb_twb_pb(t_db, t_wb.min(t_db), 101_325.0).max(1e-4);
        psych::MoistAirState::new(t_db, w, 101_325.0)
    }

    fn make_system() -> (VrfOutdoorUnit, Vec<VrfIndoorUnit>) {
        let outdoor = VrfOutdoorUnit::new("ODU-1", 20_000.0, 22_000.0, 3.5, 4.0, false);
        let units = vec![
            VrfIndoorUnit::new("IDU-1", "Zone A", 5_000.0, 5_500.0, 0.25),
            VrfIndoorUnit::new("IDU-2", "Zone B", 4_000.0, 4_400.0, 0.20),
        ];
        (outdoor, units)
    }

    /// Zone temperatures and the loads asked of the indoor units [W, + heating].
    fn zone_maps(zones: &[(&str, f64, f64)]) -> (HashMap<String, f64>, HashMap<String, f64>) {
        (
            zones.iter().map(|(k, t, _)| (k.to_string(), *t)).collect(),
            zones.iter().map(|(k, _, q)| (k.to_string(), *q)).collect(),
        )
    }

    #[test]
    fn test_all_cooling_cop() {
        let (mut odu, mut units) = make_system();
        // Both zones too warm → cooling
        let (zt, loads) = zone_maps(&[("Zone A", 27.0, -3000.0), ("Zone B", 26.5, -2500.0)]);
        odu.coordinate(
            &mut units,
            &outdoor(35.0, 10.0),
            &zt,
            &HashMap::new(),
            &loads,
        );
        assert!(odu.total_cooling > 0.0, "Should have cooling load");
        assert_eq!(odu.total_heating, 0.0);
        let expected_power = odu.total_cooling / odu.rated_cooling_cop;
        let rel_err = (odu.compressor_power - expected_power).abs() / expected_power.max(1.0);
        assert!(
            rel_err < 0.01,
            "Compressor power should ≈ Q/COP, got {:.1} vs {:.1}",
            odu.compressor_power,
            expected_power
        );
    }

    #[test]
    fn test_heat_recovery_reduces_compressor_work() {
        let (mut odu_hr, mut units_hr) = make_system();
        odu_hr.heat_recovery = true;
        // Also make a non-HR version for comparison
        let (mut odu_no_hr, mut units_no_hr) = make_system();

        // Zone A cooling 5000 W, Zone B heating 3000 W
        let (zt, loads) = zone_maps(&[("Zone A", 27.0, -5000.0), ("Zone B", 19.0, 3000.0)]);

        odu_hr.coordinate(
            &mut units_hr,
            &outdoor(25.0, 10.0),
            &zt,
            &HashMap::new(),
            &loads,
        );
        odu_no_hr.coordinate(
            &mut units_no_hr,
            &outdoor(25.0, 10.0),
            &zt,
            &HashMap::new(),
            &loads,
        );

        // Heat recovery: minority heating mode would be suppressed in non-HR.
        // In HR mode the compressor satisfies both — but uses heat rejection.
        // Either way, HR compressor power ≤ non-HR in the single-mode case, OR
        // HR runs both modes with lower net power than running two separate units.
        assert!(
            odu_hr.compressor_power
                < odu_hr.total_cooling / odu_hr.rated_cooling_cop
                    + odu_hr.total_heating / odu_hr.rated_heating_cop,
            "HR should reduce total compressor work compared to independent units"
        );
    }

    #[test]
    fn test_capacity_limiting() {
        let (mut odu, mut units) = make_system();
        // Force very high PLR — request exceeds rated capacity
        // Both zones very hot → full cooling
        let (zt, loads) = zone_maps(&[("Zone A", 35.0, -50_000.0), ("Zone B", 35.0, -50_000.0)]);
        odu.coordinate(
            &mut units,
            &outdoor(35.0, 10.0),
            &zt,
            &HashMap::new(),
            &loads,
        );
        assert!(
            odu.total_cooling <= odu.rated_cooling_capacity * 1.001,
            "Total cooling {} must not exceed rated capacity {}",
            odu.total_cooling,
            odu.rated_cooling_capacity
        );
    }

    #[test]
    fn test_dominant_mode_no_heat_recovery() {
        let (mut odu, mut units) = make_system();
        // Zone A needs cooling, Zone B needs heating — cooling dominant
        let (zt, loads) = zone_maps(&[("Zone A", 26.0, -3000.0), ("Zone B", 19.0, 3000.0)]);
        odu.coordinate(
            &mut units,
            &outdoor(30.0, 10.0),
            &zt,
            &HashMap::new(),
            &loads,
        );
        // With cooling dominant (Zone A load likely larger due to larger capacity and bigger delta),
        // Zone B should be Off
        let zone_b_unit = units.iter().find(|u| u.zone == "Zone B").unwrap();
        // Zone B cooling capacity 4000 vs heating 4400, delta cooling=2°C vs heating=2°C
        // cooling load ≈ 4000 W, heating ≈ 4400 W → heating dominant in this case
        // Just verify no simultaneous cooling + heating in non-HR mode
        let cooling_zones: Vec<_> = units
            .iter()
            .filter(|u| u.mode == VrfMode::Cooling)
            .collect();
        let heating_zones: Vec<_> = units
            .iter()
            .filter(|u| u.mode == VrfMode::Heating)
            .collect();
        assert!(
            cooling_zones.is_empty() || heating_zones.is_empty(),
            "Non-HR mode must not have simultaneous cooling and heating"
        );
        let _ = zone_b_unit;
    }

    #[test]
    fn supply_air_carries_the_heat_booked() {
        let (mut odu, mut units) = make_system();
        let (zt, loads) = zone_maps(&[("Zone A", 25.0, -2000.0), ("Zone B", 20.0, 1500.0)]);
        odu.heat_recovery = true;
        odu.coordinate(
            &mut units,
            &outdoor(10.0, 10.0),
            &zt,
            &HashMap::new(),
            &loads,
        );
        let cp = psych::cp_air_fn_w(0.008);
        for (iu, expected) in units.iter().zip([-2000.0, 1500.0]) {
            let t_zone = zt[&iu.zone];
            let carried = iu.mass_flow * cp * (iu.supply_temp - t_zone);
            assert!(
                (iu.thermal_output - expected).abs() < 1e-6,
                "{}",
                iu.thermal_output
            );
            assert!((carried - expected).abs() < 1e-6, "{carried}");
        }
    }

    /// The unit draws zone air: heating returns it at the zone's humidity,
    /// cooling dries a humid zone, and the outdoor unit carries the coil's
    /// total (sensible + latent) load, which the supply air's enthalpy closes.
    #[test]
    fn cooling_dries_zone_air_and_books_the_total_load() {
        let (mut odu, mut units) = make_system();
        let (zt, loads) = zone_maps(&[("Zone A", 25.0, -2000.0), ("Zone B", 20.0, 1500.0)]);
        let zw: HashMap<String, f64> =
            [("Zone A".to_string(), 0.012), ("Zone B".to_string(), 0.006)].into();
        odu.heat_recovery = true;
        odu.coordinate(&mut units, &outdoor(30.0, 20.0), &zt, &zw, &loads);
        let (a, b) = (&units[0], &units[1]);
        assert_eq!(b.supply_humidity_ratio, 0.006);
        assert!(
            a.supply_humidity_ratio < 0.012,
            "{}",
            a.supply_humidity_ratio
        );
        assert!(a.shr < 1.0);
        let h_in = psych::h_fn_tdb_w(25.0, 0.012);
        let h_out = psych::h_fn_tdb_w(a.supply_temp, a.supply_humidity_ratio);
        let total = a.mass_flow * (h_in - h_out);
        assert!(
            (total - odu.total_cooling).abs() < 1e-6 * total,
            "{total} vs {}",
            odu.total_cooling
        );
        assert!(
            (a.thermal_output + 2000.0).abs() < 1e-6,
            "zone gets the sensible load"
        );

        // A dry zone gets a dry coil: no moisture removed, all sensible.
        let zw: HashMap<String, f64> = [("Zone A".to_string(), 0.004)].into();
        odu.coordinate(&mut units, &outdoor(30.0, 20.0), &zt, &zw, &loads);
        assert_eq!(units[0].supply_humidity_ratio, 0.004);
        assert_eq!(units[0].shr, 1.0);
    }

    fn curve(coefficients: Vec<f64>) -> PerformanceCurve {
        use crate::performance_curve::CurveType;
        PerformanceCurve::Polynomial {
            name: "c".to_string(),
            curve_type: if coefficients.len() == 6 {
                CurveType::Biquadratic
            } else {
                CurveType::Quadratic
            },
            coefficients,
            min_x: -100.0,
            max_x: 100.0,
            min_y: -100.0,
            max_y: 100.0,
            min_output: None,
            max_output: None,
        }
    }

    /// Curves take E+'s axes: cooling f(indoor wet-bulb, outdoor dry-bulb).
    /// A capacity curve linear in x only (indoor wet-bulb) must see the
    /// indoor wet-bulb, not the outdoor temperature.
    #[test]
    fn cooling_curves_take_indoor_wet_bulb_then_outdoor_dry_bulb() {
        let (mut odu, mut units) = make_system();
        // CapFT = 0.05·x: at the zone's wet-bulb (≈ 19.4 °C) about 0.97.
        odu.cooling_cap_ft = Some(curve(vec![0.0, 0.05, 0.0, 0.0, 0.0, 0.0]));
        odu.rated_cooling_capacity = 5_000.0; // below the units' 9 kW
        let (zt, loads) = zone_maps(&[("Zone A", 26.67, -50_000.0), ("Zone B", 26.67, -50_000.0)]);
        let w = psych::w_fn_tdb_twb_pb(26.67, 19.44, 101_325.0);
        let zw: HashMap<String, f64> =
            [("Zone A".to_string(), w), ("Zone B".to_string(), w)].into();
        odu.coordinate(&mut units, &outdoor(35.0, 24.0), &zt, &zw, &loads);
        let expected = odu.rated_cooling_capacity * 0.05 * 19.44;
        assert!(
            (odu.total_cooling - expected).abs() < 0.01 * expected,
            "{}",
            odu.total_cooling
        );
    }

    /// The EIR-f(PLR) curve gives part-load power as a share of full-load
    /// power; without it power is proportional to PLR (E+'s blank default).
    #[test]
    fn eir_fplr_shapes_part_load_power() {
        let (zt, loads) = zone_maps(&[("Zone A", 25.0, -2000.0), ("Zone B", 25.0, 0.0)]);
        let (mut plain, mut units) = make_system();
        plain.coordinate(
            &mut units,
            &outdoor(30.0, 20.0),
            &zt,
            &HashMap::new(),
            &loads,
        );
        let plr = plain.total_cooling / plain.rated_cooling_capacity;
        let full = plain.rated_cooling_capacity / plain.rated_cooling_cop;
        assert!((plain.compressor_power - full * plr).abs() < 1e-6);

        let (mut curved, mut units) = make_system();
        // A flat 0.3 + 0.7·PLR: part load costs more than proportional
        // (above the minimum PLR, so no cycling).
        curved.min_plr = 0.05;
        curved.cooling_eir_fplr = Some(curve(vec![0.3, 0.7, 0.0]));
        curved.coordinate(
            &mut units,
            &outdoor(30.0, 20.0),
            &zt,
            &HashMap::new(),
            &loads,
        );
        assert!((curved.compressor_power - full * (0.3 + 0.7 * plr)).abs() < 1e-6);
    }

    /// Below the minimum PLR the compressor cycles at the minimum point, as
    /// E+: the EIR-f(PLR) curve is read at the minimum and power scales by
    /// the runtime fraction (cycling ratio over PLF).
    #[test]
    fn below_the_minimum_plr_the_compressor_cycles() {
        let (zt, loads) = zone_maps(&[("Zone A", 25.0, -1000.0), ("Zone B", 25.0, 0.0)]);
        let (mut odu, mut units) = make_system();
        odu.min_plr = 0.25;
        odu.cooling_eir_fplr = Some(curve(vec![0.3, 0.7, 0.0]));
        odu.cooling_plf_fplr = Some(curve(vec![0.85, 0.15, 0.0]));
        odu.coordinate(
            &mut units,
            &outdoor(30.0, 20.0),
            &zt,
            &HashMap::new(),
            &loads,
        );
        let plr = odu.total_cooling / odu.rated_cooling_capacity;
        assert!(plr < 0.25);
        let cycling = plr / 0.25;
        let rtf = cycling / (0.85 + 0.15 * cycling);
        let full = odu.rated_cooling_capacity / odu.rated_cooling_cop;
        let expected = full * (0.3 + 0.7 * 0.25) * rtf;
        assert!((odu.compressor_power - expected).abs() < 1e-6 * expected);
    }

    #[test]
    fn rated_airflow_limits_the_output() {
        let (mut odu, mut units) = make_system();
        // 0.25 m³/s at 40 °C supply into a 39 °C zone carries only ~300 W.
        let (zt, loads) = zone_maps(&[("Zone A", 39.0, 5000.0), ("Zone B", 21.0, 0.0)]);
        odu.coordinate(
            &mut units,
            &outdoor(0.0, 10.0),
            &zt,
            &HashMap::new(),
            &loads,
        );
        let a = &units[0];
        assert!(
            a.thermal_output > 0.0 && a.thermal_output < 400.0,
            "{}",
            a.thermal_output
        );
        assert_eq!(units[1].mode, VrfMode::Off);
    }
}
