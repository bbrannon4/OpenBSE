//! OpenBSE command-line interface.
//!
//! Runs building energy simulations from YAML input files.

use anyhow::{Context, Result};
use clap::Parser;
use log::{info, warn};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use openbse_core::graph::{GraphComponent, SimulationGraph};
use openbse_core::ports::{AirPort, EnvelopeSolver, SimulationContext, WaterPort, ZoneHvacConditions};
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
            }
        }).collect();

        let served_zones: Vec<String> = al.zones.iter()
            .map(|zc| zc.zone.clone())
            .collect();

        // Auto-detect or use explicit system type
        let system_type = al.detect_system_type();

        // Resolve minimum outdoor air fraction:
        //   1. DOAS always 100%
        //   2. Explicit controls.minimum_damper_position
        //   3. Auto-calculate from zone outdoor air requirements
        //   4. Fallback: 20%
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

        LoopInfo {
            name: al.name.clone(),
            system_type,
            component_names,
            served_zones,
            min_oa_fraction,
            min_vav_fraction: al.min_vav_fraction,
            availability_schedule: al.availability_schedule.clone(),
            heating_supply_temp: al.controls.heating_supply_temp,
            cooling_supply_temp: al.controls.cooling_supply_temp,
            cycling: al.controls.cycling,
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

    // Set up ground temperature model from weather data.
    //
    // EnergyPlus uses monthly ground temperatures for slab-on-grade F-factor
    // calculations (Site:GroundTemperature:FCfactorMethod). The default source
    // is the 0.5 m depth undisturbed ground temperatures from the EPW header.
    //
    // Priority:
    //   1. EPW ground temps at 0.5 m depth (matches E+ FCfactorMethod default)
    //   2. Kusuda-Achenbach at 0.5 m depth (fallback when EPW lacks ground data)
    if let Some(ref mut env) = envelope {
        let mut ground_temp = openbse_envelope::GroundTempModel::from_weather_hours(&weather_data.hours);

        // Use EPW ground temperatures at 0.5 m depth if available
        // (matches E+ Site:GroundTemperature:FCfactorMethod default)
        if let Some(epw_gt) = weather_data.ground_temperatures.iter()
            .find(|gt| (gt.depth - 0.5).abs() < 0.1)
        {
            ground_temp.monthly_temps = Some(epw_gt.monthly_temps);
            ground_temp.depth = epw_gt.depth;
            info!(
                "Ground temp: using EPW 0.5m monthly temps (Jan={:.1}°C, Jul={:.1}°C, mean={:.1}°C)",
                epw_gt.monthly_temps[0],
                epw_gt.monthly_temps[6],
                epw_gt.monthly_temps.iter().sum::<f64>() / 12.0,
            );
        } else {
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
    let loop_infos = build_loop_infos(&model, &resolved_zones_for_oa);
    for li in &loop_infos {
        info!(
            "Air loop '{}': type={:?}, zones=[{}], OA={:.0}%",
            li.name,
            li.system_type,
            li.served_zones.join(", "),
            li.min_oa_fraction * 100.0,
        );
    }

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
        for zc in &al.zones {
            zone_design_flows.insert(zc.zone.clone(), flow);
        }
    }

    // ── Design Day Sizing Run ──────────────────────────────────────────
    // Two-stage ASHRAE-compliant sizing:
    //   Stage 1: Zone sizing — peak loads per zone from ALL design days
    //   Stage 2: System sizing — coincident peak system loads
    if !model.design_days.is_empty() {
        if let Some(ref mut env) = envelope {
            let latitude = weather_data.location.latitude;
            let sizing_result = openbse_io::sizing::run_sizing(
                env,
                &model.design_days,
                &resolved_thermostats,
                latitude,
                &weather_data.hours,
                output_dir,
                &input_stem,
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
                        let zone_airflow: f64 = li.served_zones.iter()
                            .map(|z| sizing_result.zone_design_airflow.get(z).copied().unwrap_or(0.1))
                            .sum();
                        let zone_flow_m3 = zone_airflow / air_density;
                        let zone_heat: f64 = li.served_zones.iter()
                            .map(|z| sizing_result.zone_peak_heating.get(z).copied().unwrap_or(0.0))
                            .sum::<f64>() * 1.25;
                        let zone_cool: f64 = li.served_zones.iter()
                            .map(|z| sizing_result.zone_peak_cooling.get(z).copied().unwrap_or(0.0))
                            .sum::<f64>() * 1.25;
                        (zone_flow_m3, zone_heat, zone_cool)
                    }
                    AirLoopSystemType::Vav => {
                        // VAV: multi-zone system uses system-wide coincident peak sizing
                        (sizing_result.system_volume_flow,
                         sizing_result.system_heating_capacity,
                         sizing_result.system_cooling_capacity)
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
                            .map(|z| sizing_result.zone_design_airflow.get(z).copied().unwrap_or(0.1))
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
                        let doas_heat_cap = (oa_flow_kg * cp_air * (t_supply_heat - t_outdoor_heat).max(0.0)) * 1.25;
                        let doas_cool_cap = (oa_flow_kg * cp_air * (t_outdoor_cool - t_supply_cool).max(0.0)) * 1.25;

                        (oa_flow_m3, doas_heat_cap, doas_cool_cap)
                    }
                    AirLoopSystemType::Fcu => {
                        // FCU: sized to its served zone(s) only
                        let zone_airflow: f64 = li.served_zones.iter()
                            .map(|z| sizing_result.zone_design_airflow.get(z).copied().unwrap_or(0.1))
                            .sum();
                        let zone_flow_m3 = zone_airflow / air_density;
                        let zone_heat: f64 = li.served_zones.iter()
                            .map(|z| sizing_result.zone_peak_heating.get(z).copied().unwrap_or(0.0))
                            .sum::<f64>() * 1.25;
                        let zone_cool: f64 = li.served_zones.iter()
                            .map(|z| sizing_result.zone_peak_cooling.get(z).copied().unwrap_or(0.0))
                            .sum::<f64>() * 1.25;
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

                let (loop_flow, loop_heat, loop_cool) = loop_comp_sizing.get(&name)
                    .copied()
                    .unwrap_or((sizing_result.system_volume_flow,
                                sizing_result.system_heating_capacity,
                                sizing_result.system_cooling_capacity));

                // Autosize fan flow rate
                if let Some(_flow) = comp.design_air_flow_rate() {
                    // Fan has a non-autosize value — skip
                } else {
                    comp.set_design_air_flow_rate(loop_flow);
                    info!("Autosized '{}' flow rate: {:.4} m³/s", name, loop_flow);
                }

                // Autosize coil capacities
                if lname.contains("heat") || lname.contains("furnace")
                    || lname.contains("preheat") || lname.contains("reheat") {
                    if let Some(cap) = comp.nominal_capacity() {
                        if is_autosize(cap) {
                            comp.set_nominal_capacity(loop_heat);
                            info!("Autosized '{}' capacity: {:.0} W ({:.1} kW)",
                                name, loop_heat, loop_heat / 1000.0);
                        }
                    }
                }
                if lname.contains("cool") || lname.contains("dx") {
                    if let Some(cap) = comp.nominal_capacity() {
                        if is_autosize(cap) {
                            comp.set_nominal_capacity(loop_cool);
                            info!("Autosized '{}' capacity: {:.0} W ({:.1} kW)",
                                name, loop_cool, loop_cool / 1000.0);
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
        Some(SummaryReport::new(
            zone_heating_setpoints.clone(),
            zone_cooling_setpoints.clone(),
        ))
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

                    // Ideal loads at setpoint from previous timestep (used for load-based PLR).
                    // These represent the HVAC energy needed to hold the zone at the setpoint
                    // temperature, computed by compute_ideal_q_hvac in the envelope solver.
                    let mut current_cooling_loads: HashMap<String, f64> = env.zones.iter()
                        .map(|z| (z.input.name.clone(), z.ideal_cooling_load))
                        .collect();
                    let mut current_heating_loads: HashMap<String, f64> = env.zones.iter()
                        .map(|z| (z.input.name.clone(), z.ideal_heating_load))
                        .collect();

                    let mut final_hvac_result = None;
                    let mut final_env_result = None;

                    for _hvac_iter in 0..MAX_HVAC_ITER {
                        // Step 1: Run HVAC with current zone temps and loads
                        let (hvac_result, zone_supply_conditions) = simulate_all_loops(
                            &mut graph,
                            &ctx,
                            &loop_infos,
                            &current_zone_temps,
                            &zone_heating_setpoints,
                            &zone_cooling_setpoints,
                            &zone_unocc_heating_setpoints,
                            &zone_unocc_cooling_setpoints,
                            &zone_design_flows,
                            t_outdoor,
                            Some(&env.schedule_manager),
                            hour,
                            dow,
                            &mut nightcycle_timers,
                            dt,
                            &current_cooling_loads,
                            &current_heating_loads,
                        );

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

                        // Update zone temps and loads for next iteration.
                        // Use IDEAL loads at setpoint for PLR calculation —
                        // these represent what the HVAC must deliver to hold
                        // the zone at the setpoint temperature.
                        current_zone_temps = env_result.zone_temps.iter()
                            .map(|(k, &v)| (k.clone(), v))
                            .collect();
                        current_cooling_loads = env_result.ideal_cooling_loads.iter()
                            .map(|(k, &v)| (k.clone(), v))
                            .collect();
                        current_heating_loads = env_result.ideal_heating_loads.iter()
                            .map(|(k, &v)| (k.clone(), v))
                            .collect();

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
                    snapshot.zone_temperature.insert(name.clone(), zone.temp);
                    snapshot.zone_humidity_ratio.insert(name.clone(), zone.humidity_ratio);
                    snapshot.zone_heating_rate.insert(name.clone(), zone.heating_load);
                    snapshot.zone_cooling_rate.insert(name.clone(), zone.cooling_load);
                    snapshot.zone_infiltration_mass_flow.insert(name.clone(), zone.infiltration_mass_flow);
                    snapshot.zone_internal_gains_convective.insert(name.clone(), zone.q_internal_conv);
                    snapshot.zone_internal_gains_radiative.insert(name.clone(), zone.q_internal_rad);
                    snapshot.zone_supply_air_temperature.insert(name.clone(), zone.supply_air_temp);
                    snapshot.zone_supply_air_mass_flow.insert(name.clone(), zone.supply_air_mass_flow);

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
                    snapshot.surface_incident_solar.insert(name.clone(), surface.incident_solar);
                    snapshot.surface_transmitted_solar.insert(name.clone(), surface.transmitted_solar);
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

                // Populate energy end-use data
                for (comp_name, vars) in &result.component_outputs {
                    if comp_name == "Weather" { continue; }
                    if let Some(&pw) = vars.get("electric_power") {
                        snapshot.component_electric_power.insert(comp_name.clone(), pw);
                    }
                    if let Some(&pw) = vars.get("fuel_power") {
                        snapshot.component_fuel_power.insert(comp_name.clone(), pw);
                    }
                }
                // Zone internal gains — separate lighting and equipment energy
                for zone in &env.zones {
                    snapshot.zone_lighting_power.insert(zone.input.name.clone(), zone.lighting_power);
                    snapshot.zone_equipment_power.insert(zone.input.name.clone(), zone.equipment_power);
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

                // Populate energy end-use data
                for (comp_name, vars) in &result.component_outputs {
                    if comp_name == "Weather" { continue; }
                    if let Some(&pw) = vars.get("electric_power") {
                        snapshot.component_electric_power.insert(comp_name.clone(), pw);
                    }
                    if let Some(&pw) = vars.get("fuel_power") {
                        snapshot.component_fuel_power.insert(comp_name.clone(), pw);
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
    t_outdoor: f64,
    schedule_mgr: Option<&ScheduleManager>,
    hour: u32,
    day_of_week: u32,
    nightcycle_timers: &mut HashMap<String, f64>,
    dt: f64,
    zone_cooling_loads: &HashMap<String, f64>,
    zone_heating_loads: &HashMap<String, f64>,
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
        let nightcycle_duty = 1.0_f64; // 1.0 = full operation
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

        // Select active setpoints based on occupied/unoccupied state
        let active_heat_sp = if is_unoccupied { zone_unocc_heat_sp } else { zone_heat_sp };
        let active_cool_sp = if is_unoccupied { zone_unocc_cool_sp } else { zone_cool_sp };

        let signals = match li.system_type {
            // ──────────────────────────────────────────────────────────────
            // PSZ-AC: single-zone thermostat, mixed return + outdoor air.
            // The control zone is the first served zone.
            // ──────────────────────────────────────────────────────────────
            AirLoopSystemType::PszAc => {
                build_psz_signals(li, zone_temps, active_heat_sp, active_cool_sp,
                    zone_design_flows, t_outdoor, zone_cooling_loads, zone_heating_loads)
            }

            // ──────────────────────────────────────────────────────────────
            // DOAS: 100% outdoor air, fixed supply setpoints, always runs.
            // Pre-conditions ventilation air; no zone-temperature feedback.
            // ──────────────────────────────────────────────────────────────
            AirLoopSystemType::Doas => {
                build_doas_signals(li, zone_design_flows, active_heat_sp, active_cool_sp, t_outdoor)
            }

            // ──────────────────────────────────────────────────────────────
            // FCU: recirculating fan coil unit, per-zone thermostat.
            // Each FCU loop serves exactly one zone.
            // ──────────────────────────────────────────────────────────────
            AirLoopSystemType::Fcu => {
                build_fcu_signals(li, zone_temps, active_heat_sp, active_cool_sp,
                    zone_design_flows, t_outdoor)
            }

            // ──────────────────────────────────────────────────────────────
            // VAV: central cold-deck AHU, per-zone airflow modulation.
            // All zones get cold supply air; zone-level reheat is handled
            // by separate FCU-type loops defined in the YAML.
            // ──────────────────────────────────────────────────────────────
            AirLoopSystemType::Vav => {
                build_vav_signals(li, zone_temps, active_heat_sp, active_cool_sp,
                    zone_design_flows, t_outdoor)
            }
        };

        // Run this loop's components in order (at full capacity, PLR=1.0)
        let (mut loop_result, supply_air) = simulate_loop_components(
            graph, ctx, &li.component_names, &signals
        );

        // ── Load-Based PLR for PSZ-AC ON/OFF Fan Cycling ──
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
        let loop_plr = if li.system_type == AirLoopSystemType::PszAc {
            let control_zone = li.served_zones.first().map(|s| s.as_str()).unwrap_or("");
            let zone_cool_load = zone_cooling_loads.get(control_zone).copied().unwrap_or(0.0);
            let zone_heat_load = zone_heating_loads.get(control_zone).copied().unwrap_or(0.0);
            let control_temp = zone_temps.get(control_zone).copied().unwrap_or(21.0);
            let heat_sp = active_heat_sp.get(control_zone).copied().unwrap_or(21.1);
            let cool_sp = active_cool_sp.get(control_zone).copied().unwrap_or(23.9);
            let mode = hvac_mode(control_temp, heat_sp, cool_sp);

            let cp_air = 1006.0_f64; // J/(kg·K)

            if let Some(ref supply) = supply_air {
                let supply_temp = supply.state.t_db;
                let supply_flow = supply.mass_flow;

                // Compute net capacity using the SETPOINT as the reference
                // return air temperature. This ensures the capacity calculation
                // is consistent with the ideal load (also computed at setpoint).
                //
                // Using the actual zone temp would cause instability: at temps
                // far from setpoint, the ΔT becomes extreme, capacity becomes
                // huge, PLR becomes tiny, and the zone stabilizes at the wrong
                // temperature. Using setpoint ensures the system correctly
                // drives the zone toward setpoint.
                //
                // Small residual offset (~0.1°C) arises because the coil was
                // simulated at the actual inlet temp, not setpoint. This is
                // acceptable and diminishes with sub-hourly timesteps.
                let q_cool_capacity = supply_flow * cp_air * (cool_sp - supply_temp);
                let q_heat_capacity = supply_flow * cp_air * (supply_temp - heat_sp);

                // Load-based PLR considers both the thermostat mode AND the zone loads.
                //
                // Key insight: Even in "deadband" (zone between heat_sp and cool_sp),
                // the zone may still need cooling/heating to stay there. For example,
                // CE100 has 5400W internal gains — even at setpoint, the system must
                // run ~93% of the time to maintain temperature.
                //
                // The PLR is determined by the zone load, not the thermostat mode.
                // The mode just tells us which direction to condition.
                if zone_cool_load > 0.0 && q_cool_capacity > 100.0 {
                    // Zone has a cooling load — deliver cooling proportional to load
                    (zone_cool_load / q_cool_capacity).clamp(0.0, 1.0)
                } else if zone_heat_load > 0.0 && q_heat_capacity > 100.0 {
                    // Zone has a heating load — deliver heating proportional to load
                    (zone_heat_load / q_heat_capacity).clamp(0.0, 1.0)
                } else if mode == HvacMode::Cooling {
                    // Zone needs cooling but load is 0 or capacity is negligible.
                    // This happens during startup or when thermal mass is absorbing.
                    // Run at full capacity to pull zone toward setpoint.
                    1.0
                } else if mode == HvacMode::Heating {
                    // Zone needs heating but load is 0 — run at full capacity.
                    1.0
                } else {
                    // True deadband — no load, zone between setpoints
                    li.min_oa_fraction
                }
            } else {
                li.min_oa_fraction
            }
        } else {
            // Non-PSZ-AC systems: no PLR cycling (they modulate internally)
            signals.coil_setpoints.get("__plr__")
                .copied()
                .unwrap_or(1.0)
        } * nightcycle_duty;

        if loop_plr < 1.0 {
            for (_comp_name, outputs) in &mut loop_result {
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

        // Store PLR for reporting
        all_outputs.entry("__loop_plr__".to_string())
            .or_default()
            .insert(li.name.clone(), loop_plr);

        // Collect outputs
        for (k, v) in loop_result {
            all_outputs.insert(k, v);
        }

        // Distribute supply air to served zones
        if let Some(supply) = supply_air {
            let supply_temp = supply.state.t_db;
            // Apply PLR to flow distribution (zone receives time-averaged flow)
            let effective_flow = supply.mass_flow * loop_plr;

            for zone_name in &li.served_zones {
                // How much flow does this zone get from this loop?
                let zone_flow = match li.system_type {
                    AirLoopSystemType::PszAc => {
                        // Split total flow proportionally among served zones
                        let n = li.served_zones.len().max(1) as f64;
                        effective_flow / n
                    }
                    AirLoopSystemType::Doas => {
                        // Ventilation flow = design ventilation per zone
                        // Use a per-zone OA flow: design_zone_flow * min_oa_fraction
                        // (simplified: equal share of total DOAS flow)
                        let n = li.served_zones.len().max(1) as f64;
                        effective_flow / n
                    }
                    AirLoopSystemType::Fcu => {
                        // Single zone: all flow goes here
                        effective_flow
                    }
                    AirLoopSystemType::Vav => {
                        // Flow was modulated per zone; read from zone_air_flows
                        signals.zone_air_flows.get(zone_name)
                            .copied()
                            .unwrap_or(effective_flow / li.served_zones.len().max(1) as f64)
                            * loop_plr
                    }
                };

                zone_supply.entry(zone_name.clone())
                    .or_default()
                    .push((supply_temp, zone_flow));
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
) -> ControlSignals {
    let mut signals = ControlSignals::default();

    // Control zone = first served zone
    let control_zone = li.served_zones.first().map(|s| s.as_str()).unwrap_or("");
    let control_temp = zone_temps.get(control_zone).copied().unwrap_or(21.0);
    let heat_sp = zone_heat_sp.get(control_zone).copied().unwrap_or(21.1);
    let cool_sp = zone_cool_sp.get(control_zone).copied().unwrap_or(23.9);
    let zone_cool_load = zone_cooling_loads.get(control_zone).copied().unwrap_or(0.0);
    let zone_heat_load = zone_heating_loads.get(control_zone).copied().unwrap_or(0.0);

    // Determine HVAC mode from both temperature AND loads.
    //
    // Temperature-only mode misses the case where the zone is right at setpoint
    // but still has a net load (e.g. 5400W internal gain at 22.2°C). The zone
    // is "in deadband" by temperature but needs cooling to stay there.
    //
    // Load-informed mode: if the zone has a cooling load, enable cooling coil
    // even if the zone temp is at/below the setpoint (to prevent overshoot).
    let mode = if control_temp > cool_sp || zone_cool_load > 100.0 {
        HvacMode::Cooling
    } else if control_temp < heat_sp || zone_heat_load > 100.0 {
        HvacMode::Heating
    } else {
        HvacMode::Deadband
    };

    // Total design flow
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
                needed.clamp(li.min_oa_fraction, 1.0)
            } else {
                li.min_oa_fraction
            }
        }
        _ => li.min_oa_fraction,
    };
    let mixed_air_temp = return_air_temp * (1.0 - oa_frac) + t_outdoor * oa_frac;

    for name in &li.component_names {
        let lname = name.to_lowercase();
        match mode {
            HvacMode::Heating => {
                // Proportional heating DAT: ramps from setpoint toward max (40°C)
                // based on zone heating error. At small errors, furnace delivers
                // warm but not hot air; at large errors, full-fire to recover.
                if lname.contains("heat") || lname.contains("furnace") {
                    signals.coil_setpoints.insert(name.clone(), heating_dat);
                } else if lname.contains("cool") || lname.contains("dx") {
                    signals.coil_setpoints.insert(name.clone(), 99.0);
                }
            }
            HvacMode::Cooling => {
                // DX coil runs at full capacity when ON (PLR controls runtime).
                // The coil setpoint is set very low so capacity is the limiter.
                if lname.contains("cool") || lname.contains("dx") {
                    signals.coil_setpoints.insert(name.clone(), cooling_coil_sp);
                } else if lname.contains("heat") || lname.contains("furnace") {
                    signals.coil_setpoints.insert(name.clone(), -99.0);
                }
            }
            HvacMode::Deadband => {
                if lname.contains("heat") || lname.contains("furnace") {
                    signals.coil_setpoints.insert(name.clone(), -99.0);
                } else if lname.contains("cool") || lname.contains("dx") {
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
    zone_heat_sp: &HashMap<String, f64>,
    zone_cool_sp: &HashMap<String, f64>,
    t_outdoor: f64,
) -> ControlSignals {
    let mut signals = ControlSignals::default();

    // Total ventilation airflow = 30% of zone design flows (typical OA fraction)
    let vent_flow_total: f64 = li.served_zones.iter()
        .map(|z| zone_design_flows.get(z).copied().unwrap_or(0.1))
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
        if lname.contains("heat") || lname.contains("preheat") {
            // Fire only if OA is below heating target
            if t_outdoor < t_supply_heat {
                signals.coil_setpoints.insert(name.clone(), t_supply_heat);
            } else {
                signals.coil_setpoints.insert(name.clone(), -99.0);  // off
            }
        } else if lname.contains("cool") || lname.contains("dx") {
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
    _t_outdoor: f64,
) -> ControlSignals {
    let mut signals = ControlSignals::default();

    // FCU serves one zone (its name is the zone)
    let zone_name = li.served_zones.first().map(|s| s.as_str()).unwrap_or("");
    let zone_temp = zone_temps.get(zone_name).copied().unwrap_or(21.0);
    let heat_sp = zone_heat_sp.get(zone_name).copied().unwrap_or(21.1);
    let cool_sp = zone_cool_sp.get(zone_name).copied().unwrap_or(23.9);

    let design_flow = zone_design_flows.get(zone_name).copied().unwrap_or(0.3);

    let mode = hvac_mode(zone_temp, heat_sp, cool_sp);

    // FCU modulates fan speed: deadband = 30%, heating/cooling = proportional
    let flow = match mode {
        HvacMode::Deadband => {
            // Fan off (minimum)
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
    };

    for name in &li.component_names {
        let lname = name.to_lowercase();
        match mode {
            HvacMode::Heating => {
                let error = heat_sp - zone_temp;
                let target = (heat_sp + error.min(14.0)).clamp(heat_sp, 45.0);
                if lname.contains("heat") || lname.contains("reheat") {
                    signals.coil_setpoints.insert(name.clone(), target);
                } else if lname.contains("cool") || lname.contains("dx") {
                    signals.coil_setpoints.insert(name.clone(), 99.0);
                }
            }
            HvacMode::Cooling => {
                let error = zone_temp - cool_sp;
                let target = (cool_sp - error.min(10.0)).clamp(12.0, cool_sp);
                if lname.contains("cool") || lname.contains("dx") {
                    signals.coil_setpoints.insert(name.clone(), target);
                } else if lname.contains("heat") || lname.contains("reheat") {
                    signals.coil_setpoints.insert(name.clone(), -99.0);
                }
            }
            HvacMode::Deadband => {
                if lname.contains("heat") || lname.contains("reheat") {
                    signals.coil_setpoints.insert(name.clone(), -99.0);
                } else if lname.contains("cool") || lname.contains("dx") {
                    signals.coil_setpoints.insert(name.clone(), 99.0);
                }
            }
        }
        signals.air_mass_flows.insert(name.clone(), flow);
    }

    // FCU uses recirculated zone air (no OA mixing): set inlet override
    signals.coil_setpoints.insert(
        "__fcu_recirculation_temp__".to_string(),
        zone_temp,
    );
    // FCU is 100% recirculated — OA fraction = 0
    signals.coil_setpoints.insert(
        "__oa_fraction__".to_string(),
        0.0,
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
    t_outdoor: f64,
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

        // Store per-zone flow in signals
        signals.zone_air_flows.insert(zone_name.clone(), zone_flow);
        total_flow += zone_flow;
    }
    total_flow = total_flow.max(0.05);

    // ── SAT Reset (ASHRAE G36 §5.16) ──
    // Reset AHU supply air temperature based on cooling demand:
    //   max_cooling_demand = 1.0 → SAT = 13°C (full cooling)
    //   max_cooling_demand = 0.0 → SAT = 18°C (reset up, save energy)
    let sat_min = 13.0_f64;  // full cooling SAT
    let sat_max = 18.0_f64;  // reset SAT (mild weather)
    let sat_setpoint = sat_max - (sat_max - sat_min) * max_cooling_demand.clamp(0.0, 1.0);

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
            needed.clamp(li.min_oa_fraction, 1.0)
        } else {
            li.min_oa_fraction
        }
    } else {
        li.min_oa_fraction
    };
    let mixed_air_temp = avg_zone_temp * (1.0 - oa_frac) + t_outdoor * oa_frac;

    // ── AHU coil control ──
    for name in &li.component_names {
        let lname = name.to_lowercase();
        if lname.contains("cool") || lname.contains("dx") {
            if any_cooling {
                // AHU cooling coil targets the SAT setpoint
                signals.coil_setpoints.insert(name.clone(), sat_setpoint);
            } else {
                // No cooling demand — coil off
                signals.coil_setpoints.insert(name.clone(), 99.0);
            }
        } else if lname.contains("preheat") {
            // AHU preheat: frost protection when mixed air is cold
            if mixed_air_temp < 4.0 {
                signals.coil_setpoints.insert(name.clone(), 4.5);
            } else {
                signals.coil_setpoints.insert(name.clone(), -99.0);
            }
        } else if lname.contains("heat") {
            // AHU heating coil: when majority of zones need heating and no cooling demand,
            // heat the supply air toward the SAT reset point (up to 18°C) to provide warm deck
            let zones_needing_heat = li.served_zones.iter().filter(|z| {
                let zt = zone_temps.get(*z).copied().unwrap_or(21.0);
                let hs = zone_heat_sp.get(*z).copied().unwrap_or(21.1);
                zt < hs
            }).count();
            let mostly_heating = zones_needing_heat > li.served_zones.len() / 2;

            if mostly_heating && !any_cooling && mixed_air_temp < sat_max {
                // Warm deck: heat to SAT reset max (18°C) to assist zone reheat
                signals.coil_setpoints.insert(name.clone(), sat_max);
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
        let w_mixed = oa_fraction * ctx.outdoor_air.w + (1.0 - oa_fraction) * ctx.outdoor_air.w;
        // Note: ideally w_indoor should come from zone humidity ratio, but outdoor w is used
        // as a reasonable approximation since the zone humidity tracks close to outdoor in
        // buildings without active humidity control.
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
