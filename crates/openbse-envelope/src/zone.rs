//! Zone definition and zone air heat balance solver.
//!
//! Implements the EnergyPlus predictor-corrector zone air heat balance:
//!
//!   T_zone = (SumHAT + MCPI·Tout + MCPSYS·Tsup + Qconv + Cap·Tprev)
//!          / (SumHA + MCPI + MCPSYS + Cap)
//!
//! Also implements the ideal loads air system for ASHRAE 140 validation:
//!   1. Solve zone temp without HVAC (free-float)
//!   2. If T_free < T_heat_sp → compute Q needed to reach T_heat_sp
//!   3. If T_free > T_cool_sp → compute Q needed to reach T_cool_sp
//!   4. Clamp Q to capacity limits
//!   5. Re-solve zone temp with clamped Q
//!
//! Reference: EnergyPlus ZoneTempPredictorCorrector.cc, TARP Manual (1983).

use crate::infiltration::InfiltrationInput;
use crate::internal_gains::InternalGainInput;
use serde::{Deserialize, Deserializer, Serialize};

/// Custom deserializer that accepts either a single InfiltrationInput or a list.
/// This provides backward compatibility: `infiltration: {single}` still works,
/// while `infiltration: [{obj1}, {obj2}]` supports multiple infiltration objects
/// per zone (e.g., envelope cracks + door opening for vestibule zones).
fn deserialize_infiltration_list<'de, D>(
    deserializer: D,
) -> Result<Vec<InfiltrationInput>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum SingleOrVec {
        Single(InfiltrationInput),
        Vec(Vec<InfiltrationInput>),
    }
    match SingleOrVec::deserialize(deserializer)? {
        SingleOrVec::Single(v) => Ok(vec![v]),
        SingleOrVec::Vec(v) => Ok(v),
    }
}

/// Ideal loads air system — a perfect HVAC system that directly adds/removes
/// energy from the zone air node. Used for ASHRAE 140 validation and load
/// calculations where equipment modeling is not needed.
///
/// Implements nonproportional (on/off) control:
///   - If T_zone < heating_setpoint → add energy to reach setpoint
///   - If T_zone > cooling_setpoint → remove energy to reach setpoint
///   - In deadband → no HVAC energy
///
/// All energy is 100% convective to zone air (ASHRAE 140 requirement).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdealLoadsAirSystem {
    /// Maximum heating capacity [W] (default: 1,000,000 = 1 MW)
    #[serde(default = "default_ideal_capacity")]
    pub heating_capacity: f64,
    /// Maximum cooling capacity [W] (default: 1,000,000 = 1 MW)
    #[serde(default = "default_ideal_capacity")]
    pub cooling_capacity: f64,
    /// Heating setpoint [°C] (overridden by thermostat schedule if present)
    #[serde(default = "default_heating_sp")]
    pub heating_setpoint: f64,
    /// Cooling setpoint [°C] (overridden by thermostat schedule if present)
    #[serde(default = "default_cooling_sp")]
    pub cooling_setpoint: f64,
}

fn default_ideal_capacity() -> f64 {
    1_000_000.0
}
fn default_heating_sp() -> f64 {
    20.0
}
fn default_cooling_sp() -> f64 {
    27.0
}

impl Default for IdealLoadsAirSystem {
    fn default() -> Self {
        Self {
            heating_capacity: default_ideal_capacity(),
            cooling_capacity: default_ideal_capacity(),
            heating_setpoint: default_heating_sp(),
            cooling_setpoint: default_cooling_sp(),
        }
    }
}

/// Thermostat schedule entry — defines setpoints for a time period.
///
/// Used for Case 640 (thermostat setback): different setpoints at different
/// times of day, with linear ramp between periods.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ThermostatScheduleEntry {
    /// Start hour (0-23, inclusive)
    pub start_hour: u32,
    /// End hour (0-23, inclusive). If end < start, wraps past midnight.
    pub end_hour: u32,
    /// Heating setpoint during this period [°C]
    pub heating_setpoint: f64,
    /// Cooling setpoint during this period [°C]
    pub cooling_setpoint: f64,
}

/// Ventilation schedule entry — defines extra ventilation for a time period.
///
/// Used for Case 650 (night ventilation): scheduled mechanical ventilation
/// at high air change rates during specific hours.
///
/// Optional temperature conditions (ASHRAE 140-2023, Case 650):
///   - `min_indoor_temp`: Only ventilate if zone temp >= this value [°C]
///   - `max_outdoor_temp_delta`: Only ventilate if T_outdoor < T_zone - delta [°C]
///
/// If neither condition is set, ventilation is unconditional during the schedule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VentilationScheduleEntry {
    /// Start hour (0-23, inclusive)
    pub start_hour: u32,
    /// End hour (0-23, inclusive). If end < start, wraps past midnight.
    pub end_hour: u32,
    /// Additional ventilation flow rate [m³/s] (or use ach_rate)
    #[serde(default)]
    pub flow_rate: f64,
    /// Additional ventilation air changes per hour
    #[serde(default, alias = "air_changes_per_hour")]
    pub ach_rate: f64,
    /// Minimum indoor temperature to activate ventilation [°C]
    /// Only ventilate when zone temp >= this value
    #[serde(default)]
    pub min_indoor_temp: Option<f64>,
    /// Only ventilate when outdoor temp < indoor temp (economizer logic)
    #[serde(default)]
    pub outdoor_temp_must_be_lower: Option<bool>,
}

/// Interior solar distribution specification.
///
/// Defines how transmitted solar through windows is distributed to
/// interior surfaces. ASHRAE 140 Case 600 specifies:
///   Floor: 64.2%, Ceiling/Walls share remainder.
///
/// If not specified, all transmitted solar goes to zone air (simplified).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InteriorSolarDistribution {
    /// Fraction of transmitted solar that goes to floor surfaces [0-1]
    #[serde(default = "default_floor_fraction")]
    pub floor_fraction: f64,
    /// Fraction that goes to walls [0-1] (distributed by area)
    #[serde(default = "default_wall_fraction")]
    pub wall_fraction: f64,
    /// Fraction that goes to ceiling/roof [0-1]
    #[serde(default = "default_ceiling_fraction")]
    pub ceiling_fraction: f64,
}

fn default_floor_fraction() -> f64 {
    0.642
}
fn default_wall_fraction() -> f64 {
    0.191
}
fn default_ceiling_fraction() -> f64 {
    0.167
}

impl Default for InteriorSolarDistribution {
    fn default() -> Self {
        Self {
            floor_fraction: default_floor_fraction(),
            wall_fraction: default_wall_fraction(),
            ceiling_fraction: default_ceiling_fraction(),
        }
    }
}

/// Exhaust fan specification for a zone.
///
/// Models air being removed from the zone (e.g., restroom exhaust, kitchen hood).
/// The exhausted air is replaced by infiltration or transfer air from adjacent spaces.
/// Uses the same fan physics as supply/return fans for power calculation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExhaustFanInput {
    /// Free-form tag for output classification (default "exhaust")
    #[serde(default = "default_exhaust_fan_tag")]
    pub tag: String,
    /// Exhaust flow rate [m³/s]
    pub flow_rate: f64,
    /// Design pressure rise [Pa] (0 → no power consumption)
    #[serde(default)]
    pub pressure_rise: f64,
    /// Total fan efficiency (fan × belt × motor × VFD) [0-1]
    #[serde(default = "default_exhaust_fan_total_eff")]
    pub total_efficiency: f64,
    /// Motor efficiency [0-1]
    #[serde(default = "default_exhaust_fan_motor_eff")]
    pub motor_efficiency: f64,
    /// Fraction of motor waste heat entering the airstream [0-1]
    #[serde(default = "default_exhaust_fan_motor_in_air")]
    pub motor_in_airstream_fraction: f64,
    /// Schedule name for time-varying operation (default: always on)
    #[serde(default)]
    pub schedule: Option<String>,
}

fn default_exhaust_fan_tag() -> String {
    "exhaust".to_string()
}
fn default_exhaust_fan_total_eff() -> f64 {
    0.6
}
fn default_exhaust_fan_motor_eff() -> f64 {
    0.9
}
fn default_exhaust_fan_motor_in_air() -> f64 {
    1.0
}

/// ASHRAE 62.1 outdoor air specification for a zone.
///
/// Specifies both supply outdoor air and exhaust air requirements.
///
/// **Supply OA** method (from `oa_method`):
///   - Sum:     total = per_person × people + per_area × floor_area + absolute + ach × V/3600
///   - Maximum: total = max(per_person × people, per_area × floor_area, absolute, ach × V/3600)
///
/// **Exhaust** method (from `exhaust_method`):
///   - Sum:     total = exhaust_per_person × people + exhaust_per_area × floor_area + exhaust_absolute + exhaust_ach × V/3600
///   - Maximum: total = max(...)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutdoorAirInput {
    // ── Supply outdoor air ────────────────────────────────────────
    /// Outdoor air per person [m³/s-person] (e.g., 0.003539606 = 7.5 cfm/person)
    #[serde(default)]
    pub per_person: f64,
    /// Outdoor air per floor area [m³/s-m²] (e.g., 0.000609599 = 0.12 cfm/ft²)
    #[serde(default)]
    pub per_area: f64,
    /// Absolute supply outdoor air flow [m³/s]
    #[serde(default)]
    pub absolute: f64,
    /// Supply outdoor air as air changes per hour [1/hr]
    #[serde(default)]
    pub ach: f64,
    /// Method for combining supply OA rates
    #[serde(default)]
    pub oa_method: crate::zone_loads::OaMethod,

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
    /// Method for combining exhaust rates
    #[serde(default)]
    pub exhaust_method: crate::zone_loads::OaMethod,
}

