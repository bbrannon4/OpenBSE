//! Ground temperature model using the Kusuda-Achenbach equation.
//!
//! The Kusuda equation models ground temperature as a function of depth
//! and time of year using sinusoidal annual temperature variation:
//!
//!   T(z,t) = T_mean - A * exp(-z * sqrt(pi / (365 * alpha)))
//!            * cos(2*pi/365 * (t - t_shift - z/2 * sqrt(365 / (pi * alpha))))
//!
//! Where:
//!   T_mean = annual mean surface temperature [°C]
//!   A = annual temperature amplitude [°C]
//!   z = depth below surface [m]
//!   t = day of year [days]
//!   t_shift = day of minimum surface temperature [days]
//!   alpha = soil thermal diffusivity [m²/day]
//!
//! Reference: Kusuda & Achenbach (1965), ASHRAE Fundamentals.

use openbse_weather::WeatherHour;

/// Ground temperature model parameters.
///
/// Supports two modes:
/// 1. **Monthly table** (preferred): Direct monthly temperatures from EPW header,
///    matching EnergyPlus `Site:GroundTemperature:FCfactorMethod`. Linear interpolation
///    between months.
/// 2. **Kusuda-Achenbach** (fallback): Analytical sinusoidal model when EPW ground
///    temps are not available.
#[derive(Debug, Clone)]
pub struct GroundTempModel {
    /// Annual mean ground surface temperature [°C]
    pub t_mean: f64,
    /// Annual surface temperature amplitude [°C] (half of peak-to-peak)
    pub amplitude: f64,
    /// Day of year of minimum surface temperature [days] (typically ~35 for northern hemisphere)
    pub phase_day: f64,
    /// Soil thermal diffusivity [m²/day]
    pub soil_diffusivity: f64,
    /// Depth below surface [m]
    pub depth: f64,
    /// Monthly ground temperatures [°C], January through December.
    /// When present, these are used instead of Kusuda equation (linear interpolation
    /// between mid-month values).
    ///
    /// Source: EPW header GROUND TEMPERATURES at 0.5 m depth, matching E+'s
    /// `Site:GroundTemperature:FCfactorMethod` default behavior.
    pub monthly_temps: Option<[f64; 12]>,
}

impl Default for GroundTempModel {
    fn default() -> Self {
        Self {
            t_mean: 10.0,
            amplitude: 10.0,
            phase_day: 35.0,       // early February minimum for northern hemisphere
            soil_diffusivity: 0.04, // typical soil ~0.04 m²/day (4.6e-7 m²/s)
            depth: 0.5,            // 0.5 m depth (matches E+ FCfactorMethod default)
            monthly_temps: None,   // Use Kusuda when no EPW ground temps available
        }
    }
}

impl GroundTempModel {
    /// Create a ground temperature model from weather data.
    ///
    /// Computes annual mean temperature, amplitude, and phase from monthly
    /// averages of dry-bulb temperature (as proxy for ground surface temp).
    pub fn from_weather_hours(hours: &[WeatherHour]) -> Self {
        if hours.is_empty() {
            return Self::default();
        }

        // Compute monthly averages
        let mut month_sums = [0.0f64; 12];
        let mut month_counts = [0u32; 12];

        for hour in hours {
            let m = (hour.month as usize).saturating_sub(1).min(11);
            month_sums[m] += hour.dry_bulb;
            month_counts[m] += 1;
        }

        let mut monthly_avg = [0.0f64; 12];
        for i in 0..12 {
            if month_counts[i] > 0 {
                monthly_avg[i] = month_sums[i] / month_counts[i] as f64;
            }
        }

        // Annual mean
        let valid_months: Vec<f64> = monthly_avg.iter()
            .enumerate()
            .filter(|(i, _)| month_counts[*i] > 0)
            .map(|(_, &v)| v)
            .collect();
        let t_mean = if !valid_months.is_empty() {
            valid_months.iter().sum::<f64>() / valid_months.len() as f64
        } else {
            10.0
        };

        // Amplitude: half of peak-to-peak monthly variation
        let t_max = monthly_avg.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        let t_min = monthly_avg.iter().cloned().fold(f64::INFINITY, f64::min);
        let amplitude = (t_max - t_min) / 2.0;

        // Phase: day of minimum temperature
        // Find the month with minimum temp, convert to day of year (mid-month)
        let days_in_months: [f64; 12] = [
            31.0, 28.0, 31.0, 30.0, 31.0, 30.0,
            31.0, 31.0, 30.0, 31.0, 30.0, 31.0,
        ];
        let mut min_month = 0usize;
        let mut min_temp = f64::INFINITY;
        for (i, &t) in monthly_avg.iter().enumerate() {
            if month_counts[i] > 0 && t < min_temp {
                min_temp = t;
                min_month = i;
            }
        }
        let mut phase_day = 0.0;
        for i in 0..min_month {
            phase_day += days_in_months[i];
        }
        phase_day += days_in_months[min_month] / 2.0; // mid-month

        Self {
            t_mean,
            amplitude,
            phase_day,
            soil_diffusivity: 0.04, // typical soil
            depth: 0.5,             // 0.5 m depth (matches E+ FCfactorMethod default)
            monthly_temps: None,    // caller can set from EPW ground temps
        }
    }

