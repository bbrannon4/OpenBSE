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
//! Variables follow a hierarchical `<category>_<quantity>` pattern:
//!
//! | Category       | Description                        | Example                       |
//! |----------------|------------------------------------|-------------------------------|
//! | `zone_`        | Zone air properties and loads       | `zone_temperature`            |
//! | `surface_`     | Surface temps and heat transfer     | `surface_inside_temperature`  |
//! | `air_loop_`    | Air system level                   | `air_loop_outlet_temperature` |
//! | `site_`        | Outdoor/weather conditions          | `site_outdoor_temperature`    |

use openbse_core::simulation::TimestepResult;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Write;
use std::path::Path;

// ─── Output Configuration ────────────────────────────────────────────────────

/// Reporting frequency for output files.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputFrequency {
    Timestep,
    Hourly,
    Daily,
    Monthly,
    RunPeriod,
}

impl Default for OutputFrequency {
    fn default() -> Self {
        OutputFrequency::Hourly
    }
}

/// Aggregation method when downsampling from timestep to lower frequencies.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Aggregation {
    Mean,
    Sum,
    Min,
    Max,
}

impl Default for Aggregation {
    fn default() -> Self {
        Aggregation::Mean
    }
}

/// User-defined output file configuration.
///
/// ```yaml
/// outputs:
///   - file: "zone_results.csv"
///     frequency: hourly
///     variables:
///       - zone_temperature
///       - zone_heating_rate
///       - zone_cooling_rate
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
/// Returns (variable_name, unit_string, description).
pub fn available_variables() -> Vec<(&'static str, &'static str, &'static str)> {
    vec![
        // Zone variables
        ("zone_temperature", "°C", "Zone air dry-bulb temperature"),
        ("zone_humidity_ratio", "kg/kg", "Zone air humidity ratio"),
        ("zone_heating_rate", "W", "Zone heating load (positive = needs heating)"),
        ("zone_cooling_rate", "W", "Zone cooling load (positive = needs cooling)"),
        ("zone_heating_energy", "J", "Zone heating energy (integrated from rate)"),
        ("zone_cooling_energy", "J", "Zone cooling energy (integrated from rate)"),
        ("zone_infiltration_mass_flow", "kg/s", "Zone infiltration air mass flow rate"),
        ("zone_nat_vent_flow", "m³/s", "Zone natural ventilation volume flow rate"),
        ("zone_nat_vent_mass_flow", "kg/s", "Zone natural ventilation mass flow rate"),
        ("zone_nat_vent_active", "", "Zone natural ventilation active (1=yes, 0=no)"),
        ("zone_internal_gains_convective", "W", "Zone convective internal gains"),
        ("zone_internal_gains_radiative", "W", "Zone radiative internal gains"),
        ("zone_supply_air_temperature", "°C", "HVAC supply air temperature to zone"),
        ("zone_supply_air_mass_flow", "kg/s", "HVAC supply air mass flow to zone"),

        // Surface variables
        ("surface_inside_temperature", "°C", "Surface inside face temperature"),
        ("surface_outside_temperature", "°C", "Surface outside face temperature"),
        ("surface_inside_convection_coefficient", "W/(m²·K)", "Inside convection coefficient"),
        ("surface_incident_solar", "W/m²", "Incident solar radiation on surface"),
        ("surface_transmitted_solar", "W", "Solar transmitted through window"),

        // Site/weather variables
        ("site_outdoor_temperature", "°C", "Outdoor dry-bulb temperature"),
        ("site_wind_speed", "m/s", "Wind speed"),
        ("site_direct_normal_radiation", "W/m²", "Direct normal solar radiation"),
        ("site_diffuse_horizontal_radiation", "W/m²", "Diffuse horizontal solar radiation"),
        ("site_relative_humidity", "%", "Outdoor relative humidity"),

        // Air loop / HVAC component variables
        ("air_loop_outlet_temperature", "°C", "Air loop outlet temperature"),
        ("air_loop_mass_flow", "kg/s", "Air loop mass flow rate"),
        ("air_loop_outlet_humidity_ratio", "kg/kg", "Air loop outlet humidity ratio"),
    ]
}

