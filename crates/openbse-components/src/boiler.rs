//! Boiler component model.
//!
//! Physics match EnergyPlus Boilers.cc (CalcBoilerModel):
//!   PLR = Load / NominalCapacity
//!   EffCurveOutput = f(PLR) or f(PLR, Temp)
//!   FuelUsed = Load / (NominalEfficiency * EffCurveOutput)
//!   OutletTemp = InletTemp + Load / (MassFlow * Cp)
//!
//! Reference: EnergyPlus Engineering Reference, Chapter "Boilers"

use openbse_core::ports::*;
use openbse_core::types::*;
use openbse_psychrometrics::FluidState;
use serde::{Deserialize, Serialize};

/// Boiler efficiency curve type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EfficiencyCurve {
    /// Constant efficiency (no curve, EffCurveOutput = 1.0)
    Constant,
    /// Polynomial in PLR: C0 + C1*PLR + C2*PLR^2 + C3*PLR^3
    PartLoadRatio(Vec<f64>),
    // Future: BiQuadratic(PLR, Temp) curves
}

impl EfficiencyCurve {
    /// Evaluate the efficiency curve modifier.
    pub fn evaluate(&self, plr: f64) -> f64 {
        match self {
            EfficiencyCurve::Constant => 1.0,
            EfficiencyCurve::PartLoadRatio(coeffs) => {
                let mut result = 0.0;
                for (i, c) in coeffs.iter().enumerate() {
                    result += c * plr.powi(i as i32);
                }
                result.clamp(0.01, 1.1) // Match EnergyPlus bounds
            }
        }
    }
}

/// Boiler component matching EnergyPlus boiler model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Boiler {
    pub name: String,
    /// Nominal (design) capacity [W]. Use AUTOSIZE for autosizing.
    pub nominal_capacity: f64,
    /// Nominal thermal efficiency [0-1]
    pub nominal_efficiency: f64,
    /// Design outlet temperature [°C]
    pub design_outlet_temp: f64,
    /// Design water flow rate [m³/s]. Use AUTOSIZE for autosizing.
    pub design_water_flow_rate: f64,
    /// Minimum part load ratio [0-1]
    pub min_plr: f64,
    /// Maximum part load ratio [0-1]
    pub max_plr: f64,
    /// Optimum part load ratio [0-1]
    pub opt_plr: f64,
    /// Maximum outlet temperature limit [°C]
    pub max_outlet_temp: f64,
    /// Efficiency curve
    pub efficiency_curve: EfficiencyCurve,
    /// Parasitic electric load (forced draft fan) [W]
    pub parasitic_electric_load: f64,
    /// Sizing factor
    pub sizing_factor: f64,

    // ─── Runtime state ──────────────────────────────────────────────────
    #[serde(skip)]
    pub fuel_used: f64,
    #[serde(skip)]
    pub boiler_load: f64,
    #[serde(skip)]
    pub operating_plr: f64,
    #[serde(skip)]
    pub parasitic_power: f64,
}

impl Boiler {
    /// Create a new boiler with typical defaults.
    pub fn new(
        name: &str,
        nominal_capacity: f64,
        nominal_efficiency: f64,
        design_outlet_temp: f64,
        design_water_flow_rate: f64,
    ) -> Self {
        Self {
            name: name.to_string(),
            nominal_capacity,
            nominal_efficiency,
            design_outlet_temp,
            design_water_flow_rate,
            min_plr: 0.0,
            max_plr: 1.0,
            opt_plr: 1.0,
            max_outlet_temp: 99.9,
            efficiency_curve: EfficiencyCurve::Constant,
            parasitic_electric_load: 0.0,
            sizing_factor: 1.0,
            fuel_used: 0.0,
            boiler_load: 0.0,
            operating_plr: 0.0,
            parasitic_power: 0.0,
        }
    }
}

impl PlantComponent for Boiler {
    fn name(&self) -> &str {
        &self.name
    }

    fn simulate_plant(
        &mut self,
        inlet: &WaterPort,
        load: f64,
        _ctx: &SimulationContext,
    ) -> WaterPort {
        // No load or no flow: pass through
        if load <= 0.0 || inlet.state.mass_flow <= 0.0 {
            self.fuel_used = 0.0;
            self.boiler_load = 0.0;
            self.operating_plr = 0.0;
            self.parasitic_power = 0.0;
            return *inlet;
        }

        // Limit load to capacity
        let boiler_load = load.min(self.nominal_capacity);

        // Calculate PLR
        let plr = (boiler_load / self.nominal_capacity).clamp(self.min_plr, self.max_plr);

        // Evaluate efficiency curve
        let eff_curve_output = self.efficiency_curve.evaluate(plr);

        // Calculate outlet temperature
        let delta_t = boiler_load / (inlet.state.mass_flow * inlet.state.cp);
        let mut outlet_temp = inlet.state.temp + delta_t;

        // Limit outlet temperature
        if outlet_temp > self.max_outlet_temp {
            outlet_temp = self.max_outlet_temp;
            // Recalculate actual load based on limited temperature
            let actual_load =
                inlet.state.mass_flow * inlet.state.cp * (outlet_temp - inlet.state.temp);
            self.boiler_load = actual_load;
        } else {
            self.boiler_load = boiler_load;
        }

        // Calculate fuel use: FuelUsed = Load / (NomEff * CurveOutput)
        let boiler_eff = (self.nominal_efficiency * eff_curve_output).clamp(0.01, 1.1);
        self.fuel_used = self.boiler_load / boiler_eff;

        // Parasitic electric power
        self.operating_plr = plr;
        self.parasitic_power = self.parasitic_electric_load * plr;

        WaterPort::new(FluidState::water(outlet_temp, inlet.state.mass_flow))
    }

