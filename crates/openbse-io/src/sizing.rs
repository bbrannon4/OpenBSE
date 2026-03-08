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

use std::collections::HashMap;
use std::path::Path;
use openbse_core::ports::{EnvelopeSolver, SimulationContext, ZoneHvacConditions};
use openbse_core::types::{DayType, TimeStep};
use openbse_envelope::BuildingEnvelope;
use openbse_weather::WeatherHour;

use crate::input::DesignDayInput;
use openbse_envelope::ThermostatInput;

/// Zone-level sizing results from Stage 1.
#[derive(Debug, Clone)]
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

/// Generate 24 synthetic WeatherHour entries for a heating design day.
///
/// Heating design days use constant temperature (no daily range),
/// no solar radiation, and the specified wind speed.
fn generate_heating_design_weather(dd: &DesignDayInput) -> Vec<WeatherHour> {
    let mut hours = Vec::with_capacity(24);
    for h in 1..=24u32 {
        hours.push(WeatherHour {
            year: 2024,
            month: dd.month,
            day: dd.day,
            hour: h,
            dry_bulb: dd.design_temp,
            dew_point: dd.humidity_value.min(dd.design_temp),
            rel_humidity: 50.0,
            pressure: dd.pressure,
            global_horiz_rad: 0.0,    // no solar for heating design
            direct_normal_rad: 0.0,
            diffuse_horiz_rad: 0.0,
            wind_speed: dd.wind_speed,
            wind_direction: 0.0,
            opaque_sky_cover: 10.0,
            horiz_ir_rad: 300.0,
        });
    }
    hours
}

