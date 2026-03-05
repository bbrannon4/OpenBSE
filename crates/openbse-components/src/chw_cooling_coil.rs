//! Chilled water cooling coil component model.
//!
//! Models a chilled water cooling coil connected to a chilled water plant loop.
//! Uses a simple energy balance approach matching the pattern of the hot water
//! heating coil: capacity limited by both air-side demand and water-side available
//! heat transfer.
//!
//! Physics:
//!   Q_sensible = m_air × Cp_air × (T_air_in - T_air_out)
//!   Q_water    = m_water × Cp_water × (T_water_out - T_water_in)
//!   Q_actual   = min(Q_required, nominal_capacity, Q_water_available)
//!
//! The coil receives chilled water from the plant loop (via set_water_inlet)
//! and returns warmer water (via water_outlet). When no plant loop is connected,
//! falls back to nominal_capacity so the air-side simulation remains meaningful.
//!
//! Reference: EnergyPlus Engineering Reference, "Coil:Cooling:Water"

use openbse_core::ports::*;
use openbse_psychrometrics::{self as psych, FluidState};
use serde::{Deserialize, Serialize};

/// Chilled water cooling coil component.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoolingCoilCHW {
    pub name: String,
    /// Nominal cooling capacity [W] (sensible + latent at design conditions).
    /// Use AUTOSIZE for autosizing from zone peak cooling load.
    pub nominal_capacity: f64,
    /// Rated sensible heat ratio [0-1] at design conditions.
    /// Fraction of total cooling that is sensible (typically 0.7-0.85).
    pub rated_shr: f64,
    /// Desired outlet air temperature setpoint [°C]
    pub outlet_temp_setpoint: f64,

    // ─── Water-side design parameters ────────────────────────────────────
    /// Design chilled water flow rate [m³/s]
    pub design_water_flow_rate: f64,
    /// Design chilled water inlet (supply) temperature [°C] (typically 6.7°C / 44°F)
    pub design_water_inlet_temp: f64,
    /// Design chilled water outlet (return) temperature [°C] (typically 12.2°C / 54°F)
    pub design_water_outlet_temp: f64,

    // ─── Runtime state ───────────────────────────────────────────────────
    /// Total cooling rate delivered to air [W] (positive = cooling)
    #[serde(skip)]
    pub cooling_rate: f64,
    /// Sensible cooling rate [W]
    #[serde(skip)]
    pub sensible_cooling_rate: f64,

    #[serde(skip)]
    water_inlet: Option<WaterPort>,
    #[serde(skip)]
    water_outlet_state: Option<WaterPort>,
}

impl CoolingCoilCHW {
    /// Create a new chilled water cooling coil.
    ///
    /// # Arguments
    /// * `name` - Component name
    /// * `capacity` - Nominal cooling capacity [W] (or AUTOSIZE)
    /// * `shr` - Sensible heat ratio (typically 0.7-0.85)
    /// * `setpoint` - Desired outlet air temperature [°C]
    /// * `water_flow_rate` - Design chilled water flow rate [m³/s]
    /// * `water_inlet_temp` - Design CHW supply temperature [°C] (e.g., 6.7)
    /// * `water_outlet_temp` - Design CHW return temperature [°C] (e.g., 12.2)
    pub fn new(
        name: &str,
        capacity: f64,
        shr: f64,
        setpoint: f64,
        water_flow_rate: f64,
        water_inlet_temp: f64,
        water_outlet_temp: f64,
    ) -> Self {
        Self {
            name: name.to_string(),
            nominal_capacity: capacity,
            rated_shr: shr,
            outlet_temp_setpoint: setpoint,
            design_water_flow_rate: water_flow_rate,
            design_water_inlet_temp: water_inlet_temp,
            design_water_outlet_temp: water_outlet_temp,
            cooling_rate: 0.0,
            sensible_cooling_rate: 0.0,
            water_inlet: None,
            water_outlet_state: None,
        }
    }
}

impl AirComponent for CoolingCoilCHW {
    fn name(&self) -> &str {
        &self.name
    }

