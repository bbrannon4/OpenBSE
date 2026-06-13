//! Heating coil component models.
//!
//! Supports both simple electric and hot water coils.
//! Physics match EnergyPlus HeatingCoils.cc.
//!
//! Hot water coil: Q = m_air * Cp * (T_out - T_in), limited by water-side capacity
//! Electric coil: Q = Capacity * PLR
//!
//! Optional UA-based (NTU-effectiveness) model for hot water coils matches
//! EnergyPlus `Coil:Heating:Water` — cross-flow heat exchanger with both
//! streams unmixed.
//!
//! Reference: EnergyPlus Engineering Reference, Chapter "Coils"

use openbse_core::ports::*;
use openbse_psychrometrics::{self as psych, FluidState};
use serde::{Deserialize, Serialize};

fn default_submeter() -> String {
    "General".to_string()
}

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

/// Rated conditions for the UA-based (NTU-effectiveness) hot water coil model.
///
/// These define the single operating point from which UA_design is derived.
/// Defaults match EnergyPlus `Coil:Heating:Water` defaults.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct UaModelConfig {
    /// Rated water inlet temperature [°C] (default 82.2 / 180°F)
    pub rated_water_inlet_temp: f64,
    /// Rated water outlet temperature [°C] (default 71.1 / 160°F)
    pub rated_water_outlet_temp: f64,
    /// Rated air inlet temperature [°C] (default 16.6 / 61.9°F)
    pub rated_air_inlet_temp: f64,
    /// Rated air outlet temperature [°C] (default 32.2 / 90°F)
    pub rated_air_outlet_temp: f64,
    /// Overall heat transfer coefficient × area [W/K], computed during init
    pub ua_design: f64,
}

impl Default for UaModelConfig {
    fn default() -> Self {
        Self {
            rated_water_inlet_temp: 82.2,
            rated_water_outlet_temp: 71.1,
            rated_air_inlet_temp: 16.6,
            rated_air_outlet_temp: 32.2,
            ua_design: 0.0, // computed from rated capacity
        }
    }
}

impl UaModelConfig {
    /// Compute UA_design from the rated capacity and rated temperature conditions.
    ///
    /// Uses the log-mean temperature difference (LMTD) for a counterflow
    /// arrangement: UA = Q_rated / LMTD.
    pub fn compute_ua(&mut self, rated_capacity: f64) {
        // ΔT at hot end: water_in vs air_out
        let dt1 = self.rated_water_inlet_temp - self.rated_air_outlet_temp;
        // ΔT at cold end: water_out vs air_in
        let dt2 = self.rated_water_outlet_temp - self.rated_air_inlet_temp;

        if dt1 <= 0.0 || dt2 <= 0.0 || rated_capacity <= 0.0 {
            self.ua_design = 0.0;
            return;
        }

        let lmtd = if (dt1 - dt2).abs() < 1e-6 {
            // Degenerate case: both deltas equal → LMTD = ΔT
            dt1
        } else {
            (dt1 - dt2) / (dt1 / dt2).ln()
        };

        self.ua_design = rated_capacity / lmtd;
    }
}

