//! Autosizing module — two-stage ASHRAE-compliant sizing from design day loads.
//!
//! ## Stage 1: Zone Sizing
//! For each design day (ALL provided, not just the first):
//!   1. Generate synthetic weather for 24 hours
//!   2. Run envelope simulation for warmup days to reach quasi-steady-state
//!   3. Record peak heating/cooling loads per zone per timestep
//!   4. Take maximum across all design days of the same type
//!
//! Results: peak zone heating/cooling loads, zone design airflows.
//! These are used to size zone-level equipment (VAV boxes, fan coils, etc.).
//!
//! ## Stage 2: System Sizing
//! With zone equipment hard-sized from Stage 1:
//!   1. Re-run each design day with zone airflows set to design values
//!   2. At each timestep, sum all zone loads (coincident peak)
//!   3. System capacity = max coincident sum across all hours and design days
//!
//! Results: system heating/cooling capacity, system airflow.
//! These are used to size AHU coils, fans, and central plant equipment.
//!
//! Reference: ASHRAE Handbook — Fundamentals, Chapter 18 (Nonresidential
//! Cooling and Heating Load Calculations).

use crate::input::DesignDayHumidityType;
use openbse_core::ports::{EnvelopeSolver, SimulationContext, ZoneHvacConditions};
use openbse_core::types::{DayType, TimeStep};
use openbse_envelope::BuildingEnvelope;
use openbse_psychrometrics as psych;
use openbse_weather::WeatherHour;
use std::collections::HashMap;
use std::path::Path;

use crate::input::DesignDayInput;
use openbse_envelope::ThermostatInput;

/// Zone-level sizing results from Stage 1.
#[derive(Debug, Clone, Default)]
pub struct ZoneSizingResult {
    /// Peak heating load per zone [W]
    pub zone_peak_heating: HashMap<String, f64>,
    /// Peak cooling load per zone [W]
    pub zone_peak_cooling: HashMap<String, f64>,
    /// Design day name where heating peak occurred per zone
    pub zone_heating_dd: HashMap<String, String>,
    /// Design day name where cooling peak occurred per zone
    pub zone_cooling_dd: HashMap<String, String>,
    /// Hour when heating peak occurred per zone
    pub zone_heating_peak_hour: HashMap<String, u32>,
    /// Hour when cooling peak occurred per zone
    pub zone_cooling_peak_hour: HashMap<String, u32>,
    /// Design heating airflow per zone [kg/s]
    pub zone_heating_airflow: HashMap<String, f64>,
    /// Design cooling airflow per zone [kg/s]
    pub zone_cooling_airflow: HashMap<String, f64>,
    /// Design airflow per zone (max of heating and cooling) [kg/s]
    pub zone_design_airflow: HashMap<String, f64>,
    /// Zone cooling airflow [kg/s] at each hour of each cooling design day
    /// (E+ CoolFlowSeq), for coincident system sizing.
    pub zone_cooling_flow_sequences: Vec<Vec<HashMap<String, f64>>>,
}

/// System-level sizing results from Stage 2.
#[derive(Debug, Clone)]
pub struct SystemSizingResult {
    /// Coincident peak heating load [W] (sum of zone loads at same timestep)
    pub coincident_peak_heating: f64,
    /// Coincident peak cooling load [W]
    pub coincident_peak_cooling: f64,
    /// Design day where system heating peak occurred
    pub heating_peak_dd: String,
    /// Design day where system cooling peak occurred
    pub cooling_peak_dd: String,
    /// Hour when system heating peak occurred
    pub heating_peak_hour: u32,
    /// Hour when system cooling peak occurred
    pub cooling_peak_hour: u32,
}

/// Combined sizing results from both stages.
#[derive(Debug, Clone)]
pub struct SizingResult {
    /// Zone-level sizing (Stage 1)
    pub zone_sizing: ZoneSizingResult,
    /// System-level sizing (Stage 2)
    pub system_sizing: SystemSizingResult,

    /// Peak heating load per zone [W] (convenience alias)
    pub zone_peak_heating: HashMap<String, f64>,
    /// Peak cooling load per zone [W] (convenience alias)
    pub zone_peak_cooling: HashMap<String, f64>,
    /// Design heating airflow per zone [kg/s]
    pub zone_heating_airflow: HashMap<String, f64>,
    /// Design cooling airflow per zone [kg/s]
    pub zone_cooling_airflow: HashMap<String, f64>,
    /// Design airflow per zone (max of heating and cooling) [kg/s]
    pub zone_design_airflow: HashMap<String, f64>,
    /// Total system heating capacity [W] (with sizing factor)
    pub system_heating_capacity: f64,
    /// Total system cooling capacity [W] (with sizing factor)
    pub system_cooling_capacity: f64,
    /// Total system airflow [kg/s]
    pub system_airflow: f64,
    /// Total system volume flow [m³/s]
    pub system_volume_flow: f64,
}

/// ASHRAE fraction of the daily range below the maximum for hours 1–24
/// (*Fundamentals* 2009 Ch. 14 Table 6), E+ `DefaultTempRangeMult`.
const DAILY_RANGE_FRACTION: [f64; 24] = [
    0.88, 0.92, 0.95, 0.98, 1.00, 0.98, 0.91, 0.74, 0.55, 0.38, 0.23, 0.13, 0.05, 0.00, 0.00, 0.06,
    0.14, 0.24, 0.39, 0.50, 0.59, 0.68, 0.75, 0.82,
];

/// The design day's dry-bulb at the end of hour `h` (1–24).
fn design_dry_bulb(dd: &DesignDayInput, h: u32) -> f64 {
    dd.design_temp - dd.daily_range * DAILY_RANGE_FRACTION[(h as usize - 1) % 24]
}

/// Set the moisture fields of a design-day record from its dry-bulb, following
/// E+ `SetUpDesignDay`: every humidity type but relative humidity fixes one
/// humidity ratio for the day from the maximum dry-bulb, and the air saturates
/// if it cools below that ratio's dew point.
fn set_design_humidity(wh: &mut WeatherHour, dd: &DesignDayInput) {
    let p = dd.pressure;
    let w = match dd.humidity_type {
        DesignDayHumidityType::Wetbulb => {
            psych::w_fn_tdb_twb_pb(dd.design_temp, dd.humidity_value.min(dd.design_temp), p)
        }
        DesignDayHumidityType::Dewpoint => psych::w_fn_tdp_pb(dd.humidity_value, p),
        DesignDayHumidityType::HumidityRatio => dd.humidity_value,
        DesignDayHumidityType::Enthalpy => psych::w_fn_tdb_h(dd.design_temp, dd.humidity_value),
        DesignDayHumidityType::RelativeHumidity => {
            psych::w_fn_tdb_rh_pb(wh.dry_bulb, dd.humidity_value / 100.0, p)
        }
    };
    let w = w.min(psych::w_fn_tdb_rh_pb(wh.dry_bulb, 1.0, p));
    wh.rel_humidity = 100.0 * psych::rh_fn_tdb_w_pb(wh.dry_bulb, w, p);
    wh.dew_point = psych::tdp_fn_w_pb(w, p).min(wh.dry_bulb);
}

/// The weather for one sizing timestep: E+ interpolates a design day's hourly
/// dry-bulb profile within the hour, and recomputes humidity from the result.
/// The day repeats, so hour 1 follows hour 24.
fn design_step_weather(
    dd: &DesignDayInput,
    weather_hours: &[WeatherHour],
    hour_index: usize,
    sub: u32,
    steps: u32,
) -> WeatherHour {
    let n = weather_hours.len();
    let prev = &weather_hours[(hour_index + n - 1) % n];
    let next = &weather_hours[(hour_index + 1) % n];
    let mut wh = weather_hours[hour_index].at_timestep(prev, next, sub, steps);
    set_design_humidity(&mut wh, dd);
    wh
}

