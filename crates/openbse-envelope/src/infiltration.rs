//! Infiltration models.
//!
//! EnergyPlus "Design Flow Rate" model:
//!   Infiltration = Q_design × (A + B·|ΔT| + C·V_wind + D·V_wind²)
//!
//! Reference: EnergyPlus HeatBalanceAirManager.cc

use serde::{Deserialize, Serialize};

/// Infiltration specification for a zone.
///
/// EnergyPlus "Design Flow Rate" model:
///   `Infiltration = Q_design × (A + B·|ΔT| + C·V_wind + D·V_wind²) × schedule`
///
/// where A = constant_coefficient, B = temperature_coefficient,
/// C = wind_coefficient [s/m], D = wind_squared_coefficient [s²/m²].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InfiltrationInput {
    /// Design infiltration volume flow rate [m³/s]
    #[serde(default)]
    pub design_flow_rate: f64,
    /// Alternative: air changes per hour (converted to m³/s using zone volume)
    #[serde(default)]
    pub air_changes_per_hour: f64,
    /// Constant coefficient A (default 1.0)
    #[serde(default = "default_a", alias = "coeff_a")]
    pub constant_coefficient: f64,
    /// Temperature difference coefficient B [1/°C] — multiplied by |T_zone − T_outdoor|
    #[serde(default, alias = "coeff_b")]
    pub temperature_coefficient: f64,
    /// Wind speed coefficient C [s/m] — multiplied by wind speed V
    #[serde(default, alias = "coeff_c")]
    pub wind_coefficient: f64,
    /// Wind speed squared coefficient D [s²/m²] — multiplied by V²
    #[serde(default, alias = "coeff_d")]
    pub wind_squared_coefficient: f64,
    /// Schedule name for time-varying infiltration multiplier.
    /// The schedule fraction (0.0-1.0) multiplies the computed flow rate.
    /// E.g., PNNL infiltration schedule: 1.0 when HVAC off, 0.25 when on.
    #[serde(default)]
    pub schedule: Option<String>,
}

fn default_a() -> f64 { 1.0 }

impl Default for InfiltrationInput {
    fn default() -> Self {
        Self {
            design_flow_rate: 0.0,
            air_changes_per_hour: 0.0,
            constant_coefficient: 1.0,
            temperature_coefficient: 0.0,
            wind_coefficient: 0.0,
            wind_squared_coefficient: 0.0,
            schedule: None,
        }
    }
}

/// Calculate infiltration volume flow rate [m³/s].
pub fn calc_infiltration_flow(
    input: &InfiltrationInput,
    zone_volume: f64,
    t_zone: f64,
    t_outdoor: f64,
    wind_speed: f64,
) -> f64 {
    let base_flow = if input.design_flow_rate > 0.0 {
        input.design_flow_rate
    } else if input.air_changes_per_hour > 0.0 {
        input.air_changes_per_hour * zone_volume / 3600.0
    } else {
        return 0.0;
    };

    let dt = (t_zone - t_outdoor).abs();
    let factor = input.constant_coefficient
        + input.temperature_coefficient * dt
        + input.wind_coefficient * wind_speed
        + input.wind_squared_coefficient * wind_speed * wind_speed;

    (base_flow * factor).max(0.0)
}

/// Calculate infiltration mass flow rate [kg/s].
pub fn calc_infiltration_mass_flow(
    input: &InfiltrationInput,
    zone_volume: f64,
    t_zone: f64,
    t_outdoor: f64,
    wind_speed: f64,
    rho_outdoor: f64,
) -> f64 {
    calc_infiltration_flow(input, zone_volume, t_zone, t_outdoor, wind_speed) * rho_outdoor
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_relative_eq;

    #[test]
    fn test_design_flow_rate_with_all_coefficients() {
        // Verify the full formula: Q = Q_design × (A + B·|ΔT| + C·V + D·V²)
        // Hand-calculated reference:
        //   Q_design = 0.05 m³/s
        //   A = 0.6, B = 0.03, C = 0.01, D = 0.0
        //   T_zone = 22°C, T_outdoor = 2°C → |ΔT| = 20°C
        //   V_wind = 4.0 m/s
        //   factor = 0.6 + 0.03*20 + 0.01*4 + 0.0*16 = 0.6 + 0.6 + 0.04 = 1.24
        //   Q = 0.05 × 1.24 = 0.062 m³/s
        let input = InfiltrationInput {
            design_flow_rate: 0.05,
            constant_coefficient: 0.6,
            temperature_coefficient: 0.03,
            wind_coefficient: 0.01,
            wind_squared_coefficient: 0.0,
            ..Default::default()
        };
        let flow = calc_infiltration_flow(&input, 100.0, 22.0, 2.0, 4.0);
        assert_relative_eq!(flow, 0.062, max_relative = 1e-10);
    }

    #[test]
    fn test_wind_squared_coefficient() {
        // Verify wind² term: Q = 0.1 × (0 + 0 + 0 + 0.005·V²)
        // At V=6 m/s: factor = 0.005 × 36 = 0.18, Q = 0.1 × 0.18 = 0.018
        let input = InfiltrationInput {
            design_flow_rate: 0.1,
            constant_coefficient: 0.0,
            temperature_coefficient: 0.0,
            wind_coefficient: 0.0,
            wind_squared_coefficient: 0.005,
            ..Default::default()
        };
        let flow = calc_infiltration_flow(&input, 100.0, 20.0, 20.0, 6.0);
        assert_relative_eq!(flow, 0.018, max_relative = 1e-10);
    }

    #[test]
    fn test_ach_conversion() {
        // 0.5 ACH × 360 m³ / 3600 s/hr = 0.05 m³/s (with default A=1)
        let input = InfiltrationInput {
            air_changes_per_hour: 0.5,
            ..Default::default()
        };
        let flow = calc_infiltration_flow(&input, 360.0, 21.0, 0.0, 5.0);
        assert_relative_eq!(flow, 0.05, max_relative = 1e-10);
    }

    #[test]
    fn test_negative_factor_clamped_to_zero() {
        // If coefficients produce a negative factor, flow should be clamped to 0.
        // factor = -0.5 + 0 + 0 + 0 = -0.5, Q = max(0.05 × -0.5, 0) = 0
        let input = InfiltrationInput {
            design_flow_rate: 0.05,
            constant_coefficient: -0.5,
            ..Default::default()
        };
        let flow = calc_infiltration_flow(&input, 100.0, 21.0, 21.0, 0.0);
        assert_eq!(flow, 0.0);
    }

    #[test]
    fn test_no_infiltration_when_no_design_flow() {
        // With no design_flow_rate and no ACH, infiltration is zero regardless of conditions
        let input = InfiltrationInput::default();
        let flow = calc_infiltration_flow(&input, 100.0, 21.0, 0.0, 5.0);
        assert_eq!(flow, 0.0);
    }
}