/// Heating coil component.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeatingCoil {
    pub name: String,
    pub coil_type: HeatingCoilType,
    #[serde(default = "default_submeter")]
    pub submeter: String,
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

    /// Optional UA-based (NTU-effectiveness) model config.
    /// When `Some`, the hot water coil uses the cross-flow effectiveness
    /// method instead of the simple capacity-based model.
    pub ua_model: Option<UaModelConfig>,

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
            submeter: "General".to_string(),
            nominal_capacity: capacity,
            efficiency: 1.0,
            outlet_temp_setpoint: setpoint,
            design_water_flow_rate: 0.0,
            design_water_inlet_temp: 0.0,
            design_water_outlet_temp: 0.0,
            ua_model: None,
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
            submeter: "General".to_string(),
            nominal_capacity: capacity,
            efficiency: burner_efficiency,
            outlet_temp_setpoint: setpoint,
            design_water_flow_rate: 0.0,
            design_water_inlet_temp: 0.0,
            design_water_outlet_temp: 0.0,
            ua_model: None,
            heating_rate: 0.0,
            energy_consumption: 0.0,
            water_inlet: None,
            water_outlet: None,
        }
    }

    /// Create a hot water heating coil (simple capacity-based model).
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
            submeter: "General".to_string(),
            nominal_capacity: capacity,
            efficiency: 1.0,
            outlet_temp_setpoint: setpoint,
            design_water_flow_rate: water_flow_rate,
            design_water_inlet_temp: water_inlet_temp,
            design_water_outlet_temp: water_outlet_temp,
            ua_model: None,
            heating_rate: 0.0,
            energy_consumption: 0.0,
            water_inlet: None,
            water_outlet: None,
        }
    }

    /// Create a hot water heating coil with the UA-based NTU-effectiveness model.
    ///
    /// `ua_cfg` contains the rated conditions; UA_design is computed from
    /// `capacity` and the rated temperatures during construction.
    pub fn hot_water_ua(
        name: &str,
        capacity: f64,
        setpoint: f64,
        water_flow_rate: f64,
        water_inlet_temp: f64,
        water_outlet_temp: f64,
        mut ua_cfg: UaModelConfig,
    ) -> Self {
        ua_cfg.compute_ua(capacity);
        Self {
            name: name.to_string(),
            coil_type: HeatingCoilType::HotWater,
            submeter: "General".to_string(),
            nominal_capacity: capacity,
            efficiency: 1.0,
            outlet_temp_setpoint: setpoint,
            design_water_flow_rate: water_flow_rate,
            design_water_inlet_temp: water_inlet_temp,
            design_water_outlet_temp: water_outlet_temp,
            ua_model: Some(ua_cfg),
            heating_rate: 0.0,
            energy_consumption: 0.0,
            water_inlet: None,
            water_outlet: None,
        }
    }

    // ─── Hot water coil: simple capacity-based model ─────────────────

    fn simulate_hot_water_simple(
        &mut self,
        inlet: &AirPort,
        cp_air: f64,
        q_required: f64,
    ) -> AirPort {
        // Calculate water-side available capacity.
        //
        // When the hot water plant loop is coupled (water_inlet is set with
        // real flow from the HHW boiler loop), capacity is limited by the
        // water-side heat available. When the plant loop is not yet connected
        // (water_inlet is None — common in current simulation architecture
        // where plant loops run independently), fall back to nominal_capacity
        // so the coil behaves as if the plant loop always delivers adequate
        // hot water.
        let water_capacity = if let Some(ref wi) = self.water_inlet {
            if wi.state.mass_flow > 0.0 {
                wi.state.mass_flow
                    * wi.state.cp
                    * (wi.state.temp - self.design_water_outlet_temp).max(0.0)
            } else {
                0.0
            }
        } else {
            self.nominal_capacity
        };

        let q_actual = q_required.min(self.nominal_capacity).min(water_capacity);
        let outlet_t = inlet.state.t_db + q_actual / (inlet.mass_flow * cp_air);

        self.heating_rate = q_actual;
        self.energy_consumption = 0.0;

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

    // ─── Hot water coil: UA-based NTU-effectiveness model ────────────

    fn simulate_hot_water_ua(&mut self, inlet: &AirPort, cp_air: f64, q_required: f64) -> AirPort {
        let ua_cfg = self.ua_model.as_ref().unwrap();
        let ua = ua_cfg.ua_design;

        // If UA was never computed or is zero, fall back to simple model
        if ua <= 0.0 {
            return self.simulate_hot_water_simple(inlet, cp_air, q_required);
        }

        // When no water loop is connected, use the simple capacity model
        // (plant loop energy is tracked via boiler fuel consumption)
        let wi = match self.water_inlet {
            Some(ref wi) if wi.state.mass_flow > 1e-10 => *wi,
            Some(_) => {
                // Water present but zero/negligible flow — coil is off
                self.heating_rate = 0.0;
                self.energy_consumption = 0.0;
                return *inlet;
            }
            None => {
                // No water loop connected — fall back to simple model
                // so the coil still functions before plant-air coupling
                return self.simulate_hot_water_simple(inlet, cp_air, q_required);
            }
        };

        let c_air = inlet.mass_flow * cp_air;
        let c_water = wi.state.mass_flow * wi.state.cp;

        if c_air <= 0.0 || c_water <= 0.0 {
            self.heating_rate = 0.0;
            self.energy_consumption = 0.0;
            return *inlet;
        }

        let c_min = c_air.min(c_water);
        let c_max = c_air.max(c_water);
        let c_ratio = c_min / c_max;

        let ntu = ua / c_min;

        // Cross-flow effectiveness, both streams unmixed
        // ε = 1 - exp((NTU^0.78 / C_ratio) * (exp(-C_ratio * NTU^0.22) - 1))
        let effectiveness = if c_ratio < 1e-6 {
            // One stream has much larger capacity (C_ratio → 0):
            // ε = 1 - exp(-NTU)
            1.0 - (-ntu).exp()
        } else {
            let ntu_078 = ntu.powf(0.78);
            let ntu_022 = ntu.powf(0.22);
            1.0 - ((ntu_078 / c_ratio) * ((-c_ratio * ntu_022).exp() - 1.0)).exp()
        };

        let q_max = c_min * (wi.state.temp - inlet.state.t_db);
        if q_max <= 0.0 {
            // Water is colder than air — no heating possible
            self.heating_rate = 0.0;
            self.energy_consumption = 0.0;
            return *inlet;
        }

        let q_hx = effectiveness * q_max;

        // Limit by what's actually required AND by nominal capacity
        let q_actual = q_hx.min(q_required).min(self.nominal_capacity);

        let outlet_t = inlet.state.t_db + q_actual / (inlet.mass_flow * cp_air);

        self.heating_rate = q_actual;
        self.energy_consumption = 0.0;

        // Water outlet temperature
        let water_outlet_temp = wi.state.temp - q_actual / (wi.state.mass_flow * wi.state.cp);
        self.water_outlet = Some(WaterPort::new(FluidState::water(
            water_outlet_temp,
            wi.state.mass_flow,
        )));

        AirPort::new(
            psych::MoistAirState::new(outlet_t, inlet.state.w, inlet.state.p_b),
            inlet.mass_flow,
        )
    }
}

