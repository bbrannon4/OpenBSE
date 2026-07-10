//! Output writing for simulation results.
//!
//! Supports:
//! - **Custom CSV outputs**: User-defined output files with selectable variables
//!   and reporting frequencies (timestep, hourly, daily, monthly, runperiod).
//! - **Summary report**: Standard text report with monthly energy end-use
//!   breakdown and unmet hours analysis (similar to EnergyPlus HTML output).
//!
//! ## Variable Naming Convention
//!
//! Variables use a `category:variable` or `category:variable:name_filter` format:
//!
//! | Category    | Description                          | Example                            |
//! |-------------|--------------------------------------|------------------------------------|
//! | `zone`      | Zone air properties and loads        | `zone:temperature`                 |
//! | `surface`   | Surface temps and heat transfer      | `surface:inside_temperature`       |
//! | `component` | HVAC component outputs               | `component:outlet_temperature`     |
//! | `site`      | Outdoor/weather conditions           | `site:outdoor_temperature`         |
//! | `building`  | Building-level energy end uses       | `building:fan_electric`            |
//! | `submeter`  | Sub-meter energy tracking            | `submeter:total_electric`          |
//!
//! The optional third part is a name filter supporting `*` glob:
//! - `zone:temperature` — all zones
//! - `zone:temperature:living_unit1` — specific zone
//! - `surface:transmitted_solar:Window*` — all surfaces starting with "Window"

use openbse_core::ports::ComponentKind;
use openbse_core::simulation::TimestepResult;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Write;
use std::path::Path;

// ─── Output Configuration ────────────────────────────────────────────────────

/// Reporting frequency for output files.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum OutputFrequency {
    Timestep,
    #[default]
    Hourly,
    Daily,
    Monthly,
    RunPeriod,
}

/// Aggregation method when downsampling from timestep to lower frequencies.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum Aggregation {
    #[default]
    Mean,
    Sum,
    Min,
    Max,
}

/// User-defined output file configuration.
///
/// ```yaml
/// outputs:
///   - file: "zone_results.csv"
///     frequency: hourly
///     variables:
///       - zone:temperature
///       - zone:heating_rate
///       - zone:cooling_rate
///       - zone:temperature:living_unit1    # specific zone
///       - surface:transmitted_solar:Window*  # glob filter
///       - building:fan_electric
///       - component:outlet_temperature
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputFileConfig {
    /// Output file name (relative to simulation directory)
    pub file: String,
    /// Reporting frequency
    #[serde(default)]
    pub frequency: OutputFrequency,
    /// Aggregation method for downsampled data
    #[serde(default)]
    pub aggregation: Aggregation,
    /// List of variable names to include
    pub variables: Vec<String>,
}

// ─── Variable Registry ──────────────────────────────────────────────────────

/// All available output variables with their units.
///
/// Returns (spec, unit_string, description) where spec is `category:variable`.
pub fn available_variables() -> Vec<(&'static str, &'static str, &'static str)> {
    vec![
        // Zone variables
        ("zone:temperature", "°C", "Zone air dry-bulb temperature"),
        ("zone:humidity_ratio", "kg/kg", "Zone air humidity ratio"),
        (
            "zone:heating_rate",
            "W",
            "Zone heating load (positive = needs heating)",
        ),
        (
            "zone:cooling_rate",
            "W",
            "Zone cooling load (positive = needs cooling)",
        ),
        (
            "zone:heating_energy",
            "J",
            "Zone heating energy (integrated from rate)",
        ),
        (
            "zone:cooling_energy",
            "J",
            "Zone cooling energy (integrated from rate)",
        ),
        (
            "zone:infiltration_mass_flow",
            "kg/s",
            "Zone infiltration air mass flow rate",
        ),
        (
            "zone:pressure",
            "Pa",
            "Zone gauge pressure from the airflow network",
        ),
        (
            "zone:species_<name>",
            "kg/kg",
            "Zone species concentration (e.g. zone:species_co2)",
        ),
        (
            "zone:nat_vent_flow",
            "m³/s",
            "Zone natural ventilation volume flow rate",
        ),
        (
            "zone:nat_vent_mass_flow",
            "kg/s",
            "Zone natural ventilation mass flow rate",
        ),
        (
            "zone:nat_vent_active",
            "-",
            "Zone natural ventilation active (1=yes, 0=no)",
        ),
        (
            "zone:internal_gains_convective",
            "W",
            "Zone convective internal gains",
        ),
        (
            "zone:internal_gains_radiative",
            "W",
            "Zone radiative internal gains",
        ),
        (
            "zone:supply_air_temperature",
            "°C",
            "HVAC supply air temperature to zone",
        ),
        (
            "zone:supply_air_mass_flow",
            "kg/s",
            "HVAC supply air mass flow to zone",
        ),
        // Zone gain breakdown variables (all W, positive = heat into zone)
        (
            "zone:gain_people_sensible",
            "W",
            "Occupant sensible heat gain to zone",
        ),
        ("zone:gain_people_latent", "W", "Occupant latent heat gain"),
        ("zone:gain_lighting", "W", "Lighting heat gain to zone"),
        (
            "zone:gain_equipment_sensible",
            "W",
            "Equipment sensible heat gain to zone",
        ),
        (
            "zone:gain_equipment_latent",
            "W",
            "Equipment latent heat gain",
        ),
        (
            "zone:gain_infiltration_sensible",
            "W",
            "Infiltration sensible heat gain",
        ),
        (
            "zone:gain_infiltration_latent",
            "W",
            "Infiltration latent heat gain",
        ),
        (
            "zone:gain_ventilation_sensible",
            "W",
            "Mechanical ventilation sensible heat gain",
        ),
        (
            "zone:gain_ventilation_latent",
            "W",
            "Mechanical ventilation latent heat gain",
        ),
        (
            "zone:gain_natural_ventilation_sensible",
            "W",
            "Natural ventilation sensible heat gain",
        ),
        (
            "zone:gain_natural_ventilation_latent",
            "W",
            "Natural ventilation latent heat gain",
        ),
        (
            "zone:gain_solar",
            "W",
            "Total solar gain to zone through windows",
        ),
        (
            "zone:gain_hvac_sensible",
            "W",
            "HVAC supply air sensible heat gain",
        ),
        (
            "zone:gain_hvac_latent",
            "W",
            "HVAC supply air latent heat gain",
        ),
        // Zone air balance diagnostic variables (all W)
        (
            "zone:q_surf_conv_total",
            "W",
            "Total surface convection to zone air",
        ),
        (
            "zone:q_surf_conv_walls",
            "W",
            "Wall surface convection to zone air",
        ),
        (
            "zone:q_surf_conv_floors",
            "W",
            "Floor surface convection to zone air",
        ),
        (
            "zone:q_surf_conv_roofs",
            "W",
            "Roof/ceiling surface convection to zone air",
        ),
        (
            "zone:q_surf_conv_windows",
            "W",
            "Window convection to zone air (incl absorbed solar inward)",
        ),
        (
            "zone:q_infiltration_sensible",
            "W",
            "Infiltration sensible heat transfer to zone air",
        ),
        (
            "zone:q_thermal_mass",
            "W",
            "Thermal mass (storage) term in zone air balance",
        ),
        // Surface convection to zone
        (
            "surface:conv_to_zone",
            "W",
            "Surface convection to zone air: h_conv × A × (T_surf − T_zone)",
        ),
        (
            "surface:h_conv_inside",
            "W/(m²·K)",
            "Inside face convection coefficient",
        ),
        // Zone comfort variables
        (
            "zone:mean_radiant_temperature",
            "°C",
            "Area-weighted mean radiant temperature",
        ),
        (
            "zone:operative_temperature",
            "°C",
            "Operative temperature (avg of air and MRT)",
        ),
        // Zone unmet hours time-series
        (
            "zone:unmet_heating",
            "-",
            "Unmet heating flag (1=unmet, 0=met)",
        ),
        (
            "zone:unmet_cooling",
            "-",
            "Unmet cooling flag (1=unmet, 0=met)",
        ),
        // Surface variables
        (
            "surface:inside_temperature",
            "°C",
            "Surface inside face temperature",
        ),
        (
            "surface:outside_temperature",
            "°C",
            "Surface outside face temperature",
        ),
        (
            "surface:inside_convection_coefficient",
            "W/(m²·K)",
            "Inside convection coefficient",
        ),
        (
            "surface:incident_solar",
            "W/m²",
            "Incident solar radiation on surface",
        ),
        (
            "surface:transmitted_solar",
            "W",
            "Solar transmitted through window",
        ),
        (
            "surface:cond_inside",
            "W",
            "Conduction heat flux on inside face of surface",
        ),
        (
            "surface:convection_inside",
            "W",
            "Convective heat flux from inside surface to zone",
        ),
        (
            "surface:radiation_inside",
            "W",
            "Net longwave radiation on inside face (positive = into surface)",
        ),
        (
            "surface:inside_radiation_coefficient",
            "W/(m²·K)",
            "Inside linearized radiation coefficient",
        ),
        // Site/weather variables
        (
            "site:outdoor_temperature",
            "°C",
            "Outdoor dry-bulb temperature",
        ),
        ("site:wind_speed", "m/s", "Wind speed"),
        (
            "site:direct_normal_radiation",
            "W/m²",
            "Direct normal solar radiation",
        ),
        (
            "site:diffuse_horizontal_radiation",
            "W/m²",
            "Diffuse horizontal solar radiation",
        ),
        ("site:relative_humidity", "%", "Outdoor relative humidity"),
        // Component / HVAC variables
        (
            "component:outlet_temperature",
            "°C",
            "Component outlet temperature",
        ),
        (
            "component:mass_flow",
            "kg/s",
            "Component air/fluid mass flow rate",
        ),
        (
            "component:outlet_humidity_ratio",
            "kg/kg",
            "Component outlet humidity ratio",
        ),
        (
            "component:cop_operating",
            "-",
            "Component coefficient of performance",
        ),
        ("component:plr", "-", "Component part-load ratio"),
        ("component:rtf", "-", "Component runtime fraction"),
        (
            "component:electric_power",
            "W",
            "Component electric power consumption",
        ),
        (
            "component:fuel_power",
            "W",
            "Component fuel power consumption",
        ),
        ("component:thermal_output", "W", "Component thermal output"),
        (
            "component:inlet_temperature",
            "°C",
            "Component inlet temperature",
        ),
        // Building energy end-use variables
        (
            "building:fan_electric",
            "W",
            "Fan electric power (all fans)",
        ),
        (
            "building:cooling_electric",
            "W",
            "Cooling electric power (DX, chiller)",
        ),
        ("building:heating_electric", "W", "Heating electric power"),
        ("building:heating_gas", "W", "Heating gas/fuel power"),
        ("building:pump_electric", "W", "Pump electric power"),
        (
            "building:heat_rejection",
            "W",
            "Heat rejection electric power",
        ),
        (
            "building:humidification",
            "W",
            "Humidification electric power",
        ),
        (
            "building:heat_recovery",
            "W",
            "Heat recovery electric power",
        ),
        ("building:dhw_electric", "W", "DHW electric power"),
        ("building:dhw_gas", "W", "DHW gas/fuel power"),
        ("building:lighting", "W", "Interior lighting power"),
        ("building:ext_lighting", "W", "Exterior lighting power"),
        ("building:equipment", "W", "Interior equipment power"),
        ("building:ext_equipment", "W", "Exterior equipment power"),
        (
            "building:total_electric",
            "W",
            "Total electric power (all end uses)",
        ),
        (
            "building:total_gas",
            "W",
            "Total gas/fuel power (all end uses)",
        ),
        // Submeter variables
        (
            "submeter:total_electric",
            "W",
            "Submeter total electric power",
        ),
        ("submeter:total_gas", "W", "Submeter total gas power"),
        ("submeter:total", "W", "Submeter total power (all fuels)"),
        ("submeter:lighting", "W", "Submeter lighting power"),
        ("submeter:equipment", "W", "Submeter equipment power"),
        ("submeter:heating_gas", "W", "Submeter heating gas power"),
        (
            "submeter:cooling_electric",
            "W",
            "Submeter cooling electric power",
        ),
        ("submeter:fan_electric", "W", "Submeter fan electric power"),
        ("submeter:dhw_gas", "W", "Submeter DHW gas power"),
    ]
}

/// Get the unit string for a variable spec (`category:variable` or `category:variable:filter`).
pub fn get_unit(spec: &str) -> &'static str {
    let mut parts = spec.splitn(3, ':');
    let category = parts.next().unwrap_or("");
    let variable = parts.next().unwrap_or("");
    match (category, variable) {
        (
            "zone",
            "temperature"
            | "supply_air_temperature"
            | "mean_radiant_temperature"
            | "operative_temperature",
        ) => "°C",
        ("zone", "humidity_ratio") => "kg/kg",
        ("zone", "pressure") => "Pa",
        ("zone", v) if v.starts_with("species_") => "kg/kg",
        ("zone", "nat_vent_flow") => "m³/s",
        ("zone", "infiltration_mass_flow" | "supply_air_mass_flow" | "nat_vent_mass_flow") => {
            "kg/s"
        }
        ("zone", "heating_energy" | "cooling_energy") => "J",
        ("zone", "nat_vent_active" | "unmet_heating" | "unmet_cooling") => "-",
        ("zone", _) => "W",
        ("surface", "inside_temperature" | "outside_temperature") => "°C",
        (
            "surface",
            "inside_convection_coefficient" | "h_conv_inside" | "inside_radiation_coefficient",
        ) => "W/(m²·K)",
        ("surface", "incident_solar") => "W/m²",
        ("surface", _) => "W",
        ("site", "outdoor_temperature") => "°C",
        ("site", "wind_speed") => "m/s",
        ("site", "relative_humidity") => "%",
        ("site", _) => "W/m²",
        ("building" | "submeter", _) => "W",
        (
            "component",
            "outlet_temperature"
            | "inlet_temperature"
            | "water_inlet_temperature"
            | "water_outlet_temperature"
            | "source_inlet_temperature",
        ) => "°C",
        ("component", "outlet_humidity_ratio" | "inlet_humidity_ratio") => "kg/kg",
        ("component", "mass_flow" | "water_mass_flow" | "source_mass_flow") => "kg/s",
        ("component", "inlet_enthalpy" | "outlet_enthalpy") => "J/kg",
        ("component", "pressure_rise") => "Pa",
        (
            "component",
            "cop_operating"
            | "plr"
            | "rtf"
            | "total_efficiency"
            | "efficiency_operating"
            | "effectiveness",
        ) => "-",
        ("component", _) => "W",
        _ => "-",
    }
}

/// Whether a variable should default to sum aggregation (energy, power).
fn is_integrable(spec: &str) -> bool {
    let mut parts = spec.splitn(3, ':');
    let category = parts.next().unwrap_or("");
    let variable = parts.next().unwrap_or("");
    matches!(
        (category, variable),
        ("zone", "heating_energy" | "cooling_energy")
            | ("building" | "submeter", _)
            | (
                "component",
                "electric_power"
                    | "fuel_power"
                    | "thermal_output"
                    | "sensible_load"
                    | "latent_load"
                    | "total_load"
                    | "conduction_loss"
                    | "leakage_loss"
                    | "cycling_loss"
                    | "conduction_gain"
                    | "heat_rejected"
                    | "heat_transferred"
                    | "fan_power",
            )
    )
}

// ─── Timestep Data Collector ────────────────────────────────────────────────

/// Electric end-use bucket for a generic HVAC component (fan, coil, chiller…).
///
/// Used to categorize `component_electric_power` entries without dropping or
/// double-counting odd-named components. Pumps, towers, humidifiers, heat
/// recovery and DHW are routed to typed snapshot maps upstream and never reach
/// this classifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElecEndUse {
    Fan,
    Cooling,
    Heating,
    Other,
}

/// Classify a generic electric HVAC component into a building end use.
///
/// Dispatches on the component `kind` first (robust to naming), falling back to
/// substring matching on `name` only when the kind is unknown — e.g. for the
/// kind-less snapshots constructed directly in unit tests. The `hw && !chw`
/// guard in the fallback keeps chilled-water ("CHW") coils from reading as
/// heating just because the name contains "hw".
pub fn elec_end_use(kind: Option<ComponentKind>, name: &str) -> ElecEndUse {
    match kind {
        Some(ComponentKind::Fan) => return ElecEndUse::Fan,
        Some(
            ComponentKind::CoolingCoil
            | ComponentKind::EvapCooler
            | ComponentKind::Chiller
            | ComponentKind::Crac
            | ComponentKind::Crah
            | ComponentKind::VrfIndoor
            | ComponentKind::VrfOutdoor
            | ComponentKind::Gshp
            | ComponentKind::ThermalStorage,
        ) => return ElecEndUse::Cooling,
        Some(ComponentKind::HeatingCoil) => return ElecEndUse::Heating,
        // Other kinds (Boiler is gas; Duct/HeatExchanger/RadiantPanel/etc. carry
        // no electric power) fall through to the name heuristic below, which lets
        // any unexpected electric draw still land somewhere countable.
        Some(_) | None => {}
    }

    let l = name.to_lowercase();
    if l.contains("fan") {
        ElecEndUse::Fan
    } else if l.contains("cool")
        || l.contains("dx")
        || l.contains("chiller")
        || l.starts_with("cc ")
        || l.starts_with("cc_")
    {
        ElecEndUse::Cooling
    } else if l.contains("heat")
        || l.contains("furnace")
        || l.starts_with("hc ")
        || l.starts_with("hc_")
        // "hw" (hot-water coil) but not "chw" (chilled-water coil)
        || (l.contains("hw") && !l.contains("chw"))
    {
        ElecEndUse::Heating
    } else {
        ElecEndUse::Other
    }
}

/// Snapshot of all simulation state at a single timestep.
///
/// This is the intermediate data that flows from the simulation loop
/// to the output writers. It contains all variables that any output
/// file might request.
#[derive(Debug, Clone)]
pub struct OutputSnapshot {
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub sub_hour: u32,
    pub dt: f64,

