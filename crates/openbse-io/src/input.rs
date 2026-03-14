//! YAML input parser for OpenBSE models.
//!
//! The user writes a YAML file describing the building and HVAC system.
//! The engine parses it and builds the simulation graph automatically.
//! No nodes, branches, branch lists, or connector lists — just components
//! and what connects to what.

use openbse_components::boiler::Boiler;
use openbse_components::chiller::AirCooledChiller;
use openbse_components::chw_cooling_coil::CoolingCoilCHW;
use openbse_components::cooling_coil::CoolingCoilDX;
use openbse_components::fan::{Fan, FanType};
use openbse_components::heat_pump_coil::HeatPumpHeatingCoil;
use openbse_components::heat_recovery::HeatRecovery;
use openbse_components::heating_coil::HeatingCoil;
use openbse_components::pfp_box::PFPBox;
use openbse_components::vav_box::{VAVBox, ReheatType};
use openbse_controls::thermostat::{ZoneGroup, ZoneThermostat};
use openbse_controls::setpoint::{SetpointController, PlantLoopSetpoint};
use openbse_controls::Controller;
use openbse_core::graph::SimulationGraph;
use openbse_core::simulation::SimulationConfig;
use openbse_core::types::AutosizeValue;
use serde::{Deserialize, Serialize};
use std::path::Path;

// Re-export SolarDistributionMethod from the envelope crate.
pub use openbse_envelope::SolarDistributionMethod;

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
    /// F-factor ground floor construction definitions.
    ///
    /// Models ground-contact floor heat loss using perimeter-based F-factor method:
    ///     Q = F × P × (T_zone - T_ground)
    /// Matches EnergyPlus `Construction:FfactorGroundFloor`.
    /// Surfaces using these constructions must specify `exposed_perimeter`.
    #[serde(default)]
    pub f_factor_constructions: Vec<openbse_envelope::FFactorConstruction>,
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

    // ─── Domestic Hot Water ──────────────────────────────────────────────────

    /// Domestic hot water system definitions
    #[serde(default)]
    pub dhw_systems: Vec<DhwSystemInput>,

    // ─── Exterior Equipment ─────────────────────────────────────────────────

    /// Exterior equipment (facility-level loads not in any zone)
    #[serde(default)]
    pub exterior_equipment: Vec<ExteriorEquipmentInput>,

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

    /// Controls how infiltration interacts with exhaust fans and HVAC airflows.
    ///
    /// - `basic` (default): Fixed infiltration rate, independent of exhaust/HVAC.
    ///   Matches E+ ZoneInfiltration:DesignFlowRate (no AirflowNetwork).
    /// - `ashrae_combined`: Q_combined = sqrt(Q_infil² + Q_unbalanced_exhaust²).
    ///   More physically correct but differs from E+ without AFN.
    #[serde(default)]
    pub infiltration_interaction: openbse_envelope::InfiltrationInteraction,

    /// Monthly ground surface temperatures [°C] for surfaces with `boundary: ground`.
    ///
    /// 12 values, January through December. Matches EnergyPlus
    /// `Site:GroundTemperature:BuildingSurface`. Default: 18°C all months
    /// (E+ default when no BuildingSurface object is present).
    ///
    /// ```yaml
    /// ground_surface_temperatures: [18, 18, 18, 18, 18, 18, 18, 18, 18, 18, 18, 18]
    /// ```
    #[serde(default = "default_ground_surface_temps")]
    pub ground_surface_temperatures: Vec<f64>,

    /// Heating sizing factor applied to zone design loads and airflows.
    ///
    /// Matches EnergyPlus `Sizing:Parameters` → `Heating Sizing Factor`.
    /// A value of 1.25 means 25% safety margin.  Default: 1.25.
    #[serde(default = "default_heating_sizing_factor")]
    pub heating_sizing_factor: f64,

    /// Cooling sizing factor applied to zone design loads and airflows.
    ///
    /// Matches EnergyPlus `Sizing:Parameters` → `Cooling Sizing Factor`.
    /// A value of 1.15 means 15% safety margin.  Default: 1.15.
    #[serde(default = "default_cooling_sizing_factor")]
    pub cooling_sizing_factor: f64,

    /// Solar distribution method for interior beam solar radiation.
    ///
    /// Matches EnergyPlus `Building` → `Solar Distribution` field.
    ///
    /// - `full_exterior` (default): All beam solar is assumed to fall on
    ///   the floor, where it is absorbed and slowly released from the
    ///   thermal mass.  Matches E+ "FullExterior" behavior.
    /// - `full_interior_and_exterior`: Beam solar is geometrically projected
    ///   through windows onto interior surfaces (walls, floor, ceiling) using
    ///   ray-tracing.  More physically accurate but requires convex zones
    ///   with complete vertex data.  Matches E+ "FullInteriorAndExterior".
    #[serde(default)]
    pub solar_distribution: SolarDistributionMethod,
}

fn default_ground_surface_temps() -> Vec<f64> { vec![18.0; 12] }
fn default_heating_sizing_factor() -> f64 { 1.25 }
fn default_cooling_sizing_factor() -> f64 { 1.15 }
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
/// Air loop system type — determines airflow control behavior, OA handling,
/// and zone connection topology.
///
/// - `psz_ac` / `packaged_single_zone`: Single-zone packaged AC/RTU — one thermostat drives mode
/// - `vav` / `variable_air_volume`: Central VAV AHU — per-zone airflow modulation via VAV boxes
/// - `doas` / `dedicated_outdoor_air`: 100% outdoor air system — fixed supply temp, no zone recirculation
/// - `ptac` / `packaged_terminal`: Packaged terminal AC — one unit per zone
/// - `fcu` / `fan_coil_unit`: Per-zone recirculating fan coil (no OA mixing)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AirLoopSystemType {
    #[serde(alias = "packaged_single_zone")]
    PszAc,
    #[serde(alias = "dedicated_outdoor_air")]
    Doas,
    #[serde(alias = "fan_coil_unit")]
    Fcu,
    #[serde(alias = "packaged_terminal")]
    Ptac,
    #[serde(alias = "variable_air_volume")]
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
    /// Fan operating mode: cycling (default) or continuous.
    /// Cycling: fan cycles with coils (off during deadband).
    /// Continuous: fan runs at full speed always, coils cycle.
    #[serde(default)]
    pub fan_operating_mode: FanOperatingMode,
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
            fan_operating_mode: FanOperatingMode::default(),
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

/// Fan operating mode for unitary systems (PTAC, PSZ-AC).
///
/// In E+, the PTAC field "Supply Air Fan Operating Mode Schedule Name"
/// controls whether the fan cycles with the compressor or runs continuously:
///   - Schedule value = 0 → cycling fan (fan cycles with coils)
///   - Schedule value ≠ 0 → continuous fan (fan runs at full speed always)
///
/// The DOE prototype Mid-Rise Apartment uses "COMPACT HVAC-ALWAYS 1"
/// (value=1 → continuous fan mode): fans run at rated power for all 8760
/// hours and coils cycle ON/OFF as needed.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum FanOperatingMode {
    /// Fan cycles ON/OFF with coils. During deadband, fan is OFF.
    /// Average fan power = rated × PLR.
    Cycling,
    /// Fan runs at full speed continuously. Coils cycle ON/OFF.
    /// Fan power = rated (always). Fan heat is always delivered to the zone.
    Continuous,
}

