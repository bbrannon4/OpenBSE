//! Weather file reader and design day processing.
//!
//! Reads weather files in EPW (EnergyPlus Weather) and TMY3 CSV formats,
//! providing hourly weather data for simulation. Supports multiple weather
//! files for multi-year runs.

use openbse_psychrometrics::{self as psych, MoistAirState};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Read};
use std::path::Path;

// ─── EPW Data Structures ────────────────────────────────────────────────────

/// Location metadata from EPW header.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WeatherLocation {
    pub city: String,
    pub state_province: String,
    pub country: String,
    pub source: String,
    pub wmo: String,
    pub latitude: f64,
    pub longitude: f64,
    pub time_zone: f64,
    pub elevation: f64,
}

/// Design day specification for autosizing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesignDay {
    pub name: String,
    /// Maximum (cooling) or minimum (heating) dry-bulb temperature [°C]
    pub design_temp: f64,
    /// Daily temperature range [°C] (for cooling design days)
    pub daily_range: f64,
    /// Humidity condition type and value
    pub humidity_type: HumidityType,
    /// Barometric pressure [Pa]
    pub pressure: f64,
    /// Wind speed [m/s]
    pub wind_speed: f64,
    /// Wind direction [degrees from north]
    pub wind_direction: f64,
    /// Month of design day [1-12]
    pub month: u32,
    /// Day of month
    pub day: u32,
    /// Day type
    pub day_type: DesignDayType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum HumidityType {
    WetBulb(f64),
    DewPoint(f64),
    HumidityRatio(f64),
    Enthalpy(f64),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DesignDayType {
    SummerDesign,
    WinterDesign,
}

/// One hour of weather data from an EPW file.
#[derive(Debug, Clone, Copy)]
pub struct WeatherHour {
    pub year: u32,
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    /// Dry-bulb temperature [°C]
    pub dry_bulb: f64,
    /// Dew-point temperature [°C]
    pub dew_point: f64,
    /// Relative humidity [%]
    pub rel_humidity: f64,
    /// Atmospheric pressure [Pa]
    pub pressure: f64,
    /// Global horizontal radiation [Wh/m²]
    pub global_horiz_rad: f64,
    /// Direct normal radiation [Wh/m²]
    pub direct_normal_rad: f64,
    /// Diffuse horizontal radiation [Wh/m²]
    pub diffuse_horiz_rad: f64,
    /// Wind speed [m/s]
    pub wind_speed: f64,
    /// Wind direction [degrees]
    pub wind_direction: f64,
    /// Horizontal infrared radiation [Wh/m²]
    pub horiz_ir_rad: f64,
    /// Opaque sky cover [tenths]
    pub opaque_sky_cover: f64,
}

impl WeatherHour {
    /// Convert to a MoistAirState for the simulation engine.
    pub fn to_air_state(&self) -> MoistAirState {
        let w = psych::w_fn_tdb_rh_pb(
            self.dry_bulb,
            self.rel_humidity / 100.0,
            self.pressure,
        );
        MoistAirState::new(self.dry_bulb, w, self.pressure)
    }

    /// Linearly interpolate between two hourly weather records.
    ///
    /// EPW data represents the period **ending** at the recorded hour.
    /// For sub-hourly timesteps, E+ linearly interpolates between the
    /// previous hour (`self`) and the current hour (`next`) using:
    ///
    ///   value = (1 - frac) × prev + frac × next
    ///
    /// where `frac` is the fractional position within the hour (0.0 to 1.0).
    ///
    /// **Solar radiation is NOT interpolated** — it's an integrated energy
    /// value (Wh/m²) for the entire hour, so it's held constant.
    ///
    /// Reference: EnergyPlus WeatherManager.cc `InterpretWeatherDataLine`.
    pub fn interpolate(&self, next: &WeatherHour, frac: f64) -> WeatherHour {
        let f = frac.clamp(0.0, 1.0);
        let inv = 1.0 - f;
        WeatherHour {
            // Use the target hour's date/time metadata
            year: next.year,
            month: next.month,
            day: next.day,
            hour: next.hour,
            // Interpolate thermodynamic state variables
            dry_bulb: inv * self.dry_bulb + f * next.dry_bulb,
            dew_point: inv * self.dew_point + f * next.dew_point,
            rel_humidity: inv * self.rel_humidity + f * next.rel_humidity,
            pressure: inv * self.pressure + f * next.pressure,
            // Solar radiation: NOT interpolated (integrated hourly Wh/m²)
            // Use the current hour's values for the entire period
            global_horiz_rad: next.global_horiz_rad,
            direct_normal_rad: next.direct_normal_rad,
            diffuse_horiz_rad: next.diffuse_horiz_rad,
            // Wind: interpolate speed, use current hour's direction
            // (direction interpolation is problematic near 0/360 boundary)
            wind_speed: inv * self.wind_speed + f * next.wind_speed,
            wind_direction: next.wind_direction,
            // Longwave radiation and sky cover: interpolate
            horiz_ir_rad: inv * self.horiz_ir_rad + f * next.horiz_ir_rad,
            opaque_sky_cover: inv * self.opaque_sky_cover + f * next.opaque_sky_cover,
        }
    }
}

/// Ground temperatures at a specific depth, parsed from EPW header.
///
/// EPW files contain undisturbed ground temperatures at up to 3 depths
/// (typically 0.5 m, 2.0 m, and 4.0 m), computed by the weather converter
/// from annual air temperature patterns.
///
/// For F-factor slab-on-grade floors, EnergyPlus uses the 0.5 m depth
/// temperatures as the default for `Site:GroundTemperature:FCfactorMethod`.
#[derive(Debug, Clone)]
pub struct EpwGroundTemps {
    /// Depth below surface [m]
    pub depth: f64,
    /// Monthly temperatures [°C], January through December (12 values)
    pub monthly_temps: [f64; 12],
}

/// Complete weather data for a simulation.
#[derive(Debug)]
pub struct WeatherData {
    pub location: WeatherLocation,
    pub hours: Vec<WeatherHour>,
    pub design_days: Vec<DesignDay>,
    /// Day of week for January 1 in the weather data.
    /// 1=Monday, 2=Tuesday, ..., 6=Saturday, 7=Sunday.
    /// Parsed from the EPW DATA PERIODS header line. Defaults to 1 (Monday).
    pub start_day_of_week: u32,
    /// Ground temperatures at various depths, parsed from EPW header.
    /// Typically contains 3 depth profiles (0.5 m, 2.0 m, 4.0 m).
    pub ground_temperatures: Vec<EpwGroundTemps>,
}

impl WeatherData {
    /// Convert hourly data to the format expected by the simulation runner.
    pub fn to_simulation_hours(&self) -> Vec<(MoistAirState, f64)> {
        self.hours
            .iter()
            .map(|h| (h.to_air_state(), h.wind_speed))
            .collect()
    }
}

// ─── EPW Parser ──────────────────────────────────────────────────────────────

/// Parse an EPW weather file from a reader.
pub fn read_epw<R: Read>(reader: R) -> Result<WeatherData, WeatherError> {
    let buf = BufReader::new(reader);
    let mut lines = buf.lines();

    // Parse header (8 header lines)
    let location = parse_location_header(&mut lines)?;

    // Skip header lines 2-7, but parse specific lines:
    //   Line 2 (index 0): DESIGN CONDITIONS (skip)
    //   Line 3 (index 1): TYPICAL/EXTREME PERIODS (skip)
    //   Line 4 (index 2): GROUND TEMPERATURES (parse!)
    //   Line 5 (index 3): HOLIDAYS/DAYLIGHT SAVINGS (skip)
    //   Line 6 (index 4): COMMENTS 1 (skip)
    //   Line 7 (index 5): COMMENTS 2 (skip)
    //   Line 8 (index 6): DATA PERIODS (parse for start day of week)
    let mut start_day_of_week = 1u32; // Default: Monday
    let mut ground_temperatures: Vec<EpwGroundTemps> = Vec::new();
    for i in 0..7 {
        if let Some(Ok(line)) = lines.next() {
            // Line 4 (index 2): GROUND TEMPERATURES
            // Format: GROUND TEMPERATURES,N_depths,depth1,soil_cond,soil_dens,soil_cp,
            //         Jan,Feb,Mar,Apr,May,Jun,Jul,Aug,Sep,Oct,Nov,Dec[,depth2,...]
            // Each depth profile has 16 fields: depth, conductivity, density, specific_heat,
            // then 12 monthly temperatures.
            if i == 2 && line.starts_with("GROUND TEMPERATURES") {
                let fields: Vec<&str> = line.split(',').collect();
                if fields.len() >= 2 {
                    let n_depths: usize = fields[1].trim().parse().unwrap_or(0);
                    // Each depth has 16 fields after the header (depth + 3 soil props + 12 temps)
                    let mut offset = 2; // Start after "GROUND TEMPERATURES,N"
                    for _ in 0..n_depths {
                        if offset + 16 <= fields.len() {
                            let depth: f64 = fields[offset].trim().parse().unwrap_or(0.0);
                            // Skip soil_conductivity, soil_density, soil_specific_heat
                            let temp_start = offset + 4; // 12 monthly temps
                            let mut monthly_temps = [0.0f64; 12];
                            for m in 0..12 {
                                if temp_start + m < fields.len() {
                                    monthly_temps[m] = fields[temp_start + m]
                                        .trim().parse().unwrap_or(0.0);
                                }
                            }
                            ground_temperatures.push(EpwGroundTemps {
                                depth,
                                monthly_temps,
                            });
                            offset += 16;
                        } else {
                            break;
                        }
                    }
                }
            }

            // Line 8 (index 6): DATA PERIODS
            // Format: DATA PERIODS,N,timesteps,Name,StartDayOfWeek,StartDate,EndDate
            if i == 6 && line.starts_with("DATA PERIODS") {
                let fields: Vec<&str> = line.split(',').collect();
                if fields.len() >= 5 {
                    start_day_of_week = match fields[4].trim().to_lowercase().as_str() {
                        "monday"    => 1,
                        "tuesday"   => 2,
                        "wednesday" => 3,
                        "thursday"  => 4,
                        "friday"    => 5,
                        "saturday"  => 6,
                        "sunday"    => 7,
                        _ => 1, // Default to Monday
                    };
                }
            }
        }
    }

    // Parse hourly data
    let mut hours = Vec::with_capacity(8760);
    for line_result in lines {
        let line = line_result.map_err(|e| WeatherError::IoError(e.to_string()))?;
        if line.trim().is_empty() {
            continue;
        }
        match parse_data_line(&line) {
            Ok(hour) => hours.push(hour),
            Err(_) => continue, // Skip malformed lines
        }
    }

    if hours.is_empty() {
        return Err(WeatherError::NoData);
    }

    Ok(WeatherData {
        location,
        hours,
        design_days: Vec::new(), // Design days come from YAML input, not EPW
        start_day_of_week,
        ground_temperatures,
    })
}

/// Read an EPW file from a path.
pub fn read_epw_file(path: &Path) -> Result<WeatherData, WeatherError> {
    let file = std::fs::File::open(path)
        .map_err(|e| WeatherError::IoError(format!("{}: {}", path.display(), e)))?;
    read_epw(file)
}

fn parse_location_header(
    lines: &mut impl Iterator<Item = Result<String, std::io::Error>>,
) -> Result<WeatherLocation, WeatherError> {
    let line = lines
        .next()
        .ok_or(WeatherError::InvalidFormat("Missing location header".into()))?
        .map_err(|e| WeatherError::IoError(e.to_string()))?;

    let fields: Vec<&str> = line.split(',').collect();
    if fields.len() < 10 {
        return Err(WeatherError::InvalidFormat(
            "Location header has too few fields".into(),
        ));
    }

    Ok(WeatherLocation {
        city: fields[1].to_string(),
        state_province: fields[2].to_string(),
        country: fields[3].to_string(),
        source: fields[4].to_string(),
        wmo: fields[5].to_string(),
        latitude: fields[6].parse().unwrap_or(0.0),
        longitude: fields[7].parse().unwrap_or(0.0),
        time_zone: fields[8].parse().unwrap_or(0.0),
        elevation: fields[9].parse().unwrap_or(0.0),
    })
}

fn parse_data_line(line: &str) -> Result<WeatherHour, WeatherError> {
    let fields: Vec<&str> = line.split(',').collect();
    if fields.len() < 35 {
        return Err(WeatherError::InvalidFormat(
            "Data line has too few fields".into(),
        ));
    }

    let parse_f64 = |idx: usize| -> f64 {
        fields[idx].trim().parse().unwrap_or(0.0)
    };
    let parse_u32 = |idx: usize| -> u32 {
        fields[idx].trim().parse().unwrap_or(0)
    };

    Ok(WeatherHour {
        year: parse_u32(0),
        month: parse_u32(1),
        day: parse_u32(2),
        hour: parse_u32(3),
        // Field 4 = minute, Field 5 = data source flags
        dry_bulb: parse_f64(6),
        dew_point: parse_f64(7),
        rel_humidity: parse_f64(8),
        pressure: parse_f64(9),
        // Field 10 = extraterrestrial horiz rad
        // Field 11 = extraterrestrial direct normal rad
        horiz_ir_rad: parse_f64(12),
        global_horiz_rad: parse_f64(13),
        direct_normal_rad: parse_f64(14),
        diffuse_horiz_rad: parse_f64(15),
        // Fields 16-19 = illuminance data
        wind_direction: parse_f64(20),
        wind_speed: parse_f64(21),
        // Field 22 = total sky cover
        opaque_sky_cover: parse_f64(23),
        // Fields 24+ = visibility, ceiling, weather codes, etc.
    })
}

// ─── TMY3 CSV Parser ────────────────────────────────────────────────────────

/// Parse a TMY3 CSV weather file from a reader.
///
/// TMY3 CSV format (used by ASHRAE Standard 140):
///   Line 1: WMO,"City",State,TimeZone,Latitude,Longitude,Elevation
///   Line 2: Column headers with source/uncertainty triplets
///   Lines 3+: Data rows
///
/// Key columns (0-indexed, skipping source/uncertainty fields):
///   0: Date (MM/DD/YYYY)
///   1: Time (HH:MM)
///   4: GHI (W/m²)
///   7: DNI (W/m²)
///   10: DHI (W/m²)
///   25: TotCld (tenths)
///   28: OpqCld (tenths)
///   31: Dry-bulb (°C)
///   34: Dew-point (°C)
///   37: RHum (%)
///   40: Pressure (mbar)
///   43: Wdir (degrees)
///   46: Wspd (m/s)
pub fn read_tmy3<R: Read>(reader: R) -> Result<WeatherData, WeatherError> {
    let buf = BufReader::new(reader);
    let mut lines = buf.lines();

    // Line 1: Location header
    // Format: 725650,"DENVER INTL AP",CO,-7.0,39.833,-104.650,1650
    let loc_line = lines
        .next()
        .ok_or(WeatherError::InvalidFormat("Missing TMY3 location header".into()))?
        .map_err(|e| WeatherError::IoError(e.to_string()))?;

    let location = parse_tmy3_location(&loc_line)?;

    // Line 2: Column headers (skip)
    lines.next();

    // Parse hourly data
    let mut hours = Vec::with_capacity(8760);
    for line_result in lines {
        let line = line_result.map_err(|e| WeatherError::IoError(e.to_string()))?;
        if line.trim().is_empty() {
            continue;
        }
        match parse_tmy3_data_line(&line) {
            Ok(hour) => hours.push(hour),
            Err(_) => continue,
        }
    }

    if hours.is_empty() {
        return Err(WeatherError::NoData);
    }

    Ok(WeatherData {
        location,
        hours,
        design_days: Vec::new(),
        start_day_of_week: 1,  // TMY3 doesn't specify; default Monday
        ground_temperatures: Vec::new(), // TMY3 doesn't include ground temps
    })
}

/// Read a TMY3 CSV file from a path.
pub fn read_tmy3_file(path: &Path) -> Result<WeatherData, WeatherError> {
    let file = std::fs::File::open(path)
        .map_err(|e| WeatherError::IoError(format!("{}: {}", path.display(), e)))?;
    read_tmy3(file)
}

fn parse_tmy3_location(line: &str) -> Result<WeatherLocation, WeatherError> {
    // Parse fields, handling quoted strings (city name may be quoted)
    let fields = parse_csv_fields(line);
    if fields.len() < 7 {
        return Err(WeatherError::InvalidFormat(
            "TMY3 location header has too few fields".into(),
        ));
    }

    let wmo = fields[0].clone();
    let city = fields[1].trim_matches('"').to_string();
    let state = fields[2].clone();
    let time_zone: f64 = fields[3].parse().unwrap_or(0.0);
    let latitude: f64 = fields[4].parse().unwrap_or(0.0);
    let longitude: f64 = fields[5].parse().unwrap_or(0.0);
    let elevation: f64 = fields[6].parse().unwrap_or(0.0);

    Ok(WeatherLocation {
        city,
        state_province: state,
        country: "USA".to_string(),
        source: "TMY3".to_string(),
        wmo,
        latitude,
        longitude,
        time_zone,
        elevation,
    })
}

/// Parse CSV fields, handling quoted strings.
fn parse_csv_fields(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;

    for ch in line.chars() {
        if ch == '"' {
            in_quotes = !in_quotes;
        } else if ch == ',' && !in_quotes {
            fields.push(current.trim().to_string());
            current = String::new();
        } else {
            current.push(ch);
        }
    }
    fields.push(current.trim().to_string());
    fields
}

fn parse_tmy3_data_line(line: &str) -> Result<WeatherHour, WeatherError> {
    let fields: Vec<&str> = line.split(',').collect();
    // TMY3 CSV has many columns with source/uncertainty triplets.
    // Minimum expected: ~60+ fields
    if fields.len() < 50 {
        return Err(WeatherError::InvalidFormat(
            "TMY3 data line has too few fields".into(),
        ));
    }

    let parse_f64 = |idx: usize| -> f64 {
        fields.get(idx)
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0.0)
    };

    // Parse date: MM/DD/YYYY
    let date_str = fields[0].trim();
    let date_parts: Vec<&str> = date_str.split('/').collect();
    let (month, day, year) = if date_parts.len() == 3 {
        (
            date_parts[0].parse().unwrap_or(1u32),
            date_parts[1].parse().unwrap_or(1u32),
            date_parts[2].parse().unwrap_or(1995u32),
        )
    } else {
        (1, 1, 1995)
    };

    // Parse time: HH:MM — TMY3 uses 01:00-24:00 convention
    let time_str = fields[1].trim();
    let time_parts: Vec<&str> = time_str.split(':').collect();
    let hour: u32 = if !time_parts.is_empty() {
        time_parts[0].parse().unwrap_or(1)
    } else {
        1
    };

    // TMY3 CSV column indices (0-indexed):
    // Each "real" field is followed by source flag and uncertainty, so:
    //   Col 0: Date
    //   Col 1: Time
    //   Col 2: ETR
    //   Col 3: ETRN
    //   Col 4: GHI        Col 5: GHI source    Col 6: GHI uncert
    //   Col 7: DNI        Col 8: DNI source    Col 9: DNI uncert
    //   Col 10: DHI       Col 11: DHI source   Col 12: DHI uncert
    //   Col 13-24: illuminance (skip)
    //   Col 25: TotCld    Col 26: TotCld src   Col 27: TotCld uncert
    //   Col 28: OpqCld    Col 29: OpqCld src   Col 30: OpqCld uncert
    //   Col 31: Dry-bulb  Col 32: src          Col 33: uncert
    //   Col 34: Dew-point  Col 35: src         Col 36: uncert
    //   Col 37: RHum      Col 38: src          Col 39: uncert
    //   Col 40: Pressure  Col 41: src          Col 42: uncert
    //   Col 43: Wdir      Col 44: src          Col 45: uncert
    //   Col 46: Wspd      Col 47: src          Col 48: uncert

    let ghi = parse_f64(4);
    let dni = parse_f64(7);
    let dhi = parse_f64(10);
    let opaque_sky_cover = parse_f64(28);
    let dry_bulb = parse_f64(31);
    let dew_point = parse_f64(34);
    let rel_humidity = parse_f64(37);
    // TMY3 pressure is in mbar; convert to Pa (1 mbar = 100 Pa)
    let pressure_mbar = parse_f64(40);
    let pressure = pressure_mbar * 100.0;
    let wind_direction = parse_f64(43);
    let wind_speed = parse_f64(46);

    // TMY3 doesn't include horizontal IR radiation directly;
    // we set to 0 and rely on Berdahl-Martin sky model in heat_balance.rs
    let horiz_ir_rad = 0.0;

    Ok(WeatherHour {
        year,
        month,
        day,
        hour,
        dry_bulb,
        dew_point,
        rel_humidity,
        pressure,
        global_horiz_rad: ghi,
        direct_normal_rad: dni,
        diffuse_horiz_rad: dhi,
        wind_speed,
        wind_direction,
        horiz_ir_rad,
        opaque_sky_cover,
    })
}

