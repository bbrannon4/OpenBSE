//! VAV terminal box with optional reheat coil.
//!
//! Models a variable air volume terminal unit that modulates primary airflow
//! between minimum and maximum limits, with optional reheat (hot water or
//! electric) when the zone requires heating.
//!
//! Control sequence (dual maximum — ASHRAE Guideline 36 / E+ ReverseWithLimits):
//!   1. Cooling mode: damper opens from minimum toward maximum flow.
//!      Primary air from the central AHU is cold (typically 12-14°C).
//!      More cold air = more cooling.
//!   2. Deadband: damper at minimum position, no reheat.
//!   3. Heating mode: the reheat coil heats the minimum flow up to
//!      max_reheat_temp; a larger load then opens the damper toward
//!      max_reheat_fraction, just enough to deliver it at that temperature.
//!
//! The VAV box does NOT know zone loads directly — it receives a zone load
//! signal via control_signal (positive = heating demand [W], negative =
//! cooling demand [W]) and modulates accordingly.
//!
//! Reference: EnergyPlus Engineering Reference, "AirTerminal:SingleDuct:VAV:Reheat"

use openbse_core::ports::*;
use openbse_psychrometrics::{self as psych, FluidState};
use serde::{Deserialize, Serialize};

fn default_submeter() -> String {
    "General".to_string()
}

/// Reheat coil type for VAV boxes.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum ReheatType {
    /// No reheat coil (cooling-only VAV box)
    None,
    /// Electric resistance reheat
    Electric,
    /// Hot water reheat (connected to HW plant loop)
    HotWater,
}

/// VAV terminal box with optional reheat.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VAVBox {
    pub name: String,
    #[serde(default = "default_submeter")]
    pub submeter: String,
    /// Zone this VAV box serves
    pub zone_name: String,
    /// Maximum primary air flow rate [kg/s]
    pub max_air_flow: f64,
    /// Minimum air flow fraction [0-1] of max (typically 0.3-0.4)
    pub min_flow_fraction: f64,
    /// Reheat coil type
    pub reheat_type: ReheatType,
    /// Reheat coil capacity [W] (used for Electric and HotWater)
    pub reheat_capacity: f64,
    /// Maximum reheat outlet air temperature [°C] (typically 35-50)
    pub max_reheat_temp: f64,
    /// Maximum flow fraction during reheat [0-1] (E+ "ReverseWithLimits").
    /// In heating mode, the damper opens from min_flow_fraction toward this
    /// value to deliver more reheat energy. Typically 0.5 (ASHRAE G36
    /// dual-maximum). Default: 0.5.
    pub max_reheat_fraction: f64,

    // ─── Hot water reheat parameters ─────────────────────────────────────
    /// Design hot water flow rate [m³/s] (for HotWater reheat)
    pub design_hw_flow_rate: f64,
    /// Design hot water inlet temperature [°C] (typically 82°C / 180°F)
    pub design_hw_inlet_temp: f64,
    /// Design hot water outlet temperature [°C] (typically 71°C / 160°F)
    pub design_hw_outlet_temp: f64,

    // ─── Control signal ──────────────────────────────────────────────────
    /// Zone control signal [-1.0 to +1.0]. Positive = heating demand fraction,
    /// negative = cooling demand fraction. Set by the controls framework before
    /// simulate_air is called. The fraction scales the capacity:
    ///   heating: q_reheat = control_signal × reheat_capacity
    ///   cooling: damper opens proportional to |control_signal|
    #[serde(skip)]
    pub control_signal: f64,

    // ─── Runtime state ───────────────────────────────────────────────────
    /// Current damper position [0-1]
    #[serde(skip)]
    pub damper_position: f64,
    /// Current primary air mass flow rate [kg/s]
    #[serde(skip)]
    pub primary_air_flow: f64,
    /// Reheat coil heating rate [W]
    #[serde(skip)]
    pub reheat_rate: f64,
    /// Reheat coil power consumption [W] (electric reheat only)
    #[serde(skip)]
    pub reheat_power: f64,

    #[serde(skip)]
    water_inlet: Option<WaterPort>,
    #[serde(skip)]
    water_outlet_state: Option<WaterPort>,
}