    // Site/weather
    pub site_outdoor_temperature: f64,
    pub site_wind_speed: f64,
    pub site_direct_normal_radiation: f64,
    pub site_diffuse_horizontal_radiation: f64,
    pub site_relative_humidity: f64,

    // Per-zone data (zone_name -> value)
    pub zone_temperature: HashMap<String, f64>,
    /// Zone AFN gauge pressure [Pa] (#89); empty when the AFN is off.
    pub zone_pressure: HashMap<String, f64>,
    /// Zone species concentrations [kg/kg] keyed by species name (#89).
    pub zone_species: HashMap<String, HashMap<String, f64>>,
    pub zone_humidity_ratio: HashMap<String, f64>,
    pub zone_heating_rate: HashMap<String, f64>,
    pub zone_cooling_rate: HashMap<String, f64>,
    pub zone_infiltration_mass_flow: HashMap<String, f64>,
    pub zone_nat_vent_flow: HashMap<String, f64>,
    pub zone_nat_vent_mass_flow: HashMap<String, f64>,
    pub zone_nat_vent_active: HashMap<String, f64>,
    pub zone_internal_gains_convective: HashMap<String, f64>,
    pub zone_internal_gains_radiative: HashMap<String, f64>,
    pub zone_supply_air_temperature: HashMap<String, f64>,
    pub zone_supply_air_mass_flow: HashMap<String, f64>,

    // Per-surface data (surface_name -> value)
    pub surface_inside_temperature: HashMap<String, f64>,
    pub surface_outside_temperature: HashMap<String, f64>,
    pub surface_inside_convection_coefficient: HashMap<String, f64>,
    pub surface_incident_solar: HashMap<String, f64>,
    pub surface_transmitted_solar: HashMap<String, f64>,
    pub surface_conduction_inside: HashMap<String, f64>,
    pub surface_convection_inside: HashMap<String, f64>,
    pub surface_radiation_inside: HashMap<String, f64>,
    pub surface_inside_radiation_coefficient: HashMap<String, f64>,

    // Per-zone active setpoints for this timestep (zone_name -> value)
    // Used by summary report for schedule-aware unmet hours
    pub zone_heating_setpoint: HashMap<String, f64>,
    pub zone_cooling_setpoint: HashMap<String, f64>,

    // Per-component HVAC data (component_name -> value)
    pub air_loop_outlet_temperature: HashMap<String, f64>,
    pub air_loop_mass_flow: HashMap<String, f64>,
    pub air_loop_outlet_humidity_ratio: HashMap<String, f64>,

    // Per-component energy end uses (component_name -> watts)
    pub component_electric_power: HashMap<String, f64>,
    pub component_fuel_power: HashMap<String, f64>,
    // Component kind (by name) for the entries in component_electric_power /
    // component_fuel_power. Lets end-use categorization dispatch on kind instead
    // of fragile substring matching on the component name. Not Serialize — only
    // used in-process for energy accounting.
    pub component_kinds: HashMap<String, ComponentKind>,
    // Internal gains by type (zone_name -> watts)
    pub zone_lighting_power: HashMap<String, f64>,
    pub zone_equipment_power: HashMap<String, f64>,

    // Per-zone gain breakdown (zone_name -> watts, positive = heat into zone)
    pub zone_gain_people_sensible: HashMap<String, f64>,
    pub zone_gain_people_latent: HashMap<String, f64>,
    pub zone_gain_lighting: HashMap<String, f64>,
    pub zone_gain_equipment_sensible: HashMap<String, f64>,
    pub zone_gain_equipment_latent: HashMap<String, f64>,
    pub zone_gain_infiltration_sensible: HashMap<String, f64>,
    pub zone_gain_infiltration_latent: HashMap<String, f64>,
    pub zone_gain_ventilation_sensible: HashMap<String, f64>,
    pub zone_gain_ventilation_latent: HashMap<String, f64>,
    pub zone_gain_natural_ventilation_sensible: HashMap<String, f64>,
    pub zone_gain_natural_ventilation_latent: HashMap<String, f64>,
    pub zone_gain_solar: HashMap<String, f64>,
    pub zone_gain_hvac_sensible: HashMap<String, f64>,
    pub zone_gain_hvac_latent: HashMap<String, f64>,

    // Zone comfort metrics
    pub zone_mean_radiant_temperature: HashMap<String, f64>,
    pub zone_operative_temperature: HashMap<String, f64>,

    // Zone unmet hours time-series (0.0 or 1.0)
    pub zone_unmet_heating: HashMap<String, f64>,
    pub zone_unmet_cooling: HashMap<String, f64>,

    // Per-submeter energy: submeter_name -> end_use_category -> watts
    // End-use categories: "fan_electric", "cooling_electric", "heating_electric",
    // "heating_gas", "pump_electric", "heat_rejection", "humidification",
    // "heat_recovery", "dhw_electric", "dhw_gas", "lighting", "ext_lighting",
    // "equipment", "ext_equipment"
    pub submeter_power: HashMap<String, HashMap<String, f64>>,

    // Per-component detailed outputs (component_name -> {field_name -> value})
    // Enables dynamic "ComponentName:field" output variable resolution.
    pub component_outputs: HashMap<String, HashMap<String, f64>>,

    // Typed end-use maps — separate from generic component_electric/fuel_power
    // so the summary report can categorize without fragile name-matching
    pub dhw_electric_power: HashMap<String, f64>,
    pub dhw_fuel_power: HashMap<String, f64>,
    pub ext_lighting_power: HashMap<String, f64>,
    pub ext_equipment_power: HashMap<String, f64>,
    pub pump_electric_power: HashMap<String, f64>,
    pub heat_rejection_power: HashMap<String, f64>,
    pub humidification_power: HashMap<String, f64>,
    pub heat_recovery_power: HashMap<String, f64>,

    // ─── Data Center ─────────────────────────────────────────────────────────
    /// IT equipment electric power per zone [W] (separate from general equipment)
    pub it_equipment_power: HashMap<String, f64>,
    /// Electrical distribution losses [W] (UPS + transformer losses, ASHRAE 90.4 ELC)
    pub elec_dist_power: f64,
}

impl OutputSnapshot {
    /// Create a snapshot with default/zero values.
    pub fn new(month: u32, day: u32, hour: u32, sub_hour: u32, dt: f64) -> Self {
        Self {
            month,
            day,
            hour,
            sub_hour,
            dt,
            site_outdoor_temperature: 0.0,
            site_wind_speed: 0.0,
            site_direct_normal_radiation: 0.0,
            site_diffuse_horizontal_radiation: 0.0,
            site_relative_humidity: 0.0,
            zone_temperature: HashMap::new(),
            zone_pressure: HashMap::new(),
            zone_species: HashMap::new(),
            zone_humidity_ratio: HashMap::new(),
            zone_heating_rate: HashMap::new(),
            zone_cooling_rate: HashMap::new(),
            zone_infiltration_mass_flow: HashMap::new(),
            zone_nat_vent_flow: HashMap::new(),
            zone_nat_vent_mass_flow: HashMap::new(),
            zone_nat_vent_active: HashMap::new(),
            zone_internal_gains_convective: HashMap::new(),
            zone_internal_gains_radiative: HashMap::new(),
            zone_supply_air_temperature: HashMap::new(),
            zone_supply_air_mass_flow: HashMap::new(),
            surface_inside_temperature: HashMap::new(),
            surface_outside_temperature: HashMap::new(),
            surface_inside_convection_coefficient: HashMap::new(),
            surface_incident_solar: HashMap::new(),
            surface_transmitted_solar: HashMap::new(),
            surface_conduction_inside: HashMap::new(),
            surface_convection_inside: HashMap::new(),
            surface_radiation_inside: HashMap::new(),
            surface_inside_radiation_coefficient: HashMap::new(),
            zone_heating_setpoint: HashMap::new(),
            zone_cooling_setpoint: HashMap::new(),
            air_loop_outlet_temperature: HashMap::new(),
            air_loop_mass_flow: HashMap::new(),
            air_loop_outlet_humidity_ratio: HashMap::new(),
            component_electric_power: HashMap::new(),
            component_fuel_power: HashMap::new(),
            component_kinds: HashMap::new(),
            zone_gain_people_sensible: HashMap::new(),
            zone_gain_people_latent: HashMap::new(),
            zone_gain_lighting: HashMap::new(),
            zone_gain_equipment_sensible: HashMap::new(),
            zone_gain_equipment_latent: HashMap::new(),
            zone_gain_infiltration_sensible: HashMap::new(),
            zone_gain_infiltration_latent: HashMap::new(),
            zone_gain_ventilation_sensible: HashMap::new(),
            zone_gain_ventilation_latent: HashMap::new(),
            zone_gain_natural_ventilation_sensible: HashMap::new(),
            zone_gain_natural_ventilation_latent: HashMap::new(),
            zone_gain_solar: HashMap::new(),
            zone_gain_hvac_sensible: HashMap::new(),
            zone_gain_hvac_latent: HashMap::new(),
            zone_mean_radiant_temperature: HashMap::new(),
            zone_operative_temperature: HashMap::new(),
            zone_unmet_heating: HashMap::new(),
            zone_unmet_cooling: HashMap::new(),
            submeter_power: HashMap::new(),
            component_outputs: HashMap::new(),
            zone_lighting_power: HashMap::new(),
            zone_equipment_power: HashMap::new(),
            dhw_electric_power: HashMap::new(),
            dhw_fuel_power: HashMap::new(),
            ext_lighting_power: HashMap::new(),
            ext_equipment_power: HashMap::new(),
            pump_electric_power: HashMap::new(),
            heat_rejection_power: HashMap::new(),
            humidification_power: HashMap::new(),
            heat_recovery_power: HashMap::new(),
            it_equipment_power: HashMap::new(),
            elec_dist_power: 0.0,
        }
    }

    /// Sum the generic electric-component power that classifies into `target`.
    ///
    /// Dispatches on each component's recorded [`ComponentKind`] (when present)
    /// and falls back to its name. Only `component_electric_power` entries are
    /// considered — pumps, towers, humidifiers, heat recovery and DHW live in
    /// their own typed maps and must not be double-counted here.
    fn component_elec_by_use(&self, target: ElecEndUse) -> f64 {
        self.component_electric_power
            .iter()
            .filter(|(name, _)| {
                elec_end_use(self.component_kinds.get(*name).copied(), name) == target
            })
            .map(|(_, &v)| v)
            .sum()
    }

    /// Get all entity→value pairs for a given category and variable name.
    fn get_category_values(&self, category: &str, variable: &str) -> HashMap<String, f64> {
        match category {
            "site" => match variable {
                "outdoor_temperature" => single("Site", self.site_outdoor_temperature),
                "wind_speed" => single("Site", self.site_wind_speed),
                "direct_normal_radiation" => single("Site", self.site_direct_normal_radiation),
                "diffuse_horizontal_radiation" => {
                    single("Site", self.site_diffuse_horizontal_radiation)
                }
                "relative_humidity" => single("Site", self.site_relative_humidity),
                _ => HashMap::new(),
            },
            "zone" => match variable {
                "temperature" => self.zone_temperature.clone(),
                "pressure" => self.zone_pressure.clone(),
                "humidity_ratio" => self.zone_humidity_ratio.clone(),
                "heating_rate" => self.zone_heating_rate.clone(),
                "cooling_rate" => self.zone_cooling_rate.clone(),
                "heating_energy" => self
                    .zone_heating_rate
                    .iter()
                    .map(|(k, v)| (k.clone(), v * self.dt))
                    .collect(),
                "cooling_energy" => self
                    .zone_cooling_rate
                    .iter()
                    .map(|(k, v)| (k.clone(), v * self.dt))
                    .collect(),
                "infiltration_mass_flow" => self.zone_infiltration_mass_flow.clone(),
                "nat_vent_flow" => self.zone_nat_vent_flow.clone(),
                "nat_vent_mass_flow" => self.zone_nat_vent_mass_flow.clone(),
                "nat_vent_active" => self.zone_nat_vent_active.clone(),
                "internal_gains_convective" => self.zone_internal_gains_convective.clone(),
                "internal_gains_radiative" => self.zone_internal_gains_radiative.clone(),
                "supply_air_temperature" => self.zone_supply_air_temperature.clone(),
                "supply_air_mass_flow" => self.zone_supply_air_mass_flow.clone(),
                "gain_people_sensible" => self.zone_gain_people_sensible.clone(),
                "gain_people_latent" => self.zone_gain_people_latent.clone(),
                "gain_lighting" => self.zone_gain_lighting.clone(),
                "gain_equipment_sensible" => self.zone_gain_equipment_sensible.clone(),
                "gain_equipment_latent" => self.zone_gain_equipment_latent.clone(),
                "gain_infiltration_sensible" => self.zone_gain_infiltration_sensible.clone(),
                "gain_infiltration_latent" => self.zone_gain_infiltration_latent.clone(),
                "gain_ventilation_sensible" => self.zone_gain_ventilation_sensible.clone(),
                "gain_ventilation_latent" => self.zone_gain_ventilation_latent.clone(),
                "gain_natural_ventilation_sensible" => {
                    self.zone_gain_natural_ventilation_sensible.clone()
                }
                "gain_natural_ventilation_latent" => {
                    self.zone_gain_natural_ventilation_latent.clone()
                }
                "gain_solar" => self.zone_gain_solar.clone(),
                "gain_hvac_sensible" => self.zone_gain_hvac_sensible.clone(),
                "gain_hvac_latent" => self.zone_gain_hvac_latent.clone(),
                "mean_radiant_temperature" => self.zone_mean_radiant_temperature.clone(),
                "operative_temperature" => self.zone_operative_temperature.clone(),
                "unmet_heating" => self.zone_unmet_heating.clone(),
                "unmet_cooling" => self.zone_unmet_cooling.clone(),
                other => {
                    // Dynamic species variables (#89): zone:species_<name>
                    if let Some(species) = other.strip_prefix("species_") {
                        self.zone_species.get(species).cloned().unwrap_or_default()
                    } else {
                        HashMap::new()
                    }
                }
            },
            "surface" => match variable {
                "inside_temperature" => self.surface_inside_temperature.clone(),
                "outside_temperature" => self.surface_outside_temperature.clone(),
                "inside_convection_coefficient" => {
                    self.surface_inside_convection_coefficient.clone()
                }
                "incident_solar" => self.surface_incident_solar.clone(),
                "transmitted_solar" => self.surface_transmitted_solar.clone(),
                "cond_inside" => self.surface_conduction_inside.clone(),
                "convection_inside" => self.surface_convection_inside.clone(),
                "radiation_inside" => self.surface_radiation_inside.clone(),
                "inside_radiation_coefficient" => self.surface_inside_radiation_coefficient.clone(),
                _ => HashMap::new(),
            },
            "building" => {
                let value = match variable {
                    "fan_electric" => self.component_elec_by_use(ElecEndUse::Fan),
                    "cooling_electric" => self.component_elec_by_use(ElecEndUse::Cooling),
                    "heating_electric" => self.component_elec_by_use(ElecEndUse::Heating),
                    "heating_gas" => self.component_fuel_power.values().sum(),
                    "pump_electric" => self.pump_electric_power.values().sum(),
                    "heat_rejection" => self.heat_rejection_power.values().sum(),
                    "humidification" => self.humidification_power.values().sum(),
                    "heat_recovery" => self.heat_recovery_power.values().sum(),
                    "dhw_electric" => self.dhw_electric_power.values().sum(),
                    "dhw_gas" => self.dhw_fuel_power.values().sum(),
                    "lighting" => self.zone_lighting_power.values().sum(),
                    "ext_lighting" => self.ext_lighting_power.values().sum(),
                    "equipment" => self.zone_equipment_power.values().sum(),
                    "ext_equipment" => self.ext_equipment_power.values().sum(),
                    "total_electric" => {
                        // Sum EVERY generic electric component exactly once, plus
                        // every typed electric map. Bulletproof against the old
                        // name-filter omission/double-count bug.
                        self.component_electric_power.values().sum::<f64>()
                            + self.pump_electric_power.values().sum::<f64>()
                            + self.heat_rejection_power.values().sum::<f64>()
                            + self.humidification_power.values().sum::<f64>()
                            + self.heat_recovery_power.values().sum::<f64>()
                            + self.dhw_electric_power.values().sum::<f64>()
                            + self.zone_lighting_power.values().sum::<f64>()
                            + self.ext_lighting_power.values().sum::<f64>()
                            + self.zone_equipment_power.values().sum::<f64>()
                            + self.ext_equipment_power.values().sum::<f64>()
                    }
                    "total_gas" => {
                        // All fuel-burning components are gas heating; plus DHW gas.
                        self.component_fuel_power.values().sum::<f64>()
                            + self.dhw_fuel_power.values().sum::<f64>()
                    }
                    _ => return HashMap::new(),
                };
                single("Building", value)
            }
            "submeter" => {
                // variable is the submeter name; the end_use was already stripped as part of
                // the spec — but for the new scheme the variable IS the end_use and the name
                // filter selects which meter(s).
                // Format: submeter:end_use  (name_filter selects meter)
                // or legacy: submeter:meter_name:end_use handled via get_variable_values filter.
                // Here we return meter_name → value for every submeter that has this end_use.
                let end_use = variable;
                let mut result = HashMap::new();
                for (meter_name, end_uses) in &self.submeter_power {
                    let val = match end_use {
                        "total_electric" => end_uses
                            .iter()
                            .filter(|(k, _)| !k.contains("gas"))
                            .map(|(_, &v)| v)
                            .sum(),
                        "total_gas" => end_uses
                            .iter()
                            .filter(|(k, _)| k.contains("gas"))
                            .map(|(_, &v)| v)
                            .sum(),
                        "total" => end_uses.values().sum(),
                        other => *end_uses.get(other).unwrap_or(&0.0),
                    };
                    result.insert(meter_name.clone(), val);
                }
                result
            }
            "component" => {
                // Returns component_name → value for all components that have this field.
                let field = variable;
                // Special mappings for air_loop hashmaps
                match field {
                    "outlet_temperature" => {
                        let mut result = self.air_loop_outlet_temperature.clone();
                        // Also search component_outputs
                        for (comp, vars) in &self.component_outputs {
                            if let Some(&v) = vars.get(field) {
                                result.entry(comp.clone()).or_insert(v);
                            }
                        }
                        result
                    }
                    "mass_flow" => {
                        let mut result = self.air_loop_mass_flow.clone();
                        for (comp, vars) in &self.component_outputs {
                            if let Some(&v) = vars.get(field) {
                                result.entry(comp.clone()).or_insert(v);
                            }
                        }
                        result
                    }
                    "outlet_humidity_ratio" => {
                        let mut result = self.air_loop_outlet_humidity_ratio.clone();
                        for (comp, vars) in &self.component_outputs {
                            if let Some(&v) = vars.get(field) {
                                result.entry(comp.clone()).or_insert(v);
                            }
                        }
                        result
                    }
                    other => {
                        let mut result = HashMap::new();
                        for (comp, vars) in &self.component_outputs {
                            if let Some(&v) = vars.get(other) {
                                result.insert(comp.clone(), v);
                            }
                        }
                        result
                    }
                }
            }
            "datacenter" => {
                let value = match variable {
                    "pue" => {
                        let it: f64 = self.it_equipment_power.values().sum();
                        let mech: f64 = self
                            .submeter_power
                            .get("datacenter")
                            .map(|eu| {
                                eu.iter()
                                    .filter(|(k, _)| k.as_str() != "it_equipment")
                                    .map(|(_, &v)| v)
                                    .sum::<f64>()
                            })
                            .unwrap_or(0.0);
                        if it > 0.0 {
                            (it + mech + self.elec_dist_power) / it
                        } else {
                            // IT load is zero (scheduled off): PUE is undefined this timestep.
                            // Return NaN so mean aggregation doesn't silently depress annual PUE.
                            // Use the annual ratio from SummaryReport for the true annual PUE.
                            f64::NAN
                        }
                    }
                    "it_load_kw" => self.it_equipment_power.values().sum::<f64>() / 1000.0,
                    "mechanical_load_kw" => {
                        self.submeter_power
                            .get("datacenter")
                            .map(|eu| {
                                eu.iter()
                                    .filter(|(k, _)| k.as_str() != "it_equipment")
                                    .map(|(_, &v)| v)
                                    .sum::<f64>()
                            })
                            .unwrap_or(0.0)
                            / 1000.0
                    }
                    "electrical_loss_kw" => self.elec_dist_power / 1000.0,
                    _ => return HashMap::new(),
                };
                single("Datacenter", value)
            }
            _ => HashMap::new(),
        }
    }

