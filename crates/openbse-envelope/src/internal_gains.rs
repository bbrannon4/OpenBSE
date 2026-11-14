//! Internal heat gains: people, lighting, equipment of any fuel, and
//! outdoor-temperature-controlled baseboard heat.
//!
//! Each gain type has a design level, a radiant/convective split, and an
//! optional schedule reference for time-varying operation. Equipment carries
//! the fuel it draws, which decides the meter its energy is booked to.

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
        /// Fuel the equipment draws (default electricity). It sets only the
        /// meter the power is booked to; the heat split is the same for every
        /// fuel, as in E+.
        #[serde(default)]
        fuel: EquipmentFuel,
    },
    /// Baseboard heat whose capacity follows the outdoor temperature (E+
    /// `ZoneBaseboard:OutdoorTemperatureControlled`): electric, metered as
    /// interior equipment.
    OutdoorControlledBaseboard {
        /// Capacity at and below `low_temperature` [W]
        capacity_at_low_temperature: f64,
        /// Outdoor temperature of the low-temperature capacity [°C]
        low_temperature: f64,
        /// Capacity just below `high_temperature` [W]; none at or above it
        capacity_at_high_temperature: f64,
        /// Outdoor temperature at and above which the baseboard is off [°C]
        high_temperature: f64,
        /// Fraction radiant [0-1] (default 0, as E+)
        #[serde(default)]
        radiant_fraction: f64,
        /// Schedule name, a fraction of the capacity (default: always on)
        #[serde(default)]
        schedule: Option<String>,
        /// Submeter label for end-use reporting (default: "General")
        #[serde(default = "default_submeter")]
        submeter: String,
    },
}

/// The fuel or energy source an equipment gain draws: the fuel types of E+
/// `OtherEquipment`, which cover `ElectricEquipment` (electricity),
/// `GasEquipment` (natural gas), `HotWaterEquipment` (district heating water)
/// and `SteamEquipment` (district heating steam). `none` meters nothing and,
/// as in E+, is the only fuel whose design level may be negative (a heat sink).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EquipmentFuel {
    #[default]
    Electricity,
    NaturalGas,
    Propane,
    FuelOilNo1,
    FuelOilNo2,
    Diesel,
    Gasoline,
    Coal,
    OtherFuel1,
    OtherFuel2,
    DistrictHeatingWater,
    DistrictHeatingSteam,
    DistrictCooling,
    None,
}

impl EquipmentFuel {
    pub const ALL: [EquipmentFuel; 14] = [
        Self::Electricity,
        Self::NaturalGas,
        Self::Propane,
        Self::FuelOilNo1,
        Self::FuelOilNo2,
        Self::Diesel,
        Self::Gasoline,
        Self::Coal,
        Self::OtherFuel1,
        Self::OtherFuel2,
        Self::DistrictHeatingWater,
        Self::DistrictHeatingSteam,
        Self::DistrictCooling,
        Self::None,
    ];

    /// The YAML name.
    pub fn key(self) -> &'static str {
        match self {
            Self::Electricity => "electricity",
            Self::NaturalGas => "natural_gas",
            Self::Propane => "propane",
            Self::FuelOilNo1 => "fuel_oil_no1",
            Self::FuelOilNo2 => "fuel_oil_no2",
            Self::Diesel => "diesel",
            Self::Gasoline => "gasoline",
            Self::Coal => "coal",
            Self::OtherFuel1 => "other_fuel1",
            Self::OtherFuel2 => "other_fuel2",
            Self::DistrictHeatingWater => "district_heating_water",
            Self::DistrictHeatingSteam => "district_heating_steam",
            Self::DistrictCooling => "district_cooling",
            Self::None => "none",
        }
    }

    /// The fuel with YAML name `key`.
    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|f| f.key() == key)
    }

    /// The summary report's fuel group, or `None` for unmetered equipment.
    pub fn group(self) -> Option<FuelGroup> {
        match self {
            Self::Electricity => Some(FuelGroup::Electricity),
            Self::NaturalGas => Some(FuelGroup::NaturalGas),
            Self::DistrictHeatingWater => Some(FuelGroup::DistrictHeatingWater),
            Self::DistrictHeatingSteam => Some(FuelGroup::DistrictHeatingSteam),
            Self::None => None,
            _ => Some(FuelGroup::OtherFuel),
        }
    }

    /// The submeter end-use key of equipment on this fuel: `equipment` for
    /// electricity, `equipment_<fuel>` otherwise, and `None` when unmetered.
    pub fn end_use_key(self) -> Option<String> {
        match self {
            Self::Electricity => Some("equipment".to_string()),
            Self::None => None,
            f => Some(format!("equipment_{}", f.key())),
        }
    }
}

