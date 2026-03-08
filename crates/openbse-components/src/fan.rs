//! Fan component models.
//!
//! Physics match EnergyPlus Fans.cc:
//!   Power = MassFlow * DeltaPress / (TotalEff * RhoAir)
//!   ShaftPower = MotorEff * Power
//!   HeatToAir = ShaftPower + (Power - ShaftPower) * MotorInAirFrac
//!   OutletEnthalpy = InletEnthalpy + HeatToAir / MassFlow
//!
//! Reference: EnergyPlus Engineering Reference, Chapter "Fans"

use openbse_core::ports::*;
use openbse_core::types::*;
use openbse_psychrometrics::{self as psych};
use serde::{Deserialize, Serialize};

/// Fan type enumeration.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum FanType {
    ConstantVolume,
    VAV,
    OnOff,
}

/// Fan component matching EnergyPlus fan model physics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fan {
    pub name: String,
    pub fan_type: FanType,
    /// Design maximum air flow rate [m³/s]. Use AUTOSIZE for autosizing.
    pub design_flow_rate: f64,
    /// Design pressure rise [Pa]
    pub design_pressure_rise: f64,
    /// Total fan efficiency (fan * belt * motor * VFD) [0-1]
    pub total_efficiency: f64,
    /// Motor efficiency [0-1]
    pub motor_efficiency: f64,
    /// Fraction of motor waste heat entering the airstream [0-1]
    pub motor_in_airstream_fraction: f64,

    // VAV fan curve coefficients: Power = C1 + C2*PLR + C3*PLR^2 + C4*PLR^3 + C5*PLR^4
    pub vav_coefficients: [f64; 5],

    // ─── Runtime state (not serialized) ─────────────────────────────────
    #[serde(skip)]
    pub power: f64,
    #[serde(skip)]
    pub heat_to_air: f64,
}

impl Fan {
    /// Create a new constant volume fan.
    pub fn constant_volume(
        name: &str,
        design_flow_rate: f64,
        design_pressure_rise: f64,
        total_efficiency: f64,
        motor_efficiency: f64,
        motor_in_airstream_fraction: f64,
    ) -> Self {
        Self {
            name: name.to_string(),
            fan_type: FanType::ConstantVolume,
            design_flow_rate,
            design_pressure_rise,
            total_efficiency,
            motor_efficiency,
            motor_in_airstream_fraction,
            vav_coefficients: [1.0, 0.0, 0.0, 0.0, 0.0],
            power: 0.0,
            heat_to_air: 0.0,
        }
    }

    /// Create a new VAV fan with default ASHRAE curve.
    pub fn vav(
        name: &str,
        design_flow_rate: f64,
        design_pressure_rise: f64,
        total_efficiency: f64,
        motor_efficiency: f64,
        motor_in_airstream_fraction: f64,
    ) -> Self {
        Self {
            name: name.to_string(),
            fan_type: FanType::VAV,
            design_flow_rate,
            design_pressure_rise,
            total_efficiency,
            motor_efficiency,
            motor_in_airstream_fraction,
            // Default VAV fan curve (typical forward-curved centrifugal)
            vav_coefficients: [0.0407598940, 0.08804497, -0.0729636, 0.9437398, 0.0],
            power: 0.0,
            heat_to_air: 0.0,
        }
    }

    /// Calculate fan power and heat gain.
    /// Matches EnergyPlus `simulateConstant` and `simulateVAV`.
    fn calculate(&mut self, mass_flow: f64, rho_air: f64) {
        if mass_flow <= 0.0 {
            self.power = 0.0;
            self.heat_to_air = 0.0;
            return;
        }

        let design_mass_flow = self.design_flow_rate * rho_air;

        match self.fan_type {
            FanType::ConstantVolume | FanType::OnOff => {
                // Power = MassFlow * DeltaPress / (TotalEff * RhoAir)
                self.power = (mass_flow * self.design_pressure_rise
                    / (self.total_efficiency * rho_air))
                    .max(0.0);
            }
            FanType::VAV => {
                // Flow fraction (part-load ratio)
                let flow_frac = (mass_flow / design_mass_flow).clamp(0.0, 1.0);

                // Part-load power fraction from curve
                let c = &self.vav_coefficients;
                let plf = c[0]
                    + c[1] * flow_frac
                    + c[2] * flow_frac.powi(2)
                    + c[3] * flow_frac.powi(3)
                    + c[4] * flow_frac.powi(4);

                // Design power * part-load fraction
                let design_power = design_mass_flow * self.design_pressure_rise
                    / (self.total_efficiency * rho_air);
                self.power = (plf * design_power).max(0.0);
            }
        }

        // Heat gain to airstream
        // ShaftPower = MotorEff * TotalPower
        // HeatToAir = ShaftPower + (TotalPower - ShaftPower) * MotorInAirFrac
        let shaft_power = self.motor_efficiency * self.power;
        self.heat_to_air =
            shaft_power + (self.power - shaft_power) * self.motor_in_airstream_fraction;
    }
}