impl AirComponent for HeatingCoil {
    fn name(&self) -> &str {
        &self.name
    }

    fn component_kind(&self) -> ComponentKind {
        ComponentKind::HeatingCoil
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

        // Guard against AUTOSIZE sentinel (-99999) that was never resolved
        if self.nominal_capacity < 0.0 {
            self.heating_rate = 0.0;
            self.energy_consumption = 0.0;
            return *inlet;
        }

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
                // Dispatch to UA-based or simple capacity-based model
                if self.ua_model.is_some() {
                    self.simulate_hot_water_ua(inlet, cp_air, q_required)
                } else {
                    self.simulate_hot_water_simple(inlet, cp_air, q_required)
                }
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
        // Recompute UA when capacity is set (e.g. after autosizing)
        if let Some(ref mut ua_cfg) = self.ua_model {
            ua_cfg.compute_ua(cap);
        }
    }

    fn power_consumption(&self) -> f64 {
        match self.coil_type {
            HeatingCoilType::Electric => self.energy_consumption,
            _ => 0.0, // gas/HW coils don't consume electricity
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

    fn report_outputs(&self, out: &mut dyn FnMut(&str, f64)) {
        out("sensible_load", self.heating_rate);
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

    // ─── UA model tests ──────────────────────────────────────────────

    #[test]
    fn test_ua_compute_from_rated_conditions() {
        let mut cfg = UaModelConfig::default();
        // Rated capacity: Q = m_air * cp * (T_air_out - T_air_in)
        // For defaults: ΔT_air = 32.2 - 16.6 = 15.6 °C
        // ΔT1 = 82.2 - 32.2 = 50.0, ΔT2 = 71.1 - 16.6 = 54.5
        // LMTD = (50 - 54.5) / ln(50/54.5) ≈ 52.22
        let rated_capacity = 10000.0; // 10 kW
        cfg.compute_ua(rated_capacity);

        let dt1: f64 = 82.2 - 32.2; // 50.0
        let dt2: f64 = 71.1 - 16.6; // 54.5
        let lmtd = (dt1 - dt2) / (dt1 / dt2).ln();
        let expected_ua = rated_capacity / lmtd;

        assert_relative_eq!(cfg.ua_design, expected_ua, max_relative = 1e-6);
        assert!(cfg.ua_design > 0.0);
    }

    #[test]
    fn test_ua_model_at_design_conditions() {
        // At rated conditions, the UA model should deliver approximately
        // the rated capacity.
        let rated_capacity = 10000.0; // 10 kW
        let ua_cfg = UaModelConfig::default();

        let mut coil = HeatingCoil::hot_water_ua(
            "UA Test Coil",
            rated_capacity,
            50.0, // high setpoint so coil is not limited by setpoint
            0.001,
            82.2,
            71.1,
            ua_cfg,
        );

        // Calculate water mass flow from rated conditions:
        // Q = m_w * cp_w * (T_w_in - T_w_out) => m_w = Q / (cp_w * ΔT_w)
        let cp_water = 4180.0;
        let m_water = rated_capacity / (cp_water * (82.2 - 71.1));

        // Set water inlet at rated conditions
        coil.set_water_inlet(&WaterPort::new(FluidState::water(82.2, m_water)));

        // Air inlet at rated conditions
        // Q = m_air * cp_air * (T_air_out - T_air_in)
        let cp_air = psych::cp_air_fn_w(0.008);
        let m_air = rated_capacity / (cp_air * (32.2 - 16.6));
        let inlet_state = MoistAirState::new(16.6, 0.008, 101325.0);
        let inlet = AirPort::new(inlet_state, m_air);
        let ctx = make_ctx();

        let outlet = coil.simulate_air(&inlet, &ctx);

        // At design conditions, the cross-flow effectiveness formula gives a
        // slightly different result than the counterflow LMTD used to compute UA
        // (since the coil is modeled as cross-flow, not counterflow). The result
        // should still be reasonably close to rated capacity.
        assert_relative_eq!(coil.heating_rate, rated_capacity, max_relative = 0.15);
        assert!(coil.heating_rate > rated_capacity * 0.80);
        // Air outlet should be in the right neighborhood
        assert!(outlet.state.t_db > 28.0);
        assert!(outlet.state.t_db < 36.0);
    }

    #[test]
    fn test_ua_model_off_design_lower_water_temp() {
        // With lower water inlet temp, the UA model should deliver less heat
        // than the simple capacity model would.
        let rated_capacity = 10000.0;
        let ua_cfg = UaModelConfig::default();

        let mut coil = HeatingCoil::hot_water_ua(
            "UA Off-Design",
            rated_capacity,
            50.0,
            0.001,
            82.2,
            71.1,
            ua_cfg,
        );

        // Water inlet at 60°C instead of rated 82.2°C
        let cp_water = 4180.0;
        let m_water = rated_capacity / (cp_water * (82.2 - 71.1));
        coil.set_water_inlet(&WaterPort::new(FluidState::water(60.0, m_water)));

        let cp_air = psych::cp_air_fn_w(0.008);
        let m_air = rated_capacity / (cp_air * (32.2 - 16.6));
        let inlet_state = MoistAirState::new(16.6, 0.008, 101325.0);
        let inlet = AirPort::new(inlet_state, m_air);
        let ctx = make_ctx();

        let _outlet = coil.simulate_air(&inlet, &ctx);

        // With lower water temp, effectiveness model delivers less heat
        assert!(coil.heating_rate < rated_capacity);
        assert!(coil.heating_rate > 0.0);
    }

    #[test]
    fn test_ua_model_zero_water_flow() {
        let rated_capacity = 10000.0;
        let ua_cfg = UaModelConfig::default();

        let mut coil = HeatingCoil::hot_water_ua(
            "UA Zero Flow",
            rated_capacity,
            50.0,
            0.001,
            82.2,
            71.1,
            ua_cfg,
        );

        // Set water inlet with zero flow
        coil.set_water_inlet(&WaterPort::new(FluidState::water(82.2, 0.0)));

        let inlet_state = MoistAirState::new(16.6, 0.008, 101325.0);
        let inlet = AirPort::new(inlet_state, 0.5);
        let ctx = make_ctx();

        let outlet = coil.simulate_air(&inlet, &ctx);

        // Zero water flow => no heating
        assert_eq!(coil.heating_rate, 0.0);
        assert_relative_eq!(outlet.state.t_db, 16.6, max_relative = 0.001);
    }

    #[test]
    fn test_ua_model_no_water_loop_falls_back() {
        // When no water loop is connected, UA model should fall back to
        // the simple capacity model (backward compatibility)
        let rated_capacity = 10000.0;
        let ua_cfg = UaModelConfig::default();

        let mut coil = HeatingCoil::hot_water_ua(
            "UA Fallback",
            rated_capacity,
            35.0,
            0.001,
            82.2,
            71.1,
            ua_cfg,
        );

        // No water_inlet set — simulate without plant loop
        let inlet_state = MoistAirState::from_tdb_rh(10.0, 0.5, 101325.0);
        let inlet = AirPort::new(inlet_state, 0.3);
        let ctx = make_ctx();

        let outlet = coil.simulate_air(&inlet, &ctx);

        // Should still heat (falls back to simple model)
        assert!(coil.heating_rate > 0.0);
        assert!(outlet.state.t_db > 10.0);
    }

    #[test]
    fn test_ua_model_water_outlet_temp() {
        let rated_capacity = 10000.0;
        let ua_cfg = UaModelConfig::default();

        let mut coil = HeatingCoil::hot_water_ua(
            "UA Water Out",
            rated_capacity,
            50.0,
            0.001,
            82.2,
            71.1,
            ua_cfg,
        );

        let cp_water = 4180.0;
        let m_water = rated_capacity / (cp_water * (82.2 - 71.1));
        coil.set_water_inlet(&WaterPort::new(FluidState::water(82.2, m_water)));

        let cp_air = psych::cp_air_fn_w(0.008);
        let m_air = rated_capacity / (cp_air * (32.2 - 16.6));
        let inlet_state = MoistAirState::new(16.6, 0.008, 101325.0);
        let inlet = AirPort::new(inlet_state, m_air);
        let ctx = make_ctx();

        let _outlet = coil.simulate_air(&inlet, &ctx);

        // Check water outlet exists and temp dropped
        let wo = coil.water_outlet().expect("water outlet should be set");
        assert!(wo.state.temp < 82.2);
        assert!(wo.state.temp > 0.0);
        // Energy balance: Q = m_w * cp_w * (T_w_in - T_w_out)
        let q_from_water = m_water * cp_water * (82.2 - wo.state.temp);
        assert_relative_eq!(q_from_water, coil.heating_rate, max_relative = 0.01);
    }

    #[test]
    fn test_ua_model_capacity_limited() {
        // Even with UA model, nominal capacity should be a hard limit
        let rated_capacity = 1000.0; // very small capacity
        let ua_cfg = UaModelConfig::default();

        let mut coil = HeatingCoil::hot_water_ua(
            "UA Cap Limited",
            rated_capacity,
            50.0,
            0.001,
            82.2,
            71.1,
            ua_cfg,
        );

        // Provide lots of hot water
        coil.set_water_inlet(&WaterPort::new(FluidState::water(82.2, 1.0)));

        let inlet_state = MoistAirState::new(16.6, 0.008, 101325.0);
        let inlet = AirPort::new(inlet_state, 1.0);
        let ctx = make_ctx();

        let _outlet = coil.simulate_air(&inlet, &ctx);

        // Heating rate should not exceed nominal capacity
        assert!(coil.heating_rate <= rated_capacity + 1e-6);
    }

    #[test]
    fn test_ua_model_vs_simple_differ_off_design() {
        // At off-design conditions, the UA model should give a different
        // result than the simple model.
        let rated_capacity = 10000.0;

        // UA model coil
        let mut ua_coil = HeatingCoil::hot_water_ua(
            "UA Coil",
            rated_capacity,
            50.0,
            0.001,
            82.2,
            71.1,
            UaModelConfig::default(),
        );

        // Simple model coil
        let mut simple_coil =
            HeatingCoil::hot_water("Simple Coil", rated_capacity, 50.0, 0.001, 82.2, 71.1);

        // Off-design: moderately lower water temp (still above design_water_outlet
        // of 71°C so the simple model gets nonzero water capacity), higher air flow
        let m_water = 0.5;
        let water_in = WaterPort::new(FluidState::water(75.0, m_water));
        ua_coil.set_water_inlet(&water_in);
        simple_coil.set_water_inlet(&water_in);

        let inlet_state = MoistAirState::new(20.0, 0.008, 101325.0);
        let inlet = AirPort::new(inlet_state, 1.5);
        let ctx = make_ctx();

        let _out_ua = ua_coil.simulate_air(&inlet, &ctx);
        let _out_simple = simple_coil.simulate_air(&inlet, &ctx);

        // Both should heat, but amounts should differ
        assert!(ua_coil.heating_rate > 0.0);
        assert!(simple_coil.heating_rate > 0.0);
        // The UA model uses effectiveness so it should generally give a
        // different (typically lower) result than the simple model at off-design
        let diff = (ua_coil.heating_rate - simple_coil.heating_rate).abs();
        assert!(
            diff > 1.0,
            "UA and simple models should differ at off-design: UA={}, Simple={}",
            ua_coil.heating_rate,
            simple_coil.heating_rate
        );
    }
}
