//! Constant and variable speed pumps for plant loops (E+ `Pump:ConstantSpeed`,
//! `Pump:VariableSpeed`, `HeaderedPumps:*`).
//!
//! Design power:
//!   P_design = Q_design * H_design / (eta_motor * eta_impeller)
//! with eta_impeller = 1/1.282051282 = 0.78 by default, E+'s design shaft
//! power per unit flow per unit head.
//!
//! Part-load power of a variable-speed pump:
//!   P = P_design * (c1 + c2*x + c3*x^2 + c4*x^3),  x = Q/Q_design
//! with E+'s default coefficients (0, 1, 0, 0).
//!
//! Heat to the fluid: all shaft work ends up in the water, plus the given
//! fraction of the motor losses (E+ `CalcPumps`):
//!   Q_fluid = eta_motor * P + (1 - eta_motor) * P * f_fluid
//!
//! Reference: EnergyPlus Engineering Reference, "Pumps"

use openbse_core::ports::*;
use openbse_core::types::*;
use openbse_psychrometrics::FluidState;
use serde::{Deserialize, Serialize};

/// E+ default design shaft power per unit flow per unit head is
/// 1.282051282 W/((m³/s)·Pa), a hydraulic efficiency of 0.78.
pub const DEFAULT_IMPELLER_EFFICIENCY: f64 = 1.0 / 1.282051282;

/// E+ `Pump:VariableSpeed` default part-load coefficients: power in
/// proportion to flow.
pub const DEFAULT_POWER_CURVE: [f64; 4] = [0.0, 1.0, 0.0, 0.0];

fn default_submeter() -> String {
    "General".to_string()
}

/// Pump type enumeration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum PumpType {
    /// Constant speed: always runs at design power when on.
    ConstantSpeed,
    /// Variable speed: power follows affinity laws with flow fraction.
    VariableSpeed,
}

/// Variable or constant speed pump component for plant loops.
///
/// Supports headered pump configurations where multiple identical pumps
/// stage on/off to match flow demand (E+ HeaderedPumps:VariableSpeed).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pump {
    pub name: String,
    #[serde(default = "default_submeter")]
    pub submeter: String,
    pub pump_type: PumpType,
    /// Design (maximum) TOTAL volumetric flow rate [m^3/s]
    pub design_flow_rate: f64,
    /// Design head [Pa] (typically 150,000-300,000 Pa / 50-100 ft H2O)
    pub design_head: f64,
    /// Design (rated) motor efficiency [0-1]
    pub motor_efficiency: f64,
    /// Impeller/hydraulic efficiency [0-1] (default 0.78, E+'s 1/1.282051282).
    /// Total pump efficiency = motor_efficiency × impeller_efficiency.
    pub impeller_efficiency: f64,
    /// Fraction of the motor losses that heats the fluid [0-1] (default 0, as
    /// E+). The shaft work always heats the fluid.
    pub motor_heat_to_fluid_fraction: f64,
    /// Number of identical pumps in headered (banked) configuration (default 1).
    /// For num_pumps > 1: pumps stage on/off; each individual pump handles
    /// design_flow_rate / num_pumps at full speed.
    pub num_pumps: u32,
    /// Variable-speed part-load power curve coefficients [c1, c2, c3, c4]:
    /// power_frac = c1 + c2*PLR + c3*PLR² + c4*PLR³ (default E+'s 0, 1, 0, 0).
    #[serde(skip)]
    pub power_curve: [f64; 4],

    // ─── Runtime state ──────────────────────────────────────────────────
    /// Electric power consumption this timestep [W]
    #[serde(skip)]
    pub power: f64,
    /// Pump heat added to the water stream this timestep [W]
    #[serde(skip)]
    pub heat_to_fluid: f64,
    /// Current water mass flow rate [kg/s]
    #[serde(skip)]
    pub current_mass_flow: f64,
}