/// The fuels the summary report totals separately. Propane, the fuel oils,
/// diesel, gasoline, coal, other fuels 1 and 2 and district cooling share
/// `OtherFuel`; submeters keep each one apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FuelGroup {
    Electricity,
    NaturalGas,
    DistrictHeatingWater,
    DistrictHeatingSteam,
    OtherFuel,
}

impl FuelGroup {
    pub const ALL: [FuelGroup; 5] = [
        Self::Electricity,
        Self::NaturalGas,
        Self::DistrictHeatingWater,
        Self::DistrictHeatingSteam,
        Self::OtherFuel,
    ];

    fn index(self) -> usize {
        self as usize
    }
}

/// Power drawn by fuel group [W].
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct FuelPowers([f64; 5]);

impl FuelPowers {
    pub fn get(&self, group: FuelGroup) -> f64 {
        self.0[group.index()]
    }

    pub fn add(&mut self, group: FuelGroup, watts: f64) {
        self.0[group.index()] += watts;
    }

    /// Every group multiplied by `k` (e.g. a zone multiplier).
    pub fn scaled(&self, k: f64) -> Self {
        Self(self.0.map(|w| w * k))
    }

    /// Sum over all groups [W].
    pub fn total(&self) -> f64 {
        self.0.iter().sum()
    }
}

/// Baseboard capacity at outdoor temperature `t_out` [W], before its
/// schedule: the low-temperature capacity at and below `t_low`, linear up to
/// the high-temperature capacity, and none at or above `t_high`
/// (E+ `InitInternalHeatGains`).
pub fn baseboard_capacity(t_out: f64, cap_low: f64, t_low: f64, cap_high: f64, t_high: f64) -> f64 {
    if t_out >= t_high {
        0.0
    } else if t_out > t_low {
        cap_low + (t_out - t_low) * (cap_high - cap_low) / (t_high - t_low)
    } else {
        cap_low
    }
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
    /// Equipment and baseboard power drawn this timestep, by fuel group [W]
    /// (scheduled; unmetered `none` equipment excluded)
    pub equipment_power: FuelPowers,
    /// People sensible heat this timestep [W] (scheduled)
    pub people_heat: f64,
    /// People latent heat this timestep [W] (scheduled, for humidity modeling)
    pub people_latent: f64,
    /// Equipment latent heat this timestep [W] (from latent_fraction)
    pub equipment_latent: f64,

    // ─── Per-type gain breakdown (conv + rad = heat entering zone) ────
    /// Lighting heat to zone [W] (conv + rad, excluding return air fraction)
    pub lighting_gain_to_zone: f64,
    /// Equipment and baseboard sensible heat to zone [W] (conv + rad,
    /// excluding lost fraction)
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
pub fn resolve_gains(
    gains: &[InternalGainInput],
    zone_air_temp: f64,
    t_outdoor: f64,
) -> ResolvedGain {
    resolve_gains_scheduled(gains, None, 1, 1, 1, zone_air_temp, t_outdoor)
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
/// `t_outdoor` — the outdoor dry-bulb temperature [°C] baseboard capacity
/// follows
pub fn resolve_gains_scheduled(
    gains: &[InternalGainInput],
    schedule_mgr: Option<&crate::schedule::ScheduleManager>,
    hour: u32,
    day_of_week: u32,
    day_of_year: u32,
    zone_air_temp: f64,
    t_outdoor: f64,
) -> ResolvedGain {
    resolve_gains_with(
        gains,
        schedule_mgr,
        (hour, day_of_week, day_of_year),
        ScheduleUse::AsScheduled,
        zone_air_temp,
        t_outdoor,
    )
}

/// How a gain's schedule fraction scales its design level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScheduleUse {
    /// The design level times the fraction.
    AsScheduled,
    /// The whole design level whenever the gain's own fraction is above
    /// zero, and nothing when it is zero (the `full_when_occupied` sizing
    /// mode). Each gain follows its own schedule.
    FullWhenOn,
}

/// Resolve gains at `(hour, day_of_week, day_of_year)` with the schedule
/// fractions applied as `schedule_use` says; see [`resolve_gains_scheduled`]
/// for the other arguments.
pub fn resolve_gains_with(
    gains: &[InternalGainInput],
    schedule_mgr: Option<&crate::schedule::ScheduleManager>,
    (hour, day_of_week, day_of_year): (u32, u32, u32),
    schedule_use: ScheduleUse,
    zone_air_temp: f64,
    t_outdoor: f64,
) -> ResolvedGain {
    let mut result = ResolvedGain::default();
    let fraction = |schedule: &Option<String>| {
        let f = schedule_fraction(schedule, schedule_mgr, hour, day_of_week, day_of_year);
        match schedule_use {
            ScheduleUse::AsScheduled => f,
            ScheduleUse::FullWhenOn if f > 0.0 => 1.0,
            ScheduleUse::FullWhenOn => 0.0,
        }
    };

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
                let frac = fraction(schedule);
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
                let frac = fraction(schedule);
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
                fuel,
            } => {
                let frac = fraction(schedule);
                let total = power * frac;
                // All fractions are of the total, as in E+'s equipment objects
                // (#186), whatever the fuel. Only the non-lost portion enters
                // the zone; latent generates moisture, not sensible heat.
                let latent = total * latent_fraction;
                let sensible = total * (1.0 - lost_fraction - latent_fraction);
                let radiant = total * radiant_fraction;
                result.radiative += radiant;
                result.convective += sensible - radiant;
                result.total += sensible;
                result.equipment_latent += latent;
                // The whole input power is metered on its fuel, lost heat included.
                if let Some(group) = fuel.group() {
                    result.equipment_power.add(group, total);
                }
                result.equipment_sensible_gain_to_zone += sensible;
            }
            InternalGainInput::OutdoorControlledBaseboard {
                capacity_at_low_temperature,
                low_temperature,
                capacity_at_high_temperature,
                high_temperature,
                radiant_fraction,
                schedule,
                submeter: _,
            } => {
                let frac = fraction(schedule);
                let total = frac
                    * baseboard_capacity(
                        t_outdoor,
                        *capacity_at_low_temperature,
                        *low_temperature,
                        *capacity_at_high_temperature,
                        *high_temperature,
                    );
                let radiant = total * radiant_fraction;
                result.radiative += radiant;
                result.convective += total - radiant;
                result.total += total;
                result.equipment_power.add(FuelGroup::Electricity, total);
                result.equipment_sensible_gain_to_zone += total;
            }
        }
    }

    result
}

