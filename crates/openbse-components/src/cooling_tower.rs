//! Cooling tower component for condenser water loops.
//!
//! Models a single-cell counterflow cooling tower using the
//! effectiveness-NTU approach.
//!
//! Key physics:
//!   Q_reject = m_water * cp * (T_water_in - T_water_out)
//!   T_water_out >= T_wb_outdoor + approach (approach typically 3-5 C)
//!
//! Fan power follows cubic fan laws for variable speed towers.
//!
//! Reference: EnergyPlus Engineering Reference, "Cooling Towers"

use openbse_core::ports::*;
use openbse_core::types::*;
use openbse_psychrometrics::{FluidState, CP_WATER, RHO_WATER};
use serde::{Deserialize, Serialize};

/// Cooling tower fan speed control type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CoolingTowerType {
    /// Fan is either fully on or fully off.
    SingleSpeed,
    /// Fan operates at high speed, low speed (~50%), or off.
    TwoSpeed,
    /// Fan speed modulates continuously to match load (VFD).
    VariableSpeed,
}

/// Cooling tower component for condenser water heat rejection.
///
/// Rejects heat from the condenser water loop to the atmosphere.
/// The tower cools warm condenser water by evaporating a portion of it
/// into the outdoor air stream. Performance is fundamentally limited by
/// the outdoor wet-bulb temperature: the outlet water temperature can
/// never fall below T_wb + approach.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoolingTower {
    pub name: String,
    pub tower_type: CoolingTowerType,
    /// Design water flow rate [m3/s]
    pub design_water_flow: f64,
    /// Design air flow rate [m3/s]
    pub design_air_flow: f64,
    /// Design fan power [W]
    pub design_fan_power: f64,
    /// Design inlet water temperature [C] (typically 35 C condenser return)
    pub design_inlet_water_temp: f64,
    /// Design approach temperature [C] (T_water_out - T_wb, typically 3-5 C)
    pub design_approach: f64,
    /// Design range [C] (T_water_in - T_water_out, typically 5-6 C)
    pub design_range: f64,
    /// Minimum approach temperature [C] (physical limit, typically 2 C)
    pub min_approach: f64,

    // ---- Runtime state --------------------------------------------------------
    /// Current fan electric power [W]
    #[serde(skip)]
    pub fan_power: f64,
    /// Current heat rejection rate [W]
    #[serde(skip)]
    pub heat_rejected: f64,
}

impl CoolingTower {
    /// Create a new cooling tower with the given design parameters.
    pub fn new(
        name: &str,
        tower_type: CoolingTowerType,
        design_water_flow: f64,
        design_air_flow: f64,
        design_fan_power: f64,
        design_inlet_water_temp: f64,
        design_approach: f64,
        design_range: f64,
    ) -> Self {
        Self {
            name: name.to_string(),
            tower_type,
            design_water_flow,
            design_air_flow,
            design_fan_power,
            design_inlet_water_temp,
            design_approach,
            design_range,
            min_approach: 2.0,
            fan_power: 0.0,
            heat_rejected: 0.0,
        }
    }

    /// Design heat rejection capacity [W].
    ///
    /// capacity = m_water_design * cp * design_range
    pub fn design_capacity(&self) -> f64 {
        let mass_flow_design = self.design_water_flow * RHO_WATER;
        mass_flow_design * CP_WATER * self.design_range
    }
}

impl PlantComponent for CoolingTower {
    fn name(&self) -> &str {
        &self.name
    }