    /// Get all values for a variable spec (returns entity_name -> value pairs).
    ///
    /// Spec format: `category:variable` or `category:variable:name_filter`.
    /// For zone variables, returns one value per zone.
    /// For surface variables, returns one value per surface.
    /// For site variables, returns a single value with key "Site".
    fn get_variable_values(&self, spec: &str) -> HashMap<String, f64> {
        let mut parts = spec.splitn(3, ':');
        let category = parts.next().unwrap_or("");
        let variable = parts.next().unwrap_or("");
        let filter = parts.next();

        let raw = self.get_category_values(category, variable);

        match filter {
            None | Some("*") => raw,
            Some(pattern) => raw
                .into_iter()
                .filter(|(name, _)| glob_matches(pattern, name))
                .collect(),
        }
    }
}

fn single(key: &str, value: f64) -> HashMap<String, f64> {
    let mut m = HashMap::new();
    m.insert(key.to_string(), value);
    m
}

/// Match a glob pattern against a name. Only `*` wildcard is supported.
fn glob_matches(pattern: &str, text: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    match pattern.find('*') {
        None => pattern == text,
        Some(star_pos) => {
            let prefix = &pattern[..star_pos];
            let suffix = &pattern[star_pos + 1..];
            if !text.starts_with(prefix) {
                return false;
            }
            let rest = &text[prefix.len()..];
            if suffix.is_empty() {
                return true;
            }
            if !suffix.contains('*') {
                return rest.ends_with(suffix);
            }
            // Multiple wildcards: try each position
            for i in 0..=rest.len() {
                if glob_matches(suffix, &rest[i..]) {
                    return true;
                }
            }
            false
        }
    }
}

// ─── Output Writer ──────────────────────────────────────────────────────────

/// Manages buffering and writing of output data for one output file.
pub struct OutputWriter {
    config: OutputFileConfig,
    /// Column layout: (variable_name, entity_name) pairs
    columns: Vec<(String, String)>,
    /// Accumulator for aggregation: column_index -> (sum, count, min, max)
    accum: Vec<(f64, u32, f64, f64)>,
    /// Current aggregation period key (month, day, hour_key)
    current_period: Option<(u32, u32, u32)>,
    /// The month/day/hour/sub_hour from the first snapshot of the current period.
    /// Used to label the output row with the correct period, not the next period's values.
    period_label: Option<(u32, u32, u32, u32)>,
    /// Buffered rows ready to write
    rows: Vec<OutputRow>,
    /// Whether columns have been discovered
    columns_resolved: bool,
}

#[derive(Debug)]
struct OutputRow {
    month: u32,
    day: u32,
    hour: u32,
    sub_hour: u32,
    values: Vec<f64>,
}

impl OutputWriter {
    pub fn new(config: OutputFileConfig) -> Self {
        Self {
            config,
            columns: Vec::new(),
            period_label: None,
            accum: Vec::new(),
            current_period: None,
            rows: Vec::new(),
            columns_resolved: false,
        }
    }

    /// Discover columns from the first snapshot.
    ///
    /// Some variables (e.g. `zone:gain_solar`) may have empty HashMaps in
    /// the first timestep (midnight — no solar). To avoid permanently
    /// dropping these columns, we fall back to entity names from a reference
    /// variable in the same category (`zone:temperature` for zone variables,
    /// `surface:inside_temperature` for surface variables).
    fn resolve_columns(&mut self, snapshot: &OutputSnapshot) {
        if self.columns_resolved {
            return;
        }

        // Build reference entity lists per category (always populated).
        let zone_entities = {
            let mut v: Vec<String> = snapshot.zone_temperature.keys().cloned().collect();
            v.sort();
            v
        };
        let surface_entities = {
            let mut v: Vec<String> = snapshot
                .surface_inside_temperature
                .keys()
                .cloned()
                .collect();
            v.sort();
            v
        };

        for var_name in &self.config.variables {
            let values = snapshot.get_variable_values(var_name);
            let mut entity_names: Vec<String> = values.keys().cloned().collect();
            entity_names.sort();

            if entity_names.is_empty() {
                // Variable data is empty in this snapshot. Use the reference
                // entity list for the matching category so that the column is
                // created and will receive data in later timesteps.
                let category = var_name.split(':').next().unwrap_or("");
                let fallback = match category {
                    "zone" => &zone_entities,
                    "surface" => &surface_entities,
                    _ => continue, // site/building: truly missing, skip
                };
                // Apply name filter if present (e.g. "zone:gain_solar:living*")
                let filter = var_name.splitn(3, ':').nth(2);
                for entity in fallback {
                    let include = match filter {
                        None | Some("*") => true,
                        Some(pat) => glob_matches(pat, entity),
                    };
                    if include {
                        self.columns.push((var_name.clone(), entity.clone()));
                    }
                }
                continue;
            }

            for entity in entity_names {
                self.columns.push((var_name.clone(), entity));
            }
        }

        self.accum = vec![(0.0, 0, f64::MAX, f64::MIN); self.columns.len()];
        self.columns_resolved = true;
    }

    /// Determine the aggregation period key for a snapshot.
    fn period_key(&self, snap: &OutputSnapshot) -> (u32, u32, u32) {
        match self.config.frequency {
            OutputFrequency::Timestep => (snap.month, snap.day, snap.hour * 100 + snap.sub_hour),
            OutputFrequency::Hourly => (snap.month, snap.day, snap.hour),
            OutputFrequency::Daily => (snap.month, snap.day, 0),
            OutputFrequency::Monthly => (snap.month, 0, 0),
            OutputFrequency::RunPeriod => (0, 0, 0),
        }
    }

    /// Process one timestep snapshot.
    pub fn add_snapshot(&mut self, snapshot: &OutputSnapshot) {
        self.resolve_columns(snapshot);

        let period = self.period_key(snapshot);

        // Check if we've entered a new period -> flush the old one
        if let Some(prev_period) = self.current_period {
            if prev_period != period {
                // Flush using the PREVIOUS period's label, not the new snapshot's.
                // This fixes an off-by-one where hour N's data was labeled as hour N+1.
                let (pm, pd, ph, ps) = self.period_label.unwrap_or((
                    snapshot.month,
                    snapshot.day,
                    snapshot.hour,
                    snapshot.sub_hour,
                ));
                self.flush_period(pm, pd, ph, ps);
            }
        }
        self.current_period = Some(period);
        // Record label from the first snapshot of each new period
        if self.period_label.is_none() {
            self.period_label = Some((
                snapshot.month,
                snapshot.day,
                snapshot.hour,
                snapshot.sub_hour,
            ));
        }

        // Accumulate values
        for (i, (var_name, entity_name)) in self.columns.iter().enumerate() {
            let values = snapshot.get_variable_values(var_name);
            let val = values.get(entity_name).copied().unwrap_or(0.0);

            self.accum[i].0 += val; // sum
            self.accum[i].1 += 1; // count
            if val < self.accum[i].2 {
                self.accum[i].2 = val;
            } // min
            if val > self.accum[i].3 {
                self.accum[i].3 = val;
            } // max
        }

        // For timestep frequency, flush immediately
        if self.config.frequency == OutputFrequency::Timestep {
            self.flush_period(
                snapshot.month,
                snapshot.day,
                snapshot.hour,
                snapshot.sub_hour,
            );
        }
    }

    /// Flush accumulated data as one output row.
    fn flush_period(&mut self, month: u32, day: u32, hour: u32, sub_hour: u32) {
        if self.accum.is_empty() || self.accum[0].1 == 0 {
            return;
        }

        let mut values = Vec::with_capacity(self.columns.len());
        for (i, (var_name, _)) in self.columns.iter().enumerate() {
            let (sum, count, min, max) = self.accum[i];
            if count == 0 {
                values.push(0.0);
                continue;
            }

            // Choose aggregation: use variable-specific default for energy vars,
            // otherwise use the user's configured aggregation
            let agg = if is_integrable(var_name) {
                Aggregation::Sum
            } else {
                self.config.aggregation
            };

            let val = match agg {
                Aggregation::Mean => sum / count as f64,
                Aggregation::Sum => sum,
                Aggregation::Min => min,
                Aggregation::Max => max,
            };
            values.push(val);
        }

        self.rows.push(OutputRow {
            month,
            day,
            hour,
            sub_hour,
            values,
        });

        // Reset accumulators
        for acc in &mut self.accum {
            *acc = (0.0, 0, f64::MAX, f64::MIN);
        }
        self.current_period = None;
        self.period_label = None;
    }

    /// Finalize (flush any remaining data) and write to file.
    pub fn finalize_and_write(&mut self, output_dir: &Path) -> Result<(), OutputError> {
        // Flush any remaining accumulated data using the period's own label
        if self.current_period.is_some() {
            if let Some((pm, pd, ph, ps)) = self.period_label {
                self.flush_period(pm, pd, ph, ps);
            } else if let Some((m, d, h)) = self.current_period {
                self.flush_period(m, d, h, 0);
            }
        }

        if self.rows.is_empty() {
            return Ok(()); // No data to write
        }

        let path = output_dir.join(&self.config.file);
        self.write_to_path(&path)
    }

    /// Finalize and write, prepending `stem_` to the configured filename.
    /// E.g. config.file = "zone_output.csv" → "retail_rtu_zone_output.csv"
    pub fn finalize_and_write_prefixed(
        &mut self,
        output_dir: &Path,
        stem: &str,
    ) -> Result<(), OutputError> {
        if let Some((m, d, h)) = self.current_period {
            self.flush_period(m, d, h, 0);
        }
        if self.rows.is_empty() {
            return Ok(());
        }
        let prefixed_name = format!("{}_{}", stem, self.config.file);
        let path = output_dir.join(&prefixed_name);
        self.write_to_path(&path)
    }

    fn write_to_path(&self, path: &Path) -> Result<(), OutputError> {
        let file = std::fs::File::create(path)
            .map_err(|e| OutputError::IoError(format!("{}: {}", path.display(), e)))?;
        let mut writer = std::io::BufWriter::new(file);

        // Header
        write!(writer, "Month,Day,Hour")?;
        if self.config.frequency == OutputFrequency::Timestep {
            write!(writer, ",SubHour")?;
        }
        for (var_spec, entity_name) in &self.columns {
            let variable_part = var_spec.split(':').nth(1).unwrap_or(var_spec.as_str());
            let unit = get_unit(var_spec);
            write!(writer, ",{}:{} [{}]", entity_name, variable_part, unit)?;
        }
        writeln!(writer)?;

        // Data rows
        for row in &self.rows {
            write!(writer, "{},{},{}", row.month, row.day, row.hour)?;
            if self.config.frequency == OutputFrequency::Timestep {
                write!(writer, ",{}", row.sub_hour)?;
            }
            for val in &row.values {
                write!(writer, ",{:.4}", val)?;
            }
            writeln!(writer)?;
        }

        writer.flush()?;
        Ok(())
    }
}

// ─── Summary Report ─────────────────────────────────────────────────────────

/// Monthly energy data for the summary report.
/// Matches the 13 standard EnergyPlus end-use categories.
#[derive(Debug, Clone, Default)]
struct MonthlyEnergy {
    heating_j: f64, // Total zone heating loads [J]
    cooling_j: f64, // Total zone cooling loads [J]
    hours: f64,     // Number of hours in data
    // Electric end uses
    fan_elec_j: f64,            // Fan electric [J]
    cool_elec_j: f64,           // Cooling electric (DX compressor, chiller) [J]
    heat_elec_j: f64,           // Heating electric (electric coil, HP compressor) [J]
    pump_elec_j: f64,           // Pump electric [J]
    heat_rejection_elec_j: f64, // Cooling tower fan electric [J]
    humidification_elec_j: f64, // Humidifier electric [J]
    heat_recovery_elec_j: f64,  // Heat recovery electric (wheel motor, etc.) [J]
    dhw_elec_j: f64,            // DHW electric (water heater) [J]
    other_elec_j: f64,          // Uncategorized electric HVAC components [J]
    lighting_j: f64,            // Interior lighting [J]
    ext_lighting_j: f64,        // Exterior lighting [J]
    equipment_j: f64,           // Interior equipment/plug loads [J]
    ext_equipment_j: f64,       // Exterior equipment [J]
    // Gas end uses
    heat_gas_j: f64, // Heating gas (boiler, gas furnace) [J]
    dhw_gas_j: f64,  // DHW gas (gas water heater) [J]
    // Data center end uses
    it_equipment_j: f64, // IT server loads [J]
    elec_dist_j: f64,    // UPS + transformer losses [J]
}

/// Summary report generator — produces a standard text report with
/// monthly energy breakdown and unmet hours analysis.
pub struct SummaryReport {
    monthly: [MonthlyEnergy; 12],
    /// Unmet heating hours: zone temp < heating setpoint - tolerance
    unmet_heating_hours: f64,
    /// Unmet cooling hours: zone temp > cooling setpoint + tolerance
    unmet_cooling_hours: f64,
    /// Tolerance for unmet hours [deg C]
    unmet_tolerance: f64,
    /// Zone heating setpoints (zone_name -> setpoint)
    heating_setpoints: HashMap<String, f64>,
    /// Zone cooling setpoints (zone_name -> setpoint)
    cooling_setpoints: HashMap<String, f64>,
    /// Total timesteps processed
    total_timesteps: u64,
    /// Timestep duration [s]
    dt: f64,
    /// Peak heating rate [W] and when it occurred, with coincident conditions
    peak_heating: (f64, u32, u32, u32, f64, f64, f64), // (watts, month, day, hour, outdoor_temp, zone_temp, wind_speed)
    /// Peak cooling rate [W] and when it occurred, with coincident conditions
    peak_cooling: (f64, u32, u32, u32, f64, f64, f64),
    /// Total window transmitted solar energy [J] (for diagnostics)
    total_transmitted_solar_j: f64,
    /// Total window incident solar energy [J] (for diagnostics)
    total_incident_solar_j: f64,
    /// Monthly transmitted solar [J] (12 months)
    monthly_transmitted_solar_j: [f64; 12],
    /// Wall and window areas by cardinal direction for WWR reporting
    envelope_areas: Option<openbse_envelope::EnvelopeAreas>,
    /// Per-surface annual conduction energy [J] — CTF-based (opaque surfaces)
    surface_conduction_j: HashMap<String, f64>,
    /// Per-surface annual convection energy [J] — used for windows (q_conv_inside)
    surface_convection_j: HashMap<String, f64>,
    /// Surface metadata: (name, zone, type_str, area_m2, is_window, boundary_str)
    surface_meta: Vec<(String, String, String, f64, bool, String)>,
    /// Monthly inside surface temperature sums [°C·count] per surface
    monthly_surf_temp_inside: HashMap<String, [f64; 12]>,
    /// Monthly outside surface temperature sums [°C·count] per surface
    monthly_surf_temp_outside: HashMap<String, [f64; 12]>,
    /// Monthly incident solar sums [W/m²·count] per surface
    monthly_surf_incident_solar: HashMap<String, [f64; 12]>,
    /// Monthly timestep count per month
    monthly_surf_count: [u64; 12],
    /// Per-zone peak heating: zone_name -> (watts, month, day, hour, outdoor_temp)
    zone_peak_heating: HashMap<String, (f64, u32, u32, u32, f64)>,
    /// Per-zone peak cooling: zone_name -> (watts, month, day, hour, outdoor_temp)
    zone_peak_cooling: HashMap<String, (f64, u32, u32, u32, f64)>,
    /// Zone floor areas for W/m² calculations
    zone_floor_areas: HashMap<String, f64>,
    /// Per-submeter monthly energy [J]: (submeter, end_use) -> [12 months]
    submeter_monthly_j: HashMap<(String, String), [f64; 12]>,
    /// ASHRAE climate zone string (e.g., "5A") for 90.4 limit table lookup.
    climate_zone: Option<String>,
}