/// Generate 24 synthetic WeatherHour entries for a heating design day.
///
/// Heating design days have no solar radiation and the specified wind speed.
fn generate_heating_design_weather(dd: &DesignDayInput) -> Vec<WeatherHour> {
    let mut hours = Vec::with_capacity(24);
    for h in 1..=24u32 {
        let mut wh = WeatherHour {
            year: 2024,
            month: dd.month,
            day: dd.day,
            hour: h,
            dry_bulb: design_dry_bulb(dd, h),
            dew_point: 0.0,
            rel_humidity: 0.0,
            pressure: dd.pressure,
            global_horiz_rad: 0.0, // no solar for heating design
            direct_normal_rad: 0.0,
            diffuse_horiz_rad: 0.0,
            wind_speed: dd.wind_speed,
            wind_direction: 0.0,
            opaque_sky_cover: 10.0,
            horiz_ir_rad: 0.0, // missing: the sky comes from the emissivity model
        };
        set_design_humidity(&mut wh, dd);
        hours.push(wh);
    }
    hours
}

/// Generate 24 synthetic WeatherHour entries for a cooling design day.
///
/// Cooling design days use the ASHRAE daily-range profile below the design
/// temperature. Solar is estimated from a clear-sky model.
fn generate_cooling_design_weather(dd: &DesignDayInput, latitude: f64) -> Vec<WeatherHour> {
    let mut hours = Vec::with_capacity(24);

    for h in 1..=24u32 {
        // Simple clear-sky solar estimate
        let doy = {
            let days_in_months = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
            days_in_months[dd.month.min(12).saturating_sub(1) as usize] + dd.day
        };

        let declination = 23.45_f64.to_radians()
            * (2.0 * std::f64::consts::PI * (284 + doy) as f64 / 365.0).sin();
        let lat_rad = latitude.to_radians();
        let hour_angle_solar = (h as f64 - 12.5) * 15.0_f64.to_radians();
        let sin_altitude = lat_rad.sin() * declination.sin()
            + lat_rad.cos() * declination.cos() * hour_angle_solar.cos();
        let altitude = sin_altitude.max(0.0).asin();

        let (direct, diffuse) = if altitude > 0.01 {
            // Simple ASHRAE clear-sky model
            let air_mass = 1.0 / sin_altitude.max(0.05);
            let direct = 1080.0 * (-0.174 * air_mass).exp();
            let diffuse = 120.0 * altitude.sin();
            (direct, diffuse)
        } else {
            (0.0, 0.0)
        };

        let mut wh = WeatherHour {
            year: 2024,
            month: dd.month,
            day: dd.day,
            hour: h,
            dry_bulb: design_dry_bulb(dd, h),
            dew_point: 0.0,
            rel_humidity: 0.0,
            pressure: dd.pressure,
            global_horiz_rad: (direct * sin_altitude + diffuse).max(0.0),
            direct_normal_rad: direct.max(0.0),
            diffuse_horiz_rad: diffuse.max(0.0),
            wind_speed: dd.wind_speed,
            wind_direction: 0.0,
            opaque_sky_cover: 1.0, // clear sky for cooling design
            horiz_ir_rad: 0.0,     // missing: the sky comes from the emissivity model
        };
        set_design_humidity(&mut wh, dd);
        hours.push(wh);
    }
    hours
}

/// Classify a design day as heating or cooling from its day_type string.
fn is_heating_design_day(dd: &DesignDayInput) -> bool {
    let dt = dd.day_type.to_lowercase();
    dt.contains("winter") || dt.contains("heat")
}

fn is_cooling_design_day(dd: &DesignDayInput) -> bool {
    let dt = dd.day_type.to_lowercase();
    dt.contains("summer") || dt.contains("cool")
}

/// Zone setpoints held during sizing, from the thermostats.
#[derive(Debug, Default)]
struct SizingSetpoints {
    heating: HashMap<String, f64>,
    cooling: HashMap<String, f64>,
    /// Hourly design-day schedules, for the thermostats that give them.
    dd_heating: HashMap<String, Vec<f64>>,
    dd_cooling: HashMap<String, Vec<f64>>,
}

/// How each design day is stepped through.
#[derive(Debug, Clone, Copy)]
struct SizingSteps {
    /// Days repeated so the envelope reaches a periodic state; loads come
    /// from the last one.
    warmup_days: usize,
    per_hour: u32,
    /// Timesteps the loads are averaged over before their peak is taken.
    averaging_window: usize,
}