impl Default for FanOperatingMode {
    fn default() -> Self { FanOperatingMode::Cycling }
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
    /// Demand-controlled ventilation: modulates outdoor air based on real-time
    /// occupancy.  When `true`, the minimum OA fraction is recalculated each
    /// timestep from each zone's current occupancy schedule value × per-person
    /// OA rate + per-area OA rate (ASHRAE 62.1 Ventilation Rate Procedure).
    /// Default: false (fixed minimum OA fraction from sizing).
    #[serde(default)]
    pub dcv: bool,
    /// Supply-side equipment in order (air flows through them sequentially)
    pub equipment: Vec<EquipmentInput>,
    /// Zone terminal connections — links zones to this air loop,
    /// optionally with terminal boxes (VAV, PFP).
    #[serde(default, alias = "zones")]
    pub zone_terminals: Vec<ZoneConnection>,
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
    #[serde(rename = "humidifier")]
    Humidifier(HumidifierInput),
    #[serde(rename = "duct")]
    Duct(DuctInput),
}

/// Electric steam humidifier.
///
/// ```yaml
/// - type: humidifier
///   name: DC Humidifier
///   rated_power: 100000
///   min_rh_setpoint: 0.30
/// ```
#[derive(Debug, Serialize, Deserialize)]
pub struct HumidifierInput {
    pub name: String,
    /// Maximum electric power [W] (e.g., 100000). Use `autosize` for autosizing.
    #[serde(default = "default_humidifier_power")]
    pub rated_power: f64,
    /// Minimum relative humidity setpoint [0-1] (e.g., 0.30 for 30% RH)
    #[serde(default = "default_min_rh")]
    pub min_rh_setpoint: f64,
    /// Zone cooling setpoint temperature [°C] used as reference for RH→w conversion.
    /// Should match the cooling thermostat setpoint of the zone served (default 24.0).
    #[serde(default = "default_zone_cooling_sp")]
    pub zone_cooling_setpoint: f64,
}
fn default_humidifier_power() -> f64 { 100_000.0 }
fn default_min_rh() -> f64 { 0.30 }
fn default_zone_cooling_sp() -> f64 { 24.0 }

/// Air duct with conduction losses and leakage.
///
/// ```yaml
/// - type: duct
///   name: Supply Duct
///   length: 15.0
///   diameter: 0.3
///   u_value: 0.71
///   leakage_fraction: 0.04
///   ambient_zone: basement
/// ```
#[derive(Debug, Serialize, Deserialize)]
pub struct DuctInput {
    pub name: String,
    /// Duct length [m]
    pub length: f64,
    /// Duct hydraulic diameter [m]
    #[serde(default = "default_duct_diameter")]
    pub diameter: f64,
    /// Overall U-value [W/(m²·K)] (default 0.71 = R-8 insulation)
    #[serde(default = "default_duct_u_value")]
    pub u_value: f64,
    /// Fraction of air lost through leaks [0-1] (default 0.04)
    #[serde(default = "default_duct_leakage")]
    pub leakage_fraction: f64,
    /// Zone surrounding the duct: "outdoor", "ground", or a zone name
    #[serde(default = "default_duct_ambient")]
    pub ambient_zone: String,
}
fn default_duct_diameter() -> f64 { 0.3 }
fn default_duct_u_value() -> f64 { 0.71 }
fn default_duct_leakage() -> f64 { 0.04 }
fn default_duct_ambient() -> String { "outdoor".to_string() }

#[derive(Debug, Serialize, Deserialize)]
pub struct FanInput {
    pub name: String,
    /// Fan source: "constant_volume", "vav", "on_off"
    #[serde(default = "default_fan_source")]
    pub source: String,
    /// Free-form tag for output classification (e.g., "supply", "return",
    /// "exhaust", "transfer", "relief"). Used for end-use subcategory reporting.
    #[serde(default)]
    pub tag: Option<String>,
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
    /// VAV fan power curve coefficients [C1..C5].
    /// Power = C1 + C2*PLR + C3*PLR^2 + C4*PLR^3 + C5*PLR^4
    /// Default matches typical ASHRAE forward-curved centrifugal fan.
    #[serde(default)]
    pub vav_coefficients: Option<[f64; 5]>,
}

fn default_fan_source() -> String { "constant_volume".to_string() }
fn default_pressure_rise() -> f64 { 600.0 }
fn default_motor_efficiency() -> f64 { 0.9 }
fn default_impeller_efficiency() -> f64 { 0.71 }
fn default_motor_in_airstream() -> f64 { 1.0 }

#[derive(Debug, Serialize, Deserialize)]
pub struct HeatingCoilInput {
    pub name: String,
    /// Heating coil source: "electric", "gas", "hot_water", "heat_pump"
    #[serde(default = "default_heating_source")]
    pub source: String,
    /// Nominal heating capacity [W]. Use `autosize` to let the engine calculate.
    pub capacity: AutosizeValue,
    #[serde(default = "default_setpoint")]
    pub setpoint: f64,
    /// Burner/conversion efficiency [0-1] (default 1.0).
    /// Applies to `gas` coils (burner efficiency, e.g. 0.80) and `electric` (1.0).
    /// Ignored for `hot_water` coils — boiler efficiency is set on the boiler.
    /// Ignored for `heat_pump` coils — use `cop` instead.
    #[serde(default = "default_efficiency")]
    pub efficiency: f64,

    // ─── Heat pump fields (only used when source: heat_pump) ────────
    /// Rated COP for heat pump (default 3.0)
    #[serde(default = "default_hp_cop")]
    pub cop: f64,
    /// Rated airflow [m³/s] for heat pump
    #[serde(default)]
    pub rated_airflow: f64,
    /// Supplemental electric resistance capacity [W] (0 = none)
    #[serde(default)]
    pub supplemental_capacity: f64,
    /// Compressor lockout temperature [°C] (default: -17.78 / 0°F)
    #[serde(default)]
    pub lockout_temp: Option<f64>,
    /// Performance curve name for capacity f(T_outdoor)
    #[serde(default)]
    pub cap_ft_curve: Option<String>,
    /// Performance curve name for EIR f(T_outdoor)
    #[serde(default)]
    pub eir_ft_curve: Option<String>,

    // ─── Hot water fields (only used when source: hot_water) ─────────
    /// Plant loop name this coil connects to (for hot_water source)
    #[serde(default)]
    pub plant_loop: Option<String>,
}

fn default_heating_source() -> String { "electric".to_string() }
fn default_setpoint() -> f64 { 35.0 }
fn default_efficiency() -> f64 { 1.0 }
fn default_hp_cop() -> f64 { 3.0 }

#[derive(Debug, Serialize, Deserialize)]
pub struct CoolingCoilInput {
    pub name: String,
    /// Cooling coil source: "dx", "chilled_water"
    #[serde(default = "default_cooling_source")]
    pub source: String,
    /// Rated total cooling capacity [W]. Use `autosize` to let the engine calculate.
    pub capacity: AutosizeValue,
    /// Rated COP (coefficient of performance) — used for DX source only
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
    /// Reference to a top-level performance curve name for PLF f(PLR)
    #[serde(default)]
    pub plf_curve: Option<String>,

    // ─── Chilled water fields (only used when source: chilled_water) ──
    /// Design chilled water flow rate [m³/s]
    #[serde(default)]
    pub design_water_flow_rate: f64,
    /// Design CHW supply temperature [°C] (default 6.7 / 44°F)
    #[serde(default = "default_chw_supply_temp")]
    pub design_water_inlet_temp: f64,
    /// Design CHW return temperature [°C] (default 12.2 / 54°F)
    #[serde(default = "default_chw_return_temp")]
    pub design_water_outlet_temp: f64,
    /// Plant loop name this coil connects to
    #[serde(default)]
    pub plant_loop: Option<String>,
}

