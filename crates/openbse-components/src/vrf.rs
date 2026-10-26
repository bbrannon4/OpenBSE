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
    /// Supply humidity ratio [kg/kg] — set from outdoor or return air
    #[serde(skip)]
    pub supply_humidity_ratio: f64,
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
        }
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

    /// Cooling capacity modifier f(outdoor_db, avg_indoor_wb) — resolved at build time
    #[serde(skip)]
    pub cooling_cap_ft: Option<PerformanceCurve>,
    /// Cooling EIR modifier f(outdoor_db, avg_indoor_wb)
    #[serde(skip)]
    pub cooling_eir_ft: Option<PerformanceCurve>,
    /// Heating capacity modifier f(outdoor_db, avg_indoor_db)
    #[serde(skip)]
    pub heating_cap_ft: Option<PerformanceCurve>,
    /// Heating EIR modifier f(outdoor_db, avg_indoor_db)
    #[serde(skip)]
    pub heating_eir_ft: Option<PerformanceCurve>,

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
            heating_cap_ft: None,
            heating_eir_ft: None,
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
    /// zone's current temperature, up to its rated airflow.
    pub fn coordinate(
        &mut self,
        indoor_units: &mut [VrfIndoorUnit],
        t_outdoor: f64,
        zone_temps: &HashMap<String, f64>,
        zone_loads: &HashMap<String, f64>,
        outdoor_humidity_ratio: f64,
    ) {
        let air_density = psych::rho_air_fn_pb_tdb_w(101325.0, t_outdoor, outdoor_humidity_ratio);
        let cp_air = psych::cp_air_fn_w(outdoor_humidity_ratio);

        // ── Step 1: Mode and PLR for each indoor unit from its zone's load ─
        for iu in indoor_units.iter_mut() {
            let t_zone = zone_temps.get(&iu.zone).copied().unwrap_or(21.0);
            let load = zone_loads.get(&iu.zone).copied().unwrap_or(0.0);
            // The most the unit's rated airflow can carry at its supply temperature.
            let air_limit = |t_supply: f64| {
                (iu.rated_airflow * air_density * cp_air * (t_supply - t_zone).abs()).max(0.0)
            };
            (iu.mode, iu.plr) = if load > 0.0 && iu.heating_supply_temp > t_zone {
                let q = load
                    .min(iu.heating_capacity)
                    .min(air_limit(iu.heating_supply_temp));
                (VrfMode::Heating, q / iu.heating_capacity.max(1.0))
            } else if load < 0.0 && iu.cooling_supply_temp < t_zone {
                let q = (-load)
                    .min(iu.cooling_capacity)
                    .min(air_limit(iu.cooling_supply_temp));
                (VrfMode::Cooling, q / iu.cooling_capacity.max(1.0))
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
        let avg_indoor_t: f64 = {
            let temps: Vec<f64> = indoor_units
                .iter()
                .filter(|iu| iu.mode != VrfMode::Off)
                .filter_map(|iu| zone_temps.get(&iu.zone).copied())
                .collect();
            if temps.is_empty() {
                21.0
            } else {
                temps.iter().sum::<f64>() / temps.len() as f64
            }
        };

        let cool_cap_mod = if let Some(ref curve) = self.cooling_cap_ft {
            curve.evaluate(t_outdoor, avg_indoor_t)
        } else {
            1.0
        };
        let cool_eir_mod = if let Some(ref curve) = self.cooling_eir_ft {
            curve.evaluate(t_outdoor, avg_indoor_t)
        } else {
            1.0
        };
        let heat_cap_mod = if let Some(ref curve) = self.heating_cap_ft {
            curve.evaluate(t_outdoor, avg_indoor_t)
        } else {
            1.0
        };
        let heat_eir_mod = if let Some(ref curve) = self.heating_eir_ft {
            curve.evaluate(t_outdoor, avg_indoor_t)
        } else {
            1.0
        };

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
        // Heat recovery: cooling rejection covers part of heating load,
        // so compressor only needs to make up the difference.
        self.compressor_power = if self.heat_recovery && final_cool > 0.0 && final_heat > 0.0 {
            // Cooling rejection = final_cool + compressor work for cooling
            // Heat available from rejection = final_cool / COP * (1 + COP) ≈ final_cool * (1 + 1/COP)
            let cool_power = final_cool / self.rated_cooling_cop.max(0.1) * cool_eir_mod;
            let heat_rejection = final_cool + cool_power;
            // Heating gap = max(0, final_heat - heat_rejection)
            let heat_gap = (final_heat - heat_rejection).max(0.0);
            let heat_power = heat_gap / self.rated_heating_cop.max(0.1) * heat_eir_mod;
            cool_power + heat_power
        } else if final_cool > 0.0 {
            final_cool / self.rated_cooling_cop.max(0.1) * cool_eir_mod
        } else if final_heat > 0.0 {
            final_heat / self.rated_heating_cop.max(0.1) * heat_eir_mod
        } else {
            0.0
        };

        // ── Step 9: Set per-unit supply conditions ─────────────────────────
        // The flow is set so that ṁ·cp·(T_supply − T_zone) is the heat booked.
        let total_plr: f64 = indoor_units.iter().map(|iu| iu.plr).sum::<f64>().max(1e-9);

        for iu in indoor_units.iter_mut() {
            let t_zone = zone_temps.get(&iu.zone).copied().unwrap_or(21.0);
            let (t_supply, q) = match iu.mode {
                VrfMode::Off => (t_zone, 0.0),
                VrfMode::Cooling => (iu.cooling_supply_temp, -(iu.plr * iu.cooling_capacity)),
                VrfMode::Heating => (iu.heating_supply_temp, iu.plr * iu.heating_capacity),
            };
            iu.supply_temp = t_supply;
            iu.mass_flow = if q == 0.0 {
                0.0
            } else {
                q / (cp_air * (t_supply - t_zone))
            };
            iu.power = self.compressor_power * (iu.plr / total_plr);
            iu.thermal_output = q;
            iu.supply_humidity_ratio = outdoor_humidity_ratio;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        odu.coordinate(&mut units, 35.0, &zt, &loads, 0.010);
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

        odu_hr.coordinate(&mut units_hr, 25.0, &zt, &loads, 0.010);
        odu_no_hr.coordinate(&mut units_no_hr, 25.0, &zt, &loads, 0.010);

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
        odu.coordinate(&mut units, 35.0, &zt, &loads, 0.010);
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
        odu.coordinate(&mut units, 30.0, &zt, &loads, 0.010);
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
        odu.coordinate(&mut units, 10.0, &zt, &loads, 0.008);
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

    #[test]
    fn rated_airflow_limits_the_output() {
        let (mut odu, mut units) = make_system();
        // 0.25 m³/s at 40 °C supply into a 39 °C zone carries only ~300 W.
        let (zt, loads) = zone_maps(&[("Zone A", 39.0, 5000.0), ("Zone B", 21.0, 0.0)]);
        odu.coordinate(&mut units, 0.0, &zt, &loads, 0.003);
        let a = &units[0];
        assert!(
            a.thermal_output > 0.0 && a.thermal_output < 400.0,
            "{}",
            a.thermal_output
        );
        assert_eq!(units[1].mode, VrfMode::Off);
    }
}