/// Run a single design day through the envelope with ideal HVAC holding
/// zones at their setpoint temperature, returning per-zone loads at each hour.
///
/// Returns: Vec of (hour, HashMap<zone_name, (heating_load, cooling_load)>)
fn run_single_design_day(
    env: &mut BuildingEnvelope,
    dd: &DesignDayInput,
    weather_hours: &[WeatherHour],
    zone_setpoints: &HashMap<String, f64>, // setpoint to hold zone at
    zone_hourly_setpoints: &HashMap<String, Vec<f64>>, // per-zone hourly override
    oa_handled_by_hvac: &HashMap<String, bool>,
    steps: SizingSteps,
) -> StepLoads {
    let is_heating_dd = is_heating_design_day(dd);

    // Internal gains mode: use the design day's explicit setting, or default
    // based on day_type (heating → Off, cooling → Full).
    //
    // Full mode: all schedules at 1.0 (constant).  With constant gains and
    // thermal mass, the zone reaches steady state where cooling load = total
    // gains.  This is conservative for cooling sizing.
    //
    // E+ uses SummerDesignDay schedule profiles (typically constant 1.0 for
    // lights/equipment, low for occupancy).  For OpenBSE, Full mode is the
    // closest match since we don't have design-day-specific schedule values.
    use openbse_core::ports::SizingInternalGains;
    let gains_mode = dd.internal_gains.unwrap_or({
        if is_heating_dd {
            SizingInternalGains::Off
        } else {
            SizingInternalGains::Full
        }
    });

    // ── Enable ideal loads on all conditioned zones during sizing ─────────
    //
    // The ideal loads approach computes the exact Q_HVAC needed to hold each
    // zone at its setpoint, matching E+'s sizing methodology. The previous
    // approach (massive supply flow at zone setpoint temperature) only captured
    // ~88% of the true load because:
    //   T_zone = (numerator + MCPSYS × T_sp) / (denom + MCPSYS)
    // With finite MCPSYS, T_zone ≠ T_sp exactly, and calc_zone_loads reports
    // Q = MCPSYS × (T_zone − T_sp), which is less than the true ideal load.
    //
    // Save the original ideal_loads state so we can restore it after sizing.
    use openbse_envelope::zone::IdealLoadsAirSystem;

    let original_ideal_loads: Vec<Option<IdealLoadsAirSystem>> = env
        .zones
        .iter()
        .map(|z| z.input.ideal_loads.clone())
        .collect();

    for zone in &mut env.zones {
        if zone.input.conditioned && zone.input.ideal_loads.is_none() {
            let sp = zone_setpoints
                .get(&zone.input.name)
                .copied()
                .unwrap_or(21.0);
            zone.input.ideal_loads = Some(IdealLoadsAirSystem {
                heating_capacity: 1_000_000.0,
                cooling_capacity: 1_000_000.0,
                heating_setpoint: sp,
                cooling_setpoint: sp,
            });
        }
    }

    // Reset zone temperatures, humidity, and BDF state to setpoint
    for zone in &mut env.zones {
        let sp = zone_setpoints
            .get(&zone.input.name)
            .copied()
            .unwrap_or(21.0);
        zone.temp = sp;
        zone.temp_prev = sp;
        zone.temp_prev2 = sp;
        zone.temp_prev3 = sp;
        zone.temp_order = 0;
        // Reset humidity BDF state to initial value
        zone.humidity_ratio = 0.008;
        zone.w_prev = 0.008;
        zone.w_prev2 = 0.008;
        zone.w_prev3 = 0.008;
        zone.w_order = 0;
    }

    // The last day's loads at every timestep, for the moving average.
    let mut last_day_steps: StepLoads = Vec::new();
    let tph = steps.per_hour.max(1);
    let dt = 3600.0 / tph as f64;

    for day_num in 0..steps.warmup_days {
        let is_last_day = day_num == steps.warmup_days - 1;

        for i in 0..weather_hours.len() {
            let hour = (i + 1) as u32;

            // ── Update zone setpoints from design-day hourly schedule ─────
            //
            // E+ DOE prototypes use SummerDesignDay setpoint schedules that
            // raise the cooling setpoint during unoccupied hours (thermostat
            // setup), then recover at occupancy start.  The pulldown spike
            // when recovering from 27°C → 24°C drives equipment sizing.
            //
            // If the design day has a cooling_setpoint_schedule, update each
            // zone's ideal loads setpoint for this hour.  Otherwise the
            // constant thermostat setpoint is used (set once above).
            if let Some(ref sched) = dd.cooling_setpoint_schedule {
                if sched.len() == 24 {
                    let sp_hour = sched[(hour as usize - 1).min(23)];
                    for zone in &mut env.zones {
                        if let Some(ref mut il) = zone.input.ideal_loads {
                            il.cooling_setpoint = sp_hour;
                            // For cooling DD: heating setpoint stays low so we
                            // only size cooling, not inadvertent heating.
                            il.heating_setpoint = sp_hour.min(il.heating_setpoint);
                        }
                    }
                }
            }
            // A thermostat's own design-day schedule (E+ Summer/WinterDesignDay
            // setpoints) holds its zones at that hour's setpoint instead.
            for zone in &mut env.zones {
                let Some(sched) = zone_hourly_setpoints.get(&zone.input.name) else {
                    continue;
                };
                let sp_hour = sched[(hour as usize - 1).min(sched.len() - 1)];
                if let Some(ref mut il) = zone.input.ideal_loads {
                    if is_heating_dd {
                        il.heating_setpoint = sp_hour;
                        il.cooling_setpoint = il.cooling_setpoint.max(sp_hour);
                    } else {
                        il.cooling_setpoint = sp_hour;
                        il.heating_setpoint = il.heating_setpoint.min(sp_hour);
                    }
                }
            }

            // Run sub-hourly timesteps within each hour
            // (matches the simulation timestep so CTF coefficients are correct)
            for sub in 0..tph {
                let wh = &design_step_weather(dd, weather_hours, i, sub + 1, tph);
                let ctx = SimulationContext {
                    timestep: TimeStep {
                        month: dd.month,
                        day: dd.day,
                        hour,
                        sub_hour: sub + 1,
                        timesteps_per_hour: tph,
                        sim_time_s: (day_num * 86400 + i * 3600) as f64 + sub as f64 * dt,
                        dt,
                    },
                    outdoor_air: wh.to_air_state(),
                    day_type: DayType::SizingDay,
                    is_sizing: true,
                    sizing_internal_gains: gains_mode,
                };

                // No supply air — ideal loads handles HVAC directly.
                let hvac = ZoneHvacConditions {
                    oa_handled_by_hvac: oa_handled_by_hvac.clone(),
                    ..Default::default()
                };

                let result = env.solve_timestep(&ctx, wh, &hvac);
                env.update_bdf_history();

                if is_last_day {
                    let step_loads = env
                        .zones
                        .iter()
                        .map(|zone| {
                            let name = &zone.input.name;
                            let hl = result.zone_heating_loads.get(name).copied().unwrap_or(0.0);
                            let cl = result.zone_cooling_loads.get(name).copied().unwrap_or(0.0);
                            (name.clone(), (hl, cl))
                        })
                        .collect();
                    last_day_steps.push((hour, step_loads));
                }
            }
        }
    }

    // ── Restore original ideal_loads state ────────────────────────────────
    for (zone, orig) in env.zones.iter_mut().zip(original_ideal_loads.into_iter()) {
        zone.input.ideal_loads = orig;
    }

    hourly_peaks_of_averaged_loads(&last_day_steps, steps.averaging_window)
}

/// Zone (heating, cooling) loads per timestep, tagged with their hour.
type StepLoads = Vec<(u32, HashMap<String, (f64, f64)>)>;

/// Each hour's peak over its timesteps of the loads averaged over the
/// preceding `window` timesteps, wrapping round the day, as E+ averages its
/// zone sizing loads (General::MovingAvg) before taking the peak.
fn hourly_peaks_of_averaged_loads(steps: &StepLoads, window: usize) -> StepLoads {
    let n_steps = steps.len();
    let window = window.clamp(1, n_steps.max(1));
    let mut hourly: StepLoads = Vec::new();
    for (k, (hour, step_loads)) in steps.iter().enumerate() {
        let averaged: HashMap<String, (f64, f64)> = step_loads
            .keys()
            .map(|name| {
                let (mut h, mut c) = (0.0, 0.0);
                for back in 0..window {
                    let (_, loads) = &steps[(k + n_steps - back) % n_steps];
                    let (hl, cl) = loads.get(name).copied().unwrap_or((0.0, 0.0));
                    h += hl;
                    c += cl;
                }
                (name.clone(), (h / window as f64, c / window as f64))
            })
            .collect();
        match hourly.last_mut() {
            Some((h, loads)) if h == hour => {
                for (name, (hl, cl)) in averaged {
                    let entry = loads.entry(name).or_insert((0.0, 0.0));
                    entry.0 = entry.0.max(hl);
                    entry.1 = entry.1.max(cl);
                }
            }
            _ => hourly.push((*hour, averaged)),
        }
    }
    hourly
}