/// Internal thermal mass definition (furniture, contents, etc.).
///
/// Represents additional thermal mass within a zone that participates in
/// the zone heat balance via convective and radiative exchange. Modeled
/// as an adiabatic surface with CTF conduction (both sides face the same
/// zone), matching EnergyPlus `InternalMass` objects.
///
/// This significantly dampens zone temperature swings — without it, zones
/// respond too quickly to solar gains and outdoor temperature changes,
/// causing HVAC loads 2-7× higher than expected.
///
/// # Example (YAML)
/// ```yaml
/// internal_mass:
///   - construction: InteriorFurnishings
///     area: 88.25      # typically 1× floor area
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InternalMassInput {
    /// Construction name (must reference a defined layered construction)
    pub construction: String,
    /// Exposed surface area [m²] (both sides exchange with the zone)
    pub area: f64,
}

/// Natural ventilation through operable openings (wind + stack driven).
///
/// Models the EnergyPlus `ZoneVentilation:WindandStackOpenArea` object.
/// Total airflow is the root-sum-of-squares of wind-driven and stack-driven
/// components:
///
///   V = sqrt(V_wind² + V_stack²)
///
/// Where:
///   V_wind  = Cw × A × F_schedule × v_wind
///   V_stack = Cd × A × F_schedule × sqrt(2·g·ΔH·|Tz-To|/(Tz+273.15))
///
/// Conditions: ventilation is only active when zone and outdoor temperatures
/// are within the configured bounds and wind speed is below the maximum.
///
/// # Example (YAML)
/// ```yaml
/// natural_ventilation:
///   opening_area: 0.0374
///   effective_angle: 180.0      # south-facing
///   height_difference: 6.0957
///   min_indoor_temp: 18.89
///   max_indoor_temp: 25.56
///   min_outdoor_temp: 15.56
///   max_outdoor_temp: 23.89
///   schedule: NatVentAvailability
///   setpoint_reset:
///     heating_setpoint: 12.78
///     cooling_setpoint: 32.22
///     ramp_timesteps: 4
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NaturalVentilationInput {
    /// Opening area [m²]
    pub opening_area: f64,
    /// Effective angle of opening [degrees from north, clockwise].
    /// Used to determine windward/leeward orientation relative to wind.
    #[serde(default)]
    pub effective_angle: f64,
    /// Height difference for stack effect [m]
    #[serde(default = "default_nat_vent_height_diff")]
    pub height_difference: f64,
    /// Discharge coefficient for stack-driven flow (default: 0.65).
    /// EnergyPlus autocalculate uses 0.65 for vertical openings (tilt > 75°).
    #[serde(default = "default_nat_vent_cd")]
    pub discharge_coefficient: f64,
    /// Minimum indoor temperature to allow ventilation [°C]
    #[serde(default = "default_nat_vent_min_indoor")]
    pub min_indoor_temp: f64,
    /// Maximum indoor temperature to allow ventilation [°C]
    #[serde(default = "default_nat_vent_max_indoor")]
    pub max_indoor_temp: f64,
    /// Minimum outdoor temperature [°C]
    #[serde(default = "default_nat_vent_min_outdoor")]
    pub min_outdoor_temp: f64,
    /// Maximum outdoor temperature [°C]
    #[serde(default = "default_nat_vent_max_outdoor")]
    pub max_outdoor_temp: f64,
    /// Maximum wind speed [m/s] (default: 40.0)
    #[serde(default = "default_nat_vent_max_wind")]
    pub max_wind_speed: f64,
    /// Availability schedule name (if None, always available except design days).
    /// Schedule value > 0 means ventilation is available.
    #[serde(default)]
    pub schedule: Option<String>,
    /// Thermostat setpoint override when natural ventilation is active.
    /// Widens the deadband so HVAC does not fight the outdoor air.
    #[serde(default)]
    pub setpoint_reset: Option<NatVentSetpointReset>,
}

impl NaturalVentilationInput {
    /// Availability fraction [0-1] for natural ventilation (#88).
    ///
    /// Returns `sched_frac` when the temperature windows and wind limit
    /// permit ventilation, 0.0 otherwise. Shared by the zone-level
    /// wind-&-stack model and the AFN opening-area update so both use one
    /// availability decision.
    pub fn availability(
        &self,
        t_zone: f64,
        t_outdoor: f64,
        wind_speed: f64,
        sched_frac: f64,
    ) -> f64 {
        let temp_ok = t_zone >= self.min_indoor_temp
            && t_zone <= self.max_indoor_temp
            && t_outdoor >= self.min_outdoor_temp
            && t_outdoor <= self.max_outdoor_temp
            && wind_speed <= self.max_wind_speed;
        if temp_ok {
            sched_frac.clamp(0.0, 1.0)
        } else {
            0.0
        }
    }
}

/// Thermostat setpoint override during natural ventilation.
///
/// When natural ventilation is active, the HVAC thermostat setpoints are
/// widened to avoid heating/cooling against the open windows. When natural
/// ventilation stops, setpoints ramp linearly back to normal over
/// `ramp_timesteps` timesteps.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NatVentSetpointReset {
    /// Override heating setpoint [°C] (e.g., 12.78 — much lower than normal)
    pub heating_setpoint: f64,
    /// Override cooling setpoint [°C] (e.g., 32.22 — much higher than normal)
    pub cooling_setpoint: f64,
    /// Number of timesteps to ramp back to normal after nat vent stops (default: 4)
    #[serde(default = "default_nat_vent_ramp_steps")]
    pub ramp_timesteps: u32,
}

fn default_nat_vent_height_diff() -> f64 {
    0.0
}
fn default_nat_vent_cd() -> f64 {
    0.65
}
fn default_nat_vent_min_indoor() -> f64 {
    -100.0
}
fn default_nat_vent_max_indoor() -> f64 {
    100.0
}
fn default_nat_vent_min_outdoor() -> f64 {
    -100.0
}
fn default_nat_vent_max_outdoor() -> f64 {
    100.0
}
fn default_nat_vent_max_wind() -> f64 {
    40.0
}
fn default_nat_vent_ramp_steps() -> u32 {
    4
}

/// Zone definition from input.
///
/// Volume and floor area can be:
/// 1. Specified explicitly in YAML (existing behavior)
/// 2. Auto-calculated from surface vertices if set to 0.0 (default)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZoneInput {
    pub name: String,
    /// Zone air volume [m³] (0.0 = auto-calculate from surface vertices)
    #[serde(default)]
    pub volume: f64,
    /// Zone floor area [m²] (0.0 = auto-calculate from floor surface vertices)
    #[serde(default)]
    pub floor_area: f64,
    /// Infiltration specification(s) — single object or list.
    /// Multiple objects are summed (e.g., envelope cracks + door opening).
    #[serde(default, deserialize_with = "deserialize_infiltration_list")]
    pub infiltration: Vec<InfiltrationInput>,
    /// Internal heat gains
    #[serde(default)]
    pub internal_gains: Vec<InternalGainInput>,
    /// Internal thermal mass (furniture, contents, partitions).
    /// Each entry creates an adiabatic surface with CTF thermal storage.
    #[serde(default)]
    pub internal_mass: Vec<InternalMassInput>,
    /// Ideal loads air system (if present, envelope handles HVAC directly)
    #[serde(default)]
    pub ideal_loads: Option<IdealLoadsAirSystem>,
    /// Thermostat schedule for time-of-day setpoint changes
    #[serde(default)]
    pub thermostat_schedule: Vec<ThermostatScheduleEntry>,
    /// Ventilation schedule for time-based mechanical ventilation
    #[serde(default)]
    pub ventilation_schedule: Vec<VentilationScheduleEntry>,
    /// Interior solar distribution to surfaces (if None, all to zone air)
    #[serde(default)]
    pub solar_distribution: Option<InteriorSolarDistribution>,
    /// Exhaust fan (removes air from the zone)
    #[serde(default)]
    pub exhaust_fan: Option<ExhaustFanInput>,
    /// ASHRAE 62.1 outdoor air specification
    #[serde(default)]
    pub outdoor_air: Option<OutdoorAirInput>,
    /// Natural ventilation through operable openings (sliding doors, windows).
    /// Wind + stack driven airflow with temperature-based availability.
    #[serde(default)]
    pub natural_ventilation: Option<NaturalVentilationInput>,
    /// Whether this zone is conditioned (default: true)
    /// Unconditioned zones have no HVAC and temperature floats freely
    #[serde(default = "default_conditioned")]
    pub conditioned: bool,
    /// Zone multiplier (default: 1).
    /// Matches E+ Zone List Multiplier / Zone Multiplier behavior:
    /// - Equipment is sized for (zone_load × multiplier)
    /// - HVAC energy is multiplied by this factor for building reporting
    /// - Zone heat balance is simulated once (not multiplied)
    #[serde(default = "default_zone_multiplier")]
    pub zone_multiplier: u32,
    /// Maximum zone relative humidity [%] — triggers dehumidification when
    /// exceeded (deadband overridden to Cooling so DX coil activates).
    /// No limit if None (default).
    #[serde(default)]
    pub max_relative_humidity: Option<f64>,
    /// Minimum zone relative humidity [%] — triggers humidification when
    /// below this value (activates humidifier component if present on the loop).
    /// No limit if None (default).
    #[serde(default)]
    pub min_relative_humidity: Option<f64>,
    /// Data center zone configuration. When present, enables implicit aisle physics
    /// and IT load generation. Replaces or supplements the `equipment_it` block.
    #[serde(default)]
    pub data_center: Option<DataCenterConfig>,
    /// Duct leakage to an unconditioned space (#82). When the airflow network
    /// is enabled, adds a FixedFlow path carrying leaked duct air from this
    /// zone to the unconditioned zone containing the ducts.
    #[serde(default)]
    pub duct_leakage: Option<DuctLeakageInput>,
    /// Passive species sources in this zone (#84), e.g. CO₂ generation.
    /// Requires species configured on the airflow network.
    #[serde(default)]
    pub species_generation: Vec<crate::species::SpeciesGenerationInput>,
    /// In-zone vertical temperature stratification (#91).
    #[serde(default)]
    pub room_air: Option<RoomAirGradient>,
}