fn default_cooling_source() -> String { "dx".to_string() }
fn default_cop() -> f64 { 3.5 }
fn default_shr() -> f64 { 0.8 }
fn default_dx_coil_setpoint() -> f64 { 13.0 }
fn default_chw_supply_temp() -> f64 { 6.7 }
fn default_chw_return_temp() -> f64 { 12.2 }

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
    /// Parasitic electric power [W] (wheel motor, controls). Default 0.0.
    #[serde(default)]
    pub parasitic_power: f64,
}

fn default_hr_source() -> String { "wheel".to_string() }
fn default_sensible_effectiveness() -> f64 { 0.76 }

/// Zone connection — links a zone to an air loop, optionally with a terminal box.
///
/// ```yaml
/// zones:
///   - zone: South Perimeter
///     terminal:
///       type: vav_box
///       name: VAV Box South
///       max_air_flow: autosize
///       min_flow_fraction: 0.30
///       reheat_type: hot_water
///       reheat_capacity: autosize
/// ```
#[derive(Debug, Serialize, Deserialize)]
pub struct ZoneConnection {
    pub zone: String,
    /// Terminal box for this zone (VAV box, PFP box, or omitted for direct connection)
    #[serde(default)]
    pub terminal: Option<TerminalInput>,
    /// DCV: outdoor air per person [m³/s-person] (ASHRAE 62.1 Rp component).
    /// Only used when `dcv: true` on the air loop.
    #[serde(default)]
    pub per_person_oa: Option<f64>,
    /// DCV: outdoor air per floor area [m³/s-m²] (ASHRAE 62.1 Ra component).
    /// Only used when `dcv: true` on the air loop.
    #[serde(default)]
    pub per_area_oa: Option<f64>,
}

/// Terminal box types for zone connections.
///
/// Terminal boxes sit between the AHU supply duct and the zone.
/// They modulate airflow, provide reheat, and optionally add
/// secondary fan power (PFP boxes).
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum TerminalInput {
    /// Variable Air Volume box with optional reheat
    #[serde(rename = "vav_box")]
    VavBox(VavBoxInput),
    /// Parallel Fan-Powered box with electric reheat
    #[serde(rename = "pfp_box")]
    PfpBox(PfpBoxInput),
}

/// VAV box input — variable air volume terminal with optional reheat coil.
///
/// ```yaml
/// terminal:
///   type: vav_box
///   name: VAV Box South
///   max_air_flow: autosize
///   min_flow_fraction: 0.30
///   reheat_type: hot_water       # "none", "electric", "hot_water"
///   reheat_capacity: autosize
///   max_reheat_temp: 35.0
///   plant_loop: HHW Loop         # required for hot_water reheat
/// ```
#[derive(Debug, Serialize, Deserialize)]
pub struct VavBoxInput {
    pub name: String,
    /// Maximum primary air flow [kg/s]. Use `autosize` to let the engine calculate.
    pub max_air_flow: AutosizeValue,
    /// Minimum flow fraction [0-1] (default 0.30)
    #[serde(default = "default_min_flow_fraction")]
    pub min_flow_fraction: f64,
    /// Reheat coil type: "none", "electric", "hot_water" (default: "none")
    #[serde(default = "default_reheat_type")]
    pub reheat_type: String,
    /// Reheat coil capacity [W]. Use `autosize` to let the engine calculate.
    #[serde(default)]
    pub reheat_capacity: AutosizeValue,
    /// Maximum reheat discharge air temperature [°C] (default 35.0)
    #[serde(default)]
    pub max_reheat_temp: Option<f64>,
    /// Plant loop name for hot water reheat
    #[serde(default)]
    pub plant_loop: Option<String>,
}

/// Parallel fan-powered box input — secondary fan + electric reheat.
///
/// ```yaml
/// terminal:
///   type: pfp_box
///   name: PFP Box Core
///   max_primary_flow: autosize
///   min_primary_fraction: 0.30
///   secondary_fan_flow: 0.5
///   reheat_capacity: 5000
/// ```
#[derive(Debug, Serialize, Deserialize)]
pub struct PfpBoxInput {
    pub name: String,
    /// Maximum primary air flow [kg/s]. Use `autosize` to let the engine calculate.
    pub max_primary_flow: AutosizeValue,
    /// Minimum primary flow fraction [0-1] (default 0.30)
    #[serde(default = "default_min_flow_fraction")]
    pub min_primary_fraction: f64,
    /// Secondary fan flow rate [kg/s]. Use `autosize` to let the engine calculate.
    pub secondary_fan_flow: AutosizeValue,
    /// Electric reheat coil capacity [W]. Use `autosize` to let the engine calculate.
    #[serde(default)]
    pub reheat_capacity: AutosizeValue,
    /// Secondary air (plenum) temperature [°C] (default: zone return air temp)
    #[serde(default)]
    pub secondary_air_temp: Option<f64>,
}

fn default_min_flow_fraction() -> f64 { 0.30 }
fn default_reheat_type() -> String { "none".to_string() }

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
    #[serde(rename = "pump")]
    Pump(PumpInput),
    #[serde(rename = "cooling_tower")]
    CoolingTower(CoolingTowerInput),
    #[serde(rename = "heat_exchanger")]
    HeatExchanger(HeatExchangerInput),
}

/// Pump role in the plant loop — determines staging and control behavior.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PumpRole {
    /// Single pump or unspecified (default behavior)
    #[default]
    Single,
    /// Primary-loop pump (constant flow through chiller/boiler)
    Primary,
    /// Secondary/distribution pump (variable flow to demand side)
    Secondary,
    /// Headered pump bank (multiple pumps stage on/off)
    Headered,
}

/// Pump control strategy — how the pump modulates flow.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PumpControlStrategy {
    /// Pump follows load — flow varies with demand (default for variable_speed)
    #[default]
    Demand,
    /// Pump runs at constant speed whenever plant is active
    Continuous,
    /// Pump is OFF unless staged on by plant controller
    Staged,
}

/// Pump definition for plant water loops.
///
/// Design power = Q × H / (motor_efficiency × impeller_efficiency).
/// In EnergyPlus, impeller_efficiency is an internal default (~0.667) that
/// accounts for hydraulic/mechanical losses in the pump impeller, distinct
/// from motor losses. The effective "total efficiency" ≈ 0.9 × 0.667 = 0.60.
#[derive(Debug, Serialize, Deserialize)]
pub struct PumpInput {
    pub name: String,
    /// Pump type: "constant_speed" or "variable_speed" (default: "variable_speed")
    #[serde(default = "default_pump_type")]
    pub pump_type: String,
    /// Design water flow rate [m³/s]. Use `autosize` to let the engine calculate.
    pub design_flow_rate: AutosizeValue,
    /// Design pump head [Pa] (default: 179352 Pa ≈ 60 ft H2O)
    #[serde(default = "default_pump_head")]
    pub design_head: f64,
    /// Motor efficiency [0-1] (default: 0.9).
    /// Controls fraction of motor heat going to fluid stream.
    #[serde(default = "default_pump_motor_eff")]
    pub motor_efficiency: f64,
    /// Impeller/hydraulic efficiency [0-1] (default: 0.667).
    /// Combined with motor efficiency gives total pump efficiency.
    /// Design power = Q × H / (motor_eff × impeller_eff).
    #[serde(default = "default_impeller_eff")]
    pub impeller_efficiency: f64,
    /// Number of pumps in headered configuration (default: 1).
    /// For HeaderedPumps: pumps stage on/off to match demand.
    /// Each individual pump's design flow = total design flow / num_pumps.
    #[serde(default = "default_num_pumps")]
    pub num_pumps: u32,
    /// Fraction of motor inefficiency heat added to the fluid stream [0-1] (default: 1.0).
    /// E+ field: "Fraction of Motor Inefficiencies to Fluid Stream".
    /// Set to 0.0 to match E+ models where pump heat doesn't warm the water.
    #[serde(default = "default_motor_heat_to_fluid")]
    pub motor_heat_to_fluid_fraction: f64,
    /// Part-load power curve coefficients [c1, c2, c3, c4] for variable speed pumps.
    /// power_frac = c1 + c2*PLR + c3*PLR² + c4*PLR³
    /// Default: [0, 0, 0, 1] (pure cubic / affinity laws).
    /// E+ common curve: [0, 0.5726, -0.3010, 0.7347]
    #[serde(default)]
    pub power_curve: Option<Vec<f64>>,
    /// Pump role in the plant loop (default: single).
    /// Use `primary`/`secondary` for primary-secondary pumping configurations.
    #[serde(default)]
    pub role: PumpRole,
    /// Control strategy (default: demand — follows load).
    /// Use `continuous` for constant-speed primary pumps.
    #[serde(default)]
    pub control_strategy: PumpControlStrategy,
}

