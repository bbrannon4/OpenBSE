//! Parallel fan-powered (PFP) terminal box component model.
//!
//! Models a parallel fan-powered VAV terminal unit as used in ASHRAE 90.1
//! Appendix G Systems 6 and 8. The PFP box has:
//!   - A primary air damper (from central AHU, cold supply air)
//!   - A secondary (parallel) fan that draws plenum/return air
//!   - An electric resistance reheat coil downstream of the mixing point
//!
//! Control sequence:
//!   1. Cooling mode: primary damper modulates open (more cold AHU air),
//!      secondary fan OFF, reheat OFF.
//!   2. Deadband: primary at minimum, secondary fan OFF, reheat OFF.
//!   3. Heating mode: primary at minimum, secondary fan ON (draws warm
//!      plenum air), reheat coil modulates to meet heating load.
//!      The secondary fan provides constant flow of warm return/plenum air
//!      that mixes with the minimum primary cold air before entering the
//!      reheat coil.
//!
//! Power consumption: secondary fan power + electric reheat power.
//!
//! Reference: EnergyPlus Engineering Reference,
//!   "AirTerminal:SingleDuct:ParallelPIU:Reheat"

use openbse_core::ports::*;
use openbse_psychrometrics::{self as psych};
use serde::{Deserialize, Serialize};

/// Parallel fan-powered (PFP) terminal box with electric reheat.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PFPBox {
    pub name: String,
    /// Zone this PFP box serves
    pub zone_name: String,

    // ─── Primary air (from central AHU) ──────────────────────────────────
    /// Maximum primary air flow rate [kg/s]
    pub max_primary_flow: f64,
    /// Minimum primary air flow fraction [0-1] of max (typically 0.3-0.4)
    pub min_primary_fraction: f64,

    // ─── Secondary fan (parallel fan drawing plenum air) ─────────────────
    /// Secondary fan flow rate [kg/s] (constant when operating)
    pub secondary_fan_flow: f64,
    /// Secondary fan pressure rise [Pa] (typically 250-500 Pa)
    pub secondary_fan_pressure: f64,
    /// Secondary fan total efficiency (motor + impeller, typically 0.5-0.65)
    pub secondary_fan_efficiency: f64,
    /// Secondary (plenum/return) air temperature [°C].
    /// In a real building this comes from the return air plenum.
    /// Default: 24°C (typical return air temperature).
    pub secondary_air_temp: f64,

    // ─── Electric reheat coil ────────────────────────────────────────────
    /// Reheat coil capacity [W]
    pub reheat_capacity: f64,
    /// Maximum reheat outlet air temperature [°C] (typically 35-50)
    pub max_reheat_temp: f64,

    // ─── Control signal ──────────────────────────────────────────────────
    /// Zone control signal [-1.0 to +1.0]. Positive = heating demand fraction,
    /// negative = cooling demand fraction.
    #[serde(skip)]
    pub control_signal: f64,

    // ─── Runtime state ───────────────────────────────────────────────────
    /// Primary air mass flow rate [kg/s]
    #[serde(skip)]
    pub primary_air_flow: f64,
    /// Secondary air mass flow rate [kg/s]
    #[serde(skip)]
    pub secondary_air_flow: f64,
    /// Total outlet air mass flow rate [kg/s] (primary + secondary)
    #[serde(skip)]
    pub total_air_flow: f64,
    /// Primary damper position [0-1]
    #[serde(skip)]
    pub damper_position: f64,
    /// Secondary fan power [W]
    #[serde(skip)]
    pub fan_power: f64,
    /// Reheat coil heating rate [W]
    #[serde(skip)]
    pub reheat_rate: f64,
    /// Reheat coil electric power [W]
    #[serde(skip)]
    pub reheat_power: f64,
}

