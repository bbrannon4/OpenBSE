//! Zone thermostat controller.
//!
//! Reads zone air temperature, compares to heating/cooling setpoints,
//! and produces control actions for the HVAC system to meet the load.
//!
//! Supports zone groups — apply one thermostat definition to many zones
//! instead of repeating the same settings for each zone individually.

use crate::state::{ControlAction, SystemState};
use crate::Controller;
use openbse_core::ports::SimulationContext;

use serde::{Deserialize, Serialize};

/// A group of zones that share the same thermostat settings.
///
/// Instead of defining a thermostat for every zone individually,
/// define one zone group and list all zones that share the same setpoints.
///
/// ```yaml
/// zone_groups:
///   - name: Office Zones
///     zones: [East Office, West Office, North Office, South Office]
///     heating_setpoint: 21.1
///     cooling_setpoint: 23.9
///
///   - name: Conference Rooms
///     zones: [Conf A, Conf B, Conf C]
///     heating_setpoint: 20.0
///     cooling_setpoint: 24.4
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZoneGroup {
    /// Name of the zone group
    pub name: String,
    /// List of zone names in this group
    pub zones: Vec<String>,
    /// Heating setpoint [°C]
    pub heating_setpoint: f64,
    /// Cooling setpoint [°C]
    pub cooling_setpoint: f64,
    /// Unoccupied (setback) heating setpoint [°C].
    /// Falls back to `heating_setpoint` (no setback) when unspecified.
    #[serde(default)]
    pub unoccupied_heating_setpoint: Option<f64>,
    /// Unoccupied (setup) cooling setpoint [°C].
    /// Falls back to `cooling_setpoint` (no setup) when unspecified.
    #[serde(default)]
    pub unoccupied_cooling_setpoint: Option<f64>,
    /// Deadband between heating and cooling setpoints [°C]
    /// If not specified, the gap between heating and cooling setpoints is the deadband.
    #[serde(default)]
    pub deadband: Option<f64>,
}

/// Mode the thermostat is currently in for a given zone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThermostatMode {
    Off,
    Heating,
    Cooling,
    Deadband,
}

/// Zone thermostat controller.
///
/// Operates on one or more zones (via ZoneGroup or individual zones).
/// Reads zone temperatures and emits the zone heating/cooling setpoints
/// (occupied + unoccupied) as control actions. It is the *setpoint authority*:
/// it decides what temperature each zone wants. How that setpoint is met —
/// supply air temperature, flow, coil/plant response — is the job of the HVAC
/// engine (`simulate_all_loops`), not the thermostat. This split keeps control
/// authority and capacity response from being computed twice (see issue #75).
#[derive(Debug)]
pub struct ZoneThermostat {
    name: String,
    /// Zone groups this thermostat controls
    zone_groups: Vec<ZoneGroup>,
    /// Individual zones with their own setpoints (for zones not in a group)
    individual_zones: Vec<ZoneGroup>,

    /// Current control actions (rebuilt each timestep)
    current_actions: Vec<ControlAction>,
    /// Current mode per zone
    zone_modes: std::collections::HashMap<String, ThermostatMode>,
}

impl ZoneThermostat {
    /// Create a thermostat from zone groups.
    pub fn from_groups(name: &str, zone_groups: Vec<ZoneGroup>) -> Self {
        Self {
            name: name.to_string(),
            zone_groups,
            individual_zones: Vec::new(),
            current_actions: Vec::new(),
            zone_modes: std::collections::HashMap::new(),
        }
    }

    /// Create a thermostat for a single zone.
    pub fn single_zone(
        name: &str,
        zone: &str,
        heating_setpoint: f64,
        cooling_setpoint: f64,
        unoccupied_heating_setpoint: f64,
        unoccupied_cooling_setpoint: f64,
    ) -> Self {
        let group = ZoneGroup {
            name: zone.to_string(),
            zones: vec![zone.to_string()],
            heating_setpoint,
            cooling_setpoint,
            unoccupied_heating_setpoint: Some(unoccupied_heating_setpoint),
            unoccupied_cooling_setpoint: Some(unoccupied_cooling_setpoint),
            deadband: None,
        };
        Self {
            name: name.to_string(),
            zone_groups: vec![group],
            individual_zones: Vec::new(),
            current_actions: Vec::new(),
            zone_modes: std::collections::HashMap::new(),
        }
    }

