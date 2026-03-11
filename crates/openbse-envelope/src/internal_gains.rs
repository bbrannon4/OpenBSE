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
        /// Activity level [W/person] — total metabolic heat output
        #[serde(default = "default_activity")]
        activity_level: f64,
        /// Sensible fraction of metabolic heat [0-1] (default 0.6).
        /// Sensible heat = activity_level × sensible_fraction.
        /// Latent heat  = activity_level × (1 - sensible_fraction).
        #[serde(default = "default_sensible_fraction")]
        sensible_fraction: f64,
        /// Fraction of gain that is radiant [0-1]
        #[serde(default = "default_people_radiant")]
        radiant_fraction: f64,
        /// Schedule name for time-varying occupancy (default: always on)
        #[serde(default)]
        schedule: Option<String>,
        /// Alternative: explicit sensible gain [W/person].
        /// When set, overrides `activity_level × sensible_fraction`.
        #[serde(default)]
        sensible_gain_per_person: Option<f64>,
        /// Alternative: explicit latent gain [W/person].
        /// When set, overrides `activity_level × (1 - sensible_fraction)`.
        #[serde(default)]
        latent_gain_per_person: Option<f64>,
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
        /// Fraction of heat that is "lost" (does not enter the zone) [0-1] (default 0.0).
        /// Matches E+ ElectricEquipment "Fraction Lost" field.
        /// Example: elevator with lost_fraction=0.95 means only 5% of heat enters the zone.
        #[serde(default)]
        lost_fraction: f64,
        /// Schedule name for time-varying equipment (default: always on)
        #[serde(default)]
        schedule: Option<String>,
    },
}

fn default_activity() -> f64 { 120.0 }
fn default_sensible_fraction() -> f64 { 0.6 }
fn default_people_radiant() -> f64 { 0.3 }
fn default_lights_radiant() -> f64 { 0.7 }
fn default_equip_radiant() -> f64 { 0.3 }

/// Resolved internal gain for a timestep [W].
#[derive(Debug, Clone, Copy, Default)]
pub struct ResolvedGain {
    /// Total convective gain to zone air [W] (sensible only)
    pub convective: f64,
    /// Total radiative gain to zone surfaces [W] (sensible only)
    pub radiative: f64,
    /// Total sensible gain [W] (convective + radiative)
    pub total: f64,
    /// Lighting electric power this timestep [W] (scheduled)
    pub lighting_power: f64,
    /// Equipment electric power this timestep [W] (scheduled)
    pub equipment_power: f64,
    /// People sensible heat this timestep [W] (scheduled)
    pub people_heat: f64,
    /// People latent heat this timestep [W] (scheduled, for humidity modeling)
    pub people_latent: f64,
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
            InternalGainInput::People { count, activity_level, sensible_fraction, radiant_fraction, schedule,
                                         sensible_gain_per_person, latent_gain_per_person } => {
                let frac = schedule_fraction(schedule, schedule_mgr, hour, day_of_week);
                let sensible_per_person = sensible_gain_per_person
                    .unwrap_or(activity_level * sensible_fraction);
                let latent_per_person = latent_gain_per_person
                    .unwrap_or(activity_level * (1.0 - sensible_fraction));
                let sensible = count * sensible_per_person * frac;
                let latent = count * latent_per_person * frac;
                result.radiative += sensible * radiant_fraction;
                result.convective += sensible * (1.0 - radiant_fraction);
                result.total += sensible;
                result.people_heat += sensible;
                result.people_latent += latent;
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
            InternalGainInput::Equipment { power, radiant_fraction, lost_fraction, schedule } => {
                let frac = schedule_fraction(schedule, schedule_mgr, hour, day_of_week);
                let total = power * frac;
                // Only the non-lost portion enters the zone as heat
                let to_zone = total * (1.0 - lost_fraction);
                result.radiative += to_zone * radiant_fraction;
                result.convective += to_zone * (1.0 - radiant_fraction);
                result.total += to_zone;
                // Report full electric power for energy accounting
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
            sensible_fraction: 0.6,
            radiant_fraction: 0.3,
            schedule: None,
            sensible_gain_per_person: None,
            latent_gain_per_person: None,
        }];
        let resolved = resolve_gains(&gains);
        // 10 people × 120 W/person × 0.6 sensible = 720 W sensible
        assert_relative_eq!(resolved.total, 720.0);
        assert_relative_eq!(resolved.radiative, 216.0);  // 720 × 0.3
        assert_relative_eq!(resolved.convective, 504.0); // 720 × 0.7
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
            InternalGainInput::People { count: 5.0, activity_level: 120.0, sensible_fraction: 0.6, radiant_fraction: 0.3, schedule: None, sensible_gain_per_person: None, latent_gain_per_person: None },
            InternalGainInput::Equipment { power: 500.0, radiant_fraction: 0.3, lost_fraction: 0.0, schedule: None },
        ];
        let resolved = resolve_gains(&gains);
        assert_relative_eq!(resolved.total, 860.0); // 5×120×0.6=360 + 500
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
                monday: None,
                tuesday: None,
                wednesday: None,
                thursday: None,
                friday: None,
                compact: None,
            },
        ]);

        let gains = vec![InternalGainInput::Equipment {
            power: 1000.0,
            radiant_fraction: 0.3,
            lost_fraction: 0.0,
            schedule: Some("half".to_string()),
        }];

        // Weekday: 50% schedule → 500W
        let resolved = resolve_gains_scheduled(&gains, Some(&schedules), 12, 1);
        assert_relative_eq!(resolved.total, 500.0);

        // Weekend: 0% schedule → 0W
        let resolved = resolve_gains_scheduled(&gains, Some(&schedules), 12, 6);
        assert_relative_eq!(resolved.total, 0.0);
    }

    #[test]
    fn test_equipment_lost_fraction() {
        // Elevator with 95% heat lost (only 5% enters zone)
        let gains = vec![InternalGainInput::Equipment {
            power: 1000.0,
            radiant_fraction: 0.3,
            lost_fraction: 0.95,
            schedule: None,
        }];
        let resolved = resolve_gains(&gains);
        // 1000W total, 950W lost, 50W to zone
        // Of 50W: 15W radiant, 35W convective
        assert_relative_eq!(resolved.total, 50.0, epsilon = 1e-10);
        assert_relative_eq!(resolved.radiative, 15.0, epsilon = 1e-10);
        assert_relative_eq!(resolved.convective, 35.0, epsilon = 1e-10);
        // Full 1000W reported for electricity accounting
        assert_relative_eq!(resolved.equipment_power, 1000.0);
    }
}