/// Stage 1: Zone Sizing — run ALL design days and find peak zone loads.
///
/// Runs each heating design day and records peak heating loads per zone.
/// Runs each cooling design day and records peak cooling loads per zone.
/// Takes the maximum across all design days of each type.
fn run_zone_sizing(
    env: &mut BuildingEnvelope,
    design_days: &[DesignDayInput],
    setpoints: &SizingSetpoints,
    heating_supply_temp: f64,
    cooling_supply_temp: f64,
    latitude: f64,
    _heating_sizing_factor: f64,
    _cooling_sizing_factor: f64,
    oa_handled_by_hvac: &HashMap<String, bool>,
    steps: SizingSteps,
    sizing_fan_delta_t: f64,
) -> ZoneSizingResult {
    let cp_air = 1005.0;

    let mut zone_peak_heating: HashMap<String, f64> = HashMap::new();
    let mut zone_peak_cooling: HashMap<String, f64> = HashMap::new();
    let mut zone_heating_dd: HashMap<String, String> = HashMap::new();
    let mut zone_cooling_dd: HashMap<String, String> = HashMap::new();
    let mut zone_heating_peak_hour: HashMap<String, u32> = HashMap::new();
    let mut zone_cooling_peak_hour: HashMap<String, u32> = HashMap::new();

    // Initialize
    for zone in &env.zones {
        let name = zone.input.name.clone();
        zone_peak_heating.insert(name.clone(), 0.0);
        zone_peak_cooling.insert(name.clone(), 0.0);
        zone_heating_dd.insert(name.clone(), String::new());
        zone_cooling_dd.insert(name.clone(), String::new());
        zone_heating_peak_hour.insert(name.clone(), 0);
        zone_cooling_peak_hour.insert(name, 0);
    }

    // ── Run ALL heating design days (in parallel) ─────────────────────────
    use rayon::prelude::*;

    let heating_dds: Vec<_> = design_days
        .iter()
        .filter(|dd| is_heating_design_day(dd))
        .collect();

    log::info!(
        "Zone sizing: running {} heating DDs in parallel",
        heating_dds.len()
    );
    let heating_results: Vec<_> = heating_dds
        .par_iter()
        .map(|dd| {
            let weather_hours = generate_heating_design_weather(dd);
            let mut env_copy = env.clone();
            let loads = run_single_design_day(
                &mut env_copy,
                dd,
                &weather_hours,
                &setpoints.heating,
                &setpoints.dd_heating,
                oa_handled_by_hvac,
                steps,
            );
            (dd.name.clone(), loads)
        })
        .collect();

    // Merge heating peaks from all parallel results
    for (dd_name, hourly_loads) in &heating_results {
        for (hour, zone_loads) in hourly_loads {
            for (zone_name, &(hl, _cl)) in zone_loads {
                let current_peak = zone_peak_heating.get(zone_name).copied().unwrap_or(0.0);
                if hl > current_peak {
                    zone_peak_heating.insert(zone_name.clone(), hl);
                    zone_heating_dd.insert(zone_name.clone(), dd_name.clone());
                    zone_heating_peak_hour.insert(zone_name.clone(), *hour);
                }
            }
        }
    }

    // ── Run ALL cooling design days (in parallel) ────────────────────────
    let cooling_dds: Vec<_> = design_days
        .iter()
        .filter(|dd| is_cooling_design_day(dd))
        .collect();

    log::info!(
        "Zone sizing: running {} cooling DDs in parallel",
        cooling_dds.len()
    );
    let cooling_results: Vec<_> = cooling_dds
        .par_iter()
        .map(|dd| {
            let weather_hours = generate_cooling_design_weather(dd, latitude);
            let mut env_copy = env.clone();
            let loads = run_single_design_day(
                &mut env_copy,
                dd,
                &weather_hours,
                &setpoints.cooling,
                &setpoints.dd_cooling,
                oa_handled_by_hvac,
                steps,
            );
            (dd.name.clone(), loads)
        })
        .collect();

    // Merge cooling peaks from all parallel results
    for (dd_name, hourly_loads) in &cooling_results {
        for (hour, zone_loads) in hourly_loads {
            for (zone_name, &(_hl, cl)) in zone_loads {
                let current_peak = zone_peak_cooling.get(zone_name).copied().unwrap_or(0.0);
                if cl > current_peak {
                    zone_peak_cooling.insert(zone_name.clone(), cl);
                    zone_cooling_dd.insert(zone_name.clone(), dd_name.clone());
                    zone_cooling_peak_hour.insert(zone_name.clone(), *hour);
                }
            }
        }
    }

    // Everything here is for one instance of a multiplied zone, because that is
    // what the run simulates; main.rs multiplies the energy once.

    // ── Calculate zone airflows from peak loads ─────────────────────────
    let mut zone_heating_airflow: HashMap<String, f64> = HashMap::new();
    let mut zone_cooling_airflow: HashMap<String, f64> = HashMap::new();
    let mut zone_design_airflow: HashMap<String, f64> = HashMap::new();
    // Per zone: cooling ΔT and outdoor-air floor [kg/s], to turn
    // its hourly design-day loads into flows.
    let mut cool_flow_basis: HashMap<String, (f64, f64)> = HashMap::new();

    for zone in &env.zones {
        let name = &zone.input.name;
        let heat_sp = setpoints.heating.get(name).copied().unwrap_or(21.0);
        let cool_sp = setpoints.cooling.get(name).copied().unwrap_or(24.0);

        let heat_load = zone_peak_heating.get(name).copied().unwrap_or(0.0);
        let cool_load = zone_peak_cooling.get(name).copied().unwrap_or(0.0);

        // The zone's minimum outdoor-air flow is a floor on both design
        // flows, as in E+ (UpdateZoneSizing). The OA load itself is not part
        // of the zone load: the central coil meets it at the mixed-air state.
        let site_p = design_days.first().map(|d| d.pressure).unwrap_or(101325.0);
        let rho_site = site_p / (287.042 * 293.15);
        let m_oa = zone
            .input
            .design_outdoor_air_flow()
            .map_or(0.0, |v_oa| v_oa * rho_site);

        // Heating airflow: Q = m_dot * Cp * (T_supply - T_zone)
        let dt_heating = (heating_supply_temp - heat_sp).max(5.0);
        let m_heat = if heat_load > 0.0 {
            heat_load / (cp_air * dt_heating)
        } else {
            0.0
        }
        .max(m_oa);

        // Cooling airflow: Q = m_dot * Cp * (T_zone - T_supply_effective)
        //
        // sizing_fan_delta_t (from SimulationSettings) adds the fan heat
        // temperature rise to the cooling supply temp during sizing only.
        let t_supply_eff = cooling_supply_temp + sizing_fan_delta_t;
        let dt_cooling = (cool_sp - t_supply_eff).max(3.0);
        let m_cool = if cool_load > 0.0 {
            cool_load / (cp_air * dt_cooling)
        } else {
            0.0
        }
        .max(m_oa);

        // Design airflow is the larger of heating and cooling
        let m_design = m_heat.max(m_cool).max(0.01); // minimum 0.01 kg/s

        zone_heating_airflow.insert(name.clone(), m_heat);
        zone_cooling_airflow.insert(name.clone(), m_cool);
        zone_design_airflow.insert(name.clone(), m_design);
        cool_flow_basis.insert(name.clone(), (dt_cooling, m_oa));
    }

    // Each cooling design day's zone cooling flows, hour by hour, with the
    // same outdoor-air floor (E+ CoolFlowSeq).
    let zone_cooling_flow_sequences = cooling_results
        .iter()
        .map(|(_, hourly)| {
            hourly
                .iter()
                .map(|(_, loads)| {
                    loads
                        .iter()
                        .filter_map(|(name, &(_, cl))| {
                            let &(dt, m_oa) = cool_flow_basis.get(name)?;
                            Some((name.clone(), (cl / (cp_air * dt)).max(m_oa)))
                        })
                        .collect()
                })
                .collect()
        })
        .collect();

    ZoneSizingResult {
        zone_peak_heating,
        zone_peak_cooling,
        zone_heating_dd,
        zone_cooling_dd,
        zone_heating_peak_hour,
        zone_cooling_peak_hour,
        zone_heating_airflow,
        zone_cooling_airflow,
        zone_design_airflow,
        zone_cooling_flow_sequences,
    }
}