// ─── Auto-Detect Weather File Format ────────────────────────────────────────

/// Read a weather file, auto-detecting format from the file extension.
///
/// Supported formats:
///   - `.epw` — EnergyPlus Weather format
///   - `.csv` — TMY3 CSV format (ASHRAE Standard 140)
///
/// Returns `WeatherError` if the format is not recognized or parsing fails.
pub fn read_weather_file(path: &Path) -> Result<WeatherData, WeatherError> {
    match path.extension().and_then(|e| e.to_str()) {
        Some("epw") => read_epw_file(path),
        Some("csv") | Some("CSV") => read_tmy3_file(path),
        Some(ext) => Err(WeatherError::InvalidFormat(
            format!("Unsupported weather file extension '.{}'; expected .epw or .csv", ext),
        )),
        None => Err(WeatherError::InvalidFormat(
            "Weather file has no extension; expected .epw or .csv".into(),
        )),
    }
}

// ─── Multi-Weather File Support ──────────────────────────────────────────────

/// Load multiple weather files for multi-year simulation.
pub fn read_multiple_epw_files(paths: &[&Path]) -> Result<Vec<WeatherData>, WeatherError> {
    paths.iter().map(|p| read_epw_file(p)).collect()
}

#[derive(Debug, thiserror::Error)]
pub enum WeatherError {
    #[error("IO error: {0}")]
    IoError(String),
    #[error("Invalid EPW format: {0}")]
    InvalidFormat(String),
    #[error("No weather data found in file")]
    NoData,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_epw_location() {
        let epw_content = "LOCATION,Denver,CO,USA,TMY3,725650,39.74,-104.98,-7.0,1614.0\n\
            DESIGN CONDITIONS,0\n\
            TYPICAL/EXTREME PERIODS,0\n\
            GROUND TEMPERATURES,0\n\
            HOLIDAYS/DAYLIGHT SAVINGS,No,0,0,0\n\
            COMMENTS 1,test\n\
            COMMENTS 2,test\n\
            DATA PERIODS,1,1,Data,Sunday,1/1,12/31\n\
            1984,1,1,1,60,A7A7A7A7*0?9?9?9?9?9?9?9*0?9?9?9?9?9?9*,-5.0,-11.0,63,83200,0,0,280,0,0,0,0,0,0,0,320,4.1,10,9,16.1,77777,9,999999999,0,0.039,0,88,0.000,0.0,0\n";

        let data = read_epw(epw_content.as_bytes()).unwrap();
        assert_eq!(data.location.city, "Denver");
        assert_eq!(data.location.state_province, "CO");
        assert!((data.location.latitude - 39.74).abs() < 0.01);
        assert!((data.location.elevation - 1614.0).abs() < 1.0);
        assert_eq!(data.hours.len(), 1);
        assert!((data.hours[0].dry_bulb - (-5.0)).abs() < 0.01);
    }