/// Get the unit string for a variable name.
pub fn get_unit(var_name: &str) -> &'static str {
    for (name, unit, _) in available_variables() {
        if name == var_name {
            return unit;
        }
    }
    // Legacy variable name support
    match var_name {
        "zone_temp" | "outdoor_temp" | "outlet_temp" | "supply_air_temp" => "°C",
        "mass_flow" | "supply_air_mass_flow" | "infiltration_mass_flow" => "kg/s",
        "outlet_w" => "kg/kg",
        "heating_load" | "cooling_load" | "q_internal_conv" | "q_internal_rad" => "W",
        "outlet_enthalpy" => "J/kg",
        _ => "-",
    }
}

/// Whether a variable should default to sum aggregation (energy, mass).
fn is_integrable(var_name: &str) -> bool {
    matches!(var_name,
        "zone_heating_energy" | "zone_cooling_energy"
    )
}

// ─── Timestep Data Collector ────────────────────────────────────────────────

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
    // Internal gains by type (zone_name -> watts)
    pub zone_lighting_power: HashMap<String, f64>,
    pub zone_equipment_power: HashMap<String, f64>,

    // Typed end-use maps — separate from generic component_electric/fuel_power
    // so the summary report can categorize without fragile name-matching
    pub dhw_electric_power: HashMap<String, f64>,
    pub dhw_fuel_power: HashMap<String, f64>,
    pub ext_lighting_power: HashMap<String, f64>,
    pub ext_equipment_power: HashMap<String, f64>,
    pub pump_electric_power: HashMap<String, f64>,
    pub heat_rejection_power: HashMap<String, f64>,
    pub humidification_power: HashMap<String, f64>,
}

impl OutputSnapshot {
    /// Create a snapshot with default/zero values.
    pub fn new(month: u32, day: u32, hour: u32, sub_hour: u32, dt: f64) -> Self {
        Self {
            month, day, hour, sub_hour, dt,
            site_outdoor_temperature: 0.0,
            site_wind_speed: 0.0,
            site_direct_normal_radiation: 0.0,
            site_diffuse_horizontal_radiation: 0.0,
            site_relative_humidity: 0.0,
            zone_temperature: HashMap::new(),
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
            zone_heating_setpoint: HashMap::new(),
            zone_cooling_setpoint: HashMap::new(),
            air_loop_outlet_temperature: HashMap::new(),
            air_loop_mass_flow: HashMap::new(),
            air_loop_outlet_humidity_ratio: HashMap::new(),
            component_electric_power: HashMap::new(),
            component_fuel_power: HashMap::new(),
            zone_lighting_power: HashMap::new(),
            zone_equipment_power: HashMap::new(),
            dhw_electric_power: HashMap::new(),
            dhw_fuel_power: HashMap::new(),
            ext_lighting_power: HashMap::new(),
            ext_equipment_power: HashMap::new(),
            pump_electric_power: HashMap::new(),
            heat_rejection_power: HashMap::new(),
            humidification_power: HashMap::new(),
        }
    }