impl PFPBox {
    /// Create a new parallel fan-powered box.
    ///
    /// # Arguments
    /// * `name` - Component name
    /// * `zone_name` - Name of the zone this box serves
    /// * `max_primary_flow` - Maximum primary air flow from AHU [kg/s]
    /// * `min_primary_fraction` - Minimum primary flow fraction [0-1]
    /// * `secondary_fan_flow` - Secondary fan constant flow rate [kg/s]
    /// * `reheat_capacity` - Electric reheat capacity [W]
    pub fn new(
        name: &str,
        zone_name: &str,
        max_primary_flow: f64,
        min_primary_fraction: f64,
        secondary_fan_flow: f64,
        reheat_capacity: f64,
    ) -> Self {
        Self {
            name: name.to_string(),
            zone_name: zone_name.to_string(),
            max_primary_flow,
            min_primary_fraction: min_primary_fraction.clamp(0.0, 1.0),
            secondary_fan_flow,
            secondary_fan_pressure: 375.0,   // typical PFP fan
            secondary_fan_efficiency: 0.55,
            secondary_air_temp: 24.0,
            reheat_capacity,
            max_reheat_temp: 35.0,
            control_signal: 0.0,
            primary_air_flow: 0.0,
            secondary_air_flow: 0.0,
            total_air_flow: 0.0,
            damper_position: 0.0,
            fan_power: 0.0,
            reheat_rate: 0.0,
            reheat_power: 0.0,
        }
    }

    /// Set secondary fan parameters.
    pub fn with_fan_params(mut self, pressure: f64, efficiency: f64) -> Self {
        self.secondary_fan_pressure = pressure;
        self.secondary_fan_efficiency = efficiency;
        self
    }

    /// Set secondary (plenum/return) air temperature [°C].
    pub fn with_secondary_air_temp(mut self, temp: f64) -> Self {
        self.secondary_air_temp = temp;
        self
    }

    /// Set maximum reheat outlet temperature [°C].
    pub fn with_max_reheat_temp(mut self, temp: f64) -> Self {
        self.max_reheat_temp = temp;
        self
    }

    /// Calculate secondary fan power [W].
    fn secondary_fan_power(&self) -> f64 {
        if self.secondary_fan_efficiency > 0.0 && self.secondary_fan_flow > 0.0 {
            // P = V_dot × ΔP / η, but we have mass flow, so:
            // P ≈ m_dot × ΔP / (ρ × η), with ρ_air ≈ 1.2 kg/m³
            let rho_air = 1.2;
            self.secondary_fan_flow * self.secondary_fan_pressure
                / (rho_air * self.secondary_fan_efficiency)
        } else {
            0.0
        }
    }
}

impl AirComponent for PFPBox {
    fn name(&self) -> &str {
        &self.name
    }

