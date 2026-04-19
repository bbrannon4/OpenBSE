//! Evaporative cooler component.
//!
//! Three modes:
//!   Direct: adiabatic humidification — DBT drops, W rises, enthalpy conserved.
//!   Indirect: sensible cooling via HX using outdoor WBT as driving force, no W change.
//!   TwoStage: indirect followed by direct.
//!
//! Reference: EnergyPlus Engineering Reference, "Evaporative Coolers"

use openbse_core::ports::*;
use openbse_psychrometrics as psych;
use serde::{Deserialize, Serialize};

fn default_submeter() -> String {
    "General".to_string()
}
fn default_evap_effectiveness() -> f64 {
    0.80
}
fn default_hx_effectiveness() -> f64 {
    0.70
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum EvapCoolerMode {
    /// Adiabatic direct: adds humidity, lowers DBT, approximately conserves enthalpy.
    Direct,
    /// Indirect HX: lowers DBT using wet-bulb as driving temperature, no W addition.
    Indirect,
    /// Indirect stage followed by direct stage.
    TwoStage,
}

impl Default for EvapCoolerMode {
    fn default() -> Self {
        EvapCoolerMode::Direct
    }
}

/// Evaporative cooler component (direct, indirect, or two-stage).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvapCooler {
    pub name: String,
    #[serde(default = "default_submeter")]
    pub submeter: String,
    #[serde(default)]
    pub mode: EvapCoolerMode,
    /// Saturation effectiveness [0-1] for direct stage (default 0.80)
    #[serde(default = "default_evap_effectiveness")]
    pub effectiveness: f64,
    /// Heat exchanger effectiveness [0-1] for indirect stage (default 0.70)
    #[serde(default = "default_hx_effectiveness")]
    pub hx_effectiveness: f64,

    // ─── Runtime state ──────────────────────────────────────────────────
    #[serde(skip)]
    pub power: f64, // small pump power [W]
    #[serde(skip)]
    pub thermal_out: f64,
}

impl EvapCooler {
    pub fn new(name: &str, mode: EvapCoolerMode) -> Self {
        Self {
            name: name.to_string(),
            submeter: "General".to_string(),
            mode,
            effectiveness: 0.80,
            hx_effectiveness: 0.70,
            power: 0.0,
            thermal_out: 0.0,
        }
    }

    /// Compute direct-stage outlet: DBT drops, W rises, enthalpy conserved.
    fn direct_stage(t_db: f64, w: f64, p_b: f64, effectiveness: f64) -> (f64, f64) {
        let t_wb = psych::twb_fn_tdb_w_pb(t_db, w, p_b);
        let t_out = t_db - effectiveness * (t_db - t_wb);
        // Enthalpy balance: h_in = h_out → solve W_out
        let h_in = psych::h_fn_tdb_w(t_db, w);
        let w_out = psych::w_fn_tdb_h(t_out, h_in).max(w); // W_out >= W_in
        (t_out, w_out)
    }

    /// Compute indirect-stage outlet: DBT drops toward WBT of inlet, no moisture addition.
    fn indirect_stage(t_db: f64, w: f64, p_b: f64, effectiveness: f64, hx_eff: f64) -> (f64, f64) {
        let t_wb = psych::twb_fn_tdb_w_pb(t_db, w, p_b);
        let t_out = t_db - effectiveness * hx_eff * (t_db - t_wb);
        (t_out, w) // humidity unchanged
    }
}

impl AirComponent for EvapCooler {
    fn name(&self) -> &str {
        &self.name
    }

    fn component_kind(&self) -> ComponentKind {
        ComponentKind::EvapCooler
    }

    fn simulate_air(&mut self, inlet: &AirPort, _ctx: &SimulationContext) -> AirPort {
        if inlet.mass_flow <= 0.0 {
            self.power = 0.0;
            self.thermal_out = 0.0;
            return *inlet;
        }

        let t_db = inlet.state.t_db;
        let w = inlet.state.w;
        let p_b = inlet.state.p_b;

        let (t_out, w_out) = match self.mode {
            EvapCoolerMode::Direct => Self::direct_stage(t_db, w, p_b, self.effectiveness),
            EvapCoolerMode::Indirect => {
                Self::indirect_stage(t_db, w, p_b, self.effectiveness, self.hx_effectiveness)
            }
            EvapCoolerMode::TwoStage => {
                // Stage 1: indirect
                let (t_int, w_int) =
                    Self::indirect_stage(t_db, w, p_b, self.effectiveness, self.hx_effectiveness);
                // Stage 2: direct on intermediate state
                Self::direct_stage(t_int, w_int, p_b, self.effectiveness)
            }
        };

        let h_in = psych::h_fn_tdb_w(t_db, w);
        let h_out = psych::h_fn_tdb_w(t_out, w_out);
        // Thermal output: negative = cooling (removes enthalpy from air)
        self.thermal_out = inlet.mass_flow * (h_out - h_in);
        self.power = 100.0; // small recirculation pump

        AirPort::new(
            openbse_psychrometrics::MoistAirState::new(t_out, w_out, p_b),
            inlet.mass_flow,
        )
    }

