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

use serde::{Deserialize, Deserializer, Serialize};
use crate::infiltration::InfiltrationInput;
use crate::internal_gains::InternalGainInput;

/// Custom deserializer that accepts either a single InfiltrationInput or a list.
/// This provides backward compatibility: `infiltration: {single}` still works,
/// while `infiltration: [{obj1}, {obj2}]` supports multiple infiltration objects
/// per zone (e.g., envelope cracks + door opening for vestibule zones).
fn deserialize_infiltration_list<'de, D>(deserializer: D) -> Result<Vec<InfiltrationInput>, D::Error>
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

fn default_ideal_capacity() -> f64 { 1_000_000.0 }
fn default_heating_sp() -> f64 { 20.0 }
fn default_cooling_sp() -> f64 { 27.0 }

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

fn default_floor_fraction() -> f64 { 0.642 }
fn default_wall_fraction() -> f64 { 0.191 }
fn default_ceiling_fraction() -> f64 { 0.167 }

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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExhaustFanInput {
    /// Exhaust flow rate [m³/s]
    pub flow_rate: f64,
    /// Schedule name for time-varying operation (default: always on)
    #[serde(default)]
    pub schedule: Option<String>,
}

/// ASHRAE 62.1 outdoor air specification for a zone.
///
/// Calculates minimum outdoor air based on occupancy and floor area.
///
/// Method (from `oa_method`):
///   - Sum:     total = per_person × people + per_area × floor_area  (ASHRAE 62.1)
///   - Maximum: total = max(per_person × people, per_area × floor_area)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutdoorAirInput {
    /// Outdoor air per person [m³/s-person] (e.g., 0.003539606 = 7.5 cfm/person)
    #[serde(default)]
    pub per_person: f64,
    /// Outdoor air per floor area [m³/s-m²] (e.g., 0.000609599 = 0.12 cfm/ft²)
    #[serde(default)]
    pub per_area: f64,
    /// Method for combining per-person and per-area rates
    #[serde(default)]
    pub oa_method: crate::zone_loads::OaMethod,
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

fn default_nat_vent_height_diff() -> f64 { 0.0 }
fn default_nat_vent_cd() -> f64 { 0.65 }
fn default_nat_vent_min_indoor() -> f64 { -100.0 }
fn default_nat_vent_max_indoor() -> f64 { 100.0 }
fn default_nat_vent_min_outdoor() -> f64 { -100.0 }
fn default_nat_vent_max_outdoor() -> f64 { 100.0 }
fn default_nat_vent_max_wind() -> f64 { 40.0 }
fn default_nat_vent_ramp_steps() -> u32 { 4 }

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
    /// Zone multiplier (for identical zones)
    #[serde(default = "default_multiplier")]
    pub multiplier: u32,
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
}

fn default_multiplier() -> u32 { 1 }
fn default_conditioned() -> bool { true }

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
            if entry.outdoor_temp_must_be_lower.unwrap_or(false) {
                if outdoor_temp >= zone_temp {
                    continue; // Outdoor air not cooler — don't ventilate
                }
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
    /// Previous timestep zone air temperature [°C]
    pub temp_prev: f64,
    /// Current zone humidity ratio [kg/kg]
    pub humidity_ratio: f64,
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
}

