//! Water-source heat pump (WSHP) component.
//!
//! Models a water-source heat pump that exchanges heat with a condenser water
//! loop rather than outdoor air. Both heating and cooling modes are supported.
//!
//! Physics:
//!   Cooling: Q_supply = capacity × cap_factor; W = Q_supply / COP; Q_rejected = Q_supply + W
//!   Heating: Q_supply = capacity × cap_factor; W = Q_supply / COP; Q_absorbed = Q_supply - W
//!
//! Capacity derates linearly with leaving water temperature (LWT):
//!   cap_factor = 1 - k_cap × (lwt - lwt_rated)
//!
//! Reference: EnergyPlus Engineering Reference, "HeatPump:WaterToAir:EquationFit"

use openbse_core::ports::*;
use openbse_psychrometrics as psych;
use serde::{Deserialize, Serialize};

fn default_submeter() -> String {
    "General".to_string()
}

fn default_wshp_cop_cooling() -> f64 {
    4.5
}
fn default_wshp_cop_heating() -> f64 {
    4.0
}
fn default_wshp_lwt_rated() -> f64 {
    30.0 // °C, rated condenser leaving water temp (cooling)
}

/// Water-source heat pump.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WaterSourceHeatPump {
    pub name: String,
    #[serde(default = "default_submeter")]
    pub submeter: String,
    /// Rated cooling capacity [W] at rated condenser water temp
    pub rated_cooling_capacity: f64,
    /// Rated heating capacity [W] at rated condenser water temp
    pub rated_heating_capacity: f64,
    /// Rated COP in cooling mode
    #[serde(default = "default_wshp_cop_cooling")]
    pub cop_cooling: f64,
    /// Rated COP in heating mode
    #[serde(default = "default_wshp_cop_heating")]
    pub cop_heating: f64,
    /// Rated leaving water temperature [°C] for capacity curves
    #[serde(default = "default_wshp_lwt_rated")]
    pub lwt_rated: f64,
    /// Outlet air temperature setpoint [°C] (cooling) or supply temp (heating)
    pub outlet_temp_setpoint: f64,

    // ─── Runtime state ──────────────────────────────────────────────────
    /// Electric power consumed this timestep [W]
    #[serde(skip)]
    pub power: f64,
    /// Total cooling or heating delivered to air [W] (positive = energy to air)
    #[serde(skip)]
    pub air_thermal_output: f64,
    /// Heat rejected to (cooling) or absorbed from (heating) water loop [W]
    #[serde(skip)]
    pub water_heat_exchange: f64,
    /// Current operating mode
    #[serde(skip)]
    pub mode: WshpMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub enum WshpMode {
    #[default]
    Off,
    Cooling,
    Heating,
}

impl WaterSourceHeatPump {
    pub fn new(
        name: &str,
        rated_cooling_capacity: f64,
        rated_heating_capacity: f64,
        cop_cooling: f64,
        cop_heating: f64,
        setpoint: f64,
    ) -> Self {
        Self {
            name: name.to_string(),
            submeter: "General".to_string(),
            rated_cooling_capacity,
            rated_heating_capacity,
            cop_cooling,
            cop_heating,
            lwt_rated: 30.0,
            outlet_temp_setpoint: setpoint,
            power: 0.0,
            air_thermal_output: 0.0,
            water_heat_exchange: 0.0,
            mode: WshpMode::Off,
        }
    }

    /// Heat rejected to condenser water [W] (positive = heat into water).
    /// Cooling: reject compressor work + absorbed air heat.
    /// Heating: absorb heat from water (negative = water loses heat).
    pub fn water_heat_rejection(&self) -> f64 {
        self.water_heat_exchange
    }
}

impl AirComponent for WaterSourceHeatPump {
    fn name(&self) -> &str {
        &self.name
    }

    fn component_kind(&self) -> ComponentKind {
        ComponentKind::CoolingCoil
    }