    fn simulate_air(&mut self, inlet: &AirPort, _ctx: &SimulationContext) -> AirPort {
        if inlet.mass_flow <= 0.0 {
            self.cooling_rate = 0.0;
            self.sensible_cooling_rate = 0.0;
            self.water_outlet_state = None;
            return *inlet;
        }

        let cp_air = psych::cp_air_fn_w(inlet.state.w);

        // Calculate required sensible cooling to reach setpoint
        let q_sensible_required =
            inlet.mass_flow * cp_air * (inlet.state.t_db - self.outlet_temp_setpoint);

        // Only cool, don't heat
        if q_sensible_required <= 0.0 {
            self.cooling_rate = 0.0;
            self.sensible_cooling_rate = 0.0;
            // Pass water through unchanged if present
            if let Some(ref wi) = self.water_inlet {
                self.water_outlet_state = Some(*wi);
            }
            return *inlet;
        }

        // Water-side available capacity
        //
        // When chilled water plant loop is coupled (water_inlet set with real
        // flow from chiller loop), capacity is limited by the water-side heat
        // transfer available. When plant loop is not yet connected (water_inlet
        // is None), fall back to nominal_capacity so the coil behaves as if
        // the plant always delivers adequate chilled water.
        let water_capacity = if let Some(ref wi) = self.water_inlet {
            if wi.state.mass_flow > 0.0 {
                // Water absorbs heat: Q = m_w × cp_w × (T_w_out_max - T_w_in)
                // T_w_out_max is the design return temp (warmer than supply)
                wi.state.mass_flow
                    * wi.state.cp
                    * (self.design_water_outlet_temp - wi.state.temp).max(0.0)
            } else {
                0.0 // Water present but zero flow — coil is off
            }
        } else {
            // No water loop connected yet — use nominal capacity directly
            self.nominal_capacity
        };

        // Sensible capacity is SHR × nominal (or water) capacity
        let shr = self.rated_shr.clamp(0.5, 1.0);
        let available_sensible = (self.nominal_capacity * shr).min(water_capacity * shr);

        // Actual sensible cooling: min of required and available
        let q_sensible = q_sensible_required.min(available_sensible);

        // Total cooling (sensible / SHR)
        let q_total = if shr > 0.0 { q_sensible / shr } else { q_sensible };
        let q_total = q_total.min(self.nominal_capacity).min(water_capacity);

        // Calculate outlet air temperature
        let dt = q_sensible / (inlet.mass_flow * cp_air);
        let outlet_t = inlet.state.t_db - dt;

        self.cooling_rate = q_total;
        self.sensible_cooling_rate = q_sensible;

        // Calculate water outlet temperature (water gets warmer)
        if let Some(ref wi) = self.water_inlet {
            if wi.state.mass_flow > 0.0 {
                let water_outlet_temp =
                    wi.state.temp + q_total / (wi.state.mass_flow * wi.state.cp);
                self.water_outlet_state = Some(WaterPort::new(FluidState::water(
                    water_outlet_temp,
                    wi.state.mass_flow,
                )));
            }
        }

        // Simplified: no dehumidification modeled (humidity ratio passes through)
        AirPort::new(
            psych::MoistAirState::new(outlet_t, inlet.state.w, inlet.state.p_b),
            inlet.mass_flow,
        )
    }

    fn has_water_side(&self) -> bool {
        true
    }

    fn set_water_inlet(&mut self, inlet: &WaterPort) {
        self.water_inlet = Some(*inlet);
    }

    fn water_outlet(&self) -> Option<WaterPort> {
        self.water_outlet_state
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
        0.0 // CHW coils don't consume electricity (pumps and chillers do)
    }

