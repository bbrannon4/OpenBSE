//! OpenBSE command-line interface.
//!
//! Runs building energy simulations from YAML input files.

use anyhow::{Context, Result};
use clap::Parser;
use log::{info, warn};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use openbse_core::graph::{GraphComponent, SimulationGraph};
use openbse_core::ports::{AirPort, EnvelopeSolver, SimulationContext, SizingInternalGains, WaterPort, ZoneHvacConditions};
use openbse_core::simulation::{ControlSignals, SimulationConfig, TimestepResult};
use openbse_core::types::{DayType, TimeStep};
use openbse_envelope::schedule::ScheduleManager;
use openbse_io::input::{build_controllers, build_envelope, build_graph, compute_oa_fraction, load_model, resolve_thermostats, AirLoopSystemType};
use openbse_io::output::{write_csv, OutputSnapshot, OutputWriter, SummaryReport};
use openbse_weather::read_weather_file;

#[derive(Parser, Debug)]
#[command(name = "openbse")]
#[command(about = "Open Building Simulation Engine", long_about = None)]
#[command(version)]
struct Args {
    /// Path to the input YAML file
    #[arg(value_name = "INPUT")]
    input: PathBuf,

    /// Output CSV file path (default: <input_dir>/results.csv)
    #[arg(short, long, value_name = "OUTPUT")]
    output: Option<PathBuf>,

    /// Enable verbose logging
    #[arg(short, long)]
    verbose: bool,
}

// ─── Loop Descriptor ─────────────────────────────────────────────────────────
//
// Captures the static properties of an air loop that the control logic needs
// at every timestep. Built once at startup from the model input.

#[derive(Debug, Clone)]
#[allow(dead_code)] // Fields populated for upcoming cycling/supply temp logic
struct LoopInfo {
    name: String,
    system_type: AirLoopSystemType,
    /// Component names in simulation order (fan → coils)
    component_names: Vec<String>,
    /// Names of fan components in this loop (for PLR-exempt identification)
    fan_names: HashSet<String>,
    /// Zones served by this loop
    served_zones: Vec<String>,
    /// Minimum outdoor air fraction [0-1]. DOAS always 1.0.
    /// Resolved from controls.minimum_damper_position or auto-calculated.
    min_oa_fraction: f64,
    /// Minimum VAV box flow fraction [0-1]. Only used for VAV.
    min_vav_fraction: f64,
    /// HVAC availability schedule name. When schedule value is 0, system is OFF.
    availability_schedule: Option<String>,
    /// Design heating supply air temperature [°C] (from air loop controls)
    heating_supply_temp: f64,
    /// Design cooling supply air temperature [°C] (from air loop controls)
    cooling_supply_temp: f64,
    /// Capacity control method (from air loop controls)
    cycling: openbse_io::input::CyclingMethod,
    /// Fan operating mode: cycling (fan cycles with coils) or continuous
    /// (fan runs at full speed always, coils cycle ON/OFF).
    fan_operating_mode: openbse_io::input::FanOperatingMode,
    /// Terminal box component names per zone (zone_name -> component_name).
    /// Only populated for loops with VAV/PFP terminal boxes defined in YAML.
    terminal_boxes: HashMap<String, String>,
    /// True when the user explicitly set `minimum_damper_position` in YAML.
    /// Prevents post-sizing auto-recalculation from overriding the user value.
    explicit_min_oa: bool,
}

fn build_loop_infos(
    model: &openbse_io::input::ModelInput,
    resolved_zones: &[openbse_envelope::ZoneInput],
) -> Vec<LoopInfo> {
    model.air_loops.iter().map(|al| {
        let component_names: Vec<String> = al.equipment.iter().map(|eq| {
            use openbse_io::input::EquipmentInput;
            match eq {
                EquipmentInput::Fan(f)           => f.name.clone(),
                EquipmentInput::HeatingCoil(c)   => c.name.clone(),
                EquipmentInput::CoolingCoil(c)   => c.name.clone(),
                EquipmentInput::HeatRecovery(hr) => hr.name.clone(),
                EquipmentInput::Humidifier(h)    => h.name.clone(),
            }
        }).collect();

        let fan_names: HashSet<String> = al.equipment.iter().filter_map(|eq| {
            use openbse_io::input::EquipmentInput;
            match eq {
                EquipmentInput::Fan(f) => Some(f.name.clone()),
                _ => None,
            }
        }).collect();

        let served_zones: Vec<String> = al.zone_terminals.iter()
            .map(|zc| zc.zone.clone())
            .collect();

        // Auto-detect or use explicit system type
        let system_type = al.detect_system_type();

        // Resolve minimum outdoor air fraction:
        //   1. DOAS always 100%
        //   2. Explicit controls.minimum_damper_position
        //   3. Auto-calculate from zone outdoor air requirements
        //   4. Fallback: 20%
        let explicit_min_oa = system_type != AirLoopSystemType::Doas
            && al.minimum_damper_position().is_some();
        let min_oa_fraction = match system_type {
            AirLoopSystemType::Doas => 1.0,
            _ => al.minimum_damper_position().unwrap_or_else(|| {
                let computed = compute_oa_fraction(model, al, resolved_zones, 0.20);
                log::info!(
                    "Air loop '{}': auto-calculated minimum damper position = {:.1}%",
                    al.name, computed * 100.0
                );
                computed
            }),
        };

        // Build terminal box map: zone_name -> component_name
        let mut terminal_boxes: HashMap<String, String> = HashMap::new();
        for zc in &al.zone_terminals {
            if let Some(ref terminal) = zc.terminal {
                let term_name = match terminal {
                    openbse_io::input::TerminalInput::VavBox(vb) => vb.name.clone(),
                    openbse_io::input::TerminalInput::PfpBox(pb) => pb.name.clone(),
                };
                terminal_boxes.insert(zc.zone.clone(), term_name);
            }
        }

        LoopInfo {
            name: al.name.clone(),
            system_type,
            component_names,
            fan_names,
            served_zones,
            min_oa_fraction,
            explicit_min_oa,
            min_vav_fraction: al.min_vav_fraction,
            availability_schedule: al.availability_schedule.clone(),
            heating_supply_temp: al.controls.heating_supply_temp,
            cooling_supply_temp: al.controls.cooling_supply_temp,
            cycling: al.controls.cycling,
            fan_operating_mode: al.controls.fan_operating_mode,
            terminal_boxes,
        }
    }).collect()
}