impl SummaryReport {
    pub fn new(
        heating_setpoints: HashMap<String, f64>,
        cooling_setpoints: HashMap<String, f64>,
    ) -> Self {
        Self {
            monthly: Default::default(),
            unmet_heating_hours: 0.0,
            unmet_cooling_hours: 0.0,
            unmet_tolerance: 0.2, // 0.2 deg C tolerance
            heating_setpoints,
            cooling_setpoints,
            total_timesteps: 0,
            dt: 3600.0,
            peak_heating: (0.0, 0, 0, 0, 0.0, 0.0, 0.0),
            peak_cooling: (0.0, 0, 0, 0, 0.0, 0.0, 0.0),
            total_transmitted_solar_j: 0.0,
            total_incident_solar_j: 0.0,
            monthly_transmitted_solar_j: [0.0; 12],
            envelope_areas: None,
            surface_conduction_j: HashMap::new(),
            surface_convection_j: HashMap::new(),
            surface_meta: Vec::new(),
            monthly_surf_temp_inside: HashMap::new(),
            monthly_surf_temp_outside: HashMap::new(),
            monthly_surf_incident_solar: HashMap::new(),
            monthly_surf_count: [0; 12],
            zone_peak_heating: HashMap::new(),
            zone_peak_cooling: HashMap::new(),
            zone_floor_areas: HashMap::new(),
            submeter_monthly_j: HashMap::new(),
            climate_zone: None,
        }
    }

    /// Set the ASHRAE climate zone for 90.4 PUE/MLC/ELC limit reporting.
    pub fn set_climate_zone(&mut self, cz: String) {
        self.climate_zone = Some(cz);
    }

    /// Set zone floor areas for W/m² calculations in zone loads summary.
    pub fn set_zone_areas(&mut self, areas: HashMap<String, f64>) {
        self.zone_floor_areas = areas;
    }

    /// Set envelope area data for WWR reporting.
    pub fn set_envelope_areas(&mut self, areas: openbse_envelope::EnvelopeAreas) {
        self.envelope_areas = Some(areas);
    }

    /// Set surface metadata for conduction summary reporting.
    /// Each tuple: (name, zone, type_str, area_m2, is_window, boundary_str)
    pub fn set_surface_metadata(&mut self, meta: Vec<(String, String, String, f64, bool, String)>) {
        self.surface_meta = meta;
    }

    /// Process one timestep snapshot.
    pub fn add_snapshot(&mut self, snapshot: &OutputSnapshot) {
        self.total_timesteps += 1;
        self.dt = snapshot.dt;

        let month_idx = (snapshot.month.saturating_sub(1) as usize).min(11);
        let me = &mut self.monthly[month_idx];

        // Accumulate energy
        let total_heating: f64 = snapshot.zone_heating_rate.values().sum();
        let total_cooling: f64 = snapshot.zone_cooling_rate.values().sum();

        me.heating_j += total_heating * snapshot.dt;
        me.cooling_j += total_cooling * snapshot.dt;
        me.hours += snapshot.dt / 3600.0;

        // Track peaks with coincident conditions
        if total_heating > self.peak_heating.0 {
            // Load-weighted average zone temp at peak
            let avg_zone_temp = if !snapshot.zone_temperature.is_empty() {
                let sum: f64 = snapshot.zone_temperature.values().sum();
                sum / snapshot.zone_temperature.len() as f64
            } else {
                0.0
            };
            self.peak_heating = (
                total_heating,
                snapshot.month,
                snapshot.day,
                snapshot.hour,
                snapshot.site_outdoor_temperature,
                avg_zone_temp,
                snapshot.site_wind_speed,
            );
        }
        if total_cooling > self.peak_cooling.0 {
            let avg_zone_temp = if !snapshot.zone_temperature.is_empty() {
                let sum: f64 = snapshot.zone_temperature.values().sum();
                sum / snapshot.zone_temperature.len() as f64
            } else {
                0.0
            };
            self.peak_cooling = (
                total_cooling,
                snapshot.month,
                snapshot.day,
                snapshot.hour,
                snapshot.site_outdoor_temperature,
                avg_zone_temp,
                snapshot.site_wind_speed,
            );
        }

        // Track per-zone peaks
        for (zone_name, &rate) in &snapshot.zone_heating_rate {
            let entry = self
                .zone_peak_heating
                .entry(zone_name.clone())
                .or_insert((0.0, 0, 0, 0, 0.0));
            if rate > entry.0 {
                *entry = (
                    rate,
                    snapshot.month,
                    snapshot.day,
                    snapshot.hour,
                    snapshot.site_outdoor_temperature,
                );
            }
        }
        for (zone_name, &rate) in &snapshot.zone_cooling_rate {
            let entry = self
                .zone_peak_cooling
                .entry(zone_name.clone())
                .or_insert((0.0, 0, 0, 0, 0.0));
            if rate > entry.0 {
                *entry = (
                    rate,
                    snapshot.month,
                    snapshot.day,
                    snapshot.hour,
                    snapshot.site_outdoor_temperature,
                );
            }
        }

        // Accumulate energy end-use breakdown using typed snapshot fields
        // (avoids fragile name-based matching for DHW, ext equip, pumps, etc.)

        // 1. Typed end-use maps — DHW, exterior, pumps, heat rejection, humidification
        for &pw in snapshot.dhw_electric_power.values() {
            let energy = pw * snapshot.dt;
            if energy.is_finite() {
                me.dhw_elec_j += energy;
            }
        }
        for &pw in snapshot.dhw_fuel_power.values() {
            let energy = pw * snapshot.dt;
            if energy.is_finite() {
                me.dhw_gas_j += energy;
            }
        }
        for &pw in snapshot.ext_lighting_power.values() {
            let energy = pw * snapshot.dt;
            if energy.is_finite() {
                me.ext_lighting_j += energy;
            }
        }
        for &pw in snapshot.ext_equipment_power.values() {
            let energy = pw * snapshot.dt;
            if energy.is_finite() {
                me.ext_equipment_j += energy;
            }
        }
        for &pw in snapshot.pump_electric_power.values() {
            let energy = pw * snapshot.dt;
            if energy.is_finite() {
                me.pump_elec_j += energy;
            }
        }
        for &pw in snapshot.heat_rejection_power.values() {
            let energy = pw * snapshot.dt;
            if energy.is_finite() {
                me.heat_rejection_elec_j += energy;
            }
        }
        for &pw in snapshot.humidification_power.values() {
            let energy = pw * snapshot.dt;
            if energy.is_finite() {
                me.humidification_elec_j += energy;
            }
        }
        for &pw in snapshot.heat_recovery_power.values() {
            let energy = pw * snapshot.dt;
            if energy.is_finite() {
                me.heat_recovery_elec_j += energy;
            }
        }

        // 2. Generic HVAC component power — categorize by ComponentKind (robust to
        //    naming), with a name fallback for kind-less entries. Pumps, ext
        //    equipment, DHW, etc. are handled by typed maps above. Unrecognized
        //    components land in `other_elec_j` so they are still counted in the
        //    annual electric total (which is the sum of the end-use rows).
        for (comp_name, &pw) in &snapshot.component_electric_power {
            let energy = pw * snapshot.dt;
            if !energy.is_finite() {
                continue;
            }
            let kind = snapshot.component_kinds.get(comp_name).copied();
            match elec_end_use(kind, comp_name) {
                ElecEndUse::Fan => me.fan_elec_j += energy,
                ElecEndUse::Cooling => me.cool_elec_j += energy,
                ElecEndUse::Heating => me.heat_elec_j += energy,
                ElecEndUse::Other => me.other_elec_j += energy,
            }
        }
        // All fuel-burning HVAC components are gas heating (boilers, gas furnaces).
        for &pw in snapshot.component_fuel_power.values() {
            let energy = pw * snapshot.dt;
            if energy.is_finite() {
                me.heat_gas_j += energy;
            }
        }
        // Safety cap: gas can never be negative (sanity check)
        if me.heat_gas_j < 0.0 {
            me.heat_gas_j = 0.0;
        }

        // 3. Zone internal gains — interior lighting and equipment.
        // IT equipment is tracked separately in it_equipment_j; subtract it here
        // so it doesn't appear in both the Equipment and IT Equipment end-use rows.
        for &pw in snapshot.zone_lighting_power.values() {
            me.lighting_j += pw * snapshot.dt;
        }
        for (&pw, zone_name) in snapshot.zone_equipment_power.iter().map(|(k, v)| (v, k)) {
            let it_pw = snapshot
                .it_equipment_power
                .get(zone_name)
                .copied()
                .unwrap_or(0.0);
            let equip_only = (pw - it_pw).max(0.0);
            me.equipment_j += equip_only * snapshot.dt;
        }

        // 4. Data center end uses
        for &pw in snapshot.it_equipment_power.values() {
            let energy = pw * snapshot.dt;
            if energy.is_finite() {
                me.it_equipment_j += energy;
            }
        }
        let elec_dist_energy = snapshot.elec_dist_power * snapshot.dt;
        if elec_dist_energy.is_finite() {
            me.elec_dist_j += elec_dist_energy;
        }

        // Accumulate window solar data (transmitted solar is only non-zero for windows)
        let total_transmitted: f64 = snapshot.surface_transmitted_solar.values().sum();
        self.total_transmitted_solar_j += total_transmitted * snapshot.dt;
        // Track incident solar on window surfaces only
        for (surf_name, &trans_w) in &snapshot.surface_transmitted_solar {
            if trans_w > 0.0 || surf_name.to_lowercase().contains("window") {
                if let Some(&inc_w) = snapshot.surface_incident_solar.get(surf_name) {
                    self.total_incident_solar_j += inc_w * snapshot.dt;
                }
            }
        }
        self.monthly_transmitted_solar_j[month_idx] += total_transmitted * snapshot.dt;

        // Accumulate per-surface conduction energy (CTF-based, opaque surfaces)
        for (surf_name, &cond_w) in &snapshot.surface_conduction_inside {
            let energy_j = cond_w * snapshot.dt;
            if energy_j.is_finite() {
                *self
                    .surface_conduction_j
                    .entry(surf_name.clone())
                    .or_insert(0.0) += energy_j;
            }
        }
        // Accumulate per-surface convection energy (used for windows)
        for (surf_name, &conv_w) in &snapshot.surface_convection_inside {
            let energy_j = conv_w * snapshot.dt;
            if energy_j.is_finite() {
                *self
                    .surface_convection_j
                    .entry(surf_name.clone())
                    .or_insert(0.0) += energy_j;
            }
        }

        // Accumulate monthly surface temperatures and incident solar
        self.monthly_surf_count[month_idx] += 1;
        for (surf_name, &temp) in &snapshot.surface_inside_temperature {
            let arr = self
                .monthly_surf_temp_inside
                .entry(surf_name.clone())
                .or_insert([0.0; 12]);
            arr[month_idx] += temp;
        }
        for (surf_name, &temp) in &snapshot.surface_outside_temperature {
            let arr = self
                .monthly_surf_temp_outside
                .entry(surf_name.clone())
                .or_insert([0.0; 12]);
            arr[month_idx] += temp;
        }
        for (surf_name, &solar) in &snapshot.surface_incident_solar {
            let arr = self
                .monthly_surf_incident_solar
                .entry(surf_name.clone())
                .or_insert([0.0; 12]);
            arr[month_idx] += solar;
        }

        // Accumulate submeter energy
        for (meter_name, end_uses) in &snapshot.submeter_power {
            for (end_use, &pw) in end_uses {
                let energy = pw * snapshot.dt;
                if energy.is_finite() {
                    let arr = self
                        .submeter_monthly_j
                        .entry((meter_name.clone(), end_use.clone()))
                        .or_insert([0.0; 12]);
                    arr[month_idx] += energy;
                }
            }
        }

        // Unmet hours check
        // Use per-timestep setpoints (schedule-aware) when available,
        // otherwise fall back to static setpoints from ideal_loads defaults
        let hours_fraction = snapshot.dt / 3600.0;
        for (zone_name, &zone_temp) in &snapshot.zone_temperature {
            let heat_sp = snapshot
                .zone_heating_setpoint
                .get(zone_name)
                .or_else(|| self.heating_setpoints.get(zone_name));
            let cool_sp = snapshot
                .zone_cooling_setpoint
                .get(zone_name)
                .or_else(|| self.cooling_setpoints.get(zone_name));

            if let Some(&sp) = heat_sp {
                if zone_temp < sp - self.unmet_tolerance {
                    self.unmet_heating_hours += hours_fraction;
                }
            }
            if let Some(&sp) = cool_sp {
                if zone_temp > sp + self.unmet_tolerance {
                    self.unmet_cooling_hours += hours_fraction;
                }
            }
        }
    }

