//! Electric steam humidifier component for air loops.
//!
//! Models an electric steam humidifier that adds moisture to the supply air
//! when the supply air humidity ratio is below the setpoint.
//!
//! Physics:
//!   - Steam generation: rated_power converts water to steam
//!   - Moisture added: Δw = power × efficiency / (h_fg × mass_flow_air)
//!   - Part-load: power = rated_power × (actual_steam / rated_capacity)
//!
//! Control: maintains a minimum humidity ratio at the outlet. The setpoint
//! is typically derived from a zone minimum RH requirement (e.g., 30% RH).
//!
//! Reference: EnergyPlus Engineering Reference, "Humidifiers"

use openbse_core::ports::*;
use openbse_core::types::*;
use openbse_psychrometrics as psych;
use serde::{Deserialize, Serialize};

/// Total energy to generate 1 kg of steam from inlet water [J/kg].
///
/// Includes heating water from ~14°C to 100°C plus latent heat of vaporization.
/// h_steam(100°C) = 2,676,100 J/kg, h_water(14.4°C) ≈ 60,400 J/kg
/// Δh = 2,615,700 J/kg.
///
/// Reference: EnergyPlus Engineering Reference, "Humidifiers" — uses the same
/// enthalpy difference for sizing the rated steam capacity from rated power.
const H_STEAM_TOTAL: f64 = 2_615_700.0;

/// Electric steam humidifier.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Humidifier {
    pub name: String,
    /// Rated (maximum) electric power input [W].
    pub rated_power: f64,
    /// Rated (maximum) steam output capacity [kg/s].
    /// Autosized from rated_power / H_STEAM_TOTAL if not set explicitly.
    pub rated_capacity: f64,
    /// Minimum humidity ratio setpoint [kg_water/kg_dry_air].
    /// The humidifier adds moisture when the supply air w < this setpoint.
    /// Set to 0.0 to derive from min_rh_setpoint at each timestep.
    pub w_setpoint: f64,
    /// Minimum relative humidity setpoint [0-1] (e.g., 0.30 for 30% RH).
    /// Used only when w_setpoint == 0.0. Converted to humidity ratio using
    /// zone_cooling_setpoint as the reference temperature.
    pub min_rh_setpoint: f64,
    /// Zone cooling setpoint temperature [°C], used as reference for
    /// converting min_rh_setpoint to humidity ratio.
    pub zone_cooling_setpoint: f64,

    // ─── Runtime state ──────────────────────────────────────────────────
    /// Electric power consumed this timestep [W].
    #[serde(skip)]
    pub power: f64,
    /// Water (steam) mass flow added this timestep [kg/s].
    #[serde(skip)]
    pub moisture_added: f64,
}

impl Humidifier {
    /// Create a new electric steam humidifier.
    ///
    /// # Arguments
    /// * `name`                  - Component name
    /// * `rated_power`           - Maximum electric power [W]
    /// * `min_rh_setpoint`       - Minimum RH at zone [0-1] (e.g., 0.30)
    /// * `zone_cooling_setpoint` - Zone cooling setpoint [°C] for RH→w conversion
    pub fn new(name: &str, rated_power: f64, min_rh_setpoint: f64, zone_cooling_setpoint: f64) -> Self {
        let rated_capacity = rated_power / H_STEAM_TOTAL;
        Self {
            name: name.to_string(),
            rated_power,
            rated_capacity,
            w_setpoint: 0.0,
            min_rh_setpoint,
            zone_cooling_setpoint,
            power: 0.0,
            moisture_added: 0.0,
        }
    }
}

impl AirComponent for Humidifier {
    fn name(&self) -> &str {
        &self.name
    }

