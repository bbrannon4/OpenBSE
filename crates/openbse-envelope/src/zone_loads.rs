//! Top-level zone load definitions.
//!
//! People, lights, equipment, infiltration, ventilation, exhaust fans,
//! outdoor air, and ideal loads are defined as independent objects that
//! reference one or more zones (or zone groups) by name. This allows
//! a single definition to be shared across multiple zones.

use serde::{Deserialize, Serialize};
use crate::zone::ThermostatScheduleEntry;

/// Top-level people definition, assignable to zones or zone groups.
///
/// Supports three specification methods (use exactly one):
///   - `count`: absolute number of people
///   - `people_per_area`: occupant density [people/m²] (multiplied by zone floor area)
///   - `area_per_person`: inverse density [m²/person] (floor area divided by this)
///
/// ```yaml
/// people:
///   - name: Office Workers
///     zones: [East Office, West Office]
///     people_per_area: 0.05   # 1 person per 20 m²
///     activity_level: 120.0
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeopleInput {
    pub name: String,
    /// Zone or zone-group names this applies to
    pub zones: Vec<String>,
    /// Number of people (absolute count)
    #[serde(default)]
    pub count: f64,
    /// Alternative: occupant density [people/m²] (multiplied by zone floor area)
    #[serde(default)]
    pub people_per_area: Option<f64>,
    /// Alternative: floor area per person [m²/person] (zone floor area / this = count)
    #[serde(default)]
    pub area_per_person: Option<f64>,
    /// Activity level [W/person] — total metabolic heat output (default 120).
    /// This is split into sensible and latent components using `sensible_fraction`.
    #[serde(default = "default_activity")]
    pub activity_level: f64,
    /// Sensible fraction of metabolic heat [0-1] (default 0.6).
    /// Sensible heat = activity_level × sensible_fraction.
    /// Latent heat  = activity_level × (1 - sensible_fraction).
    /// Typical values by activity: 0.62 seated/quiet, 0.58 moderate office,
    /// 0.50 walking, 0.38 heavy exercise (per ASHRAE Fundamentals Ch.18).
    #[serde(default = "default_sensible_fraction")]
    pub sensible_fraction: f64,
    /// Fraction of gain that is radiant [0-1] (default 0.3)
    #[serde(default = "default_people_radiant")]
    pub radiant_fraction: f64,
    /// Schedule name for time-varying occupancy
    #[serde(default)]
    pub schedule: Option<String>,
    /// Alternative: explicit sensible gain [W/person].
    /// When set, overrides `activity_level × sensible_fraction`.
    #[serde(default)]
    pub sensible_gain_per_person: Option<f64>,
    /// Alternative: explicit latent gain [W/person].
    /// When set, overrides `activity_level × (1 - sensible_fraction)`.
    #[serde(default)]
    pub latent_gain_per_person: Option<f64>,
}

fn default_activity() -> f64 { 120.0 }
fn default_sensible_fraction() -> f64 { 0.6 }
fn default_people_radiant() -> f64 { 0.3 }

/// Top-level lights definition, assignable to zones or zone groups.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LightsInput {
    pub name: String,
    /// Zone or zone-group names this applies to
    pub zones: Vec<String>,
    /// Total installed power [W]
    #[serde(default)]
    pub power: f64,
    /// Alternative: power density [W/m²] (multiplied by zone floor area)
    #[serde(default)]
    pub watts_per_area: Option<f64>,
    /// Fraction radiant [0-1] (default 0.7)
    #[serde(default = "default_lights_radiant")]
    pub radiant_fraction: f64,
    /// Fraction to return air [0-1] (default 0.0)
    #[serde(default)]
    pub return_air_fraction: f64,
    /// Schedule name for time-varying lighting
    #[serde(default)]
    pub schedule: Option<String>,
}

fn default_lights_radiant() -> f64 { 0.7 }

