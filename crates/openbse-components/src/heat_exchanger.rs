//! Water-to-water heat exchanger for inter-loop connections.
//!
//! Models a plate-and-frame (or similar) heat exchanger connecting two
//! plant water loops using the effectiveness-NTU method.
//!
//! This component is installed on the "demand" loop and draws heat
//! from (or rejects heat to) a "source" loop. Source-side conditions
//! are injected by the simulation driver via `set_source_conditions()`
//! before `simulate_plant()` is called.
//!
//! Two control modes:
//! - AlwaysOn: HX active whenever there is load (general inter-loop connection)
//! - Economizer: HX active only when source temperature enables free cooling
//!   (waterside economizer — bypasses chiller when condenser water is cold enough)
//!
//! Physics:
//!   C_demand = m_demand × cp
//!   C_source = m_source × cp
//!   C_min = min(C_demand, C_source)
//!   Q_max = effectiveness × C_min × |T_source - T_demand_inlet|
//!   Q_actual = min(|load|, Q_max)
//!
//! Reference: EnergyPlus Engineering Reference, "HeatExchanger:FluidToFluid"

use openbse_core::ports::*;
use openbse_core::types::*;
use openbse_psychrometrics::{FluidState, CP_WATER};
use serde::{Deserialize, Serialize};

/// Heat exchanger control mode.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum HXControlMode {
    /// Always active when there is load on the demand loop.
    AlwaysOn,
    /// Active only when source-side temperature enables free cooling.
    /// For a waterside economizer: activates when T_source < T_demand_inlet - threshold.
    /// This avoids running the chiller when the condenser water is cold enough
    /// to directly cool the chilled water loop.
    Economizer,
}

/// Water-to-water heat exchanger for connecting two plant loops.
///
/// Installed on the demand-side loop. Source-side conditions are provided
/// by the simulation driver from the already-simulated source loop.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WaterToWaterHX {
    pub name: String,
    /// Heat transfer effectiveness [0-1]. Typical 0.70-0.95 for plate-and-frame.
    pub effectiveness: f64,
    /// Design demand-side water flow rate [m³/s]
    pub design_flow_rate: f64,
    /// Control mode
    pub control_mode: HXControlMode,
    /// For economizer mode: activate when T_source < T_demand_inlet - threshold [°C].
    /// Default 2.0°C. The source must be meaningfully colder than the demand side
    /// for heat transfer to be useful.
    pub economizer_threshold: f64,
    /// Name of the source plant loop (for dependency tracking by sim driver)
    pub source_loop_name: String,

    // ---- Runtime state (injected by simulation driver) --------------------
    /// Source-side inlet temperature [°C]. Set by `set_source_conditions()`.
    #[serde(skip)]
    source_inlet_temp: f64,
    /// Source-side mass flow rate [kg/s]. Set by `set_source_conditions()`.
    #[serde(skip)]
    source_mass_flow: f64,

    // ---- Runtime output ---------------------------------------------------
    /// Heat transferred this timestep [W]. Positive = heating demand side.
    #[serde(skip)]
    heat_transferred: f64,
}

impl WaterToWaterHX {
    /// Create a new water-to-water heat exchanger.
    pub fn new(
        name: &str,
        effectiveness: f64,
        design_flow_rate: f64,
        control_mode: HXControlMode,
        economizer_threshold: f64,
        source_loop_name: &str,
    ) -> Self {
        Self {
            name: name.to_string(),
            effectiveness: effectiveness.clamp(0.0, 1.0),
            design_flow_rate,
            control_mode,
            economizer_threshold,
            source_loop_name: source_loop_name.to_string(),
            source_inlet_temp: 20.0,
            source_mass_flow: 0.0,
            heat_transferred: 0.0,
        }
    }
}

impl PlantComponent for WaterToWaterHX {
    fn name(&self) -> &str {
        &self.name
    }

