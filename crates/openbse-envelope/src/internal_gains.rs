//! Internal heat gains: people, lighting, equipment.
//!
//! Each gain type has a design level, a radiant/convective split, and an
//! optional schedule reference for time-varying operation.

use serde::{Deserialize, Serialize};

fn default_submeter() -> String {
    "General".to_string()
}

/// Internal gain specification.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InternalGainInput {
    People {
        count: f64,
        /// Activity level [W/person]: total heat per person (required, as
        /// E+'s activity level schedule).
        activity_level: f64,
        /// Sensible fraction of the activity level [0-1]. When absent, the
        /// sensible heat per person comes from the activity level and the zone
        /// air temperature ([`people_sensible_per_person`], E+ autocalculate).
        /// The latent heat is the remainder.
        #[serde(default)]
        sensible_fraction: Option<f64>,
        /// Fraction of gain that is radiant [0-1]
        #[serde(default = "default_people_radiant")]
        radiant_fraction: f64,
        /// Schedule name for time-varying occupancy (default: always on)
        #[serde(default)]
        schedule: Option<String>,
        /// Alternative: explicit sensible gain [W/person].
        /// When set, overrides the sensible share of `activity_level`.
        #[serde(default)]
        sensible_gain_per_person: Option<f64>,
        /// Alternative: explicit latent gain [W/person].
        /// When set, overrides the latent remainder of `activity_level`.
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
        /// Submeter label for end-use reporting (default: "General")
        #[serde(default = "default_submeter")]
        submeter: String,
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
        /// Fraction of equipment heat that is latent (moisture-generating) [0-1] (default 0.0).
        /// Examples: kitchen equipment, laundry — a portion of their heat output adds
        /// moisture to the zone rather than sensible heat. The latent fraction is applied
        /// to the non-lost portion: latent = power × (1 - lost_fraction) × latent_fraction.
        #[serde(default)]
        latent_fraction: f64,
        /// Schedule name for time-varying equipment (default: always on)
        #[serde(default)]
        schedule: Option<String>,
        /// Submeter label for end-use reporting (default: "General")
        #[serde(default = "default_submeter")]
        submeter: String,
    },
}

fn default_people_radiant() -> f64 {
    0.3
}
fn default_lights_radiant() -> f64 {
    0.7
}
fn default_equip_radiant() -> f64 {
    0.3
}

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
    /// Equipment latent heat this timestep [W] (from latent_fraction)
    pub equipment_latent: f64,

    // ─── Per-type gain breakdown (conv + rad = heat entering zone) ────
    /// Lighting heat to zone [W] (conv + rad, excluding return air fraction)
    pub lighting_gain_to_zone: f64,
    /// Equipment sensible heat to zone [W] (conv + rad, excluding lost fraction)
    pub equipment_sensible_gain_to_zone: f64,
    /// Lighting heat to the return air [W] (`return_air_fraction`). It is
    /// part of `convective`: with no return air to carry it, the heat stays
    /// in the zone air, as EnergyPlus does for a zone with no return path or
    /// served only by cycling-fan systems (`Zone::NoHeatToReturnAir`). An air
    /// loop whose return air carries it takes it back out (#51).
    pub lighting_return_air: f64,
}

/// Resolve all gains for a zone at this timestep.
///
/// If a `ScheduleManager` is provided, schedule fractions are looked up by name.
/// Otherwise, all gains run at 100% (backward compatible).
pub fn resolve_gains(gains: &[InternalGainInput], zone_air_temp: f64) -> ResolvedGain {
    resolve_gains_scheduled(gains, None, 1, 1, 1, zone_air_temp)
}

/// E+ `InitInternalHeatGains` coefficients for the sensible heat of one person
/// from activity level M [W] and zone air temperature T [°C].
const PEOPLE_SENSIBLE_C: [f64; 9] = [
    6.4611027,
    0.946892,
    0.0000255737,
    7.139322,
    -0.0627909,
    0.0000589271,
    -0.198550,
    0.000940018,
    -0.00000149532,
];