/// Top-level equipment definition, assignable to zones or zone groups.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EquipmentGainInput {
    pub name: String,
    /// Zone or zone-group names this applies to
    pub zones: Vec<String>,
    /// Total installed power [W]
    #[serde(default)]
    pub power: f64,
    /// Alternative: power density [W/m²]
    #[serde(default)]
    pub watts_per_area: Option<f64>,
    /// Fraction radiant [0-1] (default 0.3)
    #[serde(default = "default_equip_radiant")]
    pub radiant_fraction: f64,
    /// Fraction of heat that is "lost" (does not enter the zone) [0-1] (default 0.0).
    /// Matches E+ ElectricEquipment "Fraction Lost" field.
    /// Example: elevator with Lost=0.95 means only 5% of heat enters the zone.
    #[serde(default)]
    pub lost_fraction: f64,
    /// Schedule name for time-varying equipment
    #[serde(default)]
    pub schedule: Option<String>,
}

fn default_equip_radiant() -> f64 { 0.3 }

/// Controls how infiltration interacts with exhaust fans and HVAC airflows.
///
/// - `basic` (default): Infiltration is a fixed rate, independent of exhaust
///   or HVAC. Matches EnergyPlus ZoneInfiltration:DesignFlowRate behavior
///   (without AirflowNetwork). Exhaust fans remove zone air but do NOT
///   increase infiltration.
///
/// - `ashrae_combined`: ASHRAE combined infiltration model. When exhaust fans
///   create unbalanced flow, outdoor air enters through envelope cracks:
///   ```
///   Q_combined = sqrt(Q_infiltration² + Q_unbalanced_exhaust²)
///   ```
///   More physically correct but gives different results than E+ without AFN.
///
/// ```yaml
/// simulation:
///   infiltration_interaction: basic           # default
///   # infiltration_interaction: ashrae_combined  # ASHRAE model
/// ```
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum InfiltrationInteraction {
    /// Fixed infiltration rate, independent of exhaust/HVAC (E+ default)
    #[default]
    Basic,
    /// ASHRAE combined: sqrt(infil² + unbalanced_exhaust²)
    AshraeCombined,
}

/// Top-level infiltration definition, assignable to zones or zone groups.
///
/// Supports four specification methods (use exactly one):
///   - `design_flow_rate`: absolute volume flow [m³/s]
///   - `air_changes_per_hour`: ACH (converted using zone volume)
///   - `flow_per_floor_area`: flow per zone floor area [m³/s/m²]
///   - `flow_per_exterior_wall_area`: flow per exterior wall area [m³/s/m²]
///
/// ```yaml
/// infiltration:
///   - name: Office Infiltration
///     zones: [East Office, West Office]
///     flow_per_exterior_wall_area: 0.000302   # ASHRAE 90.1 baseline
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InfiltrationTopLevel {
    pub name: String,
    /// Zone or zone-group names this applies to
    pub zones: Vec<String>,
    /// Design infiltration volume flow rate [m³/s]
    #[serde(default)]
    pub design_flow_rate: f64,
    /// Alternative: air changes per hour (converted using zone volume)
    #[serde(default)]
    pub air_changes_per_hour: f64,
    /// Alternative: flow per zone floor area [m³/s per m²]
    #[serde(default)]
    pub flow_per_floor_area: Option<f64>,
    /// Alternative: flow per exterior wall area [m³/s per m²]
    /// (exterior wall area is computed from the zone's outdoor-boundary wall surfaces)
    #[serde(default)]
    pub flow_per_exterior_wall_area: Option<f64>,
    /// Constant coefficient A (default 1.0)
    #[serde(default = "default_coeff_a")]
    pub constant_coefficient: f64,
    /// Temperature coefficient B [1/°C]
    #[serde(default)]
    pub temperature_coefficient: f64,
    /// Wind speed coefficient C [s/m]
    #[serde(default)]
    pub wind_coefficient: f64,
    /// Wind speed squared coefficient D [s²/m²]
    #[serde(default)]
    pub wind_squared_coefficient: f64,
    /// Schedule name for time-varying infiltration multiplier
    #[serde(default)]
    pub schedule: Option<String>,
}