    #[test]
    fn test_parse_tmy3_location() {
        let tmy3_content = "725650,\"DENVER INTL AP\",CO,-7.0,39.833,-104.650,1650\n\
            Date,Time,ETR,ETRN,GHI,src,unc,DNI,src,unc,DHI,src,unc,GHi,src,unc,DNi,src,unc,DHi,src,unc,Zen,src,unc,Tot,src,unc,Opq,src,unc,Tdb,src,unc,Tdp,src,unc,RH,src,unc,P,src,unc,Wd,src,unc,Ws,src,unc,Hv,src,unc,CH,src,unc\n\
            01/01/1995,01:00,0,0,0,1,0,0,1,0,0,1,0,0,1,0,0,1,0,0,1,0,0,1,0,2,E,9,2,E,9,-18.0,E,9,-19.7,E,9,85,A,7,837,E,9,0,E,9,0.0,E,9,-9900,?,0,20306,E,9\n";

        let data = read_tmy3(tmy3_content.as_bytes()).unwrap();
        assert_eq!(data.location.city, "DENVER INTL AP");
        assert_eq!(data.location.state_province, "CO");
        assert_eq!(data.location.wmo, "725650");
        assert!((data.location.latitude - 39.833).abs() < 0.01);
        assert!((data.location.longitude - (-104.650)).abs() < 0.01);
        assert!((data.location.time_zone - (-7.0)).abs() < 0.01);
        assert!((data.location.elevation - 1650.0).abs() < 1.0);

        assert_eq!(data.hours.len(), 1);
        let h = &data.hours[0];
        assert_eq!(h.month, 1);
        assert_eq!(h.day, 1);
        assert_eq!(h.hour, 1);
        assert!((h.dry_bulb - (-18.0)).abs() < 0.01);
        assert!((h.dew_point - (-19.7)).abs() < 0.01);
        assert!((h.rel_humidity - 85.0).abs() < 0.1);
        assert!((h.pressure - 83700.0).abs() < 100.0); // 837 mbar * 100
        assert!((h.wind_speed - 0.0).abs() < 0.01);
        assert!((h.opaque_sky_cover - 2.0).abs() < 0.1);
    }