/// Stage 2: System Sizing — find coincident peak system loads.
///
/// With zone design airflows established from Stage 1, re-run each design
/// day and at each timestep sum all zone loads to find the coincident peak.
/// This is the actual load the AHU/system must handle.
fn run_system_sizing(
    env: &mut BuildingEnvelope,
    design_days: &[DesignDayInput],
    setpoints: &SizingSetpoints,
    latitude: f64,
    oa_handled_by_hvac: &HashMap<String, bool>,
    steps: SizingSteps,
) -> SystemSizingResult {
    let mut coincident_peak_heating = 0.0_f64;
    let mut coincident_peak_cooling = 0.0_f64;
    let mut heating_peak_dd = String::new();
    let mut cooling_peak_dd = String::new();
    let mut heating_peak_hour = 0_u32;
    let mut cooling_peak_hour = 0_u32;

    // ── Run ALL heating design days (in parallel) ─────────────────────────
    use rayon::prelude::*;

    let heating_dds: Vec<_> = design_days
        .iter()
        .filter(|dd| is_heating_design_day(dd))
        .collect();

    log::info!(
        "System sizing: running {} heating DDs in parallel",
        heating_dds.len()
    );
    let heating_results: Vec<_> = heating_dds
        .par_iter()
        .map(|dd| {
            let weather_hours = generate_heating_design_weather(dd);
            let mut env_copy = env.clone();
            let loads = run_single_design_day(
                &mut env_copy,
                dd,
                &weather_hours,
                &setpoints.heating,
                &setpoints.dd_heating,
                oa_handled_by_hvac,
                steps,
            );
            (dd.name.clone(), loads)
        })
        .collect();

    for (dd_name, hourly_loads) in &heating_results {
        for (hour, zone_loads) in hourly_loads {
            // Plant pumps size from this, so it counts every zone instance.
            let total_heating: f64 = env
                .zones
                .iter()
                .filter_map(|z| {
                    let &(hl, _) = zone_loads.get(&z.input.name)?;
                    Some(hl * z.input.zone_multiplier as f64)
                })
                .sum();
            if total_heating > coincident_peak_heating {
                coincident_peak_heating = total_heating;
                heating_peak_dd = dd_name.clone();
                heating_peak_hour = *hour;
            }
        }
    }

    // ── Run ALL cooling design days (in parallel) ────────────────────────
    let cooling_dds: Vec<_> = design_days
        .iter()
        .filter(|dd| is_cooling_design_day(dd))
        .collect();

    log::info!(
        "System sizing: running {} cooling DDs in parallel",
        cooling_dds.len()
    );
    let cooling_results: Vec<_> = cooling_dds
        .par_iter()
        .map(|dd| {
            let weather_hours = generate_cooling_design_weather(dd, latitude);
            let mut env_copy = env.clone();
            let loads = run_single_design_day(
                &mut env_copy,
                dd,
                &weather_hours,
                &setpoints.cooling,
                &setpoints.dd_cooling,
                oa_handled_by_hvac,
                steps,
            );
            (dd.name.clone(), loads)
        })
        .collect();

    for (dd_name, hourly_loads) in &cooling_results {
        for (hour, zone_loads) in hourly_loads {
            let total_cooling: f64 = env
                .zones
                .iter()
                .filter_map(|z| {
                    let &(_, cl) = zone_loads.get(&z.input.name)?;
                    Some(cl * z.input.zone_multiplier as f64)
                })
                .sum();
            if total_cooling > coincident_peak_cooling {
                coincident_peak_cooling = total_cooling;
                cooling_peak_dd = dd_name.clone();
                cooling_peak_hour = *hour;
            }
        }
    }

    SystemSizingResult {
        coincident_peak_heating,
        coincident_peak_cooling,
        heating_peak_dd,
        cooling_peak_dd,
        heating_peak_hour,
        cooling_peak_hour,
    }
}

/// Compute monthly 99.6th percentile (peak design) dry-bulb from weather data.
///
/// The cooling design temperature is roughly the 0.4% exceedance temperature
/// (i.e. 99.6th percentile). We estimate this per month by taking the 99.6th
/// percentile of the monthly dry-bulb samples. Returns an array of 12 values.
fn monthly_peak_dry_bulb(weather_hours: &[WeatherHour]) -> [f64; 12] {
    let mut by_month: [Vec<f64>; 12] = Default::default();
    for wh in weather_hours {
        let m = (wh.month.saturating_sub(1)).min(11) as usize;
        by_month[m].push(wh.dry_bulb);
    }
    std::array::from_fn(|i| {
        let v = &mut by_month[i];
        if v.is_empty() {
            return 0.0;
        }
        v.sort_by(|a, b| a.total_cmp(b));
        let idx = ((v.len() as f64 * 0.996) as usize).min(v.len() - 1);
        v[idx]
    })
}

/// Typical representative day of each month (used for solar position in generated DDs).
const MONTH_REPRESENTATIVE_DAYS: [u32; 12] = [17, 15, 16, 15, 15, 11, 17, 16, 15, 15, 14, 10];

/// Auto-generate monthly cooling design days from the user's anchor cooling DD.
///
/// The user defines one (or more) cooling design days (e.g. "Boulder Cooling 0.4%"
/// for July). We automatically generate a cooling DD for every other month by:
///
/// 1. Taking the anchor month's 99.6th percentile temperature from the weather file.
/// 2. For each other month, scaling the anchor DD's design temp by the difference
///    in the monthly 99.6th percentile temperatures.
/// 3. Keeping all other DD properties (humidity, pressure, wind, daily range)
///    from the anchor DD.
/// 4. Setting the correct month/day so the solar calculation uses the right sun
///    position for that month.
///
/// This matches the approach used by tools like IES-VE: one user-defined peak DD,
/// with shoulder-season DDs auto-generated so the sizing can catch cases where
/// lower sun angles cause higher solar gains despite cooler outdoor temperatures.
fn generate_monthly_cooling_dds(
    user_cooling_dds: &[&DesignDayInput],
    weather_hours: &[WeatherHour],
) -> Vec<DesignDayInput> {
    if user_cooling_dds.is_empty() || weather_hours.is_empty() {
        return Vec::new();
    }

    // Find the single anchor DD: the one with the highest design temp (peak summer)
    let anchor = user_cooling_dds
        .iter()
        .max_by(|a, b| a.design_temp.total_cmp(&b.design_temp))
        .unwrap();

    let anchor_month_idx = (anchor.month.saturating_sub(1)).min(11) as usize;

    // Compute monthly 99.6th percentile temperatures from actual weather
    let monthly_peak = monthly_peak_dry_bulb(weather_hours);
    let anchor_peak = monthly_peak[anchor_month_idx];

    if anchor_peak <= 0.0 {
        log::warn!("Auto monthly DDs: anchor month has zero peak temp, skipping generation");
        return Vec::new();
    }

    // Months already covered by user-defined cooling DDs
    let user_months: std::collections::HashSet<u32> =
        user_cooling_dds.iter().map(|dd| dd.month).collect();

    let mut generated = Vec::new();

    for month in 1u32..=12 {
        if user_months.contains(&month) {
            continue; // User already defined a DD for this month
        }

        let m_idx = (month - 1) as usize;
        let month_peak = monthly_peak[m_idx];

        if month_peak <= 0.0 {
            continue; // No data for this month
        }

        // Scale anchor design temp by the difference in monthly peaks
        let delta = month_peak - anchor_peak;
        let scaled_temp = anchor.design_temp + delta;

        // Only generate for months where it could matter (temp within 20°C of anchor)
        // This naturally skips winter months for a cooling DD
        if scaled_temp < anchor.design_temp - 20.0 {
            continue;
        }

        let month_name = [
            "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
        ][m_idx];
        let day = MONTH_REPRESENTATIVE_DAYS[m_idx];

        generated.push(DesignDayInput {
            name: format!("{} (auto-{})", anchor.name, month_name),
            design_temp: scaled_temp,
            daily_range: anchor.daily_range,
            humidity_type: anchor.humidity_type,
            humidity_value: match anchor.humidity_type {
                // Partial humid scaling; the other types carry no temperature.
                DesignDayHumidityType::Wetbulb | DesignDayHumidityType::Dewpoint => {
                    anchor.humidity_value + delta * 0.5
                }
                _ => anchor.humidity_value,
            },
            pressure: anchor.pressure,
            wind_speed: anchor.wind_speed,
            month,
            day,
            day_type: anchor.day_type.clone(),
            internal_gains: anchor.internal_gains,
            taub: anchor.taub,
            taud: anchor.taud,
            cooling_setpoint_schedule: anchor.cooling_setpoint_schedule.clone(),
        });

        log::info!(
            "  Auto-generated cooling DD for {}: {:.1}°C (anchor {:.1}°C, Δ{:+.1}°C from weather peak diff)",
            month_name, scaled_temp, anchor.design_temp, delta
        );
    }

    generated
}

