//! DX cooling coil driven by an ASHRAE Standard 205 RS0004 performance map.
//!
//! Plays the role of [`crate::cooling_coil::CoolingCoilDX`] but reads
//! capacity / sensible / power directly from a tabulated 6-axis grid
//! rather than evaluating polynomial curves.  All thermodynamic decoupling
//! (PLR derivation, cycling degradation, latent split into outlet humidity)
//! is identical in spirit to the polynomial coil; the only difference is
//! that the performance source is a manufacturer-supplied data file.
//!
//! ### Operating logic
//!
//! 1. The upstream control loop sets `outlet_temp_setpoint`.
//! 2. Each timestep, compute the sensible cooling required to drive the
//!    inlet down to that setpoint.
//! 3. Look up the full-sequence sensible capacity at current conditions
//!    (outdoor DB, indoor DB, indoor RH, indoor mass flow, ambient pressure).
//! 4. `PLR = required_sensible / full_sensible_capacity`, clamped to [0, 1].
//! 5. Map PLR → compressor sequence (continuous or discrete-with-cycling),
//!    re-query the map at that sequence.
//! 6. Apply cycling degradation per the file's
//!    `cycling_degradation_coefficient` (E+ convention: PLF = 1 − Cd·(1−RTF)).
//! 7. Split delivered total capacity into sensible (lowers T_db) and
//!    latent (lowers W) using the map's sensible/total ratio.
//!
//! ### Limitations vs the polynomial coil
//!
//! - "Gross" capacity from the file means *before* fan effects.  The
//!   upstream fan in the same air loop must therefore not subtract fan
//!   heat from coil capacity again — same accounting as the polynomial
//!   coil with its `rated_capacity` already gross-of-fan.

use openbse_a205::rs0004::{DxCoolingInterpolator, DxCoolingQuery, Rs0004};
use openbse_core::ports::*;
use openbse_psychrometrics::{self as psych};

/// Convert °C → K
#[inline]
fn c_to_k(c: f64) -> f64 {
    c + 273.15
}

/// Approximate latent heat of vaporization of water at typical coil
/// operating temperatures [J/kg].  Same constant used elsewhere in the
/// codebase (zone moisture balance) so that mass/energy balances close.
const H_FG: f64 = 2_501_000.0;

fn default_submeter() -> String {
    "General".to_string()
}

pub struct CoolingCoilDXA205 {
    pub name: String,
    pub submeter: String,
    pub outlet_temp_setpoint: f64,
    /// Lower bound on PLR for cycling logic (matches existing DX coil).
    pub min_plr: f64,

    rs0004: Rs0004,
    interp: DxCoolingInterpolator,
    /// Nominal capacity [W] precomputed at AHRI-style design conditions
    /// for sizing and PLR-based staging.
    nominal_capacity_w: f64,
    /// Design air mass flow used when querying the map.  When zero, the
    /// inlet mass flow is used as-is.
    pub design_mass_flow: f64,

    // runtime state
    sensible_cooling_rate: f64,
    cooling_rate: f64,
    power_consumption: f64,
    plr: f64,
    sequence_number: f64,
    in_range: bool,
}

impl std::fmt::Debug for CoolingCoilDXA205 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CoolingCoilDXA205")
            .field("name", &self.name)
            .field("outlet_temp_setpoint", &self.outlet_temp_setpoint)
            .field("nominal_capacity_w", &self.nominal_capacity_w)
            .finish()
    }
}