    /// Determine thermostat mode for a zone given current temp and setpoints.
    fn determine_mode(zone_temp: f64, heating_sp: f64, cooling_sp: f64) -> ThermostatMode {
        if zone_temp < heating_sp {
            ThermostatMode::Heating
        } else if zone_temp > cooling_sp {
            ThermostatMode::Cooling
        } else {
            ThermostatMode::Deadband
        }
    }

    /// Get the current mode for a specific zone.
    pub fn zone_mode(&self, zone: &str) -> ThermostatMode {
        self.zone_modes
            .get(zone)
            .copied()
            .unwrap_or(ThermostatMode::Off)
    }
}

impl Controller for ZoneThermostat {
    fn name(&self) -> &str {
        &self.name
    }

    fn update(&mut self, state: &SystemState, _ctx: &SimulationContext) {
        self.current_actions.clear();
        self.zone_modes.clear();

        // Process all zone groups
        let all_groups = self.zone_groups.iter().chain(self.individual_zones.iter());

        for group in all_groups {
            // Unoccupied setpoints fall back to the occupied values (no setback)
            // when the group doesn't specify them.
            let unocc_heat = group
                .unoccupied_heating_setpoint
                .unwrap_or(group.heating_setpoint);
            let unocc_cool = group
                .unoccupied_cooling_setpoint
                .unwrap_or(group.cooling_setpoint);

            for zone_name in &group.zones {
                let zone_temp = state.zone_temp(zone_name);

                // Mode is reported for introspection/tests; the HVAC engine
                // decides the actual supply response from the emitted setpoints.
                let mode =
                    Self::determine_mode(zone_temp, group.heating_setpoint, group.cooling_setpoint);
                self.zone_modes.insert(zone_name.clone(), mode);

                self.current_actions.push(ControlAction::SetZoneSetpoints {
                    zone: zone_name.clone(),
                    heating_setpoint: group.heating_setpoint,
                    cooling_setpoint: group.cooling_setpoint,
                    unoccupied_heating_setpoint: unocc_heat,
                    unoccupied_cooling_setpoint: unocc_cool,
                });
            }
        }
    }