fn default_num_pumps() -> u32 { 1 }
fn default_impeller_eff() -> f64 { 0.667 }
fn default_motor_heat_to_fluid() -> f64 { 1.0 }

fn default_pump_type() -> String { "variable_speed".to_string() }
fn default_pump_head() -> f64 { 179_352.0 }
fn default_pump_motor_eff() -> f64 { 0.9 }

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

/// Performance curve coefficients for YAML input.
///
/// Supports biquadratic (6 coefficients) and quadratic (3 coefficients).
/// The curve type is inferred from the number of coefficients:
///   - 3 coefficients → quadratic: f(x) = c1 + c2*x + c3*x²
///   - 6 coefficients → biquadratic: f(x,y) = c1 + c2*x + c3*x² + c4*y + c5*y² + c6*x*y
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CurveInput {
    pub coefficients: Vec<f64>,
    #[serde(default = "default_curve_min")]
    pub min_x: f64,
    #[serde(default = "default_curve_max")]
    pub max_x: f64,
    #[serde(default = "default_curve_min")]
    pub min_y: f64,
    #[serde(default = "default_curve_max")]
    pub max_y: f64,
}

fn default_curve_min() -> f64 { -100.0 }
fn default_curve_max() -> f64 { 100.0 }

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
    /// Condenser type: "air_cooled" (default) or "water_cooled"
    #[serde(default = "default_condenser_type")]
    pub condenser_type: String,
    /// For water-cooled: fixed condenser entering water temperature [°C].
    /// Matches E+ SetpointManager:Scheduled on the condenser loop.
    /// If not set, falls back to outdoor wet-bulb + tower_approach.
    #[serde(default)]
    pub condenser_entering_temp: Option<f64>,
    /// For water-cooled (fallback): approach offset from outdoor wet-bulb to
    /// condenser entering water temperature [°C]. Default 5.56°C (10°F).
    /// Only used when condenser_entering_temp is not set.
    #[serde(default = "default_tower_approach")]
    pub tower_approach: f64,
    /// Minimum part load ratio (default 0.25, matching E+ typical value)
    #[serde(default = "default_chiller_min_plr")]
    pub min_plr: f64,
    /// CAPFT: Capacity as function of temperature (biquadratic).
    /// x = leaving CHW temp [°C], y = entering condenser fluid temp [°C].
    #[serde(default)]
    pub capft: Option<CurveInput>,
    /// EIRFT: EIR as function of temperature (biquadratic).
    /// x = leaving CHW temp [°C], y = entering condenser fluid temp [°C].
    #[serde(default)]
    pub eirft: Option<CurveInput>,
    /// EIRFPLR: EIR as function of part load ratio (quadratic).
    /// x = PLR [0-1].
    #[serde(default)]
    pub eirfplr: Option<CurveInput>,
    /// Name of the condenser water plant loop (for water-cooled chillers).
    /// When set, the chiller's condenser heat rejection drives demand on this loop.
    #[serde(default)]
    pub condenser_plant_loop: Option<String>,
}

fn default_chiller_cop() -> f64 { 3.5 }
fn default_chw_setpoint() -> f64 { 7.0 }
fn default_condenser_type() -> String { "air_cooled".to_string() }
fn default_tower_approach() -> f64 { 5.56 }
fn default_chiller_min_plr() -> f64 { 0.25 }

/// Cooling tower input for condenser water loops.
#[derive(Debug, Serialize, Deserialize)]
pub struct CoolingTowerInput {
    pub name: String,
    /// Tower type: "single_speed", "two_speed", or "variable_speed" (default)
    #[serde(default = "default_ct_tower_type")]
    pub tower_type: String,
    /// Design water flow rate [m³/s]. Use `autosize` to size from condenser demand.
    #[serde(default)]
    pub design_water_flow: AutosizeValue,
    /// Design air flow rate [m³/s]
    pub design_air_flow: f64,
    /// Design fan power [W]
    pub design_fan_power: f64,
    /// Design inlet water temperature [°C] (default 35)
    #[serde(default = "default_ct_inlet_temp")]
    pub design_inlet_water_temp: f64,
    /// Design approach temperature [°C] — T_water_out - T_wb (default 3.9)
    #[serde(default = "default_ct_approach")]
    pub design_approach: f64,
    /// Design range [°C] — T_water_in - T_water_out (default 5.56)
    #[serde(default = "default_ct_range")]
    pub design_range: f64,
    /// Fan power ratio curve coefficients [C1, C2, C3, C4, C5].
    /// PLF = C1 + C2*PLR + C3*PLR^2 + C4*PLR^3 + C5*PLR^4.
    /// Same polynomial form as the VAV fan curve.
    /// Default: E+ CoolingTower:VariableSpeed FanPowerRatioFunctionofAirFlowRateRatio.
    #[serde(default)]
    pub fan_power_curve: Option<Vec<f64>>,
}

fn default_ct_tower_type() -> String { "variable_speed".to_string() }
fn default_ct_inlet_temp() -> f64 { 35.0 }
fn default_ct_approach() -> f64 { 3.9 }
fn default_ct_range() -> f64 { 5.56 }

/// Water-to-water heat exchanger input for inter-loop connections.
///
/// Connects two plant loops via a plate-and-frame (or similar) heat exchanger.
/// Installed on the demand loop; draws heat from/rejects heat to the source loop.
/// Can operate in always-on mode (general inter-loop HX) or economizer mode
/// (activates only when source conditions enable free cooling).
#[derive(Debug, Serialize, Deserialize)]
pub struct HeatExchangerInput {
    pub name: String,
    /// Name of the source plant loop this HX draws from/rejects to
    pub source_loop: String,
    /// Heat transfer effectiveness [0-1] (default 0.80)
    #[serde(default = "default_hx_effectiveness")]
    pub effectiveness: f64,
    /// Design flow rate on the demand side [m³/s]. Use `autosize`.
    #[serde(default)]
    pub design_flow_rate: AutosizeValue,
    /// Control mode: "always_on" (default) or "economizer"
    #[serde(default = "default_hx_control")]
    pub control_mode: String,
    /// For economizer mode: activate when T_source < demand_inlet + threshold [°C] (default 2.0)
    #[serde(default = "default_hx_threshold")]
    pub economizer_threshold: f64,
}