/// In-zone vertical temperature stratification (#91): constant-gradient
/// room air model. E+ RoomAir:TemperaturePattern:ConstantGradient is the
/// minimum reference; IDA ICE offers an equivalent gradient input.
///
/// The zone heat balance still solves the mean air temperature; the gradient
/// redistributes it over height: T(z) = T_mean + gradient·(z − H/2). Interior
/// surfaces couple to the local air temperature at their centroid height
/// (a warm ceiling loses more heat upward), return air leaves at the
/// return-height temperature (so coil loads and economizers see
/// stratification), and the occupied-height temperature is reported.
///
/// ```yaml
/// zones:
///   - name: Atrium
///     room_air:
///       gradient: 1.5          # K/m, positive = warmer near ceiling
///       return_height: 6.0     # m above floor (default: ceiling)
///       thermostat_height: 1.1 # m above floor
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomAirGradient {
    /// Vertical temperature gradient [K/m], positive = warmer near ceiling.
    pub gradient: f64,
    /// Optional schedule scaling the gradient (e.g. 0 when fans force mixing).
    #[serde(default)]
    pub schedule: Option<String>,
    /// Return/exhaust air height above the floor [m] (default: ceiling).
    #[serde(default)]
    pub return_height: Option<f64>,
    /// Occupied/thermostat sensing height above the floor [m] (default 1.1).
    #[serde(default = "default_thermostat_height")]
    pub thermostat_height: f64,
    /// Zone floor-to-ceiling height [m] (default: volume / floor_area).
    #[serde(default)]
    pub ceiling_height: Option<f64>,
}

fn default_thermostat_height() -> f64 {
    1.1
}

/// Duct leakage to the unconditioned space containing the ducts (#82, #85).
///
/// Two directional AFN paths are created: supply leakage spills supply air
/// from the duct run into the ambient zone (zone → ambient), while return
/// leakage is air the below-ambient-pressure return duct ingests from the
/// space and delivers to the zone (ambient → zone). A supply-dominated
/// system pressurizes the unconditioned space and depressurizes this zone;
/// a return-dominated one does the opposite. Leakage energy is accounted for
/// separately by the duct component (#70), so these paths drive pressure
/// (and species transport) only.
///
/// ```yaml
/// zones:
///   - name: Living
///     duct_leakage:
///       ambient_zone: Attic
///       supply_leakage_fraction: 0.06
///       return_leakage_fraction: 0.03
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DuctLeakageInput {
    /// Zone containing the ducts (typically unconditioned: attic, crawlspace).
    pub ambient_zone: String,
    /// Supply duct leakage as a fraction of supply flow [0-1].
    #[serde(default)]
    pub supply_leakage_fraction: f64,
    /// Return duct leakage as a fraction of supply flow [0-1].
    #[serde(default)]
    pub return_leakage_fraction: f64,
}

/// ASHRAE A-class equipment inlet temperature limits [°C].
fn rack_inlet_temp_max(config: &DataCenterConfig) -> f64 {
    config.rack_inlet_temp_max_c.unwrap_or_else(|| {
        match config.equipment_class.as_deref().unwrap_or("A2") {
            "A1" => 32.0,
            "A2" => 35.0,
            "A3" => 40.0,
            "A4" => 45.0,
            _ => 35.0,
        }
    })
}

/// Compute the effective rack inlet temperature limit for a DC zone [°C].
pub fn dc_rack_inlet_max(config: &DataCenterConfig) -> f64 {
    rack_inlet_temp_max(config)
}

/// Data center zone physics configuration.
///
/// When present on a zone, enables implicit hot/cold-aisle physics and
/// auto-generates IT equipment loads routed to the `ItEquipment` ComponentKind.
///
/// ```yaml
/// zones:
///   - name: Server Room 1
///     data_center:
///       it_load_kw: 500.0
///       rack_outlet_temp_c: 35.0
///       equipment_class: A2
///       containment_efficiency: 0.85
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DataCenterConfig {
    /// Total IT load [kW] (use this or rack_count × kw_per_rack)
    #[serde(default)]
    pub it_load_kw: Option<f64>,
    /// Number of racks (used with kw_per_rack)
    #[serde(default)]
    pub rack_count: Option<u32>,
    /// Power per rack [kW] (used with rack_count)
    #[serde(default)]
    pub kw_per_rack: Option<f64>,
    /// Schedule name for IT load fraction [0-1] (default: constant 1.0)
    #[serde(default)]
    pub it_load_schedule: Option<String>,
    /// Hot-aisle rack exhaust temperature target [°C] (default 35.0)
    #[serde(default = "default_rack_outlet_temp")]
    pub rack_outlet_temp_c: f64,
    /// Cold-aisle supply temperature limit [°C]. If None, derived from equipment_class.
    #[serde(default)]
    pub rack_inlet_temp_max_c: Option<f64>,
    /// ASHRAE equipment class: "A1", "A2" (default), "A3", "A4"
    #[serde(default)]
    pub equipment_class: Option<String>,
    /// Containment efficiency [0-1] — fraction of hot exhaust captured and
    /// returned to CRAC/CRAH inlet rather than mixing with zone air. Default 0.85.
    #[serde(default = "default_containment_efficiency")]
    pub containment_efficiency: f64,
    /// Explicit airflow per unit IT load [m³/s/kW]. If None, auto-calculated
    /// from rack temperature difference.
    #[serde(default)]
    pub airflow_m3_per_s_per_kw: Option<f64>,
    /// Lighting power density inside the data center [W/m²] (default 5.0)
    #[serde(default)]
    pub lighting_w_per_m2: Option<f64>,
}

fn default_rack_outlet_temp() -> f64 {
    35.0
}
fn default_containment_efficiency() -> f64 {
    0.85
}

impl DataCenterConfig {
    /// Total IT load [W] before schedule modulation.
    pub fn it_load_w(&self) -> f64 {
        if let Some(kw) = self.it_load_kw {
            return kw * 1000.0;
        }
        if let (Some(racks), Some(kw_per)) = (self.rack_count, self.kw_per_rack) {
            return racks as f64 * kw_per * 1000.0;
        }
        0.0
    }

    /// Rack inlet temperature maximum [°C].
    pub fn rack_inlet_max(&self) -> f64 {
        dc_rack_inlet_max(self)
    }

    /// Server (IT) air mass flow rate [kg/s] carrying `it_power_w` of heat.
    ///
    /// The hot-aisle rack-exhaust temperature is set by the *server fans'*
    /// airflow, which is independent of the CRAC/CRAH supply flow (CRAC-1 /
    /// #62). Two cases, matching E+ `ElectricEquipment:ITE:AirCooled`:
    ///   1. Explicit `airflow_m3_per_s_per_kw` → ṁ = flow·(P_IT/1000)·ρ.
    ///   2. Otherwise size the flow to hit the design rack ΔT
    ///      (`rack_outlet_temp_c` − supply): ṁ = P_IT / (cp·ΔT).
    ///
    /// `rho` and `cp` are the moist-air density and specific heat of the
    /// supply air; `t_supply` is the cold-aisle supply temperature [°C].
    pub fn it_mass_flow(&self, it_power_w: f64, t_supply: f64, rho: f64, cp: f64) -> f64 {
        if it_power_w <= 0.0 {
            return 0.0;
        }
        if let Some(flow_per_kw) = self.airflow_m3_per_s_per_kw {
            if flow_per_kw > 0.0 {
                return flow_per_kw * (it_power_w / 1000.0) * rho;
            }
        }
        // Size to the design rack temperature rise (cold aisle → hot aisle).
        let delta_t = (self.rack_outlet_temp_c - t_supply).max(1.0);
        (it_power_w / (cp * delta_t)).max(1.0e-3)
    }
}