impl CoolingCoilDXA205 {
    pub fn from_rs0004(
        name: impl Into<String>,
        rs0004: Rs0004,
        outlet_temp_setpoint: f64,
    ) -> Result<Self, openbse_a205::A205Error> {
        let interp = DxCoolingInterpolator::new(&rs0004.performance.performance_map_cooling)?;
        // Precompute nominal capacity at AHRI 210/240 cooling-rated conditions:
        //   Outdoor DB 35°C, Indoor DB 26.7°C / WB 19.4°C (≈ RH ~0.5), sea level.
        //   Pick the file's full-load compressor sequence (max).
        // Use the file's middle mass-flow as a proxy for design flow.
        let g = &rs0004.performance.performance_map_cooling.grid_variables;
        let design_mass_flow = if g.indoor_coil_air_mass_flow_rate.len() > 1 {
            let mid = g.indoor_coil_air_mass_flow_rate.len() / 2;
            g.indoor_coil_air_mass_flow_rate[mid]
        } else {
            g.indoor_coil_air_mass_flow_rate[0]
        };
        let q = DxCoolingQuery {
            outdoor_db_k: c_to_k(35.0),
            indoor_rh: 0.5,
            indoor_db_k: c_to_k(26.7),
            indoor_mass_flow_kg_s: design_mass_flow,
            compressor_sequence: interp.sequence_range.1,
            ambient_pressure_pa: 101_325.0,
        };
        let r = interp.query(&q)?;
        let nominal_capacity_w = r.gross_total_capacity;
        Ok(Self {
            name: name.into(),
            submeter: default_submeter(),
            outlet_temp_setpoint,
            min_plr: 0.10,
            rs0004,
            interp,
            nominal_capacity_w,
            design_mass_flow,
            sensible_cooling_rate: 0.0,
            cooling_rate: 0.0,
            power_consumption: 0.0,
            plr: 0.0,
            sequence_number: 0.0,
            in_range: true,
        })
    }

    fn make_query(
        &self,
        inlet: &AirPort,
        ctx: &SimulationContext,
        sequence: f64,
    ) -> DxCoolingQuery {
        let rh = psych::rh_fn_tdb_w_pb(inlet.state.t_db, inlet.state.w, inlet.state.p_b);
        DxCoolingQuery {
            outdoor_db_k: c_to_k(ctx.outdoor_air.t_db),
            indoor_rh: rh.clamp(0.0, 1.0),
            indoor_db_k: c_to_k(inlet.state.t_db),
            indoor_mass_flow_kg_s: inlet.mass_flow,
            compressor_sequence: sequence,
            ambient_pressure_pa: inlet.state.p_b,
        }
    }
}

impl AirComponent for CoolingCoilDXA205 {
    fn name(&self) -> &str {
        &self.name
    }

    fn component_kind(&self) -> ComponentKind {
        ComponentKind::CoolingCoil
    }

    fn nominal_capacity(&self) -> Option<f64> {
        Some(self.nominal_capacity_w)
    }

    fn power_consumption(&self) -> f64 {
        self.power_consumption
    }

    fn thermal_output(&self) -> f64 {
        // Negative = heat removed
        -self.cooling_rate
    }

    fn set_setpoint(&mut self, setpoint: f64) {
        self.outlet_temp_setpoint = setpoint;
    }

    fn setpoint(&self) -> Option<f64> {
        Some(self.outlet_temp_setpoint)
    }