fn default_hx_effectiveness() -> f64 { 0.80 }
fn default_hx_control() -> String { "always_on".to_string() }
fn default_hx_threshold() -> f64 { 2.0 }

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
    /// How internal gains are handled during this design day.
    ///
    /// - `off`: No internal gains (0%). Best for heating design days.
    /// - `full`: Full design-level gains at all hours (100%). Best for cooling design days.
    /// - `scheduled`: Follow normal schedules hour-by-hour.
    /// - `full_when_occupied`: Full gains when schedule > 0, zero when unoccupied.
    ///
    /// If omitted, defaults based on `day_type`: heating → `off`, cooling → `full`.
    #[serde(default)]
    pub internal_gains: Option<openbse_core::ports::SizingInternalGains>,
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

// ─── Domestic Hot Water ──────────────────────────────────────────────────────

/// Domestic hot water system definition.
///
/// A DHW system consists of a water heater, one or more draw profiles (loads),
/// and a cold water inlet (mains) temperature. The water heater maintains a
/// storage tank at a setpoint temperature; loads draw hot water and replace it
/// with cold mains water.
///
/// ```yaml
/// dhw_systems:
///   - name: Residential DHW
///     mains_temperature: 10.0
///     water_heater:
///       name: Gas Water Heater
///       fuel_type: gas
///       tank_volume: 189
///       capacity: 11720
///       efficiency: 0.80
///       setpoint: 60.0
///       ua_standby: 2.0
///     loads:
///       - name: Domestic Hot Water
///         peak_flow_rate: 0.0003
///         schedule: DHW Schedule
///         use_temperature: 43.3
/// ```
#[derive(Debug, Serialize, Deserialize)]
pub struct DhwSystemInput {
    pub name: String,
    /// Water heater equipment
    pub water_heater: WaterHeaterInput,
    /// Hot water draw profiles
    #[serde(default)]
    pub loads: Vec<DhwLoadInput>,
    /// Cold water mains temperature [°C] (default 10.0)
    #[serde(default = "default_mains_temp")]
    pub mains_temperature: f64,
    /// Optional circulation pump for the SWH loop.
    /// Runs whenever there is a DHW draw (flow > 0).
    #[serde(default)]
    pub pump: Option<PumpInput>,
}

/// Water heater equipment input.
///
/// Models a storage tank water heater with deadband thermostat control.
/// Fuel types: gas (burner), electric (resistance element), or heat_pump (HPWH).
#[derive(Debug, Serialize, Deserialize)]
pub struct WaterHeaterInput {
    pub name: String,
    /// Fuel type: "gas", "electric", "heat_pump" (default: "gas")
    #[serde(default = "default_dhw_fuel")]
    pub fuel_type: String,
    /// Tank storage volume [liters]
    pub tank_volume: f64,
    /// Burner/element input capacity [W]
    pub capacity: f64,
    /// Thermal efficiency [0-1] for gas/electric, COP for heat_pump (default 0.80)
    #[serde(default = "default_wh_efficiency")]
    pub efficiency: f64,
    /// Tank setpoint temperature [°C] (default 60.0)
    #[serde(default = "default_wh_setpoint")]
    pub setpoint: f64,
    /// Standby loss coefficient [W/K] (default 2.0)
    #[serde(default = "default_wh_ua")]
    pub ua_standby: f64,
    /// Thermostat deadband [°C] (default 5.0)
    #[serde(default = "default_wh_deadband")]
    pub deadband: f64,
    /// Constant parasitic fuel consumption [W] (default 0.0).
    ///
    /// Represents continuous fuel draw that does NOT heat the tank
    /// (pilot light, jacket heaters, controls). Added to fuel consumption
    /// regardless of burner state. Matches E+ Off/On Cycle Parasitic fields.
    #[serde(default)]
    pub parasitic_power: f64,
}

/// DHW draw profile (load).
///
/// Each load represents a hot water end use (showers, sinks, laundry, etc.)
/// with a peak flow rate and a schedule that modulates it over time.
#[derive(Debug, Serialize, Deserialize)]
pub struct DhwLoadInput {
    pub name: String,
    /// Peak hot water draw rate [L/s]
    pub peak_flow_rate: f64,
    /// Schedule name (fraction 0-1 of peak flow). If absent, always at peak.
    #[serde(default)]
    pub schedule: Option<String>,
    /// Target use temperature at the fixture [°C] (default 43.3 / 110°F)
    #[serde(default = "default_use_temp")]
    pub use_temperature: f64,
}

fn default_mains_temp() -> f64 { 10.0 }
fn default_dhw_fuel() -> String { "gas".to_string() }
fn default_wh_efficiency() -> f64 { 0.80 }
fn default_wh_setpoint() -> f64 { 60.0 }
fn default_wh_ua() -> f64 { 2.0 }
fn default_wh_deadband() -> f64 { 5.0 }
fn default_use_temp() -> f64 { 43.3 }

// ─── Exterior Equipment ─────────────────────────────────────────────────────

/// Exterior equipment definition — facility-level loads not assigned to any zone.
///
/// Examples: parking lot lighting, signage, exterior landscape lighting.
/// The power draw is added to facility electricity (or gas) consumption
/// without contributing to any zone's internal gains.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExteriorEquipmentInput {
    pub name: String,
    /// Design power [W]
    pub power: f64,
    /// Schedule name (fraction 0-1). If absent, always at full power.
    #[serde(default)]
    pub schedule: Option<String>,
    /// Fuel type: "electricity" or "natural_gas" (default: "electricity")
    #[serde(default = "default_exterior_fuel")]
    pub fuel: String,
    /// Subcategory label for end-use reporting
    #[serde(default)]
    pub subcategory: Option<String>,
    /// When true, power is only applied during nighttime (solar altitude ≤ 0).
    /// Matches E+ Exterior:Lights AstronomicalClock control option.
    #[serde(default)]
    pub astronomical_clock: bool,
}