/// Run the full two-stage design day sizing.
///
/// # Arguments
/// * `env` — Mutable reference to the BuildingEnvelope
/// * `design_days` — All design day definitions from YAML input
/// * `thermostats` — Thermostat definitions with heating/cooling setpoints
/// * `latitude` — Site latitude [degrees]
/// * `weather_hours` — Full year weather data for monthly mean computation
/// * `output_dir` — Directory to write sizing result files
///
/// # Returns
/// `SizingResult` with zone-level and system-level sizing data.
pub fn run_sizing(
    env: &mut BuildingEnvelope,
    design_days: &[DesignDayInput],
    thermostats: &[ThermostatInput],
    latitude: f64,
    weather_hours: &[WeatherHour],
    output_dir: &Path,
    input_stem: &str,
    supply_temps: Option<(f64, f64)>,
    heating_sizing_factor: f64,
    cooling_sizing_factor: f64,
    oa_handled_by_hvac: &HashMap<String, bool>,
    timesteps_per_hour: u32,
    sizing_fan_delta_t: f64,
    auto_cooling_design_days: bool,
    averaging_window: usize,
) -> SizingResult {
    // Compute standard air density at site altitude using the design-day
    // barometric pressure.  E+ uses this same approach (ρ = P / (R·T) at 20 °C).
    // Without this, high-altitude sites like Denver (1612 m, ~83 kPa) would
    // undersize airflow by ~20%.
    let site_pressure = design_days
        .first()
        .map(|dd| dd.pressure)
        .unwrap_or(101325.0);
    let rho_air = site_pressure / (287.042 * 293.15); // ideal gas at 20°C

    log::info!(
        "Sizing factors: heating={:.2}, cooling={:.2}",
        heating_sizing_factor,
        cooling_sizing_factor
    );

    // ── Gather setpoints from resolved thermostats ───────────────────────
    let mut setpoints = SizingSetpoints::default();
    // Supply temps from air loop controls.  If the caller provides them
    // (extracted from air_loops), use those; otherwise fall back to defaults.
    let (heating_supply_temp, cooling_supply_temp) = supply_temps.unwrap_or((35.0, 13.0));
    let steps = SizingSteps {
        warmup_days: 5,
        per_hour: timesteps_per_hour,
        averaging_window,
    };

    for tstat in thermostats {
        for zone_name in &tstat.zones {
            setpoints
                .heating
                .insert(zone_name.clone(), tstat.heating_setpoint);
            setpoints
                .cooling
                .insert(zone_name.clone(), tstat.cooling_setpoint);
            if let Some(ref sched) = tstat.design_day_heating_setpoints {
                setpoints
                    .dd_heating
                    .insert(zone_name.clone(), sched.clone());
            }
            if let Some(ref sched) = tstat.design_day_cooling_setpoints {
                setpoints
                    .dd_cooling
                    .insert(zone_name.clone(), sched.clone());
            }
        }
    }

    // For zones not in any zone group, use defaults
    for zone in &env.zones {
        setpoints
            .heating
            .entry(zone.input.name.clone())
            .or_insert(21.0);
        setpoints
            .cooling
            .entry(zone.input.name.clone())
            .or_insert(24.0);
    }

    // ── Auto-generate monthly cooling design days ─────────────────────────
    // The user defines one anchor cooling DD (e.g. July 0.4%). We auto-generate
    // DDs for the remaining months so shoulder-season solar peaks are captured.
    let user_cooling_dds: Vec<&DesignDayInput> = design_days
        .iter()
        .filter(|dd| is_cooling_design_day(dd))
        .collect();

    let auto_cooling_dds =
        if auto_cooling_design_days && !user_cooling_dds.is_empty() && !weather_hours.is_empty() {
            log::info!(
                "Auto-generating monthly cooling DDs from anchor '{}'...",
                user_cooling_dds
                    .iter()
                    .max_by(|a, b| a.design_temp.total_cmp(&b.design_temp))
                    .map(|dd| dd.name.as_str())
                    .unwrap_or("?")
            );
            generate_monthly_cooling_dds(&user_cooling_dds, weather_hours)
        } else {
            Vec::new()
        };

    // Combine user DDs + auto-generated DDs into one complete list
    let all_design_days: Vec<DesignDayInput> = design_days
        .iter()
        .cloned()
        .chain(auto_cooling_dds)
        .collect();

    if all_design_days.len() > design_days.len() {
        log::info!(
            "Total design days: {} user-defined + {} auto-generated = {}",
            design_days.len(),
            all_design_days.len() - design_days.len(),
            all_design_days.len()
        );
    }

    log::info!("══════════════════════════════════════════════════════════");
    log::info!("  STAGE 1: Zone Sizing");
    log::info!("══════════════════════════════════════════════════════════");

    // ── Stage 1: Zone Sizing ─────────────────────────────────────────────
    let zone_sizing = run_zone_sizing(
        env,
        &all_design_days,
        &setpoints,
        heating_supply_temp,
        cooling_supply_temp,
        latitude,
        heating_sizing_factor,
        cooling_sizing_factor,
        oa_handled_by_hvac,
        steps,
        sizing_fan_delta_t,
    );

    // Log zone sizing results
    log::info!("── Zone Sizing Results ─────────────────────────────────");
    for zone in &env.zones {
        let name = &zone.input.name;
        let ph = zone_sizing.zone_peak_heating.get(name).unwrap_or(&0.0);
        let pc = zone_sizing.zone_peak_cooling.get(name).unwrap_or(&0.0);
        let mf = zone_sizing.zone_design_airflow.get(name).unwrap_or(&0.0);
        let hdd = zone_sizing
            .zone_heating_dd
            .get(name)
            .map(|s| s.as_str())
            .unwrap_or("-");
        let hhr = zone_sizing.zone_heating_peak_hour.get(name).unwrap_or(&0);
        let cdd = zone_sizing
            .zone_cooling_dd
            .get(name)
            .map(|s| s.as_str())
            .unwrap_or("-");
        let chr = zone_sizing.zone_cooling_peak_hour.get(name).unwrap_or(&0);
        log::info!(
            "  Zone '{}': htg={:.0}W (DD: {}, hr {}), clg={:.0}W (DD: {}, hr {}), flow={:.3} kg/s",
            name,
            ph,
            hdd,
            hhr,
            pc,
            cdd,
            chr,
            mf
        );
    }

    log::info!("══════════════════════════════════════════════════════════");
    log::info!("  STAGE 2: System Sizing");
    log::info!("══════════════════════════════════════════════════════════");

    // ── Stage 2: System Sizing ───────────────────────────────────────────
    let system_sizing = run_system_sizing(
        env,
        &all_design_days,
        &setpoints,
        latitude,
        oa_handled_by_hvac,
        steps,
    );

    // System capacities with sizing factors
    let system_heating_capacity = system_sizing.coincident_peak_heating * heating_sizing_factor;
    let system_cooling_capacity = system_sizing.coincident_peak_cooling * cooling_sizing_factor;
    let system_airflow: f64 = zone_sizing.zone_design_airflow.values().sum();
    let system_volume_flow = system_airflow / rho_air;

    log::info!("── System Sizing Results ───────────────────────────────");
    log::info!(
        "  Coincident peak heating: {:.0} W ({:.1} kW) on DD '{}' at hour {}",
        system_sizing.coincident_peak_heating,
        system_sizing.coincident_peak_heating / 1000.0,
        system_sizing.heating_peak_dd,
        system_sizing.heating_peak_hour
    );
    log::info!(
        "  Coincident peak cooling: {:.0} W ({:.1} kW) on DD '{}' at hour {}",
        system_sizing.coincident_peak_cooling,
        system_sizing.coincident_peak_cooling / 1000.0,
        system_sizing.cooling_peak_dd,
        system_sizing.cooling_peak_hour
    );
    log::info!(
        "  System heating capacity (×{:.0}%): {:.0} W ({:.1} kW)",
        heating_sizing_factor * 100.0,
        system_heating_capacity,
        system_heating_capacity / 1000.0
    );
    log::info!(
        "  System cooling capacity (×{:.0}%): {:.0} W ({:.1} kW)",
        cooling_sizing_factor * 100.0,
        system_cooling_capacity,
        system_cooling_capacity / 1000.0
    );
    log::info!(
        "  System airflow: {:.3} kg/s ({:.4} m³/s, {:.0} CFM)",
        system_airflow,
        system_volume_flow,
        system_volume_flow * 2118.88
    );
    log::info!("══════════════════════════════════════════════════════════");

    // ── Write sizing output files ────────────────────────────────────────
    write_zone_sizing_csv(&zone_sizing, &env.zones, output_dir, input_stem, rho_air);
    write_system_sizing_csv(
        &zone_sizing,
        &system_sizing,
        system_heating_capacity,
        system_cooling_capacity,
        system_airflow,
        system_volume_flow,
        heating_sizing_factor,
        cooling_sizing_factor,
        output_dir,
        input_stem,
    );

    // Reset zone temperatures for the real simulation
    for zone in &mut env.zones {
        let sp = setpoints
            .heating
            .get(&zone.input.name)
            .copied()
            .unwrap_or(21.0);
        zone.temp = sp;
        zone.temp_prev = sp;
    }

    SizingResult {
        zone_sizing: zone_sizing.clone(),
        system_sizing,
        zone_peak_heating: zone_sizing.zone_peak_heating,
        zone_peak_cooling: zone_sizing.zone_peak_cooling,
        zone_heating_airflow: zone_sizing.zone_heating_airflow,
        zone_cooling_airflow: zone_sizing.zone_cooling_airflow,
        zone_design_airflow: zone_sizing.zone_design_airflow,
        system_heating_capacity,
        system_cooling_capacity,
        system_airflow,
        system_volume_flow,
    }
}