    /// Get all values for a variable (returns entity_name -> value pairs).
    ///
    /// For zone variables, returns one value per zone.
    /// For surface variables, returns one value per surface.
    /// For site variables, returns a single value with key "Site".
    fn get_variable_values(&self, var_name: &str) -> HashMap<String, f64> {
        match var_name {
            // Site (scalar)
            "site_outdoor_temperature" => single("Site", self.site_outdoor_temperature),
            "site_wind_speed" => single("Site", self.site_wind_speed),
            "site_direct_normal_radiation" => single("Site", self.site_direct_normal_radiation),
            "site_diffuse_horizontal_radiation" => single("Site", self.site_diffuse_horizontal_radiation),
            "site_relative_humidity" => single("Site", self.site_relative_humidity),

            // Zone
            "zone_temperature" => self.zone_temperature.clone(),
            "zone_humidity_ratio" => self.zone_humidity_ratio.clone(),
            "zone_heating_rate" => self.zone_heating_rate.clone(),
            "zone_cooling_rate" => self.zone_cooling_rate.clone(),
            "zone_heating_energy" => {
                // Integrate rate * dt -> energy [J]
                self.zone_heating_rate.iter()
                    .map(|(k, v)| (k.clone(), v * self.dt))
                    .collect()
            }
            "zone_cooling_energy" => {
                self.zone_cooling_rate.iter()
                    .map(|(k, v)| (k.clone(), v * self.dt))
                    .collect()
            }
            "zone_infiltration_mass_flow" => self.zone_infiltration_mass_flow.clone(),
            "zone_nat_vent_flow" => self.zone_nat_vent_flow.clone(),
            "zone_nat_vent_mass_flow" => self.zone_nat_vent_mass_flow.clone(),
            "zone_nat_vent_active" => self.zone_nat_vent_active.clone(),
            "zone_internal_gains_convective" => self.zone_internal_gains_convective.clone(),
            "zone_internal_gains_radiative" => self.zone_internal_gains_radiative.clone(),
            "zone_supply_air_temperature" => self.zone_supply_air_temperature.clone(),
            "zone_supply_air_mass_flow" => self.zone_supply_air_mass_flow.clone(),

            // Surface
            "surface_inside_temperature" => self.surface_inside_temperature.clone(),
            "surface_outside_temperature" => self.surface_outside_temperature.clone(),
            "surface_inside_convection_coefficient" => self.surface_inside_convection_coefficient.clone(),
            "surface_incident_solar" => self.surface_incident_solar.clone(),
            "surface_transmitted_solar" => self.surface_transmitted_solar.clone(),

            // Air loop / HVAC
            "air_loop_outlet_temperature" => self.air_loop_outlet_temperature.clone(),
            "air_loop_mass_flow" => self.air_loop_mass_flow.clone(),
            "air_loop_outlet_humidity_ratio" => self.air_loop_outlet_humidity_ratio.clone(),

            _ => HashMap::new(),
        }
    }
}