impl AirComponent for Fan {
    fn name(&self) -> &str {
        &self.name
    }

    fn simulate_air(&mut self, inlet: &AirPort, _ctx: &SimulationContext) -> AirPort {
        let rho = inlet.state.rho();
        self.calculate(inlet.mass_flow, rho);

        if inlet.mass_flow <= 0.0 {
            return *inlet;
        }

        // Outlet enthalpy = inlet enthalpy + heat_to_air / mass_flow
        let outlet_h = inlet.state.h + self.heat_to_air / inlet.mass_flow;
        // Humidity ratio unchanged through fan
        let outlet_t = psych::tdb_fn_h_w(outlet_h, inlet.state.w);

        AirPort::new(
            psych::MoistAirState::new(outlet_t, inlet.state.w, inlet.state.p_b),
            inlet.mass_flow,
        )
    }

    fn design_air_flow_rate(&self) -> Option<f64> {
        if is_autosize(self.design_flow_rate) {
            None
        } else {
            Some(self.design_flow_rate)
        }
    }

    fn set_design_air_flow_rate(&mut self, flow: f64) {
        self.design_flow_rate = flow;
    }

    fn power_consumption(&self) -> f64 {
        self.power
    }

    fn thermal_output(&self) -> f64 {
        self.heat_to_air
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_relative_eq;
    use openbse_core::types::{DayType, TimeStep};
    use openbse_psychrometrics::MoistAirState;

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
            outdoor_air: MoistAirState::from_tdb_rh(20.0, 0.5, 101325.0),
            day_type: DayType::WeatherDay,
            is_sizing: false,
            sizing_internal_gains: SizingInternalGains::Full,
        }
    }

    #[test]
    fn test_constant_volume_fan_power() {
        let mut fan = Fan::constant_volume("Test Fan", 1.0, 600.0, 0.7, 0.9, 1.0);

        let inlet_state = MoistAirState::from_tdb_rh(20.0, 0.5, 101325.0);
        let rho = inlet_state.rho();
        let mass_flow = 1.0 * rho; // 1 m³/s * density
        let inlet = AirPort::new(inlet_state, mass_flow);

        let ctx = make_ctx();
        let outlet = fan.simulate_air(&inlet, &ctx);

        // Power = mass_flow * delta_p / (eff * rho)
        let expected_power = mass_flow * 600.0 / (0.7 * rho);
        assert_relative_eq!(fan.power, expected_power, max_relative = 0.001);

        // Outlet should be warmer than inlet (fan heat)
        assert!(outlet.state.t_db > inlet.state.t_db);

        // Humidity ratio should be unchanged
        assert_relative_eq!(outlet.state.w, inlet.state.w, max_relative = 0.001);
    }

    #[test]
    fn test_fan_zero_flow() {
        let mut fan = Fan::constant_volume("Test Fan", 1.0, 600.0, 0.7, 0.9, 1.0);
        let inlet = AirPort::new(
            MoistAirState::from_tdb_rh(20.0, 0.5, 101325.0),
            0.0,
        );
        let ctx = make_ctx();
        let outlet = fan.simulate_air(&inlet, &ctx);

        assert_eq!(fan.power, 0.0);
        assert_relative_eq!(outlet.state.t_db, inlet.state.t_db, max_relative = 0.001);
    }
}