/// Generate 24 synthetic WeatherHour entries for a cooling design day.
///
/// Cooling design days use a sinusoidal temperature profile based on
/// the design temp and daily range. Solar is estimated from a clear-sky model.
fn generate_cooling_design_weather(dd: &DesignDayInput, latitude: f64) -> Vec<WeatherHour> {
    let mut hours = Vec::with_capacity(24);

    for h in 1..=24u32 {
        // ASHRAE sinusoidal temperature profile
        // Peak at hour 15 (3 PM), minimum at hour 3-5 (pre-dawn)
        // T(h) = T_max - DR × f(h), where f(h) = 0.5 × (1 - cos(angle))
        //   f = 0 at h=15 → T = T_max (design temperature)
        //   f = 1 at h=3  → T = T_max - DR (minimum)
        let hour_angle = std::f64::consts::PI * (h as f64 - 15.0) / 12.0;
        let t_db = dd.design_temp - dd.daily_range * 0.5 * (1.0 - hour_angle.cos());

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

        hours.push(WeatherHour {
            year: 2024,
            month: dd.month,
            day: dd.day,
            hour: h,
            dry_bulb: t_db,
            dew_point: dd.humidity_value.min(t_db),
            rel_humidity: 30.0,
            pressure: dd.pressure,
            global_horiz_rad: (direct * sin_altitude + diffuse).max(0.0),
            direct_normal_rad: direct.max(0.0),
            diffuse_horiz_rad: diffuse.max(0.0),
            wind_speed: dd.wind_speed,
            wind_direction: 0.0,
            opaque_sky_cover: 1.0,    // clear sky for cooling design
            horiz_ir_rad: 350.0,
        });
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

/// Run a single design day through the envelope with ideal HVAC holding
/// zones at their setpoint temperature, returning per-zone loads at each hour.
///
/// Returns: Vec of (hour, HashMap<zone_name, (heating_load, cooling_load)>)
fn run_single_design_day(
    env: &mut BuildingEnvelope,
    dd: &DesignDayInput,
    weather_hours: &[WeatherHour],
    zone_setpoints: &HashMap<String, f64>,  // setpoint to hold zone at
    num_warmup_days: usize,
) -> Vec<(u32, HashMap<String, (f64, f64)>)> {
    let is_heating_dd = is_heating_design_day(dd);
    let rh = if is_heating_dd { 0.5 } else { 0.3 };

    // Internal gains mode: use the design day's explicit setting, or default
    // based on day_type (heating → Off, cooling → Full).
    use openbse_core::ports::SizingInternalGains;
    let gains_mode = dd.internal_gains.unwrap_or_else(|| {
        if is_heating_dd {
            SizingInternalGains::Off
        } else {
            SizingInternalGains::Full
        }
    });

    // Reset zone temperatures to setpoint
    for zone in &mut env.zones {
        let sp = zone_setpoints.get(&zone.input.name).copied().unwrap_or(21.0);
        zone.temp = sp;
        zone.temp_prev = sp;
    }

    let mut last_day_results = Vec::new();

    for day_num in 0..num_warmup_days {
        let is_last_day = day_num == num_warmup_days - 1;

        for (i, wh) in weather_hours.iter().enumerate() {
            let hour = (i + 1) as u32;
            let ctx = SimulationContext {
                timestep: TimeStep {
                    month: dd.month,
                    day: dd.day,
                    hour,
                    sub_hour: 1,
                    timesteps_per_hour: 1,
                    sim_time_s: (day_num * 86400 + i * 3600) as f64,
                    dt: 3600.0,
                },
                outdoor_air: openbse_psychrometrics::MoistAirState::from_tdb_rh(
                    wh.dry_bulb, rh, wh.pressure,
                ),
                day_type: DayType::SizingDay,
                is_sizing: true,
                sizing_internal_gains: gains_mode,
            };

            // Supply air at zone setpoint temp with large flow to hold at setpoint.
            // This ensures the "external HVAC" branch runs in the envelope solver,
            // which computes zone loads correctly (what Q_HVAC is needed).
            let mut hvac = ZoneHvacConditions::default();
            for zone in &env.zones {
                if zone.input.conditioned {
                    let sp = zone_setpoints.get(&zone.input.name).copied().unwrap_or(21.0);
                    hvac.supply_temps.insert(zone.input.name.clone(), sp);
                    hvac.supply_mass_flows.insert(zone.input.name.clone(), 10.0);
                }
            }

            let result = env.solve_timestep(&ctx, wh, &hvac);

            // Record loads on the last day only (after warmup)
            if is_last_day {
                let mut hour_loads: HashMap<String, (f64, f64)> = HashMap::new();
                for zone in &env.zones {
                    let name = &zone.input.name;
                    let hl = result.zone_heating_loads.get(name).copied().unwrap_or(0.0);
                    let cl = result.zone_cooling_loads.get(name).copied().unwrap_or(0.0);
                    hour_loads.insert(name.clone(), (hl, cl));
                }
                last_day_results.push((hour, hour_loads));
            }
        }
    }

    last_day_results
}

/// Stage 1: Zone Sizing — run ALL design days and find peak zone loads.
///
/// Runs each heating design day and records peak heating loads per zone.
/// Runs each cooling design day and records peak cooling loads per zone.
/// Takes the maximum across all design days of each type.
fn run_zone_sizing(
    env: &mut BuildingEnvelope,
    design_days: &[DesignDayInput],
    zone_heating_setpoints: &HashMap<String, f64>,
    zone_cooling_setpoints: &HashMap<String, f64>,
    heating_supply_temp: f64,
    cooling_supply_temp: f64,
    latitude: f64,
    heating_sizing_factor: f64,
    cooling_sizing_factor: f64,
) -> ZoneSizingResult {
    let cp_air = 1005.0;
    let num_warmup_days = 5;

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

    // ── Run ALL heating design days ──────────────────────────────────────
    let heating_dds: Vec<_> = design_days.iter()
        .filter(|dd| is_heating_design_day(dd))
        .collect();

    for dd in &heating_dds {
        let weather_hours = generate_heating_design_weather(dd);
        log::info!("Zone sizing: running heating DD '{}' at {:.1}°C", dd.name, dd.design_temp);

        let hourly_loads = run_single_design_day(
            env, dd, &weather_hours, zone_heating_setpoints, num_warmup_days,
        );

        // Update peaks (take max across all heating design days)
        for (hour, zone_loads) in &hourly_loads {
            for (zone_name, &(hl, _cl)) in zone_loads {
                let current_peak = zone_peak_heating.get(zone_name).copied().unwrap_or(0.0);
                if hl > current_peak {
                    zone_peak_heating.insert(zone_name.clone(), hl);
                    zone_heating_dd.insert(zone_name.clone(), dd.name.clone());
                    zone_heating_peak_hour.insert(zone_name.clone(), *hour);
                }
            }
        }
    }

    // ── Run ALL cooling design days ──────────────────────────────────────
    let cooling_dds: Vec<_> = design_days.iter()
        .filter(|dd| is_cooling_design_day(dd))
        .collect();

    for dd in &cooling_dds {
        let weather_hours = generate_cooling_design_weather(dd, latitude);
        log::info!("Zone sizing: running cooling DD '{}' at {:.1}°C", dd.name, dd.design_temp);

        let hourly_loads = run_single_design_day(
            env, dd, &weather_hours, zone_cooling_setpoints, num_warmup_days,
        );

        // Update peaks (take max across all cooling design days)
        for (hour, zone_loads) in &hourly_loads {
            for (zone_name, &(_hl, cl)) in zone_loads {
                let current_peak = zone_peak_cooling.get(zone_name).copied().unwrap_or(0.0);
                if cl > current_peak {
                    zone_peak_cooling.insert(zone_name.clone(), cl);
                    zone_cooling_dd.insert(zone_name.clone(), dd.name.clone());
                    zone_cooling_peak_hour.insert(zone_name.clone(), *hour);
                }
            }
        }
    }

    // ── Calculate zone airflows from peak loads ─────────────────────────
    //
    // Apply sizing factors (matching E+ Sizing:Parameters behaviour):
    //   - Heating sizing factor scales the heating design load & airflow
    //   - Cooling sizing factor scales the cooling design load & airflow
    //
    // This ensures equipment is oversized by the specified safety margin,
    // reducing unmet hours during the annual simulation.
    let mut zone_heating_airflow: HashMap<String, f64> = HashMap::new();
    let mut zone_cooling_airflow: HashMap<String, f64> = HashMap::new();
    let mut zone_design_airflow: HashMap<String, f64> = HashMap::new();

    for zone in &env.zones {
        let name = &zone.input.name;
        let heat_sp = zone_heating_setpoints.get(name).copied().unwrap_or(21.0);
        let cool_sp = zone_cooling_setpoints.get(name).copied().unwrap_or(24.0);

        // Heating airflow: Q = m_dot * Cp * (T_supply - T_zone)
        // Apply heating sizing factor to load before computing airflow
        let heat_load = zone_peak_heating.get(name).copied().unwrap_or(0.0) * heating_sizing_factor;
        let dt_heating = (heating_supply_temp - heat_sp).max(5.0);
        let m_heat = if heat_load > 0.0 {
            heat_load / (cp_air * dt_heating)
        } else {
            0.0
        };

        // Cooling airflow: Q = m_dot * Cp * (T_zone - T_supply)
        // Apply cooling sizing factor to load before computing airflow
        let cool_load = zone_peak_cooling.get(name).copied().unwrap_or(0.0) * cooling_sizing_factor;
        let dt_cooling = (cool_sp - cooling_supply_temp).max(5.0);
        let m_cool = if cool_load > 0.0 {
            cool_load / (cp_air * dt_cooling)
        } else {
            0.0
        };

        // Design airflow is the larger of heating and cooling
        let m_design = m_heat.max(m_cool).max(0.01); // minimum 0.01 kg/s

        zone_heating_airflow.insert(name.clone(), m_heat);
        zone_cooling_airflow.insert(name.clone(), m_cool);
        zone_design_airflow.insert(name.clone(), m_design);
    }

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
    zone_heating_setpoints: &HashMap<String, f64>,
    zone_cooling_setpoints: &HashMap<String, f64>,
    latitude: f64,
) -> SystemSizingResult {
    let num_warmup_days = 5;

    let mut coincident_peak_heating = 0.0_f64;
    let mut coincident_peak_cooling = 0.0_f64;
    let mut heating_peak_dd = String::new();
    let mut cooling_peak_dd = String::new();
    let mut heating_peak_hour = 0_u32;
    let mut cooling_peak_hour = 0_u32;

    // ── Run ALL heating design days ──────────────────────────────────────
    let heating_dds: Vec<_> = design_days.iter()
        .filter(|dd| is_heating_design_day(dd))
        .collect();

    for dd in &heating_dds {
        let weather_hours = generate_heating_design_weather(dd);
        log::info!("System sizing: running heating DD '{}' at {:.1}°C", dd.name, dd.design_temp);

        let hourly_loads = run_single_design_day(
            env, dd, &weather_hours, zone_heating_setpoints, num_warmup_days,
        );

        for (hour, zone_loads) in &hourly_loads {
            // Sum all zone heating loads at this timestep (coincident)
            let total_heating: f64 = zone_loads.values().map(|&(hl, _)| hl).sum();
            if total_heating > coincident_peak_heating {
                coincident_peak_heating = total_heating;
                heating_peak_dd = dd.name.clone();
                heating_peak_hour = *hour;
            }
        }
    }

    // ── Run ALL cooling design days ──────────────────────────────────────
    let cooling_dds: Vec<_> = design_days.iter()
        .filter(|dd| is_cooling_design_day(dd))
        .collect();

    for dd in &cooling_dds {
        let weather_hours = generate_cooling_design_weather(dd, latitude);
        log::info!("System sizing: running cooling DD '{}' at {:.1}°C", dd.name, dd.design_temp);

        let hourly_loads = run_single_design_day(
            env, dd, &weather_hours, zone_cooling_setpoints, num_warmup_days,
        );

        for (hour, zone_loads) in &hourly_loads {
            // Sum all zone cooling loads at this timestep (coincident)
            let total_cooling: f64 = zone_loads.values().map(|&(_, cl)| cl).sum();
            if total_cooling > coincident_peak_cooling {
                coincident_peak_cooling = total_cooling;
                cooling_peak_dd = dd.name.clone();
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
        if v.is_empty() { return 0.0; }
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
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
        .max_by(|a, b| a.design_temp.partial_cmp(&b.design_temp).unwrap())
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
    let user_months: std::collections::HashSet<u32> = user_cooling_dds
        .iter()
        .map(|dd| dd.month)
        .collect();

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

        let month_name = ["Jan","Feb","Mar","Apr","May","Jun",
                          "Jul","Aug","Sep","Oct","Nov","Dec"][m_idx];
        let day = MONTH_REPRESENTATIVE_DAYS[m_idx];

        generated.push(DesignDayInput {
            name: format!("{} (auto-{})", anchor.name, month_name),
            design_temp: scaled_temp,
            daily_range: anchor.daily_range,
            humidity_type: anchor.humidity_type.clone(),
            humidity_value: anchor.humidity_value + delta * 0.5, // partial humid scaling
            pressure: anchor.pressure,
            wind_speed: anchor.wind_speed,
            month,
            day,
            day_type: anchor.day_type.clone(),
            internal_gains: anchor.internal_gains.clone(),
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
) -> SizingResult {
    let rho_air = 1.2; // kg/m³ for volume flow conversion

    log::info!("Sizing factors: heating={:.2}, cooling={:.2}",
        heating_sizing_factor, cooling_sizing_factor);

    // ── Gather setpoints from resolved thermostats ───────────────────────
    let mut zone_heating_setpoints: HashMap<String, f64> = HashMap::new();
    let mut zone_cooling_setpoints: HashMap<String, f64> = HashMap::new();
    // Supply temps from air loop controls.  If the caller provides them
    // (extracted from air_loops), use those; otherwise fall back to defaults.
    let (heating_supply_temp, cooling_supply_temp) = supply_temps.unwrap_or((35.0, 13.0));

    for tstat in thermostats {
        for zone_name in &tstat.zones {
            zone_heating_setpoints.insert(zone_name.clone(), tstat.heating_setpoint);
            zone_cooling_setpoints.insert(zone_name.clone(), tstat.cooling_setpoint);
        }
    }

    // For zones not in any zone group, use defaults
    for zone in &env.zones {
        zone_heating_setpoints.entry(zone.input.name.clone()).or_insert(21.0);
        zone_cooling_setpoints.entry(zone.input.name.clone()).or_insert(24.0);
    }

    // ── Auto-generate monthly cooling design days ─────────────────────────
    // The user defines one anchor cooling DD (e.g. July 0.4%). We auto-generate
    // DDs for the remaining months so shoulder-season solar peaks are captured.
    let user_cooling_dds: Vec<&DesignDayInput> = design_days.iter()
        .filter(|dd| is_cooling_design_day(dd))
        .collect();

    let auto_cooling_dds = if !user_cooling_dds.is_empty() && !weather_hours.is_empty() {
        log::info!("Auto-generating monthly cooling DDs from anchor '{}'...",
            user_cooling_dds.iter()
                .max_by(|a, b| a.design_temp.partial_cmp(&b.design_temp).unwrap())
                .map(|dd| dd.name.as_str())
                .unwrap_or("?")
        );
        generate_monthly_cooling_dds(&user_cooling_dds, weather_hours)
    } else {
        Vec::new()
    };

    // Combine user DDs + auto-generated DDs into one complete list
    let all_design_days: Vec<DesignDayInput> = design_days.iter()
        .cloned()
        .chain(auto_cooling_dds.into_iter())
        .collect();

    if all_design_days.len() > design_days.len() {
        log::info!("Total design days: {} user-defined + {} auto-generated = {}",
            design_days.len(),
            all_design_days.len() - design_days.len(),
            all_design_days.len());
    }

    log::info!("══════════════════════════════════════════════════════════");
    log::info!("  STAGE 1: Zone Sizing");
    log::info!("══════════════════════════════════════════════════════════");

    // ── Stage 1: Zone Sizing ─────────────────────────────────────────────
    let zone_sizing = run_zone_sizing(
        env,
        &all_design_days,
        &zone_heating_setpoints,
        &zone_cooling_setpoints,
        heating_supply_temp,
        cooling_supply_temp,
        latitude,
        heating_sizing_factor,
        cooling_sizing_factor,
    );

    // Log zone sizing results
    log::info!("── Zone Sizing Results ─────────────────────────────────");
    for zone in &env.zones {
        let name = &zone.input.name;
        let ph = zone_sizing.zone_peak_heating.get(name).unwrap_or(&0.0);
        let pc = zone_sizing.zone_peak_cooling.get(name).unwrap_or(&0.0);
        let mf = zone_sizing.zone_design_airflow.get(name).unwrap_or(&0.0);
        let hdd = zone_sizing.zone_heating_dd.get(name).map(|s| s.as_str()).unwrap_or("-");
        let hhr = zone_sizing.zone_heating_peak_hour.get(name).unwrap_or(&0);
        let cdd = zone_sizing.zone_cooling_dd.get(name).map(|s| s.as_str()).unwrap_or("-");
        let chr = zone_sizing.zone_cooling_peak_hour.get(name).unwrap_or(&0);
        log::info!(
            "  Zone '{}': htg={:.0}W (DD: {}, hr {}), clg={:.0}W (DD: {}, hr {}), flow={:.3} kg/s",
            name, ph, hdd, hhr, pc, cdd, chr, mf
        );
    }

    log::info!("══════════════════════════════════════════════════════════");
    log::info!("  STAGE 2: System Sizing");
    log::info!("══════════════════════════════════════════════════════════");

    // ── Stage 2: System Sizing ───────────────────────────────────────────
    let system_sizing = run_system_sizing(
        env,
        &all_design_days,
        &zone_heating_setpoints,
        &zone_cooling_setpoints,
        latitude,
    );

    // System capacities with sizing factors
    let system_heating_capacity = system_sizing.coincident_peak_heating * heating_sizing_factor;
    let system_cooling_capacity = system_sizing.coincident_peak_cooling * cooling_sizing_factor;
    let system_airflow: f64 = zone_sizing.zone_design_airflow.values().sum();
    let system_volume_flow = system_airflow / rho_air;

    log::info!("── System Sizing Results ───────────────────────────────");
    log::info!("  Coincident peak heating: {:.0} W ({:.1} kW) on DD '{}' at hour {}",
        system_sizing.coincident_peak_heating,
        system_sizing.coincident_peak_heating / 1000.0,
        system_sizing.heating_peak_dd,
        system_sizing.heating_peak_hour);
    log::info!("  Coincident peak cooling: {:.0} W ({:.1} kW) on DD '{}' at hour {}",
        system_sizing.coincident_peak_cooling,
        system_sizing.coincident_peak_cooling / 1000.0,
        system_sizing.cooling_peak_dd,
        system_sizing.cooling_peak_hour);
    log::info!("  System heating capacity (×{:.0}%): {:.0} W ({:.1} kW)",
        heating_sizing_factor * 100.0, system_heating_capacity, system_heating_capacity / 1000.0);
    log::info!("  System cooling capacity (×{:.0}%): {:.0} W ({:.1} kW)",
        cooling_sizing_factor * 100.0, system_cooling_capacity, system_cooling_capacity / 1000.0);
    log::info!("  System airflow: {:.3} kg/s ({:.4} m³/s, {:.0} CFM)",
        system_airflow, system_volume_flow, system_volume_flow * 2118.88);
    log::info!("══════════════════════════════════════════════════════════");

    // ── Write sizing output files ────────────────────────────────────────
    write_zone_sizing_csv(&zone_sizing, &env.zones, output_dir, input_stem);
    write_system_sizing_csv(
        &zone_sizing, &system_sizing,
        system_heating_capacity, system_cooling_capacity,
        system_airflow, system_volume_flow,
        heating_sizing_factor, cooling_sizing_factor, output_dir, input_stem,
    );

    // Reset zone temperatures for the real simulation
    for zone in &mut env.zones {
        let sp = zone_heating_setpoints.get(&zone.input.name).copied().unwrap_or(21.0);
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
        let hdd = zone_sizing.zone_heating_dd.get(name).map(|s| s.as_str()).unwrap_or("");
        let hhr = zone_sizing.zone_heating_peak_hour.get(name).unwrap_or(&0);
        let pc = zone_sizing.zone_peak_cooling.get(name).unwrap_or(&0.0);
        let cdd = zone_sizing.zone_cooling_dd.get(name).map(|s| s.as_str()).unwrap_or("");
        let chr = zone_sizing.zone_cooling_peak_hour.get(name).unwrap_or(&0);
        let mh = zone_sizing.zone_heating_airflow.get(name).unwrap_or(&0.0);
        let mc = zone_sizing.zone_cooling_airflow.get(name).unwrap_or(&0.0);
        let md = zone_sizing.zone_design_airflow.get(name).unwrap_or(&0.0);
        let vol_flow = md / 1.2; // kg/s to m³/s
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
    lines.push(format!("Heating Sizing Factor,{:.2},,", heating_sizing_factor));
    lines.push(format!("Cooling Sizing Factor,{:.2},,", cooling_sizing_factor));
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
    lines.push(format!("Total (non-coincident),{:.1},{:.1},", total_zone_heating, total_zone_cooling));
    lines.push(String::new());

    // System sizing
    lines.push("--- System Sizing (Coincident Peaks) ---,,,".to_string());
    lines.push(format!("Coincident Peak Heating,{:.1},W,DD: {} at hour {}",
        system_sizing.coincident_peak_heating,
        system_sizing.heating_peak_dd,
        system_sizing.heating_peak_hour));
    lines.push(format!("Coincident Peak Cooling,{:.1},W,DD: {} at hour {}",
        system_sizing.coincident_peak_cooling,
        system_sizing.cooling_peak_dd,
        system_sizing.cooling_peak_hour));
    lines.push(String::new());

    lines.push("--- Sized System Capacities ---,,,".to_string());
    lines.push(format!("System Heating Capacity,{:.1},W,(coincident peak x {:.0}%)",
        system_heating_cap, heating_sizing_factor * 100.0));
    lines.push(format!("System Heating Capacity,{:.2},kW,", system_heating_cap / 1000.0));
    lines.push(format!("System Cooling Capacity,{:.1},W,(coincident peak x {:.0}%)",
        system_cooling_cap, cooling_sizing_factor * 100.0));
    lines.push(format!("System Cooling Capacity,{:.2},kW,", system_cooling_cap / 1000.0));
    lines.push(format!("System Cooling Capacity,{:.2},tons,", system_cooling_cap / 3517.0));
    lines.push(format!("System Airflow,{:.4},kg/s,(sum of zone design airflows)",
        system_airflow));
    lines.push(format!("System Airflow,{:.4},m3/s,", system_volume_flow));
    lines.push(format!("System Airflow,{:.0},CFM,", system_volume_flow * 2118.88));

    match std::fs::write(&path, lines.join("\n") + "\n") {
        Ok(()) => log::info!("System sizing results written to: {}", path.display()),
        Err(e) => log::warn!("Failed to write system sizing CSV: {}", e),
    }
}
