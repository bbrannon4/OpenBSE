//! OpenBSE command-line interface.
//!
//! Runs building energy simulations from YAML input files.

use anyhow::{Context, Result};
use clap::Parser;
use log::{info, warn};
use std::collections::{HashMap, HashSet, VecDeque};
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
    /// Name of the heat recovery component (if any) in this loop.
    /// Used for pre-processing heat recovery before the signal builder.
    heat_recovery_name: Option<String>,
    /// Efficiency of the boiler serving this loop's HW coils.
    /// Used to convert HR thermal credit to gas savings.
    hhw_boiler_efficiency: f64,
    /// Demand-controlled ventilation enabled for this loop.
    dcv: bool,
    /// Per-zone OA data for ASHRAE 62.1 VRP and DCV calculations.
    /// Always populated from zone connections (per_person_oa, per_area_oa).
    zone_oa_data: Vec<ZoneOaData>,
    /// Design supply air flow rate [m³/s] for this loop (used to compute dynamic OA fraction)
    design_supply_flow: f64,
    /// Economizer type for this loop.
    economizer_type: openbse_io::input::EconomizerType,
    /// Economizer high-limit shutoff temperature [°C] (for FixedDryBulb).
    economizer_high_limit: Option<f64>,
}

/// Per-zone data for ASHRAE 62.1 ventilation rate procedure.
/// Used for both DCV (dynamic occupancy) and multi-zone VRP (Ev correction).
#[derive(Debug, Clone)]
struct ZoneOaData {
    zone_name: String,
    design_people: f64,
    per_person_oa: f64,  // [m³/s per person]
    per_area_oa: f64,    // [m³/s per m²]
    floor_area: f64,     // [m²]
    people_schedule: Option<String>,
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
                EquipmentInput::Duct(d)          => d.name.clone(),
            }
        }).collect();

        let fan_names: HashSet<String> = al.equipment.iter().filter_map(|eq| {
            use openbse_io::input::EquipmentInput;
            match eq {
                EquipmentInput::Fan(f) => Some(f.name.clone()),
                _ => None,
            }
        }).collect();

        // Detect heat recovery component in this loop (if any)
        let heat_recovery_name: Option<String> = al.equipment.iter().find_map(|eq| {
            use openbse_io::input::EquipmentInput;
            match eq {
                EquipmentInput::HeatRecovery(hr) => Some(hr.name.clone()),
                _ => None,
            }
        });

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

        // Find the boiler efficiency for the HHW plant loop serving this
        // loop's hot water coils (used for HR gas credit calculation).
        let hhw_boiler_efficiency = {
            use openbse_io::input::{EquipmentInput, PlantEquipmentInput};
            // Find the plant loop name from the first HW coil's plant_loop field
            let hw_plant_loop: Option<&str> = al.equipment.iter().find_map(|eq| {
                if let EquipmentInput::HeatingCoil(c) = eq {
                    if c.source == "hot_water" {
                        c.plant_loop.as_deref()
                    } else {
                        None
                    }
                } else {
                    None
                }
            });
            // Find the boiler on that plant loop
            let eff = hw_plant_loop.and_then(|pl_name| {
                model.plant_loops.iter()
                    .find(|pl| pl.name == pl_name)
                    .and_then(|pl| {
                        pl.supply_equipment.iter().find_map(|eq| {
                            if let PlantEquipmentInput::Boiler(b) = eq {
                                Some(b.efficiency)
                            } else {
                                None
                            }
                        })
                    })
            });
            eff.unwrap_or(0.80) // Default 80% if no boiler found
        };

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
            heat_recovery_name,
            hhw_boiler_efficiency,
            dcv: al.dcv,
            // Always populate per-zone OA data from zone connections.
            // Needed for ASHRAE 62.1 VRP multi-zone Ev correction (even without DCV)
            // and for DCV occupancy-based OA modulation when dcv: true.
            zone_oa_data: al.zone_terminals.iter().filter_map(|zc| {
                let pp_oa = zc.per_person_oa.unwrap_or(0.0);
                let pa_oa = zc.per_area_oa.unwrap_or(0.0);
                if pp_oa == 0.0 && pa_oa == 0.0 { return None; }

                let zone = resolved_zones.iter().find(|z| z.name == zc.zone)?;
                let (design_people, people_sched) = zone.internal_gains.iter()
                    .find_map(|g| {
                        if let openbse_envelope::InternalGainInput::People { count, schedule, .. } = g {
                            Some((*count, schedule.clone()))
                        } else {
                            None
                        }
                    })
                    .unwrap_or((0.0, None));
                Some(ZoneOaData {
                    zone_name: zc.zone.clone(),
                    design_people,
                    per_person_oa: pp_oa,
                    per_area_oa: pa_oa,
                    floor_area: zone.floor_area,
                    people_schedule: people_sched,
                })
            }).collect(),
            economizer_type: al.controls.economizer.as_ref()
                .map(|e| e.economizer_type)
                .unwrap_or(openbse_io::input::EconomizerType::NoEconomizer),
            economizer_high_limit: al.controls.economizer.as_ref()
                .and_then(|e| e.high_limit),
            design_supply_flow: al.equipment.iter().find_map(|eq| {
                use openbse_io::input::EquipmentInput;
                if let EquipmentInput::Fan(f) = eq {
                    let flow = f.design_flow_rate.to_f64();
                    if flow > 0.0 { Some(flow) } else { None }
                } else {
                    None
                }
            }).unwrap_or(1.0),
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
            wh.parasitic_power = dhw_input.water_heater.parasitic_power;
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

    // Collect heat recovery names from air loops for end-use routing
    let heat_recovery_names: std::collections::HashSet<String> = model.air_loops.iter()
        .flat_map(|al| al.equipment.iter())
        .filter_map(|eq| match eq {
            openbse_io::input::EquipmentInput::HeatRecovery(hr) => Some(hr.name.clone()),
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

            // ── Per-loop cooling SAT override ──
            // Zone sizing uses the global min cooling_supply_temp.  Loops
            // with a higher cooling SAT (e.g., data center at 15.89°C vs
            // 12.8°C for offices) need proportionally more airflow.
            // Recalculate zone design airflows for those loops.
            let global_cool_sat = supply_temps.map(|t| t.1).unwrap_or(13.0);
            let cp_air_sz = 1005.0_f64;
            for li in &loop_infos {
                if (li.cooling_supply_temp - global_cool_sat).abs() > 0.5 {
                    for zone_name in &li.served_zones {
                        let cool_sp = resolved_thermostats.iter()
                            .find(|t| t.zones.contains(zone_name))
                            .map(|t| t.cooling_setpoint)
                            .unwrap_or(24.0);
                        let cool_load = sizing_result.zone_peak_cooling
                            .get(zone_name).copied().unwrap_or(0.0)
                            * model.simulation.cooling_sizing_factor;
                        let dt = (cool_sp - li.cooling_supply_temp).max(5.0);
                        let new_flow = if cool_load > 0.0 {
                            cool_load / (cp_air_sz * dt)
                        } else {
                            zone_design_flows.get(zone_name).copied().unwrap_or(0.01)
                        };
                        let old_flow = zone_design_flows.get(zone_name).copied().unwrap_or(0.01);
                        if new_flow > old_flow {
                            log::info!("Per-loop SAT override: {} airflow {:.2} → {:.2} kg/s \
                                (loop {} SAT={:.1}°C)", zone_name, old_flow, new_flow,
                                li.name, li.cooling_supply_temp);
                            zone_design_flows.insert(zone_name.clone(), new_flow);
                        }
                    }
                }
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
            // Compute standard air density at site altitude from design-day
            // barometric pressure (matches E+ site standard density).
            let site_pressure = model.design_days.first()
                .map(|dd| dd.pressure)
                .unwrap_or(101325.0);
            let air_density = site_pressure / (287.042 * 293.15);
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
                    || (lname.contains("hw") && !lname.contains("chw"))
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
            // Pass surface metadata for conduction summary
            let surface_meta: Vec<_> = env.surfaces.iter().map(|s| {
                let boundary_str = match &s.input.boundary {
                    openbse_envelope::surface::BoundaryCondition::Outdoor => "outdoor".to_string(),
                    openbse_envelope::surface::BoundaryCondition::Ground => "ground".to_string(),
                    openbse_envelope::surface::BoundaryCondition::Adiabatic => "adiabatic".to_string(),
                    openbse_envelope::surface::BoundaryCondition::Zone(z) => format!("zone:{}", z),
                };
                let type_str = match s.input.surface_type {
                    openbse_envelope::surface::SurfaceType::Wall => "wall",
                    openbse_envelope::surface::SurfaceType::Floor => "floor",
                    openbse_envelope::surface::SurfaceType::Roof => "roof",
                    openbse_envelope::surface::SurfaceType::Ceiling => "ceiling",
                    openbse_envelope::surface::SurfaceType::Window => "window",
                };
                (s.input.name.clone(), s.input.zone.clone(), type_str.to_string(),
                 s.net_area, s.is_window, boundary_str)
            }).collect();
            report.set_surface_metadata(surface_meta);
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

    // ── Zone thermal capacities for PLR correction ──────────────────────
    //
    // The PLR calculation uses frozen ideal loads from the previous timestep.
    // As the HVAC iteration updates zone temps, the frozen loads become stale.
    // The correction term adjusts the load based on zone temp changes:
    //
    //   Q_corrected = Q_ideal + C_zone × (T_initial - T_current)
    //
    // where C_zone = ρ_air × V_zone × c_p / Δt  (same as the cap_term in
    // compute_ideal_q_hvac in the envelope code).
    //
    // This makes PLR continuous near the setpoint, preventing the HVAC
    // iteration from oscillating between "full load" and "zero load" states
    // that never converge (the binary guard caused 14% energy waste).
    let zone_thermal_caps: HashMap<String, f64> = envelope.as_ref()
        .map(|env| {
            // Use the same air density as envelope heat balance (standard at site altitude).
            let site_pressure = model.design_days.first()
                .map(|dd| dd.pressure)
                .unwrap_or(101325.0);
            let rho_air = site_pressure / (287.042 * 293.15);
            env.zones.iter()
                .map(|z| {
                    // Use 3rd-order backward difference multiplier (11/6) to
                    // match the zone solve's effective thermal capacitance.
                    // This ensures the HVAC iteration convergence correction
                    // uses the same cap as the zone energy balance.
                    let cap_mult = 11.0_f64 / 6.0;
                    (z.input.name.clone(), rho_air * z.input.volume * 1006.0 * cap_mult / dt)
                })
                .collect()
        })
        .unwrap_or_default();

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
                    let empty_predictor: HashMap<String, f64> = HashMap::new();
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
                        &zone_thermal_caps,
                        &empty_predictor,
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
                    env.update_bdf_history();

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

    // ── Pre-compute plant loop simulation order (topological sort) ──────────
    //
    // Build a dependency graph from inter-loop references:
    // - HeatExchanger source_loop: source loop must simulate first
    // - Chiller condenser_plant_loop: CHW loop must simulate first
    //
    // Topological sort ensures correct ordering. If a cycle is detected
    // (e.g., waterside economizer creates CHW ↔ Condenser dependency),
    // remaining loops use lag-one-timestep for cyclic dependencies.

    let plant_loop_order: Vec<usize> = if model.plant_loops.is_empty() {
        vec![]
    } else {
        let loop_indices: HashMap<&str, usize> = model.plant_loops.iter()
            .enumerate()
            .map(|(i, pl)| (pl.name.as_str(), i))
            .collect();

        let n = model.plant_loops.len();
        let mut adj: Vec<Vec<usize>> = vec![vec![]; n];
        let mut in_degree: Vec<usize> = vec![0; n];

        for (i, pl) in model.plant_loops.iter().enumerate() {
            for eq in &pl.supply_equipment {
                // HX: source loop must simulate before demand loop
                if let openbse_io::input::PlantEquipmentInput::HeatExchanger(hx) = eq {
                    if let Some(&src_idx) = loop_indices.get(hx.source_loop.as_str()) {
                        adj[src_idx].push(i);
                        in_degree[i] += 1;
                    }
                }
                // Chiller with condenser loop: CHW loop simulates before condenser
                if let openbse_io::input::PlantEquipmentInput::Chiller(c) = eq {
                    if let Some(ref cdl) = c.condenser_plant_loop {
                        if let Some(&cond_idx) = loop_indices.get(cdl.as_str()) {
                            adj[i].push(cond_idx);
                            in_degree[cond_idx] += 1;
                        }
                    }
                }
            }
        }

        // Kahn's algorithm
        let mut queue: VecDeque<usize> = in_degree.iter()
            .enumerate()
            .filter(|(_, &d)| d == 0)
            .map(|(i, _)| i)
            .collect();
        let mut sorted: Vec<usize> = Vec::with_capacity(n);
        while let Some(node) = queue.pop_front() {
            sorted.push(node);
            for &dep in &adj[node] {
                in_degree[dep] -= 1;
                if in_degree[dep] == 0 {
                    queue.push_back(dep);
                }
            }
        }

        // If cycle detected, append remaining loops (they'll use lag-one-timestep)
        if sorted.len() < n {
            warn!("Plant loop dependency cycle detected — using lag-one-timestep for cyclic loops");
            for i in 0..n {
                if !sorted.contains(&i) {
                    sorted.push(i);
                }
            }
        }

        if sorted.len() > 1 {
            let order_names: Vec<&str> = sorted.iter()
                .map(|&i| model.plant_loops[i].name.as_str())
                .collect();
            info!("Plant loop simulation order: {:?}", order_names);
        }

        sorted
    };

    // Persistent supply conditions for lag-one-timestep cycle breaking.
    // Stores each loop's supply temperature and mass flow so downstream
    // loops (or cyclic dependencies) can read previous-timestep values.
    let mut loop_supply_conditions: HashMap<String, (f64, f64)> = HashMap::new();

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
                    env.update_bdf_history();

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

                    // Ideal loads at setpoint — initialized from previous timestep,
                    // then updated after each envelope solve to reflect current
                    // conditions.  This allows smooth load tapering during
                    // transitions (E+-style iterative convergence).
                    let mut current_cooling_loads: HashMap<String, f64> = env.zones.iter()
                        .map(|z| (z.input.name.clone(), z.ideal_cooling_load))
                        .collect();
                    let mut current_heating_loads: HashMap<String, f64> = env.zones.iter()
                        .map(|z| (z.input.name.clone(), z.ideal_heating_load))
                        .collect();

                    let mut final_hvac_result = None;
                    let mut final_env_result = None;

                    // E+-style predictor temps: free-floating zone temps WITHOUT
                    // HVAC, computed by the envelope.  Frozen from the PREVIOUS
                    // timestep and used for mode determination from the FIRST
                    // HVAC iteration (no need to wait for envelope to run).
                    let predictor_no_hvac_temps: HashMap<String, f64> = env.zones.iter()
                        .map(|z| (z.input.name.clone(), z.temp_no_hvac))
                        .collect();

                    // Track previous supply conditions for damping.
                    // ON/OFF cycling systems oscillate between full-capacity
                    // and zero, preventing convergence. Averaging successive
                    // supply conditions damps this oscillation.
                    let mut prev_supply_conditions: HashMap<String, (f64, f64)> = HashMap::new();

                    for hvac_iter in 0..MAX_HVAC_ITER {
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
                            &zone_thermal_caps,
                            &predictor_no_hvac_temps,
                        );

                        // Step 1b: Run plant loops in topological order.
                        //
                        // Loops are simulated in dependency order (pre-computed above):
                        // - Source loops before HX demand loops
                        // - CHW loops before condenser loops
                        // Each loop collects demand from air-side coils and/or
                        // condenser heat rejection, then simulates supply equipment.
                        // Supply conditions are stored for downstream dependencies.
                        for &loop_idx in &plant_loop_order {
                            let plant_loop = &model.plant_loops[loop_idx];
                            let cp_water = 4186.0; // J/(kg·K)
                            let rho_water = 998.0;  // kg/m³
                            let loop_delta_t = plant_loop.design_delta_t.max(1.0);

                            // ── Determine loop load ──────────────────────────
                            let mut total_load = 0.0_f64;

                            // 1. Air-side coil demand: sum thermal output from all
                            //    coils and terminal boxes referencing this plant loop.
                            for al in &model.air_loops {
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

                            // 2. Condenser demand: sum heat rejection from chillers
                            //    whose condenser_plant_loop references this loop.
                            //    Q_cond = Q_evap + W_compressor (already-simulated
                            //    chillers from upstream loops in topo order).
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

                            // Combine: condenser demand is always positive (heat rejection).
                            // If both coil and condenser demand exist, condenser dominates
                            // direction (this loop is a condenser loop receiving heat).
                            if condenser_load > 0.0 {
                                total_load = condenser_load;
                            }

                            // ── Inject HX source conditions ──────────────────
                            // For each HeatExchanger in this loop, provide source-side
                            // temperature and flow from the already-simulated source loop
                            // (or lag-one-timestep if source hasn't been simulated yet).
                            for equip in &plant_loop.supply_equipment {
                                if let openbse_io::input::PlantEquipmentInput::HeatExchanger(hx) = equip {
                                    if let Some(node_idx) = graph.node_by_name(&hx.name) {
                                        if let GraphComponent::Plant(component) = graph.component_mut(node_idx) {
                                            let (src_temp, src_flow) = loop_supply_conditions
                                                .get(&hx.source_loop)
                                                .copied()
                                                .unwrap_or((20.0, 0.0));
                                            component.set_source_conditions(src_temp, src_flow);
                                        }
                                    }
                                }
                            }

                            // ── Simulate loop equipment ──────────────────────
                            if total_load.abs() > 0.0 {
                                let loop_mass_flow = total_load.abs() / (cp_water * loop_delta_t);
                                // Inlet temp: condenser return is warmer, heating return
                                // is colder, cooling return is warmer.
                                let inlet_temp = if condenser_load > 0.0 {
                                    plant_loop.design_supply_temp + loop_delta_t
                                } else if total_load > 0.0 {
                                    plant_loop.design_supply_temp - loop_delta_t
                                } else {
                                    plant_loop.design_supply_temp + loop_delta_t
                                };

                                // Autosize pumps and cooling towers on first call
                                for equip in &plant_loop.supply_equipment {
                                    match equip {
                                        openbse_io::input::PlantEquipmentInput::Pump(p) => {
                                            if let Some(node_idx) = graph.node_by_name(&p.name) {
                                                if let GraphComponent::Plant(component) = graph.component_mut(node_idx) {
                                                    if component.design_water_flow_rate().is_none() {
                                                        let total_cap = if condenser_load > 0.0 {
                                                            // Condenser loop: size from chiller condenser capacities
                                                            let mut cap = 0.0_f64;
                                                            for ol in &model.plant_loops {
                                                                for eq2 in &ol.supply_equipment {
                                                                    if let openbse_io::input::PlantEquipmentInput::Chiller(c) = eq2 {
                                                                        if c.condenser_plant_loop.as_deref() == Some(plant_loop.name.as_str()) {
                                                                            let c_cap = c.capacity.to_f64();
                                                                            if c_cap > 0.0 {
                                                                                cap += c_cap * (1.0 + 1.0 / c.cop);
                                                                            }
                                                                        }
                                                                    }
                                                                }
                                                            }
                                                            cap
                                                        } else {
                                                            // Normal loop: size from boiler/chiller capacities
                                                            plant_loop.supply_equipment.iter()
                                                                .filter_map(|eq2| match eq2 {
                                                                    openbse_io::input::PlantEquipmentInput::Boiler(b) => Some(b.capacity.to_f64()),
                                                                    openbse_io::input::PlantEquipmentInput::Chiller(c) => Some(c.capacity.to_f64()),
                                                                    _ => None,
                                                                })
                                                                .filter(|c| *c > 0.0)
                                                                .sum()
                                                        };
                                                        let design_flow = total_cap / (rho_water * cp_water * loop_delta_t);
                                                        component.set_design_water_flow_rate(design_flow);
                                                    }
                                                }
                                            }
                                        }
                                        openbse_io::input::PlantEquipmentInput::CoolingTower(ct) => {
                                            // Autosize tower design_water_flow to match loop flow
                                            if let Some(node_idx) = graph.node_by_name(&ct.name) {
                                                if let GraphComponent::Plant(component) = graph.component_mut(node_idx) {
                                                    if component.design_water_flow_rate().is_none() {
                                                        // Size tower flow from condenser demand
                                                        let mut cap = 0.0_f64;
                                                        for ol in &model.plant_loops {
                                                            for eq2 in &ol.supply_equipment {
                                                                if let openbse_io::input::PlantEquipmentInput::Chiller(c) = eq2 {
                                                                    if c.condenser_plant_loop.as_deref() == Some(plant_loop.name.as_str()) {
                                                                        let c_cap = c.capacity.to_f64();
                                                                        if c_cap > 0.0 {
                                                                            cap += c_cap * (1.0 + 1.0 / c.cop);
                                                                        }
                                                                    }
                                                                }
                                                            }
                                                        }
                                                        let design_flow = cap / (rho_water * cp_water * loop_delta_t);
                                                        component.set_design_water_flow_rate(design_flow);
                                                    }
                                                }
                                            }
                                        }
                                        _ => {}
                                    }
                                }

                                // Sequential equipment loading
                                let mut remaining_load = total_load;
                                let mut current_inlet = WaterPort::new(
                                    openbse_psychrometrics::FluidState::water(inlet_temp, loop_mass_flow)
                                );
                                for equip in &plant_loop.supply_equipment {
                                    let equip_name = match equip {
                                        openbse_io::input::PlantEquipmentInput::Boiler(b) => &b.name,
                                        openbse_io::input::PlantEquipmentInput::Chiller(c) => &c.name,
                                        openbse_io::input::PlantEquipmentInput::Pump(p) => &p.name,
                                        openbse_io::input::PlantEquipmentInput::CoolingTower(ct) => &ct.name,
                                        openbse_io::input::PlantEquipmentInput::HeatExchanger(hx) => &hx.name,
                                    };
                                    let is_pump = matches!(equip, openbse_io::input::PlantEquipmentInput::Pump(_));
                                    if !is_pump && remaining_load.abs() < 1.0 { break; }
                                    if let Some(node_idx) = graph.node_by_name(equip_name) {
                                        if let GraphComponent::Plant(component) = graph.component_mut(node_idx) {
                                            let equip_load = if is_pump { total_load.abs() } else { remaining_load.abs() };
                                            let outlet = component.simulate_plant(&current_inlet, equip_load, &ctx);
                                            current_inlet = outlet;

                                            let delivered = component.thermal_output().abs();
                                            let mut plant_outputs: HashMap<String, f64> = HashMap::new();
                                            plant_outputs.insert("electric_power".to_string(), component.power_consumption());
                                            plant_outputs.insert("fuel_power".to_string(), component.fuel_consumption());
                                            plant_outputs.insert("thermal_output".to_string(), delivered);
                                            hvac_result.component_outputs.insert(equip_name.clone(), plant_outputs);

                                            if remaining_load > 0.0 {
                                                remaining_load -= delivered;
                                            } else {
                                                remaining_load += delivered;
                                            }
                                        }
                                    }
                                }

                                // Store supply conditions for downstream loops
                                loop_supply_conditions.insert(
                                    plant_loop.name.clone(),
                                    (current_inlet.state.temp, current_inlet.state.mass_flow),
                                );
                            }
                        }

                        // Step 2: Deliver HVAC supply air to envelope.
                        //
                        // Damp supply conditions to prevent ON/OFF cycling
                        // oscillation.  Without damping, the zone alternates
                        // between overcooled/overheated states every iteration,
                        // never converging.  The 50/50 blend of current and
                        // previous supply conditions converges to the correct
                        // equilibrium within 3-4 iterations.
                        let damped_supply: HashMap<String, (f64, f64)> = if hvac_iter > 0 {
                            zone_supply_conditions.iter().map(|(zn, &(t, m))| {
                                if let Some(&(pt, pm)) = prev_supply_conditions.get(zn) {
                                    // Enthalpy-correct damping: average mass flow,
                                    // then compute mixed temperature
                                    let avg_m = 0.5 * m + 0.5 * pm;
                                    let avg_t = if avg_m > 1e-6 {
                                        (0.5 * m * t + 0.5 * pm * pt) / avg_m
                                    } else {
                                        0.5 * t + 0.5 * pt
                                    };
                                    (zn.clone(), (avg_t, avg_m))
                                } else {
                                    (zn.clone(), (t, m))
                                }
                            }).collect()
                        } else {
                            zone_supply_conditions.clone()
                        };
                        prev_supply_conditions = zone_supply_conditions;

                        let mut hvac_conds = ZoneHvacConditions::default();

                        for (zone_name, (supply_temp, mass_flow)) in &damped_supply {
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
                        // Update ideal loads from the envelope so the NEXT
                        // HVAC iteration uses CURRENT conditions instead of
                        // stale previous-timestep loads.  This prevents the
                        // system from over/under-delivering during transitions
                        // (e.g., morning solar gain reducing heating need).
                        // E+ recomputes loads every iteration — matching that
                        // approach eliminates the oscillation seen with frozen loads.
                        for z in &env.zones {
                            if z.input.conditioned {
                                current_heating_loads.insert(
                                    z.input.name.clone(),
                                    z.ideal_heating_load,
                                );
                                current_cooling_loads.insert(
                                    z.input.name.clone(),
                                    z.ideal_cooling_load,
                                );
                            }
                        }

                        // Do NOT update predictor_no_hvac_temps during HVAC
                        // iterations. temp_no_hvac depends on surface temps
                        // which change with zone temp (HVAC-dependent), causing
                        // the predictor mode to flip between Heating and Deadband
                        // each iteration (non-convergence). Using the frozen
                        // previous-timestep predictor gives stable mode across
                        // all iterations, matching E+'s approach where the
                        // predictor is evaluated once before HVAC iteration.

                        final_hvac_result = Some(hvac_result);
                        final_env_result = Some(env_result);

                        if max_delta <= HVAC_CONV_TOL {
                            break;
                        }
                    }

                    (final_env_result.unwrap(), final_hvac_result.unwrap())
                };

                // Update BDF history ONCE after HVAC convergence.
                // Must not happen inside the HVAC iteration loop — that
                // would corrupt the backward-difference extrapolation.
                env.update_bdf_history();

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
                        } else if heat_recovery_names.contains(comp_name) {
                            snapshot.heat_recovery_power.insert(comp_name.clone(), pw_mult);
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

                    // Exhaust fan power → component_electric_power (name contains "fan"
                    // so output.rs routes it to fan_elec_j automatically)
                    if zone.exhaust_fan_power > 0.0 {
                        snapshot.component_electric_power.insert(
                            format!("Exhaust Fan {}", zone.input.name),
                            zone.exhaust_fan_power * mult,
                        );
                    }
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
                        } else if heat_recovery_names.contains(comp_name) {
                            snapshot.heat_recovery_power.insert(comp_name.clone(), pw_mult);
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

    // ── Diagnostic: print annual zone heat balance breakdown ──
    if let Some(ref env) = envelope {
        eprintln!("\n══════════════ ANNUAL ZONE HEAT BALANCE ══════════════");
        for zone in &env.zones {
            if !zone.input.conditioned { continue; }
            eprintln!("Zone: {}", zone.input.name);
            eprintln!("  Surface cond loss:  {:>10.1} kWh  (positive = zone losing heat)", zone.diag_surface_loss_kwh);
            eprintln!("  Infiltration loss:  {:>10.1} kWh  (positive = zone losing heat)", zone.diag_infil_loss_kwh);
            eprintln!("  Internal gains:     {:>10.1} kWh  (convective only)", zone.diag_internal_conv_kwh);
            eprintln!("  Solar transmitted:  {:>10.1} kWh  (into zone)", zone.diag_solar_trans_kwh);
            eprintln!("  Window conduction:  {:>10.1} kWh  (positive = zone losing heat)", zone.diag_window_cond_kwh);
            eprintln!("  Window convection:  {:>10.1} kWh  (h_conv × A × (T_zone - T_glass))", zone.diag_window_conv_kwh);
            eprintln!("  Q_conv (all):       {:>10.1} kWh  (radiative+internal convective)", zone.diag_q_conv_kwh);
            eprintln!("  HVAC delivered:     {:>10.1} kWh  (net: positive = heating)", zone.diag_hvac_net_kwh);
            let balance = -zone.diag_surface_loss_kwh - zone.diag_infil_loss_kwh
                + zone.diag_q_conv_kwh + zone.diag_hvac_net_kwh;
            eprintln!("  Balance check:      {:>10.1} kWh  (should be ~0)", balance);
        }
        eprintln!("══════════════════════════════════════════════════════\n");
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
    zone_thermal_caps: &HashMap<String, f64>,
    predictor_no_hvac_temps: &HashMap<String, f64>,
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
                    // System scheduled OFF — no night-cycle (matches E+
                    // simplified model without AvailabilityManager:NightCycle).
                    //
                    // Night-cycle availability management is tracked as a
                    // future feature in the README.
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
        let effective_min_oa = if is_unoccupied {
            0.0
        } else if li.dcv && !li.zone_oa_data.is_empty() {
            // ── Demand-Controlled Ventilation ──────────────────────────
            //
            // ASHRAE 62.1 Ventilation Rate Procedure with real-time occupancy:
            //   OA = Σ (per_person × design_people × schedule_frac + per_area × area)
            //
            // The per_area component is always required (dilution ventilation for
            // building materials), but the per_person component scales with actual
            // occupancy.
            //
            // IMPORTANT: The minimum_damper_position (min_oa_fraction) represents
            // the DESIGN outdoor air fraction at full occupancy.  It already
            // accounts for ASHRAE 62.1/170 requirements.  DCV can only REDUCE
            // OA during partial occupancy — never INCREASE it above the design
            // level.  Without this cap, the per_person_oa/per_area_oa rates
            // may compute higher fractions than the original design, inflating
            // heating, cooling, and humidification loads.
            //
            // At full occupancy:  effective_min_oa = min_oa_fraction (design level)
            // At zero occupancy:  effective_min_oa = area_floor (building dilution)
            let mut dynamic_oa_flow = 0.0_f64;
            for dcv in &li.zone_oa_data {
                let occ_frac = if let Some(ref sched_name) = dcv.people_schedule {
                    schedule_mgr
                        .map(|sm| sm.fraction(sched_name, hour, day_of_week))
                        .unwrap_or(1.0)
                } else {
                    1.0 // No schedule → always full occupancy
                };
                let person_flow = dcv.per_person_oa * dcv.design_people * occ_frac;
                let area_flow = dcv.per_area_oa * dcv.floor_area;
                dynamic_oa_flow += person_flow + area_flow;
            }
            let dcv_frac = if li.design_supply_flow > 0.0 {
                (dynamic_oa_flow / li.design_supply_flow).clamp(0.0, 1.0)
            } else {
                li.min_oa_fraction
            };
            // Area-based floor: absolute minimum per ASHRAE 62.1
            let area_only_flow: f64 = li.zone_oa_data.iter()
                .map(|d| d.per_area_oa * d.floor_area)
                .sum();
            let area_floor = if li.design_supply_flow > 0.0 {
                (area_only_flow / li.design_supply_flow).clamp(0.0, 1.0)
            } else {
                0.0
            };
            // Cap at min_oa_fraction (design OA); floor at area-only dilution
            dcv_frac.max(area_floor).min(li.min_oa_fraction)
        } else {
            li.min_oa_fraction
        };

        // Select active setpoints based on occupied/unoccupied state
        let active_heat_sp = if is_unoccupied { zone_unocc_heat_sp } else { zone_heat_sp };
        let active_cool_sp = if is_unoccupied { zone_unocc_cool_sp } else { zone_cool_sp };

        // ── Heat Recovery Pre-Processing ────────────────────────────────
        //
        // If this loop has a heat recovery component, compute the effective
        // outdoor air temperature and humidity AFTER the HR wheel.  The HR
        // pre-conditions outdoor air using exhaust (return) air:
        //
        //   T_effective = T_outdoor + ε_s × (T_return - T_outdoor)
        //   W_effective = W_outdoor + ε_l × (W_return - W_outdoor)
        //
        // ── Credit-based approach ───────────────────────────────────
        //
        // The HR is NOT included in the signal builder or component chain.
        // Instead, the simulation runs as if there's no HR (using raw
        // outdoor temp for ALL control decisions and mixed air calculations).
        // After the component chain runs, we compute the HR's thermal
        // recovery and apply it as a gas credit.
        //
        // This approach is necessary because the inline approach (using
        // effective_t_outdoor in mixed air) causes paradoxical heating gas
        // increases: warmer mixed air triggers cooling mode more often,
        // dropping SAT to 12.8°C, which then requires massive terminal
        // reheat from the boiler.  Until per-zone VAV flow modulation is
        // implemented, the credit approach is more accurate.
        //
        // EXHAUST CONDITIONS: Use the zone HEATING SETPOINT (~21°C) as
        // the design exhaust temperature.  Zones can be unrealistically
        // cold (–60 to –140°C) because the simulation runs without HR.
        // The setpoint represents the intended operating point.
        if let Some(ref hr_name) = li.heat_recovery_name {
            let avg_return_temp = if li.served_zones.is_empty() {
                22.0
            } else {
                li.served_zones.iter()
                    .map(|z| active_heat_sp.get(z).copied().unwrap_or(21.0))
                    .sum::<f64>() / li.served_zones.len() as f64
            };
            let avg_return_w = openbse_psychrometrics::MoistAirState::from_tdb_rh(
                avg_return_temp, 0.50, ctx.outdoor_air.p_b,
            ).w;
            if let Some(node_idx) = graph.node_by_name(hr_name) {
                match graph.component_mut(node_idx) {
                    GraphComponent::Air(ref mut comp) => {
                        comp.set_exhaust_conditions(avg_return_temp, avg_return_w);
                    }
                    _ => {}
                }
            }
        }

        // ── Predictor Mode ─────────────────────────────────────────────
        //
        // E+-style predictor for HVAC mode determination.
        //
        // PRIMARY: Use the free-floating zone temperature (temp_no_hvac)
        // computed by the envelope with CURRENT timestep conditions
        // (solar, outdoor temp, surface temps) and HVAC = 0.  This tells
        // us: "would the zone stay within the deadband if we turned off
        // HVAC?"  If yes → Deadband (coast on thermal mass).
        //
        // This prevents the self-reinforcing heating cycle where stale
        // ideal loads always indicate "heating needed" because the zone
        // was held at setpoint, preventing deadband coasting.
        //
        // FALLBACK (first iteration of each timestep, before envelope
        // has run with current conditions): use frozen ideal loads from
        // the previous timestep.
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

                // Primary: use predictor temps (E+-style free-floating prediction)
                // This uses CURRENT timestep conditions, not stale loads.
                let mode = if let Some(&t_predicted) = predictor_no_hvac_temps.get(z.as_str()) {
                    if t_predicted < hsp {
                        HvacMode::Heating
                    } else if t_predicted > csp {
                        HvacMode::Cooling
                    } else {
                        HvacMode::Deadband
                    }
                }
                // Fallback: ideal loads (first iteration before envelope runs)
                else if hload > 10.0 && hload > cload {
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

        let mut signals = match li.system_type {
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
                // All controls use raw t_outdoor. HR credit is applied post-chain.
                build_vav_signals(li, zone_temps, active_heat_sp, active_cool_sp,
                    zone_design_flows, &zone_multipliers, t_outdoor, effective_min_oa,
                    false, t_outdoor, schedule_mgr, hour, day_of_week)
            }
        };

        // Filter heat recovery out of the component chain — it was already
        // pre-processed above and its effect is baked into effective_t_outdoor.
        let chain_components: Vec<String> = if let Some(ref hr_name) = li.heat_recovery_name {
            li.component_names.iter()
                .filter(|n| n.as_str() != hr_name.as_str())
                .cloned()
                .collect()
        } else {
            li.component_names.clone()
        };

        // Run this loop's components in order (at full capacity, PLR=1.0)
        let (mut loop_result, supply_air) = simulate_loop_components(
            graph, ctx, &chain_components, &signals, zone_temps, t_outdoor
        );

        // ── Post-process heat recovery: credit-based approach ─────────
        //
        // The simulation ran as if there's no HR (raw t_outdoor for all
        // controls).  Now compute what the HR would recover and apply it
        // as a gas/electric credit via virtual components.
        if let Some(ref hr_name) = li.heat_recovery_name {
            let oa_frac = signals.coil_setpoints.get("__oa_fraction__")
                .copied().unwrap_or(effective_min_oa);
            let total_flow = supply_air.as_ref().map(|s| s.mass_flow).unwrap_or(0.0);
            let oa_mass_flow = total_flow * oa_frac;

            let mut hr_out = HashMap::new();
            let mut hr_thermal = 0.0_f64;

            if oa_mass_flow > 0.0 {
                if let Some(node_idx) = graph.node_by_name(hr_name) {
                    match graph.component_mut(node_idx) {
                        GraphComponent::Air(ref mut comp) => {
                            let oa_inlet = AirPort::new(ctx.outdoor_air, oa_mass_flow);
                            let hr_outlet = comp.simulate_air(&oa_inlet, ctx);

                            hr_thermal = comp.thermal_output();
                            let hr_electric = comp.power_consumption();

                            hr_out.insert("outlet_temp".to_string(), hr_outlet.state.t_db);
                            hr_out.insert("outlet_w".to_string(), hr_outlet.state.w);
                            hr_out.insert("mass_flow".to_string(), oa_mass_flow);
                            hr_out.insert("outlet_enthalpy".to_string(), hr_outlet.state.h);
                            hr_out.insert("electric_power".to_string(), hr_electric);
                            hr_out.insert("fuel_power".to_string(), 0.0);
                            hr_out.insert("thermal_output".to_string(), hr_thermal);
                        }
                        _ => {}
                    }
                }
            } else {
                hr_out.insert("outlet_temp".to_string(), t_outdoor);
                hr_out.insert("outlet_w".to_string(), ctx.outdoor_air.w);
                hr_out.insert("mass_flow".to_string(), 0.0);
                hr_out.insert("outlet_enthalpy".to_string(), ctx.outdoor_air.h);
                hr_out.insert("electric_power".to_string(), 0.0);
                hr_out.insert("fuel_power".to_string(), 0.0);
                hr_out.insert("thermal_output".to_string(), 0.0);
            }

            // ── Apply HR credit via virtual components ────────────────
            //
            // Cap credit at the AHU coil's heating/cooling load to prevent
            // overcrediting.  The coil load = m_dot × cp × ΔT where ΔT
            // is the difference between SAT and mixed air temp.
            if hr_thermal > 0.0 {
                // Winter heating credit: cap at what AHU coil actually provides
                let avg_zt = if li.served_zones.is_empty() { 22.0 }
                    else { li.served_zones.iter()
                        .map(|z| zone_temps.get(z).copied().unwrap_or(22.0))
                        .sum::<f64>() / li.served_zones.len() as f64 };
                let t_mixed = avg_zt * (1.0 - oa_frac) + t_outdoor * oa_frac;
                // Use actual SAT setpoint from VAV signal builder (12.8-15.6°C)
                // instead of heating_supply_temp (40°C) which is for terminal reheat.
                // This prevents over-crediting HR by 6x.
                let sat = if signals.sat_setpoint > 0.0 {
                    signals.sat_setpoint
                } else {
                    li.heating_supply_temp
                };
                let coil_load = total_flow * 1005.0 * (sat - t_mixed).max(0.0);
                let capped = hr_thermal.min(coil_load);
                let gas_credit = capped / li.hhw_boiler_efficiency;
                let credit_name = format!("{} HR Heat Savings", li.name);
                let mut c = HashMap::new();
                c.insert("fuel_power".to_string(), -gas_credit);
                c.insert("electric_power".to_string(), 0.0);
                c.insert("thermal_output".to_string(), 0.0);
                loop_result.insert(credit_name, c);
            } else if hr_thermal < 0.0 {
                // Summer cooling credit
                let avg_zt = if li.served_zones.is_empty() { 22.0 }
                    else { li.served_zones.iter()
                        .map(|z| zone_temps.get(z).copied().unwrap_or(22.0))
                        .sum::<f64>() / li.served_zones.len() as f64 };
                let t_mixed = avg_zt * (1.0 - oa_frac) + t_outdoor * oa_frac;
                let sat = li.cooling_supply_temp;
                let coil_load = total_flow * 1005.0 * (t_mixed - sat).max(0.0);
                let capped = hr_thermal.abs().min(coil_load);
                let elec_credit = capped / 3.5; // chiller COP
                let credit_name = format!("{} HR Cool Savings", li.name);
                let mut c = HashMap::new();
                c.insert("fuel_power".to_string(), 0.0);
                c.insert("electric_power".to_string(), -elec_credit);
                c.insert("thermal_output".to_string(), 0.0);
                loop_result.insert(credit_name, c);
            }

            loop_result.insert(hr_name.clone(), hr_out);
        }

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

                // Zone thermal capacity correction for HVAC iteration
                // convergence.
                //
                // The frozen ideal loads (from the previous timestep) include
                // a thermal-mass term:  Cap × (T_setpoint − T_prev).
                // As the HVAC iteration updates zone temp, that term becomes
                // stale.  The correction adjusts the load so PLR is smooth
                // and the iteration converges instead of oscillating:
                //
                //   Heating: Q_corrected = Q_ideal + Cap × (T_initial − T_current)
                //   Cooling: Q_corrected = Q_ideal + Cap × (T_current − T_initial)
                //
                // When zone temp rises above the heating setpoint during
                // iteration, the correction REDUCES the heating load (smooth
                // convergence).  The old binary guard (control_temp >= heat_sp
                // → PLR = 0) created a discontinuity that caused the HVAC
                // iteration to oscillate between full-load and zero-load,
                // never converging, wasting ~14% of annual heating fuel.
                //
                // A dead-band safety check prevents stale loads from causing
                // heating when the zone is well above setpoint (e.g., after a
                // setpoint transition from occupied to unoccupied mode).
                let zone_cap = zone_thermal_caps.get(control_zone).copied().unwrap_or(0.0);
                let init_temp = initial_zone_temps.get(control_zone).copied().unwrap_or(control_temp);
                let dead_band = (cool_sp - heat_sp).max(0.5);

                match mode {
                    HvacMode::Heating => {
                        let q_capacity = supply_flow * cp_air * (supply_temp - heat_sp);
                        if control_temp > heat_sp + dead_band * 0.5 {
                            // Zone well above heating setpoint (e.g., setpoint
                            // transition to unoccupied).  Stale ideal load is
                            // for the old setpoint — do not heat.
                            effective_min_oa
                        } else if q_capacity < 100.0 {
                            effective_min_oa
                        } else {
                            // Correct frozen ideal load for zone temp changes
                            // during HVAC iteration.
                            let correction = zone_cap * (init_temp - control_temp);
                            let corrected_load = (zone_heat_load + correction).max(0.0);

                            if corrected_load > 10.0 {
                                let adj_load = (corrected_load - q_fan).max(0.0);
                                let adj_cap = (q_capacity - q_fan).max(1.0);
                                (adj_load / adj_cap).clamp(effective_min_oa, 1.0)
                            } else {
                                // Fallback: proportional zone error for transients
                                let error = (heat_sp - control_temp).max(0.0);
                                let max_dt = (supply_temp - heat_sp).max(1.0);
                                (error / max_dt).clamp(effective_min_oa, 1.0)
                            }
                        }
                    }
                    HvacMode::Cooling => {
                        let q_capacity = supply_flow * cp_air * (cool_sp - supply_temp);
                        if control_temp < cool_sp - dead_band * 0.5 {
                            // Zone well below cooling setpoint — do not cool.
                            effective_min_oa
                        } else if q_capacity < 100.0 {
                            effective_min_oa
                        } else {
                            // Correct frozen ideal load for zone temp changes
                            let correction = zone_cap * (control_temp - init_temp);
                            let corrected_load = (zone_cool_load + correction).max(0.0);

                            if corrected_load > 10.0 {
                                let adj_load = corrected_load + q_fan;
                                let adj_cap = q_capacity + q_fan;
                                (adj_load / adj_cap).clamp(effective_min_oa, 1.0)
                            } else {
                                let error = (control_temp - cool_sp).max(0.0);
                                let max_dt = (cool_sp - supply_temp).max(1.0);
                                (error / max_dt).clamp(effective_min_oa, 1.0)
                            }
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

            // E+ Part Load Fraction: accounts for compressor cycling losses.
            // RTF = PLR / PLF > PLR, so compressor runs longer per unit of
            // cooling delivered (startup losses, refrigerant migration, etc.).
            // Default: PLF = 1 - Cd*(1-PLR) with Cd=0.15 (E+ default).
            // Fan power uses PLR directly (no cycling penalty).
            let plf = (1.0 - 0.15 * (1.0 - loop_plr)).max(0.7);
            let rtf = loop_plr / plf;

            for (comp_name, outputs) in &mut loop_result {
                let is_fan = li.fan_names.contains(comp_name);

                if is_continuous_fan && is_fan {
                    // Continuous fan mode: fan runs at full speed always.
                    // Fan power and thermal output are NOT scaled by PLR.
                    // Mass flow is NOT scaled — fan pushes air continuously.
                    // (No changes needed — outputs stay at full rated values.)
                } else {
                    // DX compressor electric power uses RTF (includes cycling
                    // penalty via PLF curve). Gas furnace fuel and fan power
                    // use PLR directly (no compressor cycling penalty).
                    //
                    // In E+, the PLF curve is specific to DX coils — gas
                    // furnaces report fuel = Q / eff × PLR without cycling
                    // degradation.  Fan power = rated × PLR (direct cycling).
                    let is_dx_coil = !is_fan && outputs.get("fuel_power")
                        .map_or(true, |fp| *fp == 0.0);
                    let power_factor = if is_dx_coil { rtf } else { loop_plr };
                    if let Some(ep) = outputs.get_mut("electric_power") {
                        *ep *= power_factor;
                    }
                    if let Some(fp) = outputs.get_mut("fuel_power") {
                        *fp *= loop_plr;
                    }
                    // Thermal output and mass flow scale with PLR
                    // (time-averaged delivery to the zone).
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
                        // Use per-zone design flow as terminal inlet. The terminal
                        // box's internal damper modulates between min_flow and
                        // max_flow based on the control signal, producing the actual
                        // demanded flow for this zone. This matches E+'s approach:
                        // duct delivers up to design flow, VAV box takes what it
                        // needs. zone_design_flows[zone] == terminal.max_air_flow
                        // (both set from the sizing run).
                        let term_inlet_flow = zone_design_flows
                            .get(zone_name)
                            .copied()
                            .unwrap_or(effective_flow / li.served_zones.len().max(1) as f64);
                        let term_inlet = AirPort::new(supply.state, term_inlet_flow);
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
    let predictor_mode = predictor_modes.get(control_zone).copied()
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

    // Safety override: prevent heating when zone is already above cooling
    // setpoint (and vice versa).  With on/off cycling at high capacity,
    // the predictor mode can be stale by one timestep, causing the system
    // to fire heating into an already-warm zone.  This guard prevents the
    // resulting temperature oscillation.
    let mode = match predictor_mode {
        HvacMode::Heating if control_temp > cool_sp => HvacMode::Cooling,
        HvacMode::Cooling if control_temp < heat_sp => HvacMode::Heating,
        other => other,
    };

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
    let plr = 1.0_f64; // Placeholder — real PLR computed post-simulation

    // Components run at FULL design flow (fan ON at full speed when cycling)
    let flow = total_flow;

    // ── Heating DAT ──
    // On/Off: E+ PSZ-AC with Fan:OnOff fires the heating coil at full
    //   capacity whenever the system is ON.  PLR controls runtime, not
    //   supply temperature.  Fixed DAT = heating_supply_temp.
    // Proportional: modulate supply temp based on deviation from setpoint.
    //   DAT ramps from setpoint to max over a 5°C error band, giving
    //   smooth modulation for systems with variable-capacity burners.
    let heating_dat = match li.cycling {
        openbse_io::input::CyclingMethod::OnOff => li.heating_supply_temp,
        openbse_io::input::CyclingMethod::Proportional => {
            let error = (heat_sp - control_temp).max(0.0);
            (heat_sp + (li.heating_supply_temp - heat_sp) * (error / 5.0).min(1.0))
                .clamp(heat_sp, li.heating_supply_temp)
        }
    };

    // ── Cooling control ──
    // Economizer target: modulate OA to achieve the supply air temperature
    // (SAT) in the mixed air, minimizing cooling coil work.  This matches
    // E+'s Controller:OutdoorAir behavior where the OA damper targets the
    // mixed-air setpoint derived from the cooling-coil leaving-air temp.
    // Use the loop's cooling SAT as the economizer target.
    let econ_target = li.cooling_supply_temp;
    // Coil setpoint: -10°C forces the DX coil to run at full physical capacity.
    // The coil's actual outlet temp is limited by its available capacity.
    let cooling_coil_sp = if mode == HvacMode::Cooling { -10.0 } else { 99.0 };

    // ── Economizer: respects loop economizer type ──
    // FixedDryBulb: OA used when OAT < high_limit
    // DifferentialDryBulb: OA used when OAT < return air temp
    // NoEconomizer: always minimum OA
    let return_air_temp = control_temp;
    use openbse_io::input::EconomizerType;
    let psz_econ_available = match li.economizer_type {
        EconomizerType::NoEconomizer => false,
        EconomizerType::FixedDryBulb => {
            let limit = li.economizer_high_limit.unwrap_or(23.889);
            t_outdoor < limit
        }
        EconomizerType::DifferentialDryBulb => t_outdoor < return_air_temp,
        EconomizerType::DifferentialEnthalpy => t_outdoor < return_air_temp,
    };
    let oa_frac = if psz_econ_available && mode != HvacMode::Heating {
        // Economizer: modulate OA to approach SAT target in mixed air.
        // Active in both Cooling and Deadband — provides free cooling from
        // outdoor air, reducing or eliminating mechanical cooling.  Matches
        // E+'s economizer which operates whenever OA conditions are favorable,
        // regardless of whether the cooling coil is currently active.
        let delta = return_air_temp - t_outdoor;
        if delta > 0.1 {
            let needed = (return_air_temp - econ_target) / delta;
            needed.clamp(effective_min_oa, 1.0)
        } else {
            effective_min_oa
        }
    } else {
        effective_min_oa
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
                    || lname.contains("hw") || lname.starts_with("hc ") || lname.starts_with("hc_") {
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
                    || lname.contains("hw") || lname.starts_with("hc ") || lname.starts_with("hc_") {
                    signals.coil_setpoints.insert(name.clone(), -99.0);
                }
            }
            HvacMode::Deadband => {
                if lname.contains("heat") || lname.contains("furnace")
                    || lname.contains("hw") || lname.starts_with("hc ") || lname.starts_with("hc_") {
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
        "__return_air_temp__".to_string(),
        return_air_temp,
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
            || lname.contains("hw") || lname.starts_with("hc ") || lname.starts_with("hc_") {
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
                        || lname.contains("hw") || lname.starts_with("hc ") || lname.starts_with("hc_") {
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
                        || lname.contains("hw") || lname.starts_with("hc ") || lname.starts_with("hc_") {
                        signals.coil_setpoints.insert(name.clone(), -99.0);
                    }
                }
                HvacMode::Deadband => {
                    if lname.contains("heat") || lname.contains("reheat")
                        || lname.contains("hw") || lname.starts_with("hc ") || lname.starts_with("hc_") {
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
                        || lname.contains("hw") || lname.starts_with("hc ") || lname.starts_with("hc_") {
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
                        || lname.contains("hw") || lname.starts_with("hc ") || lname.starts_with("hc_") {
                        signals.coil_setpoints.insert(name.clone(), -99.0);
                    }
                }
                HvacMode::Deadband => {
                    if lname.contains("heat") || lname.contains("reheat")
                        || lname.contains("hw") || lname.starts_with("hc ") || lname.starts_with("hc_") {
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
    economizer_lockout: bool,
    raw_t_outdoor: f64,
    schedule_mgr: Option<&ScheduleManager>,
    hour: u32,
    day_of_week: u32,
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

    // ── ASHRAE 62.1 §6.2.5 Multi-Zone VRP: System Ventilation Efficiency ──
    //
    // In a multi-zone recirculating system (VAV), all zones share the same
    // mixed air (same OA fraction). When zones are at part load (minimum
    // flow), they receive less absolute OA than needed. The VRP corrects
    // by increasing the system OA fraction based on the "critical zone"
    // — the zone with the highest required discharge OA fraction (Zd).
    //
    // E+ implements this via Controller:MechanicalVentilation.
    let vrp_min_oa = if !li.zone_oa_data.is_empty() {
        let air_density = 1.204_f64; // kg/m³ at standard conditions
        let mut vou = 0.0_f64;       // uncorrected total OA [m³/s]
        let mut max_zd = 0.0_f64;    // critical zone discharge OA fraction

        for oa in &li.zone_oa_data {
            // Occupancy fraction from people schedule (design occupancy if no schedule)
            let occ_frac = if let Some(ref sched_name) = oa.people_schedule {
                schedule_mgr
                    .map(|sm| sm.fraction(sched_name, hour, day_of_week))
                    .unwrap_or(1.0)
            } else {
                1.0
            };

            // Breathing zone OA [m³/s]: ASHRAE 62.1 Eq 6-1
            let vbz = oa.per_person_oa * oa.design_people * occ_frac
                    + oa.per_area_oa * oa.floor_area;
            // Zone OA with distribution effectiveness: Voz = Vbz / Ez
            // Ez = 1.0 for well-mixed ceiling supply (ASHRAE 62.1 Table 6-2)
            let voz = vbz;
            vou += voz;

            // Zone discharge OA fraction: Zd = Voz / Vdz
            // Vdz = actual zone airflow [m³/s]
            let vdz_kg = signals.zone_air_flows.get(&oa.zone_name)
                .copied()
                .unwrap_or(0.1);
            let vdz = vdz_kg / air_density; // kg/s → m³/s
            if vdz > 0.001 {
                let zd = voz / vdz;
                max_zd = max_zd.max(zd);
            }
        }

        // System ventilation efficiency: ASHRAE 62.1 Eq 6-6
        // Ev = 1 + Xs - max(Zd)
        let vps = total_flow / air_density; // total supply [m³/s]
        let xs = if vps > 0.01 { vou / vps } else { 0.0 };
        let ev = (1.0 + xs - max_zd).clamp(0.15, 1.0);

        // Corrected system OA: Vot = Vou / Ev
        let vot = vou / ev;
        let ys = if vps > 0.01 { vot / vps } else { effective_min_oa };

        // VRP OA fraction: never less than the original design OA
        ys.clamp(effective_min_oa, 1.0)
    } else {
        effective_min_oa
    };

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
    //
    // IMPORTANT: The economizer decides OA fraction based on RAW outdoor
    // temperature (not post-HR effective temperature).  The economizer benefits
    // from cold OA for free cooling — the HR's preheating effect would mislead
    // the economizer into thinking OA is warmer than it actually is.
    //
    // The mixed air calculation then uses effective_t_outdoor (= t_outdoor param)
    // which already includes the HR preheating effect.
    let any_cooling = max_cooling_demand > 0.0;
    use openbse_io::input::EconomizerType;
    let econ_available = match li.economizer_type {
        EconomizerType::NoEconomizer => false,
        EconomizerType::FixedDryBulb => {
            let limit = li.economizer_high_limit.unwrap_or(23.889);
            raw_t_outdoor < limit
        }
        EconomizerType::DifferentialDryBulb => raw_t_outdoor < avg_zone_temp,
        EconomizerType::DifferentialEnthalpy => raw_t_outdoor < avg_zone_temp, // approximate
    };
    let oa_frac = if economizer_lockout {
        // HR active → economizer locked to minimum OA (E+ EconomizerLockout: Yes)
        vrp_min_oa
    } else if any_cooling && econ_available {
        // Use RAW outdoor temp for economizer decisions
        let delta = avg_zone_temp - raw_t_outdoor;
        if delta > 0.1 {
            // Modulate OA to reach SAT setpoint as mixed air target
            let needed = (avg_zone_temp - sat_setpoint) / delta;
            needed.clamp(vrp_min_oa, 1.0)
        } else {
            vrp_min_oa
        }
    } else {
        vrp_min_oa
    };
    // Mixed air uses effective (post-HR) outdoor temperature
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
            || lname.contains("hw") || lname.starts_with("hc ") || lname.starts_with("hc_") {
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
    signals.coil_setpoints.insert(
        "__return_air_temp__".to_string(),
        avg_zone_temp,
    );

    // Store SAT setpoint for heat recovery credit cap calculation
    signals.sat_setpoint = sat_setpoint;

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
    zone_temps: &HashMap<String, f64>,
    t_outdoor: f64,
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
        // Blend humidity: w_mixed = OA_frac * w_oa + (1 - OA_frac) * w_indoor
        // When heat recovery is present, use the post-HR outdoor humidity (effective OA w)
        // instead of raw outdoor humidity. This accounts for moisture transfer in the ERV.
        let w_oa = signals.coil_setpoints.get("__effective_oa_w__")
            .copied()
            .unwrap_or(ctx.outdoor_air.w);
        let w_indoor = openbse_psychrometrics::MoistAirState::from_tdb_rh(
            inlet_temp_override.unwrap_or(ctx.outdoor_air.t_db), 0.50, ctx.outdoor_air.p_b,
        ).w;
        let w_mixed = oa_fraction * w_oa + (1.0 - oa_fraction) * w_indoor;
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

                // Resolve duct ambient temperature before simulation
                if let Some(amb_zone) = component.ambient_zone().map(|s| s.to_string()) {
                    let amb_temp = match amb_zone.as_str() {
                        "outdoor" => t_outdoor,
                        "ground" => 18.0, // default ground temp
                        zone_name => zone_temps.get(zone_name)
                            .copied()
                            .unwrap_or(t_outdoor),
                    };
                    component.set_ambient_temp(amb_temp);
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