    /// Write the summary report to a text file.
    pub fn write(&self, path: &Path) -> Result<(), OutputError> {
        let file = std::fs::File::create(path)
            .map_err(|e| OutputError::IoError(format!("{}: {}", path.display(), e)))?;
        let mut w = std::io::BufWriter::new(file);

        writeln!(
            w,
            "================================================================"
        )?;
        writeln!(
            w,
            "                    OpenBSE Summary Report                       "
        )?;
        writeln!(
            w,
            "================================================================"
        )?;
        writeln!(w)?;

        // -- Annual Totals --
        let annual_heating_j: f64 = self.monthly.iter().map(|m| m.heating_j).sum();
        let annual_cooling_j: f64 = self.monthly.iter().map(|m| m.cooling_j).sum();
        let annual_heating_kwh = annual_heating_j / 3_600_000.0;
        let annual_cooling_kwh = annual_cooling_j / 3_600_000.0;
        let annual_heating_mwh = annual_heating_kwh / 1000.0;
        let annual_cooling_mwh = annual_cooling_kwh / 1000.0;

        writeln!(
            w,
            "-- Annual Energy Summary --------------------------------------"
        )?;
        writeln!(w)?;
        writeln!(
            w,
            "  Heating:  {:>10.1} kWh  ({:.3} MWh)",
            annual_heating_kwh, annual_heating_mwh
        )?;
        writeln!(
            w,
            "  Cooling:  {:>10.1} kWh  ({:.3} MWh)",
            annual_cooling_kwh, annual_cooling_mwh
        )?;
        writeln!(
            w,
            "  Total:    {:>10.1} kWh  ({:.3} MWh)",
            annual_heating_kwh + annual_cooling_kwh,
            annual_heating_mwh + annual_cooling_mwh
        )?;
        writeln!(w)?;

        // -- Peak Loads --
        writeln!(
            w,
            "-- Peak Loads -------------------------------------------------"
        )?;
        writeln!(w)?;
        if self.peak_heating.0 > 0.0 {
            writeln!(
                w,
                "  Peak Heating: {:>10.1} W  (Month {:>2}, Day {:>2}, Hour {:>2})",
                self.peak_heating.0, self.peak_heating.1, self.peak_heating.2, self.peak_heating.3
            )?;
            writeln!(
                w,
                "    Coincident: OA {:.1}°C, Zone {:.1}°C, Wind {:.1} m/s",
                self.peak_heating.4, self.peak_heating.5, self.peak_heating.6
            )?;
        } else {
            writeln!(w, "  Peak Heating:       0.0 W  (no heating required)")?;
        }
        if self.peak_cooling.0 > 0.0 {
            writeln!(
                w,
                "  Peak Cooling: {:>10.1} W  (Month {:>2}, Day {:>2}, Hour {:>2})",
                self.peak_cooling.0, self.peak_cooling.1, self.peak_cooling.2, self.peak_cooling.3
            )?;
            writeln!(
                w,
                "    Coincident: OA {:.1}°C, Zone {:.1}°C, Wind {:.1} m/s",
                self.peak_cooling.4, self.peak_cooling.5, self.peak_cooling.6
            )?;
        } else {
            writeln!(w, "  Peak Cooling:       0.0 W  (no cooling required)")?;
        }
        writeln!(w)?;

        // -- Monthly Energy End-Use --
        {
            let rows = self.compute_enduse_rows();
            let month_names = [
                "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
            ];

            writeln!(
                w,
                "-- Monthly Energy End-Use [kWh] --------------------------------"
            )?;
            writeln!(w)?;

            // Header
            write!(w, "  {:<21}", "End Use")?;
            for mn in &month_names {
                write!(w, "  {:>7}", mn)?;
            }
            writeln!(w, "  {:>9}", "Total")?;

            write!(w, "  {:-<21}", "")?;
            for _ in 0..12 {
                write!(w, "  {:-<7}", "")?;
            }
            writeln!(w, "  {:-<9}", "")?;

            // Data rows
            for row in &rows {
                write!(w, "  {:<21}", row.label)?;
                for mi in 0..12 {
                    write!(w, "  {:>7.1}", row.monthly[mi])?;
                }
                writeln!(w, "  {:>9.1}", row.total)?;
            }

            // Separator
            write!(w, "  {:-<21}", "")?;
            for _ in 0..12 {
                write!(w, "  {:-<7}", "")?;
            }
            writeln!(w, "  {:-<9}", "")?;

            // Total Electric
            let mut total_elec = [0.0_f64; 12];
            let mut total_elec_annual = 0.0_f64;
            for row in &rows {
                if !row.label.contains("Gas") {
                    for mi in 0..12 {
                        total_elec[mi] += row.monthly[mi];
                    }
                    total_elec_annual += row.total;
                }
            }
            write!(w, "  {:<21}", "Total Electric")?;
            for mi in 0..12 {
                write!(w, "  {:>7.1}", total_elec[mi])?;
            }
            writeln!(w, "  {:>9.1}", total_elec_annual)?;

            // Total Gas
            let mut total_gas = [0.0_f64; 12];
            let mut total_gas_annual = 0.0_f64;
            for row in &rows {
                if row.label.contains("Gas") {
                    for mi in 0..12 {
                        total_gas[mi] += row.monthly[mi];
                    }
                    total_gas_annual += row.total;
                }
            }
            write!(w, "  {:<21}", "Total Gas")?;
            for mi in 0..12 {
                write!(w, "  {:>7.1}", total_gas[mi])?;
            }
            writeln!(w, "  {:>9.1}", total_gas_annual)?;

            // Grand Total
            write!(w, "  {:<21}", "Total")?;
            for mi in 0..12 {
                write!(w, "  {:>7.1}", total_elec[mi] + total_gas[mi])?;
            }
            writeln!(w, "  {:>9.1}", total_elec_annual + total_gas_annual)?;
            writeln!(w)?;

            // -- Zone Loads [kWh] --
            writeln!(
                w,
                "-- Zone Loads [kWh] -------------------------------------------"
            )?;
            writeln!(w)?;

            write!(w, "  {:<21}", "Load")?;
            for mn in &month_names {
                write!(w, "  {:>7}", mn)?;
            }
            writeln!(w, "  {:>9}", "Total")?;

            write!(w, "  {:-<21}", "")?;
            for _ in 0..12 {
                write!(w, "  {:-<7}", "")?;
            }
            writeln!(w, "  {:-<9}", "")?;

            // Heating
            write!(w, "  {:<21}", "Heating")?;
            for (i, me) in self.monthly.iter().enumerate() {
                let _ = i;
                write!(w, "  {:>7.1}", me.heating_j / 3_600_000.0)?;
            }
            writeln!(w, "  {:>9.1}", annual_heating_kwh)?;

            // Cooling
            write!(w, "  {:<21}", "Cooling")?;
            for me in &self.monthly {
                write!(w, "  {:>7.1}", me.cooling_j / 3_600_000.0)?;
            }
            writeln!(w, "  {:>9.1}", annual_cooling_kwh)?;

            // Total
            write!(w, "  {:<21}", "Total")?;
            for me in &self.monthly {
                write!(w, "  {:>7.1}", (me.heating_j + me.cooling_j) / 3_600_000.0)?;
            }
            writeln!(w, "  {:>9.1}", annual_heating_kwh + annual_cooling_kwh)?;
            writeln!(w)?;
        }

        // -- Unmet Hours --
        writeln!(
            w,
            "-- Unmet Hours ------------------------------------------------"
        )?;
        writeln!(w)?;
        writeln!(w, "  Tolerance: {:.1} C", self.unmet_tolerance)?;
        writeln!(w)?;
        writeln!(
            w,
            "  Unmet Heating Hours: {:>8.1} hr",
            self.unmet_heating_hours
        )?;
        writeln!(
            w,
            "  Unmet Cooling Hours: {:>8.1} hr",
            self.unmet_cooling_hours
        )?;

        let total_hours = self.total_timesteps as f64 * self.dt / 3600.0;
        if total_hours > 0.0 {
            let heat_pct = self.unmet_heating_hours / total_hours * 100.0;
            let cool_pct = self.unmet_cooling_hours / total_hours * 100.0;
            writeln!(w)?;
            writeln!(
                w,
                "  Heating setpoint met: {:>5.1}% of occupied hours",
                100.0 - heat_pct
            )?;
            writeln!(
                w,
                "  Cooling setpoint met: {:>5.1}% of occupied hours",
                100.0 - cool_pct
            )?;

            // ASHRAE Standard 90.1 compliance check (300 unmet hours max)
            writeln!(w)?;
            let total_unmet = self.unmet_heating_hours + self.unmet_cooling_hours;
            if total_unmet <= 300.0 {
                writeln!(
                    w,
                    "  ASHRAE 90.1 Compliance: PASS ({:.0} <= 300 unmet hours)",
                    total_unmet
                )?;
            } else {
                writeln!(
                    w,
                    "  ASHRAE 90.1 Compliance: FAIL ({:.0} > 300 unmet hours)",
                    total_unmet
                )?;
            }
        }
        writeln!(w)?;

        // -- Zone Loads Summary --
        if !self.zone_peak_heating.is_empty() || !self.zone_peak_cooling.is_empty() {
            writeln!(
                w,
                "-- Zone Loads Summary -----------------------------------------"
            )?;
            writeln!(w)?;

            let month_abbr = [
                "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
            ];

            writeln!(
                w,
                "  {:<21}  {:>8}  {:>9}  {:>6}  {:>14}  {:>9}  {:>6}  {:>14}  {:>6}",
                "Zone Name",
                "Area[m\u{00b2}]",
                "Pk Htg[W]",
                "W/m\u{00b2}",
                "Time",
                "Pk Clg[W]",
                "W/m\u{00b2}",
                "Time",
                "OA[\u{00b0}C]"
            )?;
            writeln!(
                w,
                "  {:-<21}  {:-<8}  {:-<9}  {:-<6}  {:-<14}  {:-<9}  {:-<6}  {:-<14}  {:-<6}",
                "", "", "", "", "", "", "", "", ""
            )?;

            let mut all_zones: Vec<String> = self
                .zone_peak_heating
                .keys()
                .chain(self.zone_peak_cooling.keys())
                .cloned()
                .collect();
            all_zones.sort();
            all_zones.dedup();

            let mut bldg_area = 0.0_f64;
            let mut bldg_pk_htg = 0.0_f64;
            let mut bldg_pk_clg = 0.0_f64;

            for zone in &all_zones {
                let area = self.zone_floor_areas.get(zone).copied().unwrap_or(0.0);
                let (h_w, h_mo, h_d, h_hr, _h_oa) = self
                    .zone_peak_heating
                    .get(zone)
                    .copied()
                    .unwrap_or((0.0, 0, 0, 0, 0.0));
                let (c_w, c_mo, c_d, c_hr, c_oa) = self
                    .zone_peak_cooling
                    .get(zone)
                    .copied()
                    .unwrap_or((0.0, 0, 0, 0, 0.0));

                let h_wm2 = if area > 0.0 { h_w / area } else { 0.0 };
                let c_wm2 = if area > 0.0 { c_w / area } else { 0.0 };

                let h_time = if h_w > 0.0 {
                    format!(
                        "{} {:>2} {:02}:00",
                        month_abbr[(h_mo.saturating_sub(1) as usize).min(11)],
                        h_d,
                        h_hr
                    )
                } else {
                    String::from("-")
                };
                let c_time = if c_w > 0.0 {
                    format!(
                        "{} {:>2} {:02}:00",
                        month_abbr[(c_mo.saturating_sub(1) as usize).min(11)],
                        c_d,
                        c_hr
                    )
                } else {
                    String::from("-")
                };

                let display_name = if zone.len() > 21 {
                    &zone[..21]
                } else {
                    zone.as_str()
                };

                writeln!(
                    w,
                    "  {:<21}  {:>8.1}  {:>9.1}  {:>6.1}  {:>14}  {:>9.1}  {:>6.1}  {:>14}  {:>6.1}",
                    display_name, area, h_w, h_wm2, h_time, c_w, c_wm2, c_time, c_oa
                )?;

                bldg_area += area;
                bldg_pk_htg += h_w;
                bldg_pk_clg += c_w;
            }

            writeln!(
                w,
                "  {:-<21}  {:-<8}  {:-<9}  {:-<6}  {:-<14}  {:-<9}  {:-<6}  {:-<14}  {:-<6}",
                "", "", "", "", "", "", "", "", ""
            )?;
            let bldg_h_wm2 = if bldg_area > 0.0 {
                bldg_pk_htg / bldg_area
            } else {
                0.0
            };
            let bldg_c_wm2 = if bldg_area > 0.0 {
                bldg_pk_clg / bldg_area
            } else {
                0.0
            };
            writeln!(
                w,
                "  {:<21}  {:>8.1}  {:>9.1}  {:>6.1}  {:>14}  {:>9.1}  {:>6.1}  {:>14}  {:>6}",
                "Building Total",
                bldg_area,
                bldg_pk_htg,
                bldg_h_wm2,
                "",
                bldg_pk_clg,
                bldg_c_wm2,
                "",
                ""
            )?;
            writeln!(w)?;
        }

        // -- Building Envelope Summary (Wall/Window Areas + WWR) --
        if let Some(ref ea) = self.envelope_areas {
            let total_wall = ea.total_wall_area();
            if total_wall > 0.0 {
                writeln!(
                    w,
                    "-- Building Envelope Summary ----------------------------------"
                )?;
                writeln!(w)?;
                use openbse_envelope::CardinalDirection;
                let dirs = [
                    CardinalDirection::North,
                    CardinalDirection::East,
                    CardinalDirection::South,
                    CardinalDirection::West,
                ];
                writeln!(
                    w,
                    "  {:>10}  {:>12}  {:>12}  {:>8}",
                    "Direction", "Wall [m²]", "Window [m²]", "WWR"
                )?;
                writeln!(w, "  ----------  ------------  ------------  --------")?;
                for dir in &dirs {
                    let i = match dir {
                        CardinalDirection::North => 0,
                        CardinalDirection::East => 1,
                        CardinalDirection::South => 2,
                        CardinalDirection::West => 3,
                    };
                    writeln!(
                        w,
                        "  {:>10}  {:>12.1}  {:>12.1}  {:>7.1}%",
                        dir,
                        ea.wall_area[i],
                        ea.window_area[i],
                        ea.wwr(*dir) * 100.0
                    )?;
                }
                writeln!(w, "  ----------  ------------  ------------  --------")?;
                writeln!(
                    w,
                    "  {:>10}  {:>12.1}  {:>12.1}  {:>7.1}%",
                    "Total",
                    total_wall,
                    ea.total_window_area(),
                    ea.total_wwr() * 100.0
                )?;
                writeln!(w)?;
            }
        }

        // -- Window Solar Diagnostics --
        if self.total_transmitted_solar_j > 0.0 {
            writeln!(
                w,
                "-- Window Solar Diagnostics -----------------------------------"
            )?;
            writeln!(w)?;
            let trans_kwh = self.total_transmitted_solar_j / 3_600_000.0;
            let inc_kwh = self.total_incident_solar_j / 3_600_000.0;
            writeln!(w, "  Total transmitted solar: {:>10.1} kWh", trans_kwh)?;
            writeln!(w, "  Total incident on windows: {:>7.1} kWh", inc_kwh)?;
            if inc_kwh > 0.0 {
                writeln!(
                    w,
                    "  Effective annual modifier: {:>7.4} (trans/incident)",
                    trans_kwh / inc_kwh
                )?;
            }
            writeln!(w)?;
            writeln!(w, "  {:>5}  {:>12}", "Month", "Trans[kWh]")?;
            writeln!(w, "  -----  ------------")?;
            let month_names = [
                "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
            ];
            for (i, &mj) in self.monthly_transmitted_solar_j.iter().enumerate() {
                if mj > 0.0 {
                    writeln!(w, "  {:>5}  {:>12.1}", month_names[i], mj / 3_600_000.0)?;
                }
            }
            writeln!(w, "  -----  ------------")?;
            writeln!(w, "  {:>5}  {:>12.1}", "Total", trans_kwh)?;
            writeln!(w)?;
        }

        // -- Surface Heat Gains Summary --
        if !self.surface_meta.is_empty()
            && (!self.surface_conduction_j.is_empty() || !self.surface_convection_j.is_empty())
        {
            writeln!(
                w,
                "-- Surface Heat Gains Summary ---------------------------------"
            )?;
            writeln!(w)?;

            // Collect unique zones in order of first appearance
            let mut zones_seen: Vec<String> = Vec::new();
            for (_, zone, _, _, _, _) in &self.surface_meta {
                if !zones_seen.contains(zone) {
                    zones_seen.push(zone.clone());
                }
            }

            let mut building_total = 0.0_f64;

            for zone_name in &zones_seen {
                writeln!(w, "  Zone: {}", zone_name)?;
                writeln!(
                    w,
                    "    {:<32} {:<7} {:>8}  {:<12} {:>10}",
                    "Surface Name", "Type", "Area[m²]", "Boundary", "Cond[kWh]"
                )?;
                writeln!(
                    w,
                    "    {:-<32} {:-<7} {:-<8}  {:-<12} {:-<10}",
                    "", "", "", "", ""
                )?;

                let mut zone_total = 0.0_f64;

                for (name, zone, type_str, area, is_window, boundary) in &self.surface_meta {
                    if zone != zone_name {
                        continue;
                    }
                    // Windows: use convection (q_conv_inside); opaque: use conduction (CTF q_cond_inside)
                    let energy_kwh = if *is_window {
                        self.surface_convection_j.get(name).copied().unwrap_or(0.0) / 3_600_000.0
                    } else {
                        self.surface_conduction_j.get(name).copied().unwrap_or(0.0) / 3_600_000.0
                    };
                    zone_total += energy_kwh;
                    let display_name = if name.len() > 32 {
                        &name[..32]
                    } else {
                        name.as_str()
                    };
                    writeln!(
                        w,
                        "    {:<32} {:<7} {:>8.1}  {:<12} {:>10.1}",
                        display_name, type_str, area, boundary, energy_kwh
                    )?;
                }

                building_total += zone_total;
                writeln!(
                    w,
                    "    {:<32} {:<7} {:>8}  {:<12} {:>10.1}",
                    "Zone Total", "", "", "", zone_total
                )?;
                writeln!(w)?;
            }

            // Surface type subtotals across entire building
            let mut type_totals: std::collections::BTreeMap<String, (f64, f64)> =
                std::collections::BTreeMap::new(); // (area, kwh)
            for (name, _zone, type_str, area, is_window, _boundary) in &self.surface_meta {
                let energy_kwh = if *is_window {
                    self.surface_convection_j.get(name).copied().unwrap_or(0.0) / 3_600_000.0
                } else {
                    self.surface_conduction_j.get(name).copied().unwrap_or(0.0) / 3_600_000.0
                };
                let entry = type_totals.entry(type_str.clone()).or_insert((0.0, 0.0));
                entry.0 += area;
                entry.1 += energy_kwh;
            }
            writeln!(
                w,
                "  {:<34} {:>8}  {:>10}",
                "By Surface Type", "Area[m²]", "Cond[kWh]"
            )?;
            writeln!(w, "  {:-<34} {:-<8}  {:-<10}", "", "", "")?;
            for (type_str, (area, kwh)) in &type_totals {
                writeln!(w, "  {:<34} {:>8.1}  {:>10.1}", type_str, area, kwh)?;
            }
            writeln!(w, "  {:-<34} {:-<8}  {:-<10}", "", "", "")?;
            writeln!(
                w,
                "  {:<34} {:>8.1}  {:>10.1}",
                "Building Total",
                type_totals.values().map(|(a, _)| a).sum::<f64>(),
                building_total
            )?;
            writeln!(w)?;
        }

        // -- Monthly Surface Temperature Diagnostics --
        if !self.monthly_surf_temp_inside.is_empty() {
            writeln!(
                w,
                "-- Monthly Surface Temperature Diagnostics --------------------"
            )?;
            writeln!(w)?;

            let months = [
                "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
            ];

            // Collect outdoor-boundary wall and roof surfaces for diagnostic output
            let mut diag_surfaces: Vec<String> = self
                .surface_meta
                .iter()
                .filter(|(_, _, type_str, _, is_window, boundary)| {
                    !is_window
                        && boundary == "outdoor"
                        && (type_str == "wall" || type_str == "roof")
                })
                .map(|(name, _, _, _, _, _)| name.clone())
                .collect();
            diag_surfaces.sort();

            for surf_name in &diag_surfaces {
                let inside = self.monthly_surf_temp_inside.get(surf_name);
                let outside = self.monthly_surf_temp_outside.get(surf_name);
                let solar = self.monthly_surf_incident_solar.get(surf_name);

                if inside.is_none() {
                    continue;
                }

                writeln!(w, "  Surface: {}", surf_name)?;
                writeln!(
                    w,
                    "    {:>5}  {:>10}  {:>10}  {:>14}",
                    "Month", "Inside[°C]", "Outside[°C]", "IncSolar[W/m²]"
                )?;
                writeln!(
                    w,
                    "    {}  {}  {}  {}",
                    "-".repeat(5),
                    "-".repeat(10),
                    "-".repeat(10),
                    "-".repeat(14)
                )?;

                for mi in 0..12 {
                    let count = self.monthly_surf_count[mi] as f64;
                    if count < 1.0 {
                        continue;
                    }

                    let t_in = inside.map(|a| a[mi] / count).unwrap_or(0.0);
                    let t_out = outside.map(|a| a[mi] / count).unwrap_or(0.0);
                    let sol = solar.map(|a| a[mi] / count).unwrap_or(0.0);

                    writeln!(
                        w,
                        "    {:>5}  {:>10.2}  {:>10.2}  {:>14.2}",
                        months[mi], t_in, t_out, sol
                    )?;
                }
                writeln!(w)?;
            }
        }

        // -- Simulation Statistics --
        writeln!(
            w,
            "-- Simulation Statistics ---------------------------------------"
        )?;
        writeln!(w)?;
        writeln!(w, "  Total timesteps:   {:>8}", self.total_timesteps)?;
        writeln!(
            w,
            "  Timestep size:     {:>8.0} s ({:.0} per hour)",
            self.dt,
            3600.0 / self.dt
        )?;
        writeln!(w, "  Simulated hours:   {:>8.1} hr", total_hours)?;
        writeln!(w)?;

        // -- Submeter Breakdown --
        // Only show if any non-"General" submeters exist
        {
            let has_custom = self.submeter_monthly_j.keys().any(|(m, _)| m != "General");
            if has_custom {
                writeln!(
                    w,
                    "-- Submeter Breakdown [kWh] -----------------------------------"
                )?;
                writeln!(w)?;

                let month_names = [
                    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov",
                    "Dec",
                ];

                write!(w, "  {:<16} {:<20}", "Submeter", "End Use")?;
                for mn in &month_names {
                    write!(w, " {:>7}", mn)?;
                }
                writeln!(w, " {:>9}", "Total")?;

                write!(w, "  {:-<16} {:-<20}", "", "")?;
                for _ in 0..12 {
                    write!(w, " {:-<7}", "")?;
                }
                writeln!(w, " {:-<9}", "")?;

                let mut keys: Vec<_> = self.submeter_monthly_j.keys().cloned().collect();
                keys.sort();
                for (meter, end_use) in &keys {
                    let arr = &self.submeter_monthly_j[&(meter.clone(), end_use.clone())];
                    let total: f64 = arr.iter().sum::<f64>() / 3_600_000.0;
                    if total.abs() < 0.05 {
                        continue;
                    }
                    write!(w, "  {:<16} {:<20}", meter, end_use)?;
                    for mi in 0..12 {
                        write!(w, " {:>7.1}", arr[mi] / 3_600_000.0)?;
                    }
                    writeln!(w, " {:>9.1}", total)?;
                }
                writeln!(w)?;
            }
        }

        // -- Data Center Performance (ASHRAE 90.4) --
        {
            let annual_it_j: f64 = self.monthly.iter().map(|m| m.it_equipment_j).sum();
            if annual_it_j > 0.0 {
                // MLC uses only DC-submeter-tagged mechanical energy so that
                // office/other HVAC in a mixed-use building does not inflate the ratio.
                let dc_mech_end_uses = [
                    "fan_electric",
                    "cooling_electric",
                    "pump_electric",
                    "heat_rejection",
                ];
                let annual_mech_j: f64 = dc_mech_end_uses
                    .iter()
                    .map(|&eu| {
                        self.submeter_monthly_j
                            .get(&("datacenter".to_string(), eu.to_string()))
                            .map(|arr| arr.iter().sum::<f64>())
                            .unwrap_or(0.0)
                    })
                    .sum();
                let annual_elec_dist_j: f64 = self.monthly.iter().map(|m| m.elec_dist_j).sum();
                let annual_total_j = annual_it_j + annual_mech_j + annual_elec_dist_j;

                let pue = annual_total_j / annual_it_j;
                let mlc = annual_mech_j / annual_it_j;
                let elc = annual_elec_dist_j / annual_it_j;

                let cz_str = self.climate_zone.as_deref().unwrap_or("Unknown");
                let mlc_limit = ashrae_90_4_mlc_limit(cz_str);
                let elc_limit = 0.060_f64;

                let mlc_pass = mlc <= mlc_limit;
                let elc_pass = elc <= elc_limit;

                writeln!(
                    w,
                    "-- Data Center Performance (ASHRAE 90.4) ---------------------"
                )?;
                writeln!(w, "  Climate Zone: {}", cz_str)?;
                writeln!(w, "  {:-<60}", "")?;
                writeln!(
                    w,
                    "  Annual IT Equipment Load:    {:>12.1} MWh",
                    annual_it_j / 3_600_000_000.0
                )?;
                writeln!(
                    w,
                    "  Annual Mechanical Load:      {:>12.1} MWh",
                    annual_mech_j / 3_600_000_000.0
                )?;
                writeln!(
                    w,
                    "  Annual Elec Dist Losses:     {:>12.1} MWh",
                    annual_elec_dist_j / 3_600_000_000.0
                )?;
                writeln!(
                    w,
                    "  Annual Facility Total:       {:>12.1} MWh",
                    annual_total_j / 3_600_000_000.0
                )?;
                writeln!(w)?;
                writeln!(w, "  PUE: {:>8.3}", pue)?;
                writeln!(
                    w,
                    "  MLC: {:>8.3}   (limit: {:.3} for CZ {})   {}",
                    mlc,
                    mlc_limit,
                    cz_str,
                    if mlc_pass { "PASS" } else { "FAIL" }
                )?;
                writeln!(
                    w,
                    "  ELC: {:>8.3}   (limit: {:.3})              {}",
                    elc,
                    elc_limit,
                    if elc_pass { "PASS" } else { "FAIL" }
                )?;
                writeln!(w)?;
            }
        }

        writeln!(
            w,
            "================================================================"
        )?;

        w.flush()?;
        Ok(())
    }