fn single(key: &str, value: f64) -> HashMap<String, f64> {
    let mut m = HashMap::new();
    m.insert(key.to_string(), value);
    m
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
            accum: Vec::new(),
            current_period: None,
            rows: Vec::new(),
            columns_resolved: false,
        }
    }

    /// Discover columns from the first snapshot.
    fn resolve_columns(&mut self, snapshot: &OutputSnapshot) {
        if self.columns_resolved {
            return;
        }

        for var_name in &self.config.variables {
            let values = snapshot.get_variable_values(var_name);
            let mut entity_names: Vec<String> = values.keys().cloned().collect();
            entity_names.sort();

            if entity_names.is_empty() {
                // Variable not found — skip silently (might appear later)
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
                self.flush_period(snapshot.month, snapshot.day, snapshot.hour, snapshot.sub_hour);
            }
        }
        self.current_period = Some(period);

        // Accumulate values
        for (i, (var_name, entity_name)) in self.columns.iter().enumerate() {
            let values = snapshot.get_variable_values(var_name);
            let val = values.get(entity_name).copied().unwrap_or(0.0);

            self.accum[i].0 += val;      // sum
            self.accum[i].1 += 1;        // count
            if val < self.accum[i].2 { self.accum[i].2 = val; }  // min
            if val > self.accum[i].3 { self.accum[i].3 = val; }  // max
        }

        // For timestep frequency, flush immediately
        if self.config.frequency == OutputFrequency::Timestep {
            self.flush_period(snapshot.month, snapshot.day, snapshot.hour, snapshot.sub_hour);
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

        self.rows.push(OutputRow { month, day, hour, sub_hour, values });

        // Reset accumulators
        for acc in &mut self.accum {
            *acc = (0.0, 0, f64::MAX, f64::MIN);
        }
        self.current_period = None;
    }

    /// Finalize (flush any remaining data) and write to file.
    pub fn finalize_and_write(&mut self, output_dir: &Path) -> Result<(), OutputError> {
        // Flush any remaining accumulated data
        if let Some((m, d, h)) = self.current_period {
            self.flush_period(m, d, h, 0);
        }

        if self.rows.is_empty() {
            return Ok(()); // No data to write
        }

        let path = output_dir.join(&self.config.file);
        self.write_to_path(&path)
    }

    /// Finalize and write, prepending `stem_` to the configured filename.
    /// E.g. config.file = "zone_output.csv" → "retail_rtu_zone_output.csv"
    pub fn finalize_and_write_prefixed(&mut self, output_dir: &Path, stem: &str) -> Result<(), OutputError> {
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
        for (var_name, entity_name) in &self.columns {
            let unit = get_unit(var_name);
            if entity_name == "Site" {
                write!(writer, ",{} [{}]", var_name, unit)?;
            } else {
                write!(writer, ",{}:{} [{}]", entity_name, var_name, unit)?;
            }
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
    heating_j: f64,               // Total zone heating loads [J]
    cooling_j: f64,               // Total zone cooling loads [J]
    hours: f64,                   // Number of hours in data
    // Electric end uses
    fan_elec_j: f64,              // Fan electric [J]
    cool_elec_j: f64,             // Cooling electric (DX compressor, chiller) [J]
    heat_elec_j: f64,             // Heating electric (electric coil, HP compressor) [J]
    pump_elec_j: f64,             // Pump electric [J]
    heat_rejection_elec_j: f64,   // Cooling tower fan electric [J]
    humidification_elec_j: f64,   // Humidifier electric [J]
    dhw_elec_j: f64,              // DHW electric (water heater) [J]
    lighting_j: f64,              // Interior lighting [J]
    ext_lighting_j: f64,          // Exterior lighting [J]
    equipment_j: f64,             // Interior equipment/plug loads [J]
    ext_equipment_j: f64,         // Exterior equipment [J]
    // Gas end uses
    heat_gas_j: f64,              // Heating gas (boiler, gas furnace) [J]
    dhw_gas_j: f64,               // DHW gas (gas water heater) [J]
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
    /// Peak heating rate [W] and when it occurred
    peak_heating: (f64, u32, u32, u32), // (watts, month, day, hour)
    /// Peak cooling rate [W] and when it occurred
    peak_cooling: (f64, u32, u32, u32),
    /// Total window transmitted solar energy [J] (for diagnostics)
    total_transmitted_solar_j: f64,
    /// Total window incident solar energy [J] (for diagnostics)
    total_incident_solar_j: f64,
    /// Monthly transmitted solar [J] (12 months)
    monthly_transmitted_solar_j: [f64; 12],
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
            peak_heating: (0.0, 0, 0, 0),
            peak_cooling: (0.0, 0, 0, 0),
            total_transmitted_solar_j: 0.0,
            total_incident_solar_j: 0.0,
            monthly_transmitted_solar_j: [0.0; 12],
        }
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

        // Track peaks
        if total_heating > self.peak_heating.0 {
            self.peak_heating = (total_heating, snapshot.month, snapshot.day, snapshot.hour);
        }
        if total_cooling > self.peak_cooling.0 {
            self.peak_cooling = (total_cooling, snapshot.month, snapshot.day, snapshot.hour);
        }

        // Accumulate energy end-use breakdown using typed snapshot fields
        // (avoids fragile name-based matching for DHW, ext equip, pumps, etc.)

        // 1. Typed end-use maps — DHW, exterior, pumps, heat rejection, humidification
        for &pw in snapshot.dhw_electric_power.values() {
            let energy = pw * snapshot.dt;
            if energy.is_finite() { me.dhw_elec_j += energy; }
        }
        for &pw in snapshot.dhw_fuel_power.values() {
            let energy = pw * snapshot.dt;
            if energy.is_finite() { me.dhw_gas_j += energy; }
        }
        for &pw in snapshot.ext_lighting_power.values() {
            let energy = pw * snapshot.dt;
            if energy.is_finite() { me.ext_lighting_j += energy; }
        }
        for &pw in snapshot.ext_equipment_power.values() {
            let energy = pw * snapshot.dt;
            if energy.is_finite() { me.ext_equipment_j += energy; }
        }
        for &pw in snapshot.pump_electric_power.values() {
            let energy = pw * snapshot.dt;
            if energy.is_finite() { me.pump_elec_j += energy; }
        }
        for &pw in snapshot.heat_rejection_power.values() {
            let energy = pw * snapshot.dt;
            if energy.is_finite() { me.heat_rejection_elec_j += energy; }
        }
        for &pw in snapshot.humidification_power.values() {
            let energy = pw * snapshot.dt;
            if energy.is_finite() { me.humidification_elec_j += energy; }
        }

        // 2. Generic HVAC component power — name-based matching for fans, coils, plant equip
        //    Pumps, ext equipment, DHW, etc. are handled by typed maps above.
        //    Unknown components are ignored (no fallback to cooling).
        for (comp_name, &pw) in &snapshot.component_electric_power {
            let lname = comp_name.to_lowercase();
            let energy = pw * snapshot.dt;
            if !energy.is_finite() { continue; }
            if lname.contains("fan") {
                me.fan_elec_j += energy;
            } else if lname.contains("cool") || lname.contains("dx") || lname.contains("chiller")
                    || lname.starts_with("cc ") || lname.starts_with("cc_") {
                me.cool_elec_j += energy;
            } else if lname.contains("heat") || lname.contains("furnace")
                    || lname.starts_with("hc ") || lname.starts_with("hc_") {
                me.heat_elec_j += energy;
            }
            // else: unrecognized components are not categorized
            // (pumps, ext equipment, DHW handled via typed snapshot fields)
        }
        for (comp_name, &pw) in &snapshot.component_fuel_power {
            let lname = comp_name.to_lowercase();
            let energy = pw * snapshot.dt;
            if !energy.is_finite() { continue; }
            if lname.contains("boiler") || lname.contains("heat") || lname.contains("furnace") {
                me.heat_gas_j += energy;
            }
        }

        // 3. Zone internal gains — interior lighting and equipment
        for &pw in snapshot.zone_lighting_power.values() {
            me.lighting_j += pw * snapshot.dt;
        }
        for &pw in snapshot.zone_equipment_power.values() {
            me.equipment_j += pw * snapshot.dt;
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

        // Unmet hours check
        // Use per-timestep setpoints (schedule-aware) when available,
        // otherwise fall back to static setpoints from ideal_loads defaults
        let hours_fraction = snapshot.dt / 3600.0;
        for (zone_name, &zone_temp) in &snapshot.zone_temperature {
            let heat_sp = snapshot.zone_heating_setpoint.get(zone_name)
                .or_else(|| self.heating_setpoints.get(zone_name));
            let cool_sp = snapshot.zone_cooling_setpoint.get(zone_name)
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

        writeln!(w, "================================================================")?;
        writeln!(w, "                    OpenBSE Summary Report                       ")?;
        writeln!(w, "================================================================")?;
        writeln!(w)?;

        // -- Annual Totals --
        let annual_heating_j: f64 = self.monthly.iter().map(|m| m.heating_j).sum();
        let annual_cooling_j: f64 = self.monthly.iter().map(|m| m.cooling_j).sum();
        let annual_heating_kwh = annual_heating_j / 3_600_000.0;
        let annual_cooling_kwh = annual_cooling_j / 3_600_000.0;
        let annual_heating_mwh = annual_heating_kwh / 1000.0;
        let annual_cooling_mwh = annual_cooling_kwh / 1000.0;

        writeln!(w, "-- Annual Energy Summary --------------------------------------")?;
        writeln!(w)?;
        writeln!(w, "  Heating:  {:>10.1} kWh  ({:.3} MWh)", annual_heating_kwh, annual_heating_mwh)?;
        writeln!(w, "  Cooling:  {:>10.1} kWh  ({:.3} MWh)", annual_cooling_kwh, annual_cooling_mwh)?;
        writeln!(w, "  Total:    {:>10.1} kWh  ({:.3} MWh)",
            annual_heating_kwh + annual_cooling_kwh,
            annual_heating_mwh + annual_cooling_mwh)?;
        writeln!(w)?;

        // -- Peak Loads --
        writeln!(w, "-- Peak Loads -------------------------------------------------")?;
        writeln!(w)?;
        if self.peak_heating.0 > 0.0 {
            writeln!(w, "  Peak Heating: {:>10.1} W  (Month {:>2}, Day {:>2}, Hour {:>2})",
                self.peak_heating.0, self.peak_heating.1, self.peak_heating.2, self.peak_heating.3)?;
        } else {
            writeln!(w, "  Peak Heating:       0.0 W  (no heating required)")?;
        }
        if self.peak_cooling.0 > 0.0 {
            writeln!(w, "  Peak Cooling: {:>10.1} W  (Month {:>2}, Day {:>2}, Hour {:>2})",
                self.peak_cooling.0, self.peak_cooling.1, self.peak_cooling.2, self.peak_cooling.3)?;
        } else {
            writeln!(w, "  Peak Cooling:       0.0 W  (no cooling required)")?;
        }
        writeln!(w)?;

        // -- Monthly Breakdown --
        writeln!(w, "-- Monthly Energy Breakdown -----------------------------------")?;
        writeln!(w)?;
        writeln!(w, "  {:>5}  {:>12}  {:>12}  {:>12}", "Month", "Heating[kWh]", "Cooling[kWh]", "Total[kWh]")?;
        writeln!(w, "  -----  ------------  ------------  ------------")?;

        let month_names = ["Jan", "Feb", "Mar", "Apr", "May", "Jun",
                          "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

        for (i, me) in self.monthly.iter().enumerate() {
            let h_kwh = me.heating_j / 3_600_000.0;
            let c_kwh = me.cooling_j / 3_600_000.0;
            if me.hours > 0.0 {
                writeln!(w, "  {:>5}  {:>12.1}  {:>12.1}  {:>12.1}",
                    month_names[i], h_kwh, c_kwh, h_kwh + c_kwh)?;
            }
        }

        writeln!(w, "  -----  ------------  ------------  ------------")?;
        writeln!(w, "  {:>5}  {:>12.1}  {:>12.1}  {:>12.1}",
            "Total", annual_heating_kwh, annual_cooling_kwh,
            annual_heating_kwh + annual_cooling_kwh)?;
        writeln!(w)?;

        // -- Unmet Hours --
        writeln!(w, "-- Unmet Hours ------------------------------------------------")?;
        writeln!(w)?;
        writeln!(w, "  Tolerance: {:.1} C", self.unmet_tolerance)?;
        writeln!(w)?;
        writeln!(w, "  Unmet Heating Hours: {:>8.1} hr", self.unmet_heating_hours)?;
        writeln!(w, "  Unmet Cooling Hours: {:>8.1} hr", self.unmet_cooling_hours)?;

        let total_hours = self.total_timesteps as f64 * self.dt / 3600.0;
        if total_hours > 0.0 {
            let heat_pct = self.unmet_heating_hours / total_hours * 100.0;
            let cool_pct = self.unmet_cooling_hours / total_hours * 100.0;
            writeln!(w)?;
            writeln!(w, "  Heating setpoint met: {:>5.1}% of occupied hours", 100.0 - heat_pct)?;
            writeln!(w, "  Cooling setpoint met: {:>5.1}% of occupied hours", 100.0 - cool_pct)?;

            // ASHRAE Standard 90.1 compliance check (300 unmet hours max)
            writeln!(w)?;
            let total_unmet = self.unmet_heating_hours + self.unmet_cooling_hours;
            if total_unmet <= 300.0 {
                writeln!(w, "  ASHRAE 90.1 Compliance: PASS ({:.0} <= 300 unmet hours)", total_unmet)?;
            } else {
                writeln!(w, "  ASHRAE 90.1 Compliance: FAIL ({:.0} > 300 unmet hours)", total_unmet)?;
            }
        }
        writeln!(w)?;

        // -- Energy End-Use Summary (matches EnergyPlus categories) --
        writeln!(w, "-- Energy End-Use Summary -------------------------------------")?;
        writeln!(w)?;

        let j_to_kwh = 1.0 / 3_600_000.0;
        let annual_lighting_kwh: f64 = self.monthly.iter().map(|m| m.lighting_j).sum::<f64>() * j_to_kwh;
        let annual_ext_lighting_kwh: f64 = self.monthly.iter().map(|m| m.ext_lighting_j).sum::<f64>() * j_to_kwh;
        let annual_equipment_kwh: f64 = self.monthly.iter().map(|m| m.equipment_j).sum::<f64>() * j_to_kwh;
        let annual_ext_equipment_kwh: f64 = self.monthly.iter().map(|m| m.ext_equipment_j).sum::<f64>() * j_to_kwh;
        let annual_fan_kwh: f64 = self.monthly.iter().map(|m| m.fan_elec_j).sum::<f64>() * j_to_kwh;
        let annual_pump_kwh: f64 = self.monthly.iter().map(|m| m.pump_elec_j).sum::<f64>() * j_to_kwh;
        let annual_cool_elec_kwh: f64 = self.monthly.iter().map(|m| m.cool_elec_j).sum::<f64>() * j_to_kwh;
        let annual_heat_elec_kwh: f64 = self.monthly.iter().map(|m| m.heat_elec_j).sum::<f64>() * j_to_kwh;
        let annual_heat_gas_kwh: f64 = self.monthly.iter().map(|m| m.heat_gas_j).sum::<f64>() * j_to_kwh;
        let annual_heat_rejection_kwh: f64 = self.monthly.iter().map(|m| m.heat_rejection_elec_j).sum::<f64>() * j_to_kwh;
        let annual_humidification_kwh: f64 = self.monthly.iter().map(|m| m.humidification_elec_j).sum::<f64>() * j_to_kwh;
        let annual_dhw_elec_kwh: f64 = self.monthly.iter().map(|m| m.dhw_elec_j).sum::<f64>() * j_to_kwh;
        let annual_dhw_gas_kwh: f64 = self.monthly.iter().map(|m| m.dhw_gas_j).sum::<f64>() * j_to_kwh;

        writeln!(w, "  {:>22}  {:>12}", "End Use", "Annual [kWh]")?;
        writeln!(w, "  ----------------------  ------------")?;
        writeln!(w, "  {:>22}  {:>12.1}", "Interior Lighting", annual_lighting_kwh)?;
        writeln!(w, "  {:>22}  {:>12.1}", "Exterior Lighting", annual_ext_lighting_kwh)?;
        writeln!(w, "  {:>22}  {:>12.1}", "Interior Equipment", annual_equipment_kwh)?;
        writeln!(w, "  {:>22}  {:>12.1}", "Exterior Equipment", annual_ext_equipment_kwh)?;
        writeln!(w, "  {:>22}  {:>12.1}", "Fans (Electric)", annual_fan_kwh)?;
        writeln!(w, "  {:>22}  {:>12.1}", "Pumps (Electric)", annual_pump_kwh)?;
        writeln!(w, "  {:>22}  {:>12.1}", "Cooling (Electric)", annual_cool_elec_kwh)?;
        writeln!(w, "  {:>22}  {:>12.1}", "Heating (Electric)", annual_heat_elec_kwh)?;
        writeln!(w, "  {:>22}  {:>12.1}", "Heating (Gas)", annual_heat_gas_kwh)?;
        writeln!(w, "  {:>22}  {:>12.1}", "Heat Rejection", annual_heat_rejection_kwh)?;
        writeln!(w, "  {:>22}  {:>12.1}", "Humidification", annual_humidification_kwh)?;
        writeln!(w, "  {:>22}  {:>12.1}", "DHW (Electric)", annual_dhw_elec_kwh)?;
        writeln!(w, "  {:>22}  {:>12.1}", "DHW (Gas)", annual_dhw_gas_kwh)?;
        writeln!(w, "  ----------------------  ------------")?;
        let total_end_use = annual_lighting_kwh + annual_ext_lighting_kwh
            + annual_equipment_kwh + annual_ext_equipment_kwh
            + annual_fan_kwh + annual_pump_kwh
            + annual_cool_elec_kwh + annual_heat_elec_kwh + annual_heat_gas_kwh
            + annual_heat_rejection_kwh + annual_humidification_kwh
            + annual_dhw_elec_kwh + annual_dhw_gas_kwh;
        writeln!(w, "  {:>22}  {:>12.1}", "Total", total_end_use)?;
        writeln!(w)?;

        // -- Window Solar Diagnostics --
        if self.total_transmitted_solar_j > 0.0 {
            writeln!(w, "-- Window Solar Diagnostics -----------------------------------")?;
            writeln!(w)?;
            let trans_kwh = self.total_transmitted_solar_j / 3_600_000.0;
            let inc_kwh = self.total_incident_solar_j / 3_600_000.0;
            writeln!(w, "  Total transmitted solar: {:>10.1} kWh", trans_kwh)?;
            writeln!(w, "  Total incident on windows: {:>7.1} kWh", inc_kwh)?;
            if inc_kwh > 0.0 {
                writeln!(w, "  Effective annual modifier: {:>7.4} (trans/incident)", trans_kwh / inc_kwh)?;
            }
            writeln!(w)?;
            writeln!(w, "  {:>5}  {:>12}", "Month", "Trans[kWh]")?;
            writeln!(w, "  -----  ------------")?;
            let month_names = ["Jan", "Feb", "Mar", "Apr", "May", "Jun",
                              "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
            for (i, &mj) in self.monthly_transmitted_solar_j.iter().enumerate() {
                if mj > 0.0 {
                    writeln!(w, "  {:>5}  {:>12.1}", month_names[i], mj / 3_600_000.0)?;
                }
            }
            writeln!(w, "  -----  ------------")?;
            writeln!(w, "  {:>5}  {:>12.1}", "Total", trans_kwh)?;
            writeln!(w)?;
        }

        // -- Simulation Statistics --
        writeln!(w, "-- Simulation Statistics ---------------------------------------")?;
        writeln!(w)?;
        writeln!(w, "  Total timesteps:   {:>8}", self.total_timesteps)?;
        writeln!(w, "  Timestep size:     {:>8.0} s ({:.0} per hour)", self.dt, 3600.0 / self.dt)?;
        writeln!(w, "  Simulated hours:   {:>8.1} hr", total_hours)?;
        writeln!(w)?;
        writeln!(w, "================================================================")?;

        w.flush()?;
        Ok(())
    }
}

// ─── Legacy CSV Writer (backward compatible) ────────────────────────────────

/// Write simulation results to a CSV file (legacy format from TimestepResult).
///
/// This maintains backward compatibility with the existing output format.
pub fn write_csv(
    results: &[TimestepResult],
    path: &Path,
) -> Result<(), OutputError> {
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
        assert_eq!(get_unit("zone_temperature"), "\u{00b0}C");
        assert_eq!(get_unit("zone_heating_rate"), "W");
        assert_eq!(get_unit("zone_heating_energy"), "J");
        assert_eq!(get_unit("site_outdoor_temperature"), "\u{00b0}C");
        assert_eq!(get_unit("unknown_var"), "-");
        // Legacy
        assert_eq!(get_unit("zone_temp"), "\u{00b0}C");
        assert_eq!(get_unit("heating_load"), "W");
    }

    #[test]
    fn test_output_snapshot_site_variables() {
        let mut snap = OutputSnapshot::new(1, 1, 1, 1, 3600.0);
        snap.site_outdoor_temperature = -5.0;
        snap.site_wind_speed = 3.5;

        let vals = snap.get_variable_values("site_outdoor_temperature");
        assert_eq!(vals.get("Site"), Some(&-5.0));

        let vals = snap.get_variable_values("site_wind_speed");
        assert_eq!(vals.get("Site"), Some(&3.5));
    }

    #[test]
    fn test_output_snapshot_zone_energy_integration() {
        let mut snap = OutputSnapshot::new(1, 1, 1, 1, 900.0); // 15-min timestep
        snap.zone_heating_rate.insert("Zone1".to_string(), 1000.0); // 1000W

        let energy = snap.get_variable_values("zone_heating_energy");
        // 1000W * 900s = 900000 J
        assert_eq!(energy.get("Zone1"), Some(&900_000.0));
    }

    #[test]
    fn test_output_writer_timestep_frequency() {
        let config = OutputFileConfig {
            file: "test.csv".to_string(),
            frequency: OutputFrequency::Timestep,
            aggregation: Aggregation::Mean,
            variables: vec!["site_outdoor_temperature".to_string()],
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
            variables: vec!["site_outdoor_temperature".to_string()],
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
}
