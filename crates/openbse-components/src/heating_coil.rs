//! Heating coil component models.
//!
//! Supports both simple electric and hot water coils.
//! Physics match EnergyPlus HeatingCoils.cc.
//!
//! Hot water coil: Q = m_air * Cp * (T_out - T_in), limited by water-side capacity
//! Electric coil: Q = Capacity * PLR
//!
//! Reference: EnergyPlus Engineering Reference, Chapter "Coils"

use openbse_core::ports::*;
use openbse_psychrometrics::{self as psych, FluidState};
use serde::{Deserialize, Serialize};

/// Heating coil type.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum HeatingCoilType {
    /// Simple electric resistance coil
    Electric,
    /// Hot water coil connected to a plant loop
    HotWater,
    /// Gas furnace (natural gas burner)
    Gas,
}

/// Heating coil component.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeatingCoil {
    pub name: String,
    pub coil_type: HeatingCoilType,
    /// Nominal heating capacity [W]. Use AUTOSIZE for autosizing.
    pub nominal_capacity: f64,
    /// Efficiency [0-1] (electric coils only, always 1.0 for hot water)
    pub efficiency: f64,
    /// Desired outlet air temperature setpoint [°C]
    pub outlet_temp_setpoint: f64,

    // Hot water coil parameters
    /// Design water flow rate [m³/s]
    pub design_water_flow_rate: f64,
    /// Design water inlet temperature [°C]
    pub design_water_inlet_temp: f64,
    /// Design water outlet temperature [°C]
    pub design_water_outlet_temp: f64,

    // ─── Runtime state ──────────────────────────────────────────────────
    #[serde(skip)]
    pub heating_rate: f64,
    #[serde(skip)]
    pub energy_consumption: f64,
    #[serde(skip)]
    water_inlet: Option<WaterPort>,
    #[serde(skip)]
    water_outlet: Option<WaterPort>,
}

impl HeatingCoil {
    /// Create a simple electric heating coil.
    pub fn electric(name: &str, capacity: f64, setpoint: f64) -> Self {
        Self {
            name: name.to_string(),
            coil_type: HeatingCoilType::Electric,
            nominal_capacity: capacity,
            efficiency: 1.0,
            outlet_temp_setpoint: setpoint,
            design_water_flow_rate: 0.0,
            design_water_inlet_temp: 0.0,
            design_water_outlet_temp: 0.0,
            heating_rate: 0.0,
            energy_consumption: 0.0,
            water_inlet: None,
            water_outlet: None,
        }
    }

    /// Create a gas furnace heating coil.
    ///
    /// Gas coils have a burner efficiency (typically 0.78-0.92 for furnaces).
    /// The coil delivers capacity to the air, but consumes capacity/efficiency
    /// worth of fuel energy.
    pub fn gas(name: &str, capacity: f64, setpoint: f64, burner_efficiency: f64) -> Self {
        Self {
            name: name.to_string(),
            coil_type: HeatingCoilType::Gas,
            nominal_capacity: capacity,
            efficiency: burner_efficiency,
            outlet_temp_setpoint: setpoint,
            design_water_flow_rate: 0.0,
            design_water_inlet_temp: 0.0,
            design_water_outlet_temp: 0.0,
            heating_rate: 0.0,
            energy_consumption: 0.0,
            water_inlet: None,
            water_outlet: None,
        }
    }

    /// Create a hot water heating coil.
    pub fn hot_water(
        name: &str,
        capacity: f64,
        setpoint: f64,
        water_flow_rate: f64,
        water_inlet_temp: f64,
        water_outlet_temp: f64,
    ) -> Self {
        Self {
            name: name.to_string(),
            coil_type: HeatingCoilType::HotWater,
            nominal_capacity: capacity,
            efficiency: 1.0,
            outlet_temp_setpoint: setpoint,
            design_water_flow_rate: water_flow_rate,
            design_water_inlet_temp: water_inlet_temp,
            design_water_outlet_temp: water_outlet_temp,
            heating_rate: 0.0,
            energy_consumption: 0.0,
            water_inlet: None,
            water_outlet: None,
        }
    }
}

impl AirComponent for HeatingCoil {
    fn name(&self) -> &str {
        &self.name
    }