fn default_conditioned() -> bool {
    true
}
fn default_zone_multiplier() -> u32 {
    1
}

impl ZoneInput {
    /// Get the active thermostat setpoints for a given hour of day.
    ///
    /// If a thermostat schedule is defined, returns the setpoints for the
    /// matching period. Otherwise, returns the ideal_loads default setpoints
    /// (or 20/27 if no ideal loads).
    pub fn active_setpoints(&self, hour: u32) -> (f64, f64) {
        // Check thermostat schedule first
        for entry in &self.thermostat_schedule {
            let in_range = if entry.start_hour <= entry.end_hour {
                hour >= entry.start_hour && hour <= entry.end_hour
            } else {
                // Wraps past midnight (e.g., 23 to 7)
                hour >= entry.start_hour || hour <= entry.end_hour
            };
            if in_range {
                return (entry.heating_setpoint, entry.cooling_setpoint);
            }
        }

        // Fall back to ideal_loads setpoints
        if let Some(ref il) = self.ideal_loads {
            (il.heating_setpoint, il.cooling_setpoint)
        } else {
            (20.0, 27.0)
        }
    }

    /// Get scheduled ventilation flow rate for a given hour [m³/s].
    ///
    /// Supports conditional ventilation (ASHRAE 140 Case 650):
    /// - `min_indoor_temp`: Only activate when zone temp >= threshold
    /// - `outdoor_temp_must_be_lower`: Only activate when T_outdoor < T_zone
    pub fn scheduled_ventilation_flow(
        &self,
        hour: u32,
        zone_volume: f64,
        zone_temp: f64,
        outdoor_temp: f64,
    ) -> f64 {
        let mut total_flow = 0.0;
        for entry in &self.ventilation_schedule {
            let in_range = if entry.start_hour <= entry.end_hour {
                hour >= entry.start_hour && hour <= entry.end_hour
            } else {
                hour >= entry.start_hour || hour <= entry.end_hour
            };
            if !in_range {
                continue;
            }

            // Check temperature conditions
            if let Some(min_t) = entry.min_indoor_temp {
                if zone_temp < min_t {
                    continue; // Zone too cool — don't ventilate
                }
            }
            if entry.outdoor_temp_must_be_lower.unwrap_or(false) && outdoor_temp >= zone_temp {
                continue; // Outdoor air not cooler — don't ventilate
            }

            if entry.flow_rate > 0.0 {
                total_flow += entry.flow_rate;
            } else if entry.ach_rate > 0.0 {
                total_flow += entry.ach_rate * zone_volume / 3600.0;
            }
        }
        total_flow
    }
}

/// Runtime zone state for heat balance.
#[derive(Debug, Clone)]
pub struct ZoneState {
    pub input: ZoneInput,
    /// Current zone air temperature [°C]
    pub temp: f64,
    /// Previous timestep zone air temperature [°C] (T_n)
    pub temp_prev: f64,
    /// Two timesteps ago zone air temperature [°C] (T_{n-1})
    pub temp_prev2: f64,
    /// Three timesteps ago zone air temperature [°C] (T_{n-2})
    pub temp_prev3: f64,
    /// Order of backward difference scheme (1, 2, or 3).
    /// Starts at 1 and increments each timestep until reaching 3.
    /// Matches E+'s ZoneTempPredictorCorrector ramp-up strategy.
    pub temp_order: u8,
    /// Current zone humidity ratio [kg/kg]
    pub humidity_ratio: f64,
    /// Previous timestep zone humidity ratio [kg/kg] (W_n)
    pub w_prev: f64,
    /// Two timesteps ago zone humidity ratio [kg/kg] (W_{n-1})
    pub w_prev2: f64,
    /// Three timesteps ago zone humidity ratio [kg/kg] (W_{n-2})
    pub w_prev3: f64,
    /// Order of backward difference scheme for humidity (1, 2, or 3).
    pub w_order: u8,
    /// HVAC supply air humidity ratio [kg/kg]
    pub supply_air_humidity_ratio: f64,
    /// Effective room-air vertical gradient this timestep [K/m] (#91);
    /// 0 when the zone is well mixed.
    pub current_gradient: f64,
    /// Zone gauge pressure from the AFN [Pa] (#89); 0 when the AFN is off.
    pub afn_pressure: f64,
    /// AFN interzone inflow into this zone [kg/s] (#87), aggregated after
    /// each pressure solve. Zero when the AFN is off.
    pub afn_interzone_mass_flow: f64,
    /// Mass-weighted mean temperature of AFN interzone inflow [°C] (#87),
    /// from previous-timestep source zone temps (lagged coupling).
    pub afn_interzone_temp: f64,
    /// Mass-weighted mean humidity ratio of AFN interzone inflow [kg/kg] (#87).
    pub afn_interzone_w: f64,
    /// People latent heat gain [W] (scheduled, from internal gains)
    pub people_latent: f64,
    /// Equipment latent heat gain [W] (from equipment with latent_fraction)
    pub equipment_latent: f64,
    /// Lighting heat to zone [W] (conv + rad, excluding return air fraction)
    pub lighting_gain_to_zone: f64,
    /// Equipment sensible heat to zone [W] (conv + rad, excluding lost fraction)
    pub equipment_sensible_gain_to_zone: f64,
    /// Indices into the surface array for surfaces in this zone
    pub surface_indices: Vec<usize>,
    /// Zone heating load [W] (positive = needs heating)
    pub heating_load: f64,
    /// Zone cooling load [W] (positive = needs cooling)
    pub cooling_load: f64,
    /// Ideal cooling load at setpoint [W] — HVAC energy needed to hold zone at cooling setpoint
    pub ideal_cooling_load: f64,
    /// Ideal heating load at setpoint [W] — HVAC energy needed to hold zone at heating setpoint
    pub ideal_heating_load: f64,
    /// Actual HVAC heating energy rate [W] (after capacity limits)
    pub hvac_heating_rate: f64,
    /// Actual HVAC cooling energy rate [W] (after capacity limits)
    pub hvac_cooling_rate: f64,
    /// Total convective internal gains [W]
    pub q_internal_conv: f64,
    /// Total radiative internal gains [W]
    pub q_internal_rad: f64,
    /// Lighting electric power [W] (scheduled)
    pub lighting_power: f64,
    /// Equipment electric power [W] (scheduled)
    pub equipment_power: f64,
    /// People sensible heat [W] (scheduled)
    pub people_heat: f64,
    /// Infiltration mass flow rate [kg/s]
    pub infiltration_mass_flow: f64,
    /// Scheduled ventilation mass flow rate [kg/s]
    pub ventilation_mass_flow: f64,
    /// HVAC supply air temperature [°C]
    pub supply_air_temp: f64,
    /// HVAC supply air mass flow [kg/s]
    pub supply_air_mass_flow: f64,
    /// Exhaust fan mass flow rate [kg/s]
    pub exhaust_mass_flow: f64,
    /// Exhaust fan electric power [W]
    pub exhaust_fan_power: f64,
    /// Exhaust fan motor waste heat entering the zone [W]
    /// (motor heat NOT in the airstream stays in the zone)
    pub exhaust_fan_heat_to_zone: f64,
    /// ASHRAE 62.1 outdoor air mass flow rate [kg/s]
    pub outdoor_air_mass_flow: f64,
    /// Natural ventilation volume flow rate [m³/s]
    pub nat_vent_flow: f64,
    /// Natural ventilation mass flow rate [kg/s]
    pub nat_vent_mass_flow: f64,
    /// Whether natural ventilation is currently active
    pub nat_vent_active: bool,
    /// Timesteps since natural ventilation stopped (for setpoint ramp-back)
    pub nat_vent_off_timesteps: u32,
    /// Zone centroid height above ground [m].
    /// Used for wind speed correction in infiltration calculation.
    /// Computed as area-weighted average of zone surface centroid heights.
    pub centroid_height: f64,
    /// Predicted zone air temperature WITHOUT HVAC [°C].
    ///
    /// E+-style predictor: what temperature would the zone reach at the end
    /// of this timestep if HVAC were turned off?  Used by the control layer
    /// for mode determination (Heating / Cooling / Deadband).
    ///
    /// T_no_hvac = (SumHAT + MCPI·Tout + Qconv + Cap·T_prev)
    ///           / (SumHA + MCPI + Cap)
    pub temp_no_hvac: f64,

    // ─── Diagnostic accumulators (annual kWh) ─────────────────────
    /// Ideal loads predictor mode locked for current physical timestep.
    /// +1 = heating, -1 = cooling, 0 = deadband.
    /// Set on first HVAC iteration, locked for subsequent iterations.
    pub ideal_pred_mode: i8,
    /// Whether the predictor mode has been computed for this physical timestep.
    /// Reset to false in update_bdf_history().
    pub ideal_pred_mode_locked: bool,