impl VAVBox {
    /// Create a new VAV box.
    ///
    /// # Arguments
    /// * `name` - Component name
    /// * `zone_name` - Name of the zone this box serves
    /// * `max_air_flow` - Maximum primary air flow rate [kg/s]
    /// * `min_flow_fraction` - Minimum flow as fraction of max (0.3-0.4 typical)
    /// * `reheat_type` - Type of reheat coil
    /// * `reheat_capacity` - Reheat coil capacity [W]
    pub fn new(
        name: &str,
        zone_name: &str,
        max_air_flow: f64,
        min_flow_fraction: f64,
        reheat_type: ReheatType,
        reheat_capacity: f64,
    ) -> Self {
        Self {
            name: name.to_string(),
            submeter: "General".to_string(),
            zone_name: zone_name.to_string(),
            max_air_flow,
            min_flow_fraction: min_flow_fraction.clamp(0.0, 1.0),
            reheat_type,
            reheat_capacity,
            max_reheat_temp: 40.0, // E+ default Maximum Reheat Air Temperature
            max_reheat_fraction: 0.5,
            design_hw_flow_rate: 0.0,
            design_hw_inlet_temp: 82.0,
            design_hw_outlet_temp: 71.0,
            control_signal: 0.0,
            damper_position: 0.0,
            primary_air_flow: 0.0,
            reheat_rate: 0.0,
            reheat_power: 0.0,
            water_inlet: None,
            water_outlet_state: None,
        }
    }

    /// Set hot water reheat design parameters.
    pub fn with_hw_reheat_params(
        mut self,
        water_flow_rate: f64,
        water_inlet_temp: f64,
        water_outlet_temp: f64,
    ) -> Self {
        self.design_hw_flow_rate = water_flow_rate;
        self.design_hw_inlet_temp = water_inlet_temp;
        self.design_hw_outlet_temp = water_outlet_temp;
        self
    }

    /// Set maximum reheat air outlet temperature [°C].
    pub fn with_max_reheat_temp(mut self, temp: f64) -> Self {
        self.max_reheat_temp = temp;
        self
    }
}

impl AirComponent for VAVBox {
    fn name(&self) -> &str {
        &self.name
    }

    fn component_kind(&self) -> ComponentKind {
        ComponentKind::HeatingCoil
    }

