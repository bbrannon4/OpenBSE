//! Variable speed pump component for plant loops.
//!
//! Models a centrifugal pump with variable speed drive that adjusts
//! speed to match the required flow demand from the plant loop.
//!
//! Power curve (affinity laws):
//!   P = P_design * (Q/Q_design)^n  where n ~ 2.5-3.0
//!
//! Design power calculation:
//!   P_design = (Q_design * H_design) / eta_motor
//!
//! where Q = volumetric flow [m^3/s], H = head [Pa], eta = motor efficiency.
//!
//! The pump adds a small amount of heat to the fluid equal to the fraction
//! of motor losses dissipated into the fluid stream.
//!
//! Reference: EnergyPlus Engineering Reference, "Pumps"

use openbse_core::ports::*;
use openbse_core::types::*;
use openbse_psychrometrics::FluidState;
use serde::{Deserialize, Serialize};

/// Pump type enumeration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum PumpType {
    /// Constant speed: always runs at design power when on.
    ConstantSpeed,
    /// Variable speed: power follows affinity laws with flow fraction.
    VariableSpeed,
}

/// Variable or constant speed pump component for plant loops.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pump {
    pub name: String,
    pub pump_type: PumpType,
    /// Design (maximum) volumetric flow rate [m^3/s]
    pub design_flow_rate: f64,
    /// Design head [Pa] (typically 150,000-300,000 Pa / 50-100 ft H2O)
    pub design_head: f64,
    /// Design (rated) motor efficiency [0-1]
    pub motor_efficiency: f64,
    /// Pump curve exponent for affinity laws (default 3.0)
    pub curve_exponent: f64,
    /// Minimum flow fraction for variable speed pump [0-1] (default 0.1)
    pub min_flow_fraction: f64,
    /// Fraction of motor heat going to fluid [0-1] (default 1.0)
    pub motor_heat_to_fluid_fraction: f64,

    // ─── Runtime state ──────────────────────────────────────────────────
    /// Electric power consumption this timestep [W]
    #[serde(skip)]
    pub power: f64,
    /// Pump heat added to the water stream this timestep [W]
    #[serde(skip)]
    pub heat_to_fluid: f64,
}

impl Pump {
    /// Create a new pump with typical defaults.
    pub fn new(
        name: &str,
        pump_type: PumpType,
        design_flow_rate: f64,
        design_head: f64,
        motor_efficiency: f64,
    ) -> Self {
        Self {
            name: name.to_string(),
            pump_type,
            design_flow_rate,
            design_head,
            motor_efficiency,
            curve_exponent: 3.0,
            min_flow_fraction: 0.1,
            motor_heat_to_fluid_fraction: 1.0,
            power: 0.0,
            heat_to_fluid: 0.0,
        }
    }

    /// Design (rated) power [W] = Q_design * H_design / eta_motor.
    pub fn design_power(&self) -> f64 {
        self.design_flow_rate * self.design_head / self.motor_efficiency
    }
}

impl PlantComponent for Pump {
    fn name(&self) -> &str {
        &self.name
    }

    fn simulate_plant(
        &mut self,
        inlet: &WaterPort,
        load: f64,
        _ctx: &SimulationContext,
    ) -> WaterPort {
        // No load or no flow: pump is off
        if load <= 0.0 || inlet.state.mass_flow <= 0.0 {
            self.power = 0.0;
            self.heat_to_fluid = 0.0;
            return *inlet;
        }

        let design_power = self.design_power();

        // Calculate power based on pump type
        self.power = match self.pump_type {
            PumpType::ConstantSpeed => {
                // Constant speed: always runs at full design power when on
                design_power
            }
            PumpType::VariableSpeed => {
                // Part load ratio based on actual vs design flow
                // Use mass flow as proxy for volumetric flow (density ~constant)
                let design_mass_flow =
                    self.design_flow_rate * openbse_psychrometrics::RHO_WATER;
                let plr = (inlet.state.mass_flow / design_mass_flow)
                    .clamp(self.min_flow_fraction, 1.0);
                // Affinity laws: P = P_design * PLR^n
                design_power * plr.powf(self.curve_exponent)
            }
        };

        // Heat added to fluid from pump motor losses
        self.heat_to_fluid = self.power * self.motor_heat_to_fluid_fraction;

        // Calculate small temperature rise from pump heat
        let delta_t = self.heat_to_fluid / (inlet.state.mass_flow * inlet.state.cp);
        let outlet_temp = inlet.state.temp + delta_t;

        WaterPort::new(FluidState::water(outlet_temp, inlet.state.mass_flow))
    }