    fn thermal_output(&self) -> f64 {
        self.cooling_rate
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
                month: 7,
                day: 15,
                hour: 14,
                sub_hour: 1,
                timesteps_per_hour: 1,
                sim_time_s: 0.0,
                dt: 3600.0,
            },
            outdoor_air: MoistAirState::from_tdb_rh(35.0, 0.4, 101325.0),
            day_type: DayType::WeatherDay,
            is_sizing: false,
        }
    }

    #[test]
    fn test_chw_coil_cools_to_setpoint() {
        let mut coil = CoolingCoilCHW::new(
            "Test CHW Coil",
            50000.0, // 50 kW capacity
            0.8,     // SHR
            13.0,    // setpoint °C
            0.002,   // water flow m³/s
            6.7,     // CHW supply temp
            12.2,    // CHW return temp
        );
        let inlet_state = MoistAirState::from_tdb_rh(25.0, 0.5, 101325.0);
        let inlet = AirPort::new(inlet_state, 1.0);
        let ctx = make_ctx();

        let outlet = coil.simulate_air(&inlet, &ctx);

        // Should cool to setpoint with sufficient capacity
        assert_relative_eq!(outlet.state.t_db, 13.0, max_relative = 0.01);
        assert!(coil.cooling_rate > 0.0);
        assert!(coil.sensible_cooling_rate > 0.0);
    }

    #[test]
    fn test_chw_coil_no_heating() {
        let mut coil = CoolingCoilCHW::new(
            "Test CHW", 50000.0, 0.8, 13.0, 0.002, 6.7, 12.2,
        );
        let inlet_state = MoistAirState::from_tdb_rh(10.0, 0.5, 101325.0);
        let inlet = AirPort::new(inlet_state, 1.0);
        let ctx = make_ctx();

        let outlet = coil.simulate_air(&inlet, &ctx);

        assert_relative_eq!(outlet.state.t_db, 10.0, max_relative = 0.001);
        assert_eq!(coil.cooling_rate, 0.0);
    }

    #[test]
    fn test_chw_coil_capacity_limited() {
        let mut coil = CoolingCoilCHW::new(
            "Small CHW", 1000.0, 0.8, 13.0, 0.002, 6.7, 12.2,
        );
        let inlet_state = MoistAirState::from_tdb_rh(35.0, 0.4, 101325.0);
        let inlet = AirPort::new(inlet_state, 1.0);
        let ctx = make_ctx();

        let outlet = coil.simulate_air(&inlet, &ctx);

        // Should NOT reach setpoint — capacity limited
        assert!(outlet.state.t_db > 13.0);
        assert!(outlet.state.t_db < 35.0);
    }

    #[test]
    fn test_chw_coil_water_side_limited() {
        let mut coil = CoolingCoilCHW::new(
            "CHW Water Ltd", 50000.0, 0.8, 13.0, 0.001, 6.7, 12.2,
        );

        // Connect water inlet with very low flow — limits capacity
        let water_in = WaterPort::new(FluidState::water(6.7, 0.05)); // 0.05 kg/s
        coil.set_water_inlet(&water_in);

        let inlet_state = MoistAirState::from_tdb_rh(30.0, 0.4, 101325.0);
        let inlet = AirPort::new(inlet_state, 1.0);
        let ctx = make_ctx();

        let outlet = coil.simulate_air(&inlet, &ctx);

        // Water capacity: 0.05 kg/s × 4186 J/(kg·K) × (12.2 - 6.7) = 1151 W
        // Should be capacity-limited by water side
        assert!(outlet.state.t_db > 13.0); // Can't reach setpoint
        assert!(coil.cooling_rate <= 1200.0); // Roughly water-limited

        // Water outlet should be warmer than inlet
        let wo = coil.water_outlet().unwrap();
        assert!(wo.state.temp > 6.7);
        assert!(wo.state.temp <= 12.2 + 0.1); // Should not exceed design return temp significantly
    }

    #[test]
    fn test_chw_coil_no_water_fallback() {
        // Without water connected, should use nominal capacity
        let mut coil = CoolingCoilCHW::new(
            "CHW No Water", 20000.0, 0.8, 13.0, 0.002, 6.7, 12.2,
        );
        let inlet_state = MoistAirState::from_tdb_rh(25.0, 0.5, 101325.0);
        let inlet = AirPort::new(inlet_state, 0.5);
        let ctx = make_ctx();

        let outlet = coil.simulate_air(&inlet, &ctx);

        // Should still cool (using nominal capacity fallback)
        assert!(outlet.state.t_db < 25.0);
        assert!(coil.cooling_rate > 0.0);
    }

    #[test]
    fn test_chw_coil_zero_power() {
        let coil = CoolingCoilCHW::new(
            "CHW", 50000.0, 0.8, 13.0, 0.002, 6.7, 12.2,
        );
        // CHW coils consume no electricity
        assert_eq!(coil.power_consumption(), 0.0);
    }

    #[test]
    fn test_chw_coil_zero_flow() {
        let mut coil = CoolingCoilCHW::new(
            "CHW", 50000.0, 0.8, 13.0, 0.002, 6.7, 12.2,
        );
        let inlet_state = MoistAirState::from_tdb_rh(25.0, 0.5, 101325.0);
        let inlet = AirPort::new(inlet_state, 0.0); // zero airflow
        let ctx = make_ctx();

        let outlet = coil.simulate_air(&inlet, &ctx);

        assert_eq!(coil.cooling_rate, 0.0);
        assert_relative_eq!(outlet.state.t_db, 25.0, max_relative = 0.001);
    }
}