    /// Compute monthly end-use rows from accumulated monthly energy data.
    fn compute_enduse_rows(&self) -> Vec<EndUseRow> {
        let j_to_kwh = 1.0 / 3_600_000.0;

        let make_row = |label: &'static str, extractor: fn(&MonthlyEnergy) -> f64| -> EndUseRow {
            let mut monthly = [0.0_f64; 12];
            let mut total = 0.0_f64;
            for (i, me) in self.monthly.iter().enumerate() {
                let kwh = extractor(me) * j_to_kwh;
                monthly[i] = kwh;
                total += kwh;
            }
            EndUseRow {
                label,
                monthly,
                total,
            }
        };

        vec![
            make_row("Interior Lighting", |m| m.lighting_j),
            make_row("Exterior Lighting", |m| m.ext_lighting_j),
            make_row("Interior Equipment", |m| m.equipment_j),
            make_row("Exterior Equipment", |m| m.ext_equipment_j),
            make_row("IT Equipment", |m| m.it_equipment_j),
            make_row("Elec Distribution", |m| m.elec_dist_j),
            make_row("Fans (Electric)", |m| m.fan_elec_j),
            make_row("Pumps (Electric)", |m| m.pump_elec_j),
            make_row("Cooling (Electric)", |m| m.cool_elec_j),
            make_row("Heating (Electric)", |m| m.heat_elec_j),
            make_row("Other (Electric)", |m| m.other_elec_j),
            make_row("Heating (Gas)", |m| m.heat_gas_j),
            make_row("Heat Rejection", |m| m.heat_rejection_elec_j),
            make_row("Humidification", |m| m.humidification_elec_j),
            make_row("Heat Recovery", |m| m.heat_recovery_elec_j),
            make_row("DHW (Electric)", |m| m.dhw_elec_j),
            make_row("DHW (Gas)", |m| m.dhw_gas_j),
        ]
    }

    /// Write the summary report as a self-contained HTML file.
    pub fn write_html(&self, path: &Path) -> Result<(), OutputError> {
        let file = std::fs::File::create(path)
            .map_err(|e| OutputError::IoError(format!("{}: {}", path.display(), e)))?;
        let mut w = std::io::BufWriter::new(file);

        writeln!(w, "<!DOCTYPE html>")?;
        writeln!(w, "<html lang=\"en\">")?;
        writeln!(w, "<head>")?;
        writeln!(w, "<meta charset=\"UTF-8\">")?;
        writeln!(
            w,
            "<meta name=\"viewport\" content=\"width=device-width, initial-scale=1.0\">"
        )?;
        writeln!(w, "<title>OpenBSE Summary Report</title>")?;
        writeln!(w, "<style>")?;
        writeln!(
            w,
            "body {{ font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, Helvetica, Arial, sans-serif; margin: 2em; color: #333; max-width: 1400px; }}"
        )?;
        writeln!(
            w,
            "h1 {{ border-bottom: 2px solid #333; padding-bottom: 0.3em; }}"
        )?;
        writeln!(w, "h2 {{ color: #555; margin-top: 1.5em; }}")?;
        writeln!(
            w,
            "table {{ border-collapse: collapse; width: 100%; margin: 1em 0; }}"
        )?;
        writeln!(
            w,
            "th, td {{ padding: 4px 8px; text-align: right; border: 1px solid #ddd; }}"
        )?;
        writeln!(
            w,
            "th {{ background: #f5f5f5; font-weight: 600; text-align: center; }}"
        )?;
        writeln!(w, "td:first-child, th:first-child {{ text-align: left; }}")?;
        writeln!(w, "tr:nth-child(even) {{ background-color: #fafafa; }}")?;
        writeln!(
            w,
            "tr.total-row {{ font-weight: bold; border-top: 2px solid #333; }}"
        )?;
        writeln!(w, ".pass {{ color: #2a7b2a; font-weight: bold; }}")?;
        writeln!(w, ".fail {{ color: #cc0000; font-weight: bold; }}")?;
        writeln!(
            w,
            "details {{ margin: 0.5em 0; }} summary {{ cursor: pointer; font-weight: 600; }}"
        )?;
        writeln!(w, "</style>")?;
        writeln!(w, "</head>")?;
        writeln!(w, "<body>")?;
        writeln!(w, "<h1>OpenBSE Summary Report</h1>")?;

        // -- Annual Summary --
        let annual_heating_kwh: f64 =
            self.monthly.iter().map(|m| m.heating_j).sum::<f64>() / 3_600_000.0;
        let annual_cooling_kwh: f64 =
            self.monthly.iter().map(|m| m.cooling_j).sum::<f64>() / 3_600_000.0;

        writeln!(w, "<h2>Annual Energy Summary</h2>")?;
        writeln!(w, "<table>")?;
        html_table_row(&mut w, &["", "kWh", "MWh"], true)?;
        html_table_row(
            &mut w,
            &[
                "Heating",
                &format!("{:.1}", annual_heating_kwh),
                &format!("{:.3}", annual_heating_kwh / 1000.0),
            ],
            false,
        )?;
        html_table_row(
            &mut w,
            &[
                "Cooling",
                &format!("{:.1}", annual_cooling_kwh),
                &format!("{:.3}", annual_cooling_kwh / 1000.0),
            ],
            false,
        )?;
        html_table_row(
            &mut w,
            &[
                "Total",
                &format!("{:.1}", annual_heating_kwh + annual_cooling_kwh),
                &format!("{:.3}", (annual_heating_kwh + annual_cooling_kwh) / 1000.0),
            ],
            false,
        )?;
        writeln!(w, "</table>")?;

        // -- Peak Loads --
        writeln!(w, "<h2>Peak Loads</h2>")?;
        writeln!(w, "<table>")?;
        html_table_row(
            &mut w,
            &["", "W", "Time", "OA [°C]", "Zone [°C]", "Wind [m/s]"],
            true,
        )?;
        html_table_row(
            &mut w,
            &[
                "Peak Heating",
                &format!("{:.1}", self.peak_heating.0),
                &format!(
                    "Month {} Day {} Hour {}",
                    self.peak_heating.1, self.peak_heating.2, self.peak_heating.3
                ),
                &format!("{:.1}", self.peak_heating.4),
                &format!("{:.1}", self.peak_heating.5),
                &format!("{:.1}", self.peak_heating.6),
            ],
            false,
        )?;
        html_table_row(
            &mut w,
            &[
                "Peak Cooling",
                &format!("{:.1}", self.peak_cooling.0),
                &format!(
                    "Month {} Day {} Hour {}",
                    self.peak_cooling.1, self.peak_cooling.2, self.peak_cooling.3
                ),
                &format!("{:.1}", self.peak_cooling.4),
                &format!("{:.1}", self.peak_cooling.5),
                &format!("{:.1}", self.peak_cooling.6),
            ],
            false,
        )?;
        writeln!(w, "</table>")?;

        // -- Monthly End-Use Table --
        let rows = self.compute_enduse_rows();
        let month_names = [
            "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
        ];

        writeln!(w, "<h2>Monthly Energy End-Use [kWh]</h2>")?;
        writeln!(w, "<table>")?;
        write!(w, "<tr><th>End Use</th>")?;
        for mn in &month_names {
            write!(w, "<th>{}</th>", mn)?;
        }
        writeln!(w, "<th>Total</th></tr>")?;

        for row in &rows {
            write!(w, "<tr><td>{}</td>", row.label)?;
            for mi in 0..12 {
                write!(w, "<td>{:.1}</td>", row.monthly[mi])?;
            }
            writeln!(w, "<td>{:.1}</td></tr>", row.total)?;
        }

        // Total Electric
        let mut te_monthly = [0.0_f64; 12];
        let mut te_total = 0.0_f64;
        for row in &rows {
            if !row.label.contains("Gas") {
                for mi in 0..12 {
                    te_monthly[mi] += row.monthly[mi];
                }
                te_total += row.total;
            }
        }
        write!(w, "<tr class=\"total-row\"><td>Total Electric</td>")?;
        for mi in 0..12 {
            write!(w, "<td>{:.1}</td>", te_monthly[mi])?;
        }
        writeln!(w, "<td>{:.1}</td></tr>", te_total)?;

        // Total Gas
        let mut tg_monthly = [0.0_f64; 12];
        let mut tg_total = 0.0_f64;
        for row in &rows {
            if row.label.contains("Gas") {
                for mi in 0..12 {
                    tg_monthly[mi] += row.monthly[mi];
                }
                tg_total += row.total;
            }
        }
        write!(w, "<tr class=\"total-row\"><td>Total Gas</td>")?;
        for mi in 0..12 {
            write!(w, "<td>{:.1}</td>", tg_monthly[mi])?;
        }
        writeln!(w, "<td>{:.1}</td></tr>", tg_total)?;

        // Grand Total
        write!(w, "<tr class=\"total-row\"><td>Total</td>")?;
        for mi in 0..12 {
            write!(w, "<td>{:.1}</td>", te_monthly[mi] + tg_monthly[mi])?;
        }
        writeln!(w, "<td>{:.1}</td></tr>", te_total + tg_total)?;
        writeln!(w, "</table>")?;

        // -- Zone Loads Summary --
        if !self.zone_peak_heating.is_empty() || !self.zone_peak_cooling.is_empty() {
            writeln!(w, "<h2>Zone Loads Summary</h2>")?;
            writeln!(w, "<table>")?;
            html_table_row(
                &mut w,
                &[
                    "Zone Name",
                    "Area [m\u{00b2}]",
                    "Pk Htg [W]",
                    "W/m\u{00b2}",
                    "Time",
                    "Pk Clg [W]",
                    "W/m\u{00b2}",
                    "Time",
                    "OA [\u{00b0}C]",
                ],
                true,
            )?;

            let month_abbr = [
                "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
            ];

            let mut all_zones: Vec<String> = self
                .zone_peak_heating
                .keys()
                .chain(self.zone_peak_cooling.keys())
                .cloned()
                .collect();
            all_zones.sort();
            all_zones.dedup();

            for zone in &all_zones {
                let area = self.zone_floor_areas.get(zone).copied().unwrap_or(0.0);
                let (h_w, h_mo, h_d, h_hr, _h_oa) = self
                    .zone_peak_heating
                    .get(zone)
                    .copied()
                    .unwrap_or((0.0, 0, 0, 0, 0.0));
                let (c_w, c_mo, c_d, c_hr, c_oa) = self
                    .zone_peak_cooling
                    .get(zone)
                    .copied()
                    .unwrap_or((0.0, 0, 0, 0, 0.0));
                let h_wm2 = if area > 0.0 { h_w / area } else { 0.0 };
                let c_wm2 = if area > 0.0 { c_w / area } else { 0.0 };
                let h_time = if h_w > 0.0 {
                    format!(
                        "{} {} {:02}:00",
                        month_abbr[(h_mo.saturating_sub(1) as usize).min(11)],
                        h_d,
                        h_hr
                    )
                } else {
                    "-".to_string()
                };
                let c_time = if c_w > 0.0 {
                    format!(
                        "{} {} {:02}:00",
                        month_abbr[(c_mo.saturating_sub(1) as usize).min(11)],
                        c_d,
                        c_hr
                    )
                } else {
                    "-".to_string()
                };
                html_table_row(
                    &mut w,
                    &[
                        zone,
                        &format!("{:.1}", area),
                        &format!("{:.1}", h_w),
                        &format!("{:.1}", h_wm2),
                        &h_time,
                        &format!("{:.1}", c_w),
                        &format!("{:.1}", c_wm2),
                        &c_time,
                        &format!("{:.1}", c_oa),
                    ],
                    false,
                )?;
            }
            writeln!(w, "</table>")?;
        }

        // -- Unmet Hours --
        let total_hours = self.total_timesteps as f64 * self.dt / 3600.0;
        let total_unmet = self.unmet_heating_hours + self.unmet_cooling_hours;

        writeln!(w, "<h2>Unmet Hours</h2>")?;
        writeln!(w, "<table>")?;
        html_table_row(&mut w, &["", "Hours"], true)?;
        html_table_row(
            &mut w,
            &["Unmet Heating", &format!("{:.1}", self.unmet_heating_hours)],
            false,
        )?;
        html_table_row(
            &mut w,
            &["Unmet Cooling", &format!("{:.1}", self.unmet_cooling_hours)],
            false,
        )?;
        writeln!(w, "</table>")?;

        if total_hours > 0.0 {
            let compliance_class = if total_unmet <= 300.0 { "pass" } else { "fail" };
            let compliance_text = if total_unmet <= 300.0 {
                format!("PASS ({:.0} &le; 300 unmet hours)", total_unmet)
            } else {
                format!("FAIL ({:.0} &gt; 300 unmet hours)", total_unmet)
            };
            writeln!(
                w,
                "<p>ASHRAE 90.1 Compliance: <span class=\"{}\">{}</span></p>",
                compliance_class, compliance_text
            )?;
        }

        // -- Building Envelope --
        if let Some(ref ea) = self.envelope_areas {
            let total_wall = ea.total_wall_area();
            if total_wall > 0.0 {
                writeln!(w, "<h2>Building Envelope</h2>")?;
                writeln!(w, "<table>")?;
                html_table_row(
                    &mut w,
                    &["Direction", "Wall [m\u{00b2}]", "Window [m\u{00b2}]", "WWR"],
                    true,
                )?;
                use openbse_envelope::CardinalDirection;
                let dirs = [
                    CardinalDirection::North,
                    CardinalDirection::East,
                    CardinalDirection::South,
                    CardinalDirection::West,
                ];
                for dir in &dirs {
                    let i = match dir {
                        CardinalDirection::North => 0,
                        CardinalDirection::East => 1,
                        CardinalDirection::South => 2,
                        CardinalDirection::West => 3,
                    };
                    html_table_row(
                        &mut w,
                        &[
                            &format!("{}", dir),
                            &format!("{:.1}", ea.wall_area[i]),
                            &format!("{:.1}", ea.window_area[i]),
                            &format!("{:.1}%", ea.wwr(*dir) * 100.0),
                        ],
                        false,
                    )?;
                }
                writeln!(w, "</table>")?;
            }
        }

        // -- Window Solar (collapsible) --
        if self.total_transmitted_solar_j > 0.0 {
            writeln!(w, "<details><summary>Window Solar Diagnostics</summary>")?;
            let trans_kwh = self.total_transmitted_solar_j / 3_600_000.0;
            let inc_kwh = self.total_incident_solar_j / 3_600_000.0;
            writeln!(
                w,
                "<p>Total transmitted solar: {:.1} kWh<br>Total incident on windows: {:.1} kWh</p>",
                trans_kwh, inc_kwh
            )?;
            writeln!(w, "</details>")?;
        }

        // -- Submeter Breakdown (HTML) --
        {
            let has_custom = self.submeter_monthly_j.keys().any(|(m, _)| m != "General");
            if has_custom {
                writeln!(w, "<h2>Submeter Breakdown [kWh]</h2>")?;
                writeln!(w, "<table>")?;
                let month_names = [
                    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov",
                    "Dec",
                ];
                write!(w, "<tr><th>Submeter</th><th>End Use</th>")?;
                for mn in &month_names {
                    write!(w, "<th>{}</th>", mn)?;
                }
                writeln!(w, "<th>Total</th></tr>")?;

                let mut keys: Vec<_> = self.submeter_monthly_j.keys().cloned().collect();
                keys.sort();
                for (meter, end_use) in &keys {
                    let arr = &self.submeter_monthly_j[&(meter.clone(), end_use.clone())];
                    let total: f64 = arr.iter().sum::<f64>() / 3_600_000.0;
                    if total.abs() < 0.05 {
                        continue;
                    }
                    write!(w, "<tr><td>{}</td><td>{}</td>", meter, end_use)?;
                    for mi in 0..12 {
                        write!(w, "<td>{:.1}</td>", arr[mi] / 3_600_000.0)?;
                    }
                    writeln!(w, "<td><b>{:.1}</b></td></tr>", total)?;
                }
                writeln!(w, "</table>")?;
            }
        }

        // -- Data Center Performance (ASHRAE 90.4) --
        {
            let annual_it_j: f64 = self.monthly.iter().map(|m| m.it_equipment_j).sum();
            if annual_it_j > 0.0 {
                let dc_mech_end_uses = [
                    "fan_electric",
                    "cooling_electric",
                    "pump_electric",
                    "heat_rejection",
                ];
                let annual_mech_j: f64 = dc_mech_end_uses
                    .iter()
                    .map(|&eu| {
                        self.submeter_monthly_j
                            .get(&("datacenter".to_string(), eu.to_string()))
                            .map(|arr| arr.iter().sum::<f64>())
                            .unwrap_or(0.0)
                    })
                    .sum();
                let annual_elec_dist_j: f64 = self.monthly.iter().map(|m| m.elec_dist_j).sum();
                let annual_total_j = annual_it_j + annual_mech_j + annual_elec_dist_j;

                let pue = annual_total_j / annual_it_j;
                let mlc = annual_mech_j / annual_it_j;
                let elc = annual_elec_dist_j / annual_it_j;

                let cz_str = self.climate_zone.as_deref().unwrap_or("Unknown");
                let mlc_limit = ashrae_90_4_mlc_limit(cz_str);
                let elc_limit = 0.060_f64;

                let mlc_pass = mlc <= mlc_limit;
                let elc_pass = elc <= elc_limit;

                writeln!(w, "<h2>Data Center Performance (ASHRAE 90.4)</h2>")?;
                writeln!(w, "<p>Climate Zone: <strong>{}</strong></p>", cz_str)?;
                writeln!(w, "<table>")?;
                html_table_row(&mut w, &["Metric", "Value", "Unit"], true)?;
                html_table_row(
                    &mut w,
                    &[
                        "Annual IT Equipment Load",
                        &format!("{:.1}", annual_it_j / 3_600_000_000.0),
                        "MWh",
                    ],
                    false,
                )?;
                html_table_row(
                    &mut w,
                    &[
                        "Annual Mechanical Load",
                        &format!("{:.1}", annual_mech_j / 3_600_000_000.0),
                        "MWh",
                    ],
                    false,
                )?;
                html_table_row(
                    &mut w,
                    &[
                        "Annual Elec Distribution Losses",
                        &format!("{:.1}", annual_elec_dist_j / 3_600_000_000.0),
                        "MWh",
                    ],
                    false,
                )?;
                html_table_row(
                    &mut w,
                    &[
                        "Annual Facility Total",
                        &format!("{:.1}", annual_total_j / 3_600_000_000.0),
                        "MWh",
                    ],
                    false,
                )?;
                writeln!(w, "</table>")?;

                writeln!(w, "<table>")?;
                html_table_row(&mut w, &["Metric", "Value", "Limit", "Result"], true)?;
                html_table_row(&mut w, &["PUE", &format!("{:.3}", pue), "—", "—"], false)?;
                let mlc_result = if mlc_pass {
                    "<span class=\"pass\">PASS</span>"
                } else {
                    "<span class=\"fail\">FAIL</span>"
                };
                html_table_row(
                    &mut w,
                    &[
                        "MLC",
                        &format!("{:.3}", mlc),
                        &format!("{:.3} (CZ {})", mlc_limit, cz_str),
                        mlc_result,
                    ],
                    false,
                )?;
                let elc_result = if elc_pass {
                    "<span class=\"pass\">PASS</span>"
                } else {
                    "<span class=\"fail\">FAIL</span>"
                };
                html_table_row(
                    &mut w,
                    &[
                        "ELC",
                        &format!("{:.3}", elc),
                        &format!("{:.3}", elc_limit),
                        elc_result,
                    ],
                    false,
                )?;
                writeln!(w, "</table>")?;
            }
        }

        writeln!(w, "</body></html>")?;
        w.flush()?;
        Ok(())
    }

    /// Write the summary report as a structured CSV file.
    pub fn write_summary_csv(&self, path: &Path) -> Result<(), OutputError> {
        let file = std::fs::File::create(path)
            .map_err(|e| OutputError::IoError(format!("{}: {}", path.display(), e)))?;
        let mut w = std::io::BufWriter::new(file);

        let month_names = [
            "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
        ];

        // -- Monthly Energy End-Use [kWh] --
        writeln!(w, "Monthly Energy End-Use [kWh]")?;
        write!(w, "End Use")?;
        for mn in &month_names {
            write!(w, ",{}", mn)?;
        }
        writeln!(w, ",Total")?;

        let rows = self.compute_enduse_rows();
        for row in &rows {
            write!(w, "{}", row.label)?;
            for mi in 0..12 {
                write!(w, ",{:.1}", row.monthly[mi])?;
            }
            writeln!(w, ",{:.1}", row.total)?;
        }

        // Total Electric
        let mut te_monthly = [0.0_f64; 12];
        let mut te_total = 0.0_f64;
        for row in &rows {
            if !row.label.contains("Gas") {
                for mi in 0..12 {
                    te_monthly[mi] += row.monthly[mi];
                }
                te_total += row.total;
            }
        }
        write!(w, "Total Electric")?;
        for mi in 0..12 {
            write!(w, ",{:.1}", te_monthly[mi])?;
        }
        writeln!(w, ",{:.1}", te_total)?;

        // Total Gas
        let mut tg_monthly = [0.0_f64; 12];
        let mut tg_total = 0.0_f64;
        for row in &rows {
            if row.label.contains("Gas") {
                for mi in 0..12 {
                    tg_monthly[mi] += row.monthly[mi];
                }
                tg_total += row.total;
            }
        }
        write!(w, "Total Gas")?;
        for mi in 0..12 {
            write!(w, ",{:.1}", tg_monthly[mi])?;
        }
        writeln!(w, ",{:.1}", tg_total)?;

        // Grand Total
        write!(w, "Total")?;
        for mi in 0..12 {
            write!(w, ",{:.1}", te_monthly[mi] + tg_monthly[mi])?;
        }
        writeln!(w, ",{:.1}", te_total + tg_total)?;

        writeln!(w)?;

        // -- Zone Loads [kWh] --
        let annual_heating_kwh: f64 =
            self.monthly.iter().map(|m| m.heating_j).sum::<f64>() / 3_600_000.0;
        let annual_cooling_kwh: f64 =
            self.monthly.iter().map(|m| m.cooling_j).sum::<f64>() / 3_600_000.0;

        writeln!(w, "Zone Loads [kWh]")?;
        write!(w, "Load")?;
        for mn in &month_names {
            write!(w, ",{}", mn)?;
        }
        writeln!(w, ",Total")?;

        write!(w, "Heating")?;
        for me in &self.monthly {
            write!(w, ",{:.1}", me.heating_j / 3_600_000.0)?;
        }
        writeln!(w, ",{:.1}", annual_heating_kwh)?;

        write!(w, "Cooling")?;
        for me in &self.monthly {
            write!(w, ",{:.1}", me.cooling_j / 3_600_000.0)?;
        }
        writeln!(w, ",{:.1}", annual_cooling_kwh)?;

        write!(w, "Total")?;
        for me in &self.monthly {
            write!(w, ",{:.1}", (me.heating_j + me.cooling_j) / 3_600_000.0)?;
        }
        writeln!(w, ",{:.1}", annual_heating_kwh + annual_cooling_kwh)?;

        writeln!(w)?;

        // -- Zone Loads Summary --
        if !self.zone_peak_heating.is_empty() || !self.zone_peak_cooling.is_empty() {
            writeln!(w, "Zone Loads Summary")?;
            writeln!(
                w,
                "Zone Name,Area [m2],Pk Htg [W],W/m2,Htg Time,Pk Clg [W],W/m2,Clg Time,OA [C]"
            )?;

            let month_abbr = [
                "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
            ];

            let mut all_zones: Vec<String> = self
                .zone_peak_heating
                .keys()
                .chain(self.zone_peak_cooling.keys())
                .cloned()
                .collect();
            all_zones.sort();
            all_zones.dedup();

            for zone in &all_zones {
                let area = self.zone_floor_areas.get(zone).copied().unwrap_or(0.0);
                let (h_w, h_mo, h_d, h_hr, _h_oa) = self
                    .zone_peak_heating
                    .get(zone)
                    .copied()
                    .unwrap_or((0.0, 0, 0, 0, 0.0));
                let (c_w, c_mo, c_d, c_hr, c_oa) = self
                    .zone_peak_cooling
                    .get(zone)
                    .copied()
                    .unwrap_or((0.0, 0, 0, 0, 0.0));
                let h_wm2 = if area > 0.0 { h_w / area } else { 0.0 };
                let c_wm2 = if area > 0.0 { c_w / area } else { 0.0 };
                let h_time = if h_w > 0.0 {
                    format!(
                        "{} {} {:02}:00",
                        month_abbr[(h_mo.saturating_sub(1) as usize).min(11)],
                        h_d,
                        h_hr
                    )
                } else {
                    "-".to_string()
                };
                let c_time = if c_w > 0.0 {
                    format!(
                        "{} {} {:02}:00",
                        month_abbr[(c_mo.saturating_sub(1) as usize).min(11)],
                        c_d,
                        c_hr
                    )
                } else {
                    "-".to_string()
                };
                writeln!(
                    w,
                    "{},{:.1},{:.1},{:.1},{},{:.1},{:.1},{},{:.1}",
                    zone, area, h_w, h_wm2, h_time, c_w, c_wm2, c_time, c_oa
                )?;
            }
            writeln!(w)?;
        }

        // -- Unmet Hours --
        writeln!(w, "Unmet Hours")?;
        writeln!(w, "Unmet Heating Hours,{:.1}", self.unmet_heating_hours)?;
        writeln!(w, "Unmet Cooling Hours,{:.1}", self.unmet_cooling_hours)?;
        let total_unmet = self.unmet_heating_hours + self.unmet_cooling_hours;
        writeln!(w, "Total Unmet Hours,{:.1}", total_unmet)?;

        w.flush()?;
        Ok(())
    }
}