    fn simulate_air(&mut self, inlet: &AirPort, _ctx: &SimulationContext) -> AirPort {
        // Reset runtime state
        self.fan_power = 0.0;
        self.reheat_rate = 0.0;
        self.reheat_power = 0.0;
        self.secondary_air_flow = 0.0;

        if inlet.mass_flow <= 0.0 || self.max_primary_flow <= 0.0 {
            self.primary_air_flow = 0.0;
            self.total_air_flow = 0.0;
            self.damper_position = 0.0;
            return *inlet;
        }

        let cp_air = psych::cp_air_fn_w(inlet.state.w);
        let min_primary = self.max_primary_flow * self.min_primary_fraction;

        // ─── Determine mode based on zone load signal ────────────────────
        let secondary_fan_on: bool;
        let primary_flow: f64;

        if self.control_signal < 0.0 {
            // COOLING MODE: modulate primary damper, secondary fan OFF
            // control_signal is -1.0 (full cooling) to 0.0 (no cooling)
            secondary_fan_on = false;

            let cooling_frac = (-self.control_signal).clamp(0.0, 1.0);
            primary_flow = min_primary + cooling_frac * (self.max_primary_flow - min_primary);
        } else if self.control_signal > 0.0 {
            // HEATING MODE: primary at minimum, secondary fan ON, reheat on
            secondary_fan_on = true;
            primary_flow = min_primary;
        } else {
            // DEADBAND: primary at minimum, secondary fan OFF
            secondary_fan_on = false;
            primary_flow = min_primary;
        }

        self.primary_air_flow = primary_flow.min(inlet.mass_flow);
        self.damper_position = if self.max_primary_flow > 0.0 {
            self.primary_air_flow / self.max_primary_flow
        } else {
            0.0
        };

        // ─── Mix primary and secondary air ───────────────────────────────
        let mut mixed_flow = self.primary_air_flow;
        let mut mixed_temp = inlet.state.t_db;
        let mut mixed_w = inlet.state.w;

        if secondary_fan_on && self.secondary_fan_flow > 0.0 {
            self.secondary_air_flow = self.secondary_fan_flow;
            self.fan_power = self.secondary_fan_power();

            // Mix primary (cold from AHU) + secondary (warm from plenum)
            let total = self.primary_air_flow + self.secondary_air_flow;
            if total > 0.0 {
                mixed_temp = (self.primary_air_flow * inlet.state.t_db
                    + self.secondary_air_flow * self.secondary_air_temp)
                    / total;
                // Simplified: use primary humidity for mixed (plenum similar)
                mixed_w = inlet.state.w;
            }
            mixed_flow = total;

            // Fan heat addition to mixed air
            let fan_heat_rise = if mixed_flow > 0.0 && cp_air > 0.0 {
                self.fan_power / (mixed_flow * cp_air)
            } else {
                0.0
            };
            mixed_temp += fan_heat_rise;
        }

        self.total_air_flow = mixed_flow;

        // ─── Electric reheat coil ────────────────────────────────────────
        let mut outlet_t = mixed_temp;

        if self.control_signal > 0.0 && self.reheat_capacity > 0.0 {
            let heating_frac = self.control_signal.clamp(0.0, 1.0);
            let q_heating_demand = heating_frac * self.reheat_capacity;
            let q_actual = q_heating_demand.min(self.reheat_capacity).max(0.0);
            let dt = q_actual / (mixed_flow * cp_air).max(0.001);
            outlet_t = (mixed_temp + dt).min(self.max_reheat_temp);
            let q_delivered = mixed_flow * cp_air * (outlet_t - mixed_temp);
            self.reheat_rate = q_delivered.max(0.0);
            self.reheat_power = self.reheat_rate; // Electric: COP = 1.0
        }

        AirPort::new(
            psych::MoistAirState::new(outlet_t, mixed_w, inlet.state.p_b),
            mixed_flow,
        )
    }

    fn has_water_side(&self) -> bool {
        false // PFP boxes use electric reheat only (per 90.1 Appendix G)
    }

    fn design_air_flow_rate(&self) -> Option<f64> {
        if openbse_core::types::is_autosize(self.max_primary_flow) {
            None
        } else {
            Some(self.max_primary_flow)
        }
    }

    fn set_design_air_flow_rate(&mut self, flow: f64) {
        self.max_primary_flow = flow;
    }