    fn simulate_air(
        &mut self,
        inlet: &AirPort,
        _ctx: &SimulationContext,
    ) -> AirPort {
        // No air flow → humidifier off
        if inlet.mass_flow <= 0.0 {
            self.power = 0.0;
            self.moisture_added = 0.0;
            return *inlet;
        }

        // Determine humidity ratio setpoint
        let w_target = if self.w_setpoint > 0.0 {
            self.w_setpoint
        } else {
            // Convert min RH to humidity ratio using the zone cooling setpoint
            // as reference temperature (the zone is typically near this temp).
            psych::w_fn_tdb_rh_pb(self.zone_cooling_setpoint, self.min_rh_setpoint, inlet.state.p_b)
        };

        let w_in = inlet.state.w;

        // If supply air is already humid enough, no humidification needed
        if w_in >= w_target {
            self.power = 0.0;
            self.moisture_added = 0.0;
            return *inlet;
        }

        // Calculate required moisture addition [kg/s]
        let w_deficit = w_target - w_in;
        let moisture_needed = inlet.mass_flow * w_deficit; // kg_water/s

        // Clamp to rated capacity
        let moisture_actual = moisture_needed.min(self.rated_capacity);

        // Part-load power
        if self.rated_capacity > 0.0 {
            self.power = self.rated_power * (moisture_actual / self.rated_capacity);
        } else {
            self.power = 0.0;
        }
        self.moisture_added = moisture_actual;

        // Update outlet air state: add moisture, slight temperature increase from steam
        let w_out = w_in + moisture_actual / inlet.mass_flow;
        // Steam at ~100°C enters airstream, raising temp slightly
        // ΔT ≈ moisture_actual × (h_steam - h_vapor_at_T_in) / (m_air × cp_air)
        // For simplicity, use small temp rise from latent heat absorbed
        let t_out = inlet.state.t_db; // Steam humidifier: approximately isothermal for dry air
                                       // (the steam energy goes into moisture, not sensible heating)

        AirPort::new(
            psych::MoistAirState::new(t_out, w_out, inlet.state.p_b),
            inlet.mass_flow,
        )
    }

    fn power_consumption(&self) -> f64 {
        self.power
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
                day: 15,
                hour: 12,
                sub_hour: 1,
                timesteps_per_hour: 1,
                sim_time_s: 0.0,
                dt: 3600.0,
            },
            outdoor_air: MoistAirState::from_tdb_rh(-10.0, 0.3, 83594.0),
            day_type: DayType::WeatherDay,
            is_sizing: false,
            sizing_internal_gains: SizingInternalGains::Full,
        }
    }

    #[test]
    fn test_humidifier_adds_moisture_when_dry() {
        let mut hum = Humidifier::new("Test Hum", 100_000.0, 0.30, 24.0);
        let ctx = make_ctx();

        // Very dry inlet air (cold outdoor air after heating)
        // -10°C, 30% RH heated to 35°C → very low w
        let dry_state = MoistAirState::from_tdb_rh(-10.0, 0.30, 83594.0);
        let inlet = AirPort::new(
            MoistAirState::new(35.0, dry_state.w, 83594.0),
            5.0, // 5 kg/s
        );

        let outlet = hum.simulate_air(&inlet, &ctx);

        assert!(hum.power > 0.0, "Humidifier should consume power");
        assert!(hum.moisture_added > 0.0, "Should add moisture");
        assert!(outlet.state.w > inlet.state.w, "Outlet should be more humid");
    }

    #[test]
    fn test_humidifier_off_when_humid() {
        let mut hum = Humidifier::new("Test Hum", 100_000.0, 0.30, 24.0);
        let ctx = make_ctx();

        // Already humid air (summer conditions)
        let inlet = AirPort::new(
            MoistAirState::from_tdb_rh(25.0, 0.60, 83594.0),
            5.0,
        );

        let _outlet = hum.simulate_air(&inlet, &ctx);

        assert_eq!(hum.power, 0.0, "Humidifier should be off when air is humid");
        assert_eq!(hum.moisture_added, 0.0);
    }

    #[test]
    fn test_humidifier_zero_flow() {
        let mut hum = Humidifier::new("Test Hum", 100_000.0, 0.30, 24.0);
        let ctx = make_ctx();

        let inlet = AirPort::new(
            MoistAirState::from_tdb_rh(20.0, 0.10, 83594.0),
            0.0,
        );

        let _outlet = hum.simulate_air(&inlet, &ctx);

        assert_eq!(hum.power, 0.0);
    }

    #[test]
    fn test_humidifier_power_proportional_to_moisture() {
        let mut hum = Humidifier::new("Test Hum", 100_000.0, 0.30, 24.0);
        let ctx = make_ctx();

        // Moderately dry air
        let inlet = AirPort::new(
            MoistAirState::from_tdb_rh(20.0, 0.15, 83594.0),
            2.0,
        );

        let _outlet = hum.simulate_air(&inlet, &ctx);

        // Power should be proportional to moisture added / rated capacity
        let expected_plr = hum.moisture_added / hum.rated_capacity;
        let expected_power = hum.rated_power * expected_plr;
        assert_relative_eq!(hum.power, expected_power, max_relative = 0.001);
        assert!(hum.power > 0.0 && hum.power <= hum.rated_power);
    }
}