    fn simulate_plant(
        &mut self,
        inlet: &WaterPort,
        load: f64,
        ctx: &SimulationContext,
    ) -> WaterPort {
        // No load or no flow: tower is idle
        if load <= 0.0 || inlet.state.mass_flow <= 0.0 {
            self.fan_power = 0.0;
            self.heat_rejected = 0.0;
            return *inlet;
        }

        let t_wb = ctx.outdoor_air.t_wb();
        let t_water_in = inlet.state.temp;
        let mass_flow = inlet.state.mass_flow;
        let cp = inlet.state.cp;

        // Minimum achievable outlet temperature (physical limit)
        let t_min_outlet = t_wb + self.min_approach;

        // Maximum heat rejection limited by approach temperature
        let max_rejection = if t_water_in > t_min_outlet {
            mass_flow * cp * (t_water_in - t_min_outlet)
        } else {
            // Inlet water is already at or below the minimum achievable outlet;
            // tower cannot reject any heat.
            0.0
        };

        // Actual heat rejection: limited by both demand and approach constraint
        let actual_rejection = load.min(max_rejection);

        // Design capacity for PLR calculation
        let design_cap = self.design_capacity();

        // Calculate outlet temperature
        let t_water_out = if actual_rejection > 0.0 {
            let t_out = t_water_in - actual_rejection / (mass_flow * cp);
            t_out.max(t_min_outlet)
        } else {
            t_water_in
        };

        // Fan power depends on tower type
        self.fan_power = if actual_rejection <= 0.0 {
            0.0
        } else {
            match self.tower_type {
                CoolingTowerType::SingleSpeed => {
                    // Fan is fully on whenever there is any load
                    self.design_fan_power
                }
                CoolingTowerType::TwoSpeed => {
                    let plr = (actual_rejection / design_cap).clamp(0.0, 1.0);
                    if plr > 0.5 {
                        // High speed
                        self.design_fan_power
                    } else {
                        // Low speed (~50% speed -> ~12.5% power via cubic law)
                        self.design_fan_power * 0.125
                    }
                }
                CoolingTowerType::VariableSpeed => {
                    // Fan speed modulates to match load.
                    // PLR = actual_rejection / design_capacity
                    // Fan power follows cubic fan law: P = P_design * PLR^3
                    let plr = (actual_rejection / design_cap).clamp(0.0, 1.0);
                    self.design_fan_power * plr * plr * plr
                }
            }
        };

        self.heat_rejected = actual_rejection;

        WaterPort::new(FluidState::water(t_water_out, mass_flow))
    }

    fn design_water_flow_rate(&self) -> Option<f64> {
        if is_autosize(self.design_water_flow) {
            None
        } else {
            Some(self.design_water_flow)
        }
    }

    fn set_design_water_flow_rate(&mut self, flow: f64) {
        self.design_water_flow = flow;
    }

    fn power_consumption(&self) -> f64 {
        self.fan_power
    }

