//! YAML input parser for OpenBSE models.
//!
//! The user writes a YAML file describing the building and HVAC system.
//! The engine parses it and builds the simulation graph automatically.
//! No nodes, branches, branch lists, or connector lists — just components
//! and what connects to what.

use openbse_components::boiler::Boiler;
use openbse_components::chiller::AirCooledChiller;
use openbse_components::cooling_coil::CoolingCoilDX;
use openbse_components::fan::{Fan, FanType};
use openbse_components::heat_recovery::HeatRecovery;
use openbse_components::heating_coil::HeatingCoil;
use openbse_controls::thermostat::{ZoneGroup, ZoneThermostat};
use openbse_controls::setpoint::{SetpointController, PlantLoopSetpoint};
use openbse_controls::Controller;
use openbse_core::graph::SimulationGraph;
use openbse_core::simulation::SimulationConfig;
use openbse_core::types::AutosizeValue;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Top-level model definition.
#[derive(Debug, Serialize, Deserialize)]
pub struct ModelInput {
    /// Simulation settings
    pub simulation: SimulationSettings,
    /// Weather file paths (supports multiple for multi-year runs)
    pub weather_files: Vec<String>,
    /// Design days for autosizing
    #[serde(default)]
    pub design_days: Vec<DesignDayInput>,
    /// Air loop definitions
    #[serde(default)]
    pub air_loops: Vec<AirLoopInput>,
    /// Plant loop definitions
    #[serde(default)]
    pub plant_loops: Vec<PlantLoopInput>,
    /// Zone groups — named lists of zones for referencing in thermostats and loads
    #[serde(default)]
    pub zone_groups: Vec<ZoneGroupInput>,
    /// Thermostat definitions — setpoints assignable to zones or zone groups
    #[serde(default)]
    pub thermostats: Vec<openbse_envelope::ThermostatInput>,
    /// Controls definitions
    #[serde(default)]
    pub controls: Vec<ControlInput>,
    /// Parametric run definitions
    #[serde(default)]
    pub parametrics: Option<ParametricInput>,
    /// Reusable performance curves for equipment
    #[serde(default)]
    pub performance_curves: Vec<openbse_components::performance_curve::PerformanceCurve>,

    // ─── Schedules ───────────────────────────────────────────────────────────

    /// Named schedule definitions for time-varying inputs
    #[serde(default)]
    pub schedules: Vec<openbse_envelope::ScheduleInput>,

    // ─── Envelope (Phase 2) ──────────────────────────────────────────────────

    /// Material definitions
    #[serde(default)]
    pub materials: Vec<openbse_envelope::Material>,
    /// Opaque construction definitions (layers: outside to inside)
    #[serde(default)]
    pub constructions: Vec<openbse_envelope::Construction>,
    /// Window construction definitions (U-factor + SHGC based)
    #[serde(default)]
    pub window_constructions: Vec<openbse_envelope::WindowConstruction>,
    /// Simple construction definitions (U-factor + thermal capacity based)
    #[serde(default)]
    pub simple_constructions: Vec<openbse_envelope::SimpleConstruction>,
    /// Zone definitions (envelope thermal zones)
    #[serde(default)]
    pub zones: Vec<openbse_envelope::ZoneInput>,
    /// Surface definitions (walls, floors, roofs, windows)
    #[serde(default)]
    pub surfaces: Vec<openbse_envelope::SurfaceInput>,
    /// External shading surface definitions (overhangs, fins, neighboring buildings, etc.)
    /// These surfaces only cast shadows — they have no thermal mass.
    #[serde(default)]
    pub shading_surfaces: Vec<openbse_envelope::ShadingSurfaceInput>,

    // ─── Top-Level Zone Loads ────────────────────────────────────────────────
    // These objects define loads assignable to one or more zones by name.
    // They replace the old approach of embedding loads inside each zone.

    /// People definitions (assignable to zones)
    #[serde(default)]
    pub people: Vec<openbse_envelope::PeopleInput>,
    /// Lights definitions (assignable to zones)
    #[serde(default)]
    pub lights: Vec<openbse_envelope::LightsInput>,
    /// Equipment gain definitions (assignable to zones)
    #[serde(default)]
    pub equipment: Vec<openbse_envelope::EquipmentGainInput>,
    /// Infiltration definitions (assignable to zones)
    #[serde(default)]
    pub infiltration: Vec<openbse_envelope::InfiltrationTopLevel>,
    /// Ventilation definitions (assignable to zones)
    #[serde(default)]
    pub ventilation: Vec<openbse_envelope::VentilationTopLevel>,
    /// Exhaust fan definitions (assignable to zones)
    #[serde(default)]
    pub exhaust_fans: Vec<openbse_envelope::ExhaustFanTopLevel>,
    /// Outdoor air definitions (assignable to zones)
    #[serde(default)]
    pub outdoor_air: Vec<openbse_envelope::OutdoorAirTopLevel>,
    /// Ideal loads definitions (assignable to zones)
    #[serde(default)]
    pub ideal_loads: Vec<openbse_envelope::IdealLoadsTopLevel>,

    // ─── Outputs ────────────────────────────────────────────────────────────

    /// Custom output file definitions
    #[serde(default)]
    pub outputs: Vec<crate::output::OutputFileConfig>,
    /// Whether to generate the standard summary report (default: true)
    #[serde(default = "default_summary_report")]
    pub summary_report: bool,
}

fn default_summary_report() -> bool { true }

#[derive(Debug, Serialize, Deserialize)]
pub struct SimulationSettings {
    #[serde(default = "default_timesteps_per_hour")]
    pub timesteps_per_hour: u32,
    #[serde(default = "default_start_month")]
    pub start_month: u32,
    #[serde(default = "default_start_day")]
    pub start_day: u32,
    #[serde(default = "default_end_month")]
    pub end_month: u32,
    #[serde(default = "default_end_day")]
    pub end_day: u32,
    /// Solar shading calculation method.
    ///
    /// - `basic` (default): No geometric shadow calculations. All surfaces
    ///   receive full unobstructed solar radiation.
    /// - `detailed`: Full Sutherland-Hodgman polygon clipping. Computes sunlit
    ///   fractions per surface per timestep using explicit shading surfaces,
    ///   window overhangs/fins, and building self-shading.
    #[serde(default)]
    pub shading_calculation: openbse_envelope::ShadingCalculation,
    /// Site terrain classification for wind profile calculations.
    ///
    /// Determines how wind speed varies with height above ground, affecting
    /// exterior convection coefficients.
    ///
    /// - `suburbs` (default): Residential areas, light suburban development.
    /// - `country`: Open terrain, flat unobstructed areas (ASHRAE 140).
    /// - `city`: Urban areas with tall buildings.
    /// - `ocean`: Unobstructed ocean or large lake exposure.
    #[serde(default)]
    pub terrain: openbse_envelope::convection::Terrain,
}

fn default_timesteps_per_hour() -> u32 { 1 }
fn default_start_month() -> u32 { 1 }
fn default_start_day() -> u32 { 1 }
fn default_end_month() -> u32 { 12 }
fn default_end_day() -> u32 { 31 }

impl SimulationSettings {
    pub fn to_config(&self) -> SimulationConfig {
        SimulationConfig {
            timesteps_per_hour: self.timesteps_per_hour,
            start_month: self.start_month,
            start_day: self.start_day,
            end_month: self.end_month,
            end_day: self.end_day,
            ..SimulationConfig::default()
        }
    }
}