    // ─── Zone air balance component outputs [W] ───────────────────
    // These capture each term in the zone air energy balance at the
    // end of each timestep, for diagnostic comparison with E+.
    //
    //   Q_surfaces + Q_infiltration + Q_internal_conv + Q_solar_to_air
    //     + Q_window_conv + Q_window_absorbed + Q_hvac + Q_thermal_mass = 0
    //
    // Positive = heat flowing INTO the zone air.
    /// Total surface convection to zone air [W]:
    /// Σ h_conv × A × (T_surface_inside − T_zone) for all surfaces.
    pub q_surf_conv_total: f64,
    /// Wall surface convection to zone air [W]
    pub q_surf_conv_walls: f64,
    /// Floor surface convection to zone air [W]
    pub q_surf_conv_floors: f64,
    /// Roof/ceiling surface convection to zone air [W]
    pub q_surf_conv_roofs: f64,
    /// Window convection to zone air [W]
    /// (includes both glass-to-zone convection and absorbed solar inward)
    pub q_surf_conv_windows: f64,
    /// Infiltration sensible heat transfer to zone air [W]:
    /// m_dot_total × cp × (T_outdoor − T_zone)
    pub q_infiltration_sensible: f64,
    /// Thermal mass (storage) term [W]:
    /// ρ × V × cp / dt_eff × (T_prev_eff − T_zone)
    /// Positive when zone is cooling down (releasing stored heat).
    pub q_thermal_mass: f64,

    // ─── Diagnostic accumulators (annual kWh) ─────────────────────
    /// Last sim_time_s when accumulators were committed
    pub diag_last_sim_time: f64,
    /// Pending per-timestep values (overwritten each HVAC iteration, committed on next timestep)
    pub diag_pending_surface: f64,
    pub diag_pending_infil: f64,
    pub diag_pending_q_conv: f64,
    pub diag_pending_solar: f64,
    pub diag_pending_hvac: f64,
    pub diag_pending_internal: f64,
    pub diag_pending_wincond: f64,
    pub diag_pending_wincond_conv: f64,
    /// Cumulative annual values [kWh]
    pub diag_surface_loss_kwh: f64,
    pub diag_infil_loss_kwh: f64,
    pub diag_q_conv_kwh: f64,
    pub diag_solar_trans_kwh: f64,
    pub diag_hvac_net_kwh: f64,
    pub diag_internal_conv_kwh: f64,
    pub diag_window_cond_kwh: f64,
    pub diag_window_conv_kwh: f64,
}

impl ZoneState {
    /// Zone floor-to-ceiling height [m] for the room-air model (#91).
    pub fn room_air_height(&self) -> f64 {
        if let Some(ref ra) = self.input.room_air {
            if let Some(h) = ra.ceiling_height {
                return h.max(0.1);
            }
        }
        if self.input.floor_area > 0.0 {
            (self.input.volume / self.input.floor_area).max(0.1)
        } else {
            3.0
        }
    }

    /// Air temperature at a height above the floor [°C] (#91):
    /// T(z) = T_mean + gradient·(z − H/2).
    pub fn air_temp_at_height(&self, height_above_floor: f64) -> f64 {
        self.temp + self.current_gradient * (height_above_floor - self.room_air_height() / 2.0)
    }

    /// Return/exhaust air temperature [°C] (#91): the air-loop return draws
    /// from the return height (default: ceiling). Equals the mean zone
    /// temperature for well-mixed zones.
    pub fn return_air_temp(&self) -> f64 {
        match &self.input.room_air {
            Some(ra) => {
                self.air_temp_at_height(ra.return_height.unwrap_or_else(|| self.room_air_height()))
            }
            None => self.temp,
        }
    }

    /// Occupied/thermostat-height air temperature [°C] (#91).
    pub fn occupied_air_temp(&self) -> f64 {
        match &self.input.room_air {
            Some(ra) => self.air_temp_at_height(ra.thermostat_height),
            None => self.temp,
        }
    }

    pub fn new(input: ZoneInput, initial_temp: f64) -> Self {
        Self {
            input,
            temp: initial_temp,
            temp_prev: initial_temp,
            temp_prev2: initial_temp,
            temp_prev3: initial_temp,
            temp_order: 1,
            humidity_ratio: 0.008,
            w_prev: 0.008,
            w_prev2: 0.008,
            w_prev3: 0.008,
            w_order: 1,
            supply_air_humidity_ratio: 0.008,
            current_gradient: 0.0,
            afn_pressure: 0.0,
            afn_interzone_mass_flow: 0.0,
            afn_interzone_temp: 20.0,
            afn_interzone_w: 0.008,
            people_latent: 0.0,
            equipment_latent: 0.0,
            lighting_gain_to_zone: 0.0,
            equipment_sensible_gain_to_zone: 0.0,
            surface_indices: Vec::new(),
            heating_load: 0.0,
            cooling_load: 0.0,
            ideal_cooling_load: 0.0,
            ideal_heating_load: 0.0,
            hvac_heating_rate: 0.0,
            hvac_cooling_rate: 0.0,
            q_internal_conv: 0.0,
            q_internal_rad: 0.0,
            lighting_power: 0.0,
            equipment_power: 0.0,
            people_heat: 0.0,
            infiltration_mass_flow: 0.0,
            ventilation_mass_flow: 0.0,
            supply_air_temp: initial_temp,
            supply_air_mass_flow: 0.0,
            exhaust_mass_flow: 0.0,
            exhaust_fan_power: 0.0,
            exhaust_fan_heat_to_zone: 0.0,
            outdoor_air_mass_flow: 0.0,
            nat_vent_flow: 0.0,
            nat_vent_mass_flow: 0.0,
            nat_vent_active: false,
            nat_vent_off_timesteps: u32::MAX, // large value = long since stopped
            centroid_height: 0.0,             // set after surface assignment
            temp_no_hvac: initial_temp,
            ideal_pred_mode: 0,
            ideal_pred_mode_locked: false,
            q_surf_conv_total: 0.0,
            q_surf_conv_walls: 0.0,
            q_surf_conv_floors: 0.0,
            q_surf_conv_roofs: 0.0,
            q_surf_conv_windows: 0.0,
            q_infiltration_sensible: 0.0,
            q_thermal_mass: 0.0,
            diag_last_sim_time: -1.0,
            diag_pending_surface: 0.0,
            diag_pending_infil: 0.0,
            diag_pending_q_conv: 0.0,
            diag_pending_solar: 0.0,
            diag_pending_hvac: 0.0,
            diag_pending_internal: 0.0,
            diag_pending_wincond: 0.0,
            diag_pending_wincond_conv: 0.0,
            diag_surface_loss_kwh: 0.0,
            diag_infil_loss_kwh: 0.0,
            diag_q_conv_kwh: 0.0,
            diag_solar_trans_kwh: 0.0,
            diag_hvac_net_kwh: 0.0,
            diag_internal_conv_kwh: 0.0,
            diag_window_cond_kwh: 0.0,
            diag_window_conv_kwh: 0.0,
        }
    }
}

/// Compute effective `dt` and `t_prev` for the E+-style backward difference.
///
/// EnergyPlus uses a 3rd-order backward difference for the zone air energy
/// balance, ramping from 1st-order on the first timestep to 3rd-order once
/// three timesteps of history are available.  Rather than changing every
/// solve function's signature, we fold the higher-order coefficients into
/// an effective timestep (`dt_eff`) and effective previous temperature
/// (`t_prev_eff`), keeping the same 1st-order formula.
///
/// ```text
///   Order 1: dT/dt ≈ (T_{n+1} - T_n) / dt
///            cap_mult = 1,  t_eff = T_n
///   Order 2: dT/dt ≈ (3/2·T_{n+1} - 2·T_n + 1/2·T_{n-1}) / dt
///            cap_mult = 3/2,  t_eff = (4·T_n - T_{n-1}) / 3
///   Order 3: dT/dt ≈ (11/6·T_{n+1} - 3·T_n + 3/2·T_{n-1} - 1/3·T_{n-2}) / dt
///            cap_mult = 11/6,  t_eff = (18·T_n - 9·T_{n-1} + 2·T_{n-2}) / 11
/// ```
///
/// Using `dt_eff = dt / cap_mult` in the solve functions yields
/// `cap_term = ρ·V·cp / dt_eff = cap_mult × ρ·V·cp / dt`, which is the
/// correct coefficient for the higher-order scheme.
pub fn backward_diff_effective(
    order: u8,
    dt: f64,
    t_prev: f64,
    t_prev2: f64,
    t_prev3: f64,
) -> (f64, f64) {
    match order.min(3) {
        3 => {
            let cap_mult = 11.0 / 6.0;
            let t_eff = (18.0 * t_prev - 9.0 * t_prev2 + 2.0 * t_prev3) / 11.0;
            (dt / cap_mult, t_eff)
        }
        2 => {
            let cap_mult = 1.5;
            let t_eff = (4.0 * t_prev - t_prev2) / 3.0;
            (dt / cap_mult, t_eff)
        }
        _ => {
            // 1st-order backward Euler (original behavior)
            (dt, t_prev)
        }
    }
}