impl Pump {
    /// Volume flow as a fraction of design, E+ `CalcPumps`: the design rating is a
    /// volume, so the mass flow converts at the water's actual density.
    fn flow_fraction(&self, inlet: &WaterPort) -> f64 {
        let volume_flow =
            inlet.state.mass_flow / openbse_psychrometrics::water_rho(inlet.state.temp);
        (volume_flow / self.design_flow_rate).clamp(0.0, 1.0)
    }

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
            submeter: "General".to_string(),
            pump_type,
            design_flow_rate,
            design_head,
            motor_efficiency,
            impeller_efficiency: DEFAULT_IMPELLER_EFFICIENCY,
            motor_heat_to_fluid_fraction: 0.0,
            num_pumps: 1,
            power_curve: DEFAULT_POWER_CURVE,
            power: 0.0,
            heat_to_fluid: 0.0,
            current_mass_flow: 0.0,
        }
    }

    /// Create a headered pump bank with staging, optional power curve, and
    /// custom impeller efficiency.
    pub fn new_headered(
        name: &str,
        pump_type: PumpType,
        design_flow_rate: f64,
        design_head: f64,
        motor_efficiency: f64,
        impeller_efficiency: f64,
        num_pumps: u32,
        power_curve: Option<[f64; 4]>,
    ) -> Self {
        let mut pump = Self::new(
            name,
            pump_type,
            design_flow_rate,
            design_head,
            motor_efficiency,
        );
        pump.impeller_efficiency = impeller_efficiency;
        pump.num_pumps = num_pumps.max(1);
        pump.power_curve = power_curve.unwrap_or(DEFAULT_POWER_CURVE);
        pump
    }

    /// Fraction of design power at part-load ratio `plr` (E+ `CalcPumps`).
    fn power_fraction(&self, plr: f64) -> f64 {
        let c = &self.power_curve;
        (c[0] + c[1] * plr + c[2] * plr * plr + c[3] * plr * plr * plr).max(0.0)
    }

    /// Design (rated) power [W] = Q_design * H_design / (eta_motor * eta_impeller).
    pub fn design_power(&self) -> f64 {
        let total_eff = self.motor_efficiency * self.impeller_efficiency;
        self.design_flow_rate * self.design_head / total_eff
    }
}

impl PlantComponent for Pump {
    fn name(&self) -> &str {
        &self.name
    }

    fn component_kind(&self) -> ComponentKind {
        ComponentKind::Pump
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
            self.current_mass_flow = 0.0;
            return *inlet;
        }

        let total_design_power = self.design_power();

        // Calculate power based on pump type and staging
        self.power = match self.pump_type {
            PumpType::ConstantSpeed => {
                if self.num_pumps > 1 {
                    // Headered constant speed: stage pumps on/off.
                    // Each pump has design_flow / num_pumps capacity.
                    let flow_fraction = self.flow_fraction(inlet);
                    let n = self.num_pumps as f64;
                    let pumps_on = (flow_fraction * n).ceil().max(1.0) as u32;
                    // Each running pump uses full per-pump power
                    total_design_power / n * pumps_on as f64
                } else {
                    // Single constant speed: always runs at full design power when on
                    total_design_power
                }
            }
            PumpType::VariableSpeed => {
                let system_flow_frac = self.flow_fraction(inlet);

                if self.num_pumps > 1 {
                    // Headered variable speed: stage pumps on/off, then
                    // apply power curve to each running pump's individual PLR.
                    let n = self.num_pumps as f64;
                    let per_pump_flow_frac = 1.0 / n;
                    let pumps_on = (system_flow_frac / per_pump_flow_frac).ceil().max(1.0) as u32;
                    // Individual pump PLR
                    let individual_plr = (system_flow_frac * n / pumps_on as f64).min(1.0);
                    let per_pump_power =
                        (total_design_power / n) * self.power_fraction(individual_plr);
                    per_pump_power * pumps_on as f64
                } else {
                    total_design_power * self.power_fraction(system_flow_frac)
                }
            }
        };

        // Store current flow for detailed outputs
        self.current_mass_flow = inlet.state.mass_flow;

        // The shaft work all ends up in the water; the motor losses only in
        // the given fraction.
        let shaft_power = self.power * self.motor_efficiency;
        self.heat_to_fluid =
            shaft_power + (self.power - shaft_power) * self.motor_heat_to_fluid_fraction;

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