    fn simulate_plant(
        &mut self,
        inlet: &WaterPort,
        load: f64,
        _ctx: &SimulationContext,
    ) -> WaterPort {
        self.heat_transferred = 0.0;

        // No load, no demand-side flow, or no source flow: pass through
        if load.abs() < 1.0 || inlet.state.mass_flow <= 0.0 || self.source_mass_flow <= 0.0 {
            return *inlet;
        }

        let t_demand_in = inlet.state.temp;
        let m_demand = inlet.state.mass_flow;
        let cp = inlet.state.cp;

        // Check economizer mode: source must be able to provide useful cooling
        // For cooling: source must be colder than demand inlet
        // For heating: source must be hotter than demand inlet
        if self.control_mode == HXControlMode::Economizer {
            // In economizer mode, HX provides cooling: source must be meaningfully
            // colder than demand inlet.
            if self.source_inlet_temp >= t_demand_in - self.economizer_threshold {
                return *inlet; // Source not cold enough, bypass HX
            }
        }

        // Temperature difference determines heat transfer direction
        let dt = self.source_inlet_temp - t_demand_in;
        if dt.abs() < 0.01 {
            return *inlet; // No meaningful temperature difference
        }

        // Effectiveness-NTU calculation
        let c_demand = m_demand * cp;
        let c_source = self.source_mass_flow * CP_WATER;
        let c_min = c_demand.min(c_source);

        // Maximum possible heat transfer
        let q_max = self.effectiveness * c_min * dt.abs();

        // Actual heat transfer: limited by demand (load) and capacity (q_max)
        // load > 0 means demand side needs heating, load < 0 means needs cooling
        let q_actual = if load > 0.0 && dt > 0.0 {
            // Heating: source is hotter, demand needs heating
            load.min(q_max)
        } else if load < 0.0 && dt < 0.0 {
            // Cooling: source is colder, demand needs cooling
            // load is negative (cooling), q_max is positive magnitude
            load.abs().min(q_max)
        } else {
            // Direction mismatch: source can't serve this load direction
            0.0
        };

        if q_actual <= 0.0 {
            return *inlet;
        }

        // Calculate demand-side outlet temperature
        let t_demand_out = if load > 0.0 {
            // Heating: demand side warms up
            t_demand_in + q_actual / c_demand
        } else {
            // Cooling: demand side cools down
            t_demand_in - q_actual / c_demand
        };

        self.heat_transferred = if load > 0.0 { q_actual } else { -q_actual };

        WaterPort::new(FluidState::water(t_demand_out, m_demand))
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

    fn thermal_output(&self) -> f64 {
        self.heat_transferred
    }

    fn set_source_conditions(&mut self, temp: f64, mass_flow: f64) {
        self.source_inlet_temp = temp;
        self.source_mass_flow = mass_flow;
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
                month: 7, day: 15, hour: 14, sub_hour: 1,
                timesteps_per_hour: 1, sim_time_s: 0.0, dt: 3600.0,
            },
            outdoor_air: MoistAirState::from_tdb_rh(30.0, 0.40, 101325.0),
            day_type: DayType::WeatherDay,
            is_sizing: false,
            sizing_internal_gains: SizingInternalGains::Full,
        }
    }

    #[test]
    fn test_cooling_heat_transfer() {
        // Source at 5°C (cold condenser water), demand at 12°C (warm CHW return)
        // HX should cool the demand side.
        let mut hx = WaterToWaterHX::new(
            "Test HX", 0.80, 0.01, HXControlMode::AlwaysOn, 2.0, "Source Loop",
        );
        hx.set_source_conditions(5.0, 10.0); // 5°C, 10 kg/s

        let ctx = make_ctx();
        let demand_inlet = WaterPort::new(FluidState::water(12.0, 5.0)); // 12°C, 5 kg/s
        let load = -50_000.0; // 50 kW cooling demand

        let outlet = hx.simulate_plant(&demand_inlet, load, &ctx);

        // Demand should be cooled
        assert!(outlet.state.temp < 12.0, "Outlet should be cooler than inlet");

        // Check effectiveness limiting
        let c_demand = 5.0 * CP_WATER;
        let c_source = 10.0 * CP_WATER;
        let c_min = c_demand.min(c_source);
        let q_max = 0.80 * c_min * (12.0 - 5.0); // effectiveness * C_min * dT
        let q_actual = 50_000.0_f64.min(q_max);

        let expected_t_out = 12.0 - q_actual / c_demand;
        assert_relative_eq!(outlet.state.temp, expected_t_out, max_relative = 0.001);

        // Thermal output should be negative (cooling)
        assert!(hx.thermal_output() < 0.0, "Thermal output should be negative for cooling");
    }

    #[test]
    fn test_heating_heat_transfer() {
        // Source at 80°C (hot water from boiler loop), demand at 60°C (warm return)
        // HX should heat the demand side.
        let mut hx = WaterToWaterHX::new(
            "Heating HX", 0.90, 0.01, HXControlMode::AlwaysOn, 2.0, "HHW Loop",
        );
        hx.set_source_conditions(80.0, 8.0);

        let ctx = make_ctx();
        let demand_inlet = WaterPort::new(FluidState::water(60.0, 5.0));
        let load = 100_000.0; // 100 kW heating

        let outlet = hx.simulate_plant(&demand_inlet, load, &ctx);

        // Demand should be heated
        assert!(outlet.state.temp > 60.0, "Outlet should be warmer than inlet");
        assert!(hx.thermal_output() > 0.0, "Thermal output should be positive for heating");
    }

    #[test]
    fn test_effectiveness_limiting() {
        // With very large load, HX is limited by effectiveness * C_min * dT
        let mut hx = WaterToWaterHX::new(
            "Limited HX", 0.50, 0.01, HXControlMode::AlwaysOn, 2.0, "Source",
        );
        hx.set_source_conditions(5.0, 2.0); // small source flow → small C_source

        let ctx = make_ctx();
        let demand_inlet = WaterPort::new(FluidState::water(15.0, 10.0));
        let huge_load = -1_000_000.0; // 1 MW cooling demand

        let outlet = hx.simulate_plant(&demand_inlet, huge_load, &ctx);

        // Q_actual should be capped at effectiveness * C_min * dT
        let c_source = 2.0 * CP_WATER;
        let c_demand = 10.0 * CP_WATER;
        let c_min = c_source.min(c_demand); // source is limiting
        let q_max = 0.50 * c_min * (15.0 - 5.0);

        let actual_q = (15.0 - outlet.state.temp) * c_demand;
        assert_relative_eq!(actual_q, q_max, max_relative = 0.01);
    }

    #[test]
    fn test_zero_load_passthrough() {
        let mut hx = WaterToWaterHX::new(
            "Idle HX", 0.80, 0.01, HXControlMode::AlwaysOn, 2.0, "Source",
        );
        hx.set_source_conditions(5.0, 10.0);

        let ctx = make_ctx();
        let inlet = WaterPort::new(FluidState::water(12.0, 5.0));

        let outlet = hx.simulate_plant(&inlet, 0.0, &ctx);
        assert_relative_eq!(outlet.state.temp, 12.0, max_relative = 0.001);
        assert_eq!(hx.thermal_output(), 0.0);
    }

    #[test]
    fn test_zero_source_flow_passthrough() {
        let mut hx = WaterToWaterHX::new(
            "No Source HX", 0.80, 0.01, HXControlMode::AlwaysOn, 2.0, "Source",
        );
        // Source has zero flow — HX can't transfer anything
        hx.set_source_conditions(5.0, 0.0);

        let ctx = make_ctx();
        let inlet = WaterPort::new(FluidState::water(12.0, 5.0));

        let outlet = hx.simulate_plant(&inlet, -50_000.0, &ctx);
        assert_relative_eq!(outlet.state.temp, 12.0, max_relative = 0.001);
    }

    #[test]
    fn test_economizer_mode_activates() {
        // Source at 5°C, demand at 12°C, threshold 2°C.
        // 5 < 12 - 2 = 10 → economizer should activate.
        let mut hx = WaterToWaterHX::new(
            "Econ HX", 0.80, 0.01, HXControlMode::Economizer, 2.0, "CDW Loop",
        );
        hx.set_source_conditions(5.0, 10.0);

        let ctx = make_ctx();
        let inlet = WaterPort::new(FluidState::water(12.0, 5.0));

        let outlet = hx.simulate_plant(&inlet, -50_000.0, &ctx);
        assert!(outlet.state.temp < 12.0, "Economizer should activate and cool");
    }

    #[test]
    fn test_economizer_mode_bypasses_when_source_too_warm() {
        // Source at 11°C, demand at 12°C, threshold 2°C.
        // 11 >= 12 - 2 = 10 → economizer should NOT activate.
        let mut hx = WaterToWaterHX::new(
            "Bypass HX", 0.80, 0.01, HXControlMode::Economizer, 2.0, "CDW Loop",
        );
        hx.set_source_conditions(11.0, 10.0);

        let ctx = make_ctx();
        let inlet = WaterPort::new(FluidState::water(12.0, 5.0));

        let outlet = hx.simulate_plant(&inlet, -50_000.0, &ctx);
        // Economizer should bypass (source too warm)
        assert_relative_eq!(outlet.state.temp, 12.0, max_relative = 0.001);
    }

    #[test]
    fn test_direction_mismatch_no_transfer() {
        // Source at 5°C (cold), but demand wants heating (load > 0).
        // Can't heat demand with cold source.
        let mut hx = WaterToWaterHX::new(
            "Mismatch HX", 0.80, 0.01, HXControlMode::AlwaysOn, 2.0, "Source",
        );
        hx.set_source_conditions(5.0, 10.0);

        let ctx = make_ctx();
        let inlet = WaterPort::new(FluidState::water(12.0, 5.0));
        let load = 50_000.0; // wants heating, but source is colder

        let outlet = hx.simulate_plant(&inlet, load, &ctx);
        // No heat transfer when direction mismatches
        assert_relative_eq!(outlet.state.temp, 12.0, max_relative = 0.001);
    }
}