    fn simulate_air(&mut self, inlet: &AirPort, _ctx: &SimulationContext) -> AirPort {
        self.reheat_rate = 0.0;
        self.reheat_power = 0.0;
        self.water_outlet_state = None;

        if inlet.mass_flow <= 0.0 || self.max_air_flow <= 0.0 {
            self.damper_position = 0.0;
            self.primary_air_flow = 0.0;
            return *inlet;
        }

        let cp_air = psych::cp_air_fn_w(inlet.state.w);
        let min_flow = self.max_air_flow * self.min_flow_fraction;

        // ─── Determine damper position based on zone load signal ─────────
        // control_signal > 0 → zone needs heating → damper at minimum, reheat on
        // control_signal < 0 → zone needs cooling → damper modulates open
        // control_signal = 0 → deadband → damper at minimum

        let (air_flow, needs_reheat) = if self.control_signal < 0.0 {
            // COOLING MODE: modulate damper from min toward max
            // control_signal is -1.0 (full cooling) to 0.0 (no cooling)
            // |control_signal| directly scales the damper position.
            let cooling_frac = (-self.control_signal).clamp(0.0, 1.0);

            // Linear interpolation between min and max flow
            let flow = min_flow + cooling_frac * (self.max_air_flow - min_flow);
            (flow, false)
        } else if self.control_signal > 0.0 {
            // HEATING MODE: dual-maximum (E+ ReverseWithLimits). Reheat first
            // at minimum flow, up to the maximum reheat air temperature; only a
            // larger load opens the damper, as far as the load needs, toward
            // the heating maximum. The flow used to scale with the signal
            // (load ÷ capacity), so a bigger coil opened the damper less and
            // the outlet temperature cap then left the zone short. The
            // heating maximum never drops below the minimum flow, so a
            // constant-volume box (min_flow_fraction = 1.0) holds full flow.
            let heat_max_flow =
                self.max_air_flow * self.max_reheat_fraction.max(self.min_flow_fraction);
            let q_demand = self.control_signal.clamp(0.0, 1.0) * self.reheat_capacity;
            let dt_max = (self.max_reheat_temp - inlet.state.t_db).max(0.1);
            let flow = (q_demand / (cp_air * dt_max)).clamp(min_flow, heat_max_flow);
            (flow, true)
        } else {
            // DEADBAND: minimum flow, no reheat
            (min_flow, false)
        };

        self.primary_air_flow = air_flow;
        self.damper_position = if self.max_air_flow > 0.0 {
            air_flow / self.max_air_flow
        } else {
            0.0
        };

        // Scale inlet mass flow to the VAV box's modulated flow
        // (inlet.mass_flow represents what the AHU provides; VAV box takes
        // only what it needs by damper modulation)
        let actual_flow = air_flow.min(inlet.mass_flow);
        let mut outlet_t = inlet.state.t_db;

        // ─── Reheat coil ─────────────────────────────────────────────────
        if needs_reheat && !matches!(self.reheat_type, ReheatType::None) {
            // control_signal is 0.0 to 1.0 fractional heating demand.
            // Scale by reheat_capacity to get desired heating in Watts.
            let heating_frac = self.control_signal.clamp(0.0, 1.0);
            let q_heating_demand = heating_frac * self.reheat_capacity;

            match self.reheat_type {
                ReheatType::Electric => {
                    let q_actual = q_heating_demand.min(self.reheat_capacity).max(0.0);
                    let dt = q_actual / (actual_flow * cp_air).max(0.001);
                    outlet_t = (inlet.state.t_db + dt).min(self.max_reheat_temp);
                    let q_delivered = actual_flow * cp_air * (outlet_t - inlet.state.t_db);
                    self.reheat_rate = q_delivered.max(0.0);
                    self.reheat_power = self.reheat_rate; // COP = 1.0
                }
                ReheatType::HotWater => {
                    // Water-side available capacity
                    let water_cap = if let Some(ref wi) = self.water_inlet {
                        if wi.state.mass_flow > 0.0 {
                            wi.state.mass_flow
                                * wi.state.cp
                                * (wi.state.temp - self.design_hw_outlet_temp).max(0.0)
                        } else {
                            0.0
                        }
                    } else {
                        // No water loop connected — use nominal capacity
                        self.reheat_capacity
                    };

                    let q_actual = q_heating_demand
                        .min(self.reheat_capacity)
                        .min(water_cap)
                        .max(0.0);
                    let dt = q_actual / (actual_flow * cp_air).max(0.001);
                    outlet_t = (inlet.state.t_db + dt).min(self.max_reheat_temp);
                    let q_delivered = actual_flow * cp_air * (outlet_t - inlet.state.t_db);
                    self.reheat_rate = q_delivered.max(0.0);
                    self.reheat_power = 0.0; // HW coils don't consume electricity

                    // Water outlet temperature
                    if let Some(ref wi) = self.water_inlet {
                        if wi.state.mass_flow > 0.0 {
                            let water_out_t = wi.state.temp
                                - self.reheat_rate / (wi.state.mass_flow * wi.state.cp);
                            self.water_outlet_state = Some(WaterPort::new(FluidState::water(
                                water_out_t,
                                wi.state.mass_flow,
                            )));
                        }
                    }
                }
                ReheatType::None => {} // already handled above
            }
        }

        AirPort::new(
            psych::MoistAirState::new(outlet_t, inlet.state.w, inlet.state.p_b),
            actual_flow,
        )
    }

    fn has_water_side(&self) -> bool {
        matches!(self.reheat_type, ReheatType::HotWater)
    }

    fn set_water_inlet(&mut self, inlet: &WaterPort) {
        self.water_inlet = Some(*inlet);
    }

    fn water_outlet(&self) -> Option<WaterPort> {
        self.water_outlet_state
    }

    fn design_air_flow_rate(&self) -> Option<f64> {
        if openbse_core::types::is_autosize(self.max_air_flow) {
            None
        } else {
            Some(self.max_air_flow)
        }
    }

    fn set_design_air_flow_rate(&mut self, flow: f64) {
        self.max_air_flow = flow;
    }

    fn set_setpoint(&mut self, signal: f64) {
        // For VAV boxes, "setpoint" is overloaded as the zone load signal
        self.control_signal = signal;
    }

    fn setpoint(&self) -> Option<f64> {
        Some(self.control_signal)
    }

