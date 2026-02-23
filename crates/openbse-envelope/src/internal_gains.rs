//! Internal heat gains: people, lighting, equipment.
//!
//! Each gain type has a design level, a radiant/convective split, and an
//! optional schedule reference for time-varying operation.

use serde::{Deserialize, Serialize};

/// Internal gain specification.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InternalGainInput {
    People {
        count: f64,
        /// Activity level [W/person]
        #[serde(default = "default_activity")]
        activity_level: f64,
        /// Fraction of gain that is radiant [0-1]
        #[serde(default = "default_people_radiant")]
        radiant_fraction: f64,
        /// Schedule name for time-varying occupancy (default: always on)
        #[serde(default)]
        schedule: Option<String>,
    },
    Lights {
        /// Total installed power [W]
        power: f64,
        /// Fraction radiant [0-1]
        #[serde(default = "default_lights_radiant")]
        radiant_fraction: f64,
        /// Fraction to return air [0-1]
        #[serde(default)]
        return_air_fraction: f64,
        /// Schedule name for time-varying lighting (default: always on)
        #[serde(default)]
        schedule: Option<String>,
    },
    Equipment {
        /// Total installed power [W]
        power: f64,
        /// Fraction radiant [0-1]
        #[serde(default = "default_equip_radiant")]
        radiant_fraction: f64,
        /// Schedule name for time-varying equipment (default: always on)
        #[serde(default)]
        schedule: Option<String>,
    },
}

fn default_activity() -> f64 { 120.0 }
fn default_people_radiant() -> f64 { 0.3 }
fn default_lights_radiant() -> f64 { 0.7 }
fn default_equip_radiant() -> f64 { 0.3 }

/// Resolved internal gain for a timestep [W].
#[derive(Debug, Clone, Copy, Default)]
pub struct ResolvedGain {
    /// Total convective gain to zone air [W]
    pub convective: f64,
    /// Total radiative gain to zone surfaces [W]
    pub radiative: f64,
    /// Total gain [W]
    pub total: f64,
    /// Lighting electric power this timestep [W] (scheduled)
    pub lighting_power: f64,
    /// Equipment electric power this timestep [W] (scheduled)
    pub equipment_power: f64,
    /// People sensible heat this timestep [W] (scheduled)
    pub people_heat: f64,
}

/// Resolve all gains for a zone at this timestep.
///
/// If a `ScheduleManager` is provided, schedule fractions are looked up by name.
/// Otherwise, all gains run at 100% (backward compatible).
pub fn resolve_gains(gains: &[InternalGainInput]) -> ResolvedGain {
    resolve_gains_scheduled(gains, None, 1, 1)
}

/// Resolve gains with schedule support.
///
/// `schedule_mgr` — optional schedule manager for time-varying fractions
/// `hour` — hour of day (1-24)
/// `day_of_week` — 1=Monday through 7=Sunday
pub fn resolve_gains_scheduled(
    gains: &[InternalGainInput],
    schedule_mgr: Option<&crate::schedule::ScheduleManager>,
    hour: u32,
    day_of_week: u32,
) -> ResolvedGain {
    let mut result = ResolvedGain::default();

    for gain in gains {
        match gain {
            InternalGainInput::People { count, activity_level, radiant_fraction, schedule } => {
                let frac = schedule_fraction(schedule, schedule_mgr, hour, day_of_week);
                let total = count * activity_level * frac;
                result.radiative += total * radiant_fraction;
                result.convective += total * (1.0 - radiant_fraction);
                result.total += total;
                result.people_heat += total;
            }
            InternalGainInput::Lights { power, radiant_fraction, return_air_fraction, schedule } => {
                let frac = schedule_fraction(schedule, schedule_mgr, hour, day_of_week);
                let total = power * frac;
                let to_zone = total * (1.0 - return_air_fraction);
                result.radiative += to_zone * radiant_fraction;
                result.convective += to_zone * (1.0 - radiant_fraction);
                result.total += total;
                result.lighting_power += total;
            }
            InternalGainInput::Equipment { power, radiant_fraction, schedule } => {
                let frac = schedule_fraction(schedule, schedule_mgr, hour, day_of_week);
                let total = power * frac;
                result.radiative += total * radiant_fraction;
                result.convective += total * (1.0 - radiant_fraction);
                result.total += total;
                result.equipment_power += total;
            }
        }
    }

    result
}

/// Helper: get schedule fraction, defaulting to 1.0 if no schedule specified.
fn schedule_fraction(
    schedule_name: &Option<String>,
    schedule_mgr: Option<&crate::schedule::ScheduleManager>,
    hour: u32,
    day_of_week: u32,
) -> f64 {
    match (schedule_name, schedule_mgr) {
        (Some(name), Some(mgr)) => mgr.fraction(name, hour, day_of_week),
        _ => 1.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_relative_eq;

    #[test]
    fn test_people_gains() {
        let gains = vec![InternalGainInput::People {
            count: 10.0,
            activity_level: 120.0,
            radiant_fraction: 0.3,
            schedule: None,
        }];
        let resolved = resolve_gains(&gains);
        assert_relative_eq!(resolved.total, 1200.0);
        assert_relative_eq!(resolved.radiative, 360.0);
        assert_relative_eq!(resolved.convective, 840.0);
    }

    #[test]
    fn test_lights_with_return_air() {
        let gains = vec![InternalGainInput::Lights {
            power: 1000.0,
            radiant_fraction: 0.7,
            return_air_fraction: 0.2,
            schedule: None,
        }];
        let resolved = resolve_gains(&gains);
        // 1000 total, 200 to return air, 800 to zone
        // Of 800: 560 radiant, 240 convective
        assert_relative_eq!(resolved.total, 1000.0);
        assert_relative_eq!(resolved.radiative, 560.0);
        assert_relative_eq!(resolved.convective, 240.0);
    }

    #[test]
    fn test_combined_gains() {
        let gains = vec![
            InternalGainInput::People { count: 5.0, activity_level: 120.0, radiant_fraction: 0.3, schedule: None },
            InternalGainInput::Equipment { power: 500.0, radiant_fraction: 0.3, schedule: None },
        ];
        let resolved = resolve_gains(&gains);
        assert_relative_eq!(resolved.total, 1100.0); // 600 + 500
    }

    #[test]
    fn test_scheduled_gains() {
        use crate::schedule::{ScheduleInput, ScheduleManager};

        let schedules = ScheduleManager::from_inputs(vec![
            ScheduleInput {
                name: "half".to_string(),
                weekday: vec![0.5; 24],
                weekend: Some(vec![0.0; 24]),
                saturday: None,
                sunday: None,
                holiday: None,
            },
        ]);

        let gains = vec![InternalGainInput::Equipment {
            power: 1000.0,
            radiant_fraction: 0.3,
            schedule: Some("half".to_string()),
        }];

        // Weekday: 50% schedule → 500W
        let resolved = resolve_gains_scheduled(&gains, Some(&schedules), 12, 1);
        assert_relative_eq!(resolved.total, 500.0);

        // Weekend: 0% schedule → 0W
        let resolved = resolve_gains_scheduled(&gains, Some(&schedules), 12, 6);
        assert_relative_eq!(resolved.total, 0.0);
    }
}
