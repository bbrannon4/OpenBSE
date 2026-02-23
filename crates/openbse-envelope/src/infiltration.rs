//! Infiltration models.
//!
//! EnergyPlus "Design Flow Rate" model:
//!   Infiltration = Q_design × (A + B·|ΔT| + C·V_wind + D·V_wind²)
//!
//! Reference: EnergyPlus HeatBalanceAirManager.cc

use serde::{Deserialize, Serialize};

/// Infiltration specification for a zone.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InfiltrationInput {
    /// Design infiltration volume flow rate [m³/s]
    #[serde(default)]
    pub design_flow_rate: f64,
    /// Alternative: air changes per hour (converted to m³/s using zone volume)
    #[serde(default)]
    pub air_changes_per_hour: f64,
    /// Constant coefficient A (default 1.0)
    #[serde(default = "default_a", alias = "constant_coefficient")]
    pub coeff_a: f64,
    /// Temperature coefficient B [1/°C]
    #[serde(default, alias = "temperature_coefficient")]
    pub coeff_b: f64,
    /// Wind speed coefficient C [s/m]
    #[serde(default, alias = "wind_coefficient")]
    pub coeff_c: f64,
    /// Wind speed squared coefficient D [s²/m²]
    #[serde(default, alias = "wind_squared_coefficient")]
    pub coeff_d: f64,
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
            coeff_a: 1.0,
            coeff_b: 0.0,
            coeff_c: 0.0,
            coeff_d: 0.0,
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
    let factor = input.coeff_a
        + input.coeff_b * dt
        + input.coeff_c * wind_speed
        + input.coeff_d * wind_speed * wind_speed;

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
            coeff_a: 0.0,
            coeff_c: 0.01,
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