fn main() -> Result<()> {
    let args = Args::parse();

    // Initialize logger
    let log_level = if args.verbose { "debug" } else { "info" };
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(log_level)).init();

    // Derive a stem from the input filename (e.g. "retail_rtu" from "retail_rtu.yaml").
    // All output files are prefixed with this stem so results always sit alongside
    // the input file and are clearly associated with it.
    let input_dir = args.input.parent().unwrap_or_else(|| Path::new("."));
    let input_stem = args.input
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("openbse")
        .to_string();

    // Main results CSV: <input_dir>/<stem>_results.csv, or explicit --output path.
    let output_path: PathBuf = args.output
        .clone()
        .unwrap_or_else(|| input_dir.join(format!("{}_results.csv", input_stem)));

    info!("OpenBSE v{}", env!("CARGO_PKG_VERSION"));
    info!("Reading input file: {}", args.input.display());

    // ── 1. Load and parse the model ─────────────────────────────────────────
    let model = load_model(&args.input)
        .with_context(|| format!("Failed to load model from {}", args.input.display()))?;

    info!(
        "Model loaded: {} air loops, {} plant loops, {} zone groups",
        model.air_loops.len(),
        model.plant_loops.len(),
        model.zone_groups.len()
    );

    // ── 1b. Validate model cross-references ────────────────────────────────
    let validation = openbse_io::validate_model(&model);

    // Write .err file (always, even if no errors — matches E+ behavior)
    let err_path = args.input.with_extension("err");
    if let Err(e) = std::fs::write(&err_path, validation.to_err_file()) {
        warn!("Could not write error file {}: {}", err_path.display(), e);
    }

    // Log all diagnostics to console
    for diag in &validation.diagnostics {
        match diag.severity {
            openbse_io::DiagSeverity::Warning => warn!("{}", diag.message),
            openbse_io::DiagSeverity::Severe  => log::error!("{}", diag.message),
        }
    }

    // Abort if there are severe errors
    if validation.error_count() > 0 {
        anyhow::bail!(
            "Model validation failed: {} severe error(s), {} warning(s). See {}",
            validation.error_count(),
            validation.warning_count(),
            err_path.display()
        );
    }
    if validation.warning_count() > 0 {
        warn!("{} validation warning(s) — see {}", validation.warning_count(), err_path.display());
    }

    // ── 2. Load weather data ────────────────────────────────────────────────
    if model.weather_files.is_empty() {
        anyhow::bail!("No weather files specified in the model");
    }

    let weather_path = resolve_path(&args.input, &model.weather_files[0]);
    info!("Loading weather file: {}", weather_path.display());

    let weather_data = read_weather_file(&weather_path)
        .with_context(|| format!("Failed to read weather file {}", weather_path.display()))?;

    info!(
        "Weather loaded: {}, lat={:.2}, lon={:.2}, {} hourly records",
        weather_data.location.city,
        weather_data.location.latitude,
        weather_data.location.longitude,
        weather_data.hours.len()
    );

    // ── 3. Build simulation components ──────────────────────────────────────
    let mut graph = build_graph(&model).context("Failed to build simulation graph")?;
    info!("Graph built: {} components", graph.component_count());

    let controllers = build_controllers(&model);
    info!("Controllers built: {} controllers", controllers.len());

    let mut envelope = build_envelope(
        &model,
        weather_data.location.latitude,
        weather_data.location.longitude,
        weather_data.location.time_zone,
        weather_data.location.elevation,
    );

    // Set up ground temperature model for surfaces with `boundary: ground`.
    //
    // EnergyPlus uses `Site:GroundTemperature:BuildingSurface` for these surfaces.
    // When that object is absent (as in DOE prototype buildings), E+ defaults to
    // 18°C for all months. This is NOT the same as the EPW ground temps or
    // FCfactorMethod temps, which serve different purposes.
    //
    // Priority:
    //   1. YAML-specified `ground_surface_temperatures` (12 monthly values)
    //   2. Default: 18°C constant (matches E+ BuildingSurface default)
    if let Some(ref mut env) = envelope {
        let mut ground_temp = openbse_envelope::GroundTempModel::from_weather_hours(&weather_data.hours);

        // Use YAML-specified ground surface temperatures (or E+ default of 18°C)
        let gt_monthly = &model.simulation.ground_surface_temperatures;
        if gt_monthly.len() == 12 {
            let mut temps = [0.0_f64; 12];
            temps.copy_from_slice(gt_monthly);
            ground_temp.monthly_temps = Some(temps);
            info!(
                "Ground temp: using YAML monthly temps (Jan={:.1}°C, Jul={:.1}°C, mean={:.1}°C)",
                temps[0], temps[6],
                temps.iter().sum::<f64>() / 12.0,
            );
        } else {
            // Fallback to Kusuda-Achenbach model
            info!(
                "Ground temp: Kusuda model at {:.1}m depth (mean={:.1}°C, amplitude={:.1}°C, phase=day {:.0})",
                ground_temp.depth, ground_temp.t_mean, ground_temp.amplitude, ground_temp.phase_day
            );
        }

        env.ground_temp_model = Some(ground_temp);
        env.jan1_dow = weather_data.start_day_of_week;
        info!("Weather file start day of week: {} (1=Mon..7=Sun)", weather_data.start_day_of_week);
    }

    if let Some(ref env) = envelope {
        info!(
            "Envelope built: {} zones, {} surfaces",
            env.zones.len(),
            env.surfaces.len()
        );
    } else {
        info!("No envelope defined (HVAC-only simulation)");
    }

    // ── 4. Build loop descriptors ──────────────────────────────────────────
    // Get resolved zones for OA fraction auto-calculation
    let resolved_zones_for_oa: Vec<openbse_envelope::ZoneInput> = envelope.as_ref()
        .map(|env| env.zones.iter().map(|z| z.input.clone()).collect())
        .unwrap_or_else(|| model.zones.clone());
    let mut loop_infos = build_loop_infos(&model, &resolved_zones_for_oa);
    for li in &loop_infos {
        info!(
            "Air loop '{}': type={:?}, zones=[{}], OA={:.0}%",
            li.name,
            li.system_type,
            li.served_zones.join(", "),
            li.min_oa_fraction * 100.0,
        );
    }

    // ── 4b. Build DHW systems ────────────────────────────────────────────
    let mut dhw_systems: Vec<openbse_components::water_heater::WaterHeater> = model.dhw_systems.iter()
        .map(|dhw_input| {
            use openbse_components::water_heater::{WaterHeater, WaterHeaterFuel};
            let fuel = match dhw_input.water_heater.fuel_type.as_str() {
                "electric" | "Electric" => WaterHeaterFuel::Electric,
                "heat_pump" | "HeatPump" | "hpwh" => WaterHeaterFuel::HeatPump,
                _ => WaterHeaterFuel::Gas,
            };
            let mut wh = WaterHeater::new(
                &dhw_input.water_heater.name,
                fuel,
                dhw_input.water_heater.tank_volume,
                dhw_input.water_heater.capacity,
                dhw_input.water_heater.efficiency,
                dhw_input.water_heater.setpoint,
                dhw_input.water_heater.ua_standby,
            );
            wh.deadband = dhw_input.water_heater.deadband;
            wh
        })
        .collect();
    if !dhw_systems.is_empty() {
        info!("DHW systems built: {}", dhw_systems.len());
    }

    // Collect pump names from plant loops and DHW systems for end-use routing
    let mut pump_names: std::collections::HashSet<String> = model.plant_loops.iter()
        .flat_map(|pl| pl.supply_equipment.iter())
        .filter_map(|eq| match eq {
            openbse_io::input::PlantEquipmentInput::Pump(p) => Some(p.name.clone()),
            _ => None,
        })
        .collect();
    // Also include DHW pump names
    for dhw in &model.dhw_systems {
        if let Some(ref pump) = dhw.pump {
            pump_names.insert(pump.name.clone());
        }
    }

    // Collect humidifier names from air loops for end-use routing
    let humidifier_names: std::collections::HashSet<String> = model.air_loops.iter()
        .flat_map(|al| al.equipment.iter())
        .filter_map(|eq| match eq {
            openbse_io::input::EquipmentInput::Humidifier(h) => Some(h.name.clone()),
            _ => None,
        })
        .collect();

    // ── 5. Set up simulation timing ─────────────────────────────────────────
    let config = SimulationConfig {
        timesteps_per_hour: model.simulation.timesteps_per_hour,
        start_month: model.simulation.start_month,
        start_day: model.simulation.start_day,
        end_month: model.simulation.end_month,
        end_day: model.simulation.end_day,
        ..Default::default()
    };

    let days_in_months: [u32; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let dt = 3600.0 / config.timesteps_per_hour as f64;

    let start_hour = day_of_year(config.start_month, config.start_day, &days_in_months) * 24;
    let end_hour = (day_of_year(config.end_month, config.end_day, &days_in_months) + 1) * 24;
    let end_hour = end_hour.min(weather_data.hours.len() as u32);
    let total_timesteps = (end_hour - start_hour) * config.timesteps_per_hour;

    info!(
        "Simulation: {}/{} to {}/{}, {} timesteps/hr, {} total timesteps",
        config.start_month, config.start_day,
        config.end_month, config.end_day,
        config.timesteps_per_hour, total_timesteps,
    );

    // Initialize envelope
    if let Some(ref mut env) = envelope {
        env.initialize(dt)
            .map_err(|e| anyhow::anyhow!("Failed to initialize envelope: {}", e))?;
    }

    // Output directory (needed by sizing and output writers)
    let output_dir = output_path.parent().unwrap_or_else(|| Path::new("."));

    // Gather zone setpoints from resolved thermostats
    let resolved_thermostats = resolve_thermostats(&model);
    let mut zone_heating_setpoints: HashMap<String, f64> = HashMap::new();
    let mut zone_cooling_setpoints: HashMap<String, f64> = HashMap::new();
    let mut zone_unocc_heating_setpoints: HashMap<String, f64> = HashMap::new();
    let mut zone_unocc_cooling_setpoints: HashMap<String, f64> = HashMap::new();
    let mut zone_design_flows: HashMap<String, f64> = HashMap::new();
    for tstat in &resolved_thermostats {
        for zone_name in &tstat.zones {
            zone_heating_setpoints.insert(zone_name.clone(), tstat.heating_setpoint);
            zone_cooling_setpoints.insert(zone_name.clone(), tstat.cooling_setpoint);
            zone_unocc_heating_setpoints.insert(zone_name.clone(), tstat.unoccupied_heating_setpoint);
            zone_unocc_cooling_setpoints.insert(zone_name.clone(), tstat.unoccupied_cooling_setpoint);
        }
    }

    // Gather design zone flows from air loop controls (not thermostats).
    // Each air loop's controls.design_zone_flow applies to all zones it serves.
    for al in &model.air_loops {
        let flow = al.controls.design_zone_flow.to_f64();
        for zc in &al.zone_terminals {
            zone_design_flows.insert(zc.zone.clone(), flow);
        }
    }

    // Build zone multiplier map for sizing calculations.
    // Zone multiplier accounts for identical zones (e.g., 5 identical hotel rooms).
    let zone_multipliers: HashMap<String, f64> = envelope.as_ref()
        .map(|env| env.zones.iter()
            .map(|z| (z.input.name.clone(), z.input.multiplier as f64))
            .collect())
        .unwrap_or_else(|| model.zones.iter()
            .map(|z| (z.name.clone(), z.multiplier as f64))
            .collect());

    // Build component-to-multiplier map for HVAC outputs.
    // For per-zone loops (PTAC/FCU), HVAC energy must be scaled by the
    // zone multiplier (e.g., M floor multiplier=2 means 2 identical PTACs).
    let component_multipliers: HashMap<String, f64> = loop_infos.iter()
        .flat_map(|li| {
            // For loops serving exactly one zone, apply that zone's multiplier
            // to every component in the loop.
            let mult = if li.served_zones.len() == 1 {
                zone_multipliers.get(&li.served_zones[0]).copied().unwrap_or(1.0)
            } else {
                1.0 // Multi-zone AHUs: don't apply per-zone multiplier
            };
            li.component_names.iter().map(move |name| (name.clone(), mult))
        })
        .collect();

    // Build OA handling flags for sizing: zones served by HVAC with
    // min_oa_fraction=0 (e.g. PTAC with separate ERV) have zone OA flowing
    // directly, so sizing must include that OA load.
    let sizing_oa_handled: HashMap<String, bool> = loop_infos.iter()
        .flat_map(|li| {
            let handles_oa = li.min_oa_fraction > 0.001;
            li.served_zones.iter().map(move |z| (z.clone(), handles_oa))
        })
        .collect();

    // ── Design Day Sizing Run ──────────────────────────────────────────
    // Two-stage ASHRAE-compliant sizing:
    //   Stage 1: Zone sizing — peak loads per zone from ALL design days
    //   Stage 2: System sizing — coincident peak system loads
    if !model.design_days.is_empty() {
        if let Some(ref mut env) = envelope {
            let latitude = weather_data.location.latitude;
            // Extract supply temps from air loop controls.
            // Use the max heating supply temp and min cooling supply temp
            // across all air loops to ensure sizing covers worst case.
            let supply_temps = if !model.air_loops.is_empty() {
                let max_heat = model.air_loops.iter()
                    .map(|al| al.controls.heating_supply_temp)
                    .fold(f64::NEG_INFINITY, f64::max);
                let min_cool = model.air_loops.iter()
                    .map(|al| al.controls.cooling_supply_temp)
                    .fold(f64::INFINITY, f64::min);
                Some((max_heat, min_cool))
            } else {
                None
            };

            let sizing_result = openbse_io::sizing::run_sizing(
                env,
                &model.design_days,
                &resolved_thermostats,
                latitude,
                &weather_data.hours,
                output_dir,
                &input_stem,
                supply_temps,
                model.simulation.heating_sizing_factor,
                model.simulation.cooling_sizing_factor,
                &sizing_oa_handled,
            );

            // Apply sized zone airflows (override design_zone_flow)
            for (zone_name, &flow) in &sizing_result.zone_design_airflow {
                zone_design_flows.insert(zone_name.clone(), flow);
            }

            // Apply sized capacities to HVAC components.
            //
            // Sizing is loop-aware:
            //   - PSZ-AC / VAV loops: use system-wide capacities and total airflow
            //   - DOAS loops: use a fraction of total OA flow (30% of zone design flows)
            //   - FCU loops: use the served zone's design airflow and peak zone load
            //
            // This ensures each loop's components are sized for their actual duty,
            // not the system-wide peak.
            use openbse_core::types::is_autosize;

            // Build a map: component_name -> (loop_flow [m³/s], loop_heat [W], loop_cool [W])
            let air_density = 1.204_f64;  // kg/m³ at 20°C, 101.325 kPa
            let mut loop_comp_sizing: HashMap<String, (f64, f64, f64)> = HashMap::new();

            for li in &loop_infos {
                let (loop_flow, loop_heat, loop_cool) = match li.system_type {
                    AirLoopSystemType::PszAc => {
                        // PSZ-AC: each unit serves its own zone(s) independently.
                        // Size from served zone peak loads (like FCU), not system-wide.
                        // Do NOT apply zone multiplier: each PSZ-AC is one physical unit
                        // per zone instance; multiplier represents N identical units.
                        let zone_airflow: f64 = li.served_zones.iter()
                            .map(|z| {
                                sizing_result.zone_design_airflow.get(z).copied().unwrap_or(0.1)
                            })
                            .sum();
                        let zone_flow_m3 = zone_airflow / air_density;
                        let zone_heat: f64 = li.served_zones.iter()
                            .map(|z| {
                                sizing_result.zone_peak_heating.get(z).copied().unwrap_or(0.0)
                            })
                            .sum::<f64>() * model.simulation.heating_sizing_factor;
                        let zone_cool: f64 = li.served_zones.iter()
                            .map(|z| {
                                sizing_result.zone_peak_cooling.get(z).copied().unwrap_or(0.0)
                            })
                            .sum::<f64>() * model.simulation.cooling_sizing_factor;
                        (zone_flow_m3, zone_heat, zone_cool)
                    }
                    AirLoopSystemType::Vav => {
                        // VAV: multi-zone system. Compute per-system flow + capacities
                        // from served zones, accounting for zone multipliers.
                        // (Zone multiplier means N identical zones share this AHU;
                        // the fan must handle the total multiplied airflow.)
                        let zone_airflow: f64 = li.served_zones.iter()
                            .map(|z| {
                                let m = zone_multipliers.get(z).copied().unwrap_or(1.0);
                                sizing_result.zone_design_airflow.get(z).copied().unwrap_or(0.1) * m
                            })
                            .sum();
                        let zone_flow_m3 = zone_airflow / air_density;
                        let zone_heat: f64 = li.served_zones.iter()
                            .map(|z| {
                                let m = zone_multipliers.get(z).copied().unwrap_or(1.0);
                                sizing_result.zone_peak_heating.get(z).copied().unwrap_or(0.0) * m
                            })
                            .sum::<f64>() * model.simulation.heating_sizing_factor;
                        let zone_cool: f64 = li.served_zones.iter()
                            .map(|z| {
                                let m = zone_multipliers.get(z).copied().unwrap_or(1.0);
                                sizing_result.zone_peak_cooling.get(z).copied().unwrap_or(0.0) * m
                            })
                            .sum::<f64>() * model.simulation.cooling_sizing_factor;
                        (zone_flow_m3, zone_heat, zone_cool)
                    }
                    AirLoopSystemType::Doas => {
                        // DOAS sizing: coils are sized to pre-condition 100% OA from
                        // design outdoor conditions to fixed supply setpoints.
                        //
                        // Heating: Q = m_oa * cp * (T_supply_heat - T_outdoor_heat_design)
                        // Cooling: Q = m_oa * cp * (T_outdoor_cool_design - T_supply_cool)
                        //
                        // Design outdoor temps from the coldest/hottest design days.
                        let zone_airflow: f64 = li.served_zones.iter()
                            .map(|z| {
                                let m = zone_multipliers.get(z).copied().unwrap_or(1.0);
                                sizing_result.zone_design_airflow.get(z).copied().unwrap_or(0.1) * m
                            })
                            .sum();
                        let oa_flow_kg = zone_airflow * 0.30;
                        let oa_flow_m3 = oa_flow_kg / air_density;

                        // Find design outdoor temps from design days
                        let t_outdoor_heat_design = model.design_days.iter()
                            .filter(|dd| dd.day_type.to_lowercase().contains("heat") || dd.day_type.to_lowercase().contains("winter"))
                            .map(|dd| dd.design_temp)
                            .fold(f64::INFINITY, f64::min);
                        let t_outdoor_heat = if t_outdoor_heat_design.is_finite() { t_outdoor_heat_design } else { -20.0 };

                        let t_outdoor_cool_design = model.design_days.iter()
                            .filter(|dd| dd.day_type.to_lowercase().contains("cool") || dd.day_type.to_lowercase().contains("summer"))
                            .map(|dd| dd.design_temp)
                            .fold(f64::NEG_INFINITY, f64::max);
                        let t_outdoor_cool = if t_outdoor_cool_design.is_finite() { t_outdoor_cool_design } else { 35.0 };

                        // DOAS supply setpoints (default: heat to 20°C, cool to 18°C)
                        let t_supply_heat = 20.0_f64;
                        let t_supply_cool = 18.0_f64;

                        let cp_air = 1005.0_f64;
                        let doas_heat_cap = (oa_flow_kg * cp_air * (t_supply_heat - t_outdoor_heat).max(0.0)) * model.simulation.heating_sizing_factor;
                        let doas_cool_cap = (oa_flow_kg * cp_air * (t_outdoor_cool - t_supply_cool).max(0.0)) * model.simulation.cooling_sizing_factor;

                        (oa_flow_m3, doas_heat_cap, doas_cool_cap)
                    }
                    AirLoopSystemType::Fcu | AirLoopSystemType::Ptac => {
                        // FCU/PTAC: sized to its served zone(s)
                        // Coil capacity must include ventilation heating/cooling load
                        // (outdoor air mixed with return air before entering the coil).
                        // Do NOT apply zone multiplier: each FCU/PTAC is one physical unit
                        // per zone instance; multiplier represents N identical units.
                        let zone_airflow: f64 = li.served_zones.iter()
                            .map(|z| {
                                sizing_result.zone_design_airflow.get(z).copied().unwrap_or(0.1)
                            })
                            .sum();
                        let zone_flow_m3 = zone_airflow / air_density;

                        // Zone peak loads (envelope + internal gains only)
                        let zone_peak_heat: f64 = li.served_zones.iter()
                            .map(|z| {
                                sizing_result.zone_peak_heating.get(z).copied().unwrap_or(0.0)
                            })
                            .sum::<f64>();
                        let zone_peak_cool: f64 = li.served_zones.iter()
                            .map(|z| {
                                sizing_result.zone_peak_cooling.get(z).copied().unwrap_or(0.0)
                            })
                            .sum::<f64>();

                        // Design outdoor temps from design days
                        let t_outdoor_heat_design = model.design_days.iter()
                            .filter(|dd| dd.day_type.to_lowercase().contains("heat") || dd.day_type.to_lowercase().contains("winter"))
                            .map(|dd| dd.design_temp)
                            .fold(f64::INFINITY, f64::min);
                        let t_outdoor_heat = if t_outdoor_heat_design.is_finite() { t_outdoor_heat_design } else { -20.0 };

                        let t_outdoor_cool_design = model.design_days.iter()
                            .filter(|dd| dd.day_type.to_lowercase().contains("cool") || dd.day_type.to_lowercase().contains("summer"))
                            .map(|dd| dd.design_temp)
                            .fold(f64::NEG_INFINITY, f64::max);
                        let t_outdoor_cool = if t_outdoor_cool_design.is_finite() { t_outdoor_cool_design } else { 35.0 };

                        // Mixed air temperature = blend of return air (≈zone setpoint)
                        // and outdoor air at design conditions
                        let cp_air = 1005.0_f64;
                        let oa_frac = li.min_oa_fraction;
                        let t_zone_heat = 21.0_f64;  // heating setpoint
                        let t_zone_cool = 24.0_f64;  // cooling setpoint

                        // Heating: coil heats mixed air from T_mixed to T_supply_heat
                        let t_mixed_heat = (1.0 - oa_frac) * t_zone_heat + oa_frac * t_outdoor_heat;
                        let coil_heat_cap = zone_airflow * cp_air * (li.heating_supply_temp - t_mixed_heat).max(0.0);

                        // Cooling: coil cools mixed air from T_mixed to T_supply_cool
                        let t_mixed_cool = (1.0 - oa_frac) * t_zone_cool + oa_frac * t_outdoor_cool;
                        let coil_cool_cap = zone_airflow * cp_air * (t_mixed_cool - li.cooling_supply_temp).max(0.0);

                        // Use the larger of (zone peak load × sizing factor) and
                        // coil capacity (which already includes sizing factor via
                        // sized airflow from zone sizing).
                        let zone_heat = (zone_peak_heat * model.simulation.heating_sizing_factor).max(coil_heat_cap);
                        let zone_cool = (zone_peak_cool * model.simulation.cooling_sizing_factor).max(coil_cool_cap);

                        (zone_flow_m3, zone_heat, zone_cool)
                    }
                };

                for comp_name in &li.component_names {
                    loop_comp_sizing.insert(comp_name.clone(), (loop_flow, loop_heat, loop_cool));
                }
            }

            for comp in graph.air_components_mut() {
                let name = comp.name().to_string();
                let lname = name.to_lowercase();

                // Only autosize components that belong to an air loop's equipment list.
                // Terminal boxes (VAV boxes, PFP boxes) are handled separately below
                // with zone-specific sizing. Without this guard, terminal boxes would
                // get the full system flow/capacity from the unwrap_or default, then
                // the terminal-specific sizing would skip them (no longer autosize).
                let (loop_flow, loop_heat, loop_cool) = match loop_comp_sizing.get(&name) {
                    Some(&vals) => vals,
                    None => continue,  // Skip terminal boxes and other non-loop components
                };

                // Autosize fan flow rate
                if let Some(_flow) = comp.design_air_flow_rate() {
                    // Fan has a non-autosize value — skip
                } else {
                    comp.set_design_air_flow_rate(loop_flow);
                    info!("Autosized '{}' flow rate: {:.4} m³/s", name, loop_flow);
                }

                // Autosize coil capacities
                if lname.contains("heat") || lname.contains("furnace")
                    || lname.contains("preheat") || lname.contains("reheat")
                    || lname.starts_with("hc ") || lname.starts_with("hc_") {
                    if let Some(cap) = comp.nominal_capacity() {
                        if is_autosize(cap) {
                            comp.set_nominal_capacity(loop_heat);
                            info!("Autosized '{}' capacity: {:.0} W ({:.1} kW)",
                                name, loop_heat, loop_heat / 1000.0);
                        }
                    }
                }
                if lname.contains("cool") || lname.contains("dx")
                    || lname.starts_with("cc ") || lname.starts_with("cc_") {
                    if let Some(cap) = comp.nominal_capacity() {
                        if is_autosize(cap) {
                            comp.set_nominal_capacity(loop_cool);
                            info!("Autosized '{}' capacity: {:.0} W ({:.1} kW)",
                                name, loop_cool, loop_cool / 1000.0);
                        }
                    }
                }
            }

            // ── Terminal Box Sizing ───────────────────────────────────────
            //
            // Terminal boxes (VAV boxes, PFP boxes) are per-zone components
            // that need zone-specific sizing for airflow and reheat capacity.
            //
            // Unlike AHU components (sized to system-wide peaks), terminals
            // are sized to their individual zone's peak loads:
            //   max_air_flow    = zone peak design airflow [kg/s]
            //   reheat_capacity = zone peak heating load [W] × 1.25 safety factor
            for li in &loop_infos {
                for (zone_name, term_name) in &li.terminal_boxes {
                    if let Some(node_idx) = graph.node_by_name(term_name) {
                        if let GraphComponent::Air(comp) = graph.component_mut(node_idx) {
                            // Size terminal max_air_flow from zone design airflow
                            if comp.design_air_flow_rate().is_none() {
                                let zone_flow = sizing_result.zone_design_airflow
                                    .get(zone_name).copied().unwrap_or(0.1);
                                // Set in kg/s — terminal boxes use max_air_flow as mass flow
                                // (compared against inlet.mass_flow in kg/s), unlike fans
                                // which use design_flow_rate in m³/s.
                                comp.set_design_air_flow_rate(zone_flow);
                                info!("Autosized terminal '{}' max airflow: {:.4} kg/s",
                                    term_name, zone_flow);
                            }

                            // Size terminal reheat capacity from zone peak heating load
                            if let Some(cap) = comp.nominal_capacity() {
                                if is_autosize(cap) {
                                    let zone_heat = sizing_result.zone_peak_heating
                                        .get(zone_name).copied().unwrap_or(0.0) * model.simulation.heating_sizing_factor;
                                    comp.set_nominal_capacity(zone_heat);
                                    info!("Autosized terminal '{}' reheat capacity: {:.0} W ({:.1} kW)",
                                        term_name, zone_heat, zone_heat / 1000.0);
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // ── Recompute OA fractions after sizing ──────────────────────────────────
    // At build time, compute_oa_fraction falls back to 20% when the fan is
    // autosize (design_flow = -99999).  Now that fans are autosized we can
    // compute the real fraction from the zone's outdoor_air spec.
    if let Some(ref envelope) = envelope {
        for li in &mut loop_infos {
            // Find the fan in this loop and get its autosized flow [m³/s]
            let fan_flow = li.component_names.iter().find_map(|cname| {
                graph.node_by_name(cname).and_then(|idx| {
                    match graph.component(idx) {
                        GraphComponent::Air(comp) => comp.design_air_flow_rate(),
                        _ => None,
                    }
                })
            });

            if let Some(flow_m3s) = fan_flow {
                if flow_m3s > 0.0 {
                    // Sum outdoor air requirements for served zones
                    let mut total_oa_flow = 0.0_f64;
                    let mut has_oa = false;
                    for zone_name in &li.served_zones {
                        if let Some(zone) = envelope.zones.iter()
                            .find(|z| z.input.name == *zone_name)
                        {
                            if let Some(ref oa) = zone.input.outdoor_air {
                                has_oa = true;
                                let people_count: f64 = zone.input.internal_gains.iter()
                                    .filter_map(|g| match g {
                                        openbse_envelope::InternalGainInput::People { count, .. } => Some(*count),
                                        _ => None,
                                    })
                                    .sum();
                                total_oa_flow += oa.per_person * people_count
                                    + oa.per_area * zone.input.floor_area;
                            }
                        }
                    }
                    if has_oa && !li.explicit_min_oa {
                        let new_frac = (total_oa_flow / flow_m3s).clamp(0.0, 1.0);
                        if (new_frac - li.min_oa_fraction).abs() > 0.001 {
                            info!("Air loop '{}': OA fraction updated {:.1}% → {:.1}% (after sizing)",
                                li.name, li.min_oa_fraction * 100.0, new_frac * 100.0);
                            li.min_oa_fraction = new_frac;
                        }
                    }
                }
            }
        }
    }

    // Check if envelope uses ideal loads (ASHRAE 140 mode)
    let uses_ideal_loads = envelope.as_ref()
        .map(|env| env.has_ideal_loads())
        .unwrap_or(false);

    if uses_ideal_loads {
        info!("Ideal loads air system detected — envelope handles HVAC directly");

        // For summary report, use ideal loads setpoints if zone_groups don't specify
        if let Some(ref env) = envelope {
            for zone in &env.zones {
                if let Some(ref il) = zone.input.ideal_loads {
                    zone_heating_setpoints.entry(zone.input.name.clone())
                        .or_insert(il.heating_setpoint);
                    zone_cooling_setpoints.entry(zone.input.name.clone())
                        .or_insert(il.cooling_setpoint);
                }
            }
        }
    }

    // ── 6. Set up output writers ──────────────────────────────────────────
    let mut output_writers: Vec<OutputWriter> = model.outputs.iter()
        .map(|cfg| OutputWriter::new(cfg.clone()))
        .collect();

    let mut summary_report = if model.summary_report {
        let mut report = SummaryReport::new(
            zone_heating_setpoints.clone(),
            zone_cooling_setpoints.clone(),
        );
        // Pass envelope area data for WWR reporting
        if let Some(ref env) = envelope {
            report.set_envelope_areas(env.envelope_areas.clone());
        }
        Some(report)
    } else {
        None
    };

    info!(
        "Output: {} custom file(s), summary report: {}",
        output_writers.len(),
        if summary_report.is_some() { "enabled" } else { "disabled" }
    );

    // ── 7. Run the simulation loop ──────────────────────────────────────────
    info!("Starting simulation...");
    let mut results: Vec<TimestepResult> = Vec::with_capacity(total_timesteps as usize);
    let mut sim_time = start_hour as f64 * 3600.0;

    // Night-cycle timers: per-loop remaining ON time [seconds].
    //
    // E+ AvailabilityManager:NightCycle uses cycling_run_time = 1800 s (30 min).
    // Once night-cycle triggers, the system stays ON for this duration before
    // rechecking. This prevents destructive ON/OFF oscillation at sub-hourly
    // timesteps where the system would heat the zone above the trigger point,
    // turn off, let the zone crash, and repeat — wasting energy recharging
    // thermal mass each cycle.
    let mut nightcycle_timers: HashMap<String, f64> = HashMap::new();

    // ── 7a. Warmup: repeat first week of weather until surfaces stabilize ──
    //
    // E+ runs 25+ warmup days, repeating the first simulation day until
    // zone temps converge.  Without warmup, zones start at 21°C but
    // surfaces (especially ground slabs) haven't equilibrated — they
    // cool rapidly, creating enormous heat losses that persist for
    // weeks and cause massive HVAC overconsumption.
    //
    // We repeat the first 7 days of weather (one full week for correct
    // schedule cycling) for up to 4 repetitions (28 warmup days).
    // Convergence is checked at the end of each 7-day cycle.
    let warmup_period = 7_u32 * 24; // 7 days of weather to cycle through
    let max_warmup_reps = 4_u32;    // Up to 28 warmup days

    if envelope.is_some() {
        info!("Running warmup (up to {} days)...", max_warmup_reps * 7);
        let env = envelope.as_mut().unwrap();

        for rep in 0..max_warmup_reps {
            // Save zone temps at start of this warmup week
            let temps_before: Vec<f64> = env.zones.iter().map(|z| z.temp).collect();

            for warmup_hour_idx in 0..warmup_period {
                let w_hour_idx = warmup_hour_idx as usize;
                let weather_hour = &weather_data.hours[w_hour_idx];
                let prev_w_hour_idx = if w_hour_idx > 0 { w_hour_idx - 1 } else { warmup_period as usize - 1 };
                let prev_weather = &weather_data.hours[prev_w_hour_idx];
                let (month, day) = month_day_from_hour(warmup_hour_idx, &days_in_months);
                let hour = (warmup_hour_idx % 24) + 1;

                for sub in 1..=config.timesteps_per_hour {
                    let interp_frac = sub as f64 / config.timesteps_per_hour as f64;
                    let interp_weather = prev_weather.interpolate(weather_hour, interp_frac);
                    let outdoor_air = interp_weather.to_air_state();
                    let t_outdoor = interp_weather.dry_bulb;

                    let ctx = SimulationContext {
                        timestep: TimeStep {
                            month, day, hour, sub_hour: sub,
                            timesteps_per_hour: config.timesteps_per_hour,
                            sim_time_s: sim_time,
                            dt,
                        },
                        outdoor_air,
                        day_type: DayType::WeatherDay,
                        is_sizing: false,
                        sizing_internal_gains: SizingInternalGains::Full,
                    };

                    let dow = openbse_envelope::schedule::day_of_week(month, day, env.jan1_dow);

                    // Build zone state maps for HVAC
                    let current_zone_temps: HashMap<String, f64> = env.zones.iter()
                        .map(|z| (z.input.name.clone(), z.temp))
                        .collect();
                    let initial_zone_temps: HashMap<String, f64> = current_zone_temps.clone();
                    let current_cooling_loads: HashMap<String, f64> = env.zones.iter()
                        .map(|z| (z.input.name.clone(), z.ideal_cooling_load))
                        .collect();
                    let current_heating_loads: HashMap<String, f64> = env.zones.iter()
                        .map(|z| (z.input.name.clone(), z.ideal_heating_load))
                        .collect();

                    // Single HVAC pass (no iterating during warmup — faster)
                    let (_, zone_supply_conditions) = simulate_all_loops(
                        &mut graph,
                        &ctx,
                        &loop_infos,
                        &current_zone_temps,
                        &zone_heating_setpoints,
                        &zone_cooling_setpoints,
                        &zone_unocc_heating_setpoints,
                        &zone_unocc_cooling_setpoints,
                        &zone_design_flows,
                        &zone_multipliers,
                        t_outdoor,
                        Some(&env.schedule_manager),
                        hour,
                        dow,
                        &mut nightcycle_timers,
                        dt,
                        &current_cooling_loads,
                        &current_heating_loads,
                        &initial_zone_temps,
                    );

                    // Skip plant loop during warmup — it only affects energy
                    // accounting, not zone temperature equilibration.

                    // Build HVAC conditions for envelope
                    let mut hvac_conds = ZoneHvacConditions::default();

                    for (zone_name, (supply_temp, mass_flow)) in &zone_supply_conditions {
                        hvac_conds.supply_temps.insert(zone_name.clone(), *supply_temp);
                        hvac_conds.supply_mass_flows.insert(zone_name.clone(), *mass_flow);
                    }
                    // Populate OA handling flags for warmup too
                    for li in &loop_infos {
                        let handles_oa = li.min_oa_fraction > 0.001;
                        for zone_name in &li.served_zones {
                            hvac_conds.oa_handled_by_hvac.insert(zone_name.clone(), handles_oa);
                        }
                    }
                    hvac_conds.cooling_setpoints = zone_cooling_setpoints.clone();
                    hvac_conds.heating_setpoints = zone_heating_setpoints.clone();

                    // Solve envelope (updates zone temps, surface temps, CTF history)
                    env.solve_timestep(&ctx, &interp_weather, &hvac_conds);

                    sim_time += dt;
                }
            }

            // Check warmup convergence: max zone temp change from start to end of this week
            let max_delta: f64 = env.zones.iter()
                .zip(temps_before.iter())
                .map(|(z, &t_before)| (z.temp - t_before).abs())
                .fold(0.0_f64, f64::max);

            info!("Warmup rep {}/{}: max zone temp delta = {:.3}°C", rep + 1, max_warmup_reps, max_delta);

            if max_delta < 0.5 {
                info!("Warmup converged after {} days", (rep + 1) * 7);
                break;
            }
        }

        // Reset sim_time for actual simulation
        sim_time = start_hour as f64 * 3600.0;
        // Reset nightcycle timers (start fresh for actual simulation)
        nightcycle_timers.clear();
    }

    info!("Starting main simulation...");

    for hour_idx in start_hour..end_hour {
        let weather_hour = &weather_data.hours[hour_idx as usize];
        // Previous hour for sub-hourly interpolation (wraps to last hour of year)
        let prev_hour_idx = if hour_idx > 0 { hour_idx - 1 } else { weather_data.hours.len() as u32 - 1 };
        let prev_weather = &weather_data.hours[prev_hour_idx as usize];
        let (month, day) = month_day_from_hour(hour_idx, &days_in_months);
        let hour = (hour_idx % 24) + 1;

        for sub in 1..=config.timesteps_per_hour {
            // Sub-hourly weather interpolation (matches E+ WeatherManager.cc):
            // EPW data for hour h covers period h-1 to h.
            // For sub-step s of N: frac = s/N, interpolating prev→current.
            // At s=N (end of hour), we get the current hour's value exactly.
            let interp_frac = sub as f64 / config.timesteps_per_hour as f64;
            let interp_weather = prev_weather.interpolate(weather_hour, interp_frac);
            let outdoor_air = interp_weather.to_air_state();

            let ctx = SimulationContext {
                timestep: TimeStep {
                    month, day, hour, sub_hour: sub,
                    timesteps_per_hour: config.timesteps_per_hour,
                    sim_time_s: sim_time,
                    dt,
                },
                outdoor_air,
                day_type: DayType::WeatherDay,
                is_sizing: false,
                sizing_internal_gains: SizingInternalGains::Full,
            };

            if let Some(ref mut env) = envelope {
                let t_outdoor = interp_weather.dry_bulb;

                // Build HVAC conditions and solve envelope
                let has_external_hvac = !resolved_thermostats.is_empty();
                let (env_result, hvac_result) = if uses_ideal_loads || !has_external_hvac {
                    // ═══════════════════════════════════════════════════════
                    // IDEAL LOADS or FREE-FLOAT MODE
                    // ═══════════════════════════════════════════════════════
                    let hvac_conds = ZoneHvacConditions::default();
                    let env_result = env.solve_timestep(&ctx, &interp_weather, &hvac_conds);

                    let result = TimestepResult {
                        month, day, hour, sub_hour: sub,
                        component_outputs: HashMap::new(),
                    };
                    (env_result, result)
                } else {
                    // ═══════════════════════════════════════════════════════
                    // COUPLED ENVELOPE + HVAC SIMULATION
                    // Multi-loop aware: dispatches to PSZ-AC, DOAS, FCU, VAV
                    // control strategies based on each loop's system type.
                    // ═══════════════════════════════════════════════════════

                    // Compute day-of-week for schedule lookups
                    let dow = openbse_envelope::schedule::day_of_week(month, day, env.jan1_dow);

                    // ── Predictor-Corrector HVAC-Envelope Iteration ──
                    //
                    // The HVAC system response depends on zone temperature
                    // (return air temp → coil inlet → cooling capacity) and the
                    // zone temperature depends on HVAC supply conditions. A single
                    // sequential pass uses stale zone temps from the previous
                    // timestep, which can cause the zone to oscillate or settle
                    // at the wrong temperature.
                    //
                    // We iterate: HVAC → Envelope → (check convergence) → repeat.
                    // Typically converges in 2-3 iterations.
                    const MAX_HVAC_ITER: usize = 10;
                    const HVAC_CONV_TOL: f64 = 0.05; // °C

                    let mut current_zone_temps: HashMap<String, f64> = env.zones.iter()
                        .map(|z| (z.input.name.clone(), z.temp))
                        .collect();
                    // Save initial zone temps for terminal control signals (frozen across
                    // HVAC iterations to prevent oscillation).  AHU-level controls use
                    // the updated current_zone_temps for SAT reset/economizer convergence,
                    // but terminal control signals must be stable across iterations.
                    let initial_zone_temps: HashMap<String, f64> = current_zone_temps.clone();

                    // Ideal loads at setpoint from previous timestep (used for load-based PLR).
                    // These are FROZEN across HVAC iterations — they don't change because
                    // the loads are computed once and the terminal signals use frozen zone
                    // temps.  This prevents the oscillation where the ideal load changes
                    // as the zone temp changes between iterations.
                    let current_cooling_loads: HashMap<String, f64> = env.zones.iter()
                        .map(|z| (z.input.name.clone(), z.ideal_cooling_load))
                        .collect();
                    let current_heating_loads: HashMap<String, f64> = env.zones.iter()
                        .map(|z| (z.input.name.clone(), z.ideal_heating_load))
                        .collect();

                    let mut final_hvac_result = None;
                    let mut final_env_result = None;

                    for _hvac_iter in 0..MAX_HVAC_ITER {
                        // Step 1: Run HVAC with current zone temps and loads
                        let (mut hvac_result, zone_supply_conditions) = simulate_all_loops(
                            &mut graph,
                            &ctx,
                            &loop_infos,
                            &current_zone_temps,
                            &zone_heating_setpoints,
                            &zone_cooling_setpoints,
                            &zone_unocc_heating_setpoints,
                            &zone_unocc_cooling_setpoints,
                            &zone_design_flows,
                            &zone_multipliers,
                            t_outdoor,
                            Some(&env.schedule_manager),
                            hour,
                            dow,
                            &mut nightcycle_timers,
                            dt,
                            &current_cooling_loads,
                            &current_heating_loads,
                            &initial_zone_temps,
                        );

                        // Step 1b: Run plant loops (single-pass coupling).
                        //
                        // Collect water-side demand from air components (CHW coils,
                        // HW reheat coils, etc.) and pass to plant equipment. Plant
                        // supply water feeds back to coils on the NEXT iteration.
                        if !model.plant_loops.is_empty() {
                            // Identify condenser water loops (referenced by chillers).
                            // These are simulated in a second pass after chiller outputs
                            // are known, so skip them in the primary pass.
                            let condenser_loop_names: std::collections::HashSet<&str> =
                                model.plant_loops.iter()
                                    .flat_map(|pl| pl.supply_equipment.iter())
                                    .filter_map(|eq| {
                                        if let openbse_io::input::PlantEquipmentInput::Chiller(c) = eq {
                                            c.condenser_plant_loop.as_deref()
                                        } else {
                                            None
                                        }
                                    })
                                    .collect();

                            // First pass: simulate non-condenser plant loops
                            // (demand from air-side coils and terminal box reheat).
                            for plant_loop in &model.plant_loops {
                                if condenser_loop_names.contains(plant_loop.name.as_str()) {
                                    continue; // handled in second pass
                                }
                                let mut total_load = 0.0_f64;

                                // Sum thermal output from all coils that reference this plant loop.
                                // Apply zone multipliers: per-zone coils (PTAC, FCU, VAV box
                                // reheat) must be multiplied by the zone multiplier since
                                // multiplied zones represent multiple identical units.
                                for al in &model.air_loops {
                                    // Check main equipment (cooling coils, heating coils)
                                    for eq in &al.equipment {
                                        let (coil_name, coil_plant) = match eq {
                                            openbse_io::input::EquipmentInput::CoolingCoil(c) => {
                                                (c.name.as_str(), c.plant_loop.as_deref())
                                            }
                                            openbse_io::input::EquipmentInput::HeatingCoil(c) => {
                                                (c.name.as_str(), c.plant_loop.as_deref())
                                            }
                                            _ => ("", None),
                                        };
                                        if coil_plant == Some(plant_loop.name.as_str()) {
                                            if let Some(outputs) = hvac_result.component_outputs.get(coil_name) {
                                                let mult = component_multipliers.get(coil_name)
                                                    .copied().unwrap_or(1.0);
                                                total_load += outputs.get("thermal_output")
                                                    .copied().unwrap_or(0.0) * mult;
                                            }
                                        }
                                    }
                                    // Check terminal boxes (VAV box reheat coils)
                                    for zc in &al.zone_terminals {
                                        if let Some(ref terminal) = zc.terminal {
                                            let (term_name, term_plant) = match terminal {
                                                openbse_io::input::TerminalInput::VavBox(vb) => {
                                                    (vb.name.as_str(), vb.plant_loop.as_deref())
                                                }
                                                _ => ("", None),
                                            };
                                            if term_plant == Some(plant_loop.name.as_str()) {
                                                if let Some(outputs) = hvac_result.component_outputs.get(term_name) {
                                                    let mult = zone_multipliers.get(&zc.zone)
                                                        .copied().unwrap_or(1.0);
                                                    total_load += outputs.get("thermal_output")
                                                        .copied().unwrap_or(0.0) * mult;
                                                }
                                            }
                                        }
                                    }
                                }

                                // Simulate plant equipment with sequential loading
                                // (lead equipment takes load up to capacity, remainder to lag)
                                if total_load.abs() > 0.0 {
                                    // Compute plant loop water mass flow from thermal demand
                                    // mass_flow = Q / (cp * delta_T)
                                    let cp_water = 4186.0; // J/(kg·K)
                                    let loop_delta_t = plant_loop.design_delta_t.max(1.0); // avoid div by zero
                                    let loop_mass_flow = total_load.abs() / (cp_water * loop_delta_t);
                                    // Inlet water temperature: for heating loops, return water is colder
                                    // For cooling loops, return water is warmer
                                    let inlet_temp = if total_load > 0.0 {
                                        plant_loop.design_supply_temp - loop_delta_t // HHW return temp
                                    } else {
                                        plant_loop.design_supply_temp + loop_delta_t // CHW return temp
                                    };

                                    // Autosize pump design_flow_rate on first call:
                                    // design_flow = total_capacity / (rho * cp * delta_T)
                                    let rho_water = 998.0; // kg/m³
                                    for equip in &plant_loop.supply_equipment {
                                        if let openbse_io::input::PlantEquipmentInput::Pump(p) = equip {
                                            if let Some(node_idx) = graph.node_by_name(&p.name) {
                                                if let GraphComponent::Plant(component) = graph.component_mut(node_idx) {
                                                    if component.design_water_flow_rate().is_none() {
                                                        // Sum capacities of thermal equipment in this loop
                                                        let total_cap: f64 = plant_loop.supply_equipment.iter()
                                                            .filter_map(|eq| match eq {
                                                                openbse_io::input::PlantEquipmentInput::Boiler(b) => Some(b.capacity.to_f64()),
                                                                openbse_io::input::PlantEquipmentInput::Chiller(c) => Some(c.capacity.to_f64()),
                                                                _ => None,
                                                            })
                                                            .filter(|c| *c > 0.0) // exclude autosize sentinels
                                                            .sum();
                                                        let design_flow = total_cap / (rho_water * cp_water * loop_delta_t);
                                                        component.set_design_water_flow_rate(design_flow);
                                                    }
                                                }
                                            }
                                        }
                                    }

                                    let mut remaining_load = total_load;
                                    let mut current_inlet = WaterPort::new(
                                        openbse_psychrometrics::FluidState::water(inlet_temp, loop_mass_flow)
                                    );
                                    for equip in &plant_loop.supply_equipment {
                                        let equip_name = match equip {
                                            openbse_io::input::PlantEquipmentInput::Boiler(b) => &b.name,
                                            openbse_io::input::PlantEquipmentInput::Chiller(c) => &c.name,
                                            openbse_io::input::PlantEquipmentInput::Pump(p) => &p.name,
                                        };
                                        // Pumps always run when there's demand; thermal equipment
                                        // stops when load is met
                                        let is_pump = matches!(equip, openbse_io::input::PlantEquipmentInput::Pump(_));
                                        if !is_pump && remaining_load.abs() < 1.0 { break; } // < 1W = done
                                        if let Some(node_idx) = graph.node_by_name(equip_name) {
                                            if let GraphComponent::Plant(component) = graph.component_mut(node_idx) {
                                                // Pass absolute load to equipment — chillers and
                                                // boilers always receive positive load values.
                                                // The sign of remaining_load tracks heating (+) vs
                                                // cooling (-) demand direction.
                                                let equip_load = if is_pump { total_load.abs() } else { remaining_load.abs() };
                                                let outlet = component.simulate_plant(&current_inlet, equip_load, &ctx);
                                                // Chain outlet → next inlet (pump → boiler → ...)
                                                current_inlet = outlet;

                                                // Record plant outputs
                                                let delivered = component.thermal_output().abs();
                                                let mut plant_outputs: HashMap<String, f64> = HashMap::new();
                                                plant_outputs.insert("electric_power".to_string(), component.power_consumption());
                                                plant_outputs.insert("fuel_power".to_string(), component.fuel_consumption());
                                                plant_outputs.insert("thermal_output".to_string(), delivered);
                                                hvac_result.component_outputs.insert(equip_name.clone(), plant_outputs);

                                                // Reduce remaining load by what this equipment delivered
                                                if remaining_load > 0.0 {
                                                    remaining_load -= delivered;
                                                } else {
                                                    remaining_load += delivered;
                                                }
                                            }
                                        }
                                    }
                                }
                            }

                            // Second pass: simulate condenser water loops.
                            // Demand = chiller condenser heat rejection (cooling + compressor power).
                            for plant_loop in &model.plant_loops {
                                if !condenser_loop_names.contains(plant_loop.name.as_str()) {
                                    continue; // already simulated in first pass
                                }
                                // Sum condenser heat rejection from all chillers referencing this loop
                                let mut condenser_load = 0.0_f64;
                                for other_loop in &model.plant_loops {
                                    for eq in &other_loop.supply_equipment {
                                        if let openbse_io::input::PlantEquipmentInput::Chiller(c) = eq {
                                            if c.condenser_plant_loop.as_deref() == Some(plant_loop.name.as_str()) {
                                                if let Some(outputs) = hvac_result.component_outputs.get(c.name.as_str()) {
                                                    let thermal = outputs.get("thermal_output").copied().unwrap_or(0.0);
                                                    let electric = outputs.get("electric_power").copied().unwrap_or(0.0);
                                                    condenser_load += thermal + electric;
                                                }
                                            }
                                        }
                                    }
                                }

                                if condenser_load > 0.0 {
                                    let cp_water = 4186.0;
                                    let loop_delta_t = plant_loop.design_delta_t.max(1.0);
                                    let loop_mass_flow = condenser_load / (cp_water * loop_delta_t);
                                    let inlet_temp = plant_loop.design_supply_temp + loop_delta_t; // condenser return (warmer)

                                    // Autosize condenser pumps using design chiller capacities
                                    // (not instantaneous load) to get correct design_power.
                                    let rho_water = 998.0;
                                    for equip in &plant_loop.supply_equipment {
                                        if let openbse_io::input::PlantEquipmentInput::Pump(p) = equip {
                                            if let Some(node_idx) = graph.node_by_name(&p.name) {
                                                if let GraphComponent::Plant(component) = graph.component_mut(node_idx) {
                                                    if component.design_water_flow_rate().is_none() {
                                                        // Sum design capacities of all chillers
                                                        // on this condenser loop. Q_cond = Q_evap × (1 + 1/COP).
                                                        let mut total_cond_cap = 0.0_f64;
                                                        for other_loop in &model.plant_loops {
                                                            for eq2 in &other_loop.supply_equipment {
                                                                if let openbse_io::input::PlantEquipmentInput::Chiller(c) = eq2 {
                                                                    if c.condenser_plant_loop.as_deref() == Some(plant_loop.name.as_str()) {
                                                                        let cap = c.capacity.to_f64();
                                                                        if cap > 0.0 {
                                                                            total_cond_cap += cap * (1.0 + 1.0 / c.cop);
                                                                        }
                                                                    }
                                                                }
                                                            }
                                                        }
                                                        let design_flow = total_cond_cap / (rho_water * cp_water * loop_delta_t);
                                                        component.set_design_water_flow_rate(design_flow);
                                                    }
                                                }
                                            }
                                        }
                                    }

                                    let mut current_inlet = WaterPort::new(
                                        openbse_psychrometrics::FluidState::water(inlet_temp, loop_mass_flow)
                                    );
                                    for equip in &plant_loop.supply_equipment {
                                        let equip_name = match equip {
                                            openbse_io::input::PlantEquipmentInput::Boiler(b) => &b.name,
                                            openbse_io::input::PlantEquipmentInput::Chiller(c) => &c.name,
                                            openbse_io::input::PlantEquipmentInput::Pump(p) => &p.name,
                                        };
                                        if let Some(node_idx) = graph.node_by_name(equip_name) {
                                            if let GraphComponent::Plant(component) = graph.component_mut(node_idx) {
                                                let outlet = component.simulate_plant(&current_inlet, condenser_load, &ctx);
                                                current_inlet = outlet;

                                                let mut plant_outputs: HashMap<String, f64> = HashMap::new();
                                                plant_outputs.insert("electric_power".to_string(), component.power_consumption());
                                                plant_outputs.insert("fuel_power".to_string(), component.fuel_consumption());
                                                plant_outputs.insert("thermal_output".to_string(), component.thermal_output().abs());
                                                hvac_result.component_outputs.insert(equip_name.clone(), plant_outputs);
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        // Step 2: Deliver HVAC supply air to envelope
                        let mut hvac_conds = ZoneHvacConditions::default();
    
                        for (zone_name, (supply_temp, mass_flow)) in &zone_supply_conditions {
                            let zone_conditioned = env.zones.iter()
                                .find(|z| z.input.name == *zone_name)
                                .map(|z| z.input.conditioned)
                                .unwrap_or(true);

                            if zone_conditioned {
                                hvac_conds.supply_temps.insert(zone_name.clone(), *supply_temp);
                                hvac_conds.supply_mass_flows.insert(zone_name.clone(), *mass_flow);
                            }
                        }
                        // Tell the envelope which zones have HVAC-handled OA.
                        // If a zone's air loop has min_oa_fraction > 0, HVAC handles OA
                        // and zone-level OA should be suppressed. If min_oa_fraction == 0,
                        // zone OA flows directly (like E+ separate ERV configuration).
                        for li in &loop_infos {
                            let handles_oa = li.min_oa_fraction > 0.001;
                            for zone_name in &li.served_zones {
                                hvac_conds.oa_handled_by_hvac.insert(zone_name.clone(), handles_oa);
                            }
                        }
                        // Pass setpoints so envelope can compute ideal loads at setpoint
                        hvac_conds.cooling_setpoints = zone_cooling_setpoints.clone();
                        hvac_conds.heating_setpoints = zone_heating_setpoints.clone();
                        // Step 3: Solve envelope with HVAC supply
                        let env_result = env.solve_timestep(&ctx, &interp_weather, &hvac_conds);

                        // Step 4: Check convergence — did zone temps change?
                        let max_delta: f64 = env_result.zone_temps.iter()
                            .map(|(name, &new_temp)| {
                                let old_temp = current_zone_temps.get(name).copied().unwrap_or(new_temp);
                                (new_temp - old_temp).abs()
                            })
                            .fold(0.0_f64, f64::max);

                        // Update zone temps for next HVAC iteration.
                        // AHU-level controls (SAT reset, economizer) use
                        // converging zone temps.  Terminal control signals
                        // use FROZEN initial_zone_temps (set before the loop)
                        // to prevent oscillation.
                        current_zone_temps = env_result.zone_temps.iter()
                            .map(|(k, &v)| (k.clone(), v))
                            .collect();
                        // NOTE: current_cooling_loads and current_heating_loads
                        // are intentionally NOT updated — they're frozen at
                        // pre-HVAC values to keep terminal signals stable.

                        final_hvac_result = Some(hvac_result);
                        final_env_result = Some(env_result);

                        if max_delta <= HVAC_CONV_TOL {
                            break;
                        }
                    }

                    (final_env_result.unwrap(), final_hvac_result.unwrap())
                };

                // ── Assemble timestep result ──────────────────────────
                let mut result = hvac_result;

                result.component_outputs
                    .entry("Weather".to_string())
                    .or_default()
                    .insert("outdoor_temp".to_string(), t_outdoor);

                for (zone_name, outputs) in env_result.zone_outputs {
                    result.component_outputs.insert(zone_name, outputs);
                }
                for (name, &temp) in &env_result.zone_temps {
                    result.component_outputs
                        .entry(name.clone())
                        .or_default()
                        .insert("zone_temp".to_string(), temp);
                }
                for (name, &load) in &env_result.zone_heating_loads {
                    result.component_outputs
                        .entry(name.clone())
                        .or_default()
                        .insert("heating_load".to_string(), load);
                }
                for (name, &load) in &env_result.zone_cooling_loads {
                    result.component_outputs
                        .entry(name.clone())
                        .or_default()
                        .insert("cooling_load".to_string(), load);
                }

                // ── Build output snapshot ─────────────────────────────
                let mut snapshot = OutputSnapshot::new(month, day, hour, sub, dt);

                snapshot.site_outdoor_temperature = t_outdoor;
                snapshot.site_wind_speed = interp_weather.wind_speed;
                snapshot.site_direct_normal_radiation = interp_weather.direct_normal_rad;
                snapshot.site_diffuse_horizontal_radiation = interp_weather.diffuse_horiz_rad;
                snapshot.site_relative_humidity = interp_weather.rel_humidity;

                for zone in &env.zones {
                    let name = zone.input.name.clone();
                    let mult = zone.input.multiplier as f64;
                    snapshot.zone_temperature.insert(name.clone(), zone.temp);
                    snapshot.zone_humidity_ratio.insert(name.clone(), zone.humidity_ratio);
                    snapshot.zone_heating_rate.insert(name.clone(), zone.heating_load * mult);
                    snapshot.zone_cooling_rate.insert(name.clone(), zone.cooling_load * mult);
                    snapshot.zone_infiltration_mass_flow.insert(name.clone(), zone.infiltration_mass_flow * mult);
                    snapshot.zone_nat_vent_flow.insert(name.clone(), zone.nat_vent_flow * mult);
                    snapshot.zone_nat_vent_mass_flow.insert(name.clone(), zone.nat_vent_mass_flow * mult);
                    snapshot.zone_nat_vent_active.insert(name.clone(), if zone.nat_vent_active { 1.0 } else { 0.0 });
                    snapshot.zone_internal_gains_convective.insert(name.clone(), zone.q_internal_conv * mult);
                    snapshot.zone_internal_gains_radiative.insert(name.clone(), zone.q_internal_rad * mult);
                    snapshot.zone_supply_air_temperature.insert(name.clone(), zone.supply_air_temp);
                    snapshot.zone_supply_air_mass_flow.insert(name.clone(), zone.supply_air_mass_flow * mult);

                    // Active setpoints for this timestep (all zones tracked for unmet hours)
                    let has_setpoints = zone_heating_setpoints.contains_key(&name)
                        || zone_cooling_setpoints.contains_key(&name);
                    let is_conditioned = zone.input.conditioned;

                    if has_setpoints && is_conditioned {
                        let (heat_sp, cool_sp) = zone.input.active_setpoints(hour);
                        snapshot.zone_heating_setpoint.insert(name.clone(), heat_sp);
                        snapshot.zone_cooling_setpoint.insert(name.clone(), cool_sp);
                    }
                }

                for surface in &env.surfaces {
                    let name = surface.input.name.clone();
                    snapshot.surface_inside_temperature.insert(name.clone(), surface.temp_inside);
                    snapshot.surface_outside_temperature.insert(name.clone(), surface.temp_outside);
                    snapshot.surface_inside_convection_coefficient.insert(name.clone(), surface.h_conv_inside);
                    // Apply zone multiplier to solar diagnostics so building totals
                    // correctly count multiplied zones (e.g., M floor mult=2).
                    let surf_mult = zone_multipliers.get(&surface.input.zone)
                        .copied().unwrap_or(1.0);
                    snapshot.surface_incident_solar.insert(name.clone(), surface.incident_solar * surf_mult);
                    snapshot.surface_transmitted_solar.insert(name.clone(), surface.transmitted_solar * surf_mult);
                    // q_cond_inside from apply_ctf is W/m², multiply by net_area for total [W]
                    snapshot.surface_conduction_inside.insert(name.clone(), surface.q_cond_inside * surface.net_area * surf_mult);
                    // q_conv_inside is also W/m², multiply by net_area for total [W]
                    snapshot.surface_convection_inside.insert(name.clone(), surface.q_conv_inside * surface.net_area * surf_mult);
                }

                for (comp_name, vars) in &result.component_outputs {
                    if comp_name == "Weather" { continue; }
                    if let Some(&temp) = vars.get("outlet_temp") {
                        snapshot.air_loop_outlet_temperature.insert(comp_name.clone(), temp);
                    }
                    if let Some(&flow) = vars.get("mass_flow") {
                        snapshot.air_loop_mass_flow.insert(comp_name.clone(), flow);
                    }
                    if let Some(&w) = vars.get("outlet_w") {
                        snapshot.air_loop_outlet_humidity_ratio.insert(comp_name.clone(), w);
                    }
                }

                // Populate energy end-use data (with zone multiplier for per-zone HVAC)
                for (comp_name, vars) in &result.component_outputs {
                    if comp_name == "Weather" { continue; }
                    let mult = component_multipliers.get(comp_name).copied().unwrap_or(1.0);
                    if let Some(&pw) = vars.get("electric_power") {
                        let pw_mult = pw * mult;
                        if pump_names.contains(comp_name) {
                            snapshot.pump_electric_power.insert(comp_name.clone(), pw_mult);
                        } else if humidifier_names.contains(comp_name) {
                            snapshot.humidification_power.insert(comp_name.clone(), pw_mult);
                        } else {
                            snapshot.component_electric_power.insert(comp_name.clone(), pw_mult);
                        }
                    }
                    if let Some(&pw) = vars.get("fuel_power") {
                        snapshot.component_fuel_power.insert(comp_name.clone(), pw * mult);
                    }
                }
                // Zone internal gains — separate lighting and equipment energy
                for zone in &env.zones {
                    let mult = zone.input.multiplier as f64;
                    snapshot.zone_lighting_power.insert(zone.input.name.clone(), zone.lighting_power * mult);
                    snapshot.zone_equipment_power.insert(zone.input.name.clone(), zone.equipment_power * mult);
                }

                // ── DHW simulation ─────────────────────────────────────
                // Simulate domestic hot water systems and add energy to snapshot.
                let dhw_dow = openbse_envelope::schedule::day_of_week(month, day, env.jan1_dow);
                for (dhw_sys, dhw_input) in dhw_systems.iter_mut().zip(&model.dhw_systems) {
                    // Compute current draw rate from schedule.
                    // E+ WaterUse:Equipment mixes hot water from the tank with cold
                    // mains water at the fixture to reach the target use_temperature.
                    // The HOT water drawn from the tank is only a fraction of the
                    // total fixture flow: hot_frac = (T_use - T_mains) / (T_hot - T_mains).
                    let t_hot = dhw_sys.setpoint_temp; // tank setpoint
                    let t_mains = dhw_input.mains_temperature;
                    let total_draw: f64 = dhw_input.loads.iter().map(|load| {
                        let frac = load.schedule.as_ref().map(|sched_name| {
                            env.schedule_manager.fraction(sched_name, hour, dhw_dow)
                        }).unwrap_or(1.0);
                        let fixture_flow = load.peak_flow_rate * frac;
                        // Compute hot water fraction drawn from tank
                        let hot_frac = if t_hot > t_mains {
                            ((load.use_temperature - t_mains) / (t_hot - t_mains)).clamp(0.0, 1.0)
                        } else {
                            1.0
                        };
                        fixture_flow * hot_frac
                    }).sum();

                    dhw_sys.simulate(total_draw, dhw_input.mains_temperature, dt);

                    let ep = dhw_sys.electric_power();
                    let fp = dhw_sys.fuel_power();
                    if ep > 0.0 {
                        snapshot.dhw_electric_power.insert(dhw_sys.name.clone(), ep);
                    }
                    if fp > 0.0 {
                        snapshot.dhw_fuel_power.insert(dhw_sys.name.clone(), fp);
                    }

                    // SWH circulation pump — runs whenever there is a DHW draw
                    if let Some(ref pump_input) = dhw_input.pump {
                        if total_draw > 0.0 {
                            // Pump design flow = sum of peak draw rates [L/s → m³/s]
                            let design_flow_m3s = pump_input.design_flow_rate.to_f64();
                            let total_eff = pump_input.motor_efficiency * pump_input.impeller_efficiency;
                            let design_power = design_flow_m3s * pump_input.design_head / total_eff;
                            let pump_power = if pump_input.pump_type == "constant_speed" {
                                design_power
                            } else {
                                // Variable speed: scale by flow fraction cubed
                                let total_peak: f64 = dhw_input.loads.iter()
                                    .map(|l| l.peak_flow_rate).sum();
                                let flow_frac = (total_draw / total_peak.max(1e-10)).clamp(0.1, 1.0);
                                design_power * flow_frac.powi(3)
                            };
                            snapshot.pump_electric_power.insert(
                                pump_input.name.clone(), pump_power,
                            );
                        }
                    }
                }

                // ── Exterior equipment ────────────────────────────────────
                for ext in &model.exterior_equipment {
                    let frac = ext.schedule.as_ref()
                        .map(|s| env.schedule_manager.fraction(s, hour, dhw_dow))
                        .unwrap_or(1.0);
                    let mut power = ext.power * frac;
                    // AstronomicalClock: exterior lights only on during nighttime
                    if ext.astronomical_clock && power > 0.0 {
                        let doy = (hour_idx / 24) + 1;
                        // Use standard time hour (mid-timestep for hourly)
                        let solar_hr = (hour_idx % 24) as f64 + 0.5;
                        let sol = openbse_envelope::solar::solar_position(
                            doy, solar_hr, weather_data.location.latitude,
                        );
                        if sol.is_sunup {
                            power = 0.0; // lights off during daytime
                        }
                    }
                    // Route exterior lights to ext_lighting, everything else to ext_equipment
                    let is_ext_lights = ext.subcategory.as_deref()
                        .map(|s| s.to_lowercase().contains("light"))
                        .unwrap_or(false);
                    if is_ext_lights {
                        snapshot.ext_lighting_power
                            .entry(ext.name.clone())
                            .and_modify(|v| *v += power)
                            .or_insert(power);
                    } else if ext.fuel == "natural_gas" {
                        snapshot.component_fuel_power
                            .entry(ext.name.clone())
                            .and_modify(|v| *v += power)
                            .or_insert(power);
                    } else {
                        snapshot.ext_equipment_power
                            .entry(ext.name.clone())
                            .and_modify(|v| *v += power)
                            .or_insert(power);
                    }
                }

                for writer in &mut output_writers {
                    writer.add_snapshot(&snapshot);
                }
                if let Some(ref mut report) = summary_report {
                    report.add_snapshot(&snapshot);
                }

                results.push(result);
            } else {
                // ═══════════════════════════════════════════════════════════
                // HVAC-ONLY SIMULATION (no envelope)
                // ═══════════════════════════════════════════════════════════
                let mut signals = ControlSignals::default();
                for control in &model.controls {
                    match control {
                        openbse_io::input::ControlInput::Setpoint(sp) => {
                            signals.coil_setpoints.insert(sp.component.clone(), sp.value);
                        }
                        openbse_io::input::ControlInput::PlantLoopSetpoint(pls) => {
                            signals.plant_loop_setpoints.insert(
                                pls.loop_name.clone(), pls.supply_temp
                            );
                        }
                    }
                }

                let (mut result, _) = simulate_hvac(&mut graph, &ctx, &signals);
                result.component_outputs
                    .entry("Weather".to_string())
                    .or_default()
                    .insert("outdoor_temp".to_string(), interp_weather.dry_bulb);

                // Build snapshot for HVAC-only
                let mut snapshot = OutputSnapshot::new(month, day, hour, sub, dt);
                snapshot.site_outdoor_temperature = interp_weather.dry_bulb;
                snapshot.site_wind_speed = interp_weather.wind_speed;
                snapshot.site_direct_normal_radiation = interp_weather.direct_normal_rad;
                snapshot.site_diffuse_horizontal_radiation = interp_weather.diffuse_horiz_rad;
                snapshot.site_relative_humidity = interp_weather.rel_humidity;

                for (comp_name, vars) in &result.component_outputs {
                    if comp_name == "Weather" { continue; }
                    if let Some(&temp) = vars.get("outlet_temp") {
                        snapshot.air_loop_outlet_temperature.insert(comp_name.clone(), temp);
                    }
                    if let Some(&flow) = vars.get("mass_flow") {
                        snapshot.air_loop_mass_flow.insert(comp_name.clone(), flow);
                    }
                    if let Some(&w) = vars.get("outlet_w") {
                        snapshot.air_loop_outlet_humidity_ratio.insert(comp_name.clone(), w);
                    }
                }

                // Populate energy end-use data (with zone multiplier for per-zone HVAC)
                for (comp_name, vars) in &result.component_outputs {
                    if comp_name == "Weather" { continue; }
                    let mult = component_multipliers.get(comp_name).copied().unwrap_or(1.0);
                    if let Some(&pw) = vars.get("electric_power") {
                        let pw_mult = pw * mult;
                        if pump_names.contains(comp_name) {
                            snapshot.pump_electric_power.insert(comp_name.clone(), pw_mult);
                        } else if humidifier_names.contains(comp_name) {
                            snapshot.humidification_power.insert(comp_name.clone(), pw_mult);
                        } else {
                            snapshot.component_electric_power.insert(comp_name.clone(), pw_mult);
                        }
                    }
                    if let Some(&pw) = vars.get("fuel_power") {
                        snapshot.component_fuel_power.insert(comp_name.clone(), pw * mult);
                    }
                }

                // ── DHW simulation (HVAC-only mode) ────────────────────
                for (dhw_sys, dhw_input) in dhw_systems.iter_mut().zip(&model.dhw_systems) {
                    let total_draw: f64 = dhw_input.loads.iter()
                        .map(|load| load.peak_flow_rate)
                        .sum();  // No schedule manager in HVAC-only mode
                    dhw_sys.simulate(total_draw, dhw_input.mains_temperature, dt);
                    let ep = dhw_sys.electric_power();
                    let fp = dhw_sys.fuel_power();
                    if ep > 0.0 {
                        snapshot.dhw_electric_power.insert(dhw_sys.name.clone(), ep);
                    }
                    if fp > 0.0 {
                        snapshot.dhw_fuel_power.insert(dhw_sys.name.clone(), fp);
                    }
                }

                // ── Exterior equipment (HVAC-only mode) ──────────────────
                // No schedule manager in HVAC-only mode — run at full power.
                // Route to typed ext_equipment_power (same as full simulation mode).
                for ext in &model.exterior_equipment {
                    let power = ext.power;
                    let is_hvac_ext_light = ext.subcategory.as_deref()
                        .map(|s| s.to_lowercase().contains("light"))
                        .unwrap_or(false);
                    if is_hvac_ext_light {
                        snapshot.ext_lighting_power
                            .entry(ext.name.clone())
                            .and_modify(|v| *v += power)
                            .or_insert(power);
                    } else if ext.fuel == "natural_gas" {
                        snapshot.component_fuel_power
                            .entry(ext.name.clone())
                            .and_modify(|v| *v += power)
                            .or_insert(power);
                    } else {
                        snapshot.ext_equipment_power
                            .entry(ext.name.clone())
                            .and_modify(|v| *v += power)
                            .or_insert(power);
                    }
                }

                for writer in &mut output_writers {
                    writer.add_snapshot(&snapshot);
                }
                if let Some(ref mut report) = summary_report {
                    report.add_snapshot(&snapshot);
                }

                results.push(result);
            }

            sim_time += dt;
        }
    }

    info!("Simulation complete: {} timesteps", results.len());

    // ── 8. Write output ─────────────────────────────────────────────────────

    // Full timeseries CSV (all component outputs)
    if !results.is_empty() {
        write_csv(&results, &output_path)
            .with_context(|| format!("Failed to write results to {}", output_path.display()))?;

        let mut cols = std::collections::HashSet::new();
        for r in &results {
            for (comp, vars) in &r.component_outputs {
                for var in vars.keys() {
                    cols.insert(format!("{}:{}", comp, var));
                }
            }
        }
        info!("Results written to: {} ({} rows x {} columns)", output_path.display(), results.len(), cols.len());
    } else {
        warn!("No results to write");
    }

    // Custom output files (user-defined variable selections, prefixed with input stem)
    for writer in &mut output_writers {
        writer.finalize_and_write_prefixed(output_dir, &input_stem)
            .with_context(|| format!("Failed to write custom output"))?;
    }
    if !output_writers.is_empty() {
        info!("Custom output files written: {}", output_writers.len());
    }

    // Summary report
    if let Some(ref report) = summary_report {
        let summary_path = output_dir.join(format!("{}_summary.txt", input_stem));
        report.write(&summary_path)
            .with_context(|| format!("Failed to write summary report"))?;
        info!("Summary report written to: {}", summary_path.display());
    }

    info!("OpenBSE finished");
    Ok(())
}

// ─── Multi-Loop Control Dispatcher ──────────────────────────────────────────
//
// Runs all air loops for one timestep, dispatching to the appropriate control
// strategy for each loop type. Returns:
//   - A combined TimestepResult with all component outputs
//   - A per-zone map of (supply_temp, mass_flow) aggregated across all loops

fn simulate_all_loops(
    graph: &mut SimulationGraph,
    ctx: &SimulationContext,
    loop_infos: &[LoopInfo],
    zone_temps: &HashMap<String, f64>,
    zone_heat_sp: &HashMap<String, f64>,
    zone_cool_sp: &HashMap<String, f64>,
    zone_unocc_heat_sp: &HashMap<String, f64>,
    zone_unocc_cool_sp: &HashMap<String, f64>,
    zone_design_flows: &HashMap<String, f64>,
    zone_multipliers: &HashMap<String, f64>,
    t_outdoor: f64,
    schedule_mgr: Option<&ScheduleManager>,
    hour: u32,
    day_of_week: u32,
    nightcycle_timers: &mut HashMap<String, f64>,
    dt: f64,
    zone_cooling_loads: &HashMap<String, f64>,
    zone_heating_loads: &HashMap<String, f64>,
    initial_zone_temps: &HashMap<String, f64>,
) -> (TimestepResult, HashMap<String, (f64, f64)>) {

    let mut all_outputs: HashMap<String, HashMap<String, f64>> = HashMap::new();
    // zone_name -> Vec<(supply_temp, mass_flow)> — accumulate from multiple loops
    let mut zone_supply: HashMap<String, Vec<(f64, f64)>> = HashMap::new();

    for li in loop_infos {
        // ── HVAC Availability Schedule & Night-Cycle Check ─────────────────
        //
        // When the availability schedule = 0, the system is normally OFF.
        // However, the night-cycle controller (E+ AvailabilityManager:NightCycle)
        // will cycle the system ON if any zone temperature drops below the
        // unoccupied heating setpoint or rises above the unoccupied cooling
        // setpoint. This prevents zone temperatures from drifting too far during
        // unoccupied periods while saving significant energy vs. maintaining
        // occupied setpoints 24/7.
        //
        // E+ AvailabilityManager:NightCycle behavior:
        //   - Control type: CycleOnAny (any zone triggers night-cycle)
        //   - Thermostat tolerance: 1.0°C (zone must be 1°C beyond setpoint)
        //   - Cycling run time: 1800s (30 min ON, then recheck)
        //
        // The cycling_run_time is critical: once night-cycle triggers ON, the
        // system stays ON for the full 1800s before rechecking conditions.
        // Without this, sub-hourly timesteps cause destructive ON/OFF
        // oscillation where the system repeatedly heats thermal mass then
        // lets it drain, wasting enormous energy.
        let mut is_unoccupied = false;
        let mut nightcycle_duty = 1.0_f64; // 1.0 = full operation during occupied
        let cycling_run_time = 1800.0_f64; // E+ default: 1800 seconds (30 min)
        let nightcycle_tolerance = 1.0_f64; // degrees C

        if let Some(ref sched_name) = li.availability_schedule {
            if let Some(mgr) = schedule_mgr {
                let avail = mgr.fraction(sched_name, hour, day_of_week);
                if avail < 0.5 {
                    // System scheduled OFF — check night-cycle timer first.
                    //
                    // If timer > 0, the system was already triggered and must
                    // keep running until the cycling_run_time expires.
                    let timer = nightcycle_timers.get(&li.name).copied().unwrap_or(0.0);

                    if timer > 0.0 {
                        // Still within cycling run time — keep system ON.
                        // Decrement timer by this timestep duration.
                        nightcycle_timers.insert(li.name.clone(), (timer - dt).max(0.0));
                        is_unoccupied = true;
                        // Night-cycle duty: system only runs for cycling_run_time
                        // out of each timestep (e.g. 30 min out of 60 min = 0.5).
                        nightcycle_duty = (cycling_run_time / dt).min(1.0);
                    } else {
                        // Timer expired or was never set — check if night-cycle
                        // should trigger based on zone temperatures.
                        let needs_nightcycle = li.served_zones.iter().any(|z| {
                            let zt = zone_temps.get(z).copied().unwrap_or(21.0);
                            let unocc_heat = zone_unocc_heat_sp.get(z).copied().unwrap_or(15.56);
                            let unocc_cool = zone_unocc_cool_sp.get(z).copied().unwrap_or(29.44);
                            zt < (unocc_heat - nightcycle_tolerance)
                                || zt > (unocc_cool + nightcycle_tolerance)
                        });

                        if needs_nightcycle {
                            // Start new night-cycle run.
                            // Timer = cycling_run_time minus this timestep (we're running now).
                            nightcycle_timers.insert(
                                li.name.clone(),
                                (cycling_run_time - dt).max(0.0),
                            );
                            is_unoccupied = true;
                            // Night-cycle duty: system only runs for cycling_run_time
                            // out of each timestep (e.g. 30 min out of 60 min = 0.5).
                            nightcycle_duty = (cycling_run_time / dt).min(1.0);
                        } else {
                            // System is OFF and no night-cycle needed — skip entirely.
                            nightcycle_timers.insert(li.name.clone(), 0.0);
                            for comp_name in &li.component_names {
                                let mut comp_outputs = HashMap::new();
                                comp_outputs.insert("outlet_temp".to_string(), t_outdoor);
                                comp_outputs.insert("outlet_w".to_string(), ctx.outdoor_air.w);
                                comp_outputs.insert("mass_flow".to_string(), 0.0);
                                comp_outputs.insert("outlet_enthalpy".to_string(), ctx.outdoor_air.h);
                                comp_outputs.insert("electric_power".to_string(), 0.0);
                                comp_outputs.insert("fuel_power".to_string(), 0.0);
                                comp_outputs.insert("thermal_output".to_string(), 0.0);
                                all_outputs.insert(comp_name.clone(), comp_outputs);
                            }
                            continue; // Skip this loop entirely
                        }
                    }
                } else {
                    // System is occupied/ON — clear any night-cycle timer
                    nightcycle_timers.insert(li.name.clone(), 0.0);
                }
            }
        }

        // ── Minimum OA schedule (E+ MinOA_MotorizedDamper_Sched) ──────────
        // E+ sets minimum outdoor air to 0 during unoccupied hours.
        // During night-cycle, the system recirculates return air only —
        // no cold outdoor air is mixed in, dramatically reducing reheat.
        let effective_min_oa = if is_unoccupied { 0.0 } else { li.min_oa_fraction };

        // Select active setpoints based on occupied/unoccupied state
        let active_heat_sp = if is_unoccupied { zone_unocc_heat_sp } else { zone_heat_sp };
        let active_cool_sp = if is_unoccupied { zone_unocc_cool_sp } else { zone_cool_sp };

        // ── Predictor Mode ─────────────────────────────────────────────
        //
        // Determine HVAC mode from the FROZEN ideal loads (E+ predictor
        // equivalent), NOT from the iterating zone temperature.  This
        // prevents mode flip-flopping at the setpoint boundary during the
        // HVAC↔envelope iteration loop.
        //
        // The frozen ideal loads represent: "what Q is needed to maintain
        // setpoint given current envelope conditions?"  If heating_load > 0,
        // the zone needs heating regardless of where the iterating zone
        // temp currently sits.
        //
        // For each served zone, compute a predictor mode and store it.
        // PTAC/FCU (single-zone) use the single zone's mode.
        // PSZ-AC uses the control zone's mode.
        let predictor_modes: HashMap<String, HvacMode> = li.served_zones.iter()
            .map(|z| {
                let hload = zone_heating_loads.get(z).copied().unwrap_or(0.0);
                let cload = zone_cooling_loads.get(z).copied().unwrap_or(0.0);
                let zt = zone_temps.get(z).copied().unwrap_or(21.0);
                let hsp = active_heat_sp.get(z).copied().unwrap_or(21.1);
                let csp = active_cool_sp.get(z).copied().unwrap_or(23.9);
                // Primary: use ideal loads (predictor)
                // Fallback: if both loads are zero (e.g., first timestep),
                // use zone temp vs setpoints
                let mode = if hload > 10.0 && hload > cload {
                    HvacMode::Heating
                } else if cload > 10.0 && cload > hload {
                    HvacMode::Cooling
                } else {
                    // Fallback to zone-temp method for initial timesteps
                    // or when loads are truly zero (deadband)
                    hvac_mode(zt, hsp, csp)
                };
                (z.clone(), mode)
            })
            .collect();

        let signals = match li.system_type {
            // ──────────────────────────────────────────────────────────────
            // PSZ-AC: single-zone thermostat, mixed return + outdoor air.
            // The control zone is the first served zone.
            // ──────────────────────────────────────────────────────────────
            AirLoopSystemType::PszAc => {
                build_psz_signals(li, zone_temps, active_heat_sp, active_cool_sp,
                    zone_design_flows, t_outdoor, zone_cooling_loads, zone_heating_loads,
                    effective_min_oa, &predictor_modes)
            }

            // ──────────────────────────────────────────────────────────────
            // DOAS: 100% outdoor air, fixed supply setpoints, always runs.
            // Pre-conditions ventilation air; no zone-temperature feedback.
            // ──────────────────────────────────────────────────────────────
            AirLoopSystemType::Doas => {
                build_doas_signals(li, zone_design_flows, &zone_multipliers, active_heat_sp, active_cool_sp, t_outdoor)
            }

            // ──────────────────────────────────────────────────────────────
            // FCU / PTAC: recirculating unit, per-zone thermostat.
            // Each FCU/PTAC loop serves exactly one zone.
            // ──────────────────────────────────────────────────────────────
            AirLoopSystemType::Fcu | AirLoopSystemType::Ptac => {
                build_fcu_signals(li, zone_temps, active_heat_sp, active_cool_sp,
                    zone_design_flows, t_outdoor, zone_heating_loads, zone_cooling_loads,
                    &predictor_modes)
            }

            // ──────────────────────────────────────────────────────────────
            // VAV: central cold-deck AHU, per-zone airflow modulation.
            // All zones get cold supply air; zone-level reheat is handled
            // by separate FCU-type loops defined in the YAML.
            // ──────────────────────────────────────────────────────────────
            AirLoopSystemType::Vav => {
                build_vav_signals(li, zone_temps, active_heat_sp, active_cool_sp,
                    zone_design_flows, &zone_multipliers, t_outdoor, effective_min_oa)
            }
        };

        // Run this loop's components in order (at full capacity, PLR=1.0)
        let (mut loop_result, supply_air) = simulate_loop_components(
            graph, ctx, &li.component_names, &signals
        );

        // ── Pre-compute continuous fan heat for PLR correction ──
        //
        // In continuous fan mode the fan runs at full speed regardless of PLR.
        // During the off-cycle (1-PLR fraction) the fan delivers
        //   Q_fan = m_dot * cp * dT_fan
        // of heat to the zone.  This must be subtracted from the heating
        // PLR numerator (fan already covers part of the load) and added to
        // the cooling PLR numerator (fan heat is an extra cooling burden).
        // Without this correction the system persistently over-delivers in
        // heating, pushing the zone above setpoint into deadband, where it
        // then cools and re-enters heating — the classic oscillation.
        let continuous_fan_heat_rise_pre = if li.fan_operating_mode
            == openbse_io::input::FanOperatingMode::Continuous
        {
            let fan_power: f64 = li.fan_names.iter()
                .filter_map(|fn_name| {
                    loop_result.get(fn_name)
                        .and_then(|o| o.get("electric_power"))
                        .copied()
                })
                .sum();
            let mass_flow = supply_air.as_ref().map(|s| s.mass_flow).unwrap_or(0.0);
            let cp_air_fan = 1006.0_f64;
            if mass_flow > 0.001 {
                fan_power / (mass_flow * cp_air_fan)
            } else {
                0.0
            }
        } else {
            0.0
        };

        // ── Mode-Based PLR for PSZ-AC / PTAC ON/OFF Cycling ──
        //
        // Components were simulated at full design flow. Now compute PLR
        // from the zone load and the system's actual net cooling/heating
        // capacity at current conditions.
        //
        // PLR = zone_load / system_net_capacity
        //
        // The system net capacity is computed from the supply air state:
        //   Q_net = m_dot × cp × (T_return - T_supply)  [cooling]
        //   Q_net = m_dot × cp × (T_supply - T_return)  [heating]
        //
        // This includes fan heat effects (draw-through fan warms the air,
        // reducing net cooling capacity).
        //
        // For non-PSZ-AC systems, PLR = 1.0 (they handle modulation internally).
        let loop_plr = if li.system_type == AirLoopSystemType::PszAc
            || li.system_type == AirLoopSystemType::Ptac {
            let control_zone = li.served_zones.first().map(|s| s.as_str()).unwrap_or("");
            let zone_cool_load = zone_cooling_loads.get(control_zone).copied().unwrap_or(0.0);
            let zone_heat_load = zone_heating_loads.get(control_zone).copied().unwrap_or(0.0);
            let control_temp = zone_temps.get(control_zone).copied().unwrap_or(21.0);
            let heat_sp = active_heat_sp.get(control_zone).copied().unwrap_or(21.1);
            let cool_sp = active_cool_sp.get(control_zone).copied().unwrap_or(23.9);
            // Use predictor mode (from frozen ideal loads) — stable across
            // HVAC iterations, preventing mode flip-flop at setpoint boundary.
            let mode = predictor_modes.get(control_zone).copied()
                .unwrap_or_else(|| hvac_mode(control_temp, heat_sp, cool_sp));

            let cp_air = 1006.0_f64; // J/(kg·K)

            if let Some(ref supply) = supply_air {
                let supply_temp = supply.state.t_db;
                let supply_flow = supply.mass_flow;

                // Mode-based PLR with continuous fan heat correction.
                //
                // The mode (heating/cooling/deadband) is determined by zone
                // temp vs setpoints.  Within each mode, PLR uses the frozen
                // ideal load adjusted for fan heat.  When ideal loads are
                // stale (transients), a proportional zone-error fallback
                // prevents full-capacity overshoot.
                //
                // Continuous fan correction:
                //   Heating: PLR = (Q_load - Q_fan) / (Q_cap - Q_fan)
                //   Cooling: PLR = (Q_load + Q_fan) / (Q_cap + Q_fan)
                // The fan delivers dT_fan of heating regardless of PLR; the
                // coils need only make up the difference.
                let q_fan = supply_flow * cp_air * continuous_fan_heat_rise_pre;

                match mode {
                    HvacMode::Heating => {
                        // E+ PTAC/PSZ-AC: PLR = Q_zone / Q_capacity.
                        // Fan cycles with coils (Fan:OnOff).
                        // Fan heat correction only applies in continuous fan mode.
                        let q_capacity = supply_flow * cp_air * (supply_temp - heat_sp);
                        if zone_heat_load > 10.0 && q_capacity > 100.0 {
                            let adj_load = (zone_heat_load - q_fan).max(0.0);
                            let adj_cap = (q_capacity - q_fan).max(1.0);
                            (adj_load / adj_cap).clamp(effective_min_oa, 1.0)
                        } else {
                            // Fallback: proportional zone error for transients
                            let error = (heat_sp - control_temp).max(0.0);
                            let max_dt = (supply_temp - heat_sp).max(1.0);
                            (error / max_dt).clamp(effective_min_oa, 1.0)
                        }
                    }
                    HvacMode::Cooling => {
                        let q_capacity = supply_flow * cp_air * (cool_sp - supply_temp);
                        if zone_cool_load > 10.0 && q_capacity > 100.0 {
                            // Add fan heat: fan adds extra cooling burden
                            let adj_load = zone_cool_load + q_fan;
                            let adj_cap = q_capacity + q_fan;
                            (adj_load / adj_cap).clamp(effective_min_oa, 1.0)
                        } else {
                            let error = (control_temp - cool_sp).max(0.0);
                            let max_dt = (cool_sp - supply_temp).max(1.0);
                            (error / max_dt).clamp(effective_min_oa, 1.0)
                        }
                    }
                    HvacMode::Deadband => {
                        // No active heating or cooling; fan only (if continuous).
                        effective_min_oa
                    }
                }
            } else {
                effective_min_oa
            }
        } else {
            // Non-PSZ-AC systems: no PLR cycling (they modulate internally)
            signals.coil_setpoints.get("__plr__")
                .copied()
                .unwrap_or(1.0)
        } * nightcycle_duty;

        if loop_plr < 1.0 {
            let is_continuous_fan = li.fan_operating_mode
                == openbse_io::input::FanOperatingMode::Continuous;

            for (comp_name, outputs) in &mut loop_result {
                let is_fan = li.fan_names.contains(comp_name);

                if is_continuous_fan && is_fan {
                    // Continuous fan mode: fan runs at full speed always.
                    // Fan power and thermal output are NOT scaled by PLR.
                    // Mass flow is NOT scaled — fan pushes air continuously.
                    // (No changes needed — outputs stay at full rated values.)
                } else {
                    // Cycling fan mode: all outputs scale with PLR.
                    // Also applies to coil outputs in continuous fan mode
                    // (coils cycle ON/OFF, average output = rated × PLR).
                    if let Some(ep) = outputs.get_mut("electric_power") {
                        *ep *= loop_plr;
                    }
                    if let Some(fp) = outputs.get_mut("fuel_power") {
                        *fp *= loop_plr;
                    }
                    if let Some(to) = outputs.get_mut("thermal_output") {
                        *to *= loop_plr;
                    }
                    if let Some(mf) = outputs.get_mut("mass_flow") {
                        *mf *= loop_plr;
                    }
                }
            }
        }

        // Reuse the pre-computed fan heat rise (computed before PLR for the
        // fan heat correction).  In continuous fan mode the fan power is NOT
        // scaled by PLR, so the value is the same before and after scaling.
        let continuous_fan_heat_rise = continuous_fan_heat_rise_pre;

        // Store PLR for reporting
        all_outputs.entry("__loop_plr__".to_string())
            .or_default()
            .insert(li.name.clone(), loop_plr);

        // Collect outputs
        for (k, v) in loop_result {
            all_outputs.insert(k, v);
        }

        // Distribute supply air to served zones.
        //
        // For zones with terminal boxes (VAV/PFP), the AHU supply air passes
        // through the terminal component first — the terminal modulates flow
        // and applies reheat. The terminal's outlet becomes the zone supply.
        if let Some(supply) = supply_air {
            let supply_temp = supply.state.t_db;

            let (effective_flow, effective_supply_temp) = if li.fan_operating_mode
                == openbse_io::input::FanOperatingMode::Continuous
            {
                // Continuous fan mode: fan runs at full speed always.
                // Full mass flow delivered at a weighted-average supply temp:
                //   ON-cycle  (PLR fraction):   T = supply_temp (coils active + fan heat)
                //   OFF-cycle (1-PLR fraction): T = T_zone + ΔT_fan (recirculated + fan heat)
                //
                // Average supply temp = PLR × T_supply + (1-PLR) × (T_zone + ΔT_fan)
                //
                // Since OA=0 for PTAC, T_mixed = T_zone (return air = zone air).
                let control_zone = li.served_zones.first()
                    .map(|s| s.as_str()).unwrap_or("");
                let t_zone = zone_temps.get(control_zone).copied().unwrap_or(21.0);
                let t_off = t_zone + continuous_fan_heat_rise;
                let t_avg = loop_plr * supply_temp + (1.0 - loop_plr) * t_off;
                (supply.mass_flow, t_avg)
            } else {
                // Cycling fan mode: PLR-scaled flow at full supply temp.
                // Fan cycles with coils: air only flows for PLR fraction of timestep.
                (supply.mass_flow * loop_plr, supply_temp)
            };

            for zone_name in &li.served_zones {
                // Check if this zone has a terminal box
                if let Some(term_name) = li.terminal_boxes.get(zone_name) {
                    // Simulate the terminal box with AHU supply as inlet.
                    // Set control signal: positive = heating demand, negative = cooling demand.
                    //
                    // Use FROZEN initial zone temps for the signal to prevent
                    // iteration oscillation.  AHU-level controls (SAT reset,
                    // economizer) use converging zone_temps, but the terminal
                    // signal must be constant across HVAC iterations.
                    //
                    // Load-based signal with frozen initial zone temps.
                    //
                    // Uses steady-state ideal load / reheat capacity for signal
                    // magnitude.  Initial zone temps (frozen across HVAC iterations)
                    // determine heating/cooling mode.
                    let zone_temp_init = initial_zone_temps.get(zone_name).copied().unwrap_or(21.0);
                    let heat_sp = active_heat_sp.get(zone_name).copied().unwrap_or(21.1);
                    let cool_sp = active_cool_sp.get(zone_name).copied().unwrap_or(23.9);
                    let zone_heat_load = zone_heating_loads.get(zone_name).copied().unwrap_or(0.0);

                    // Get reheat capacity for load-based signal
                    let reheat_cap = if let Some(node_idx) = graph.node_by_name(term_name) {
                        if let GraphComponent::Air(component) = graph.component_mut(node_idx) {
                            component.nominal_capacity().unwrap_or(10000.0).max(100.0)
                        } else {
                            10000.0
                        }
                    } else {
                        10000.0
                    };

                    let control_signal = if zone_temp_init < heat_sp && zone_heat_load > 0.0 {
                        // Heating: signal proportional to ideal load / capacity.
                        (zone_heat_load / reheat_cap).clamp(0.0, 1.0)
                    } else if zone_temp_init > cool_sp {
                        // Cooling: negative signal proportional to error
                        -((zone_temp_init - cool_sp) / 5.0).clamp(0.0, 1.0)
                    } else {
                        0.0  // Deadband
                    };

                    if let Some(node_idx) = graph.node_by_name(term_name) {
                        // Set control signal on the terminal box
                        if let GraphComponent::Air(component) = graph.component_mut(node_idx) {
                            component.set_setpoint(control_signal);
                        }
                        // Simulate with AHU supply as inlet
                        let term_inlet = AirPort::new(supply.state, effective_flow / li.served_zones.len().max(1) as f64);
                        if let GraphComponent::Air(component) = graph.component_mut(node_idx) {
                            let term_outlet = component.simulate_air(&term_inlet, ctx);

                            // Record terminal outputs
                            let mut term_outputs = HashMap::new();
                            term_outputs.insert("outlet_temp".to_string(), term_outlet.state.t_db);
                            term_outputs.insert("outlet_w".to_string(), term_outlet.state.w);
                            term_outputs.insert("mass_flow".to_string(), term_outlet.mass_flow);
                            term_outputs.insert("electric_power".to_string(), component.power_consumption());
                            term_outputs.insert("thermal_output".to_string(), component.thermal_output());
                            all_outputs.insert(term_name.clone(), term_outputs);

                            // Terminal outlet → zone supply
                            // Note: the terminal was already simulated with PLR-reduced
                            // inlet flow (effective_flow at line 1624), so its outlet
                            // flow is already time-averaged. Do NOT apply loop_plr again.
                            let term_supply_temp = term_outlet.state.t_db;
                            let term_flow = term_outlet.mass_flow;
                            zone_supply.entry(zone_name.clone())
                                .or_default()
                                .push((term_supply_temp, term_flow));
                        }
                    }
                } else {
                    // No terminal box — distribute AHU supply directly
                    let zone_flow = match li.system_type {
                        AirLoopSystemType::PszAc => {
                            let n = li.served_zones.len().max(1) as f64;
                            effective_flow / n
                        }
                        AirLoopSystemType::Doas => {
                            let n = li.served_zones.len().max(1) as f64;
                            effective_flow / n
                        }
                        AirLoopSystemType::Fcu | AirLoopSystemType::Ptac => {
                            effective_flow
                        }
                        AirLoopSystemType::Vav => {
                            signals.zone_air_flows.get(zone_name)
                                .copied()
                                .unwrap_or(effective_flow / li.served_zones.len().max(1) as f64)
                                * loop_plr
                        }
                    };

                    zone_supply.entry(zone_name.clone())
                        .or_default()
                        .push((effective_supply_temp, zone_flow));
                }
            }
        }
    }

    // Mix supply air from multiple loops per zone (DOAS + FCU additive)
    // For a zone receiving both DOAS ventilation and FCU recirculation:
    //   mixed_temp = Σ(T_i * m_i) / Σ(m_i)  (enthalpy-weighted mix)
    //   total_flow = Σ(m_i)
    let mut zone_supply_conditions: HashMap<String, (f64, f64)> = HashMap::new();
    for (zone_name, contributions) in zone_supply {
        let total_flow: f64 = contributions.iter().map(|(_, m)| m).sum();
        if total_flow > 0.0 {
            let mixed_temp = contributions.iter()
                .map(|(t, m)| t * m)
                .sum::<f64>() / total_flow;
            zone_supply_conditions.insert(zone_name, (mixed_temp, total_flow));
        }
    }

    let result = TimestepResult {
        month: ctx.timestep.month,
        day: ctx.timestep.day,
        hour: ctx.timestep.hour,
        sub_hour: ctx.timestep.sub_hour,
        component_outputs: all_outputs,
    };

    (result, zone_supply_conditions)
}

// ─── Per-System-Type Signal Builders ─────────────────────────────────────────

/// PSZ-AC: single thermostat in control zone, return-air mixing.
///
/// ASHRAE Guideline 36 / standard RTU control:
///   - Economizer: differential dry-bulb (100% OA when OA < return in cooling)
///   - Heating: proportional DAT from heat_sp toward max_dat (35-40°C)
///     based on zone heating error, matching E+ SingleZoneReheat control
///   - Cooling: proportional DAT (approximates DX compressor staging)
///   - Fan: constant volume when enabled, cycles in deadband
fn build_psz_signals(
    li: &LoopInfo,
    zone_temps: &HashMap<String, f64>,
    zone_heat_sp: &HashMap<String, f64>,
    zone_cool_sp: &HashMap<String, f64>,
    zone_design_flows: &HashMap<String, f64>,
    t_outdoor: f64,
    zone_cooling_loads: &HashMap<String, f64>,
    zone_heating_loads: &HashMap<String, f64>,
    effective_min_oa: f64,
    predictor_modes: &HashMap<String, HvacMode>,
) -> ControlSignals {
    let mut signals = ControlSignals::default();

    // Control zone = first served zone
    let control_zone = li.served_zones.first().map(|s| s.as_str()).unwrap_or("");
    let control_temp = zone_temps.get(control_zone).copied().unwrap_or(21.0);
    let heat_sp = zone_heat_sp.get(control_zone).copied().unwrap_or(21.1);
    let cool_sp = zone_cool_sp.get(control_zone).copied().unwrap_or(23.9);
    let zone_cool_load = zone_cooling_loads.get(control_zone).copied().unwrap_or(0.0);
    let zone_heat_load = zone_heating_loads.get(control_zone).copied().unwrap_or(0.0);

    // Use predictor mode (from frozen ideal loads) to prevent mode
    // flip-flopping during HVAC↔envelope iteration loop.
    let mode = predictor_modes.get(control_zone).copied()
        .unwrap_or_else(|| {
            // Fallback: temperature-based with load-informed deadband tiebreaker
            if control_temp > cool_sp {
                HvacMode::Cooling
            } else if control_temp < heat_sp {
                HvacMode::Heating
            } else if zone_cool_load > zone_heat_load && zone_cool_load > 100.0 {
                HvacMode::Cooling
            } else if zone_heat_load > zone_cool_load && zone_heat_load > 100.0 {
                HvacMode::Heating
            } else {
                HvacMode::Deadband
            }
        });

    // Total design flow (single instance — zone multiplier applied in snapshot output)
    let mut total_flow = 0.0f64;
    for zone_name in &li.served_zones {
        total_flow += zone_design_flows.get(zone_name).copied().unwrap_or(0.5);
    }
    total_flow = total_flow.max(0.01);

    // ── Part-Load Ratio (PLR) for ON/OFF Fan Cycling ──
    //
    // PLR is computed AFTER component simulation in simulate_all_loops
    // using load-based PLR: PLR = zone_load / system_capacity.
    //
    // Components are simulated at full flow (PLR = 1.0), then outputs
    // are scaled by PLR to represent the time-averaged effect.
    //
    // Here we just set PLR = 1.0 as a placeholder; the actual load-based
    // PLR is computed in simulate_all_loops after we know the system
    // capacity from the component simulation.
    let max_heating_dat = 40.0_f64;

    let plr = 1.0_f64; // Placeholder — real PLR computed post-simulation

    // Components run at FULL design flow (fan ON at full speed when cycling)
    let flow = total_flow;

    // ── Heating DAT target (proportional, E+ SingleZoneReheat-style) ──
    // When PLR is low (small heating need), deliver warmer air at low flow.
    // When PLR is high (large heating need), deliver hot air at high flow.
    // The heating DAT ramps toward max as the heating error increases.
    let heating_error = (heat_sp - control_temp).max(0.0);
    let heating_dat = (heat_sp + (max_heating_dat - heat_sp) * (heating_error / 5.0).min(1.0))
        .clamp(heat_sp, max_heating_dat);

    // ── Cooling control ──
    // Two separate targets:
    // 1. Economizer target: proportional DAT for OA mixing (12°C to cool_sp)
    // 2. Coil setpoint: very low so coil runs at full capacity when ON.
    //    For ON/OFF cycling, PLR controls runtime, not the coil setpoint.
    let cooling_error = (control_temp - cool_sp).max(0.0);
    let econ_target = (cool_sp - cooling_error.min(10.0)).clamp(12.0, cool_sp);
    // Coil setpoint: -10°C forces the DX coil to run at full physical capacity.
    // The coil's actual outlet temp is limited by its available capacity.
    let cooling_coil_sp = if mode == HvacMode::Cooling { -10.0 } else { 99.0 };

    // ── Economizer: modulating differential dry-bulb (ASHRAE 90.1 §6.5.1) ──
    // In cooling mode: modulate OA fraction to achieve the economizer target DAT.
    // If OA can fully satisfy the target, DX coil stays off (free cooling).
    // If OA is too cold, mix with return air to reach target.
    // If OA is too warm, use minimum OA and let DX coil handle it.
    let return_air_temp = control_temp;
    let oa_frac = match mode {
        HvacMode::Cooling if t_outdoor < return_air_temp => {
            // Economizer available: OA is cooler than return air.
            // Modulate to achieve economizer target.
            let delta = return_air_temp - t_outdoor;
            if delta > 0.1 {
                let needed = (return_air_temp - econ_target) / delta;
                needed.clamp(effective_min_oa, 1.0)
            } else {
                effective_min_oa
            }
        }
        _ => effective_min_oa,
    };
    let mixed_air_temp = return_air_temp * (1.0 - oa_frac) + t_outdoor * oa_frac;

    for name in &li.component_names {
        let lname = name.to_lowercase();
        match mode {
            HvacMode::Heating => {
                // Proportional heating DAT: ramps from setpoint toward max (40°C)
                // based on zone heating error. At small errors, furnace delivers
                // warm but not hot air; at large errors, full-fire to recover.
                if lname.contains("heat") || lname.contains("furnace")
                    || lname.starts_with("hc ") || lname.starts_with("hc_") {
                    signals.coil_setpoints.insert(name.clone(), heating_dat);
                } else if lname.contains("cool") || lname.contains("dx")
                    || lname.starts_with("cc ") || lname.starts_with("cc_") {
                    signals.coil_setpoints.insert(name.clone(), 99.0);
                }
            }
            HvacMode::Cooling => {
                // DX coil runs at full capacity when ON (PLR controls runtime).
                // The coil setpoint is set very low so capacity is the limiter.
                if lname.contains("cool") || lname.contains("dx")
                    || lname.starts_with("cc ") || lname.starts_with("cc_") {
                    signals.coil_setpoints.insert(name.clone(), cooling_coil_sp);
                } else if lname.contains("heat") || lname.contains("furnace")
                    || lname.starts_with("hc ") || lname.starts_with("hc_") {
                    signals.coil_setpoints.insert(name.clone(), -99.0);
                }
            }
            HvacMode::Deadband => {
                if lname.contains("heat") || lname.contains("furnace")
                    || lname.starts_with("hc ") || lname.starts_with("hc_") {
                    signals.coil_setpoints.insert(name.clone(), -99.0);
                } else if lname.contains("cool") || lname.contains("dx")
                    || lname.starts_with("cc ") || lname.starts_with("cc_") {
                    signals.coil_setpoints.insert(name.clone(), 99.0);
                }
            }
        }
        signals.air_mass_flows.insert(name.clone(), flow);
    }

    // Inject mixed air temperature, OA fraction, and PLR
    signals.coil_setpoints.insert(
        "__pszac_mixed_air_temp__".to_string(),
        mixed_air_temp,
    );
    signals.coil_setpoints.insert(
        "__oa_fraction__".to_string(),
        oa_frac,
    );
    signals.coil_setpoints.insert(
        "__plr__".to_string(),
        plr,
    );

    signals
}

/// DOAS: 100% outdoor air, fixed supply setpoints, always on.
///
/// Supply temperature setpoints:
///   Heating:  max zone heating setpoint + 2°C (ensures OA is delivered above zone setpoint)
///   Cooling:  min zone cooling setpoint - 2°C (dehumidified neutral air)
///
/// This prevents the DOAS from delivering supply air that is colder than the zone
/// heating setpoint in winter (which would add heating load to the zones).
fn build_doas_signals(
    li: &LoopInfo,
    zone_design_flows: &HashMap<String, f64>,
    zone_multipliers: &HashMap<String, f64>,
    zone_heat_sp: &HashMap<String, f64>,
    zone_cool_sp: &HashMap<String, f64>,
    t_outdoor: f64,
) -> ControlSignals {
    let mut signals = ControlSignals::default();

    // Total ventilation airflow = 30% of zone design flows (with zone multipliers)
    let vent_flow_total: f64 = li.served_zones.iter()
        .map(|z| {
            let m = zone_multipliers.get(z).copied().unwrap_or(1.0);
            zone_design_flows.get(z).copied().unwrap_or(0.1) * m
        })
        .sum::<f64>() * 0.30;
    let vent_flow = vent_flow_total.max(0.05);

    // Supply setpoints: heat to 2°C above zone heating setpoint,
    // cool to 2°C below zone cooling setpoint.
    // Clamp: never heat if OA is already above heating setpoint; never cool if below.
    let max_heat_sp = li.served_zones.iter()
        .map(|z| zone_heat_sp.get(z).copied().unwrap_or(21.0))
        .fold(f64::NEG_INFINITY, f64::max);
    let min_cool_sp = li.served_zones.iter()
        .map(|z| zone_cool_sp.get(z).copied().unwrap_or(24.0))
        .fold(f64::INFINITY, f64::min);

    // DOAS heating setpoint: 2°C above zone heating setpoint (deliver warm neutral air)
    let t_supply_heat = max_heat_sp + 2.0;
    // DOAS cooling setpoint: 2°C below zone cooling setpoint (deliver cool dehumidified air)
    let t_supply_cool = (min_cool_sp - 2.0).max(14.0);  // 14°C minimum for dehumidification

    for name in &li.component_names {
        let lname = name.to_lowercase();
        if lname.contains("heat") || lname.contains("preheat")
            || lname.starts_with("hc ") || lname.starts_with("hc_") {
            // Fire only if OA is below heating target
            if t_outdoor < t_supply_heat {
                signals.coil_setpoints.insert(name.clone(), t_supply_heat);
            } else {
                signals.coil_setpoints.insert(name.clone(), -99.0);  // off
            }
        } else if lname.contains("cool") || lname.contains("dx")
            || lname.starts_with("cc ") || lname.starts_with("cc_") {
            // Fire only if OA is above cooling target (summer dehumidification)
            if t_outdoor > t_supply_cool {
                signals.coil_setpoints.insert(name.clone(), t_supply_cool);
            } else {
                signals.coil_setpoints.insert(name.clone(), 99.0);  // off
            }
        }
        signals.air_mass_flows.insert(name.clone(), vent_flow);
    }

    // DOAS inlet is always 100% outdoor air
    signals.coil_setpoints.insert(
        "__oa_fraction__".to_string(),
        1.0,
    );

    signals
}

/// FCU: recirculating fan coil, per-zone thermostat (one zone per FCU loop).
fn build_fcu_signals(
    li: &LoopInfo,
    zone_temps: &HashMap<String, f64>,
    zone_heat_sp: &HashMap<String, f64>,
    zone_cool_sp: &HashMap<String, f64>,
    zone_design_flows: &HashMap<String, f64>,
    t_outdoor: f64,
    zone_heating_loads: &HashMap<String, f64>,
    zone_cooling_loads: &HashMap<String, f64>,
    predictor_modes: &HashMap<String, HvacMode>,
) -> ControlSignals {
    let mut signals = ControlSignals::default();

    // FCU serves one zone (its name is the zone)
    let zone_name = li.served_zones.first().map(|s| s.as_str()).unwrap_or("");
    let zone_temp = zone_temps.get(zone_name).copied().unwrap_or(21.0);
    let heat_sp = zone_heat_sp.get(zone_name).copied().unwrap_or(21.1);
    let cool_sp = zone_cool_sp.get(zone_name).copied().unwrap_or(23.9);

    let design_flow = zone_design_flows.get(zone_name).copied().unwrap_or(0.3);

    // Use predictor mode (from frozen ideal loads) to prevent mode
    // flip-flopping during HVAC↔envelope iteration.
    let mode = predictor_modes.get(zone_name).copied()
        .unwrap_or_else(|| hvac_mode(zone_temp, heat_sp, cool_sp));

    // PTAC: Fan runs at design flow when heating or cooling (mode != Deadband).
    // In deadband with cycling fan the system is off.
    // In deadband with continuous fan, fan runs at design flow recirculating
    // zone air (fan heat only — coils disabled).  This matches E+ behaviour
    // where Supply Air Fan Operating Mode Schedule = 1 (continuous).
    // E+ PTAC heating uses water coil modulation (PLR=1, valve throttles).
    // E+ PTAC cooling uses DX ON/OFF cycling (PLR < 1).
    //
    // FCU: modulates fan speed proportionally.
    let is_ptac = li.system_type == AirLoopSystemType::Ptac;
    let is_continuous_fan_mode = li.fan_operating_mode
        == openbse_io::input::FanOperatingMode::Continuous;
    let flow = if is_ptac {
        match mode {
            HvacMode::Deadband => {
                if is_continuous_fan_mode {
                    design_flow  // continuous fan: recirculate zone air
                } else {
                    0.0  // cycling fan: system off
                }
            }
            _ => design_flow,
        }
    } else {
        // FCU modulates fan speed: deadband = 20%, heating/cooling = proportional
        match mode {
            HvacMode::Deadband => {
                design_flow * 0.20
            }
            HvacMode::Heating => {
                let error = (heat_sp - zone_temp).clamp(0.0, 5.0);
                let frac = 0.30 + 0.70 * (error / 5.0);  // 30-100% of design
                design_flow * frac
            }
            HvacMode::Cooling => {
                let error = (zone_temp - cool_sp).clamp(0.0, 5.0);
                let frac = 0.30 + 0.70 * (error / 5.0);  // 30-100% of design
                design_flow * frac
            }
        }
    };

    // PTAC OA = 0 (matching E+): PTAC recirculates zone air only.
    // Zone ventilation is handled independently by zone outdoor_air spec
    // (equivalent to E+ separate ERV with 0% effectiveness).
    // FCU: also recirculates zone air only (OA fraction = 0).
    let oa_frac = if is_ptac { li.min_oa_fraction } else { 0.0 };
    let mixed_air_temp = (1.0 - oa_frac) * zone_temp + oa_frac * t_outdoor;

    // PTAC uses ON/OFF cycling with PLR modulation (like PSZ-AC):
    // coils target design supply temps at full capacity, then PLR
    // scales the output to match the zone load.
    //
    // FCU uses proportional modulation: coil setpoint varies with zone error.
    for name in &li.component_names {
        let lname = name.to_lowercase();
        if is_ptac {
            // PTAC control matching EnergyPlus:
            //
            // Heating (Coil:Heating:Water): E+ modulates water flow rate
            // to deliver exactly the zone load.  We approximate this by
            // computing the supply temp target from the predictor load:
            //   T_target = T_zone + Q_heat / (m_dot * cp)
            // The coil modulates internally to meet this target.  PLR = 1
            // for heating (no ON/OFF cycling for water coils).
            //
            // Cooling (DX coil): E+ cycles the compressor ON/OFF.  We
            // run the DX at full capacity and set PLR = Q_cool / Q_cap
            // after component simulation.
            match mode {
                HvacMode::Heating => {
                    // E+ PTAC (Fan:OnOff cycling): run heating coil at
                    // design supply temp during ON-period, off during
                    // OFF-period.  PLR sets the duty cycle.
                    if lname.contains("heat") || lname.contains("reheat")
                        || lname.starts_with("hc ") || lname.starts_with("hc_") {
                        signals.coil_setpoints.insert(name.clone(), li.heating_supply_temp);
                    } else if lname.contains("cool") || lname.contains("dx")
                        || lname.starts_with("cc ") || lname.starts_with("cc_") {
                        signals.coil_setpoints.insert(name.clone(), 99.0);
                    }
                }
                HvacMode::Cooling => {
                    // DX cooling: run at full capacity, PLR handles cycling.
                    if lname.contains("cool") || lname.contains("dx")
                        || lname.starts_with("cc ") || lname.starts_with("cc_") {
                        signals.coil_setpoints.insert(name.clone(), li.cooling_supply_temp);
                    } else if lname.contains("heat") || lname.contains("reheat")
                        || lname.starts_with("hc ") || lname.starts_with("hc_") {
                        signals.coil_setpoints.insert(name.clone(), -99.0);
                    }
                }
                HvacMode::Deadband => {
                    if lname.contains("heat") || lname.contains("reheat")
                        || lname.starts_with("hc ") || lname.starts_with("hc_") {
                        signals.coil_setpoints.insert(name.clone(), -99.0);
                    } else if lname.contains("cool") || lname.contains("dx")
                        || lname.starts_with("cc ") || lname.starts_with("cc_") {
                        signals.coil_setpoints.insert(name.clone(), 99.0);
                    }
                }
            }
        } else {
            // FCU: proportional modulation
            match mode {
                HvacMode::Heating => {
                    let error = heat_sp - zone_temp;
                    let target = (heat_sp + error.min(14.0)).clamp(heat_sp, 45.0);
                    if lname.contains("heat") || lname.contains("reheat")
                        || lname.starts_with("hc ") || lname.starts_with("hc_") {
                        signals.coil_setpoints.insert(name.clone(), target);
                    } else if lname.contains("cool") || lname.contains("dx")
                        || lname.starts_with("cc ") || lname.starts_with("cc_") {
                        signals.coil_setpoints.insert(name.clone(), 99.0);
                    }
                }
                HvacMode::Cooling => {
                    let error = zone_temp - cool_sp;
                    let target = (cool_sp - error.min(10.0)).clamp(12.0, cool_sp);
                    if lname.contains("cool") || lname.contains("dx")
                        || lname.starts_with("cc ") || lname.starts_with("cc_") {
                        signals.coil_setpoints.insert(name.clone(), target);
                    } else if lname.contains("heat") || lname.contains("reheat")
                        || lname.starts_with("hc ") || lname.starts_with("hc_") {
                        signals.coil_setpoints.insert(name.clone(), -99.0);
                    }
                }
                HvacMode::Deadband => {
                    if lname.contains("heat") || lname.contains("reheat")
                        || lname.starts_with("hc ") || lname.starts_with("hc_") {
                        signals.coil_setpoints.insert(name.clone(), -99.0);
                    } else if lname.contains("cool") || lname.contains("dx")
                        || lname.starts_with("cc ") || lname.starts_with("cc_") {
                        signals.coil_setpoints.insert(name.clone(), 99.0);
                    }
                }
            }
        }
        signals.air_mass_flows.insert(name.clone(), flow);
    }

    signals.coil_setpoints.insert(
        "__fcu_recirculation_temp__".to_string(),
        mixed_air_temp,
    );
    signals.coil_setpoints.insert(
        "__oa_fraction__".to_string(),
        oa_frac,
    );

    signals
}

/// VAV: central AHU + per-zone VAV boxes with reheat.
///
/// ASHRAE Guideline 36 §5.2 / §5.16 — Dual-Maximum VAV control:
///
///   **Zone-level (VAV box):**
///   - Cooling: airflow ramps from V_min up to V_cool_max (100% design) proportional to error
///   - Deadband: airflow at V_min (ventilation minimum)
///   - Heating: airflow ramps from V_min up to V_heat_max (50% design), AND reheat coil fires
///     This is "dual-maximum" — heating has its own max, not the single-maximum of old systems
///
///   **AHU-level:**
///   - SAT reset (G36 §5.16): reset supply temp from 13°C (max cooling) to 18°C (min cooling)
///     based on cooling demand across all zones. Saves energy in mild weather.
///   - Economizer: differential dry-bulb (100% OA when OA < return in cooling)
///   - Preheat: frost protection when mixed air < 4°C
fn build_vav_signals(
    li: &LoopInfo,
    zone_temps: &HashMap<String, f64>,
    zone_heat_sp: &HashMap<String, f64>,
    zone_cool_sp: &HashMap<String, f64>,
    zone_design_flows: &HashMap<String, f64>,
    zone_multipliers: &HashMap<String, f64>,
    t_outdoor: f64,
    effective_min_oa: f64,
) -> ControlSignals {
    let mut signals = ControlSignals::default();

    // ── ASHRAE G36 Dual-Maximum: per-zone airflow + reheat ──
    //
    // V_heat_max = 50% of V_cool_max (design flow). This is the key dual-maximum concept:
    // in heating, the box opens wider than minimum to deliver more reheat energy, but
    // doesn't go to full design flow (that would overcool from the cold deck).
    let v_heat_max_frac = 0.50;  // G36 typical: 50% of design

    let mut total_flow = 0.0f64;
    let mut max_cooling_demand = 0.0f64;  // for SAT reset

    for zone_name in &li.served_zones {
        let zone_temp = zone_temps.get(zone_name).copied().unwrap_or(21.0);
        let heat_sp = zone_heat_sp.get(zone_name).copied().unwrap_or(21.1);
        let cool_sp = zone_cool_sp.get(zone_name).copied().unwrap_or(23.9);
        let design_flow = zone_design_flows.get(zone_name).copied().unwrap_or(0.5);

        let mode = hvac_mode(zone_temp, heat_sp, cool_sp);

        let zone_flow = match mode {
            HvacMode::Cooling => {
                // Cooling: ramp V_min → V_cool_max (100% design)
                let error = (zone_temp - cool_sp).clamp(0.0, 5.0);
                let frac = li.min_vav_fraction + (1.0 - li.min_vav_fraction) * (error / 5.0);
                max_cooling_demand = max_cooling_demand.max(error / 5.0);
                design_flow * frac
            }
            HvacMode::Heating => {
                // Dual-maximum: ramp V_min → V_heat_max (50% design)
                let error = (heat_sp - zone_temp).clamp(0.0, 5.0);
                let frac = li.min_vav_fraction
                    + (v_heat_max_frac - li.min_vav_fraction) * (error / 5.0);
                design_flow * frac
            }
            HvacMode::Deadband => {
                // Minimum ventilation flow
                design_flow * li.min_vav_fraction
            }
        };

        // Store per-zone flow in signals (per-instance, not multiplied)
        signals.zone_air_flows.insert(zone_name.clone(), zone_flow);
        // Accumulate total fan flow WITH zone multiplier:
        // if a zone has multiplier=10, the fan handles 10× that zone's flow.
        let mult = zone_multipliers.get(zone_name).copied().unwrap_or(1.0);
        total_flow += zone_flow * mult;
    }
    total_flow = total_flow.max(0.05);

    // ── SAT Reset (E+ SetpointManager:Warmest, MaximumTemperature) ──
    //
    // E+ finds the HIGHEST supply air temp that satisfies all zones.
    // SAT stays at sat_max (15.6°C) unless a zone truly needs lower SAT
    // because it can't be cooled at max VAV flow. This avoids unnecessarily
    // cold supply air that forces VAV boxes to waste energy on reheat.
    //
    // We approximate this: SAT only drops when the most cooling-needy zone
    // requests near-maximum VAV flow (max_cooling_demand > threshold).
    // Below the threshold, the zone can be satisfied at higher SAT by
    // simply opening the VAV damper more.
    let sat_min = 12.8_f64;  // full cooling SAT (E+ SetpointManager:Warmest min)
    let sat_max = 15.6_f64;  // reset SAT (E+ SetpointManager:Warmest max)
    let sat_threshold = 0.80_f64; // SAT drops only when VAV nearing max flow
    let excess_demand = ((max_cooling_demand - sat_threshold) / (1.0 - sat_threshold)).clamp(0.0, 1.0);
    let sat_setpoint = sat_max - (sat_max - sat_min) * excess_demand;

    // ── Return air temperature (flow-weighted average of zone temps) ──
    let avg_zone_temp = if li.served_zones.is_empty() {
        21.0
    } else {
        li.served_zones.iter()
            .map(|z| zone_temps.get(z).copied().unwrap_or(21.0))
            .sum::<f64>() / li.served_zones.len() as f64
    };

    // ── Economizer: modulating differential dry-bulb ──
    // In cooling mode: modulate OA fraction to achieve SAT setpoint.
    // If OA can fully satisfy SAT, no mechanical cooling needed (free cooling).
    let any_cooling = max_cooling_demand > 0.0;
    let oa_frac = if any_cooling && t_outdoor < avg_zone_temp {
        let delta = avg_zone_temp - t_outdoor;
        if delta > 0.1 {
            // Modulate OA to reach SAT setpoint as mixed air target
            let needed = (avg_zone_temp - sat_setpoint) / delta;
            needed.clamp(effective_min_oa, 1.0)
        } else {
            effective_min_oa
        }
    } else {
        effective_min_oa
    };
    let mixed_air_temp = avg_zone_temp * (1.0 - oa_frac) + t_outdoor * oa_frac;

    // ── AHU coil control ──
    for name in &li.component_names {
        let lname = name.to_lowercase();
        if lname.contains("cool") || lname.contains("dx")
            || lname.starts_with("cc ") || lname.starts_with("cc_") {
            if any_cooling {
                // AHU cooling coil targets the SAT setpoint
                signals.coil_setpoints.insert(name.clone(), sat_setpoint);
            } else {
                // No cooling demand — coil off
                signals.coil_setpoints.insert(name.clone(), 99.0);
            }
        } else if lname.contains("preheat") || lname.contains("heat")
            || lname.starts_with("hc ") || lname.starts_with("hc_") {
            // AHU heating coil (E+ SetpointManager:Warmest, MaximumTemperature).
            //
            // The heating coil targets the SAT setpoint to temper mixed air:
            // - When mixed air < sat_setpoint: heat up to sat_setpoint
            // - When mixed air >= sat_setpoint: coil off (cooling coil handles)
            //
            // This is critical for VAV reheat reduction: without AHU heating,
            // cold mixed air (e.g. 5°C) goes directly to VAV boxes which must
            // reheat from 5°C to ~30°C. With AHU heating to sat_setpoint
            // (12.8-15.6°C), reheat delta drops dramatically.
            //
            // E+ uses LockoutWithHeating: when the AHU heating coil fires,
            // the economizer locks to minimum OA and the cooling coil is off.
            // Our SAT setpoint already prevents over-heating: if any zone
            // needs significant cooling, sat_setpoint drops toward 12.8°C,
            // and the economizer provides the cooling via cold OA.
            if mixed_air_temp < sat_setpoint {
                signals.coil_setpoints.insert(name.clone(), sat_setpoint);
            } else {
                signals.coil_setpoints.insert(name.clone(), -99.0);
            }
        }
        signals.air_mass_flows.insert(name.clone(), total_flow);
    }

    // Inject mixed air temp + OA fraction
    signals.coil_setpoints.insert(
        "__vav_mixed_air_temp__".to_string(),
        mixed_air_temp,
    );
    signals.coil_setpoints.insert(
        "__oa_fraction__".to_string(),
        oa_frac,
    );

    signals
}

// ─── Loop Component Runner ───────────────────────────────────────────────────
//
// Simulates a subset of graph components (one air loop's worth) in order,
// applying the provided control signals. Returns per-component outputs and
// the final air outlet state.

fn simulate_loop_components(
    graph: &mut SimulationGraph,
    ctx: &SimulationContext,
    component_names: &[String],
    signals: &ControlSignals,
) -> (HashMap<String, HashMap<String, f64>>, Option<AirPort>) {
    let mut outputs: HashMap<String, HashMap<String, f64>> = HashMap::new();

    // Check for inlet override signals (mixed air temp for PSZ, recirculation for FCU, VAV)
    let inlet_temp_override: Option<f64> = signals.coil_setpoints.get("__pszac_mixed_air_temp__")
        .or_else(|| signals.coil_setpoints.get("__fcu_recirculation_temp__"))
        .or_else(|| signals.coil_setpoints.get("__vav_mixed_air_temp__"))
        .copied();

    // OA fraction for humidity blending (defaults to 1.0 = 100% outdoor air if not set)
    let oa_fraction = signals.coil_setpoints.get("__oa_fraction__")
        .copied()
        .unwrap_or(1.0);

    // Build inlet air state with proper humidity blending
    let mut inlet_air = AirPort::new(ctx.outdoor_air, 1.0);
    if let Some(override_temp) = inlet_temp_override {
        // Blend humidity: w_mixed = OA_frac * w_outdoor + (1 - OA_frac) * w_indoor
        // For recirculated air (FCU, OA_frac=0): uses zone humidity (approximated by outdoor for now)
        // For mixed air (PSZ-AC, VAV): proper OA/RA blend
        // Blend humidity: OA fraction × outdoor humidity + (1 - OA fraction) × indoor humidity.
        // Indoor humidity approximated from the zone temperature using a typical RH of ~50%.
        let w_indoor = openbse_psychrometrics::MoistAirState::from_tdb_rh(
            inlet_temp_override.unwrap_or(ctx.outdoor_air.t_db), 0.50, ctx.outdoor_air.p_b,
        ).w;
        let w_mixed = oa_fraction * ctx.outdoor_air.w + (1.0 - oa_fraction) * w_indoor;
        let mixed_state = openbse_psychrometrics::MoistAirState::new(
            override_temp,
            w_mixed,
            ctx.outdoor_air.p_b,
        );
        inlet_air = AirPort::new(mixed_state, inlet_air.mass_flow);
    }

    let mut last_outlet: Option<AirPort> = None;

    for comp_name in component_names {
        // Get node index for this component
        let node_idx = match graph.node_by_name(comp_name) {
            Some(idx) => idx,
            None => continue,
        };

        match graph.component_mut(node_idx) {
            GraphComponent::Air(component) => {
                // Apply setpoint override (skip special sentinel keys)
                if let Some(&sp) = signals.coil_setpoints.get(comp_name.as_str()) {
                    component.set_setpoint(sp);
                }

                // Use previous component's outlet as inlet; first component uses loop inlet
                let mut this_inlet = last_outlet.unwrap_or(inlet_air);

                // Apply mass flow override if set
                if let Some(&flow) = signals.air_mass_flows.get(comp_name.as_str()) {
                    this_inlet.mass_flow = flow;
                }

                let outlet = component.simulate_air(&this_inlet, ctx);

                let mut comp_outputs = HashMap::new();
                comp_outputs.insert("outlet_temp".to_string(), outlet.state.t_db);
                comp_outputs.insert("outlet_w".to_string(), outlet.state.w);
                comp_outputs.insert("mass_flow".to_string(), outlet.mass_flow);
                comp_outputs.insert("outlet_enthalpy".to_string(), outlet.state.h);
                comp_outputs.insert("electric_power".to_string(), component.power_consumption());
                comp_outputs.insert("fuel_power".to_string(), component.fuel_consumption());
                comp_outputs.insert("thermal_output".to_string(), component.thermal_output());
                outputs.insert(comp_name.clone(), comp_outputs);

                last_outlet = Some(outlet);
            }
            GraphComponent::Plant(_) => {
                // Plant components are not part of air loops — skip
            }
        }
    }

    (outputs, last_outlet)
}

// ─── Legacy simulate_hvac (HVAC-only mode) ───────────────────────────────────
//
// Used when there's no envelope (pure HVAC simulation with user-defined controls).

fn simulate_hvac(
    graph: &mut SimulationGraph,
    ctx: &SimulationContext,
    signals: &ControlSignals,
) -> (TimestepResult, Option<AirPort>) {
    let order: Vec<_> = graph.simulation_order().to_vec();
    let mut air_states: HashMap<petgraph::graph::NodeIndex, AirPort> = HashMap::new();
    let mut water_states: HashMap<petgraph::graph::NodeIndex, WaterPort> = HashMap::new();
    let mut component_outputs: HashMap<String, HashMap<String, f64>> = HashMap::new();

    let default_air = AirPort::new(ctx.outdoor_air, 1.0);
    let default_water = WaterPort::default_water();
    let mut last_air_outlet: Option<AirPort> = None;

    for &node_idx in &order {
        let predecessors = graph.predecessors(node_idx);

        match graph.component_mut(node_idx) {
            GraphComponent::Air(component) => {
                let comp_name = component.name().to_string();

                if let Some(&sp) = signals.coil_setpoints.get(&comp_name) {
                    component.set_setpoint(sp);
                }

                let mut inlet = if let Some(&pred) = predecessors.first() {
                    air_states.get(&pred).copied().unwrap_or(default_air)
                } else {
                    default_air
                };

                if let Some(&flow) = signals.air_mass_flows.get(&comp_name) {
                    inlet.mass_flow = flow;
                }

                let outlet = component.simulate_air(&inlet, ctx);

                let mut outputs = HashMap::new();
                outputs.insert("outlet_temp".to_string(), outlet.state.t_db);
                outputs.insert("outlet_w".to_string(), outlet.state.w);
                outputs.insert("mass_flow".to_string(), outlet.mass_flow);
                outputs.insert("outlet_enthalpy".to_string(), outlet.state.h);
                outputs.insert("electric_power".to_string(), component.power_consumption());
                outputs.insert("fuel_power".to_string(), component.fuel_consumption());
                outputs.insert("thermal_output".to_string(), component.thermal_output());
                component_outputs.insert(comp_name, outputs);

                last_air_outlet = Some(outlet);
                air_states.insert(node_idx, outlet);
            }
            GraphComponent::Plant(component) => {
                let comp_name = component.name().to_string();
                let inlet = if let Some(&pred) = predecessors.first() {
                    water_states.get(&pred).copied().unwrap_or(default_water)
                } else {
                    default_water
                };
                let load = signals.plant_loads.get(&comp_name).copied().unwrap_or(0.0);
                let outlet = component.simulate_plant(&inlet, load, ctx);

                let mut outputs = HashMap::new();
                outputs.insert("outlet_temp".to_string(), outlet.state.temp);
                outputs.insert("mass_flow".to_string(), outlet.state.mass_flow);
                outputs.insert("electric_power".to_string(), component.power_consumption());
                outputs.insert("fuel_power".to_string(), component.fuel_consumption());
                component_outputs.insert(comp_name, outputs);
                water_states.insert(node_idx, outlet);
            }
        }
    }

    let result = TimestepResult {
        month: ctx.timestep.month,
        day: ctx.timestep.day,
        hour: ctx.timestep.hour,
        sub_hour: ctx.timestep.sub_hour,
        component_outputs,
    };
    (result, last_air_outlet)
}

// ─── HVAC Mode ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq)]
enum HvacMode { Heating, Cooling, Deadband }

fn hvac_mode(zone_temp: f64, heat_sp: f64, cool_sp: f64) -> HvacMode {
    if zone_temp < heat_sp {
        HvacMode::Heating
    } else if zone_temp > cool_sp {
        HvacMode::Cooling
    } else {
        HvacMode::Deadband
    }
}

// ─── Utility Functions ───────────────────────────────────────────────────────

fn resolve_path(input_file: &Path, relative_path: &str) -> PathBuf {
    if Path::new(relative_path).is_absolute() {
        PathBuf::from(relative_path)
    } else {
        input_file
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(relative_path)
    }
}

fn day_of_year(month: u32, day: u32, dims: &[u32; 12]) -> u32 {
    let mut doy = 0u32;
    for m in 0..(month - 1) as usize {
        doy += dims[m];
    }
    doy + day - 1
}

fn month_day_from_hour(hour_of_year: u32, dims: &[u32; 12]) -> (u32, u32) {
    let day_of_year = hour_of_year / 24;
    let mut remaining = day_of_year;
    for (m, &days) in dims.iter().enumerate() {
        if remaining < days {
            return ((m + 1) as u32, remaining + 1);
        }
        remaining -= days;
    }
    (12, 31)
}