/// Air loop system type — controls how the loop is simulated.
///
/// - `psz_ac`  (default): Packaged single-zone AC. One thermostat in the served zone
///             drives heating/cooling mode. Return air is mixed with outdoor air.
///             Suitable for residential unitary, rooftop units, etc.
/// - `doas`:   Dedicated outdoor air system. Handles 100% outdoor air only,
///             pre-conditions ventilation to a fixed supply temperature.
///             Does NOT modulate based on zone temperature — always runs.
/// - `fcu`:    Fan coil unit (recirculating). Per-zone thermostat. Recirculates
///             zone air (no OA mixing). Independent of DOAS ventilation.
/// - `vav`:    Variable air volume AHU. Central cold-deck at fixed cooling setpoint.
///             Per-zone airflow modulation via VAV boxes. Zone reheat coils
///             (if present in the zone terminal) fire when zone needs heat.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AirLoopSystemType {
    PszAc,
    Doas,
    Fcu,
    Vav,
}

impl Default for AirLoopSystemType {
    fn default() -> Self { AirLoopSystemType::PszAc }
}


// ─── Air Loop Controls ───────────────────────────────────────────────────────

/// Controls section for an air loop — defines how the system responds to loads.
///
/// Supply temperatures, cycling method, deadband, economizer, and minimum
/// damper position all belong here (not on the thermostat, which only has
/// temperature goals).
///
/// ```yaml
/// air_loops:
///   - name: RTU-1
///     controls:
///       cooling_supply_temp: 13.0
///       heating_supply_temp: 35.0
///       cycling: proportional
///       deadband: 1.0
///       design_zone_flow: 0.5
///       minimum_damper_position: 0.15
///       economizer:
///         economizer_type: differential_dry_bulb
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AirLoopControls {
    /// Target supply air temperature for heating [°C] (default 35.0)
    #[serde(default = "default_controls_heating_supply")]
    pub heating_supply_temp: f64,
    /// Target supply air temperature for cooling [°C] (default 13.0)
    #[serde(default = "default_controls_cooling_supply")]
    pub cooling_supply_temp: f64,
    /// Capacity control method: on_off or proportional (default: proportional)
    #[serde(default)]
    pub cycling: CyclingMethod,
    /// Deadband width [°C] around setpoints (default 1.0)
    #[serde(default = "default_controls_deadband")]
    pub deadband: f64,
    /// Design air flow per zone [kg/s] (default 0.5)
    #[serde(default = "default_controls_zone_flow")]
    pub design_zone_flow: AutosizeValue,
    /// Minimum outdoor air damper position [0-1].
    /// If omitted, auto-calculated from zone outdoor air requirements.
    /// Falls back to 0.20 if no outdoor_air definitions exist.
    #[serde(default)]
    pub minimum_damper_position: Option<f64>,
    /// Economizer settings (optional — absent = no economizer)
    #[serde(default)]
    pub economizer: Option<EconomizerControls>,
}

impl Default for AirLoopControls {
    fn default() -> Self {
        Self {
            heating_supply_temp: 35.0,
            cooling_supply_temp: 13.0,
            cycling: CyclingMethod::default(),
            deadband: 1.0,
            design_zone_flow: AutosizeValue::Value(0.5),
            minimum_damper_position: None,
            economizer: None,
        }
    }
}

fn default_controls_heating_supply() -> f64 { 35.0 }
fn default_controls_cooling_supply() -> f64 { 13.0 }
fn default_controls_deadband() -> f64 { 1.0 }
fn default_controls_zone_flow() -> AutosizeValue { AutosizeValue::Value(0.5) }

/// Capacity control method for an air loop.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CyclingMethod {
    /// On/off cycling — system runs at full capacity or not at all
    OnOff,
    /// Proportional modulation — system output varies with load
    Proportional,
}

impl Default for CyclingMethod {
    fn default() -> Self { CyclingMethod::Proportional }
}

/// Economizer controls for outdoor air mixing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EconomizerControls {
    /// Economizer type (default: differential_dry_bulb)
    #[serde(default)]
    pub economizer_type: EconomizerType,
    /// High limit shutoff temperature [°C].
    /// Economizer disabled when outdoor temp exceeds this.
    #[serde(default)]
    pub high_limit: Option<f64>,
}

/// Economizer control strategy.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum EconomizerType {
    /// OA used when OA temp < return air temp
    DifferentialDryBulb,
    /// OA used when OA temp < fixed high limit
    FixedDryBulb,
    /// OA used when OA enthalpy < return air enthalpy
    DifferentialEnthalpy,
    /// No economizer — always use minimum damper position
    NoEconomizer,
}

impl Default for EconomizerType {
    fn default() -> Self { EconomizerType::DifferentialDryBulb }
}

// ─── Air Loop Input ──────────────────────────────────────────────────────────

/// Air loop input definition — the user-facing topology.
#[derive(Debug, Serialize, Deserialize)]
pub struct AirLoopInput {
    pub name: String,
    /// System type hint — if omitted, auto-detected from components and controls.
    /// Auto-detection rules:
    ///   - Has VAV fan → vav
    ///   - minimum_damper_position = 1.0 → doas
    ///   - Default → psz_ac
    /// Explicit values: psz_ac, doas, fcu, vav.
    #[serde(default)]
    pub system_type: Option<AirLoopSystemType>,
    /// Controls section — supply temps, cycling, deadband, economizer, damper position.
    #[serde(default)]
    pub controls: AirLoopControls,
    /// For VAV systems: minimum flow fraction at each zone box [0-1].
    /// Zone receives at least (min_vav_fraction * design_flow) at all times.
    /// Default: 0.30.
    #[serde(default = "default_min_vav_fraction")]
    pub min_vav_fraction: f64,
    /// HVAC availability schedule name. When 0, the system is OFF (fan off,
    /// no outdoor air, no heating/cooling). When absent or always-on, system
    /// runs every hour. Default: None (always available).
    #[serde(default)]
    pub availability_schedule: Option<String>,
    /// Supply-side equipment in order (air flows through them sequentially)
    pub equipment: Vec<EquipmentInput>,
    /// Zones served by this air loop
    #[serde(default)]
    pub zones: Vec<ZoneConnection>,
}

impl AirLoopInput {
    /// Get the minimum damper position from the controls section.
    pub fn minimum_damper_position(&self) -> Option<f64> {
        self.controls.minimum_damper_position
    }

    /// Detect the system behavior from components + controls, or use explicit hint.
    ///
    /// Auto-detection rules:
    ///   1. Explicit `system_type` → use it directly
    ///   2. Has a VAV fan → VAV
    ///   3. `minimum_damper_position: 1.0` → DOAS
    ///   4. Default → PSZ-AC
    pub fn detect_system_type(&self) -> AirLoopSystemType {
        if let Some(ref explicit) = self.system_type {
            return explicit.clone();
        }

        // Check for VAV fan
        for eq in &self.equipment {
            if let EquipmentInput::Fan(f) = eq {
                if f.source.eq_ignore_ascii_case("vav") {
                    return AirLoopSystemType::Vav;
                }
            }
        }

        // Check for DOAS (100% outdoor air)
        if let Some(pos) = self.minimum_damper_position() {
            if (pos - 1.0).abs() < 0.01 {
                return AirLoopSystemType::Doas;
            }
        }

        // Default: PSZ-AC
        AirLoopSystemType::PszAc
    }
}

fn default_min_vav_fraction() -> f64 { 0.30 }