    fn nominal_capacity(&self) -> Option<f64> {
        let cap = self.design_capacity();
        if cap > 0.0 {
            Some(cap)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_relative_eq;
    use openbse_core::types::{DayType, TimeStep};
    use openbse_psychrometrics::MoistAirState;

    /// Helper to create a SimulationContext with the specified outdoor conditions.
    /// Uses `from_tdb_rh` to build the MoistAirState so that `t_wb()` is
    /// derived from thermodynamic properties (not a stored field).
    fn make_ctx(t_db: f64, rh: f64) -> SimulationContext {
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
            outdoor_air: MoistAirState::from_tdb_rh(t_db, rh, 101325.0),
            day_type: DayType::WeatherDay,
            is_sizing: false,
        }
    }

    /// Create a standard variable-speed tower for testing.
    /// Design: 0.01 m3/s water, 10 m3/s air, 5000 W fan, 35 C inlet,
    ///         4 C approach, 5 C range.
    fn make_tower() -> CoolingTower {
        CoolingTower::new(
            "Test Tower",
            CoolingTowerType::VariableSpeed,
            0.01,   // design water flow [m3/s]
            10.0,   // design air flow [m3/s]
            5000.0, // design fan power [W]
            35.0,   // design inlet water temp [C]
            4.0,    // design approach [C]
            5.0,    // design range [C]
        )
    }

    #[test]
    fn test_normal_operation() {
        // Tower cools condenser water under normal conditions.
        // Outdoor: 30 C dry-bulb, 40% RH -> T_wb ~ 20 C
        let mut tower = make_tower();
        let ctx = make_ctx(30.0, 0.40);
        let t_wb = ctx.outdoor_air.t_wb();

        // Inlet at 35 C, 10 kg/s
        let mass_flow = 10.0;
        let inlet = WaterPort::new(FluidState::water(35.0, mass_flow));
        let load = 100_000.0; // 100 kW rejection demand

        let outlet = tower.simulate_plant(&inlet, load, &ctx);

        // Outlet should be cooler than inlet
        assert!(
            outlet.state.temp < inlet.state.temp,
            "Outlet {:.2} C should be < inlet {:.2} C",
            outlet.state.temp,
            inlet.state.temp
        );

        // Outlet should be above wet-bulb + min_approach
        assert!(
            outlet.state.temp >= t_wb + tower.min_approach - 0.01,
            "Outlet {:.2} C should be >= T_wb + min_approach ({:.2} C)",
            outlet.state.temp,
            t_wb + tower.min_approach
        );

        // Heat rejected should match load (within capacity)
        let expected_dt = load / (mass_flow * CP_WATER);
        assert_relative_eq!(
            inlet.state.temp - outlet.state.temp,
            expected_dt,
            max_relative = 0.01
        );

        // Fan power should be positive
        assert!(tower.fan_power > 0.0, "Fan power should be > 0");
        assert!(tower.heat_rejected > 0.0, "Heat rejected should be > 0");
    }

    #[test]
    fn test_approach_temperature_limit() {
        // Tower cannot cool water below T_wb + min_approach.
        // Use a high wet-bulb scenario: 32 C dry-bulb, 80% RH -> T_wb ~ 29 C
        // min_approach = 2 C -> minimum outlet = ~31 C
        // If inlet is 33 C and we demand huge load, outlet is clamped.
        let mut tower = make_tower();
        let ctx = make_ctx(32.0, 0.80);
        let t_wb = ctx.outdoor_air.t_wb();
        let t_min_outlet = t_wb + tower.min_approach;

        let mass_flow = 10.0;
        let inlet = WaterPort::new(FluidState::water(33.0, mass_flow));
        // Demand far more rejection than physically possible
        let huge_load = 1_000_000.0; // 1 MW

        let outlet = tower.simulate_plant(&inlet, huge_load, &ctx);

        // Outlet must not go below T_wb + min_approach
        assert!(
            outlet.state.temp >= t_min_outlet - 0.01,
            "Outlet {:.2} C should be >= T_wb + min_approach ({:.2} C)",
            outlet.state.temp,
            t_min_outlet
        );

        // Heat rejected should be less than demanded
        assert!(
            tower.heat_rejected < huge_load,
            "Heat rejected ({:.0} W) should be < demanded ({:.0} W)",
            tower.heat_rejected,
            huge_load
        );
    }

    #[test]
    fn test_variable_speed_cubic_fan_law() {
        // Fan power should follow cubic law: P = P_design * PLR^3
        let mut tower = make_tower();
        let ctx = make_ctx(25.0, 0.40); // mild conditions, plenty of capacity

        let mass_flow = 10.0;
        let inlet = WaterPort::new(FluidState::water(35.0, mass_flow));

        let design_cap = tower.design_capacity();

        // Run at 50% load
        let half_load = design_cap * 0.5;
        tower.simulate_plant(&inlet, half_load, &ctx);

        // Expected: PLR = 0.5, fan power = 5000 * 0.5^3 = 625 W
        let expected_fan_power = tower.design_fan_power * 0.5_f64.powi(3);
        assert_relative_eq!(
            tower.fan_power,
            expected_fan_power,
            max_relative = 0.02
        );

        // Run at 25% load
        let quarter_load = design_cap * 0.25;
        tower.simulate_plant(&inlet, quarter_load, &ctx);

        // Expected: PLR = 0.25, fan power = 5000 * 0.25^3 = 78.125 W
        let expected_fan_power_25 = tower.design_fan_power * 0.25_f64.powi(3);
        assert_relative_eq!(
            tower.fan_power,
            expected_fan_power_25,
            max_relative = 0.02
        );
    }

    #[test]
    fn test_zero_load_zero_fan_power() {
        // Zero load should produce zero fan power and pass-through temperatures.
        let mut tower = make_tower();
        let ctx = make_ctx(30.0, 0.40);

        let inlet = WaterPort::new(FluidState::water(35.0, 10.0));

        let outlet = tower.simulate_plant(&inlet, 0.0, &ctx);

        assert_eq!(tower.fan_power, 0.0, "Fan power should be 0 at no load");
        assert_eq!(
            tower.heat_rejected, 0.0,
            "Heat rejected should be 0 at no load"
        );
        assert_relative_eq!(
            outlet.state.temp,
            inlet.state.temp,
            max_relative = 0.001
        );
    }

    #[test]
    fn test_higher_wet_bulb_reduces_capacity() {
        // At higher wet-bulb temperatures, the tower can reject less heat.
        let mut tower_cool = make_tower();
        let mut tower_hot = make_tower();

        let mass_flow = 10.0;
        let inlet = WaterPort::new(FluidState::water(35.0, mass_flow));
        let large_load = 500_000.0; // demand more than capacity to see the limit

        // Cool conditions: 25 C dry-bulb, 30% RH -> low T_wb
        let ctx_cool = make_ctx(25.0, 0.30);
        tower_cool.simulate_plant(&inlet, large_load, &ctx_cool);

        // Hot/humid conditions: 35 C dry-bulb, 80% RH -> high T_wb
        let ctx_hot = make_ctx(35.0, 0.80);
        tower_hot.simulate_plant(&inlet, large_load, &ctx_hot);

        assert!(
            tower_cool.heat_rejected > tower_hot.heat_rejected,
            "Cool conditions should reject more heat ({:.0} W) than hot ({:.0} W)",
            tower_cool.heat_rejected,
            tower_hot.heat_rejected
        );
    }
}