/// A single end-use row for the monthly breakdown table.
struct EndUseRow {
    label: &'static str,
    monthly: [f64; 12], // in kWh
    total: f64,         // in kWh
}

/// Return the ASHRAE 90.4 MLC (mechanical load coefficient) limit for a climate zone.
/// Uses the CZ number (first char of the string, e.g. "5" from "5A").
fn ashrae_90_4_mlc_limit(climate_zone: &str) -> f64 {
    let cz_num = climate_zone.chars().next().and_then(|c| c.to_digit(10));
    match cz_num {
        Some(1) | Some(2) => 0.050,
        Some(3) | Some(4) => 0.060,
        Some(5) | Some(6) => 0.070,
        Some(7) | Some(8) => 0.080,
        _ => 0.070, // default to moderate climate when CZ unknown
    }
}

/// Helper to write an HTML table row.
fn html_table_row(w: &mut impl Write, cells: &[&str], is_header: bool) -> std::io::Result<()> {
    let tag = if is_header { "th" } else { "td" };
    write!(w, "<tr>")?;
    for cell in cells {
        write!(w, "<{0}>{1}</{0}>", tag, cell)?;
    }
    writeln!(w, "</tr>")
}

// ─── Legacy CSV Writer (backward compatible) ────────────────────────────────

/// Write simulation results to a CSV file (legacy format from TimestepResult).
///
/// This maintains backward compatibility with the existing output format.
pub fn write_csv(results: &[TimestepResult], path: &Path) -> Result<(), OutputError> {
    if results.is_empty() {
        return Err(OutputError::NoResults);
    }

    // Collect all unique component-variable pairs for column headers
    let mut columns: Vec<(String, String)> = Vec::new();
    for result in results {
        for (comp_name, vars) in &result.component_outputs {
            for var_name in vars.keys() {
                let key = (comp_name.clone(), var_name.clone());
                if !columns.contains(&key) {
                    columns.push(key);
                }
            }
        }
    }
    columns.sort();

    let file = std::fs::File::create(path)
        .map_err(|e| OutputError::IoError(format!("{}: {}", path.display(), e)))?;
    let mut writer = std::io::BufWriter::new(file);

    // Write header with units
    write!(writer, "Month,Day,Hour,SubHour")?;
    for (comp, var) in &columns {
        let unit = get_unit(var);
        write!(writer, ",{}:{} [{}]", comp, var, unit)?;
    }
    writeln!(writer)?;

    // Write data rows
    for result in results {
        write!(
            writer,
            "{},{},{},{}",
            result.month, result.day, result.hour, result.sub_hour
        )?;
        for (comp, var) in &columns {
            let value = result
                .component_outputs
                .get(comp)
                .and_then(|vars| vars.get(var))
                .copied()
                .unwrap_or(0.0);
            write!(writer, ",{:.4}", value)?;
        }
        writeln!(writer)?;
    }

    writer.flush()?;
    Ok(())
}

/// Write results from multiple parametric runs to separate CSV files.
pub fn write_parametric_results(
    run_results: &[(String, Vec<TimestepResult>)],
    output_dir: &Path,
) -> Result<Vec<std::path::PathBuf>, OutputError> {
    std::fs::create_dir_all(output_dir)
        .map_err(|e| OutputError::IoError(format!("{}: {}", output_dir.display(), e)))?;

    let mut paths = Vec::new();
    for (run_name, results) in run_results {
        let filename = format!("{}.csv", run_name);
        let path = output_dir.join(&filename);
        write_csv(results, &path)?;
        paths.push(path);
    }
    Ok(paths)
}

#[derive(Debug, thiserror::Error)]
pub enum OutputError {
    #[error("IO error: {0}")]
    IoError(String),
    #[error("No results to write")]
    NoResults,
}