// ─── Sizing Output Files ─────────────────────────────────────────────────────

/// Write zone sizing results to a CSV file.
fn write_zone_sizing_csv(
    zone_sizing: &ZoneSizingResult,
    zones: &[openbse_envelope::zone::ZoneState],
    output_dir: &Path,
    input_stem: &str,
    rho_air: f64,
) {
    let path = output_dir.join(format!("{}_zone_sizing.csv", input_stem));
    let mut lines = Vec::new();

    lines.push(
        "Zone,Peak Heating [W],Heating DD,Heating Peak Hour,\
         Peak Cooling [W],Cooling DD,Cooling Peak Hour,\
         Heating Airflow [kg/s],Cooling Airflow [kg/s],Design Airflow [kg/s],Design Airflow [m3/s],Design Airflow [CFM]"
            .to_string()
    );

    for zone in zones {
        let name = &zone.input.name;
        let ph = zone_sizing.zone_peak_heating.get(name).unwrap_or(&0.0);
        let hdd = zone_sizing
            .zone_heating_dd
            .get(name)
            .map(|s| s.as_str())
            .unwrap_or("");
        let hhr = zone_sizing.zone_heating_peak_hour.get(name).unwrap_or(&0);
        let pc = zone_sizing.zone_peak_cooling.get(name).unwrap_or(&0.0);
        let cdd = zone_sizing
            .zone_cooling_dd
            .get(name)
            .map(|s| s.as_str())
            .unwrap_or("");
        let chr = zone_sizing.zone_cooling_peak_hour.get(name).unwrap_or(&0);
        let mh = zone_sizing.zone_heating_airflow.get(name).unwrap_or(&0.0);
        let mc = zone_sizing.zone_cooling_airflow.get(name).unwrap_or(&0.0);
        let md = zone_sizing.zone_design_airflow.get(name).unwrap_or(&0.0);
        let vol_flow = md / rho_air; // kg/s to m³/s
        let cfm = vol_flow * 2118.88;

        lines.push(format!(
            "{},{:.1},{},{},{:.1},{},{},{:.4},{:.4},{:.4},{:.4},{:.0}",
            name, ph, hdd, hhr, pc, cdd, chr, mh, mc, md, vol_flow, cfm
        ));
    }

    match std::fs::write(&path, lines.join("\n") + "\n") {
        Ok(()) => log::info!("Zone sizing results written to: {}", path.display()),
        Err(e) => log::warn!("Failed to write zone sizing CSV: {}", e),
    }
}