fn default_exterior_fuel() -> String { "electricity".to_string() }

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
                    // Apply tag for output classification
                    if let Some(ref tag) = f.tag {
                        fan.tag = tag.clone();
                    }
                    // Apply custom VAV curve coefficients if provided
                    if let Some(coeffs) = f.vav_coefficients {
                        fan.vav_coefficients = coeffs;
                    }
                    graph.add_air_component(Box::new(fan))
                }
                EquipmentInput::HeatingCoil(c) => {
                    let cap = c.capacity.to_f64();
                    match c.source.as_str() {
                        "heat_pump" | "HeatPump" | "hp" => {
                            // Air-source heat pump DX heating coil
                            let rated_airflow = if c.rated_airflow > 0.0 {
                                c.rated_airflow
                            } else {
                                // Default: derive from capacity (500 CFM/ton ≈ 0.0568 m³/s per 3517W)
                                cap / 3517.0 * 0.0568
                            };
                            let mut coil = HeatPumpHeatingCoil::new(
                                &c.name,
                                cap,
                                c.cop,
                                rated_airflow,
                                c.setpoint,
                            );
                            if c.supplemental_capacity > 0.0 {
                                coil = coil.with_supplemental(c.supplemental_capacity);
                            }
                            if let Some(lockout) = c.lockout_temp {
                                coil = coil.with_lockout_temp(lockout);
                            }
                            // Look up optional performance curves by name
                            let cap_curve = c.cap_ft_curve.as_ref().and_then(|name| {
                                model.performance_curves.iter().find(|pc| pc.name == *name).cloned()
                            });
                            let eir_curve = c.eir_ft_curve.as_ref().and_then(|name| {
                                model.performance_curves.iter().find(|pc| pc.name == *name).cloned()
                            });
                            coil = coil.with_curves(cap_curve, eir_curve);
                            graph.add_air_component(Box::new(coil))
                        }
                        "hot_water" | "HotWater" => {
                            let coil = HeatingCoil::hot_water(
                                &c.name,
                                cap,
                                c.setpoint,
                                0.001,  // default water flow
                                82.0,   // default water inlet temp
                                71.0,   // default water outlet temp
                            );
                            graph.add_air_component(Box::new(coil))
                        }
                        "gas" | "Gas" | "furnace" | "Furnace" => {
                            let coil = HeatingCoil::gas(
                                &c.name,
                                cap,
                                c.setpoint,
                                c.efficiency,
                            );
                            graph.add_air_component(Box::new(coil))
                        }
                        _ => {
                            let coil = HeatingCoil::electric(&c.name, cap, c.setpoint);
                            graph.add_air_component(Box::new(coil))
                        }
                    }
                }
                EquipmentInput::CoolingCoil(c) => {
                    match c.source.as_str() {
                        "chilled_water" | "ChilledWater" | "chw" => {
                            // Chilled water cooling coil (connected to CHW plant loop)
                            let water_flow = if c.design_water_flow_rate > 0.0 {
                                c.design_water_flow_rate
                            } else {
                                // Default: derive from capacity (2.4 GPM/ton ≈ 0.000151 m³/s per 3517W)
                                let cap = c.capacity.to_f64();
                                if cap > 0.0 { cap / 3517.0 * 0.000151 } else { 0.001 }
                            };
                            let coil = CoolingCoilCHW::new(
                                &c.name,
                                c.capacity.to_f64(),
                                c.shr,
                                c.setpoint,
                                water_flow,
                                c.design_water_inlet_temp,
                                c.design_water_outlet_temp,
                            );
                            graph.add_air_component(Box::new(coil))
                        }
                        _ => {
                            // DX cooling coil (default)
                            let cap_curve = c.cap_ft_curve.as_ref().and_then(|name| {
                                model.performance_curves.iter().find(|pc| pc.name == *name).cloned()
                            });
                            let eir_curve = c.eir_ft_curve.as_ref().and_then(|name| {
                                model.performance_curves.iter().find(|pc| pc.name == *name).cloned()
                            });
                            let mut coil = CoolingCoilDX::new(
                                &c.name,
                                c.capacity.to_f64(),
                                c.cop,
                                c.shr,
                                c.rated_airflow.to_f64(),
                                c.setpoint,
                            ).with_curves(cap_curve, eir_curve);
                            if let Some(plf) = c.plf_curve.as_ref().and_then(|name| {
                                model.performance_curves.iter().find(|pc| pc.name == *name).cloned()
                            }) {
                                coil = coil.with_plf_curve(plf);
                            }
                            graph.add_air_component(Box::new(coil))
                        }
                    }
                }
                EquipmentInput::HeatRecovery(hr) => {
                    let erv = match hr.source.as_str() {
                        "plate" | "plate_hx" => HeatRecovery::plate_hx(
                            &hr.name,
                            hr.sensible_effectiveness,
                            hr.parasitic_power,
                        ),
                        _ => HeatRecovery::enthalpy_wheel(
                            &hr.name,
                            hr.sensible_effectiveness,
                            hr.latent_effectiveness,
                            hr.parasitic_power,
                        ),
                    };
                    graph.add_air_component(Box::new(erv))
                }
                EquipmentInput::Humidifier(h) => {
                    let hum = openbse_components::humidifier::Humidifier::new(
                        &h.name,
                        h.rated_power,
                        h.min_rh_setpoint,
                        h.zone_cooling_setpoint,
                    );
                    graph.add_air_component(Box::new(hum))
                }
                EquipmentInput::Duct(d) => {
                    let duct = openbse_components::duct::Duct::new(
                        &d.name,
                        d.length,
                        d.diameter,
                        d.u_value,
                        d.leakage_fraction,
                        &d.ambient_zone,
                    );
                    graph.add_air_component(Box::new(duct))
                }
            };

            // Connect to previous component in sequence
            if let Some(prev) = prev_node {
                graph.connect_air(prev, node);
            }
            prev_node = Some(node);
        }

        // Build terminal boxes from zone connections.
        // Terminal boxes are air components that sit between the AHU supply duct
        // and each zone. They are connected after the last supply-side component.
        for zc in &air_loop.zone_terminals {
            if let Some(ref terminal) = zc.terminal {
                let terminal_node = match terminal {
                    TerminalInput::VavBox(vb) => {
                        let reheat = match vb.reheat_type.as_str() {
                            "electric" | "Electric" => ReheatType::Electric,
                            "hot_water" | "HotWater" | "hw" => ReheatType::HotWater,
                            _ => ReheatType::None,
                        };
                        let box_component = VAVBox::new(
                            &vb.name,
                            &zc.zone,
                            vb.max_air_flow.to_f64(),
                            vb.min_flow_fraction,
                            reheat,
                            vb.reheat_capacity.to_f64(),
                        );
                        graph.add_air_component(Box::new(box_component))
                    }
                    TerminalInput::PfpBox(pb) => {
                        let box_component = PFPBox::new(
                            &pb.name,
                            &zc.zone,
                            pb.max_primary_flow.to_f64(),
                            pb.min_primary_fraction,
                            pb.secondary_fan_flow.to_f64(),
                            pb.reheat_capacity.to_f64(),
                        );
                        graph.add_air_component(Box::new(box_component))
                    }
                };

                // Connect terminal to the last supply-side component
                if let Some(prev) = prev_node {
                    graph.connect_air(prev, terminal_node);
                }
            }
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
                    let is_water_cooled = ci.condenser_type == "water_cooled";
                    let mut chiller = AirCooledChiller::new(
                        &ci.name,
                        capacity,
                        ci.cop,
                        ci.chw_setpoint,
                        chw_flow,
                    );
                    chiller.min_plr = ci.min_plr;
                    chiller.water_cooled = is_water_cooled;
                    chiller.condenser_entering_temp = ci.condenser_entering_temp;
                    chiller.tower_approach = ci.tower_approach;
                    // Convert CurveInput → PerformanceCurve for CAPFT
                    if let Some(ref c) = ci.capft {
                        use openbse_components::performance_curve::{PerformanceCurve, CurveType};
                        let ct = if c.coefficients.len() >= 6 { CurveType::Biquadratic } else { CurveType::Quadratic };
                        chiller.capft_curve = Some(PerformanceCurve {
                            name: format!("{}_capft", ci.name),
                            curve_type: ct,
                            coefficients: c.coefficients.clone(),
                            min_x: c.min_x, max_x: c.max_x,
                            min_y: c.min_y, max_y: c.max_y,
                            min_output: None, max_output: None,
                        });
                    }
                    // Convert CurveInput → PerformanceCurve for EIRFT
                    if let Some(ref c) = ci.eirft {
                        use openbse_components::performance_curve::{PerformanceCurve, CurveType};
                        let ct = if c.coefficients.len() >= 6 { CurveType::Biquadratic } else { CurveType::Quadratic };
                        chiller.eirft_curve = Some(PerformanceCurve {
                            name: format!("{}_eirft", ci.name),
                            curve_type: ct,
                            coefficients: c.coefficients.clone(),
                            min_x: c.min_x, max_x: c.max_x,
                            min_y: c.min_y, max_y: c.max_y,
                            min_output: None, max_output: None,
                        });
                    }
                    // Convert CurveInput → PerformanceCurve for EIRFPLR
                    if let Some(ref c) = ci.eirfplr {
                        use openbse_components::performance_curve::{PerformanceCurve, CurveType};
                        chiller.eirfplr_curve = Some(PerformanceCurve {
                            name: format!("{}_eirfplr", ci.name),
                            curve_type: CurveType::Quadratic,
                            coefficients: c.coefficients.clone(),
                            min_x: c.min_x, max_x: c.max_x,
                            min_y: 0.0, max_y: 0.0,
                            min_output: None, max_output: None,
                        });
                    }
                    graph.add_plant_component(Box::new(chiller))
                }
                PlantEquipmentInput::Pump(p) => {
                    let pump_type = match p.pump_type.as_str() {
                        "constant_speed" => openbse_components::pump::PumpType::ConstantSpeed,
                        _ => openbse_components::pump::PumpType::VariableSpeed,
                    };
                    // Convert optional Vec<f64> power curve to [f64; 4]
                    let power_curve = p.power_curve.as_ref().and_then(|v| {
                        if v.len() >= 4 {
                            Some([v[0], v[1], v[2], v[3]])
                        } else {
                            None
                        }
                    });
                    let mut pump = openbse_components::pump::Pump::new_headered(
                        &p.name,
                        pump_type,
                        p.design_flow_rate.to_f64(),
                        p.design_head,
                        p.motor_efficiency,
                        p.impeller_efficiency,
                        p.num_pumps,
                        power_curve,
                    );
                    pump.motor_heat_to_fluid_fraction = p.motor_heat_to_fluid_fraction;
                    graph.add_plant_component(Box::new(pump))
                }
                PlantEquipmentInput::CoolingTower(ct) => {
                    use openbse_components::cooling_tower::{CoolingTower, CoolingTowerType};
                    let tower_type = match ct.tower_type.as_str() {
                        "single_speed" => CoolingTowerType::SingleSpeed,
                        "two_speed" => CoolingTowerType::TwoSpeed,
                        _ => CoolingTowerType::VariableSpeed,
                    };
                    let mut tower = CoolingTower::new(
                        &ct.name,
                        tower_type,
                        ct.design_water_flow.to_f64(),
                        ct.design_air_flow,
                        ct.design_fan_power,
                        ct.design_inlet_water_temp,
                        ct.design_approach,
                        ct.design_range,
                    );
                    // Apply custom fan power curve if specified
                    if let Some(ref curve) = ct.fan_power_curve {
                        if curve.len() >= 5 {
                            tower.fan_power_curve = [curve[0], curve[1], curve[2], curve[3], curve[4]];
                        }
                    }
                    graph.add_plant_component(Box::new(tower))
                }
                PlantEquipmentInput::HeatExchanger(hx) => {
                    use openbse_components::heat_exchanger::{WaterToWaterHX, HXControlMode};
                    let control = match hx.control_mode.as_str() {
                        "economizer" => HXControlMode::Economizer,
                        _ => HXControlMode::AlwaysOn,
                    };
                    let hx_component = WaterToWaterHX::new(
                        &hx.name,
                        hx.effectiveness,
                        hx.design_flow_rate.to_f64(),
                        control,
                        hx.economizer_threshold,
                        &hx.source_loop,
                    );
                    graph.add_plant_component(Box::new(hx_component))
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
                al.zone_terminals.iter().any(|zc| tstat.zones.contains(&zc.zone))
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
        model.f_factor_constructions.clone(),
        zones,
        model.surfaces.clone(),
        latitude,
        longitude,
        time_zone,
        elevation,
        model.simulation.solar_distribution,
    );

    // Set up schedule manager from model schedules
    env.schedule_manager = openbse_envelope::ScheduleManager::from_inputs(model.schedules.clone());

    // Set shading calculation mode from simulation settings
    env.shading_calculation = model.simulation.shading_calculation;

    // Set site terrain for wind profile calculations
    env.terrain = model.simulation.terrain;
    log::info!("Site terrain: {:?} (wind exp={:.2}, BL height={:.0}m)",
        env.terrain, env.terrain.wind_exp(), env.terrain.wind_bl_height());

    // Set infiltration interaction mode
    env.infiltration_interaction = model.simulation.infiltration_interaction;
    if env.infiltration_interaction != openbse_envelope::InfiltrationInteraction::Basic {
        log::info!("Infiltration interaction: {:?}", env.infiltration_interaction);
    }

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
                    sensible_fraction: people.sensible_fraction,
                    radiant_fraction: people.radiant_fraction,
                    schedule: people.schedule.clone(),
                    sensible_gain_per_person: people.sensible_gain_per_person,
                    latent_gain_per_person: people.latent_gain_per_person,
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
                    lost_fraction: equip.lost_fraction,
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
                    constant_coefficient: infil.constant_coefficient,
                    temperature_coefficient: infil.temperature_coefficient,
                    wind_coefficient: infil.wind_coefficient,
                    wind_squared_coefficient: infil.wind_squared_coefficient,
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
                    tag: exhaust.tag.clone(),
                    flow_rate: exhaust.flow_rate,
                    pressure_rise: exhaust.pressure_rise,
                    total_efficiency: exhaust.total_efficiency,
                    motor_efficiency: exhaust.motor_efficiency,
                    motor_in_airstream_fraction: exhaust.motor_in_airstream_fraction,
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
                    absolute: oa.absolute,
                    ach: oa.ach,
                    oa_method: oa.oa_method.clone(),
                    exhaust_per_person: oa.exhaust_per_person,
                    exhaust_per_area: oa.exhaust_per_area,
                    exhaust_absolute: oa.exhaust_absolute,
                    exhaust_ach: oa.exhaust_ach,
                    exhaust_method: oa.exhaust_method.clone(),
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
    let served_zone_names: Vec<String> = air_loop.zone_terminals.iter()
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
                let volume = zone.volume;

                // Count people from resolved internal gains
                let people_count: f64 = zone.internal_gains.iter().map(|g| {
                    match g {
                        openbse_envelope::InternalGainInput::People { count, .. } => *count,
                        _ => 0.0,
                    }
                }).sum();

                let person_flow = oa.per_person * people_count;
                let area_flow = oa.per_area * floor_area;
                let abs_flow = oa.absolute;
                let ach_flow = oa.ach * volume / 3600.0;
                total_oa_flow += match oa.oa_method {
                    openbse_envelope::zone_loads::OaMethod::Sum => person_flow + area_flow + abs_flow + ach_flow,
                    openbse_envelope::zone_loads::OaMethod::Maximum => person_flow.max(area_flow).max(abs_flow).max(ach_flow),
                };
            }
        } else if let Some(zi) = zone_input {
            // Zone not in envelope — use raw model input
            if let Some(oa) = &zi.outdoor_air {
                has_oa_data = true;
                let floor_area = zi.floor_area;
                let volume = zi.volume;

                let people_count: f64 = zi.internal_gains.iter().map(|g| {
                    match g {
                        openbse_envelope::InternalGainInput::People { count, .. } => *count,
                        _ => 0.0,
                    }
                }).sum();

                let person_flow = oa.per_person * people_count;
                let area_flow = oa.per_area * floor_area;
                let abs_flow = oa.absolute;
                let ach_flow = oa.ach * volume / 3600.0;
                total_oa_flow += match oa.oa_method {
                    openbse_envelope::zone_loads::OaMethod::Sum => person_flow + area_flow + abs_flow + ach_flow,
                    openbse_envelope::zone_loads::OaMethod::Maximum => person_flow.max(area_flow).max(abs_flow).max(ach_flow),
                };
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

// ─── Model Validation ─────────────────────────────────────────────────────────
//
// Validates all cross-references in a parsed model: zone names, schedule names,
// construction names, etc.  Returns a list of diagnostics (warnings and errors)
// that should be written to an .err file.

/// Severity level for a diagnostic message.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DiagSeverity {
    /// Non-fatal issue (simulation continues but results may be wrong)
    Warning,
    /// Fatal issue (simulation should not proceed)
    Severe,
}

/// A single diagnostic message from model validation.
#[derive(Debug, Clone)]
pub struct DiagMessage {
    pub severity: DiagSeverity,
    pub message: String,
}

impl std::fmt::Display for DiagMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.severity {
            DiagSeverity::Warning => write!(f, "** Warning ** {}", self.message),
            DiagSeverity::Severe  => write!(f, "** Severe  ** {}", self.message),
        }
    }
}