    /// Ground temperature at a given day of year [°C].
    ///
    /// If monthly temperatures are available (from EPW header), uses linear
    /// interpolation between mid-month values (matching E+ FCfactorMethod).
    /// Otherwise falls back to Kusuda-Achenbach equation.
    pub fn temperature(&self, day_of_year: f64) -> f64 {
        // Prefer monthly table from EPW (matches E+ FCfactorMethod behavior)
        if let Some(ref temps) = self.monthly_temps {
            return Self::interpolate_monthly(temps, day_of_year);
        }

        // Fallback: Kusuda-Achenbach equation
        let z = self.depth;
        let alpha = self.soil_diffusivity;

        if alpha <= 0.0 {
            return self.t_mean;
        }

        // Damping factor: exp(-z * sqrt(pi / (365 * alpha)))
        let damping_arg = z * (std::f64::consts::PI / (365.0 * alpha)).sqrt();
        let damping = (-damping_arg).exp();

        // Phase shift due to depth: z/2 * sqrt(365 / (pi * alpha))
        let phase_shift = z / 2.0 * (365.0 / (std::f64::consts::PI * alpha)).sqrt();

        // Cosine argument
        let cos_arg = 2.0 * std::f64::consts::PI / 365.0
            * (day_of_year - self.phase_day - phase_shift);

        self.t_mean - self.amplitude * damping * cos_arg.cos()
    }