/// Individual equipment specification.
/// Uses `type` to select the component category and `source` for the specific variant.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum EquipmentInput {
    #[serde(rename = "fan")]
    Fan(FanInput),
    #[serde(rename = "heating_coil")]
    HeatingCoil(HeatingCoilInput),
    #[serde(rename = "cooling_coil")]
    CoolingCoil(CoolingCoilInput),
    #[serde(rename = "heat_recovery")]
    HeatRecovery(HeatRecoveryInput),
}

#[derive(Debug, Serialize, Deserialize)]
pub struct FanInput {
    pub name: String,
    /// Fan source: "constant_volume", "vav", "on_off"
    #[serde(default = "default_fan_source")]
    pub source: String,
    /// Design flow rate [m³/s]. Use `autosize` to let the engine calculate.
    pub design_flow_rate: AutosizeValue,
    #[serde(default = "default_pressure_rise")]
    pub pressure_rise: f64,
    /// Motor efficiency [0-1] (default 0.9)
    #[serde(default = "default_motor_efficiency")]
    pub motor_efficiency: f64,
    /// Impeller efficiency [0-1] (default 0.71)
    #[serde(default = "default_impeller_efficiency")]
    pub impeller_efficiency: f64,
    #[serde(default = "default_motor_in_airstream")]
    pub motor_in_airstream_fraction: f64,
}

fn default_fan_source() -> String { "constant_volume".to_string() }
fn default_pressure_rise() -> f64 { 600.0 }
fn default_motor_efficiency() -> f64 { 0.9 }
fn default_impeller_efficiency() -> f64 { 0.71 }
fn default_motor_in_airstream() -> f64 { 1.0 }

#[derive(Debug, Serialize, Deserialize)]
pub struct HeatingCoilInput {
    pub name: String,
    /// Heating coil source: "electric", "gas", "hot_water"
    #[serde(default = "default_heating_source")]
    pub source: String,
    /// Nominal heating capacity [W]. Use `autosize` to let the engine calculate.
    pub capacity: AutosizeValue,
    #[serde(default = "default_setpoint")]
    pub setpoint: f64,
    #[serde(default = "default_efficiency")]
    pub efficiency: f64,
}

fn default_heating_source() -> String { "electric".to_string() }
fn default_setpoint() -> f64 { 35.0 }
fn default_efficiency() -> f64 { 1.0 }

#[derive(Debug, Serialize, Deserialize)]
pub struct CoolingCoilInput {
    pub name: String,
    /// Cooling coil source: "dx", "chilled_water"
    #[serde(default = "default_cooling_source")]
    pub source: String,
    /// Rated total cooling capacity [W]. Use `autosize` to let the engine calculate.
    pub capacity: AutosizeValue,
    /// Rated COP (coefficient of performance)
    #[serde(default = "default_cop")]
    pub cop: f64,
    /// Rated sensible heat ratio [0-1]
    #[serde(default = "default_shr")]
    pub shr: f64,
    /// Rated air flow rate [m³/s]. Use `autosize` to let the engine calculate.
    #[serde(default)]
    pub rated_airflow: AutosizeValue,
    /// Outlet temperature setpoint [°C]
    #[serde(default = "default_dx_coil_setpoint")]
    pub setpoint: f64,
    /// Reference to a top-level performance curve name for capacity f(T)
    #[serde(default)]
    pub cap_ft_curve: Option<String>,
    /// Reference to a top-level performance curve name for EIR f(T)
    #[serde(default)]
    pub eir_ft_curve: Option<String>,
}

fn default_cooling_source() -> String { "dx".to_string() }
fn default_cop() -> f64 { 3.5 }
fn default_shr() -> f64 { 0.8 }
fn default_dx_coil_setpoint() -> f64 { 13.0 }

#[derive(Debug, Serialize, Deserialize)]
pub struct HeatRecoveryInput {
    pub name: String,
    /// Heat recovery source: "wheel", "plate", "runaround_coil"
    #[serde(default = "default_hr_source")]
    pub source: String,
    /// Sensible effectiveness at design conditions [0-1]
    #[serde(default = "default_sensible_effectiveness")]
    pub sensible_effectiveness: f64,
    /// Latent effectiveness at design conditions [0-1] (0.0 for sensible-only)
    #[serde(default)]
    pub latent_effectiveness: f64,
}

fn default_hr_source() -> String { "wheel".to_string() }
fn default_sensible_effectiveness() -> f64 { 0.76 }

#[derive(Debug, Serialize, Deserialize)]
pub struct ZoneConnection {
    pub zone: String,
    #[serde(default)]
    pub terminal: Option<String>,
}

/// Plant loop input definition.
#[derive(Debug, Serialize, Deserialize)]
pub struct PlantLoopInput {
    pub name: String,
    pub supply_equipment: Vec<PlantEquipmentInput>,
    #[serde(default = "default_supply_temp")]
    pub design_supply_temp: f64,
    #[serde(default = "default_delta_t")]
    pub design_delta_t: f64,
}

fn default_supply_temp() -> f64 { 82.0 }
fn default_delta_t() -> f64 { 11.0 }

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum PlantEquipmentInput {
    #[serde(rename = "boiler")]
    Boiler(BoilerInput),
    #[serde(rename = "chiller")]
    Chiller(ChillerInput),
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BoilerInput {
    pub name: String,
    /// Nominal capacity [W]. Use `autosize` to let the engine calculate.
    pub capacity: AutosizeValue,
    #[serde(default = "default_boiler_efficiency")]
    pub efficiency: f64,
    #[serde(default = "default_boiler_outlet_temp")]
    pub design_outlet_temp: f64,
    /// Design water flow rate [m³/s]. Use `autosize` to let the engine calculate.
    #[serde(default)]
    pub design_water_flow_rate: AutosizeValue,
}

fn default_boiler_efficiency() -> f64 { 0.80 }
fn default_boiler_outlet_temp() -> f64 { 82.0 }

#[derive(Debug, Serialize, Deserialize)]
pub struct ChillerInput {
    pub name: String,
    /// Rated cooling capacity [W]. Use `autosize` to let the engine calculate.
    pub capacity: AutosizeValue,
    /// Rated COP at ARI conditions (typical 2.5-4.5 for air-cooled)
    #[serde(default = "default_chiller_cop")]
    pub cop: f64,
    /// Chilled water supply temperature setpoint [C]
    #[serde(default = "default_chw_setpoint")]
    pub chw_setpoint: f64,
    /// Design CHW flow rate [m3/s]. Calculated from capacity if not specified.
    #[serde(default)]
    pub design_chw_flow: f64,
}

fn default_chiller_cop() -> f64 { 3.5 }
fn default_chw_setpoint() -> f64 { 7.0 }

/// Design day input.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesignDayInput {
    pub name: String,
    pub design_temp: f64,
    pub daily_range: f64,
    pub humidity_type: String,
    pub humidity_value: f64,
    pub pressure: f64,
    pub wind_speed: f64,
    pub month: u32,
    pub day: u32,
    pub day_type: String,
}

// ─── Zone Group Input ─────────────────────────────────────────────────────────

/// Zone group definition — a named list of zones for referencing in thermostats
/// and zone load definitions.
///
/// Zone groups are purely organizational — they don't carry any control settings.
/// Setpoints belong on thermostats; supply temperatures and flow rates belong on
/// air loop controls.
///
/// ```yaml
/// zone_groups:
///   - name: Office Zones
///     zones: [East Office, West Office, North Office]
/// ```
#[derive(Debug, Serialize, Deserialize)]
pub struct ZoneGroupInput {
    pub name: String,
    pub zones: Vec<String>,
}

// ─── Control Input ───────────────────────────────────────────────────────────