    fn design_water_flow_rate(&self) -> Option<f64> {
        if is_autosize(self.design_flow_rate) {
            None
        } else {
            Some(self.design_flow_rate)
        }
    }

    fn set_design_water_flow_rate(&mut self, flow: f64) {
        self.design_flow_rate = flow;
    }

    fn power_consumption(&self) -> f64 {
        self.power
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_relative_eq;
    use openbse_core::types::{DayType, TimeStep};
    use openbse_psychrometrics::{MoistAirState, CP_WATER, RHO_WATER};

    fn make_ctx() -> SimulationContext {
        SimulationContext {
            timestep: TimeStep {
                month: 1,
                day: 1,
                hour: 12,
                sub_hour: 1,
                timesteps_per_hour: 1,
                sim_time_s: 0.0,
                dt: 3600.0,
            },
            outdoor_air: MoistAirState::from_tdb_rh(0.0, 0.5, 101325.0),
            day_type: DayType::WeatherDay,
            is_sizing: false,
        }
    }

    /// Variable speed pump at 50% flow should use ~12.5% power (affinity laws, n=3).
    /// PLR = 0.5, power = design_power * 0.5^3 = 0.125 * design_power
    #[test]
    fn test_variable_speed_pump_affinity_laws() {
        let design_flow = 0.01; // m^3/s
        let design_head = 200_000.0; // Pa
        let motor_eff = 0.90;
        let mut pump = Pump::new("VS Pump", PumpType::VariableSpeed, design_flow, design_head, motor_eff);
        pump.curve_exponent = 3.0;

        let design_power = design_flow * design_head / motor_eff;

        // 50% of design mass flow
        let half_mass_flow = design_flow * RHO_WATER * 0.5;
        let inlet = WaterPort::new(FluidState::water(20.0, half_mass_flow));
        let ctx = make_ctx();

        let _outlet = pump.simulate_plant(&inlet, 1.0, &ctx);

        // Expected: design_power * 0.5^3 = design_power * 0.125
        let expected_power = design_power * 0.125;
        assert_relative_eq!(pump.power, expected_power, max_relative = 0.001);
    }

    /// Constant speed pump always uses full design power when load > 0.
    #[test]
    fn test_constant_speed_pump_full_power() {
        let design_flow = 0.01;
        let design_head = 200_000.0;
        let motor_eff = 0.90;
        let mut pump = Pump::new("CS Pump", PumpType::ConstantSpeed, design_flow, design_head, motor_eff);

        let design_power = design_flow * design_head / motor_eff;

        // Even at 30% flow, constant speed pump runs at full power
        let partial_mass_flow = design_flow * RHO_WATER * 0.3;
        let inlet = WaterPort::new(FluidState::water(20.0, partial_mass_flow));
        let ctx = make_ctx();

        let _outlet = pump.simulate_plant(&inlet, 1.0, &ctx);

        assert_relative_eq!(pump.power, design_power, max_relative = 0.001);
    }

    /// Zero load means the pump is off: zero power and pass-through.
    #[test]
    fn test_pump_zero_load_off() {
        let mut pump = Pump::new(
            "Off Pump",
            PumpType::VariableSpeed,
            0.01,
            200_000.0,
            0.90,
        );

        let inlet = WaterPort::new(FluidState::water(20.0, 5.0));
        let ctx = make_ctx();

        let outlet = pump.simulate_plant(&inlet, 0.0, &ctx);

        assert_eq!(pump.power, 0.0);
        assert_eq!(pump.heat_to_fluid, 0.0);
        assert_relative_eq!(outlet.state.temp, inlet.state.temp, max_relative = 0.001);
    }

    /// Pump adds small heat to water: delta_T = power * frac / (m_dot * cp).
    #[test]
    fn test_pump_heat_addition() {
        let design_flow = 0.01;
        let design_head = 200_000.0;
        let motor_eff = 0.90;
        let mut pump = Pump::new(
            "Heat Pump",
            PumpType::ConstantSpeed,
            design_flow,
            design_head,
            motor_eff,
        );
        pump.motor_heat_to_fluid_fraction = 1.0;

        let design_power = design_flow * design_head / motor_eff;
        let mass_flow = design_flow * RHO_WATER;
        let inlet = WaterPort::new(FluidState::water(20.0, mass_flow));
        let ctx = make_ctx();

        let outlet = pump.simulate_plant(&inlet, 1.0, &ctx);

        // Expected temperature rise
        let expected_dt = design_power * 1.0 / (mass_flow * CP_WATER);
        assert_relative_eq!(
            outlet.state.temp - inlet.state.temp,
            expected_dt,
            max_relative = 0.001
        );
        assert!(outlet.state.temp > inlet.state.temp);
    }
}