    fn simulate_air(&mut self, inlet: &AirPort, _ctx: &SimulationContext) -> AirPort {
        if inlet.mass_flow <= 0.0 {
            self.heating_rate = 0.0;
            self.energy_consumption = 0.0;
            return *inlet;
        }

        let cp_air = psych::cp_air_fn_w(inlet.state.w);

        // Calculate required heating to reach setpoint
        let q_required = inlet.mass_flow * cp_air * (self.outlet_temp_setpoint - inlet.state.t_db);

        // Only heat, don't cool
        let q_required = q_required.max(0.0);

        match self.coil_type {
            HeatingCoilType::Electric => {
                // Limit by capacity
                let q_actual = q_required.min(self.nominal_capacity);
                let outlet_t = inlet.state.t_db + q_actual / (inlet.mass_flow * cp_air);

                self.heating_rate = q_actual;
                self.energy_consumption = q_actual / self.efficiency;

                AirPort::new(
                    psych::MoistAirState::new(outlet_t, inlet.state.w, inlet.state.p_b),
                    inlet.mass_flow,
                )
            }
            HeatingCoilType::Gas => {
                // Gas furnace: same as electric but with burner efficiency
                let q_actual = q_required.min(self.nominal_capacity);
                let outlet_t = inlet.state.t_db + q_actual / (inlet.mass_flow * cp_air);

                self.heating_rate = q_actual;
                // Gas consumption = delivered heat / burner efficiency
                self.energy_consumption = if self.efficiency > 0.0 {
                    q_actual / self.efficiency
                } else {
                    q_actual
                };

                AirPort::new(
                    psych::MoistAirState::new(outlet_t, inlet.state.w, inlet.state.p_b),
                    inlet.mass_flow,
                )
            }
            HeatingCoilType::HotWater => {
                // Calculate water-side available capacity.
                //
                // When the hot water plant loop is coupled (water_inlet is set with
                // real flow from the HHW boiler loop), capacity is limited by the
                // water-side heat available. When the plant loop is not yet connected
                // (water_inlet is None — common in current simulation architecture
                // where plant loops run independently), fall back to nominal_capacity
                // so the coil behaves as if the plant loop always delivers adequate
                // hot water. This allows the air-side simulation to be physically
                // meaningful before full plant-air coupling is implemented.
                let water_capacity = if let Some(ref wi) = self.water_inlet {
                    if wi.state.mass_flow > 0.0 {
                        wi.state.mass_flow
                            * wi.state.cp
                            * (wi.state.temp - self.design_water_outlet_temp).max(0.0)
                    } else {
                        // Water present but zero flow — coil is off
                        0.0
                    }
                } else {
                    // No water loop connected yet — use nominal capacity directly
                    // (plant loop energy is still tracked via boiler fuel consumption)
                    self.nominal_capacity
                };

                // Actual heating is minimum of required, capacity, and water available
                let q_actual = q_required.min(self.nominal_capacity).min(water_capacity);
                let outlet_t = inlet.state.t_db + q_actual / (inlet.mass_flow * cp_air);

                self.heating_rate = q_actual;
                self.energy_consumption = 0.0; // Hot water coils don't consume electricity

                // Calculate water outlet temperature
                if let Some(ref wi) = self.water_inlet {
                    if wi.state.mass_flow > 0.0 {
                        let water_outlet_temp =
                            wi.state.temp - q_actual / (wi.state.mass_flow * wi.state.cp);
                        self.water_outlet = Some(WaterPort::new(FluidState::water(
                            water_outlet_temp,
                            wi.state.mass_flow,
                        )));
                    }
                }

                AirPort::new(
                    psych::MoistAirState::new(outlet_t, inlet.state.w, inlet.state.p_b),
                    inlet.mass_flow,
                )
            }
        }
    }

    fn has_water_side(&self) -> bool {
        matches!(self.coil_type, HeatingCoilType::HotWater)
    }

    fn set_water_inlet(&mut self, inlet: &WaterPort) {
        self.water_inlet = Some(*inlet);
    }

    fn water_outlet(&self) -> Option<WaterPort> {
        self.water_outlet
    }

    fn design_air_flow_rate(&self) -> Option<f64> {
        None // Coils don't set air flow rate
    }

    fn set_setpoint(&mut self, setpoint: f64) {
        self.outlet_temp_setpoint = setpoint;
    }

    fn setpoint(&self) -> Option<f64> {
        Some(self.outlet_temp_setpoint)
    }

    fn nominal_capacity(&self) -> Option<f64> {
        Some(self.nominal_capacity)
    }

    fn set_nominal_capacity(&mut self, cap: f64) {
        self.nominal_capacity = cap;
    }

    fn power_consumption(&self) -> f64 {
        match self.coil_type {
            HeatingCoilType::Electric => self.energy_consumption,
            _ => 0.0,  // gas/HW coils don't consume electricity
        }
    }

    fn fuel_consumption(&self) -> f64 {
        match self.coil_type {
            HeatingCoilType::Gas => self.energy_consumption,
            _ => 0.0,
        }
    }

    fn thermal_output(&self) -> f64 {
        self.heating_rate
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
            outdoor_air: MoistAirState::from_tdb_rh(0.0, 0.5, 101325.0),
            day_type: DayType::WeatherDay,
            is_sizing: false,
            sizing_internal_gains: SizingInternalGains::Full,
        }
    }

    #[test]
    fn test_electric_coil_heats_to_setpoint() {
        let mut coil = HeatingCoil::electric("Test Coil", 50000.0, 35.0);
        let inlet_state = MoistAirState::from_tdb_rh(10.0, 0.5, 101325.0);
        let inlet = AirPort::new(inlet_state, 1.0);
        let ctx = make_ctx();

        let outlet = coil.simulate_air(&inlet, &ctx);

        // Should reach setpoint (coil has enough capacity)
        assert_relative_eq!(outlet.state.t_db, 35.0, max_relative = 0.01);
        assert!(coil.heating_rate > 0.0);
    }

    #[test]
    fn test_electric_coil_capacity_limited() {
        // Very small coil capacity
        let mut coil = HeatingCoil::electric("Small Coil", 1000.0, 35.0);
        let inlet_state = MoistAirState::from_tdb_rh(10.0, 0.5, 101325.0);
        let inlet = AirPort::new(inlet_state, 1.0);
        let ctx = make_ctx();

        let outlet = coil.simulate_air(&inlet, &ctx);

        // Should NOT reach setpoint — capacity limited
        assert!(outlet.state.t_db < 35.0);
        assert!(outlet.state.t_db > 10.0);
        assert_relative_eq!(coil.heating_rate, 1000.0, max_relative = 0.001);
    }

    #[test]
    fn test_coil_no_cooling() {
        // If inlet is already above setpoint, coil should not cool
        let mut coil = HeatingCoil::electric("Test Coil", 50000.0, 20.0);
        let inlet_state = MoistAirState::from_tdb_rh(25.0, 0.5, 101325.0);
        let inlet = AirPort::new(inlet_state, 1.0);
        let ctx = make_ctx();

        let outlet = coil.simulate_air(&inlet, &ctx);

        assert_relative_eq!(outlet.state.t_db, 25.0, max_relative = 0.001);
        assert_eq!(coil.heating_rate, 0.0);
    }
}