/// Write system sizing results to a CSV file.
fn write_system_sizing_csv(
    zone_sizing: &ZoneSizingResult,
    system_sizing: &SystemSizingResult,
    system_heating_cap: f64,
    system_cooling_cap: f64,
    system_airflow: f64,
    system_volume_flow: f64,
    heating_sizing_factor: f64,
    cooling_sizing_factor: f64,
    output_dir: &Path,
    input_stem: &str,
) {
    let path = output_dir.join(format!("{}_system_sizing.csv", input_stem));
    let mut lines = Vec::new();

    lines.push("Parameter,Value,Unit,Notes".to_string());
    lines.push(format!(
        "Heating Sizing Factor,{:.2},,",
        heating_sizing_factor
    ));
    lines.push(format!(
        "Cooling Sizing Factor,{:.2},,",
        cooling_sizing_factor
    ));
    lines.push(String::new());

    // Zone summary
    lines.push("--- Zone Sizing Summary ---,,,".to_string());
    lines.push("Zone,Peak Heating [W],Peak Cooling [W],Design Airflow [kg/s]".to_string());

    let mut total_zone_heating = 0.0_f64;
    let mut total_zone_cooling = 0.0_f64;
    for (name, &ph) in &zone_sizing.zone_peak_heating {
        let pc = zone_sizing.zone_peak_cooling.get(name).unwrap_or(&0.0);
        let md = zone_sizing.zone_design_airflow.get(name).unwrap_or(&0.0);
        total_zone_heating += ph;
        total_zone_cooling += pc;
        lines.push(format!("{},{:.1},{:.1},{:.4}", name, ph, pc, md));
    }
    lines.push(format!(
        "Total (non-coincident),{:.1},{:.1},",
        total_zone_heating, total_zone_cooling
    ));
    lines.push(String::new());

    // System sizing
    lines.push("--- System Sizing (Coincident Peaks) ---,,,".to_string());
    lines.push(format!(
        "Coincident Peak Heating,{:.1},W,DD: {} at hour {}",
        system_sizing.coincident_peak_heating,
        system_sizing.heating_peak_dd,
        system_sizing.heating_peak_hour
    ));
    lines.push(format!(
        "Coincident Peak Cooling,{:.1},W,DD: {} at hour {}",
        system_sizing.coincident_peak_cooling,
        system_sizing.cooling_peak_dd,
        system_sizing.cooling_peak_hour
    ));
    lines.push(String::new());

    lines.push("--- Sized System Capacities ---,,,".to_string());
    lines.push(format!(
        "System Heating Capacity,{:.1},W,(coincident peak x {:.0}%)",
        system_heating_cap,
        heating_sizing_factor * 100.0
    ));
    lines.push(format!(
        "System Heating Capacity,{:.2},kW,",
        system_heating_cap / 1000.0
    ));
    lines.push(format!(
        "System Cooling Capacity,{:.1},W,(coincident peak x {:.0}%)",
        system_cooling_cap,
        cooling_sizing_factor * 100.0
    ));
    lines.push(format!(
        "System Cooling Capacity,{:.2},kW,",
        system_cooling_cap / 1000.0
    ));
    lines.push(format!(
        "System Cooling Capacity,{:.2},tons,",
        system_cooling_cap / 3517.0
    ));
    lines.push(format!(
        "System Airflow,{:.4},kg/s,(sum of zone design airflows)",
        system_airflow
    ));
    lines.push(format!("System Airflow,{:.4},m3/s,", system_volume_flow));
    lines.push(format!(
        "System Airflow,{:.0},CFM,",
        system_volume_flow * 2118.88
    ));

    match std::fs::write(&path, lines.join("\n") + "\n") {
        Ok(()) => log::info!("System sizing results written to: {}", path.display()),
        Err(e) => log::warn!("Failed to write system sizing CSV: {}", e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn design_day(humidity_type: DesignDayHumidityType, humidity_value: f64) -> DesignDayInput {
        DesignDayInput {
            name: "DD".to_string(),
            design_temp: 35.0,
            daily_range: 10.0,
            humidity_type,
            humidity_value,
            pressure: 101_325.0,
            wind_speed: 3.0,
            month: 7,
            day: 21,
            day_type: "summer".to_string(),
            internal_gains: None,
            taub: None,
            taud: None,
            cooling_setpoint_schedule: None,
        }
    }

    #[test]
    fn dry_bulb_follows_the_ashrae_profile() {
        let dd = design_day(DesignDayHumidityType::Wetbulb, 24.0);
        let hours = generate_cooling_design_weather(&dd, 40.0);
        // Peak in hours 14–15, minimum in hour 5, E+ DefaultTempRangeMult.
        assert_eq!(hours[13].dry_bulb, 35.0);
        assert_eq!(hours[14].dry_bulb, 35.0);
        assert_eq!(hours[4].dry_bulb, 25.0);
        assert!((hours[0].dry_bulb - (35.0 - 8.8)).abs() < 1e-12);
    }

    #[test]
    fn step_weather_interpolates_within_the_hour() {
        let dd = design_day(DesignDayHumidityType::Wetbulb, 15.0);
        let hours = generate_cooling_design_weather(&dd, 40.0);
        // The first hour starts from the last: fractions 0.82 then 0.88.
        let t = design_step_weather(&dd, &hours, 0, 2, 4).dry_bulb;
        assert!((t - (35.0 - 8.5)).abs() < 1e-12);
        assert_eq!(
            design_step_weather(&dd, &hours, 7, 4, 4).dry_bulb,
            hours[7].dry_bulb
        );
    }

    #[test]
    fn humidity_type_sets_the_outdoor_humidity_ratio() {
        use DesignDayHumidityType::*;
        let p = 101_325.0;
        let w_of = |dd: &DesignDayInput| {
            generate_cooling_design_weather(dd, 40.0)[14]
                .to_air_state()
                .w
        };
        // The issue's case: 35 °C / 24 °C wet-bulb is about 0.0144 kg/kg, not
        // the 0.0107 kg/kg of the old fixed 30% RH.
        let w_wb = w_of(&design_day(Wetbulb, 24.0));
        assert!((w_wb - psych::w_fn_tdb_twb_pb(35.0, 24.0, p)).abs() < 1e-6);
        assert!((w_wb - 0.0144).abs() < 0.0005, "{w_wb}");

        let w_dp = w_of(&design_day(Dewpoint, 18.0));
        assert!((w_dp - psych::w_fn_tdp_pb(18.0, p)).abs() < 1e-6);

        assert!((w_of(&design_day(HumidityRatio, 0.012)) - 0.012).abs() < 1e-6);

        let h = psych::h_fn_tdb_w(35.0, 0.012);
        assert!((w_of(&design_day(Enthalpy, h)) - 0.012).abs() < 1e-5);

        let w_rh = w_of(&design_day(RelativeHumidity, 40.0));
        assert!((w_rh - psych::w_fn_tdb_rh_pb(35.0, 0.4, p)).abs() < 1e-6);
    }

    #[test]
    fn humidity_ratio_stays_constant_until_the_air_saturates() {
        let dd = design_day(DesignDayHumidityType::Wetbulb, 24.0);
        let hours = generate_cooling_design_weather(&dd, 40.0);
        let (peak, night) = (hours[14].to_air_state().w, hours[4].to_air_state().w);
        let w_sat_night = psych::w_fn_tdb_rh_pb(25.0, 1.0, dd.pressure);
        assert!(peak < w_sat_night);
        assert!((peak - night).abs() < 1e-6);
        // Humid enough to saturate at night: capped at saturation, as E+ caps RH.
        let dd = DesignDayInput {
            daily_range: 20.0,
            ..design_day(DesignDayHumidityType::Wetbulb, 28.0)
        };
        let hours = generate_cooling_design_weather(&dd, 40.0);
        assert!((hours[4].rel_humidity - 100.0).abs() < 1e-6);
        assert!((hours[4].dew_point - hours[4].dry_bulb).abs() < 1e-3);
    }

    #[test]
    fn unknown_humidity_type_is_rejected() {
        assert!(serde_yaml::from_str::<DesignDayHumidityType>("wetbulb").is_ok());
        assert!(serde_yaml::from_str::<DesignDayHumidityType>("wet_bulb").is_ok());
        assert!(serde_yaml::from_str::<DesignDayHumidityType>("enthalpy").is_ok());
        assert!(serde_yaml::from_str::<DesignDayHumidityType>("wetbulbb").is_err());
    }

    fn steps(loads: &[(u32, f64)]) -> StepLoads {
        loads
            .iter()
            .map(|&(hour, heat)| (hour, HashMap::from([("Z".to_string(), (heat, 0.0))])))
            .collect()
    }

    #[test]
    fn averaging_window_of_one_keeps_the_peak_timestep() {
        let hourly =
            hourly_peaks_of_averaged_loads(&steps(&[(1, 2.0), (1, 6.0), (2, 1.0), (2, 3.0)]), 1);
        assert_eq!(hourly.len(), 2);
        assert_eq!(hourly[0].1["Z"].0, 6.0);
        assert_eq!(hourly[1].1["Z"].0, 3.0);
    }

    #[test]
    fn averaging_window_smooths_a_one_timestep_spike() {
        // A spike of 12 in an otherwise flat day, averaged over 3 steps,
        // reads 4 in the step it occurs and the two after (into hour 2).
        let hourly = hourly_peaks_of_averaged_loads(
            &steps(&[(1, 0.0), (1, 12.0), (2, 0.0), (2, 0.0), (3, 0.0), (3, 0.0)]),
            3,
        );
        assert_eq!(hourly[0].1["Z"].0, 4.0);
        assert_eq!(hourly[1].1["Z"].0, 4.0);
        assert_eq!(hourly[2].1["Z"].0, 0.0);
    }

    #[test]
    fn averaging_window_wraps_round_the_day() {
        // The first step averages in the last step of the (periodic) day.
        let hourly = hourly_peaks_of_averaged_loads(&steps(&[(1, 0.0), (2, 0.0), (3, 9.0)]), 2);
        assert_eq!(hourly[0].1["Z"].0, 4.5);
    }
}