impl From<std::io::Error> for OutputError {
    fn from(e: std::io::Error) -> Self {
        OutputError::IoError(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_available_variables_have_units() {
        let vars = available_variables();
        assert!(!vars.is_empty());
        for (name, unit, desc) in &vars {
            assert!(!name.is_empty(), "Variable name is empty");
            assert!(!unit.is_empty(), "Unit for {} is empty", name);
            assert!(!desc.is_empty(), "Description for {} is empty", name);
        }
    }

    #[test]
    fn test_get_unit() {
        assert_eq!(get_unit("zone:temperature"), "\u{00b0}C");
        assert_eq!(get_unit("zone:heating_rate"), "W");
        assert_eq!(get_unit("zone:heating_energy"), "J");
        assert_eq!(get_unit("site:outdoor_temperature"), "\u{00b0}C");
        assert_eq!(get_unit("unknown:var"), "-");
        // Zone catch-all returns W
        assert_eq!(get_unit("zone:heating_load"), "W");
        // Component temperatures
        assert_eq!(get_unit("component:outlet_temperature"), "\u{00b0}C");
        // AFN outputs (#89)
        assert_eq!(get_unit("zone:pressure"), "Pa");
        assert_eq!(get_unit("zone:species_co2"), "kg/kg");
    }

    /// AFN output variables (#89): zone pressure and dynamic per-species
    /// concentrations resolve through the zone category.
    #[test]
    fn test_zone_pressure_and_species_outputs() {
        let mut snap = OutputSnapshot::new(1, 1, 1, 1, 3600.0);
        snap.zone_pressure.insert("Living".to_string(), -2.5);
        snap.zone_species
            .entry("co2".to_string())
            .or_default()
            .insert("Living".to_string(), 0.00095);

        let vals = snap.get_variable_values("zone:pressure");
        assert_eq!(vals.get("Living"), Some(&-2.5));

        let vals = snap.get_variable_values("zone:species_co2");
        assert_eq!(vals.get("Living"), Some(&0.00095));

        // Unknown species yields no values (not a panic)
        let vals = snap.get_variable_values("zone:species_radon");
        assert!(vals.is_empty());
    }

    #[test]
    fn test_output_snapshot_site_variables() {
        let mut snap = OutputSnapshot::new(1, 1, 1, 1, 3600.0);
        snap.site_outdoor_temperature = -5.0;
        snap.site_wind_speed = 3.5;

        let vals = snap.get_variable_values("site:outdoor_temperature");
        assert_eq!(vals.get("Site"), Some(&-5.0));

        let vals = snap.get_variable_values("site:wind_speed");
        assert_eq!(vals.get("Site"), Some(&3.5));
    }

    #[test]
    fn test_output_snapshot_zone_energy_integration() {
        let mut snap = OutputSnapshot::new(1, 1, 1, 1, 900.0); // 15-min timestep
        snap.zone_heating_rate.insert("Zone1".to_string(), 1000.0); // 1000W

        let energy = snap.get_variable_values("zone:heating_energy");
        // 1000W * 900s = 900000 J
        assert_eq!(energy.get("Zone1"), Some(&900_000.0));
    }

    #[test]
    fn test_output_writer_timestep_frequency() {
        let config = OutputFileConfig {
            file: "test.csv".to_string(),
            frequency: OutputFrequency::Timestep,
            aggregation: Aggregation::Mean,
            variables: vec!["site:outdoor_temperature".to_string()],
        };
        let mut writer = OutputWriter::new(config);

        let mut snap1 = OutputSnapshot::new(1, 1, 1, 1, 3600.0);
        snap1.site_outdoor_temperature = -5.0;
        writer.add_snapshot(&snap1);

        let mut snap2 = OutputSnapshot::new(1, 1, 1, 2, 3600.0);
        snap2.site_outdoor_temperature = -4.0;
        writer.add_snapshot(&snap2);

        assert_eq!(writer.rows.len(), 2);
    }

    #[test]
    fn test_output_writer_hourly_aggregation() {
        let config = OutputFileConfig {
            file: "test.csv".to_string(),
            frequency: OutputFrequency::Hourly,
            aggregation: Aggregation::Mean,
            variables: vec!["site:outdoor_temperature".to_string()],
        };
        let mut writer = OutputWriter::new(config);

        // 4 sub-hourly timesteps in hour 1
        for sub in 1..=4 {
            let mut snap = OutputSnapshot::new(1, 1, 1, sub, 900.0);
            snap.site_outdoor_temperature = sub as f64; // 1, 2, 3, 4
            writer.add_snapshot(&snap);
        }

        // Start hour 2 to flush hour 1
        let mut snap = OutputSnapshot::new(1, 1, 2, 1, 900.0);
        snap.site_outdoor_temperature = 10.0;
        writer.add_snapshot(&snap);

        // Hour 1 should be flushed with mean = 2.5
        assert_eq!(writer.rows.len(), 1);
        assert!((writer.rows[0].values[0] - 2.5).abs() < 0.01);
    }

    #[test]
    fn test_summary_report_monthly_energy() {
        let mut heating_sp = HashMap::new();
        heating_sp.insert("Zone1".to_string(), 20.0);
        let mut cooling_sp = HashMap::new();
        cooling_sp.insert("Zone1".to_string(), 27.0);

        let mut report = SummaryReport::new(heating_sp, cooling_sp);

        // January: 100W heating for 10 hours
        for h in 1..=10 {
            let mut snap = OutputSnapshot::new(1, 1, h, 1, 3600.0);
            snap.zone_heating_rate.insert("Zone1".to_string(), 100.0);
            snap.zone_cooling_rate.insert("Zone1".to_string(), 0.0);
            snap.zone_temperature.insert("Zone1".to_string(), 20.5);
            report.add_snapshot(&snap);
        }

        // Jan heating = 100W * 3600s * 10 = 3,600,000 J = 1.0 kWh
        let jan_kwh = report.monthly[0].heating_j / 3_600_000.0;
        assert!((jan_kwh - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_summary_report_unmet_hours() {
        let mut heating_sp = HashMap::new();
        heating_sp.insert("Zone1".to_string(), 20.0);
        let cooling_sp = HashMap::new();

        let mut report = SummaryReport::new(heating_sp, cooling_sp);

        // Zone at 19.0 C (below 20.0 - 0.2 = 19.8 C tolerance)
        let mut snap = OutputSnapshot::new(1, 1, 1, 1, 3600.0);
        snap.zone_temperature.insert("Zone1".to_string(), 19.0);
        snap.zone_heating_rate.insert("Zone1".to_string(), 0.0);
        snap.zone_cooling_rate.insert("Zone1".to_string(), 0.0);
        report.add_snapshot(&snap);

        assert!((report.unmet_heating_hours - 1.0).abs() < 0.01);

        // Zone at 19.9 C (above 19.8 C tolerance -- NOT unmet)
        let mut snap2 = OutputSnapshot::new(1, 1, 2, 1, 3600.0);
        snap2.zone_temperature.insert("Zone1".to_string(), 19.9);
        snap2.zone_heating_rate.insert("Zone1".to_string(), 0.0);
        snap2.zone_cooling_rate.insert("Zone1".to_string(), 0.0);
        report.add_snapshot(&snap2);

        // Should still be 1.0 (second snapshot was within tolerance)
        assert!((report.unmet_heating_hours - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_output_frequency_serde() {
        let yaml = r#"
file: "test.csv"
frequency: daily
aggregation: sum
variables:
  - zone_heating_energy
"#;
        let config: OutputFileConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.frequency, OutputFrequency::Daily);
        assert_eq!(config.aggregation, Aggregation::Sum);
        assert_eq!(config.variables, vec!["zone_heating_energy"]);
    }

    #[test]
    fn test_per_component_variable_resolution() {
        let mut snap = OutputSnapshot::new(1, 1, 1, 1, 3600.0);
        let mut boiler_vars = HashMap::new();
        boiler_vars.insert("fuel_power".to_string(), 50000.0);
        boiler_vars.insert("plr".to_string(), 0.75);
        boiler_vars.insert("water_outlet_temperature".to_string(), 82.0);
        snap.component_outputs
            .insert("Boiler-1".to_string(), boiler_vars);

        // Resolve "component:fuel_power:Boiler-1" — specific component filter
        let vals = snap.get_variable_values("component:fuel_power:Boiler-1");
        assert_eq!(vals.get("Boiler-1"), Some(&50000.0));

        // Resolve "component:plr:Boiler-1"
        let vals = snap.get_variable_values("component:plr:Boiler-1");
        assert_eq!(vals.get("Boiler-1"), Some(&0.75));

        // Resolve "component:water_outlet_temperature:Boiler-1"
        let vals = snap.get_variable_values("component:water_outlet_temperature:Boiler-1");
        assert_eq!(vals.get("Boiler-1"), Some(&82.0));

        // Unknown variable returns empty
        let vals = snap.get_variable_values("component:nonexistent:Boiler-1");
        assert!(vals.is_empty());

        // Unknown component filter returns empty
        let vals = snap.get_variable_values("component:fuel_power:FakeComp");
        assert!(vals.is_empty());
    }

    #[test]
    fn test_total_electric_counts_odd_named_components_once() {
        // Regression for #74: building:total_electric must include components
        // whose names don't match any fan/cool/heat substring (e.g. "VRF-1"),
        // and must not double-count names that hit multiple buckets.
        let mut snap = OutputSnapshot::new(1, 1, 1, 1, 3600.0);
        snap.component_electric_power
            .insert("VRF-1".to_string(), 1000.0); // odd name, no substring match
        snap.component_electric_power
            .insert("Supply Fan".to_string(), 200.0);
        // Name hits both "cool" and "heat" — must be counted exactly once.
        snap.component_electric_power
            .insert("Heat-Cool Unit".to_string(), 300.0);

        // total_electric must equal the plain sum of every electric component.
        let total = snap.get_variable_values("building:total_electric");
        assert_eq!(total.get("Building"), Some(&1500.0));

        // The odd-named VRF lands in "Other" via the name fallback (no kind set).
        assert_eq!(elec_end_use(None, "VRF-1"), ElecEndUse::Other);
        // With a kind, VRF routes to Cooling.
        assert_eq!(
            elec_end_use(Some(ComponentKind::VrfOutdoor), "VRF-1"),
            ElecEndUse::Cooling
        );

        // Summary report: the annual total is the sum of its category rows, so
        // VRF-1 must survive into "Other (Electric)" and be counted in the total.
        let mut report = SummaryReport::new(HashMap::new(), HashMap::new());
        report.add_snapshot(&snap);
        let rows = report.compute_enduse_rows();
        let other = rows.iter().find(|r| r.label == "Other (Electric)").unwrap();
        assert!((other.total - 1.0).abs() < 1e-9); // 1000 W * 3600 s = 1 kWh
        let elec_total: f64 = rows
            .iter()
            .filter(|r| !r.label.contains("Gas"))
            .map(|r| r.total)
            .sum();
        // 1500 W * 3600 s = 1.5 kWh total electric.
        assert!((elec_total - 1.5).abs() < 1e-9);
    }

    #[test]
    fn test_component_variable_units() {
        assert_eq!(get_unit("component:electric_power"), "W");
        assert_eq!(get_unit("component:outlet_temperature"), "\u{00b0}C");
        assert_eq!(get_unit("component:pressure_rise"), "Pa");
        assert_eq!(get_unit("component:plr"), "-");
        assert_eq!(get_unit("component:water_mass_flow"), "kg/s");
        assert_eq!(get_unit("component:inlet_humidity_ratio"), "kg/kg");
        assert_eq!(get_unit("component:inlet_enthalpy"), "J/kg");
    }

    #[test]
    fn test_component_variable_is_integrable() {
        assert!(is_integrable("component:electric_power"));
        assert!(is_integrable("component:fuel_power"));
        assert!(is_integrable("component:sensible_load"));
        assert!(is_integrable("component:conduction_loss"));
        assert!(!is_integrable("component:plr"));
        assert!(!is_integrable("component:outlet_temperature"));
    }

    #[test]
    fn test_zone_gain_variables() {
        let mut snap = OutputSnapshot::new(1, 1, 1, 1, 3600.0);
        snap.zone_gain_people_sensible
            .insert("Zone1".to_string(), 200.0);
        snap.zone_gain_lighting.insert("Zone1".to_string(), 500.0);
        snap.zone_gain_solar.insert("Zone1".to_string(), 1000.0);
        snap.zone_gain_hvac_sensible
            .insert("Zone1".to_string(), -3000.0);

        let vals = snap.get_variable_values("zone:gain_people_sensible");
        assert_eq!(vals.get("Zone1"), Some(&200.0));

        let vals = snap.get_variable_values("zone:gain_lighting");
        assert_eq!(vals.get("Zone1"), Some(&500.0));

        let vals = snap.get_variable_values("zone:gain_solar");
        assert_eq!(vals.get("Zone1"), Some(&1000.0));

        let vals = snap.get_variable_values("zone:gain_hvac_sensible");
        assert_eq!(vals.get("Zone1"), Some(&-3000.0));
    }

    #[test]
    fn test_mrt_and_operative_temperature() {
        let mut snap = OutputSnapshot::new(7, 15, 14, 1, 3600.0);
        // Set known MRT and operative temp
        snap.zone_mean_radiant_temperature
            .insert("Zone1".to_string(), 26.0);
        snap.zone_operative_temperature
            .insert("Zone1".to_string(), 25.0); // (24 + 26) / 2

        let vals = snap.get_variable_values("zone:mean_radiant_temperature");
        assert_eq!(vals.get("Zone1"), Some(&26.0));

        let vals = snap.get_variable_values("zone:operative_temperature");
        assert_eq!(vals.get("Zone1"), Some(&25.0));
    }

    #[test]
    fn test_unmet_hours_variables() {
        let mut snap = OutputSnapshot::new(1, 15, 8, 1, 3600.0);
        snap.zone_unmet_heating.insert("Zone1".to_string(), 1.0);
        snap.zone_unmet_cooling.insert("Zone1".to_string(), 0.0);

        let vals = snap.get_variable_values("zone:unmet_heating");
        assert_eq!(vals.get("Zone1"), Some(&1.0));

        let vals = snap.get_variable_values("zone:unmet_cooling");
        assert_eq!(vals.get("Zone1"), Some(&0.0));
    }

    #[test]
    fn test_surface_conduction_gain_alias() {
        let mut snap = OutputSnapshot::new(1, 1, 1, 1, 3600.0);
        snap.surface_conduction_inside
            .insert("Wall-N".to_string(), -150.0);

        // "surface:cond_inside:Wall-N" resolves to surface_conduction_inside for that surface
        let vals = snap.get_variable_values("surface:cond_inside:Wall-N");
        assert_eq!(vals.get("Wall-N"), Some(&-150.0));
    }

    #[test]
    fn test_peak_coincident_conditions() {
        let mut report = SummaryReport::new(
            vec![("Zone1".to_string(), 20.0)].into_iter().collect(),
            vec![("Zone1".to_string(), 25.0)].into_iter().collect(),
        );

        let mut snap = OutputSnapshot::new(1, 15, 8, 1, 3600.0);
        snap.zone_heating_rate.insert("Zone1".to_string(), 5000.0);
        snap.zone_cooling_rate.insert("Zone1".to_string(), 0.0);
        snap.zone_temperature.insert("Zone1".to_string(), 18.0);
        snap.site_outdoor_temperature = -10.0;
        snap.site_wind_speed = 5.5;
        report.add_snapshot(&snap);

        // Check that peak heating captured coincident conditions
        assert!((report.peak_heating.0 - 5000.0).abs() < 0.1);
        assert!((report.peak_heating.4 - (-10.0)).abs() < 0.1); // outdoor temp
        assert!((report.peak_heating.5 - 18.0).abs() < 0.1); // zone temp
        assert!((report.peak_heating.6 - 5.5).abs() < 0.1); // wind speed
    }

    #[test]
    fn test_submeter_variable_resolution() {
        let mut snap = OutputSnapshot::new(1, 1, 1, 1, 3600.0);
        let mut general = HashMap::new();
        general.insert("lighting".to_string(), 500.0);
        general.insert("fan_electric".to_string(), 200.0);
        snap.submeter_power.insert("General".to_string(), general);

        let mut decorative = HashMap::new();
        decorative.insert("lighting".to_string(), 100.0);
        snap.submeter_power
            .insert("Decorative".to_string(), decorative);

        // Specific meter — format: submeter:end_use:meter_name (name last, filtered)
        let vals = snap.get_variable_values("submeter:lighting:General");
        assert_eq!(vals.get("General"), Some(&500.0));

        let vals = snap.get_variable_values("submeter:lighting:Decorative");
        assert_eq!(vals.get("Decorative"), Some(&100.0));

        // All meters for an end use (no filter)
        let vals = snap.get_variable_values("submeter:lighting");
        assert_eq!(vals.get("General"), Some(&500.0));
        assert_eq!(vals.get("Decorative"), Some(&100.0));

        // Total electric for a specific meter
        let vals = snap.get_variable_values("submeter:total_electric:General");
        assert_eq!(vals.get("General"), Some(&700.0)); // 500 + 200

        // Total for a specific meter
        let vals = snap.get_variable_values("submeter:total:Decorative");
        assert_eq!(vals.get("Decorative"), Some(&100.0));

        // Non-existent meter filter returns empty
        let vals = snap.get_variable_values("submeter:lighting:NonExistent");
        assert!(vals.is_empty());
    }

    #[test]
    fn test_submeter_units_and_aggregation() {
        assert_eq!(get_unit("submeter:lighting:General"), "W");
        assert_eq!(get_unit("submeter:equipment:Kitchen"), "W");
        assert!(is_integrable("submeter:fan_electric:General"));
        assert!(is_integrable("submeter:lighting:Decorative"));
    }

    #[test]
    fn test_submeter_summary_accumulation() {
        let mut report = SummaryReport::new(HashMap::new(), HashMap::new());

        let mut snap = OutputSnapshot::new(1, 15, 12, 1, 3600.0);
        let mut general = HashMap::new();
        general.insert("lighting".to_string(), 1000.0); // 1000W
        snap.submeter_power.insert("General".to_string(), general);
        let mut decorative = HashMap::new();
        decorative.insert("lighting".to_string(), 250.0); // 250W
        snap.submeter_power
            .insert("Decorative".to_string(), decorative);
        report.add_snapshot(&snap);

        // Check that monthly submeter energy was accumulated
        let gen_light = report
            .submeter_monthly_j
            .get(&("General".to_string(), "lighting".to_string()));
        assert!(gen_light.is_some());
        // 1000W * 3600s = 3,600,000 J in January (index 0)
        assert!((gen_light.unwrap()[0] - 3_600_000.0).abs() < 1.0);

        let dec_light = report
            .submeter_monthly_j
            .get(&("Decorative".to_string(), "lighting".to_string()));
        assert!(dec_light.is_some());
        assert!((dec_light.unwrap()[0] - 900_000.0).abs() < 1.0); // 250W * 3600s
    }

    #[test]
    fn test_ashrae_90_4_mlc_limits() {
        assert!((ashrae_90_4_mlc_limit("1A") - 0.050).abs() < 1e-9);
        assert!((ashrae_90_4_mlc_limit("2B") - 0.050).abs() < 1e-9);
        assert!((ashrae_90_4_mlc_limit("3C") - 0.060).abs() < 1e-9);
        assert!((ashrae_90_4_mlc_limit("5A") - 0.070).abs() < 1e-9);
        assert!((ashrae_90_4_mlc_limit("7") - 0.080).abs() < 1e-9);
        assert!((ashrae_90_4_mlc_limit("8A") - 0.080).abs() < 1e-9);
        // Unknown CZ defaults to moderate
        assert!((ashrae_90_4_mlc_limit("Unknown") - 0.070).abs() < 1e-9);
    }

    #[test]
    fn test_it_equipment_power_accumulation() {
        let mut report = SummaryReport::new(HashMap::new(), HashMap::new());
        let mut snap = OutputSnapshot::new(1, 15, 12, 1, 3600.0);
        snap.it_equipment_power
            .insert("DataHall".to_string(), 500_000.0); // 500 kW
        snap.elec_dist_power = 25_000.0; // 25 kW UPS losses
        report.add_snapshot(&snap);

        let annual_it: f64 = report.monthly.iter().map(|m| m.it_equipment_j).sum();
        let annual_elec_dist: f64 = report.monthly.iter().map(|m| m.elec_dist_j).sum();
        // 500kW * 3600s = 1,800,000,000 J
        assert!((annual_it - 1_800_000_000.0).abs() < 1.0);
        // 25kW * 3600s = 90,000,000 J
        assert!((annual_elec_dist - 90_000_000.0).abs() < 1.0);
    }
}