    fn simulate_air(&mut self, inlet: &AirPort, ctx: &SimulationContext) -> AirPort {
        if inlet.mass_flow <= 0.0 {
            self.power = 0.0;
            self.air_thermal_output = 0.0;
            self.water_heat_exchange = 0.0;
            self.mode = WshpMode::Off;
            return *inlet;
        }

        let cp = psych::cp_air_fn_w(inlet.state.w);
        let t_in = inlet.state.t_db;
        let t_sp = self.outlet_temp_setpoint;

        // Determine mode from setpoint vs inlet temp
        if t_sp >= t_in - 0.1 {
            // No cooling needed (setpoint above or near inlet) — check heating
            if t_sp > t_in + 0.1 {
                // Heating mode
                let cap_needed = inlet.mass_flow * cp * (t_sp - t_in);
                let cap_available = self.rated_heating_capacity;
                let cap_actual = cap_needed.min(cap_available);
                let t_out = t_in + cap_actual / (inlet.mass_flow * cp).max(1e-6);
                self.power = cap_actual / self.cop_heating.max(0.1);
                self.air_thermal_output = cap_actual;
                // Water absorbs heat to drive heat pump; heat absorbed = supply - compressor work
                self.water_heat_exchange = -(cap_actual - self.power);
                self.mode = WshpMode::Heating;
                return AirPort::new(
                    psych::MoistAirState::new(t_out, inlet.state.w, inlet.state.p_b),
                    inlet.mass_flow,
                );
            } else {
                self.power = 0.0;
                self.air_thermal_output = 0.0;
                self.water_heat_exchange = 0.0;
                self.mode = WshpMode::Off;
                return *inlet;
            }
        }

        // Cooling mode
        // Condenser water temp from outdoor_air temp (proxy; actual loop temp not tracked here)
        let lwt = ctx.outdoor_air.t_db.clamp(15.0, 45.0);
        let cap_factor = (1.0 - 0.015 * (lwt - self.lwt_rated)).clamp(0.5, 1.2);
        let cap_available = self.rated_cooling_capacity * cap_factor;
        let cap_needed = inlet.mass_flow * cp * (t_in - t_sp);
        let cap_actual = cap_needed.min(cap_available).max(0.0);
        let t_out = t_in - cap_actual / (inlet.mass_flow * cp).max(1e-6);
        self.power = cap_actual / self.cop_cooling.max(0.1);
        self.air_thermal_output = -cap_actual; // negative = cooling air
                                               // Water receives compressor work + absorbed air heat
        self.water_heat_exchange = cap_actual + self.power;
        self.mode = WshpMode::Cooling;

        AirPort::new(
            psych::MoistAirState::new(t_out, inlet.state.w, inlet.state.p_b),
            inlet.mass_flow,
        )
    }

    fn set_setpoint(&mut self, setpoint: f64) {
        self.outlet_temp_setpoint = setpoint;
    }

    fn power_consumption(&self) -> f64 {
        self.power
    }

    fn thermal_output(&self) -> f64 {
        self.air_thermal_output
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_relative_eq;
    use openbse_core::ports::SizingInternalGains;
    use openbse_core::types::{DayType, TimeStep};
    use openbse_psychrometrics::MoistAirState;

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
            outdoor_air: MoistAirState::from_tdb_rh(t_outdoor, 0.40, 101325.0),
            day_type: DayType::WeatherDay,
            is_sizing: false,
            sizing_internal_gains: SizingInternalGains::Full,
        }
    }

    fn make_wshp() -> WaterSourceHeatPump {
        WaterSourceHeatPump::new("WSHP-1", 10_000.0, 9_000.0, 4.5, 4.0, 13.0)
    }

    #[test]
    fn test_cooling_reduces_air_temp() {
        let mut wshp = make_wshp();
        let inlet = AirPort::new(MoistAirState::from_tdb_rh(26.0, 0.5, 101325.0), 0.5);
        let ctx = make_ctx(30.0);
        let outlet = wshp.simulate_air(&inlet, &ctx);
        assert!(
            outlet.state.t_db < inlet.state.t_db,
            "Cooling must reduce air temp"
        );
        assert!(wshp.power > 0.0, "Cooling consumes power");
        assert_eq!(wshp.mode, WshpMode::Cooling);
    }

    #[test]
    fn test_heat_rejection_conservation() {
        // In cooling mode: Q_rejected = Q_cooling + W_compressor
        let mut wshp = make_wshp();
        let inlet = AirPort::new(MoistAirState::from_tdb_rh(26.0, 0.5, 101325.0), 0.5);
        let ctx = make_ctx(30.0);
        wshp.simulate_air(&inlet, &ctx);
        let q_cooling = -wshp.air_thermal_output; // positive cooling load
        let expected_rejection = q_cooling + wshp.power;
        assert_relative_eq!(
            wshp.water_heat_exchange,
            expected_rejection,
            max_relative = 0.001
        );
    }

    #[test]
    fn test_heating_raises_air_temp() {
        let mut wshp = WaterSourceHeatPump::new("WSHP-H", 10_000.0, 9_000.0, 4.5, 4.0, 40.0);
        let inlet = AirPort::new(MoistAirState::from_tdb_rh(20.0, 0.4, 101325.0), 0.3);
        let ctx = make_ctx(20.0);
        let outlet = wshp.simulate_air(&inlet, &ctx);
        assert!(
            outlet.state.t_db > inlet.state.t_db,
            "Heating must raise air temp"
        );
        assert_eq!(wshp.mode, WshpMode::Heating);
        assert!(
            wshp.water_heat_exchange < 0.0,
            "Heating absorbs heat from water loop"
        );
    }

    #[test]
    fn test_zero_flow_no_operation() {
        let mut wshp = make_wshp();
        let inlet = AirPort::new(MoistAirState::from_tdb_rh(26.0, 0.5, 101325.0), 0.0);
        let ctx = make_ctx(30.0);
        wshp.simulate_air(&inlet, &ctx);
        assert_eq!(wshp.power, 0.0);
        assert_eq!(wshp.mode, WshpMode::Off);
    }
}