    /// Linear interpolation of monthly ground temperatures.
    ///
    /// Each month's value is anchored at mid-month. Between mid-month points,
    /// linearly interpolate. Wraps around Dec→Jan for continuity.
    fn interpolate_monthly(temps: &[f64; 12], day_of_year: f64) -> f64 {
        // Mid-month day of year for each month (0-indexed from Jan 1 = day 0)
        let days_in_month: [f64; 12] = [
            31.0, 28.0, 31.0, 30.0, 31.0, 30.0,
            31.0, 31.0, 30.0, 31.0, 30.0, 31.0,
        ];
        let mut mid_days = [0.0f64; 12];
        let mut cum = 0.0;
        for m in 0..12 {
            mid_days[m] = cum + days_in_month[m] / 2.0;
            cum += days_in_month[m];
        }

        // Wrap day_of_year to [0, 365)
        let doy = ((day_of_year % 365.0) + 365.0) % 365.0;

        // Find surrounding months
        if doy <= mid_days[0] {
            // Before Jan mid-month: interpolate between Dec and Jan
            let dec_mid = mid_days[11] - 365.0; // negative (wrap)
            let span = mid_days[0] - dec_mid;
            let frac = (doy - dec_mid) / span;
            temps[11] + frac * (temps[0] - temps[11])
        } else if doy >= mid_days[11] {
            // After Dec mid-month: interpolate between Dec and Jan
            let jan_mid = mid_days[0] + 365.0; // wrap forward
            let span = jan_mid - mid_days[11];
            let frac = (doy - mid_days[11]) / span;
            temps[11] + frac * (temps[0] - temps[11])
        } else {
            // Between two mid-month points
            for m in 0..11 {
                if doy >= mid_days[m] && doy < mid_days[m + 1] {
                    let span = mid_days[m + 1] - mid_days[m];
                    let frac = (doy - mid_days[m]) / span;
                    return temps[m] + frac * (temps[m + 1] - temps[m]);
                }
            }
            temps[11] // Fallback (shouldn't reach here)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_relative_eq;

    #[test]
    fn test_annual_mean_at_great_depth() {
        // At infinite depth, temperature should equal annual mean
        let model = GroundTempModel {
            t_mean: 15.0,
            amplitude: 12.0,
            phase_day: 35.0,
            soil_diffusivity: 0.04,
            depth: 100.0, // very deep
            monthly_temps: None, // Force Kusuda
        };
        let t = model.temperature(180.0);
        assert_relative_eq!(t, 15.0, epsilon = 0.1);
    }

    #[test]
    fn test_surface_amplitude() {
        // At surface (depth=0), amplitude should equal the input amplitude
        let model = GroundTempModel {
            t_mean: 10.0,
            amplitude: 15.0,
            phase_day: 0.0, // minimum at day 0
            soil_diffusivity: 0.04,
            depth: 0.0,
            monthly_temps: None, // Force Kusuda
        };

        // At day 0 (minimum): T = T_mean - A * cos(0) = 10 - 15 = -5
        let t_min = model.temperature(0.0);
        assert_relative_eq!(t_min, -5.0, epsilon = 0.1);

        // At day 182.5 (half year, maximum): T = T_mean - A * cos(pi) = 10 + 15 = 25
        let t_max = model.temperature(182.5);
        assert_relative_eq!(t_max, 25.0, epsilon = 0.1);

        // Range should be 2 * amplitude = 30
        assert_relative_eq!(t_max - t_min, 30.0, epsilon = 0.2);
    }

    #[test]
    fn test_damping_with_depth() {
        // Deeper soil should have smaller temperature swings
        let shallow = GroundTempModel {
            t_mean: 10.0,
            amplitude: 15.0,
            phase_day: 0.0,
            soil_diffusivity: 0.04,
            depth: 0.5,
            monthly_temps: None,
        };
        let deep = GroundTempModel {
            t_mean: 10.0,
            amplitude: 15.0,
            phase_day: 0.0,
            soil_diffusivity: 0.04,
            depth: 3.0,
            monthly_temps: None,
        };

        // Compute ranges over a year
        let shallow_range = (0..365)
            .map(|d| shallow.temperature(d as f64))
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), t| (lo.min(t), hi.max(t)));
        let deep_range = (0..365)
            .map(|d| deep.temperature(d as f64))
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), t| (lo.min(t), hi.max(t)));

        let shallow_amp = shallow_range.1 - shallow_range.0;
        let deep_amp = deep_range.1 - deep_range.0;

        // Deep soil should have smaller amplitude than shallow
        assert!(deep_amp < shallow_amp,
            "Deep amplitude {} should be less than shallow {}", deep_amp, shallow_amp);
    }

    #[test]
    fn test_from_synthetic_weather() {
        // Create synthetic hourly data: sinusoidal annual cycle
        // Mean = 12°C, amplitude = 10°C (max 22, min 2)
        let mut hours = Vec::new();
        let days_in_months: [u32; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
        let mut day_count = 0u32;

        for (m, &days) in days_in_months.iter().enumerate() {
            for d in 1..=days {
                for h in 1..=24 {
                    let doy = day_count + d;
                    // Sinusoidal: minimum in January (day ~15), maximum in July
                    let t = 12.0 - 10.0 * (2.0 * std::f64::consts::PI
                        * (doy as f64 - 15.0) / 365.0).cos();
                    hours.push(WeatherHour {
                        year: 2023,
                        month: (m + 1) as u32,
                        day: d,
                        hour: h,
                        dry_bulb: t,
                        dew_point: 0.0,
                        rel_humidity: 50.0,
                        pressure: 101325.0,
                        global_horiz_rad: 0.0,
                        direct_normal_rad: 0.0,
                        diffuse_horiz_rad: 0.0,
                        wind_speed: 3.0,
                        wind_direction: 0.0,
                        horiz_ir_rad: 0.0,
                        opaque_sky_cover: 0.0,
                    });
                }
            }
            day_count += days;
        }

        let model = GroundTempModel::from_weather_hours(&hours);

        // Mean should be close to 12°C
        assert_relative_eq!(model.t_mean, 12.0, epsilon = 1.0);
        // Amplitude should be close to 10°C
        assert_relative_eq!(model.amplitude, 10.0, epsilon = 2.0);
        // Phase should be near January (coldest month)
        assert!(model.phase_day < 60.0, "Phase day {} should be in winter", model.phase_day);
    }

    #[test]
    fn test_monthly_interpolation() {
        // Test that monthly table interpolation returns mid-month values exactly
        // and interpolates between them correctly.
        let monthly = [
            -0.09, -1.03, 0.64, 3.26, 10.11, 15.39,
            18.96, 20.04, 18.19, 14.09, 8.61, 3.52,
        ]; // Denver EPW 0.5 m ground temps

        let model = GroundTempModel {
            monthly_temps: Some(monthly),
            ..Default::default()
        };

        // Mid-January (day ~15.5): should be close to Jan value (-0.09)
        let t_jan = model.temperature(15.5);
        assert_relative_eq!(t_jan, -0.09, epsilon = 0.2);

        // Mid-July (day ~196.5): should be close to Jul value (18.96)
        let t_jul = model.temperature(196.5);
        assert_relative_eq!(t_jul, 18.96, epsilon = 0.2);

        // Annual mean of monthly temps should equal average of 12 values
        let mean: f64 = monthly.iter().sum::<f64>() / 12.0;
        let mut sum = 0.0f64;
        for d in 0..365 {
            sum += model.temperature(d as f64);
        }
        let model_mean = sum / 365.0;
        assert_relative_eq!(model_mean, mean, epsilon = 0.5);
    }

    #[test]
    fn test_monthly_temps_override_kusuda() {
        // When monthly_temps is set, Kusuda parameters should be ignored
        let monthly = [18.0; 12]; // Constant 18°C year-round

        let model = GroundTempModel {
            t_mean: 0.0,      // Would give very different result if Kusuda used
            amplitude: 20.0,
            monthly_temps: Some(monthly),
            ..Default::default()
        };

        // Should always return 18°C regardless of Kusuda params
        assert_relative_eq!(model.temperature(0.0), 18.0, epsilon = 0.01);
        assert_relative_eq!(model.temperature(180.0), 18.0, epsilon = 0.01);
    }
}