    fn simulate_air(&mut self, inlet: &AirPort, ctx: &SimulationContext) -> AirPort {
        if inlet.mass_flow <= 0.0 {
            self.cooling_rate = 0.0;
            self.sensible_cooling_rate = 0.0;
            self.power_consumption = 0.0;
            self.plr = 0.0;
            self.sequence_number = 0.0;
            return *inlet;
        }
        let cp_air = psych::cp_air_fn_w(inlet.state.w);
        let q_sensible_required =
            inlet.mass_flow * cp_air * (inlet.state.t_db - self.outlet_temp_setpoint);
        if q_sensible_required <= 0.0 {
            self.cooling_rate = 0.0;
            self.sensible_cooling_rate = 0.0;
            self.power_consumption = 0.0;
            self.plr = 0.0;
            self.sequence_number = 0.0;
            return *inlet;
        }

        // 1) Full-load sensible capacity at current conditions
        let seq_max = self.interp.sequence_range.1;
        let seq_min = self.interp.sequence_range.0;
        let q_full = self.make_query(inlet, ctx, seq_max);
        let r_full = match self.interp.query(&q_full) {
            Ok(r) => r,
            Err(e) => {
                log::warn!("CoolingCoilDXA205 '{}' map query failed: {}", self.name, e);
                self.cooling_rate = 0.0;
                self.sensible_cooling_rate = 0.0;
                self.power_consumption = 0.0;
                return *inlet;
            }
        };
        let full_sensible = r_full.gross_sensible_capacity.max(1.0);

        // 2) PLR (use sensible load vs sensible capacity, as in the
        // polynomial coil's wet-mode path)
        let plr_raw = (q_sensible_required / full_sensible).clamp(0.0, 1.0);

        // 3) Map PLR → sequence; resolve cycling below min step
        let (sequence, cycling_ratio) = {
            let cont_seq = seq_min + plr_raw * (seq_max - seq_min);
            match self.rs0004.performance.compressor_speed_control_type {
                openbse_a205::rs0001::SpeedControl::Continuous => {
                    if cont_seq < seq_min {
                        let cyc = plr_raw / self.min_plr.max(1e-6);
                        (seq_min, cyc.min(1.0))
                    } else {
                        (cont_seq, 1.0)
                    }
                }
                openbse_a205::rs0001::SpeedControl::Discrete => {
                    let snapped = cont_seq.round().clamp(seq_min, seq_max);
                    if cont_seq < seq_min {
                        let cyc = plr_raw / self.min_plr.max(1e-6);
                        (seq_min, cyc.min(1.0))
                    } else {
                        (snapped, 1.0)
                    }
                }
            }
        };

        // 4) Re-query at the operating sequence
        let q_op = self.make_query(inlet, ctx, sequence);
        let r_op = self.interp.query(&q_op).unwrap_or(r_full);
        self.sequence_number = sequence;
        self.in_range = r_op.in_range;

        // 5) Apply cycling degradation
        let cd = self.rs0004.performance.cycling_degradation_coefficient;
        let (eff_total, eff_sensible, eff_power) = if cycling_ratio < 1.0 {
            let plf = (1.0 - cd * (1.0 - cycling_ratio)).max(0.01);
            let rtf = (cycling_ratio / plf).min(1.0);
            (
                r_op.gross_total_capacity * cycling_ratio,
                r_op.gross_sensible_capacity * cycling_ratio,
                r_op.gross_power * rtf,
            )
        } else {
            (
                r_op.gross_total_capacity,
                r_op.gross_sensible_capacity,
                r_op.gross_power,
            )
        };

        // Don't over-deliver: cap delivered sensible at what's required
        let delivered_sensible = eff_sensible.min(q_sensible_required);
        // Pro-rate total cooling by sensible delivery ratio
        let delivered_total = if eff_sensible > 0.0 {
            eff_total * (delivered_sensible / eff_sensible)
        } else {
            0.0
        };

        self.sensible_cooling_rate = delivered_sensible;
        self.cooling_rate = delivered_total;
        self.power_consumption = eff_power
            * if eff_sensible > 0.0 {
                delivered_sensible / eff_sensible
            } else {
                0.0
            };
        self.plr = plr_raw;

        // 6) Compute outlet state
        let dt = delivered_sensible / (inlet.mass_flow * cp_air);
        let out_t = (inlet.state.t_db - dt).max(self.outlet_temp_setpoint - 0.1);
        let q_latent = (delivered_total - delivered_sensible).max(0.0);
        let dw = q_latent / (inlet.mass_flow * H_FG);
        let out_w = (inlet.state.w - dw).max(1.0e-5);

        AirPort::new(
            psych::MoistAirState::new(out_t, out_w, inlet.state.p_b),
            inlet.mass_flow,
        )
    }

    fn report_outputs(&self, out: &mut dyn FnMut(&str, f64)) {
        out("plr", self.plr);
        out("compressor_sequence", self.sequence_number);
        out("in_range", if self.in_range { 1.0 } else { 0.0 });
        out("sensible_cooling_rate", self.sensible_cooling_rate);
        let latent = (self.cooling_rate - self.sensible_cooling_rate).max(0.0);
        out("latent_cooling_rate", latent);
        let cop = if self.power_consumption > 0.0 {
            self.cooling_rate / self.power_consumption
        } else {
            0.0
        };
        out("efficiency_operating", cop);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openbse_core::types::{DayType, TimeStep};
    use openbse_psychrometrics::MoistAirState;
    use std::path::PathBuf;

    fn example_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("openbse-a205")
            .join("examples")
            .join("RS0004_Residential.RS0004.a205.json")
    }