fn default_coeff_a() -> f64 { 1.0 }

/// Method for combining multiple ventilation requirements for a zone.
///
/// When multiple ventilation objects target the same zone, this controls
/// how the total ventilation is computed:
///   - `Sum` (default): add all requirements together
///   - `Maximum`: take the largest individual requirement
///
/// ```yaml
/// ventilation:
///   - name: Code Minimum OA
///     zones: [Conference Room]
///     per_person: 0.00236        # 5 cfm/person
///     per_area: 0.000305         # 0.06 cfm/ft²
///     combining_method: maximum   # take the larger of per-person and per-area
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum VentilationCombiningMethod {
    /// Sum all ventilation requirements (default)
    #[default]
    Sum,
    /// Take the maximum individual requirement
    Maximum,
}

/// Top-level ventilation definition, assignable to zones or zone groups.
///
/// Supports multiple specification methods (can be combined):
///   - `flow_rate`: absolute volume flow [m³/s]
///   - `ach_rate` / `air_changes_per_hour`: ACH (converted using zone volume)
///   - `per_person`: outdoor air per person [m³/s/person] (ASHRAE 62.1 Rp)
///   - `per_area`: outdoor air per floor area [m³/s/m²] (ASHRAE 62.1 Ra)
///
/// When multiple methods are specified, `combining_method` controls whether
/// they are summed (default, ASHRAE 62.1 ventilation rate procedure) or
/// the maximum is used.
///
/// For scheduled night ventilation (ASHRAE 140 Case 650), use `start_hour`
/// and `end_hour` with optional temperature conditions.
///
/// ```yaml
/// ventilation:
///   - name: ASHRAE 62.1 Office
///     zones: [East Office, West Office]
///     per_person: 0.00236       # 5 cfm/person
///     per_area: 0.000305        # 0.06 cfm/ft²
///     combining_method: sum      # ASHRAE 62.1 VRP: Rp*Pz + Ra*Az
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VentilationTopLevel {
    pub name: String,
    /// Zone or zone-group names this applies to
    pub zones: Vec<String>,
    /// Start hour of ventilation schedule (0-23)
    #[serde(default)]
    pub start_hour: u32,
    /// End hour of ventilation schedule (0-23)
    #[serde(default = "default_end_hour")]
    pub end_hour: u32,
    /// Ventilation flow rate [m³/s]
    #[serde(default)]
    pub flow_rate: f64,
    /// Alternative: air changes per hour (converted using zone volume)
    #[serde(default, alias = "air_changes_per_hour")]
    pub ach_rate: f64,
    /// Outdoor air per person [m³/s/person] (ASHRAE 62.1 Rp component)
    #[serde(default)]
    pub per_person: Option<f64>,
    /// Outdoor air per floor area [m³/s/m²] (ASHRAE 62.1 Ra component)
    #[serde(default)]
    pub per_area: Option<f64>,
    /// Method for combining multiple specification methods: "sum" (default) or "maximum"
    #[serde(default)]
    pub combining_method: VentilationCombiningMethod,
    /// Minimum indoor temperature for ventilation [°C]
    #[serde(default)]
    pub min_indoor_temp: Option<f64>,
    /// Only ventilate when outdoor temp < indoor temp
    #[serde(default)]
    pub outdoor_temp_must_be_lower: Option<bool>,
}

fn default_end_hour() -> u32 { 24 }

