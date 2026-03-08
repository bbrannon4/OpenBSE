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
/// Reads zone temperatures, determines heating/cooling mode, and calculates
/// the required supply air temperature and flow to meet the load.
#[derive(Debug)]
pub struct ZoneThermostat {
    name: String,
    /// Zone groups this thermostat controls
    zone_groups: Vec<ZoneGroup>,
    /// Individual zones with their own setpoints (for zones not in a group)
    individual_zones: Vec<ZoneGroup>,
    /// Design supply air temp for heating [°C]
    heating_supply_temp: f64,
    /// Design supply air temp for cooling [°C]
    cooling_supply_temp: f64,
    /// Design air flow rate per zone [kg/s] (for load calculation)
    design_zone_flow: f64,

    /// Current control actions (rebuilt each timestep)
    current_actions: Vec<ControlAction>,
    /// Current mode per zone
    zone_modes: std::collections::HashMap<String, ThermostatMode>,
}

impl ZoneThermostat {
    /// Create a thermostat from zone groups.
    pub fn from_groups(
        name: &str,
        zone_groups: Vec<ZoneGroup>,
        heating_supply_temp: f64,
        cooling_supply_temp: f64,
        design_zone_flow: f64,
    ) -> Self {
        Self {
            name: name.to_string(),
            zone_groups,
            individual_zones: Vec::new(),
            heating_supply_temp,
            cooling_supply_temp,
            design_zone_flow,
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
        heating_supply_temp: f64,
        cooling_supply_temp: f64,
        design_zone_flow: f64,
    ) -> Self {
        let group = ZoneGroup {
            name: zone.to_string(),
            zones: vec![zone.to_string()],
            heating_setpoint,
            cooling_setpoint,
            deadband: None,
        };
        Self {
            name: name.to_string(),
            zone_groups: vec![group],
            individual_zones: Vec::new(),
            heating_supply_temp,
            cooling_supply_temp,
            design_zone_flow,
            current_actions: Vec::new(),
            zone_modes: std::collections::HashMap::new(),
        }
    }

    /// Determine thermostat mode for a zone given current temp and setpoints.
    fn determine_mode(
        zone_temp: f64,
        heating_sp: f64,
        cooling_sp: f64,
    ) -> ThermostatMode {
        if zone_temp < heating_sp {
            ThermostatMode::Heating
        } else if zone_temp > cooling_sp {
            ThermostatMode::Cooling
        } else {
            ThermostatMode::Deadband
        }
    }

    /// Calculate required supply air temperature to meet zone load.
    ///
    /// For heating: supply temp must be above zone temp to add heat.
    /// For cooling: supply temp must be below zone temp to remove heat.
    /// Uses the proportional approach: the further from setpoint, the more
    /// extreme the supply temp (up to the design limits).
    fn calc_supply_temp(
        zone_temp: f64,
        setpoint: f64,
        mode: ThermostatMode,
        heating_supply_temp: f64,
        cooling_supply_temp: f64,
    ) -> f64 {
        match mode {
            ThermostatMode::Heating => {
                // Proportional: if zone is far below setpoint, use full heating supply temp.
                // If zone is close to setpoint, reduce supply temp.
                let error = setpoint - zone_temp;
                let max_error = 5.0; // °C — full output at 5°C error
                let frac = (error / max_error).clamp(0.0, 1.0);
                // Blend between zone temp and design heating supply temp
                zone_temp + frac * (heating_supply_temp - zone_temp)
            }
            ThermostatMode::Cooling => {
                let error = zone_temp - setpoint;
                let max_error = 5.0;
                let frac = (error / max_error).clamp(0.0, 1.0);
                zone_temp - frac * (zone_temp - cooling_supply_temp)
            }
            _ => zone_temp, // Deadband/off — supply at zone temp (no load)
        }
    }

    /// Calculate required air mass flow rate to meet zone load.
    fn calc_zone_flow(
        zone_temp: f64,
        setpoint: f64,
        mode: ThermostatMode,
        design_flow: f64,
    ) -> f64 {
        match mode {
            ThermostatMode::Heating | ThermostatMode::Cooling => {
                let error = (zone_temp - setpoint).abs();
                let max_error = 5.0;
                let frac = (error / max_error).clamp(0.1, 1.0); // minimum 10% flow
                design_flow * frac
            }
            _ => design_flow * 0.1, // Deadband: minimum ventilation
        }
    }

    /// Get the current mode for a specific zone.
    pub fn zone_mode(&self, zone: &str) -> ThermostatMode {
        self.zone_modes.get(zone).copied().unwrap_or(ThermostatMode::Off)
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
            for zone_name in &group.zones {
                let zone_temp = state.zone_temp(zone_name);

                let mode = Self::determine_mode(
                    zone_temp,
                    group.heating_setpoint,
                    group.cooling_setpoint,
                );

                self.zone_modes.insert(zone_name.clone(), mode);

                // Calculate target supply air temp for this zone
                let setpoint = match mode {
                    ThermostatMode::Heating => group.heating_setpoint,
                    ThermostatMode::Cooling => group.cooling_setpoint,
                    _ => zone_temp,
                };

                let supply_temp = Self::calc_supply_temp(
                    zone_temp,
                    setpoint,
                    mode,
                    self.heating_supply_temp,
                    self.cooling_supply_temp,
                );

                let mass_flow = Self::calc_zone_flow(
                    zone_temp,
                    setpoint,
                    mode,
                    self.design_zone_flow,
                );

                self.current_actions.push(ControlAction::SetZoneSupplyTemp {
                    zone: zone_name.clone(),
                    supply_temp,
                });

                self.current_actions.push(ControlAction::SetZoneAirFlow {
                    zone: zone_name.clone(),
                    mass_flow,
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
                month: 1, day: 15, hour: 12, sub_hour: 1,
                timesteps_per_hour: 1, sim_time_s: 0.0, dt: 3600.0,
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
            deadband: None,
        };

        let mut thermostat = ZoneThermostat::from_groups(
            "Office Thermostat",
            vec![group],
            35.0,  // heating supply temp
            13.0,  // cooling supply temp
            0.5,   // design zone flow
        );

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

        // Should have 6 actions (2 per zone: supply temp + flow)
        assert_eq!(thermostat.actions().len(), 6);
    }

    #[test]
    fn test_zone_group_mixed_modes() {
        let group = ZoneGroup {
            name: "Mixed".to_string(),
            zones: vec!["ZoneA".to_string(), "ZoneB".to_string(), "ZoneC".to_string()],
            heating_setpoint: 21.0,
            cooling_setpoint: 24.0,
            deadband: None,
        };

        let mut thermostat = ZoneThermostat::from_groups(
            "Mixed Thermostat",
            vec![group],
            35.0, 13.0, 0.5,
        );

        let mut state = SystemState::new(MoistAirState::from_tdb_rh(20.0, 0.5, 101325.0));
        state.zone_temps.insert("ZoneA".to_string(), 19.0);  // needs heating
        state.zone_temps.insert("ZoneB".to_string(), 22.0);  // deadband
        state.zone_temps.insert("ZoneC".to_string(), 26.0);  // needs cooling

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
            21.0, 24.0,   // heating/cooling setpoints
            35.0, 13.0,   // supply temps
            0.3,
        );

        let mut state = SystemState::new(MoistAirState::from_tdb_rh(0.0, 0.5, 101325.0));
        state.zone_temps.insert("Living Room".to_string(), 18.0);

        let ctx = make_ctx();
        thermostat.update(&state, &ctx);

        assert_eq!(thermostat.zone_mode("Living Room"), ThermostatMode::Heating);
        assert_eq!(thermostat.actions().len(), 2); // supply temp + flow

        // Check that supply temp action has a value above zone temp
        match &thermostat.actions()[0] {
            ControlAction::SetZoneSupplyTemp { supply_temp, .. } => {
                assert!(*supply_temp > 18.0, "Supply temp should be above zone temp for heating");
                assert!(*supply_temp <= 35.0, "Supply temp should not exceed design heating supply temp");
            }
            _ => panic!("Expected SetZoneSupplyTemp action"),
        }
    }

    #[test]
    fn test_multiple_zone_groups() {
        let offices = ZoneGroup {
            name: "Offices".to_string(),
            zones: vec!["Office1".to_string(), "Office2".to_string()],
            heating_setpoint: 21.0,
            cooling_setpoint: 24.0,
            deadband: None,
        };
        let server = ZoneGroup {
            name: "Server Room".to_string(),
            zones: vec!["Server".to_string()],
            heating_setpoint: 18.0,    // server room can be cooler
            cooling_setpoint: 22.0,    // but needs more cooling
            deadband: None,
        };

        let mut thermostat = ZoneThermostat::from_groups(
            "Building Thermostat",
            vec![offices, server],
            35.0, 13.0, 0.5,
        );

        let mut state = SystemState::new(MoistAirState::from_tdb_rh(20.0, 0.5, 101325.0));
        state.zone_temps.insert("Office1".to_string(), 19.0);  // heating (below 21)
        state.zone_temps.insert("Office2".to_string(), 22.0);  // deadband (21-24)
        state.zone_temps.insert("Server".to_string(), 23.0);   // cooling (above 22)

        let ctx = make_ctx();
        thermostat.update(&state, &ctx);

        assert_eq!(thermostat.zone_mode("Office1"), ThermostatMode::Heating);
        assert_eq!(thermostat.zone_mode("Office2"), ThermostatMode::Deadband);
        assert_eq!(thermostat.zone_mode("Server"), ThermostatMode::Cooling);
    }
}
