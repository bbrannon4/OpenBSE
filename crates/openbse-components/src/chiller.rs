//! Air-cooled chiller component model.

use openbse_core::ports::*;
use openbse_psychrometrics::FluidState;
use serde::{Deserialize, Serialize};

/// Air-cooled electric chiller.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AirCooledChiller {
    pub name: String,
    pub rated_capacity: f64,
    pub rated_cop: f64,
    pub chw_setpoint: f64,
    pub design_chw_flow: f64,
    #[serde(default = "default_min_plr")]
    pub min_plr: f64,
    #[serde(skip)]
    pub actual_capacity: f64,
    #[serde(skip)]
    pub actual_cop: f64,
    #[serde(skip)]
    pub electric_power: f64,
    #[serde(skip)]
    pub plr: f64,
}

fn default_min_plr() -> f64 { 0.1 }

impl AirCooledChiller {
    pub fn new(name: &str, rated_capacity: f64, rated_cop: f64, chw_setpoint: f64, design_chw_flow: f64) -> Self {
        Self { name: name.to_string(), rated_capacity, rated_cop, chw_setpoint, design_chw_flow, min_plr: 0.1, actual_capacity: 0.0, actual_cop: 0.0, electric_power: 0.0, plr: 0.0 }
    }
    fn capacity_correction(&self, t_outdoor: f64) -> f64 {
        (1.0 - 0.015 * (t_outdoor - 29.4)).clamp(0.5, 1.1)
    }
    fn cop_correction(&self, t_outdoor: f64) -> f64 {
        (1.0 - 0.02 * (t_outdoor - 29.4)).clamp(0.4, 1.1)
    }
    fn plr_efficiency(&self, plr: f64) -> f64 {
        let p = plr.clamp(self.min_plr, 1.0);
        (0.5 + 0.75 * p - 0.25 * p * p).clamp(0.3, 1.0)
    }
}

impl PlantComponent for AirCooledChiller {
    fn name(&self) -> &str { &self.name }
    fn simulate_plant(&mut self, inlet: &WaterPort, load: f64, ctx: &SimulationContext) -> WaterPort {
        let t_outdoor = ctx.outdoor_air.t_db;
        let cp_water = 4186.0;
        if load <= 0.0 {
            self.actual_capacity = 0.0; self.actual_cop = 0.0; self.electric_power = 0.0; self.plr = 0.0;
            return *inlet;
        }
        let cap_factor = self.capacity_correction(t_outdoor);
        let available_cap = self.rated_capacity * cap_factor;
        self.plr = (load / available_cap).clamp(self.min_plr, 1.0);
        self.actual_capacity = self.plr * available_cap;
        let cop_factor = self.cop_correction(t_outdoor);
        let plr_factor = self.plr_efficiency(self.plr);
        self.actual_cop = (self.rated_cop * cop_factor * plr_factor).max(0.5);
        self.electric_power = self.actual_capacity / self.actual_cop;
        let mass_flow = inlet.state.mass_flow.max(0.001);
        let delta_t = self.actual_capacity / (mass_flow * cp_water);
        let t_outlet = (inlet.state.temp - delta_t).max(self.chw_setpoint - 2.0);
        WaterPort::new(FluidState::water(t_outlet, mass_flow))
    }
    fn design_water_flow_rate(&self) -> Option<f64> {
        if self.design_chw_flow <= 0.0 { None } else { Some(self.design_chw_flow) }
    }
    fn power_consumption(&self) -> f64 { self.electric_power }
    fn nominal_capacity(&self) -> Option<f64> { Some(self.rated_capacity) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_relative_eq;
    use openbse_core::types::{DayType, TimeStep};
    use openbse_psychrometrics::MoistAirState;
    fn make_ctx(t_outdoor: f64) -> SimulationContext {
        SimulationContext {
            timestep: TimeStep { month: 7, day: 15, hour: 14, sub_hour: 1, timesteps_per_hour: 1, sim_time_s: 0.0, dt: 3600.0 },
            outdoor_air: MoistAirState::from_tdb_rh(t_outdoor, 0.4, 101325.0),
            day_type: DayType::WeatherDay,
            is_sizing: false,
        }
    }
    #[test]
    fn test_chiller_rated_conditions() {
        let mut chiller = AirCooledChiller::new("Test Chiller", 100_000.0, 3.0, 7.0, 0.005);
        let inlet = WaterPort::new(FluidState::water(12.0, 5.0));
        let ctx = make_ctx(29.4);
        let _outlet = chiller.simulate_plant(&inlet, 100_000.0, &ctx);
        assert!(chiller.actual_cop > 2.0, "COP at rated: {}", chiller.actual_cop);
        assert!(chiller.electric_power > 0.0);
        assert_relative_eq!(chiller.plr, 1.0, epsilon = 0.01);
    }
    #[test]
    fn test_chiller_zero_load() {
        let mut chiller = AirCooledChiller::new("Test Chiller", 100_000.0, 3.0, 7.0, 0.005);
        let inlet = WaterPort::new(FluidState::water(12.0, 5.0));
        let ctx = make_ctx(35.0);
        let _outlet = chiller.simulate_plant(&inlet, 0.0, &ctx);
        assert_eq!(chiller.electric_power, 0.0);
        assert_eq!(chiller.plr, 0.0);
    }
    #[test]
    fn test_chiller_hot_outdoor_reduces_cop() {
        let mut chiller_cool = AirCooledChiller::new("C1", 100_000.0, 3.0, 7.0, 0.005);
        let mut chiller_hot = AirCooledChiller::new("C2", 100_000.0, 3.0, 7.0, 0.005);
        let inlet = WaterPort::new(FluidState::water(12.0, 5.0));
        chiller_cool.simulate_plant(&inlet, 80_000.0, &make_ctx(25.0));
        chiller_hot.simulate_plant(&inlet, 80_000.0, &make_ctx(40.0));
        assert!(chiller_cool.actual_cop > chiller_hot.actual_cop,
            "COP at 25C ({:.2}) should be > COP at 40C ({:.2})", chiller_cool.actual_cop, chiller_hot.actual_cop);
    }
    #[test]
    fn test_chiller_part_load() {
        let mut chiller = AirCooledChiller::new("Test Chiller", 100_000.0, 3.0, 7.0, 0.005);
        let inlet = WaterPort::new(FluidState::water(12.0, 5.0));
        let ctx = make_ctx(29.4);
        let _out = chiller.simulate_plant(&inlet, 50_000.0, &ctx);
        assert!((chiller.plr - 0.5).abs() < 0.05, "PLR should be ~0.5, got {}", chiller.plr);
        assert!(chiller.electric_power > 0.0);
    }
}