/// Top-level exhaust fan definition, assignable to zones or zone groups.
///
/// Models air being removed from a zone (restroom exhaust, kitchen hood, etc.).
/// Uses the same physics as the supply/return Fan component:
///   Power = MassFlow × PressureRise / (TotalEfficiency × ρ_air)
///
/// ```yaml
/// exhaust_fans:
///   - name: Zone Exhaust Fan
///     zones: [Living]
///     flow_rate: 0.0283168       # 60 cfm
///     pressure_rise: 454.046     # Pa
///     total_efficiency: 0.6
///     motor_efficiency: 0.863
///     motor_in_airstream_fraction: 1.0
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExhaustFanTopLevel {
    pub name: String,
    /// Zone or zone-group names this applies to
    pub zones: Vec<String>,
    /// Free-form tag for output classification (default "exhaust").
    /// Used for end-use subcategory reporting.
    #[serde(default = "default_exhaust_tag")]
    pub tag: String,
    /// Exhaust flow rate [m³/s]
    pub flow_rate: f64,
    /// Design pressure rise [Pa] (default 0 → no power consumption)
    #[serde(default)]
    pub pressure_rise: f64,
    /// Total fan efficiency (fan × belt × motor × VFD) [0-1] (default 0.6)
    #[serde(default = "default_exhaust_total_eff")]
    pub total_efficiency: f64,
    /// Motor efficiency [0-1] (default 0.9)
    #[serde(default = "default_exhaust_motor_eff")]
    pub motor_efficiency: f64,
    /// Fraction of motor waste heat entering the airstream [0-1] (default 1.0).
    /// For zone exhaust fans the motor is usually in the exhaust stream,
    /// so motor heat exits with the exhaust air (does not warm the zone).
    #[serde(default = "default_exhaust_motor_in_air")]
    pub motor_in_airstream_fraction: f64,
    /// Schedule name
    #[serde(default)]
    pub schedule: Option<String>,
}

fn default_exhaust_tag() -> String { "exhaust".to_string() }
fn default_exhaust_total_eff() -> f64 { 0.6 }
fn default_exhaust_motor_eff() -> f64 { 0.9 }
fn default_exhaust_motor_in_air() -> f64 { 1.0 }

/// Method for combining per-person and per-area outdoor air rates.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum OaMethod {
    /// Total OA = per_person × people + per_area × floor_area (default, ASHRAE 62.1)
    Sum,
    /// Total OA = max(per_person × people, per_area × floor_area)
    Maximum,
}
impl Default for OaMethod { fn default() -> Self { OaMethod::Sum } }

/// Top-level outdoor air definition, assignable to zones or zone groups.
///
/// Specifies both **supply** outdoor air requirements and **exhaust** air
/// requirements for a zone. Supply and exhaust each support four specification
/// methods (all defaulting to 0; use one or combine as needed):
///
/// **Supply OA** (existing + new):
///   - `per_person`: per occupant [m³/s/person] (ASHRAE 62.1 Rp)
///   - `per_area`: per floor area [m³/s/m²] (ASHRAE 62.1 Ra)
///   - `absolute`: fixed volume flow [m³/s]
///   - `ach`: air changes per hour
///
/// **Exhaust** (new):
///   - `exhaust_per_person`, `exhaust_per_area`, `exhaust_absolute`, `exhaust_ach`
///
/// ```yaml
/// outdoor_air:
///   - name: Living OA
///     zones: [Living]
///     per_person: 0.00236
///     per_area: 0.0003
///     exhaust_absolute: 0.02832   # 60 cfm kitchen exhaust
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutdoorAirTopLevel {
    pub name: String,
    /// Zone or zone-group names this applies to
    pub zones: Vec<String>,

    // ── Supply outdoor air ────────────────────────────────────────
    /// Outdoor air per person [m³/s/person]
    #[serde(default)]
    pub per_person: f64,
    /// Outdoor air per floor area [m³/s/m²]
    #[serde(default)]
    pub per_area: f64,
    /// Absolute supply outdoor air flow [m³/s]
    #[serde(default)]
    pub absolute: f64,
    /// Supply outdoor air as air changes per hour [1/hr]
    #[serde(default)]
    pub ach: f64,
    /// Method for combining supply OA rates: "sum" (default) or "maximum"
    #[serde(default)]
    pub oa_method: OaMethod,

    // ── Exhaust air requirements ──────────────────────────────────
    /// Exhaust air per person [m³/s/person]
    #[serde(default)]
    pub exhaust_per_person: f64,
    /// Exhaust air per floor area [m³/s/m²]
    #[serde(default)]
    pub exhaust_per_area: f64,
    /// Absolute exhaust air flow [m³/s]
    #[serde(default)]
    pub exhaust_absolute: f64,
    /// Exhaust air as air changes per hour [1/hr]
    #[serde(default)]
    pub exhaust_ach: f64,
    /// Method for combining exhaust rates: "sum" (default) or "maximum"
    #[serde(default)]
    pub exhaust_method: OaMethod,
}