/// Solve zone air temperature for one timestep.
///
/// EnergyPlus predictor-corrector formulation:
///   T = (SumHAT + MCPI·Tout + MCPSYS·Tsup + Qconv + Cap·Tprev)
///     / (SumHA + MCPI + MCPSYS + Cap)
///
/// Where:
///   SumHA = Σ(h_conv × Area) for all zone surfaces [W/K]
///   SumHAT = Σ(h_conv × Area × T_surface) for all zone surfaces [W]
///   MCPI = infiltration mass_flow × Cp [W/K]
///   MCPSYS = HVAC supply mass_flow × Cp [W/K]
///   Cap = ρ × V × Cp / dt [W/K] (zone air thermal capacitance)
///   Qconv = total convective gains [W]
pub fn solve_zone_air_temp(
    sum_ha: f64,
    sum_hat: f64,
    mcpi: f64,
    t_outdoor: f64,
    mcpsys: f64,
    t_supply: f64,
    q_conv: f64,
    rho_air: f64,
    volume: f64,
    cp_air: f64,
    dt: f64,
    t_prev: f64,
) -> f64 {
    let cap_term = rho_air * volume * cp_air / dt;

    let numerator = sum_hat + mcpi * t_outdoor + mcpsys * t_supply + q_conv + cap_term * t_prev;

    let denominator = sum_ha + mcpi + mcpsys + cap_term;

    if denominator.abs() < 1.0e-10 {
        t_prev
    } else {
        numerator / denominator
    }
}

/// Solve zone air temperature with a direct convective Q_hvac added [W].
///
/// Same as solve_zone_air_temp but with Q_hvac added directly to the
/// convective gains. This is used by the ideal loads system where
/// HVAC energy goes directly to zone air (no supply air flow).
///
///   T = (SumHAT + MCPI·Tout + Qconv + Qhvac + Cap·Tprev)
///     / (SumHA + MCPI + Cap)
pub fn solve_zone_air_temp_with_q(
    sum_ha: f64,
    sum_hat: f64,
    mcpi: f64,
    t_outdoor: f64,
    q_conv: f64,
    q_hvac: f64,
    rho_air: f64,
    volume: f64,
    cp_air: f64,
    dt: f64,
    t_prev: f64,
) -> f64 {
    let cap_term = rho_air * volume * cp_air / dt;

    let numerator = sum_hat + mcpi * t_outdoor + q_conv + q_hvac + cap_term * t_prev;

    let denominator = sum_ha + mcpi + cap_term;

    if denominator.abs() < 1.0e-10 {
        t_prev
    } else {
        numerator / denominator
    }
}

/// Solve zone air humidity ratio for one timestep.
///
/// Moisture balance (analogous to the energy balance):
///
///   ρ·V/dt · (W_new - W_prev) = ṁ_infil·(W_outdoor - W_new)
///                               + ṁ_supply·(W_supply - W_new)
///                               + m_latent_gains
///
/// Rearranges to:
///   W_new = (Cap·W_prev + ṁ_infil·W_outdoor + ṁ_supply·W_supply + m_latent)
///         / (Cap + ṁ_infil + ṁ_supply)
///
/// Where:
///   Cap = ρ·V / dt [kg/s] (zone air moisture capacitance)
///   m_latent = Q_latent / h_fg [kg/s] (latent gains converted to moisture rate)
///   h_fg ≈ 2,501,000 J/kg (latent heat of vaporization at ~20°C)
///
/// Reference: EnergyPlus Engineering Reference, "Zone Air Moisture Predictor-Corrector"
pub fn solve_zone_humidity(
    rho_air: f64,
    volume: f64,
    dt: f64,
    w_prev: f64,
    m_infil: f64,
    w_outdoor: f64,
    m_supply: f64,
    w_supply: f64,
    q_latent: f64,
) -> f64 {
    /// Latent heat of vaporization [J/kg] at ~20°C
    const H_FG: f64 = 2_501_000.0;

    let cap = rho_air * volume / dt;

    // Convert latent heat [W] to moisture generation rate [kg/s]
    let m_latent = q_latent / H_FG;

    let numerator = cap * w_prev + m_infil * w_outdoor + m_supply * w_supply + m_latent;

    let denominator = cap + m_infil + m_supply;

    if denominator.abs() < 1.0e-20 {
        w_prev
    } else {
        let w_new = numerator / denominator;
        // Clamp to physically reasonable range [0, 0.10]
        // (0.10 kg/kg ≈ tropical extreme; negative is unphysical)
        w_new.clamp(0.0, 0.10)
    }
}

/// Mix two advective air streams into one effective stream (#87).
///
/// Returns (combined mass flow, mass-weighted property). Used to fold AFN
/// interzone inflows into the outdoor-air stream of the zone air balances:
/// the combined stream (m₁+m₂) at the mixed temperature/humidity is
/// mathematically identical to the two separate advective terms, so every
/// existing solve path picks up interzone advection without new terms.
pub fn mix_advective_streams(m1: f64, x1: f64, m2: f64, x2: f64) -> (f64, f64) {
    // Short-circuit single-stream cases so the no-interzone path is
    // bit-exact with the pre-#87 math ((m·x)/m can round off by an ulp,
    // which matters for ASHRAE 140 cases sitting on acceptance limits).
    if m2 <= 0.0 {
        return (m1.max(0.0), x1);
    }
    if m1 <= 0.0 {
        return (m2, x2);
    }
    let m = m1 + m2;
    (m, (m1 * x1 + m2 * x2) / m)
}

/// Compute the Q_hvac needed to hold the zone at a target temperature.
///
/// Given the zone energy balance terms, returns the convective energy
/// [W] that must be added to the zone air to achieve t_target.
///
///   Q_hvac = (SumHA + MCPI + Cap) · t_target - SumHAT - MCPI·Tout - Qconv - Cap·Tprev
///
/// Positive = heating needed, Negative = cooling needed.
pub fn compute_ideal_q_hvac(
    sum_ha: f64,
    sum_hat: f64,
    mcpi: f64,
    t_outdoor: f64,
    q_conv: f64,
    rho_air: f64,
    volume: f64,
    cp_air: f64,
    dt: f64,
    t_prev: f64,
    t_target: f64,
) -> f64 {
    let cap_term = rho_air * volume * cp_air / dt;
    let denominator = sum_ha + mcpi + cap_term;

    denominator * t_target - sum_hat - mcpi * t_outdoor - q_conv - cap_term * t_prev
}