    fn power_consumption(&self) -> f64 {
        self.power
    }

    fn thermal_output(&self) -> f64 {
        self.thermal_out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_relative_eq;
    use openbse_core::types::{DayType, TimeStep};
    use openbse_psychrometrics::{MoistAirState, STD_PRESSURE};

    fn make_ctx() -> SimulationContext {
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
            outdoor_air: MoistAirState::from_tdb_rh(35.0, 0.20, STD_PRESSURE),
            day_type: DayType::WeatherDay,
            is_sizing: false,
            sizing_internal_gains: SizingInternalGains::Full,
        }
    }

    fn make_inlet(t_db: f64, rh: f64) -> AirPort {
        AirPort::new(MoistAirState::from_tdb_rh(t_db, rh, STD_PRESSURE), 1.0)
    }

    #[test]
    fn test_direct_cools_and_humidifies() {
        let mut ec = EvapCooler::new("EC1", EvapCoolerMode::Direct);
        let inlet = make_inlet(35.0, 0.15);
        let ctx = make_ctx();
        let outlet = ec.simulate_air(&inlet, &ctx);

        // DBT drops
        assert!(
            outlet.state.t_db < inlet.state.t_db,
            "direct: DBT should drop"
        );
        // Humidity rises
        assert!(outlet.state.w > inlet.state.w, "direct: W should rise");
        // Enthalpy approximately conserved (within 1%)
        let h_in = psych::h_fn_tdb_w(inlet.state.t_db, inlet.state.w);
        let h_out = psych::h_fn_tdb_w(outlet.state.t_db, outlet.state.w);
        assert_relative_eq!(h_in, h_out, max_relative = 0.01);
    }

    #[test]
    fn test_indirect_cools_no_humidity_change() {
        let mut ec = EvapCooler::new("EC2", EvapCoolerMode::Indirect);
        let inlet = make_inlet(35.0, 0.15);
        let ctx = make_ctx();
        let outlet = ec.simulate_air(&inlet, &ctx);

        assert!(
            outlet.state.t_db < inlet.state.t_db,
            "indirect: DBT should drop"
        );
        assert_relative_eq!(outlet.state.w, inlet.state.w, max_relative = 0.001);
    }

    #[test]
    fn test_two_stage_cools_more_than_either_alone() {
        let inlet = make_inlet(35.0, 0.15);
        let ctx = make_ctx();

        let mut direct = EvapCooler::new("D", EvapCoolerMode::Direct);
        let outlet_d = direct.simulate_air(&inlet, &ctx);

        let mut indirect = EvapCooler::new("I", EvapCoolerMode::Indirect);
        let outlet_i = indirect.simulate_air(&inlet, &ctx);

        let mut two = EvapCooler::new("2S", EvapCoolerMode::TwoStage);
        let outlet_2 = two.simulate_air(&inlet, &ctx);

        assert!(
            outlet_2.state.t_db < outlet_d.state.t_db,
            "two-stage should be cooler than direct alone"
        );
        assert!(
            outlet_2.state.t_db < outlet_i.state.t_db,
            "two-stage should be cooler than indirect alone"
        );
    }

    #[test]
    fn test_zero_flow_passthrough() {
        let mut ec = EvapCooler::new("EC3", EvapCoolerMode::Direct);
        let inlet = AirPort::new(MoistAirState::from_tdb_rh(30.0, 0.50, STD_PRESSURE), 0.0);
        let ctx = make_ctx();
        let outlet = ec.simulate_air(&inlet, &ctx);
        assert_relative_eq!(outlet.state.t_db, inlet.state.t_db, max_relative = 0.001);
        assert_eq!(ec.power, 0.0);
    }
}