impl InternalGainInput {
    /// The gain's schedule, if any.
    pub fn schedule(&self) -> Option<&str> {
        match self {
            Self::People { schedule, .. }
            | Self::Lights { schedule, .. }
            | Self::Equipment { schedule, .. }
            | Self::OutdoorControlledBaseboard { schedule, .. } => schedule.as_deref(),
        }
    }

    /// The energy this gain meters at schedule fraction `frac` and outdoor
    /// temperature `t_outdoor`: its submeter, its end-use key and the power
    /// [W]. People and unmetered (`none`) equipment meter nothing.
    pub fn metered_use(&self, frac: f64, t_outdoor: f64) -> Option<(&str, String, f64)> {
        match self {
            Self::People { .. } => None,
            Self::Lights {
                power, submeter, ..
            } => Some((submeter, "lighting".to_string(), power * frac)),
            Self::Equipment {
                power,
                submeter,
                fuel,
                ..
            } => fuel
                .end_use_key()
                .map(|key| (submeter.as_str(), key, power * frac)),
            Self::OutdoorControlledBaseboard {
                capacity_at_low_temperature,
                low_temperature,
                capacity_at_high_temperature,
                high_temperature,
                submeter,
                ..
            } => {
                let cap = baseboard_capacity(
                    t_outdoor,
                    *capacity_at_low_temperature,
                    *low_temperature,
                    *capacity_at_high_temperature,
                    *high_temperature,
                );
                Some((submeter, "equipment".to_string(), cap * frac))
            }
        }
    }
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
        let resolved = resolve_gains(&gains, 24.0, 0.0);
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
            let r = resolve_gains(&[people(None)], t, 0.0);
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
            let r = resolve_gains(&[people(Some(0.6))], t, 0.0);
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
        let resolved = resolve_gains(&gains, 24.0, 0.0);
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
                fuel: EquipmentFuel::Electricity,
            },
        ];
        let resolved = resolve_gains(&gains, 24.0, 0.0);
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
            fuel: EquipmentFuel::Electricity,
        }];

        // Weekday: 50% schedule → 500W
        let resolved = resolve_gains_scheduled(&gains, Some(&schedules), 12, 1, 1, 24.0, 0.0);
        assert_relative_eq!(resolved.total, 500.0);

        // Weekend: 0% schedule → 0W
        let resolved = resolve_gains_scheduled(&gains, Some(&schedules), 12, 6, 1, 24.0, 0.0);
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
            fuel: EquipmentFuel::Electricity,
        }];
        let resolved = resolve_gains(&gains, 24.0, 0.0);
        // 1000W total, 950W lost, 50W to zone: 15W radiant, 35W convective
        assert_relative_eq!(resolved.total, 50.0, epsilon = 1e-10);
        assert_relative_eq!(resolved.radiative, 15.0, epsilon = 1e-10);
        assert_relative_eq!(resolved.convective, 35.0, epsilon = 1e-10);
        // Full 1000W reported for electricity accounting
        assert_relative_eq!(resolved.equipment_power.get(FuelGroup::Electricity), 1000.0);
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
        let r = resolve_gains(&gains, 24.0, 0.0);
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
            fuel: EquipmentFuel::Electricity,
        }];
        let r = resolve_gains(&gains, 24.0, 0.0);
        assert!((r.radiative - 300.0).abs() < 1e-9);
        assert!((r.equipment_latent - 200.0).abs() < 1e-9);
        assert!((r.convective - 400.0).abs() < 1e-9);
        let lost = 100.0;
        assert!((r.radiative + r.convective + r.equipment_latent + lost - 1000.0).abs() < 1e-9);
    }

    /// `full_when_occupied` sizing: each gain at its whole design level in
    /// hours its own schedule is above zero, and off in hours it is zero, so
    /// a gain that runs all night doesn't switch the zone's other gains on.
    #[test]
    fn full_when_on_follows_each_gains_own_schedule() {
        use crate::schedule::{ScheduleInput, ScheduleManager};
        let day = |name: &str, night: f64, daytime: f64| ScheduleInput {
            name: name.to_string(),
            weekday: (1..=24)
                .map(|h| {
                    if (9..=17).contains(&h) {
                        daytime
                    } else {
                        night
                    }
                })
                .collect(),
            weekend: None,
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
        };
        let schedules = ScheduleManager::from_inputs(vec![
            day("occupancy", 0.0, 0.8),
            day("lighting", 0.05, 0.9),
            day("refrigeration", 0.5, 0.5),
        ]);
        let gains = vec![
            InternalGainInput::People {
                count: 10.0,
                activity_level: 120.0,
                sensible_fraction: Some(0.5),
                radiant_fraction: 0.3,
                schedule: Some("occupancy".to_string()),
                sensible_gain_per_person: None,
                latent_gain_per_person: None,
            },
            InternalGainInput::Lights {
                power: 1000.0,
                radiant_fraction: 0.7,
                return_air_fraction: 0.0,
                schedule: Some("lighting".to_string()),
                submeter: "General".to_string(),
            },
            InternalGainInput::Equipment {
                power: 400.0,
                radiant_fraction: 0.0,
                lost_fraction: 0.0,
                latent_fraction: 0.0,
                schedule: Some("refrigeration".to_string()),
                submeter: "General".to_string(),
                fuel: EquipmentFuel::Electricity,
            },
            InternalGainInput::Equipment {
                power: 200.0,
                radiant_fraction: 0.0,
                lost_fraction: 0.0,
                latent_fraction: 0.0,
                schedule: Some("always_off".to_string()),
                submeter: "General".to_string(),
                fuel: EquipmentFuel::Electricity,
            },
        ];
        let at = |hour| {
            resolve_gains_with(
                &gains,
                Some(&schedules),
                (hour, 1, 1),
                ScheduleUse::FullWhenOn,
                24.0,
                0.0,
            )
        };
        // Night: people off; the 5% night lighting and the refrigeration are
        // on, so both run at their design levels.
        let night = at(3);
        assert_eq!(night.people_heat, 0.0);
        assert_relative_eq!(night.lighting_power, 1000.0);
        assert_relative_eq!(night.equipment_power.get(FuelGroup::Electricity), 400.0);
        // Day: everything that is on runs at its design level; the gain whose
        // schedule is always off stays off.
        let day = at(12);
        assert_relative_eq!(day.people_heat, 600.0);
        assert_relative_eq!(day.lighting_power, 1000.0);
        assert_relative_eq!(day.equipment_power.get(FuelGroup::Electricity), 400.0);
        // As scheduled, the same hour scales each gain by its fraction.
        let scheduled = resolve_gains_scheduled(&gains, Some(&schedules), 12, 1, 1, 24.0, 0.0);
        assert_relative_eq!(scheduled.people_heat, 480.0);
        assert_relative_eq!(scheduled.lighting_power, 900.0);
        assert_relative_eq!(scheduled.equipment_power.get(FuelGroup::Electricity), 200.0);
    }

    fn equipment(power: f64, fuel: EquipmentFuel) -> InternalGainInput {
        InternalGainInput::Equipment {
            power,
            radiant_fraction: 0.3,
            lost_fraction: 0.25,
            latent_fraction: 0.05,
            schedule: None,
            submeter: "Kitchen".to_string(),
            fuel,
        }
    }

    /// E+ `InitInternalHeatGains` gives every equipment type the same split
    /// (Q·FractionRadiant, Q·FractionConvected, Q·FractionLatent, Q·FractionLost,
    /// FractionConvected = 1 − the other three) and meters the whole Q on the
    /// object's resource: Electricity, NaturalGas, DistrictHeatingWater,
    /// DistrictHeatingSteam, or OtherEquipment's fuel (none for `None`).
    #[test]
    fn every_fuel_splits_heat_alike_and_meters_its_whole_input() {
        let (q, rad, lost, lat) = (1000.0, 0.3, 0.25, 0.05);
        for fuel in EquipmentFuel::ALL {
            let r = resolve_gains(&[equipment(q, fuel)], 24.0, 0.0);
            assert_relative_eq!(r.radiative, q * rad, epsilon = 1e-9);
            assert_relative_eq!(r.convective, q * (1.0 - rad - lost - lat), epsilon = 1e-9);
            assert_relative_eq!(r.equipment_latent, q * lat, epsilon = 1e-9);
            // Radiant + convective + latent + lost is the whole input.
            assert_relative_eq!(
                r.radiative + r.convective + r.equipment_latent + q * lost,
                q,
                epsilon = 1e-9
            );
            assert_relative_eq!(
                r.equipment_sensible_gain_to_zone,
                q * (1.0 - lost - lat),
                epsilon = 1e-9
            );
            match fuel.group() {
                Some(g) => {
                    assert_relative_eq!(r.equipment_power.get(g), q);
                    assert_relative_eq!(r.equipment_power.total(), q);
                }
                None => assert_eq!(r.equipment_power.total(), 0.0),
            }
        }
        use EquipmentFuel as F;
        assert_eq!(F::Electricity.group(), Some(FuelGroup::Electricity));
        assert_eq!(F::NaturalGas.group(), Some(FuelGroup::NaturalGas));
        assert_eq!(
            F::DistrictHeatingWater.group(),
            Some(FuelGroup::DistrictHeatingWater)
        );
        assert_eq!(
            F::DistrictHeatingSteam.group(),
            Some(FuelGroup::DistrictHeatingSteam)
        );
        for f in [
            F::Propane,
            F::FuelOilNo1,
            F::FuelOilNo2,
            F::Diesel,
            F::Gasoline,
            F::Coal,
        ] {
            assert_eq!(f.group(), Some(FuelGroup::OtherFuel));
        }
        for f in [F::OtherFuel1, F::OtherFuel2, F::DistrictCooling] {
            assert_eq!(f.group(), Some(FuelGroup::OtherFuel));
        }
        assert_eq!(F::None.group(), None);
    }

    #[test]
    fn fuel_names_round_trip() {
        for fuel in EquipmentFuel::ALL {
            let yaml = serde_yaml::to_string(&fuel).unwrap();
            assert_eq!(yaml.trim(), fuel.key());
            assert_eq!(EquipmentFuel::from_key(fuel.key()), Some(fuel));
            let back: EquipmentFuel = serde_yaml::from_str(fuel.key()).unwrap();
            assert_eq!(back, fuel);
        }
        let g: InternalGainInput =
            serde_yaml::from_str("{type: equipment, power: 100, fuel: natural_gas}").unwrap();
        assert!(matches!(
            g,
            InternalGainInput::Equipment {
                fuel: EquipmentFuel::NaturalGas,
                ..
            }
        ));
        // Without a fuel, equipment is electric.
        let g: InternalGainInput = serde_yaml::from_str("{type: equipment, power: 100}").unwrap();
        assert!(matches!(
            g,
            InternalGainInput::Equipment {
                fuel: EquipmentFuel::Electricity,
                ..
            }
        ));
    }

    /// E+ OtherEquipment with fuel None and a negative level is a heat sink
    /// (the ASHRAE 140 AE cases' imposed heating loads).
    #[test]
    fn unmetered_negative_equipment_removes_heat() {
        let sink = InternalGainInput::Equipment {
            power: -2931.0,
            radiant_fraction: 0.0,
            lost_fraction: 0.0,
            latent_fraction: 0.0,
            schedule: None,
            submeter: "General".to_string(),
            fuel: EquipmentFuel::None,
        };
        let r = resolve_gains(&[sink.clone()], 21.0, -29.0);
        assert_relative_eq!(r.convective, -2931.0);
        assert_relative_eq!(r.total, -2931.0);
        assert_eq!(r.equipment_power.total(), 0.0);
        assert!(sink.metered_use(1.0, 0.0).is_none());
    }

    #[test]
    fn metered_use_keys_follow_the_fuel() {
        let key = |fuel| {
            equipment(1000.0, fuel)
                .metered_use(0.5, 0.0)
                .map(|(m, k, w)| (m.to_string(), k, w))
        };
        let kitchen = |k: &str| Some(("Kitchen".to_string(), k.to_string(), 500.0));
        assert_eq!(key(EquipmentFuel::Electricity), kitchen("equipment"));
        assert_eq!(
            key(EquipmentFuel::NaturalGas),
            kitchen("equipment_natural_gas")
        );
        assert_eq!(
            key(EquipmentFuel::DistrictHeatingWater),
            kitchen("equipment_district_heating_water")
        );
        assert_eq!(key(EquipmentFuel::Propane), kitchen("equipment_propane"));
        assert_eq!(key(EquipmentFuel::None), None);
    }

    /// E+ `InitInternalHeatGains` for `ZoneBaseboard:OutdoorTemperatureControlled`:
    /// none at or above the high temperature, linear between the two points,
    /// the low-temperature capacity at and below the low temperature, all times
    /// the schedule; radiant by its fraction and the rest convective.
    #[test]
    fn baseboard_capacity_follows_eplus() {
        let cap = |t| baseboard_capacity(t, 1500.0, -10.0, 500.0, 15.0);
        assert_eq!(cap(-30.0), 1500.0);
        assert_eq!(cap(-10.0), 1500.0);
        assert_relative_eq!(cap(0.0), 1100.0, epsilon = 1e-9);
        assert_relative_eq!(cap(14.999), 500.04, epsilon = 1e-9);
        assert_eq!(cap(15.0), 0.0);
        assert_eq!(cap(25.0), 0.0);

        use crate::schedule::{ScheduleInput, ScheduleManager};
        let schedules = ScheduleManager::from_inputs(vec![ScheduleInput {
            name: "half".to_string(),
            weekday: vec![0.5; 24],
            weekend: None,
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
        let bb = InternalGainInput::OutdoorControlledBaseboard {
            capacity_at_low_temperature: 1500.0,
            low_temperature: -10.0,
            capacity_at_high_temperature: 500.0,
            high_temperature: 15.0,
            radiant_fraction: 0.4,
            schedule: Some("half".to_string()),
            submeter: "Lobby".to_string(),
        };
        let r = resolve_gains_scheduled(
            std::slice::from_ref(&bb),
            Some(&schedules),
            12,
            1,
            1,
            21.0,
            0.0,
        );
        assert_relative_eq!(r.total, 550.0, epsilon = 1e-9);
        assert_relative_eq!(r.radiative, 220.0, epsilon = 1e-9);
        assert_relative_eq!(r.convective, 330.0, epsilon = 1e-9);
        assert_relative_eq!(
            r.equipment_power.get(FuelGroup::Electricity),
            550.0,
            epsilon = 1e-9
        );
        assert_eq!(r.equipment_latent, 0.0);
        // Metered as electric interior equipment.
        let (meter, key, w) = bb.metered_use(0.5, 0.0).unwrap();
        assert_eq!((meter, key.as_str()), ("Lobby", "equipment"));
        assert_relative_eq!(w, 550.0, epsilon = 1e-9);
    }
}