/// Sensible heat of one person [W] at activity level `m` [W] and zone air
/// temperature `t` [°C], held within [0, m]: the E+ correlation used when the
/// sensible heat fraction is autocalculated (`InternalHeatGains.cc`).
pub fn people_sensible_per_person(m: f64, t: f64) -> f64 {
    let c = &PEOPLE_SENSIBLE_C;
    let q = c[0]
        + m * (c[1] + m * c[2])
        + t * ((c[3] + m * (c[4] + m * c[5])) + t * (c[6] + m * (c[7] + m * c[8])));
    q.clamp(0.0, m.max(0.0))
}

/// Resolve gains with schedule support.
///
/// `schedule_mgr` — optional schedule manager for time-varying fractions
/// `hour` — hour of day (1-24)
/// `day_of_week` — 1=Monday through 7=Sunday
/// `day_of_year` — 1-365, for resolving seasonal (date-range) schedules
/// `zone_air_temp` — the zone air temperature [°C] people's sensible heat
/// follows when their sensible fraction is autocalculated (E+ uses the zone
/// mean air temperature at the start of the timestep)
pub fn resolve_gains_scheduled(
    gains: &[InternalGainInput],
    schedule_mgr: Option<&crate::schedule::ScheduleManager>,
    hour: u32,
    day_of_week: u32,
    day_of_year: u32,
    zone_air_temp: f64,
) -> ResolvedGain {
    let mut result = ResolvedGain::default();

    for gain in gains {
        match gain {
            InternalGainInput::People {
                count,
                activity_level,
                sensible_fraction,
                radiant_fraction,
                schedule,
                sensible_gain_per_person,
                latent_gain_per_person,
            } => {
                let frac =
                    schedule_fraction(schedule, schedule_mgr, hour, day_of_week, day_of_year);
                // The activity level splits into sensible heat (a given fraction,
                // or E+'s correlation in the zone air temperature) and latent
                // heat, the remainder; explicit W/person values override either.
                let split = match sensible_fraction {
                    Some(f) => activity_level * f,
                    None => people_sensible_per_person(*activity_level, zone_air_temp),
                };
                let sensible_per_person = sensible_gain_per_person.unwrap_or(split);
                let latent_per_person = latent_gain_per_person.unwrap_or(activity_level - split);
                let sensible = count * sensible_per_person * frac;
                let latent = count * latent_per_person * frac;
                result.radiative += sensible * radiant_fraction;
                result.convective += sensible * (1.0 - radiant_fraction);
                result.total += sensible;
                result.people_heat += sensible;
                result.people_latent += latent;
            }
            InternalGainInput::Lights {
                power,
                radiant_fraction,
                return_air_fraction,
                schedule,
                submeter: _,
            } => {
                let frac =
                    schedule_fraction(schedule, schedule_mgr, hour, day_of_week, day_of_year);
                let total = power * frac;
                // All fractions are of the total, as in E+ `Lights` (#186).
                let to_return_air = total * return_air_fraction;
                let to_zone = total - to_return_air;
                let radiant = total * radiant_fraction;
                result.radiative += radiant;
                result.convective += to_zone - radiant + to_return_air;
                result.total += total;
                result.lighting_power += total;
                result.lighting_gain_to_zone += to_zone;
                result.lighting_return_air += to_return_air;
            }
            InternalGainInput::Equipment {
                power,
                radiant_fraction,
                lost_fraction,
                latent_fraction,
                schedule,
                submeter: _,
            } => {
                let frac =
                    schedule_fraction(schedule, schedule_mgr, hour, day_of_week, day_of_year);
                let total = power * frac;
                // All fractions are of the total, as in E+ `ElectricEquipment` (#186).
                // Only the non-lost portion enters the zone; latent generates
                // moisture, not sensible heat.
                let latent = total * latent_fraction;
                let sensible = total * (1.0 - lost_fraction - latent_fraction);
                let radiant = total * radiant_fraction;
                result.radiative += radiant;
                result.convective += sensible - radiant;
                result.total += sensible;
                result.equipment_latent += latent;
                // Report full electric power for energy accounting
                result.equipment_power += total;
                result.equipment_sensible_gain_to_zone += sensible;
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
    day_of_year: u32,
) -> f64 {
    match (schedule_name, schedule_mgr) {
        (Some(name), Some(mgr)) => mgr.fraction_doy(name, hour, day_of_week, day_of_year),
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
            sensible_fraction: Some(0.6),
            radiant_fraction: 0.3,
            schedule: None,
            sensible_gain_per_person: None,
            latent_gain_per_person: None,
        }];
        let resolved = resolve_gains(&gains, 24.0);
        // 10 people × 120 W/person × 0.6 sensible = 720 W sensible
        assert_relative_eq!(resolved.total, 720.0);
        assert_relative_eq!(resolved.radiative, 216.0); // 720 × 0.3
        assert_relative_eq!(resolved.convective, 504.0); // 720 × 0.7
    }

    /// E+ `InitInternalHeatGains`, written out term by term from its
    /// constants, independently of `people_sensible_per_person`.
    fn eplus_sensible(m: f64, t: f64) -> f64 {
        6.4611027 + 0.946892 * m + 0.0000255737 * m * m + 7.139322 * t - 0.0627909 * t * m
            + 0.0000589271 * t * m * m
            - 0.198550 * t * t
            + 0.000940018 * t * t * m
            - 0.00000149532 * t * t * m * m
    }

    #[test]
    fn people_sensible_heat_is_the_eplus_correlation() {
        for m in [70.0, 100.0, 120.0, 200.0, 300.0] {
            for t in [16.0, 20.0, 21.0, 23.89, 26.0, 29.44, 32.0] {
                let want = eplus_sensible(m, t).clamp(0.0, m);
                let got = people_sensible_per_person(m, t);
                assert!((got - want).abs() < 1e-9, "M={m} T={t}: {got} vs {want}");
            }
        }
        // Values quoted in the Engineering Reference (M = 120 W).
        for (t, q) in [(21.0, 82.7), (23.89, 70.0), (29.44, 40.8)] {
            let got = people_sensible_per_person(120.0, t);
            assert!((got - q).abs() < 0.05, "T={t}: {got}");
        }
        // Held within [0, M], as E+ does: a low activity level in a cool zone
        // would exceed M, a light activity in a very hot zone falls below 0.
        assert_eq!(people_sensible_per_person(10.0, 15.0), 10.0);
        assert!(eplus_sensible(10.0, 15.0) > 10.0);
        assert_eq!(people_sensible_per_person(80.0, 60.0), 0.0);
        assert!(eplus_sensible(80.0, 60.0) < 0.0);
    }

    #[test]
    fn autocalculated_people_split_the_activity_level_by_zone_temperature() {
        let people = |sensible_fraction| InternalGainInput::People {
            count: 10.0,
            activity_level: 120.0,
            sensible_fraction,
            radiant_fraction: 0.3,
            schedule: None,
            sensible_gain_per_person: None,
            latent_gain_per_person: None,
        };
        let mut previous = f64::INFINITY;
        for t in [18.0, 21.0, 24.0, 27.0, 30.0] {
            let r = resolve_gains(&[people(None)], t);
            // Sensible plus latent is the whole activity level.
            assert_relative_eq!(r.people_heat + r.people_latent, 1200.0, epsilon = 1e-9);
            assert_relative_eq!(
                r.people_heat,
                10.0 * people_sensible_per_person(120.0, t),
                epsilon = 1e-9
            );
            assert_relative_eq!(r.radiative, 0.3 * r.people_heat, epsilon = 1e-9);
            assert!(r.people_heat < previous, "sensible falls as the zone warms");
            previous = r.people_heat;
        }
        // A given fraction ignores the zone temperature.
        for t in [18.0, 30.0] {
            let r = resolve_gains(&[people(Some(0.6))], t);
            assert_relative_eq!(r.people_heat, 720.0);
            assert_relative_eq!(r.people_latent, 480.0);
        }
    }

    #[test]
    fn test_lights_with_return_air() {
        let gains = vec![InternalGainInput::Lights {
            power: 1000.0,
            radiant_fraction: 0.7,
            return_air_fraction: 0.2,
            schedule: None,
            submeter: "General".to_string(),
        }];
        let resolved = resolve_gains(&gains, 24.0);
        // 1000 total: 700 radiant, 200 to return air, 100 convective. The 200
        // bound for the return air is zone convective until an air loop's
        // return carries it.
        assert_relative_eq!(resolved.total, 1000.0);
        assert_relative_eq!(resolved.radiative, 700.0);
        assert_relative_eq!(resolved.convective, 300.0);
        assert_relative_eq!(resolved.lighting_gain_to_zone, 800.0);
        assert_relative_eq!(resolved.lighting_return_air, 200.0);
        // Nothing is lost: zone conv + rad = electric power.
        assert_relative_eq!(
            resolved.convective + resolved.radiative,
            resolved.lighting_power
        );
    }

    #[test]
    fn test_combined_gains() {
        let gains = vec![
            InternalGainInput::People {
                count: 5.0,
                activity_level: 120.0,
                sensible_fraction: Some(0.6),
                radiant_fraction: 0.3,
                schedule: None,
                sensible_gain_per_person: None,
                latent_gain_per_person: None,
            },
            InternalGainInput::Equipment {
                power: 500.0,
                radiant_fraction: 0.3,
                lost_fraction: 0.0,
                latent_fraction: 0.0,
                schedule: None,
                submeter: "General".to_string(),
            },
        ];
        let resolved = resolve_gains(&gains, 24.0);
        assert_relative_eq!(resolved.total, 860.0); // 5×120×0.6=360 + 500
    }

    #[test]
    fn test_scheduled_gains() {
        use crate::schedule::{ScheduleInput, ScheduleManager};

        let schedules = ScheduleManager::from_inputs(vec![ScheduleInput {
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
            seasons: None,
        }]);

        let gains = vec![InternalGainInput::Equipment {
            power: 1000.0,
            radiant_fraction: 0.3,
            lost_fraction: 0.0,
            latent_fraction: 0.0,
            schedule: Some("half".to_string()),
            submeter: "General".to_string(),
        }];

        // Weekday: 50% schedule → 500W
        let resolved = resolve_gains_scheduled(&gains, Some(&schedules), 12, 1, 1, 24.0);
        assert_relative_eq!(resolved.total, 500.0);

        // Weekend: 0% schedule → 0W
        let resolved = resolve_gains_scheduled(&gains, Some(&schedules), 12, 6, 1, 24.0);
        assert_relative_eq!(resolved.total, 0.0);
    }

    #[test]
    fn test_equipment_lost_fraction() {
        // Elevator with 95% heat lost (only 5% enters zone)
        let gains = vec![InternalGainInput::Equipment {
            power: 1000.0,
            radiant_fraction: 0.015,
            lost_fraction: 0.95,
            latent_fraction: 0.0,
            schedule: None,
            submeter: "General".to_string(),
        }];
        let resolved = resolve_gains(&gains, 24.0);
        // 1000W total, 950W lost, 50W to zone: 15W radiant, 35W convective
        assert_relative_eq!(resolved.total, 50.0, epsilon = 1e-10);
        assert_relative_eq!(resolved.radiative, 15.0, epsilon = 1e-10);
        assert_relative_eq!(resolved.convective, 35.0, epsilon = 1e-10);
        // Full 1000W reported for electricity accounting
        assert_relative_eq!(resolved.equipment_power, 1000.0);
    }

    /// #186: E+'s worked example, 0.4 return, 0.4 radiant + 0.2 visible.
    #[test]
    fn lights_fractions_are_of_the_total() {
        let gains = vec![InternalGainInput::Lights {
            power: 1000.0,
            radiant_fraction: 0.6,
            return_air_fraction: 0.4,
            schedule: None,
            submeter: Default::default(),
        }];
        let r = resolve_gains(&gains, 24.0);
        assert!((r.radiative - 600.0).abs() < 1e-9);
        assert!((r.lighting_return_air - 400.0).abs() < 1e-9);
        assert!((r.radiative + r.convective - 1000.0).abs() < 1e-9);
    }

    /// #186: latent 0.2, radiant 0.3, lost 0.1 of 1000 W, the rest convective.
    #[test]
    fn equipment_fractions_are_of_the_total_and_close() {
        let gains = vec![InternalGainInput::Equipment {
            power: 1000.0,
            radiant_fraction: 0.3,
            lost_fraction: 0.1,
            latent_fraction: 0.2,
            schedule: None,
            submeter: Default::default(),
        }];
        let r = resolve_gains(&gains, 24.0);
        assert!((r.radiative - 300.0).abs() < 1e-9);
        assert!((r.equipment_latent - 200.0).abs() < 1e-9);
        assert!((r.convective - 400.0).abs() < 1e-9);
        let lost = 100.0;
        assert!((r.radiative + r.convective + r.equipment_latent + lost - 1000.0).abs() < 1e-9);
    }
}