/// Top-level ideal loads definition, assignable to zones or zone groups.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdealLoadsTopLevel {
    pub name: String,
    /// Zone or zone-group names this applies to
    pub zones: Vec<String>,
    /// Heating setpoint [°C]
    #[serde(default = "default_heating_setpoint")]
    pub heating_setpoint: f64,
    /// Cooling setpoint [°C]
    #[serde(default = "default_cooling_setpoint")]
    pub cooling_setpoint: f64,
    /// Maximum heating capacity [W] (default 1 MW)
    #[serde(default = "default_capacity")]
    pub heating_capacity: f64,
    /// Maximum cooling capacity [W] (default 1 MW)
    #[serde(default = "default_capacity")]
    pub cooling_capacity: f64,
    /// Thermostat schedule for time-of-day setpoint changes (e.g., Case 640 nighttime setback)
    #[serde(default)]
    pub thermostat_schedule: Vec<ThermostatScheduleEntry>,
}

fn default_heating_setpoint() -> f64 { 20.0 }
fn default_cooling_setpoint() -> f64 { 27.0 }
fn default_capacity() -> f64 { 1_000_000.0 }

// ─── Thermostat ─────────────────────────────────────────────────────────────

/// Top-level thermostat definition, assignable to zones or zone groups.
///
/// A thermostat defines only **temperature goals** for a zone. It does not
/// specify how those goals are achieved (supply temperatures, flow rates, etc.)
/// — that belongs on the air loop controls.
///
/// ```yaml
/// thermostats:
///   - name: Office Thermostat
///     zones: [Office Zones]       # references a zone_group name
///     heating_setpoint: 21.1
///     cooling_setpoint: 23.9
///     unoccupied_heating_setpoint: 15.56
///     unoccupied_cooling_setpoint: 29.44
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThermostatInput {
    pub name: String,
    /// Zone or zone-group names this thermostat controls
    pub zones: Vec<String>,
    /// Occupied heating setpoint [°C] (default 21.1)
    #[serde(default = "default_tstat_heating")]
    pub heating_setpoint: f64,
    /// Occupied cooling setpoint [°C] (default 23.9)
    #[serde(default = "default_tstat_cooling")]
    pub cooling_setpoint: f64,
    /// Unoccupied (night setback) heating setpoint [°C] (default 15.56 / 60°F)
    #[serde(default = "default_unocc_heating")]
    pub unoccupied_heating_setpoint: f64,
    /// Unoccupied (night setback) cooling setpoint [°C] (default 29.44 / 85°F)
    #[serde(default = "default_unocc_cooling")]
    pub unoccupied_cooling_setpoint: f64,
}

fn default_tstat_heating() -> f64 { 21.1 }    // 70°F
fn default_tstat_cooling() -> f64 { 23.9 }    // 75°F
fn default_unocc_heating() -> f64 { 15.56 }   // 60°F (ASHRAE 90.1 default setback)
fn default_unocc_cooling() -> f64 { 29.44 }   // 85°F (ASHRAE 90.1 default setup)