/// Calculate zone heating and cooling loads from the zone energy balance.
///
/// Load = energy the HVAC must deliver to maintain the current zone temp.
/// Computed as the residual of the non-HVAC energy balance.
pub fn calc_zone_loads(
    t_zone: f64,
    sum_ha: f64,
    sum_hat: f64,
    mcpi: f64,
    t_outdoor: f64,
    q_conv: f64,
    rho_air: f64,
    volume: f64,
    cp_air: f64,
    dt: f64,
    t_prev: f64,
) -> (f64, f64) {
    let cap_term = rho_air * volume * cp_air / dt;

    // Energy balance without HVAC: positive = zone gaining heat
    let q_balance = sum_hat - sum_ha * t_zone
        + mcpi * (t_outdoor - t_zone)
        + q_conv
        + cap_term * (t_prev - t_zone);

    // Negative balance = zone losing heat = needs heating
    let heating_load = (-q_balance).max(0.0);
    let cooling_load = q_balance.max(0.0);

    (heating_load, cooling_load)
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_relative_eq;

    fn dc_config() -> DataCenterConfig {
        DataCenterConfig {
            it_load_kw: Some(100.0),
            rack_count: None,
            kw_per_rack: None,
            it_load_schedule: None,
            rack_outlet_temp_c: 35.0,
            rack_inlet_temp_max_c: None,
            equipment_class: None,
            containment_efficiency: 0.85,
            airflow_m3_per_s_per_kw: None,
            lighting_w_per_m2: None,
        }
    }

    /// Interzone advection stream mixing (#87): mass-weighted mixing is
    /// equivalent to the separate advective terms, and degenerates safely.
    #[test]
    fn test_mix_advective_streams() {
        // 0.1 kg/s at 0°C + 0.1 kg/s at 30°C → 0.2 kg/s at 15°C
        let (m, t) = mix_advective_streams(0.1, 0.0, 0.1, 30.0);
        assert_relative_eq!(m, 0.2, max_relative = 1e-12);
        assert_relative_eq!(t, 15.0, max_relative = 1e-12);

        // Uneven weighting
        let (m, t) = mix_advective_streams(0.3, 10.0, 0.1, 30.0);
        assert_relative_eq!(m, 0.4, max_relative = 1e-12);
        assert_relative_eq!(t, 15.0, max_relative = 1e-12);

        // No interzone flow → outdoor stream unchanged
        let (m, t) = mix_advective_streams(0.25, -5.0, 0.0, 30.0);
        assert_relative_eq!(m, 0.25, max_relative = 1e-12);
        assert_relative_eq!(t, -5.0, max_relative = 1e-12);

        // Both zero → zero flow, first property retained (unused downstream)
        let (m, t) = mix_advective_streams(0.0, -5.0, 0.0, 30.0);
        assert_eq!(m, 0.0);
        assert_eq!(t, -5.0);

        // Equivalence: mcpi·T_mix == m1·cp·T1 + m2·cp·T2 (cp cancels)
        let (m, t) = mix_advective_streams(0.07, 3.0, 0.13, 26.0);
        assert_relative_eq!(m * t, 0.07 * 3.0 + 0.13 * 26.0, max_relative = 1e-12);
    }

    /// Room-air gradient model (#91): local air temperatures follow
    /// T(z) = T_mean + g·(z − H/2); return and occupied heights resolve.
    #[test]
    fn test_room_air_gradient_temps() {
        let input = ZoneInput {
            name: "Atrium".to_string(),
            volume: 300.0,
            floor_area: 50.0, // → ceiling height 6 m, mid-height 3 m
            infiltration: vec![],
            internal_gains: vec![],
            internal_mass: vec![],
            ideal_loads: None,
            thermostat_schedule: vec![],
            ventilation_schedule: vec![],
            solar_distribution: None,
            exhaust_fan: None,
            outdoor_air: None,
            natural_ventilation: None,
            conditioned: true,
            zone_multiplier: 1,
            max_relative_humidity: None,
            min_relative_humidity: None,
            data_center: None,
            duct_leakage: None,
            species_generation: vec![],
            room_air: Some(RoomAirGradient {
                gradient: 1.5,
                schedule: None,
                return_height: Some(5.5),
                thermostat_height: 1.1,
                ceiling_height: None,
            }),
        };
        let mut zone = ZoneState::new(input, 22.0);
        zone.current_gradient = 1.5;

        assert_relative_eq!(zone.room_air_height(), 6.0, max_relative = 1e-12);
        // Mid-height = mean temperature
        assert_relative_eq!(zone.air_temp_at_height(3.0), 22.0, max_relative = 1e-12);
        // Ceiling: +1.5 K/m × 3 m = +4.5 K
        assert_relative_eq!(zone.air_temp_at_height(6.0), 26.5, max_relative = 1e-12);
        // Floor: −4.5 K
        assert_relative_eq!(zone.air_temp_at_height(0.0), 17.5, max_relative = 1e-12);
        // Return at 5.5 m: 22 + 1.5·2.5 = 25.75
        assert_relative_eq!(zone.return_air_temp(), 25.75, max_relative = 1e-12);
        // Occupied at 1.1 m: 22 + 1.5·(1.1 − 3.0) = 19.15
        assert_relative_eq!(zone.occupied_air_temp(), 19.15, max_relative = 1e-12);

        // Well-mixed (gradient forced to 0): everything collapses to T_mean
        zone.current_gradient = 0.0;
        assert_relative_eq!(zone.return_air_temp(), 22.0, max_relative = 1e-12);
        assert_relative_eq!(zone.occupied_air_temp(), 22.0, max_relative = 1e-12);

        // Zones without a room_air block report the mean everywhere
        let mut plain = zone.clone();
        plain.input.room_air = None;
        plain.current_gradient = 0.0;
        assert_eq!(plain.return_air_temp(), 22.0);
        assert_eq!(plain.occupied_air_temp(), 22.0);
    }

    /// NV availability (#88): temperature windows and wind limit gate the
    /// schedule fraction.
    #[test]
    fn test_nat_vent_availability() {
        let nv = NaturalVentilationInput {
            opening_area: 2.0,
            effective_angle: 0.0,
            height_difference: 1.0,
            discharge_coefficient: 0.65,
            min_indoor_temp: 22.0,
            max_indoor_temp: 100.0,
            min_outdoor_temp: 10.0,
            max_outdoor_temp: 30.0,
            max_wind_speed: 15.0,
            schedule: None,
            setpoint_reset: None,
        };
        // All conditions met → schedule fraction passes through
        assert_eq!(nv.availability(25.0, 20.0, 3.0, 1.0), 1.0);
        assert_eq!(nv.availability(25.0, 20.0, 3.0, 0.5), 0.5);
        // Indoor too cold
        assert_eq!(nv.availability(20.0, 20.0, 3.0, 1.0), 0.0);
        // Outdoor too hot
        assert_eq!(nv.availability(25.0, 35.0, 3.0, 1.0), 0.0);
        // Too windy
        assert_eq!(nv.availability(25.0, 20.0, 20.0, 1.0), 0.0);
        // Schedule fraction clamped to [0,1]
        assert_eq!(nv.availability(25.0, 20.0, 3.0, 1.8), 1.0);
    }

    #[test]
    fn test_it_mass_flow_sizes_to_rack_delta_t() {
        // No explicit airflow → flow sized to hit the design rack ΔT
        // (rack_outlet_temp_c − supply). At 18 °C supply, ΔT = 17 °C, so the
        // hot-aisle rise lands exactly on rack_outlet_temp_c. (CRAC-1 / #62)
        let dc = dc_config();
        let cp = 1006.0;
        let it_w = 100_000.0; // 100 kW
        let t_supply = 18.0;
        let m_it = dc.it_mass_flow(it_w, t_supply, 1.2, cp);
        let t_hot = t_supply + it_w / (m_it * cp);
        assert_relative_eq!(t_hot, 35.0, max_relative = 0.001);
        // This flow is independent of any CRAC supply flow.
        assert!(m_it > 0.0);
    }

    #[test]
    fn test_it_mass_flow_explicit_airflow() {
        // Explicit airflow per kW overrides the ΔT sizing.
        let mut dc = dc_config();
        dc.airflow_m3_per_s_per_kw = Some(0.05); // 0.05 m³/s per kW
        let rho = 1.2;
        let m_it = dc.it_mass_flow(100_000.0, 18.0, rho, 1006.0);
        // 0.05 · 100 kW · 1.2 kg/m³ = 6.0 kg/s
        assert_relative_eq!(m_it, 0.05 * 100.0 * rho, max_relative = 1e-9);
    }

    #[test]
    fn test_steady_state_zone_temp() {
        // Scenario: no HVAC, no infiltration, no internal gains.
        // Single surface: h=5 W/(m²·K), A=20 m², T_surface = 30°C
        // Zone should converge toward surface temp.
        let sum_ha = 5.0 * 20.0; // 100 W/K
        let sum_hat = 5.0 * 20.0 * 30.0; // 3000 W
        let mcpi = 0.0;
        let mcpsys = 0.0;
        let q_conv = 0.0;
        let rho = 1.2;
        let vol = 100.0;
        let cp = 1005.0;
        let dt = 3600.0;
        let t_prev = 20.0;

        let t = solve_zone_air_temp(
            sum_ha, sum_hat, mcpi, 0.0, mcpsys, 0.0, q_conv, rho, vol, cp, dt, t_prev,
        );

        // With thermal mass, zone temp should be between 20 and 30
        assert!(t > 20.0);
        assert!(t < 30.0);

        // After many iterations (steady state), should approach surface temp
        let mut temp = t_prev;
        for _ in 0..1000 {
            temp = solve_zone_air_temp(
                sum_ha, sum_hat, mcpi, 0.0, mcpsys, 0.0, q_conv, rho, vol, cp, dt, temp,
            );
        }
        assert_relative_eq!(temp, 30.0, max_relative = 0.01);
    }

    #[test]
    fn test_hvac_maintains_setpoint() {
        // Outdoor is cold, but HVAC supplies warm air
        let sum_ha = 5.0 * 20.0; // 100 W/K surface coupling
        let sum_hat = 5.0 * 20.0 * 10.0; // surfaces at 10°C
        let mcpi = 0.01 * 1005.0; // small infiltration
        let t_outdoor = 0.0;
        let mcpsys = 0.5 * 1005.0; // HVAC supply: 0.5 kg/s
        let t_supply = 35.0; // supply at 35°C
        let q_conv = 500.0; // internal gains
        let rho = 1.2;
        let vol = 100.0;
        let cp = 1005.0;
        let dt = 3600.0;

        let mut temp = 21.0;
        for _ in 0..100 {
            temp = solve_zone_air_temp(
                sum_ha, sum_hat, mcpi, t_outdoor, mcpsys, t_supply, q_conv, rho, vol, cp, dt, temp,
            );
        }

        // With HVAC at 35°C, zone should stay warm despite cold surfaces
        assert!(temp > 15.0);
    }

    #[test]
    fn test_zone_loads() {
        let (hl, cl) = calc_zone_loads(
            21.0,         // zone temp
            100.0,        // sum_ha
            100.0 * 15.0, // sum_hat (surfaces at 15°C → zone loses heat)
            0.0,
            0.0,
            0.0,
            1.2,
            100.0,
            1005.0,
            3600.0,
            21.0,
        );
        // Zone is at 21°C, surfaces at 15°C → zone loses heat → heating needed
        assert!(hl > 0.0);
        assert_relative_eq!(cl, 0.0);
    }

    #[test]
    fn test_ideal_q_hvac_heating() {
        // Zone losing heat, need heating to reach 20°C
        let sum_ha = 100.0;
        let sum_hat = 100.0 * 15.0; // surfaces at 15°C
        let mcpi = 0.0;
        let t_outdoor = 0.0;
        let q_conv = 0.0;
        let rho = 1.2;
        let vol = 100.0;
        let cp = 1005.0;
        let dt = 3600.0;
        let t_prev = 20.0;

        let q = compute_ideal_q_hvac(
            sum_ha, sum_hat, mcpi, t_outdoor, q_conv, rho, vol, cp, dt, t_prev, 20.0,
        );
        // Heating needed → positive Q
        assert!(q > 0.0, "Expected positive heating Q, got {}", q);

        // Verify: solving with this Q should give T = 20.0
        let t = solve_zone_air_temp_with_q(
            sum_ha, sum_hat, mcpi, t_outdoor, q_conv, q, rho, vol, cp, dt, t_prev,
        );
        assert_relative_eq!(t, 20.0, max_relative = 0.001);
    }

    #[test]
    fn test_ideal_q_hvac_cooling() {
        // Zone gaining heat, need cooling to reach 27°C
        let sum_ha = 100.0;
        let sum_hat = 100.0 * 35.0; // surfaces at 35°C (hot)
        let mcpi = 0.0;
        let t_outdoor = 35.0;
        let q_conv = 2000.0; // large internal gains
        let rho = 1.2;
        let vol = 100.0;
        let cp = 1005.0;
        let dt = 3600.0;
        let t_prev = 27.0;

        let q = compute_ideal_q_hvac(
            sum_ha, sum_hat, mcpi, t_outdoor, q_conv, rho, vol, cp, dt, t_prev, 27.0,
        );
        // Cooling needed → negative Q
        assert!(q < 0.0, "Expected negative cooling Q, got {}", q);

        // Verify: solving with this Q should give T = 27.0
        let t = solve_zone_air_temp_with_q(
            sum_ha, sum_hat, mcpi, t_outdoor, q_conv, q, rho, vol, cp, dt, t_prev,
        );
        assert_relative_eq!(t, 27.0, max_relative = 0.001);
    }

    #[test]
    fn test_thermostat_schedule() {
        let input = ZoneInput {
            name: "Test".to_string(),
            volume: 100.0,
            floor_area: 50.0,
            infiltration: vec![InfiltrationInput::default()],
            internal_gains: vec![],
            internal_mass: vec![],

            ideal_loads: Some(IdealLoadsAirSystem {
                heating_setpoint: 20.0,
                cooling_setpoint: 27.0,
                ..Default::default()
            }),
            thermostat_schedule: vec![ThermostatScheduleEntry {
                start_hour: 23,
                end_hour: 7,
                heating_setpoint: 10.0,
                cooling_setpoint: 99.0,
            }],
            ventilation_schedule: vec![],
            solar_distribution: None,
            exhaust_fan: None,
            outdoor_air: None,
            natural_ventilation: None,
            conditioned: true,
            zone_multiplier: 1,
            max_relative_humidity: None,
            min_relative_humidity: None,
            data_center: None,
            duct_leakage: None,
            species_generation: vec![],
            room_air: None,
        };

        // During night setback
        let (h, c) = input.active_setpoints(3);
        assert_relative_eq!(h, 10.0);
        assert_relative_eq!(c, 99.0);

        // During day (falls through to ideal_loads defaults)
        let (h, c) = input.active_setpoints(12);
        assert_relative_eq!(h, 20.0);
        assert_relative_eq!(c, 27.0);
    }

    #[test]
    fn test_ventilation_schedule() {
        let input = ZoneInput {
            name: "Test".to_string(),
            volume: 130.0,
            floor_area: 48.0,
            infiltration: vec![InfiltrationInput::default()],
            internal_gains: vec![],
            internal_mass: vec![],

            ideal_loads: None,
            thermostat_schedule: vec![],
            ventilation_schedule: vec![VentilationScheduleEntry {
                start_hour: 18,
                end_hour: 7,
                flow_rate: 0.0,
                ach_rate: 13.12,
                min_indoor_temp: None,
                outdoor_temp_must_be_lower: None,
            }],
            solar_distribution: None,
            exhaust_fan: None,
            outdoor_air: None,
            natural_ventilation: None,
            conditioned: true,
            zone_multiplier: 1,
            max_relative_humidity: None,
            min_relative_humidity: None,
            data_center: None,
            duct_leakage: None,
            species_generation: vec![],
            room_air: None,
        };

        // During night ventilation period (unconditional — no temp conditions)
        let flow = input.scheduled_ventilation_flow(22, 130.0, 30.0, 15.0);
        let expected = 13.12 * 130.0 / 3600.0;
        assert_relative_eq!(flow, expected, max_relative = 0.01);

        // During day (no ventilation)
        let flow = input.scheduled_ventilation_flow(12, 130.0, 30.0, 15.0);
        assert_relative_eq!(flow, 0.0);

        // With temperature conditions: min_indoor_temp
        let input2 = ZoneInput {
            name: "Test2".to_string(),
            volume: 130.0,
            floor_area: 48.0,
            infiltration: vec![InfiltrationInput::default()],
            internal_gains: vec![],
            internal_mass: vec![],

            ideal_loads: None,
            thermostat_schedule: vec![],
            ventilation_schedule: vec![VentilationScheduleEntry {
                start_hour: 18,
                end_hour: 7,
                flow_rate: 0.0,
                ach_rate: 13.12,
                min_indoor_temp: Some(27.0),
                outdoor_temp_must_be_lower: Some(true),
            }],
            solar_distribution: None,
            exhaust_fan: None,
            outdoor_air: None,
            natural_ventilation: None,
            conditioned: true,
            zone_multiplier: 1,
            max_relative_humidity: None,
            min_relative_humidity: None,
            data_center: None,
            duct_leakage: None,
            species_generation: vec![],
            room_air: None,
        };

        // Zone hot enough, outdoor cooler → ventilate
        let flow = input2.scheduled_ventilation_flow(22, 130.0, 30.0, 15.0);
        assert_relative_eq!(flow, expected, max_relative = 0.01);

        // Zone too cool → no ventilation
        let flow = input2.scheduled_ventilation_flow(22, 130.0, 20.0, 15.0);
        assert_relative_eq!(flow, 0.0);

        // Outdoor warmer than zone → no ventilation
        let flow = input2.scheduled_ventilation_flow(22, 130.0, 30.0, 32.0);
        assert_relative_eq!(flow, 0.0);
    }

    #[test]
    fn test_solve_zone_humidity_steady_state() {
        // Outdoor air at W=0.010, zone initially at W=0.008.
        // With infiltration only, zone should converge toward outdoor W.
        let rho = 1.2;
        let vol = 100.0;
        let dt = 3600.0;
        let m_infil = 0.05; // kg/s
        let w_outdoor = 0.010;
        let m_supply = 0.0;
        let w_supply = 0.008;
        let q_latent = 0.0;

        let mut w = 0.008;
        for _ in 0..1000 {
            w = solve_zone_humidity(
                rho, vol, dt, w, m_infil, w_outdoor, m_supply, w_supply, q_latent,
            );
        }
        // Should converge to outdoor humidity
        assert_relative_eq!(w, w_outdoor, max_relative = 0.01);
    }

    #[test]
    fn test_solve_zone_humidity_with_supply() {
        // HVAC supply at W=0.006 (dehumidified), outdoor at W=0.012.
        // Zone should settle between, weighted by mass flows.
        let rho = 1.2;
        let vol = 100.0;
        let dt = 3600.0;
        let m_infil = 0.02;
        let w_outdoor = 0.012;
        let m_supply = 0.5; // large supply flow dominates
        let w_supply = 0.006;
        let q_latent = 0.0;

        let mut w = 0.010;
        for _ in 0..500 {
            w = solve_zone_humidity(
                rho, vol, dt, w, m_infil, w_outdoor, m_supply, w_supply, q_latent,
            );
        }
        // Supply dominates, so zone W should be close to supply W
        assert!(w > 0.006);
        assert!(w < 0.008);
    }

    #[test]
    fn test_solve_zone_humidity_with_latent_gains() {
        // People adding latent heat → moisture increases above outdoor level
        let rho = 1.2;
        let vol = 100.0;
        let dt = 3600.0;
        let m_infil = 0.05;
        let w_outdoor = 0.008;
        let m_supply = 0.0;
        let w_supply = 0.0;
        let q_latent = 500.0; // 500 W of latent heat from people

        let mut w = 0.008;
        for _ in 0..500 {
            w = solve_zone_humidity(
                rho, vol, dt, w, m_infil, w_outdoor, m_supply, w_supply, q_latent,
            );
        }
        // Latent gains should push zone W above outdoor
        assert!(w > w_outdoor);
    }
}