/// Control definition — sets setpoints on components and loops.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ControlInput {
    /// Fixed setpoint on a coil or component
    #[serde(rename = "setpoint")]
    Setpoint(SetpointInput),
    /// Fixed setpoint on a plant loop
    #[serde(rename = "plant_loop_setpoint")]
    PlantLoopSetpoint(PlantLoopSetpointInput),
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SetpointInput {
    pub name: String,
    /// Target component name
    pub component: String,
    /// Setpoint value [°C]
    pub value: f64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PlantLoopSetpointInput {
    pub name: String,
    /// Target plant loop name
    pub loop_name: String,
    /// Supply temperature setpoint [°C]
    pub supply_temp: f64,
}

/// Parametric run configuration.
///
/// Allows varying parameters across multiple simulation runs — this is
/// the automation-first approach for batch analysis workflows.
#[derive(Debug, Serialize, Deserialize)]
pub struct ParametricInput {
    /// Named parameter sets. Each key is a run name, values override model parameters.
    pub runs: Vec<ParametricRun>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ParametricRun {
    pub name: String,
    /// Optional override weather file for this run
    #[serde(default)]
    pub weather_file: Option<String>,
    /// Parameter overrides (component_name.parameter_name -> value)
    #[serde(default)]
    pub overrides: std::collections::HashMap<String, f64>,
}

// ─── Model Builder ───────────────────────────────────────────────────────────

/// Parse a YAML model file and build the simulation graph.
pub fn load_model(path: &Path) -> Result<ModelInput, InputError> {
    let contents = std::fs::read_to_string(path)
        .map_err(|e| InputError::IoError(format!("{}: {}", path.display(), e)))?;
    parse_model_yaml(&contents)
}

/// Parse YAML string into a model.
pub fn parse_model_yaml(yaml: &str) -> Result<ModelInput, InputError> {
    serde_yaml::from_str(yaml).map_err(|e| InputError::ParseError(e.to_string()))
}

/// Build a simulation graph from parsed model input.
pub fn build_graph(model: &ModelInput) -> Result<SimulationGraph, InputError> {
    let mut graph = SimulationGraph::new();

    // Build air loops
    for air_loop in &model.air_loops {
        let mut prev_node = None;

        for equipment in &air_loop.equipment {
            let node = match equipment {
                EquipmentInput::Fan(f) => {
                    let fan_type = match f.source.as_str() {
                        "vav" | "VAV" => FanType::VAV,
                        "on_off" | "OnOff" => FanType::OnOff,
                        _ => FanType::ConstantVolume,
                    };
                    let flow = f.design_flow_rate.to_f64();
                    let total_efficiency = f.motor_efficiency * f.impeller_efficiency;
                    let mut fan = match fan_type {
                        FanType::VAV => Fan::vav(
                            &f.name,
                            flow,
                            f.pressure_rise,
                            total_efficiency,
                            f.motor_efficiency,
                            f.motor_in_airstream_fraction,
                        ),
                        _ => Fan::constant_volume(
                            &f.name,
                            flow,
                            f.pressure_rise,
                            total_efficiency,
                            f.motor_efficiency,
                            f.motor_in_airstream_fraction,
                        ),
                    };
                    fan.fan_type = fan_type;
                    graph.add_air_component(Box::new(fan))
                }
                EquipmentInput::HeatingCoil(c) => {
                    let cap = c.capacity.to_f64();
                    let coil = match c.source.as_str() {
                        "hot_water" | "HotWater" => HeatingCoil::hot_water(
                            &c.name,
                            cap,
                            c.setpoint,
                            0.001,  // default water flow
                            82.0,   // default water inlet temp
                            71.0,   // default water outlet temp
                        ),
                        "gas" | "Gas" | "furnace" | "Furnace" => HeatingCoil::gas(
                            &c.name,
                            cap,
                            c.setpoint,
                            c.efficiency,
                        ),
                        _ => HeatingCoil::electric(&c.name, cap, c.setpoint),
                    };
                    graph.add_air_component(Box::new(coil))
                }
                EquipmentInput::CoolingCoil(c) => {
                    // Look up optional performance curves by name
                    let cap_curve = c.cap_ft_curve.as_ref().and_then(|name| {
                        model.performance_curves.iter().find(|pc| pc.name == *name).cloned()
                    });
                    let eir_curve = c.eir_ft_curve.as_ref().and_then(|name| {
                        model.performance_curves.iter().find(|pc| pc.name == *name).cloned()
                    });
                    let coil = CoolingCoilDX::new(
                        &c.name,
                        c.capacity.to_f64(),
                        c.cop,
                        c.shr,
                        c.rated_airflow.to_f64(),
                        c.setpoint,
                    ).with_curves(cap_curve, eir_curve);
                    graph.add_air_component(Box::new(coil))
                }
                EquipmentInput::HeatRecovery(hr) => {
                    let erv = match hr.source.as_str() {
                        "plate" | "plate_hx" => HeatRecovery::plate_hx(
                            &hr.name,
                            hr.sensible_effectiveness,
                            0.0, // no parasitic power by default
                        ),
                        _ => HeatRecovery::enthalpy_wheel(
                            &hr.name,
                            hr.sensible_effectiveness,
                            hr.latent_effectiveness,
                            0.0, // no parasitic power by default
                        ),
                    };
                    graph.add_air_component(Box::new(erv))
                }
            };

            // Connect to previous component in sequence
            if let Some(prev) = prev_node {
                graph.connect_air(prev, node);
            }
            prev_node = Some(node);
        }
    }

    // Build plant loops
    for plant_loop in &model.plant_loops {
        let mut prev_node = None;

        for equipment in &plant_loop.supply_equipment {
            let node = match equipment {
                PlantEquipmentInput::Boiler(b) => {
                    let boiler = Boiler::new(
                        &b.name,
                        b.capacity.to_f64(),
                        b.efficiency,
                        b.design_outlet_temp,
                        b.design_water_flow_rate.to_f64(),
                    );
                    graph.add_plant_component(Box::new(boiler))
                }
                PlantEquipmentInput::Chiller(ci) => {
                    let capacity = ci.capacity.to_f64();
                    let chw_flow = if ci.design_chw_flow <= 0.0 {
                        if capacity > 0.0 {
                            capacity / (4186.0 * 5.0 * 1000.0)
                        } else {
                            0.005
                        }
                    } else {
                        ci.design_chw_flow
                    };
                    let chiller = AirCooledChiller::new(
                        &ci.name,
                        capacity,
                        ci.cop,
                        ci.chw_setpoint,
                        chw_flow,
                    );
                    graph.add_plant_component(Box::new(chiller))
                }
            };

            if let Some(prev) = prev_node {
                graph.connect_water(prev, node);
            }
            prev_node = Some(node);
        }
    }

    // Compute simulation order
    graph
        .compute_simulation_order()
        .map_err(|e| InputError::GraphError(e.to_string()))?;

    Ok(graph)
}

/// Build controllers from parsed model input.
///
/// Returns a list of boxed Controller trait objects ready to use in the simulation.
pub fn build_controllers(model: &ModelInput) -> Vec<Box<dyn Controller>> {
    let mut controllers: Vec<Box<dyn Controller>> = Vec::new();

    // Resolve thermostats, expanding zone group references to individual zones.
    let resolved_thermostats = resolve_thermostats(model);

    // Default supply temps and design flow — these will come from air loop
    // controls once Phase 2 is wired up. For now, use standard defaults.
    let default_heating_supply = 35.0;  // °C
    let default_cooling_supply = 13.0;  // °C
    let default_zone_flow = 0.5;        // kg/s

    for tstat in &resolved_thermostats {
        let group = ZoneGroup {
            name: tstat.name.clone(),
            zones: tstat.zones.clone(),
            heating_setpoint: tstat.heating_setpoint,
            cooling_setpoint: tstat.cooling_setpoint,
            deadband: None,
        };

        // Look up air loop controls for supply temps + design flow.
        // Find the first air loop that serves any of this thermostat's zones.
        let (heat_supply, cool_supply, zone_flow) = model.air_loops.iter()
            .find(|al| {
                al.zones.iter().any(|zc| tstat.zones.contains(&zc.zone))
            })
            .map(|al| {
                (
                    al.controls.heating_supply_temp,
                    al.controls.cooling_supply_temp,
                    al.controls.design_zone_flow.to_f64(),
                )
            })
            .unwrap_or((default_heating_supply, default_cooling_supply, default_zone_flow));

        let thermostat = ZoneThermostat::from_groups(
            &format!("{} Thermostat", tstat.name),
            vec![group],
            heat_supply,
            cool_supply,
            zone_flow,
        );
        controllers.push(Box::new(thermostat));
    }

    // Build explicit controls
    for control in &model.controls {
        match control {
            ControlInput::Setpoint(sp) => {
                let ctrl = SetpointController::air_setpoint(
                    &sp.name,
                    &sp.component,
                    sp.value,
                );
                controllers.push(Box::new(ctrl));
            }
            ControlInput::PlantLoopSetpoint(pls) => {
                let ctrl = PlantLoopSetpoint::new(
                    &pls.name,
                    &pls.loop_name,
                    pls.supply_temp,
                );
                controllers.push(Box::new(ctrl));
            }
        }
    }

    controllers
}

/// Build a building envelope from parsed model input.
///
/// Returns `None` if no zones or surfaces are defined (HVAC-only models).
pub fn build_envelope(
    model: &ModelInput,
    latitude: f64,
    longitude: f64,
    time_zone: f64,
    elevation: f64,
) -> Option<openbse_envelope::BuildingEnvelope> {
    if model.zones.is_empty() || model.surfaces.is_empty() {
        return None;
    }

    // Resolve top-level zone load objects into per-zone ZoneInput fields.
    // This bridges the new top-level input format with the existing runtime code
    // which reads zone.input.infiltration, zone.input.internal_gains, etc.
    let zones = resolve_zone_loads(model);

    let mut env = openbse_envelope::BuildingEnvelope::from_input_full(
        model.materials.clone(),
        model.constructions.clone(),
        model.window_constructions.clone(),
        model.simple_constructions.clone(),
        zones,
        model.surfaces.clone(),
        latitude,
        longitude,
        time_zone,
        elevation,
    );

    // Set up schedule manager from model schedules
    env.schedule_manager = openbse_envelope::ScheduleManager::from_inputs(model.schedules.clone());

    // Set shading calculation mode from simulation settings
    env.shading_calculation = model.simulation.shading_calculation;

    // Set site terrain for wind profile calculations
    env.terrain = model.simulation.terrain;
    log::info!("Site terrain: {:?} (wind exp={:.2}, BL height={:.0}m)",
        env.terrain, env.terrain.wind_exp(), env.terrain.wind_bl_height());

    // Resolve shading surfaces and register them with the envelope
    // (always resolve geometry so it's ready if mode is switched at runtime)
    env.resolve_shading(&model.surfaces, &model.shading_surfaces);

    if env.shading_calculation == openbse_envelope::ShadingCalculation::Detailed {
        log::info!("Shading calculation: DETAILED (geometric shadow calculations enabled)");
        // Compute diffuse sky shading ratios using hemisphere sampling
        env.compute_diffuse_shading_ratios();
    } else {
        log::info!("Shading calculation: BASIC (all surfaces fully sunlit)");
    }

    Some(env)
}

/// Resolve top-level zone load objects into per-zone `ZoneInput` fields.
///
/// For each top-level object (people, lights, equipment, infiltration,
/// ventilation, exhaust_fans, outdoor_air, ideal_loads), expand to the
/// zones listed in its `zones` field. The resolved data is merged into
/// the zone's existing fields (supporting both old embedded format and
/// new top-level format simultaneously).
fn resolve_zone_loads(model: &ModelInput) -> Vec<openbse_envelope::ZoneInput> {
    use openbse_envelope::{InternalGainInput, InfiltrationInput};
    use openbse_envelope::zone::{
        IdealLoadsAirSystem, VentilationScheduleEntry,
        ExhaustFanInput, OutdoorAirInput,
    };

    let mut zones = model.zones.clone();

    // Build zone name → zone group names mapping for expansion
    let zone_group_map: std::collections::HashMap<String, Vec<String>> = model.zone_groups.iter()
        .map(|zg| (zg.name.clone(), zg.zones.clone()))
        .collect();

    // Helper: expand a zone list (which may contain zone group names) to individual zone names
    let expand_zones = |zone_refs: &[String]| -> Vec<String> {
        let mut result = Vec::new();
        for name in zone_refs {
            if let Some(group_zones) = zone_group_map.get(name) {
                result.extend(group_zones.clone());
            } else {
                result.push(name.clone());
            }
        }
        result
    };

    // Resolve top-level people → internal_gains (People variant)
    // Supports: count (absolute), people_per_area [people/m²], area_per_person [m²/person]
    for people in &model.people {
        let target_zones = expand_zones(&people.zones);
        for zone_name in &target_zones {
            if let Some(zone) = zones.iter_mut().find(|z| z.name == *zone_name) {
                let count = if let Some(ppa) = people.people_per_area {
                    // people_per_area × floor_area = number of people
                    ppa * zone.floor_area
                } else if let Some(app) = people.area_per_person {
                    // floor_area / area_per_person = number of people
                    if app > 0.0 { zone.floor_area / app } else { 0.0 }
                } else {
                    people.count
                };
                zone.internal_gains.push(InternalGainInput::People {
                    count,
                    activity_level: people.activity_level,
                    radiant_fraction: people.radiant_fraction,
                    schedule: people.schedule.clone(),
                });
            }
        }
    }

    // Resolve top-level lights → internal_gains (Lights variant)
    for lights in &model.lights {
        let target_zones = expand_zones(&lights.zones);
        for zone_name in &target_zones {
            if let Some(zone) = zones.iter_mut().find(|z| z.name == *zone_name) {
                // If watts_per_area is specified, multiply by zone floor area
                let power = if let Some(wpf) = lights.watts_per_area {
                    wpf * zone.floor_area
                } else {
                    lights.power
                };
                zone.internal_gains.push(InternalGainInput::Lights {
                    power,
                    radiant_fraction: lights.radiant_fraction,
                    return_air_fraction: lights.return_air_fraction,
                    schedule: lights.schedule.clone(),
                });
            }
        }
    }

    // Resolve top-level equipment → internal_gains (Equipment variant)
    for equip in &model.equipment {
        let target_zones = expand_zones(&equip.zones);
        for zone_name in &target_zones {
            if let Some(zone) = zones.iter_mut().find(|z| z.name == *zone_name) {
                let power = if let Some(wpf) = equip.watts_per_area {
                    wpf * zone.floor_area
                } else {
                    equip.power
                };
                zone.internal_gains.push(InternalGainInput::Equipment {
                    power,
                    radiant_fraction: equip.radiant_fraction,
                    schedule: equip.schedule.clone(),
                });
            }
        }
    }

    // Pre-compute exterior wall area per zone (needed for flow_per_exterior_wall_area)
    // Exterior wall area = sum of gross areas of all wall surfaces with outdoor boundary
    let exterior_wall_area: std::collections::HashMap<String, f64> = {
        let mut map: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
        for surf in &model.surfaces {
            if surf.surface_type == openbse_envelope::SurfaceType::Wall
                && surf.boundary == openbse_envelope::BoundaryCondition::Outdoor
            {
                // Compute surface area from vertices if available, otherwise use area field
                let area = if let Some(ref verts) = surf.vertices {
                    if verts.len() >= 3 {
                        openbse_envelope::geometry::polygon_area(verts)
                    } else {
                        surf.area
                    }
                } else {
                    surf.area
                };
                *map.entry(surf.zone.clone()).or_insert(0.0) += area;
            }
        }
        map
    };

    // Resolve top-level infiltration
    // Supports: design_flow_rate, air_changes_per_hour, flow_per_floor_area, flow_per_exterior_wall_area
    for infil in &model.infiltration {
        let target_zones = expand_zones(&infil.zones);
        for zone_name in &target_zones {
            if let Some(zone) = zones.iter_mut().find(|z| z.name == *zone_name) {
                // Resolve the specification method to design_flow_rate and/or air_changes_per_hour
                let (resolved_flow, resolved_ach) = if let Some(fpfa) = infil.flow_per_floor_area {
                    // flow_per_floor_area × floor_area = design_flow_rate [m³/s]
                    (fpfa * zone.floor_area, 0.0)
                } else if let Some(fpewa) = infil.flow_per_exterior_wall_area {
                    // flow_per_exterior_wall_area × ext_wall_area = design_flow_rate [m³/s]
                    let ext_area = exterior_wall_area.get(zone_name).copied().unwrap_or(0.0);
                    (fpewa * ext_area, 0.0)
                } else {
                    // Use explicit design_flow_rate or air_changes_per_hour
                    (infil.design_flow_rate, infil.air_changes_per_hour)
                };
                zone.infiltration.push(InfiltrationInput {
                    design_flow_rate: resolved_flow,
                    air_changes_per_hour: resolved_ach,
                    coeff_a: infil.constant_coefficient,
                    coeff_b: infil.temperature_coefficient,
                    coeff_c: infil.wind_coefficient,
                    coeff_d: infil.wind_squared_coefficient,
                    schedule: infil.schedule.clone(),
                });
            }
        }
    }

    // Resolve top-level ventilation
    // Supports: flow_rate, ach_rate, per_person, per_area, with combining_method (sum or max)
    for vent in &model.ventilation {
        let target_zones = expand_zones(&vent.zones);
        for zone_name in &target_zones {
            if let Some(zone) = zones.iter_mut().find(|z| z.name == *zone_name) {
                // Resolve per_person and per_area into flow_rate using zone data
                let mut resolved_flow = vent.flow_rate;
                let resolved_ach = vent.ach_rate;

                // Compute per_person and per_area flow components
                let pp_flow = if let Some(pp) = vent.per_person {
                    // Count people already resolved for this zone
                    let people_count: f64 = zone.internal_gains.iter().map(|g| {
                        match g {
                            InternalGainInput::People { count, .. } => *count,
                            _ => 0.0,
                        }
                    }).sum();
                    pp * people_count
                } else {
                    0.0
                };

                let pa_flow = if let Some(pa) = vent.per_area {
                    pa * zone.floor_area
                } else {
                    0.0
                };

                // Apply combining method
                if pp_flow > 0.0 || pa_flow > 0.0 {
                    let occupancy_based = match vent.combining_method {
                        openbse_envelope::VentilationCombiningMethod::Sum => pp_flow + pa_flow,
                        openbse_envelope::VentilationCombiningMethod::Maximum => pp_flow.max(pa_flow),
                    };
                    resolved_flow += occupancy_based;
                }

                zone.ventilation_schedule.push(VentilationScheduleEntry {
                    start_hour: vent.start_hour,
                    end_hour: vent.end_hour,
                    flow_rate: resolved_flow,
                    ach_rate: resolved_ach,
                    min_indoor_temp: vent.min_indoor_temp,
                    outdoor_temp_must_be_lower: vent.outdoor_temp_must_be_lower,
                });
            }
        }
    }

    // Resolve top-level exhaust fans
    for exhaust in &model.exhaust_fans {
        let target_zones = expand_zones(&exhaust.zones);
        for zone_name in &target_zones {
            if let Some(zone) = zones.iter_mut().find(|z| z.name == *zone_name) {
                zone.exhaust_fan = Some(ExhaustFanInput {
                    flow_rate: exhaust.flow_rate,
                    schedule: exhaust.schedule.clone(),
                });
            }
        }
    }

    // Resolve top-level outdoor air
    for oa in &model.outdoor_air {
        let target_zones = expand_zones(&oa.zones);
        for zone_name in &target_zones {
            if let Some(zone) = zones.iter_mut().find(|z| z.name == *zone_name) {
                zone.outdoor_air = Some(OutdoorAirInput {
                    per_person: oa.per_person,
                    per_area: oa.per_area,
                });
            }
        }
    }

    // Resolve top-level ideal loads
    for il in &model.ideal_loads {
        let target_zones = expand_zones(&il.zones);
        for zone_name in &target_zones {
            if let Some(zone) = zones.iter_mut().find(|z| z.name == *zone_name) {
                zone.ideal_loads = Some(IdealLoadsAirSystem {
                    heating_setpoint: il.heating_setpoint,
                    cooling_setpoint: il.cooling_setpoint,
                    heating_capacity: il.heating_capacity,
                    cooling_capacity: il.cooling_capacity,
                });
                // Copy thermostat schedule (e.g., Case 640 nighttime setback)
                if !il.thermostat_schedule.is_empty() {
                    zone.thermostat_schedule = il.thermostat_schedule.clone();
                }
            }
        }
    }

    zones
}

/// Compute minimum outdoor air fraction for an air loop from zone ventilation requirements.
///
/// Uses ASHRAE 62.1 methodology:
///   total_oa = Σ (oa.per_person × people_count + oa.per_area × floor_area)
///   min_oa_fraction = total_oa / design_supply_flow
///
/// Returns the computed fraction clamped to [0, 1], or `default_fraction` if
/// no outdoor air data is available for the served zones.
pub fn compute_oa_fraction(
    model: &ModelInput,
    air_loop: &AirLoopInput,
    resolved_zones: &[openbse_envelope::ZoneInput],
    default_fraction: f64,
) -> f64 {
    let served_zone_names: Vec<String> = air_loop.zones.iter()
        .map(|zc| zc.zone.clone())
        .collect();

    if served_zone_names.is_empty() {
        return default_fraction;
    }

    // Compute total outdoor air requirement [m³/s] from zone OA definitions
    let mut total_oa_flow = 0.0_f64;
    let mut has_oa_data = false;

    for zone_name in &served_zone_names {
        // Find the resolved zone data
        let zone = resolved_zones.iter().find(|z| z.name == *zone_name);
        let zone_input = model.zones.iter().find(|z| z.name == *zone_name);

        if let Some(zone) = zone {
            if let Some(oa) = &zone.outdoor_air {
                has_oa_data = true;
                let floor_area = zone.floor_area;

                // Count people from resolved internal gains
                let people_count: f64 = zone.internal_gains.iter().map(|g| {
                    match g {
                        openbse_envelope::InternalGainInput::People { count, .. } => *count,
                        _ => 0.0,
                    }
                }).sum();

                total_oa_flow += oa.per_person * people_count + oa.per_area * floor_area;
            }
        } else if let Some(zi) = zone_input {
            // Zone not in envelope — use raw model input
            if let Some(oa) = &zi.outdoor_air {
                has_oa_data = true;
                let floor_area = zi.floor_area;

                let people_count: f64 = zi.internal_gains.iter().map(|g| {
                    match g {
                        openbse_envelope::InternalGainInput::People { count, .. } => *count,
                        _ => 0.0,
                    }
                }).sum();

                total_oa_flow += oa.per_person * people_count + oa.per_area * floor_area;
            }
        }
    }

    if !has_oa_data || total_oa_flow <= 0.0 {
        return default_fraction;
    }

    // Find design supply flow from the fan in this air loop
    let design_flow = air_loop.equipment.iter().find_map(|eq| {
        match eq {
            EquipmentInput::Fan(f) => {
                let flow = f.design_flow_rate.to_f64();
                if flow > 0.0 { Some(flow) } else { None }
            }
            _ => None,
        }
    });

    match design_flow {
        Some(flow) => {
            // OA flow is in m³/s and fan design flow is also in m³/s,
            // so the ratio is dimensionally consistent.
            (total_oa_flow / flow).clamp(0.0, 1.0)
        }
        _ => default_fraction,
    }
}

/// Resolve thermostat definitions, expanding zone group references to
/// individual zone names.
///
/// Returns a flat list of resolved thermostat definitions with individual zone names.
pub fn resolve_thermostats(model: &ModelInput) -> Vec<openbse_envelope::ThermostatInput> {
    // Build zone group name → zone names mapping
    let zone_group_map: std::collections::HashMap<String, Vec<String>> = model.zone_groups.iter()
        .map(|zg| (zg.name.clone(), zg.zones.clone()))
        .collect();

    // Expand zone references (which may include zone group names)
    let expand_zones = |zone_refs: &[String]| -> Vec<String> {
        let mut result = Vec::new();
        for name in zone_refs {
            if let Some(group_zones) = zone_group_map.get(name) {
                result.extend(group_zones.clone());
            } else {
                result.push(name.clone());
            }
        }
        result
    };

    model.thermostats.iter().map(|t| {
        let mut resolved = t.clone();
        resolved.zones = expand_zones(&t.zones);
        resolved
    }).collect()
}

#[derive(Debug, thiserror::Error)]
pub enum InputError {
    #[error("IO error: {0}")]
    IoError(String),
    #[error("YAML parse error: {0}")]
    ParseError(String),
    #[error("Graph construction error: {0}")]
    GraphError(String),
    #[error("Validation error: {0}")]
    ValidationError(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_example_yaml_with_envelope() {
        let yaml = include_str!("../../../examples/simple_heating.yaml");
        let model = parse_model_yaml(yaml).expect("Failed to parse example YAML");

        // HVAC still works
        assert_eq!(model.air_loops.len(), 1);
        assert_eq!(model.air_loops[0].name, "Main AHU");
        assert_eq!(model.zone_groups.len(), 2);
        assert_eq!(model.controls.len(), 1);

        // Thermostat definitions parsed (new top-level section)
        assert_eq!(model.thermostats.len(), 2);
        assert_eq!(model.thermostats[0].name, "Office Thermostat");
        assert_eq!(model.thermostats[0].zones, vec!["Office Zones"]);
        assert!((model.thermostats[0].heating_setpoint - 21.1).abs() < 0.01);
        assert!((model.thermostats[0].cooling_setpoint - 23.9).abs() < 0.01);

        // Thermostat resolution expands zone group references
        let resolved = resolve_thermostats(&model);
        assert_eq!(resolved.len(), 2);
        // "Office Zones" zone group should expand to individual zone names
        assert_eq!(resolved[0].zones, vec!["East Office", "West Office"]);
        assert!((resolved[0].heating_setpoint - 21.1).abs() < 0.01);
        // "Conference Rooms" zone group should expand
        assert_eq!(resolved[1].zones, vec!["Conference Room"]);
        assert!((resolved[1].heating_setpoint - 20.0).abs() < 0.01);

        // Envelope sections parsed
        assert_eq!(model.materials.len(), 3);
        assert_eq!(model.constructions.len(), 3);
        assert_eq!(model.window_constructions.len(), 1);
        assert_eq!(model.zones.len(), 3);
        assert_eq!(model.surfaces.len(), 11);

        // Zones are now geometry-only (loads are in top-level sections)
        let east = &model.zones[0];
        assert_eq!(east.name, "East Office");
        assert!(east.internal_gains.is_empty(), "Gains moved to top-level");
        assert!(east.infiltration.is_empty(), "Infiltration moved to top-level");

        // Verify top-level zone load sections parsed correctly
        assert_eq!(model.people.len(), 2);   // Office + Conference
        assert_eq!(model.lights.len(), 2);   // Office + Conference
        assert_eq!(model.equipment.len(), 1); // Office only
        assert_eq!(model.infiltration.len(), 2); // Office + Conference

        // Verify people assigned to correct zones
        let office_people = &model.people[0];
        assert_eq!(office_people.zones, vec!["East Office", "West Office"]);
        assert!((office_people.count - 5.0).abs() < 0.01);

        // Verify build_envelope correctly resolves top-level loads to zones
        let envelope = build_envelope(&model, 39.74, -104.99, -7.0, 0.0);
        assert!(envelope.is_some());
        let env = envelope.unwrap();

        // After resolution, zones should have gains populated
        let east_zone = &env.zones[0];
        assert_eq!(east_zone.input.name, "East Office");
        assert_eq!(east_zone.input.internal_gains.len(), 3,
            "East Office should have 3 resolved gains (people + lights + equipment)");
        assert!(!east_zone.input.infiltration.is_empty(),
            "East Office should have resolved infiltration");
        assert!(east_zone.input.infiltration[0].design_flow_rate > 0.0);
    }

    #[test]
    fn test_parse_hvac_only_yaml_no_envelope() {
        let yaml = r#"
simulation:
  timesteps_per_hour: 1
weather_files:
  - test.epw
air_loops:
  - name: AHU1
    equipment:
      - type: fan
        name: Fan1
        design_flow_rate: 1.0
"#;
        let model = parse_model_yaml(yaml).expect("Failed to parse HVAC-only YAML");
        assert_eq!(model.air_loops.len(), 1);
        assert!(model.zones.is_empty());
        assert!(model.surfaces.is_empty());

        // build_envelope should return None for HVAC-only models
        let envelope = build_envelope(&model, 40.0, -105.0, -7.0, 0.0);
        assert!(envelope.is_none());
    }

    #[test]
    fn test_parse_ashrae140_case600() {
        let yaml = r#"
simulation:
  timesteps_per_hour: 4
  start_month: 1
  start_day: 1
  end_month: 12
  end_day: 31
weather_files:
  - "weather/725650TY.csv"
simple_constructions:
  - name: Case600 Wall
    u_factor: 0.559
    thickness: 0.087
    thermal_capacity: 14000.0
    solar_absorptance: 0.6
    thermal_absorptance: 0.9
  - name: Case600 Roof
    u_factor: 0.334
    thickness: 0.141
    thermal_capacity: 15000.0
    solar_absorptance: 0.6
    thermal_absorptance: 0.9
  - name: Case600 Floor
    u_factor: 0.040
    thickness: 1.028
    thermal_capacity: 5000.0
    solar_absorptance: 0.6
    thermal_absorptance: 0.9
window_constructions:
  - name: Case600 Window
    u_factor: 3.0
    shgc: 0.789
    visible_transmittance: 0.74
outputs:
  - file: "zone_results.csv"
    frequency: hourly
    variables:
      - zone_temperature
      - zone_heating_rate
      - zone_cooling_rate
      - site_outdoor_temperature
summary_report: true
zones:
  - name: Case600 Zone
    solar_distribution:
      floor_fraction: 0.642
      wall_fraction: 0.191
      ceiling_fraction: 0.167
equipment:
  - name: Internal Gains
    zones: [Case600 Zone]
    power: 200.0
    radiant_fraction: 0.6
infiltration:
  - name: Zone Infiltration
    zones: [Case600 Zone]
    air_changes_per_hour: 0.5
ideal_loads:
  - name: Zone Ideal Loads
    zones: [Case600 Zone]
    heating_setpoint: 20.0
    cooling_setpoint: 27.0
    heating_capacity: 1000000.0
    cooling_capacity: 1000000.0
surfaces:
  - name: South Wall
    zone: Case600 Zone
    type: wall
    construction: Case600 Wall
    boundary: outdoor
    vertices:
      - {x: 0.0, y: 0.0, z: 0.0}
      - {x: 8.0, y: 0.0, z: 0.0}
      - {x: 8.0, y: 0.0, z: 2.7}
      - {x: 0.0, y: 0.0, z: 2.7}
  - name: South Window 1
    zone: Case600 Zone
    type: window
    construction: Case600 Window
    boundary: outdoor
    parent_surface: South Wall
    vertices:
      - {x: 0.5, y: 0.0, z: 0.2}
      - {x: 3.5, y: 0.0, z: 0.2}
      - {x: 3.5, y: 0.0, z: 2.2}
      - {x: 0.5, y: 0.0, z: 2.2}
  - name: South Window 2
    zone: Case600 Zone
    type: window
    construction: Case600 Window
    boundary: outdoor
    parent_surface: South Wall
    vertices:
      - {x: 4.5, y: 0.0, z: 0.2}
      - {x: 7.5, y: 0.0, z: 0.2}
      - {x: 7.5, y: 0.0, z: 2.2}
      - {x: 4.5, y: 0.0, z: 2.2}
  - name: North Wall
    zone: Case600 Zone
    type: wall
    construction: Case600 Wall
    boundary: outdoor
    vertices:
      - {x: 8.0, y: 6.0, z: 0.0}
      - {x: 0.0, y: 6.0, z: 0.0}
      - {x: 0.0, y: 6.0, z: 2.7}
      - {x: 8.0, y: 6.0, z: 2.7}
  - name: East Wall
    zone: Case600 Zone
    type: wall
    construction: Case600 Wall
    boundary: outdoor
    vertices:
      - {x: 8.0, y: 0.0, z: 0.0}
      - {x: 8.0, y: 6.0, z: 0.0}
      - {x: 8.0, y: 6.0, z: 2.7}
      - {x: 8.0, y: 0.0, z: 2.7}
  - name: West Wall
    zone: Case600 Zone
    type: wall
    construction: Case600 Wall
    boundary: outdoor
    vertices:
      - {x: 0.0, y: 6.0, z: 0.0}
      - {x: 0.0, y: 0.0, z: 0.0}
      - {x: 0.0, y: 0.0, z: 2.7}
      - {x: 0.0, y: 6.0, z: 2.7}
  - name: Roof
    zone: Case600 Zone
    type: roof
    construction: Case600 Roof
    boundary: outdoor
    vertices:
      - {x: 0.0, y: 0.0, z: 2.7}
      - {x: 8.0, y: 0.0, z: 2.7}
      - {x: 8.0, y: 6.0, z: 2.7}
      - {x: 0.0, y: 6.0, z: 2.7}
  - name: Floor
    zone: Case600 Zone
    type: floor
    construction: Case600 Floor
    boundary: outdoor
    vertices:
      - {x: 0.0, y: 6.0, z: 0.0}
      - {x: 8.0, y: 6.0, z: 0.0}
      - {x: 8.0, y: 0.0, z: 0.0}
      - {x: 0.0, y: 0.0, z: 0.0}
"#;
        let model = parse_model_yaml(yaml).expect("Failed to parse ASHRAE 140 Case 600 YAML");

        // Verify parsing
        assert_eq!(model.simple_constructions.len(), 3);
        assert_eq!(model.window_constructions.len(), 1);
        assert_eq!(model.zones.len(), 1);
        assert_eq!(model.surfaces.len(), 8); // 4 walls + roof + floor + 2 windows
        assert_eq!(model.air_loops.len(), 0); // No air loops — uses ideal_loads

        // Zone should have auto-calculate volume (0 in YAML)
        assert_eq!(model.zones[0].volume, 0.0);

        // Top-level ideal loads should be parsed
        assert_eq!(model.ideal_loads.len(), 1);
        assert!((model.ideal_loads[0].heating_setpoint - 20.0).abs() < 0.01);
        assert!((model.ideal_loads[0].cooling_setpoint - 27.0).abs() < 0.01);

        // Top-level infiltration should be parsed
        assert_eq!(model.infiltration.len(), 1);
        assert!((model.infiltration[0].air_changes_per_hour - 0.5).abs() < 0.01);

        // Top-level equipment should be parsed
        assert_eq!(model.equipment.len(), 1);

        // Zone should have solar distribution (stays on zone)
        assert!(model.zones[0].solar_distribution.is_some(), "Zone should have solar_distribution");
        let sd = model.zones[0].solar_distribution.as_ref().unwrap();
        assert!((sd.floor_fraction - 0.642).abs() < 0.01);

        // Surfaces should have vertices
        for surf in &model.surfaces {
            assert!(surf.vertices.is_some(), "Surface '{}' missing vertices", surf.name);
        }

        // Build envelope and verify auto-calculations
        let envelope = build_envelope(&model, 39.74, -105.18, -7.0, 0.0);
        assert!(envelope.is_some());
        let env = envelope.unwrap();

        // Envelope should detect ideal loads
        assert!(env.has_ideal_loads(), "Envelope should have ideal loads");

        // Zone volume should be auto-calculated: 8 x 6 x 2.7 = 129.6 m3
        let zone = &env.zones[0];
        assert!((zone.input.volume - 129.6).abs() < 1.0,
            "Zone volume should be ~129.6, got {}", zone.input.volume);

        // Floor area should be auto-calculated: 8 x 6 = 48.0 m2
        assert!((zone.input.floor_area - 48.0).abs() < 0.5,
            "Floor area should be ~48.0, got {}", zone.input.floor_area);

        // South wall should be 21.6 m2 (8 x 2.7) with azimuth 180, tilt 90
        let south_wall = env.surfaces.iter()
            .find(|s| s.input.name == "South Wall")
            .expect("South Wall not found");
        assert!((south_wall.input.area - 21.6).abs() < 0.1,
            "South Wall area should be 21.6, got {}", south_wall.input.area);
        assert!((south_wall.input.azimuth - 180.0).abs() < 1.0,
            "South Wall azimuth should be 180, got {}", south_wall.input.azimuth);
        assert!((south_wall.input.tilt - 90.0).abs() < 1.0,
            "South Wall tilt should be 90, got {}", south_wall.input.tilt);

        // Each window should be 6.0 m2 (3 x 2)
        let win1 = env.surfaces.iter()
            .find(|s| s.input.name == "South Window 1")
            .expect("South Window 1 not found");
        assert!((win1.input.area - 6.0).abs() < 0.1,
            "Window area should be 6.0, got {}", win1.input.area);

        // South wall net area = 21.6 - 2*6.0 = 9.6 m2
        assert!((south_wall.net_area - 9.6).abs() < 0.1,
            "South Wall net area should be 9.6, got {}", south_wall.net_area);
    }
}