    fn actions(&self) -> &[ControlAction] {
        &self.current_actions
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openbse_core::ports::SizingInternalGains;
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
            outdoor_air: MoistAirState::from_tdb_rh(0.0, 0.5, 101325.0),
            day_type: DayType::WeatherDay,
            is_sizing: false,
            sizing_internal_gains: SizingInternalGains::Full,
        }
    }

    #[test]
    fn test_zone_group_heating() {
        let group = ZoneGroup {
            name: "Offices".to_string(),
            zones: vec!["East".to_string(), "West".to_string(), "North".to_string()],
            heating_setpoint: 21.0,
            cooling_setpoint: 24.0,
            unoccupied_heating_setpoint: None,
            unoccupied_cooling_setpoint: None,
            deadband: None,
        };

        let mut thermostat = ZoneThermostat::from_groups("Office Thermostat", vec![group]);

        // All zones cold
        let mut state = SystemState::new(MoistAirState::from_tdb_rh(0.0, 0.5, 101325.0));
        state.zone_temps.insert("East".to_string(), 18.0);
        state.zone_temps.insert("West".to_string(), 19.0);
        state.zone_temps.insert("North".to_string(), 15.0);

        let ctx = make_ctx();
        thermostat.update(&state, &ctx);

        // All three zones should be in heating mode
        assert_eq!(thermostat.zone_mode("East"), ThermostatMode::Heating);
        assert_eq!(thermostat.zone_mode("West"), ThermostatMode::Heating);
        assert_eq!(thermostat.zone_mode("North"), ThermostatMode::Heating);

        // One SetZoneSetpoints action per zone.
        assert_eq!(thermostat.actions().len(), 3);
    }

    #[test]
    fn test_zone_group_mixed_modes() {
        let group = ZoneGroup {
            name: "Mixed".to_string(),
            zones: vec![
                "ZoneA".to_string(),
                "ZoneB".to_string(),
                "ZoneC".to_string(),
            ],
            heating_setpoint: 21.0,
            cooling_setpoint: 24.0,
            unoccupied_heating_setpoint: None,
            unoccupied_cooling_setpoint: None,
            deadband: None,
        };

        let mut thermostat = ZoneThermostat::from_groups("Mixed Thermostat", vec![group]);

        let mut state = SystemState::new(MoistAirState::from_tdb_rh(20.0, 0.5, 101325.0));
        state.zone_temps.insert("ZoneA".to_string(), 19.0); // needs heating
        state.zone_temps.insert("ZoneB".to_string(), 22.0); // deadband
        state.zone_temps.insert("ZoneC".to_string(), 26.0); // needs cooling

        let ctx = make_ctx();
        thermostat.update(&state, &ctx);

        assert_eq!(thermostat.zone_mode("ZoneA"), ThermostatMode::Heating);
        assert_eq!(thermostat.zone_mode("ZoneB"), ThermostatMode::Deadband);
        assert_eq!(thermostat.zone_mode("ZoneC"), ThermostatMode::Cooling);
    }

    #[test]
    fn test_single_zone_thermostat() {
        let mut thermostat = ZoneThermostat::single_zone(
            "Living Room",
            "Living Room",
            21.0,
            24.0, // occupied heating/cooling setpoints
            15.6,
            26.7, // unoccupied setback/setup
        );

        let mut state = SystemState::new(MoistAirState::from_tdb_rh(0.0, 0.5, 101325.0));
        state.zone_temps.insert("Living Room".to_string(), 18.0);

        let ctx = make_ctx();
        thermostat.update(&state, &ctx);

        assert_eq!(thermostat.zone_mode("Living Room"), ThermostatMode::Heating);
        assert_eq!(thermostat.actions().len(), 1); // one setpoint bundle

        // The emitted action carries the occupied + unoccupied setpoints.
        match &thermostat.actions()[0] {
            ControlAction::SetZoneSetpoints {
                zone,
                heating_setpoint,
                cooling_setpoint,
                unoccupied_heating_setpoint,
                unoccupied_cooling_setpoint,
            } => {
                assert_eq!(zone, "Living Room");
                assert!((heating_setpoint - 21.0).abs() < 1e-9);
                assert!((cooling_setpoint - 24.0).abs() < 1e-9);
                assert!((unoccupied_heating_setpoint - 15.6).abs() < 1e-9);
                assert!((unoccupied_cooling_setpoint - 26.7).abs() < 1e-9);
            }
            _ => panic!("Expected SetZoneSetpoints action"),
        }
    }

    #[test]
    fn test_multiple_zone_groups() {
        let offices = ZoneGroup {
            name: "Offices".to_string(),
            zones: vec!["Office1".to_string(), "Office2".to_string()],
            heating_setpoint: 21.0,
            cooling_setpoint: 24.0,
            unoccupied_heating_setpoint: None,
            unoccupied_cooling_setpoint: None,
            deadband: None,
        };
        let server = ZoneGroup {
            name: "Server Room".to_string(),
            zones: vec!["Server".to_string()],
            heating_setpoint: 18.0, // server room can be cooler
            cooling_setpoint: 22.0, // but needs more cooling
            unoccupied_heating_setpoint: None,
            unoccupied_cooling_setpoint: None,
            deadband: None,
        };

        let mut thermostat =
            ZoneThermostat::from_groups("Building Thermostat", vec![offices, server]);

        let mut state = SystemState::new(MoistAirState::from_tdb_rh(20.0, 0.5, 101325.0));
        state.zone_temps.insert("Office1".to_string(), 19.0); // heating (below 21)
        state.zone_temps.insert("Office2".to_string(), 22.0); // deadband (21-24)
        state.zone_temps.insert("Server".to_string(), 23.0); // cooling (above 22)

        let ctx = make_ctx();
        thermostat.update(&state, &ctx);

        assert_eq!(thermostat.zone_mode("Office1"), ThermostatMode::Heating);
        assert_eq!(thermostat.zone_mode("Office2"), ThermostatMode::Deadband);
        assert_eq!(thermostat.zone_mode("Server"), ThermostatMode::Cooling);
    }
}
