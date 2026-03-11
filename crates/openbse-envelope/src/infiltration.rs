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
    fn test_constant_infiltration() {
        let input = InfiltrationInput {
            design_flow_rate: 0.05,
            ..Default::default()
        };
        let flow = calc_infiltration_flow(&input, 100.0, 21.0, 0.0, 5.0);
        // With default coefficients (A=1, B=C=D=0), flow = design_flow_rate
        assert_relative_eq!(flow, 0.05, max_relative = 0.001);
    }

    #[test]
    fn test_ach_conversion() {
        let input = InfiltrationInput {
            air_changes_per_hour: 0.5,
            ..Default::default()
        };
        let flow = calc_infiltration_flow(&input, 360.0, 21.0, 0.0, 5.0);
        // 0.5 ACH * 360 m³ / 3600 s = 0.05 m³/s
        assert_relative_eq!(flow, 0.05, max_relative = 0.001);
    }

    #[test]
    fn test_wind_dependent_infiltration() {
        let input = InfiltrationInput {
            design_flow_rate: 0.05,
            constant_coefficient: 0.0,
            wind_coefficient: 0.01,
            ..Default::default()
        };
        let flow_calm = calc_infiltration_flow(&input, 100.0, 21.0, 0.0, 1.0);
        let flow_windy = calc_infiltration_flow(&input, 100.0, 21.0, 0.0, 10.0);
        assert!(flow_windy > flow_calm);
    }

    #[test]
    fn test_no_infiltration() {
        let input = InfiltrationInput::default();
        let flow = calc_infiltration_flow(&input, 100.0, 21.0, 0.0, 5.0);
        assert_eq!(flow, 0.0);
    }
}