    fn make_ctx(t_outdoor: f64) -> SimulationContext {
        SimulationContext {
            timestep: TimeStep {
                month: 7,
                day: 15,
                hour: 14,
                sub_hour: 1,
                timesteps_per_hour: 1,
                sim_time_s: 0.0,
                dt: 3600.0,
            },
            outdoor_air: MoistAirState::from_tdb_rh(t_outdoor, 0.4, 101325.0),
            day_type: DayType::WeatherDay,
            is_sizing: false,
            sizing_internal_gains: SizingInternalGains::Full,
        }
    }

    #[test]
    fn loads_and_sizes() {
        let rs = Rs0004::load(&example_path()).unwrap();
        let coil = CoolingCoilDXA205::from_rs0004("test", rs, 13.0).unwrap();
        // Residential file: ~36 kBtu/h ≈ 10.7 kW. Nominal cap should be in the
        // expected ballpark.
        assert!(
            coil.nominal_capacity_w > 5_000.0,
            "nominal too low: {}",
            coil.nominal_capacity_w
        );
        assert!(
            coil.nominal_capacity_w < 50_000.0,
            "nominal too high: {}",
            coil.nominal_capacity_w
        );
    }

    #[test]
    fn cools_air_to_setpoint() {
        let rs = Rs0004::load(&example_path()).unwrap();
        let mut coil = CoolingCoilDXA205::from_rs0004("test", rs, 13.0).unwrap();
        // Hot, humid inlet, design-ish mass flow
        let inlet = AirPort::new(
            MoistAirState::from_tdb_rh(26.7, 0.5, 101325.0),
            coil.design_mass_flow,
        );
        let ctx = make_ctx(35.0);
        let outlet = coil.simulate_air(&inlet, &ctx);
        // Should cool the air
        assert!(outlet.state.t_db < inlet.state.t_db);
        // At least some power consumed
        assert!(coil.power_consumption > 0.0);
        // Should dehumidify (latent cooling at 50% RH)
        assert!(outlet.state.w <= inlet.state.w);
        // Reasonable operating COP
        let cop = coil.cooling_rate / coil.power_consumption;
        assert!(cop > 1.5, "COP too low: {}", cop);
        assert!(cop < 8.0, "COP too high: {}", cop);
    }

    #[test]
    fn zero_flow_idle() {
        let rs = Rs0004::load(&example_path()).unwrap();
        let mut coil = CoolingCoilDXA205::from_rs0004("test", rs, 13.0).unwrap();
        let inlet = AirPort::new(MoistAirState::from_tdb_rh(26.7, 0.5, 101325.0), 0.0);
        let ctx = make_ctx(35.0);
        let outlet = coil.simulate_air(&inlet, &ctx);
        assert_eq!(coil.power_consumption, 0.0);
        assert_eq!(coil.cooling_rate, 0.0);
        assert_eq!(outlet.state.t_db, inlet.state.t_db);
    }

    #[test]
    fn does_not_heat_when_inlet_below_setpoint() {
        let rs = Rs0004::load(&example_path()).unwrap();
        let mut coil = CoolingCoilDXA205::from_rs0004("test", rs, 13.0).unwrap();
        let inlet = AirPort::new(
            MoistAirState::from_tdb_rh(10.0, 0.5, 101325.0),
            coil.design_mass_flow,
        );
        let ctx = make_ctx(20.0);
        let outlet = coil.simulate_air(&inlet, &ctx);
        assert_eq!(coil.power_consumption, 0.0);
        assert_eq!(coil.cooling_rate, 0.0);
        assert_eq!(outlet.state.t_db, inlet.state.t_db);
    }
}