    fn nominal_capacity(&self) -> Option<f64> {
        Some(self.reheat_capacity)
    }

    fn set_nominal_capacity(&mut self, cap: f64) {
        self.reheat_capacity = cap;
    }

    fn unsized_fields(&self) -> Vec<&'static str> {
        openbse_core::types::autosize_fields([
            ("max_air_flow", self.max_air_flow),
            ("reheat_capacity", self.reheat_capacity),
        ])
    }

    fn reheat_primary_air_flow(&self) -> Option<f64> {
        // While reheating the damper opens to the heating maximum, never below
        // the minimum (EnergyPlus ReverseWithLimits reheat flow).
        if openbse_core::types::is_autosize(self.max_air_flow) {
            return None;
        }
        Some(self.max_air_flow * self.max_reheat_fraction.max(self.min_flow_fraction))
    }

    fn reheat_for_zone_load(&self, zone_load: f64, zone_temp: f64, inlet_temp: f64) -> Option<f64> {
        if openbse_core::types::is_autosize(self.max_air_flow) || self.max_air_flow <= 0.0 {
            return None;
        }
        let cp_air = 1006.0;
        let min_flow = self.max_air_flow * self.min_flow_fraction;
        let heat_max_flow = self.reheat_primary_air_flow().unwrap_or(min_flow);
        // At minimum flow the coil supplies the load and warms the primary
        // air to the zone; a negative (deadband) load offsets that, so the
        // coil only stops the zone being cooled below setpoint. If the result
        // would exceed the reheat limit, the damper opens instead
        // (ReverseWithLimits): air at the limit, just enough of it to carry
        // the load, which `simulate_air` reproduces from this demand.
        let at_min = zone_load + min_flow * cp_air * (zone_temp - inlet_temp);
        if at_min.is_nan() || at_min <= 0.0 {
            return Some(0.0);
        }
        if min_flow > 0.0 && inlet_temp + at_min / (min_flow * cp_air) <= self.max_reheat_temp {
            return Some(at_min);
        }
        let flow = if self.max_reheat_temp > zone_temp {
            (zone_load.max(0.0) / (cp_air * (self.max_reheat_temp - zone_temp)))
                .clamp(min_flow, heat_max_flow)
        } else {
            heat_max_flow
        };
        Some((flow * cp_air * (self.max_reheat_temp - inlet_temp)).max(0.0))
    }

    fn power_consumption(&self) -> f64 {
        self.reheat_power
    }

    fn thermal_output(&self) -> f64 {
        self.reheat_rate
    }

    fn report_outputs(&self, out: &mut dyn FnMut(&str, f64)) {
        out("sensible_load", self.reheat_rate);
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
            sizing_internal_gains: SizingInternalGains::Full,
        }
    }

    fn make_supply_air(temp: f64, flow: f64) -> AirPort {
        AirPort::new(MoistAirState::from_tdb_rh(temp, 0.5, 101325.0), flow)
    }

    #[test]
    fn reheat_primary_flow_is_the_heating_maximum() {
        // Default heating maximum 0.5 above a 0.3 minimum.
        let vav = VAVBox::new("V", "Z", 2.0, 0.3, ReheatType::HotWater, 0.0);
        assert_relative_eq!(vav.reheat_primary_air_flow().unwrap(), 1.0);
        // A constant-volume box reheats its full flow.
        let cav = VAVBox::new("V", "Z", 2.0, 1.0, ReheatType::HotWater, 0.0);
        assert_relative_eq!(cav.reheat_primary_air_flow().unwrap(), 2.0);
        // Not sized yet: nothing to report.
        let autosized = VAVBox::new(
            "V",
            "Z",
            openbse_core::types::AUTOSIZE,
            0.3,
            ReheatType::HotWater,
            0.0,
        );
        assert!(autosized.reheat_primary_air_flow().is_none());
    }

    #[test]
    fn test_vav_cooling_mode_increases_flow() {
        let mut vav = VAVBox::new("Zone1 VAV", "Zone1", 1.0, 0.3, ReheatType::Electric, 5000.0);

        // Supply air at 13°C, max flow 1.0 kg/s
        let inlet = make_supply_air(13.0, 2.0); // AHU provides plenty
        let ctx = make_ctx();

        // Light cooling demand (20% signal)
        vav.control_signal = -0.2;
        let out1 = vav.simulate_air(&inlet, &ctx);
        let flow1 = out1.mass_flow;

        // Heavy cooling demand (100% signal)
        vav.control_signal = -1.0;
        let out2 = vav.simulate_air(&inlet, &ctx);
        let flow2 = out2.mass_flow;

        // Heavier cooling → more air flow
        assert!(flow2 > flow1);
        // Both above minimum
        assert!(flow1 >= 0.3 * 1.0 - 0.001);
        // Heavy demand should be at or near max
        assert!(flow2 >= 0.8 * 1.0);
    }

    #[test]
    fn test_vav_deadband_minimum_flow() {
        let mut vav = VAVBox::new("Zone1 VAV", "Zone1", 1.0, 0.3, ReheatType::None, 0.0);

        let inlet = make_supply_air(13.0, 2.0);
        let ctx = make_ctx();

        vav.control_signal = 0.0; // deadband
        let outlet = vav.simulate_air(&inlet, &ctx);

        // Should be at minimum flow
        assert_relative_eq!(outlet.mass_flow, 0.3, max_relative = 0.01);
        assert_eq!(vav.reheat_rate, 0.0);
    }

    #[test]
    fn test_vav_heating_mode_with_electric_reheat() {
        let mut vav = VAVBox::new("Zone1 VAV", "Zone1", 1.0, 0.3, ReheatType::Electric, 5000.0);

        let inlet = make_supply_air(13.0, 2.0);
        let ctx = make_ctx();

        // Heating demand (60% of capacity = 3 kW)
        vav.control_signal = 0.6;
        let outlet = vav.simulate_air(&inlet, &ctx);

        // Dual-maximum control (ASHRAE G36 / E+ ReverseWithLimits): 3 kW
        // fits within the minimum flow at the 40 °C reheat limit, so the
        // damper stays at minimum and the coil does the work.
        assert_relative_eq!(outlet.mass_flow, 0.3, max_relative = 0.01);
        assert!(outlet.state.t_db > 13.0 && outlet.state.t_db < 40.0);
        assert_relative_eq!(vav.reheat_rate, 3000.0, max_relative = 0.01);
        assert!(vav.reheat_power > 0.0); // Electric reheat consumes power
    }

    #[test]
    fn reheat_for_zone_load_delivers_the_load_to_the_zone() {
        let cp = 1006.0;
        let capacity = 20_000.0;
        let inlet = make_supply_air(13.0, 2.0);
        let ctx = make_ctx();
        let mut vav = VAVBox::new("V", "Z", 1.0, 0.3, ReheatType::Electric, capacity);

        // 3 kW at minimum flow: warm 0.3 kg/s from 13 to 21 °C, plus the load.
        let q = vav.reheat_for_zone_load(3000.0, 21.0, 13.0).unwrap();
        assert_relative_eq!(q, 3000.0 + 0.3 * cp * 8.0, max_relative = 1e-9);
        vav.control_signal = q / capacity;
        let outlet = vav.simulate_air(&inlet, &ctx);
        let cp_moist = psych::cp_air_fn_w(inlet.state.w);
        let to_zone = outlet.mass_flow * cp_moist * (outlet.state.t_db - 21.0);
        assert_relative_eq!(to_zone, 3000.0, max_relative = 0.01);

        // A zone with no load of its own, overcooled to 19 °C by the minimum
        // flow, still gets its primary air warmed to room temperature.
        let q = vav.reheat_for_zone_load(0.0, 19.0, 13.0).unwrap();
        assert_relative_eq!(q, 0.3 * cp * 6.0, max_relative = 1e-9);

        // Deadband: the zone can lose 1 kW before reaching setpoint, so the
        // coil adds only what the minimum flow would take beyond that...
        let q = vav.reheat_for_zone_load(-1000.0, 21.0, 13.0).unwrap();
        assert_relative_eq!(q, 0.3 * cp * 8.0 - 1000.0, max_relative = 1e-9);
        // ...and nothing when the zone can absorb all of it.
        assert_eq!(vav.reheat_for_zone_load(-5000.0, 21.0, 13.0), Some(0.0));
        // No heating setpoint at all: never reheat.
        assert_eq!(
            vav.reheat_for_zone_load(f64::NEG_INFINITY, 21.0, 13.0),
            Some(0.0)
        );

        // 12 kW would need 60 °C at minimum flow: the damper opens to the 0.5
        // heating maximum at the 40 °C limit, which is all the box can give.
        let q = vav.reheat_for_zone_load(12_000.0, 21.0, 13.0).unwrap();
        assert_relative_eq!(q, 0.5 * cp * 27.0, max_relative = 1e-9);
    }

    #[test]
    fn heating_opens_the_damper_only_past_the_reheat_limit() {
        // 12 kW will not fit through the 0.3 kg/s minimum below 40 °C, so the
        // damper opens just enough (toward the 0.5 heating maximum) to carry
        // it at 40 °C. The opening depends on the load, not on how large the
        // coil is.
        let inlet = make_supply_air(13.0, 2.0);
        let ctx = make_ctx();
        for (capacity, signal) in [(20_000.0, 0.6), (40_000.0, 0.3)] {
            let mut vav = VAVBox::new("V", "Z", 1.0, 0.3, ReheatType::Electric, capacity);
            vav.control_signal = signal;
            let outlet = vav.simulate_air(&inlet, &ctx);
            let cp = psych::cp_air_fn_w(inlet.state.w);
            let flow = 12_000.0 / (cp * (40.0 - 13.0));
            assert_relative_eq!(outlet.mass_flow, flow, max_relative = 1e-6);
            assert_relative_eq!(outlet.state.t_db, 40.0, max_relative = 1e-6);
            assert_relative_eq!(vav.reheat_rate, 12_000.0, max_relative = 1e-6);
        }
    }

    #[test]
    fn test_vav_heating_mode_with_hw_reheat() {
        let mut vav = VAVBox::new("Zone1 VAV", "Zone1", 1.0, 0.3, ReheatType::HotWater, 5000.0)
            .with_hw_reheat_params(0.001, 82.0, 71.0);

        // Connect hot water
        let hw = WaterPort::new(FluidState::water(82.0, 0.1));
        vav.set_water_inlet(&hw);

        let inlet = make_supply_air(13.0, 2.0);
        let ctx = make_ctx();

        vav.control_signal = 0.6; // 60% of reheat capacity
        let outlet = vav.simulate_air(&inlet, &ctx);

        assert!(outlet.state.t_db > 13.0);
        assert!(vav.reheat_rate > 0.0);
        assert_eq!(vav.reheat_power, 0.0); // HW reheat = no electricity

        // Water outlet should be cooler than inlet
        let wo = vav.water_outlet().unwrap();
        assert!(wo.state.temp < 82.0);
    }

    #[test]
    fn test_vav_max_reheat_temp_limit() {
        let mut vav = VAVBox::new(
            "Zone1 VAV",
            "Zone1",
            0.1,
            0.3,
            ReheatType::Electric,
            50000.0,
        )
        .with_max_reheat_temp(35.0);

        let inlet = make_supply_air(13.0, 2.0);
        let ctx = make_ctx();

        // Full heating demand (100% of capacity) with small airflow
        vav.control_signal = 1.0;
        let outlet = vav.simulate_air(&inlet, &ctx);

        // Should not exceed max reheat temp
        assert!(outlet.state.t_db <= 35.1);
    }

    #[test]
    fn test_vav_no_reheat_type_none() {
        let mut vav = VAVBox::new("Zone1 VAV", "Zone1", 1.0, 0.3, ReheatType::None, 0.0);

        let inlet = make_supply_air(13.0, 2.0);
        let ctx = make_ctx();

        vav.control_signal = 0.5; // heating demand (fractional)
        let outlet = vav.simulate_air(&inlet, &ctx);

        // No reheat — air passes through at supply temp
        assert_relative_eq!(outlet.state.t_db, 13.0, max_relative = 0.001);
        assert_eq!(vav.reheat_rate, 0.0);
    }

    #[test]
    fn test_vav_zero_inlet_flow() {
        let mut vav = VAVBox::new("Zone1 VAV", "Zone1", 1.0, 0.3, ReheatType::Electric, 5000.0);

        let inlet = make_supply_air(13.0, 0.0);
        let ctx = make_ctx();

        vav.control_signal = -0.5;
        let outlet = vav.simulate_air(&inlet, &ctx);

        assert_eq!(outlet.mass_flow, 0.0);
        assert_eq!(vav.primary_air_flow, 0.0);
    }
}