/// Result of model validation.
pub struct ValidationResult {
    pub diagnostics: Vec<DiagMessage>,
}

impl ValidationResult {
    pub fn new() -> Self {
        Self { diagnostics: Vec::new() }
    }

    fn warn(&mut self, msg: String) {
        self.diagnostics.push(DiagMessage { severity: DiagSeverity::Warning, message: msg });
    }

    fn severe(&mut self, msg: String) {
        self.diagnostics.push(DiagMessage { severity: DiagSeverity::Severe, message: msg });
    }

    /// Number of severe errors.
    pub fn error_count(&self) -> usize {
        self.diagnostics.iter().filter(|d| d.severity == DiagSeverity::Severe).count()
    }

    /// Number of warnings.
    pub fn warning_count(&self) -> usize {
        self.diagnostics.iter().filter(|d| d.severity == DiagSeverity::Warning).count()
    }

    /// Format the full error file content (E+-style).
    pub fn to_err_file(&self) -> String {
        let mut lines = Vec::new();
        lines.push("OpenBSE Error File".to_string());
        lines.push(format!("Errors: {}, Warnings: {}", self.error_count(), self.warning_count()));
        lines.push(String::new());
        for diag in &self.diagnostics {
            lines.push(diag.to_string());
        }
        if self.error_count() > 0 {
            lines.push(format!(
                "**  Fatal  ** Simulation cancelled: {} severe error(s) found",
                self.error_count()
            ));
        } else {
            lines.push("** Summary ** Simulation can proceed".to_string());
        }
        lines.push(String::new());
        lines.join("\n")
    }
}