    fn set_setpoint(&mut self, signal: f64) {
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

    fn power_consumption(&self) -> f64 {
        self.fan_power + self.reheat_power
    }

    fn thermal_output(&self) -> f64 {
        self.reheat_rate
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
        AirPort::new(
            MoistAirState::from_tdb_rh(temp, 0.5, 101325.0),
            flow,
        )
    }

    #[test]
    fn test_pfp_cooling_mode_no_secondary_fan() {
        let mut pfp = PFPBox::new("Zone1 PFP", "Zone1", 1.0, 0.3, 0.5, 5000.0);

        let inlet = make_supply_air(13.0, 2.0);
        let ctx = make_ctx();

        pfp.control_signal = -0.5; // cooling (50% signal)
        let outlet = pfp.simulate_air(&inlet, &ctx);

        // Secondary fan should be OFF in cooling mode
        assert_eq!(pfp.secondary_air_flow, 0.0);
        assert_eq!(pfp.fan_power, 0.0);
        // Air should be cold supply only
        assert_relative_eq!(outlet.state.t_db, 13.0, max_relative = 0.01);
        // Flow should be above minimum
        assert!(outlet.mass_flow > 0.3);
    }

    #[test]
    fn test_pfp_heating_mode_secondary_fan_on() {
        let mut pfp = PFPBox::new("Zone1 PFP", "Zone1", 1.0, 0.3, 0.5, 10000.0)
            .with_secondary_air_temp(24.0);

        let inlet = make_supply_air(13.0, 2.0);
        let ctx = make_ctx();

        pfp.control_signal = 0.6; // heating (60% signal)
        let outlet = pfp.simulate_air(&inlet, &ctx);

        // Secondary fan should be ON
        assert!(pfp.secondary_air_flow > 0.0);
        assert!(pfp.fan_power > 0.0);
        // Mixed temp should be between primary (13C) and secondary (24C)
        // Then reheat pushes it higher
        assert!(outlet.state.t_db > 13.0);
        // Total flow = primary (0.3) + secondary (0.5)
        assert_relative_eq!(outlet.mass_flow, 0.3 + 0.5, max_relative = 0.01);
        // Reheat should be active
        assert!(pfp.reheat_rate > 0.0);
        assert!(pfp.reheat_power > 0.0);
    }

    #[test]
    fn test_pfp_deadband_minimum_flow() {
        let mut pfp = PFPBox::new("Zone1 PFP", "Zone1", 1.0, 0.3, 0.5, 5000.0);

        let inlet = make_supply_air(13.0, 2.0);
        let ctx = make_ctx();

        pfp.control_signal = 0.0; // deadband
        let outlet = pfp.simulate_air(&inlet, &ctx);

        // Primary at minimum, no secondary fan
        assert_relative_eq!(outlet.mass_flow, 0.3, max_relative = 0.01);
        assert_eq!(pfp.secondary_air_flow, 0.0);
        assert_eq!(pfp.fan_power, 0.0);
        assert_eq!(pfp.reheat_rate, 0.0);
    }

    #[test]
    fn test_pfp_mixed_air_temperature() {
        let mut pfp = PFPBox::new("Zone1 PFP", "Zone1", 1.0, 0.3, 0.7, 0.0) // no reheat
            .with_secondary_air_temp(24.0);

        let inlet = make_supply_air(13.0, 2.0);
        let ctx = make_ctx();

        pfp.control_signal = 0.5; // heating (but no reheat capacity)
        let outlet = pfp.simulate_air(&inlet, &ctx);

        // Mixed temp: (0.3 × 13 + 0.7 × 24) / 1.0 = (3.9 + 16.8) / 1.0 = 20.7°C + fan heat
        let expected_mixed = (0.3 * 13.0 + 0.7 * 24.0) / (0.3 + 0.7);
        // Should be close to expected (plus small fan heat addition)
        assert!(outlet.state.t_db > expected_mixed - 0.5);
        assert!(outlet.state.t_db < expected_mixed + 2.0); // fan heat adds ~1°C
    }

    #[test]
    fn test_pfp_max_reheat_temp_limit() {
        let mut pfp = PFPBox::new("Zone1 PFP", "Zone1", 0.1, 1.0, 0.1, 50000.0)
            .with_max_reheat_temp(35.0);

        let inlet = make_supply_air(13.0, 2.0);
        let ctx = make_ctx();

        pfp.control_signal = 1.0; // full heating demand
        let outlet = pfp.simulate_air(&inlet, &ctx);

        // Should not exceed max reheat temp
        assert!(outlet.state.t_db <= 35.1);
    }

    #[test]
    fn test_pfp_power_consumption() {
        let mut pfp = PFPBox::new("Zone1 PFP", "Zone1", 1.0, 0.3, 0.5, 5000.0);

        let inlet = make_supply_air(13.0, 2.0);
        let ctx = make_ctx();

        // Cooling: only primary flow, no fan or reheat power
        pfp.control_signal = -0.5;
        pfp.simulate_air(&inlet, &ctx);
        assert_eq!(pfp.power_consumption(), 0.0);

        // Heating: fan + reheat power
        pfp.control_signal = 0.6;
        pfp.simulate_air(&inlet, &ctx);
        assert!(pfp.power_consumption() > 0.0);
        assert!(pfp.fan_power > 0.0);
    }

    #[test]
    fn test_pfp_zero_inlet_flow() {
        let mut pfp = PFPBox::new("Zone1 PFP", "Zone1", 1.0, 0.3, 0.5, 5000.0);

        let inlet = make_supply_air(13.0, 0.0);
        let ctx = make_ctx();

        pfp.control_signal = -0.5;
        let outlet = pfp.simulate_air(&inlet, &ctx);

        assert_eq!(outlet.mass_flow, 0.0);
        assert_eq!(pfp.primary_air_flow, 0.0);
    }
}