    fn design_water_flow_rate(&self) -> Option<f64> {
        if is_autosize(self.design_water_flow_rate) {
            None
        } else {
            Some(self.design_water_flow_rate)
        }
    }

    fn set_design_water_flow_rate(&mut self, flow: f64) {
        self.design_water_flow_rate = flow;
    }

    fn power_consumption(&self) -> f64 {
        // Boiler parasitic electric (e.g. forced draft fan)
        self.parasitic_power
    }

    fn fuel_consumption(&self) -> f64 {
        // Gas/fuel consumed = heat output / efficiency
        self.fuel_used
    }

    fn nominal_capacity(&self) -> Option<f64> {
        if is_autosize(self.nominal_capacity) {
            None
        } else {
            Some(self.nominal_capacity)
        }
    }

    fn set_nominal_capacity(&mut self, cap: f64) {
        self.nominal_capacity = cap;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_relative_eq;
    use openbse_core::types::{DayType, TimeStep};
    use openbse_psychrometrics::{MoistAirState, CP_WATER};

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

    #[test]
    fn test_boiler_basic_operation() {
        let mut boiler = Boiler::new("Test Boiler", 100_000.0, 0.80, 82.0, 0.001);

        let inlet = WaterPort::new(FluidState::water(60.0, 2.0)); // 60°C, 2 kg/s
        let load = 50_000.0; // 50 kW
        let ctx = make_ctx();

        let outlet = boiler.simulate_plant(&inlet, load, &ctx);

        // Outlet should be warmer
        assert!(outlet.state.temp > inlet.state.temp);

        // Delta-T = Q / (m * cp) = 50000 / (2 * 4180) = 5.98°C
        let expected_dt = 50_000.0 / (2.0 * CP_WATER);
        assert_relative_eq!(
            outlet.state.temp - inlet.state.temp,
            expected_dt,
            max_relative = 0.001
        );

        // Fuel used = Load / Efficiency = 50000 / 0.80 = 62500 W
        assert_relative_eq!(boiler.fuel_used, 50_000.0 / 0.80, max_relative = 0.001);
    }

    #[test]
    fn test_boiler_capacity_limited() {
        let mut boiler = Boiler::new("Small Boiler", 30_000.0, 0.80, 82.0, 0.001);

        let inlet = WaterPort::new(FluidState::water(60.0, 2.0));
        let load = 50_000.0; // Exceeds capacity
        let ctx = make_ctx();

        let _outlet = boiler.simulate_plant(&inlet, load, &ctx);

        // Should be limited to 30 kW
        assert_relative_eq!(boiler.boiler_load, 30_000.0, max_relative = 0.001);
    }

    #[test]
    fn test_boiler_no_load() {
        let mut boiler = Boiler::new("Test Boiler", 100_000.0, 0.80, 82.0, 0.001);

        let inlet = WaterPort::new(FluidState::water(60.0, 2.0));
        let ctx = make_ctx();

        let outlet = boiler.simulate_plant(&inlet, 0.0, &ctx);

        assert_eq!(boiler.fuel_used, 0.0);
        assert_relative_eq!(outlet.state.temp, inlet.state.temp, max_relative = 0.001);
    }

    #[test]
    fn test_boiler_plr_curve() {
        let mut boiler = Boiler::new("Curved Boiler", 100_000.0, 0.80, 82.0, 0.001);
        // Simple PLR curve: 0.8 + 0.2*PLR (efficiency improves at higher load)
        boiler.efficiency_curve =
            EfficiencyCurve::PartLoadRatio(vec![0.8, 0.2]);

        let inlet = WaterPort::new(FluidState::water(60.0, 2.0));
        let ctx = make_ctx();

        // At 50% load: curve output = 0.8 + 0.2*0.5 = 0.9
        let _outlet = boiler.simulate_plant(&inlet, 50_000.0, &ctx);
        let expected_fuel = 50_000.0 / (0.80 * 0.9);
        assert_relative_eq!(boiler.fuel_used, expected_fuel, max_relative = 0.001);
    }
}