    #[test]
    fn test_parse_tmy3_solar_data() {
        // Test that solar radiation fields are correctly parsed
        let tmy3_content = "725650,\"DENVER\",CO,-7.0,39.833,-104.650,1650\n\
            Date,Time,headers...\n\
            01/01/1995,10:00,433,1415,266,1,17,494,1,21,115,1,17,27534,1,17,43873,1,21,14050,1,17,2191,1,32,8,E,9,4,E,9,-2.2,E,9,-7.0,E,9,66,A,7,827,E,9,200,E,9,1.7,E,9,-9900,?,0,11251,E,9\n";

        let data = read_tmy3(tmy3_content.as_bytes()).unwrap();
        let h = &data.hours[0];
        assert_eq!(h.hour, 10);
        assert!((h.global_horiz_rad - 266.0).abs() < 0.1, "GHI should be 266, got {}", h.global_horiz_rad);
        assert!((h.direct_normal_rad - 494.0).abs() < 0.1, "DNI should be 494, got {}", h.direct_normal_rad);
        assert!((h.diffuse_horiz_rad - 115.0).abs() < 0.1, "DHI should be 115, got {}", h.diffuse_horiz_rad);
        assert!((h.dry_bulb - (-2.2)).abs() < 0.01);
        assert!((h.wind_speed - 1.7).abs() < 0.01);
    }

    #[test]
    fn test_csv_fields_with_quotes() {
        let line = r#"725650,"DENVER INTL AP",CO,-7.0,39.833"#;
        let fields = parse_csv_fields(line);
        assert_eq!(fields[0], "725650");
        assert_eq!(fields[1], "DENVER INTL AP");
        assert_eq!(fields[2], "CO");
    }
}