impl ZoneState {
    pub fn new(input: ZoneInput, initial_temp: f64) -> Self {
        Self {
            input,
            temp: initial_temp,
            temp_prev: initial_temp,
            humidity_ratio: 0.008,
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
            outdoor_air_mass_flow: 0.0,
            nat_vent_flow: 0.0,
            nat_vent_mass_flow: 0.0,
            nat_vent_active: false,
            nat_vent_off_timesteps: u32::MAX, // large value = long since stopped
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

    let numerator = sum_hat
        + mcpi * t_outdoor
        + mcpsys * t_supply
        + q_conv
        + cap_term * t_prev;

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

    let numerator = sum_hat
        + mcpi * t_outdoor
        + q_conv + q_hvac
        + cap_term * t_prev;

    let denominator = sum_ha + mcpi + cap_term;

    if denominator.abs() < 1.0e-10 {
        t_prev
    } else {
        numerator / denominator
    }
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

    denominator * t_target
        - sum_hat
        - mcpi * t_outdoor
        - q_conv
        - cap_term * t_prev
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

    #[test]
    fn test_steady_state_zone_temp() {
        // Scenario: no HVAC, no infiltration, no internal gains.
        // Single surface: h=5 W/(m²·K), A=20 m², T_surface = 30°C
        // Zone should converge toward surface temp.
        let sum_ha = 5.0 * 20.0;  // 100 W/K
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
            sum_ha, sum_hat, mcpi, 0.0, mcpsys, 0.0,
            q_conv, rho, vol, cp, dt, t_prev,
        );

        // With thermal mass, zone temp should be between 20 and 30
        assert!(t > 20.0);
        assert!(t < 30.0);

        // After many iterations (steady state), should approach surface temp
        let mut temp = t_prev;
        for _ in 0..1000 {
            temp = solve_zone_air_temp(
                sum_ha, sum_hat, mcpi, 0.0, mcpsys, 0.0,
                q_conv, rho, vol, cp, dt, temp,
            );
        }
        assert_relative_eq!(temp, 30.0, max_relative = 0.01);
    }

    #[test]
    fn test_hvac_maintains_setpoint() {
        // Outdoor is cold, but HVAC supplies warm air
        let sum_ha = 5.0 * 20.0;   // 100 W/K surface coupling
        let sum_hat = 5.0 * 20.0 * 10.0; // surfaces at 10°C
        let mcpi = 0.01 * 1005.0;  // small infiltration
        let t_outdoor = 0.0;
        let mcpsys = 0.5 * 1005.0; // HVAC supply: 0.5 kg/s
        let t_supply = 35.0;       // supply at 35°C
        let q_conv = 500.0;        // internal gains
        let rho = 1.2;
        let vol = 100.0;
        let cp = 1005.0;
        let dt = 3600.0;

        let mut temp = 21.0;
        for _ in 0..100 {
            temp = solve_zone_air_temp(
                sum_ha, sum_hat, mcpi, t_outdoor, mcpsys, t_supply,
                q_conv, rho, vol, cp, dt, temp,
            );
        }

        // With HVAC at 35°C, zone should stay warm despite cold surfaces
        assert!(temp > 15.0);
    }

    #[test]
    fn test_zone_loads() {
        let (hl, cl) = calc_zone_loads(
            21.0,       // zone temp
            100.0,      // sum_ha
            100.0 * 15.0, // sum_hat (surfaces at 15°C → zone loses heat)
            0.0, 0.0, 0.0,
            1.2, 100.0, 1005.0, 3600.0, 21.0,
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
            sum_ha, sum_hat, mcpi, t_outdoor, q_conv,
            rho, vol, cp, dt, t_prev, 20.0,
        );
        // Heating needed → positive Q
        assert!(q > 0.0, "Expected positive heating Q, got {}", q);

        // Verify: solving with this Q should give T = 20.0
        let t = solve_zone_air_temp_with_q(
            sum_ha, sum_hat, mcpi, t_outdoor, q_conv, q,
            rho, vol, cp, dt, t_prev,
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
            sum_ha, sum_hat, mcpi, t_outdoor, q_conv,
            rho, vol, cp, dt, t_prev, 27.0,
        );
        // Cooling needed → negative Q
        assert!(q < 0.0, "Expected negative cooling Q, got {}", q);

        // Verify: solving with this Q should give T = 27.0
        let t = solve_zone_air_temp_with_q(
            sum_ha, sum_hat, mcpi, t_outdoor, q_conv, q,
            rho, vol, cp, dt, t_prev,
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
            multiplier: 1,
            ideal_loads: Some(IdealLoadsAirSystem {
                heating_setpoint: 20.0,
                cooling_setpoint: 27.0,
                ..Default::default()
            }),
            thermostat_schedule: vec![
                ThermostatScheduleEntry {
                    start_hour: 23,
                    end_hour: 7,
                    heating_setpoint: 10.0,
                    cooling_setpoint: 99.0,
                },
            ],
            ventilation_schedule: vec![],
            solar_distribution: None,
            exhaust_fan: None,
            outdoor_air: None,
            natural_ventilation: None,
            conditioned: true,
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
            multiplier: 1,
            ideal_loads: None,
            thermostat_schedule: vec![],
            ventilation_schedule: vec![
                VentilationScheduleEntry {
                    start_hour: 18,
                    end_hour: 7,
                    flow_rate: 0.0,
                    ach_rate: 13.12,
                    min_indoor_temp: None,
                    outdoor_temp_must_be_lower: None,
                },
            ],
            solar_distribution: None,
            exhaust_fan: None,
            outdoor_air: None,
            natural_ventilation: None,
            conditioned: true,
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
            multiplier: 1,
            ideal_loads: None,
            thermostat_schedule: vec![],
            ventilation_schedule: vec![
                VentilationScheduleEntry {
                    start_hour: 18,
                    end_hour: 7,
                    flow_rate: 0.0,
                    ach_rate: 13.12,
                    min_indoor_temp: Some(27.0),
                    outdoor_temp_must_be_lower: Some(true),
                },
            ],
            solar_distribution: None,
            exhaust_fan: None,
            outdoor_air: None,
            natural_ventilation: None,
            conditioned: true,
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
}