    fn unsized_fields(&self) -> Vec<&'static str> {
        autosize_fields([("design_flow_rate", self.design_flow_rate)])
    }

    fn power_consumption(&self) -> f64 {
        self.power
    }

    fn report_outputs(&self, out: &mut dyn FnMut(&str, f64)) {
        out("water_mass_flow", self.current_mass_flow);
        out("pressure_rise", self.design_head);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_relative_eq;
    use openbse_core::types::{DayType, TimeStep};
    use openbse_psychrometrics::{water_cp, water_rho, MoistAirState};

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
            sizing_internal_gains: SizingInternalGains::Full,
        }
    }

    /// Variable speed pump at 50% flow: E+'s default linear curve gives 50%
    /// of design power, and a cubic curve 12.5%.
    #[test]
    fn test_variable_speed_pump_part_load() {
        let design_flow = 0.01; // m^3/s
        let design_head = 200_000.0; // Pa
        let motor_eff = 0.90;
        let mut pump = Pump::new(
            "VS Pump",
            PumpType::VariableSpeed,
            design_flow,
            design_head,
            motor_eff,
        );
        let design_power = design_flow * design_head * 1.282051282 / motor_eff;
        assert_relative_eq!(pump.design_power(), design_power, max_relative = 1e-9);

        let half_mass_flow = design_flow * water_rho(20.0) * 0.5;
        let inlet = WaterPort::new(FluidState::water(20.0, half_mass_flow));
        let ctx = make_ctx();
        pump.simulate_plant(&inlet, 1.0, &ctx);
        assert_relative_eq!(pump.power, 0.5 * design_power, max_relative = 1e-9);

        pump.power_curve = [0.0, 0.0, 0.0, 1.0];
        pump.simulate_plant(&inlet, 1.0, &ctx);
        assert_relative_eq!(pump.power, 0.125 * design_power, max_relative = 1e-9);
    }

    /// Constant speed pump always uses full design power when load > 0.
    #[test]
    fn test_constant_speed_pump_full_power() {
        let design_flow = 0.01;
        let design_head = 200_000.0;
        let motor_eff = 0.90;
        let impeller_eff = DEFAULT_IMPELLER_EFFICIENCY;
        let mut pump = Pump::new(
            "CS Pump",
            PumpType::ConstantSpeed,
            design_flow,
            design_head,
            motor_eff,
        );

        let design_power = design_flow * design_head / (motor_eff * impeller_eff);

        // Even at 30% flow, constant speed pump runs at full power
        let partial_mass_flow = design_flow * water_rho(20.0) * 0.3;
        let inlet = WaterPort::new(FluidState::water(20.0, partial_mass_flow));
        let ctx = make_ctx();

        let _outlet = pump.simulate_plant(&inlet, 1.0, &ctx);

        assert_relative_eq!(pump.power, design_power, max_relative = 0.001);
    }

    /// Zero load means the pump is off: zero power and pass-through.
    #[test]
    fn test_pump_zero_load_off() {
        let mut pump = Pump::new("Off Pump", PumpType::VariableSpeed, 0.01, 200_000.0, 0.90);

        let inlet = WaterPort::new(FluidState::water(20.0, 5.0));
        let ctx = make_ctx();

        let outlet = pump.simulate_plant(&inlet, 0.0, &ctx);

        assert_eq!(pump.power, 0.0);
        assert_eq!(pump.heat_to_fluid, 0.0);
        assert_relative_eq!(outlet.state.temp, inlet.state.temp, max_relative = 0.001);
    }

    /// Heat to the water is the shaft work plus the given fraction of the
    /// motor losses (E+ `CalcPumps`), and the outlet carries exactly it.
    #[test]
    fn test_pump_heat_addition() {
        let design_flow = 0.01;
        let motor_eff = 0.90;
        let mut pump = Pump::new(
            "Heat Pump",
            PumpType::ConstantSpeed,
            design_flow,
            200_000.0,
            motor_eff,
        );
        let mass_flow = design_flow * water_rho(20.0);
        let inlet = WaterPort::new(FluidState::water(20.0, mass_flow));
        let ctx = make_ctx();

        for f in [0.0, 0.5, 1.0] {
            pump.motor_heat_to_fluid_fraction = f;
            let outlet = pump.simulate_plant(&inlet, 1.0, &ctx);
            let p = pump.design_power();
            let expected = motor_eff * p + (1.0 - motor_eff) * p * f;
            assert_relative_eq!(pump.heat_to_fluid, expected, max_relative = 1e-9);
            assert_relative_eq!(
                (outlet.state.temp - inlet.state.temp) * mass_flow * water_cp(20.0),
                expected,
                max_relative = 1e-9
            );
        }
    }
}