/// Validate all cross-references in a parsed model.
///
/// Checks that every zone name, zone group name, and schedule name referenced
/// by people, lights, equipment, infiltration, ventilation, exhaust fans,
/// outdoor air, ideal loads, thermostats, and air loop zone_terminals actually
/// exists in the model definitions.
///
/// Returns a `ValidationResult` containing all warnings and errors found.
pub fn validate_model(model: &ModelInput) -> ValidationResult {
    let mut result = ValidationResult::new();

    // ── Build lookup sets ──────────────────────────────────────────────────

    let zone_names: std::collections::HashSet<&str> = model.zones.iter()
        .map(|z| z.name.as_str())
        .collect();

    let zone_group_map: std::collections::HashMap<&str, &[String]> = model.zone_groups.iter()
        .map(|zg| (zg.name.as_str(), zg.zones.as_slice()))
        .collect();

    let schedule_names: std::collections::HashSet<&str> = model.schedules.iter()
        .map(|s| s.name.as_str())
        .collect();

    // Built-in schedules that are always available
    let builtin_schedules: std::collections::HashSet<&str> =
        ["always_on", "always_off"].iter().copied().collect();

    // ── Helper: validate a list of zone/zone-group references ──────────────

    let check_zone_refs = |refs: &[String], obj_type: &str, obj_name: &str, result: &mut ValidationResult| {
        for name in refs {
            if zone_names.contains(name.as_str()) {
                continue; // valid zone
            }
            if let Some(group_zones) = zone_group_map.get(name.as_str()) {
                // Valid zone group — check that all zones in the group exist
                for gz in *group_zones {
                    if !zone_names.contains(gz.as_str()) {
                        result.severe(format!(
                            "Zone '{}' in zone_group '{}' (referenced by {} '{}') not found in zones list",
                            gz, name, obj_type, obj_name
                        ));
                    }
                }
            } else {
                result.severe(format!(
                    "Zone '{}' referenced by {} '{}' not found in zones list (and not a zone_group)",
                    name, obj_type, obj_name
                ));
            }
        }
    };

    let check_schedule = |sched: &Option<String>, obj_type: &str, obj_name: &str, result: &mut ValidationResult| {
        if let Some(name) = sched {
            if !schedule_names.contains(name.as_str()) && !builtin_schedules.contains(name.as_str()) {
                result.warn(format!(
                    "Schedule '{}' referenced by {} '{}' not found (will default to always-on)",
                    name, obj_type, obj_name
                ));
            }
        }
    };

    // ── Validate people ────────────────────────────────────────────────────

    for p in &model.people {
        check_zone_refs(&p.zones, "People", &p.name, &mut result);
        check_schedule(&p.schedule, "People", &p.name, &mut result);
    }

    // ── Validate lights ────────────────────────────────────────────────────

    for l in &model.lights {
        check_zone_refs(&l.zones, "Lights", &l.name, &mut result);
        check_schedule(&l.schedule, "Lights", &l.name, &mut result);
    }

    // ── Validate equipment ─────────────────────────────────────────────────

    for e in &model.equipment {
        check_zone_refs(&e.zones, "Equipment", &e.name, &mut result);
        check_schedule(&e.schedule, "Equipment", &e.name, &mut result);
    }

    // ── Validate infiltration ──────────────────────────────────────────────

    for inf in &model.infiltration {
        check_zone_refs(&inf.zones, "Infiltration", &inf.name, &mut result);
        check_schedule(&inf.schedule, "Infiltration", &inf.name, &mut result);
    }

    // ── Validate ventilation ───────────────────────────────────────────────

    for v in &model.ventilation {
        check_zone_refs(&v.zones, "Ventilation", &v.name, &mut result);
    }

    // ── Validate exhaust fans ──────────────────────────────────────────────

    for ef in &model.exhaust_fans {
        check_zone_refs(&ef.zones, "ExhaustFan", &ef.name, &mut result);
        check_schedule(&ef.schedule, "ExhaustFan", &ef.name, &mut result);
    }

    // ── Validate outdoor air ───────────────────────────────────────────────

    for oa in &model.outdoor_air {
        check_zone_refs(&oa.zones, "OutdoorAir", &oa.name, &mut result);
    }

    // ── Validate ideal loads ───────────────────────────────────────────────

    for il in &model.ideal_loads {
        check_zone_refs(&il.zones, "IdealLoads", &il.name, &mut result);
    }

    // ── Validate thermostats ───────────────────────────────────────────────

    for t in &model.thermostats {
        check_zone_refs(&t.zones, "Thermostat", &t.name, &mut result);
    }

    // ── Validate air loop zone_terminals ────────────────────────────────────

    for al in &model.air_loops {
        for zt in &al.zone_terminals {
            if !zone_names.contains(zt.zone.as_str()) {
                result.severe(format!(
                    "Zone '{}' in AirLoop '{}' zone_terminal not found in zones list",
                    zt.zone, al.name
                ));
            }
        }
    }

    result
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
