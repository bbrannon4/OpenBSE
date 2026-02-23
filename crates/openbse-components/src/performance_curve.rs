//! Reusable performance curves for HVAC equipment.
//!
//! Curves modify rated equipment performance as a function of operating
//! conditions (temperatures, flow ratios, etc.).
//!
//! Supported curve types:
//! - **Biquadratic**: f(x,y) = c1 + c2*x + c3*x² + c4*y + c5*y² + c6*x*y
//! - **Quadratic**: f(x) = c1 + c2*x + c3*x²
//! - **Cubic**: f(x) = c1 + c2*x + c3*x² + c4*x³
//! - **Linear**: f(x) = c1 + c2*x
//!
//! Reference: EnergyPlus Engineering Reference, "Performance Curves"

use serde::{Deserialize, Serialize};

/// Curve type determines the functional form.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CurveType {
    /// f(x,y) = c1 + c2*x + c3*x² + c4*y + c5*y² + c6*x*y
    Biquadratic,
    /// f(x) = c1 + c2*x + c3*x²
    Quadratic,
    /// f(x) = c1 + c2*x + c3*x² + c4*x³
    Cubic,
    /// f(x) = c1 + c2*x
    Linear,
}

/// A reusable performance curve for HVAC equipment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceCurve {
    pub name: String,
    pub curve_type: CurveType,
    /// Polynomial coefficients (length depends on curve_type)
    pub coefficients: Vec<f64>,
    /// Minimum value of independent variable x
    #[serde(default = "default_min")]
    pub min_x: f64,
    /// Maximum value of independent variable x
    #[serde(default = "default_max")]
    pub max_x: f64,
    /// Minimum value of independent variable y (biquadratic only)
    #[serde(default = "default_min")]
    pub min_y: f64,
    /// Maximum value of independent variable y (biquadratic only)
    #[serde(default = "default_max")]
    pub max_y: f64,
    /// Minimum allowed output value
    #[serde(default)]
    pub min_output: Option<f64>,
    /// Maximum allowed output value
    #[serde(default)]
    pub max_output: Option<f64>,
}

fn default_min() -> f64 { -100.0 }
fn default_max() -> f64 { 100.0 }

impl PerformanceCurve {
    /// Evaluate the curve at the given independent variable(s).
    ///
    /// For single-variable curves (linear, quadratic, cubic), only `x` is used.
    /// For biquadratic curves, both `x` and `y` are used.
    ///
    /// Input values are clamped to [min, max] ranges before evaluation.
    /// Output is clamped to [min_output, max_output] if those limits are set.
    pub fn evaluate(&self, x: f64, y: f64) -> f64 {
        let x = x.clamp(self.min_x, self.max_x);
        let y = y.clamp(self.min_y, self.max_y);
        let c = &self.coefficients;

        let result = match self.curve_type {
            CurveType::Linear => {
                // f(x) = c1 + c2*x
                c.get(0).copied().unwrap_or(0.0)
                    + c.get(1).copied().unwrap_or(0.0) * x
            }
            CurveType::Quadratic => {
                // f(x) = c1 + c2*x + c3*x²
                c.get(0).copied().unwrap_or(0.0)
                    + c.get(1).copied().unwrap_or(0.0) * x
                    + c.get(2).copied().unwrap_or(0.0) * x * x
            }
            CurveType::Cubic => {
                // f(x) = c1 + c2*x + c3*x² + c4*x³
                c.get(0).copied().unwrap_or(0.0)
                    + c.get(1).copied().unwrap_or(0.0) * x
                    + c.get(2).copied().unwrap_or(0.0) * x * x
                    + c.get(3).copied().unwrap_or(0.0) * x * x * x
            }
            CurveType::Biquadratic => {
                // f(x,y) = c1 + c2*x + c3*x² + c4*y + c5*y² + c6*x*y
                c.get(0).copied().unwrap_or(0.0)
                    + c.get(1).copied().unwrap_or(0.0) * x
                    + c.get(2).copied().unwrap_or(0.0) * x * x
                    + c.get(3).copied().unwrap_or(0.0) * y
                    + c.get(4).copied().unwrap_or(0.0) * y * y
                    + c.get(5).copied().unwrap_or(0.0) * x * y
            }
        };

        // Clamp output
        let result = if let Some(min) = self.min_output {
            result.max(min)
        } else {
            result
        };
        if let Some(max) = self.max_output {
            result.min(max)
        } else {
            result
        }
    }

    /// Evaluate with a single independent variable (for linear/quadratic/cubic).
    pub fn evaluate_1d(&self, x: f64) -> f64 {
        self.evaluate(x, 0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_relative_eq;

    #[test]
    fn test_biquadratic_at_rated() {
        // Typical DX cooling capacity curve (EnergyPlus default)
        // At rated conditions: EWB=19.44°C, ODB=35°C → modifier ≈ 1.0
        let curve = PerformanceCurve {
            name: "DX Cap fT".to_string(),
            curve_type: CurveType::Biquadratic,
            coefficients: vec![
                0.942587793, 0.009543347, 0.000683770,
                -0.011042676, 0.000005249, -0.000009720,
            ],
            min_x: 12.78,
            max_x: 23.89,
            min_y: 18.33,
            max_y: 46.11,
            min_output: None,
            max_output: None,
        };

        // At rated: EWB=19.44, ODB=35.0
        let modifier = curve.evaluate(19.44, 35.0);
        // Should be close to 1.0 at rated conditions
        assert_relative_eq!(modifier, 1.0, max_relative = 0.05);
    }

    #[test]
    fn test_biquadratic_hot_outdoor() {
        let curve = PerformanceCurve {
            name: "DX Cap fT".to_string(),
            curve_type: CurveType::Biquadratic,
            coefficients: vec![
                0.942587793, 0.009543347, 0.000683770,
                -0.011042676, 0.000005249, -0.000009720,
            ],
            min_x: 12.78,
            max_x: 23.89,
            min_y: 18.33,
            max_y: 46.11,
            min_output: None,
            max_output: None,
        };

        // Higher ODB → capacity should decrease
        let mod_35 = curve.evaluate(19.44, 35.0);
        let mod_45 = curve.evaluate(19.44, 45.0);
        assert!(mod_45 < mod_35, "Capacity should decrease at higher ODB");
    }

    #[test]
    fn test_quadratic() {
        let curve = PerformanceCurve {
            name: "PLF".to_string(),
            curve_type: CurveType::Quadratic,
            coefficients: vec![0.85, 0.15, 0.0],
            min_x: 0.0,
            max_x: 1.0,
            min_y: 0.0,
            max_y: 0.0,
            min_output: None,
            max_output: None,
        };

        // At PLR=1.0: PLF = 0.85 + 0.15*1.0 = 1.0
        assert_relative_eq!(curve.evaluate_1d(1.0), 1.0, max_relative = 0.001);
        // At PLR=0.0: PLF = 0.85
        assert_relative_eq!(curve.evaluate_1d(0.0), 0.85, max_relative = 0.001);
    }

    #[test]
    fn test_linear() {
        let curve = PerformanceCurve {
            name: "Linear".to_string(),
            curve_type: CurveType::Linear,
            coefficients: vec![1.0, -0.01],
            min_x: 0.0,
            max_x: 50.0,
            min_y: 0.0,
            max_y: 0.0,
            min_output: Some(0.5),
            max_output: Some(1.1),
        };

        // f(35) = 1.0 - 0.01*35 = 0.65
        assert_relative_eq!(curve.evaluate_1d(35.0), 0.65, max_relative = 0.001);
        // f(0) = 1.0, clamped to max_output 1.1 (no clamp needed)
        assert_relative_eq!(curve.evaluate_1d(0.0), 1.0, max_relative = 0.001);
    }

    #[test]
    fn test_output_clamping() {
        let curve = PerformanceCurve {
            name: "Clamped".to_string(),
            curve_type: CurveType::Linear,
            coefficients: vec![2.0, -0.1],
            min_x: 0.0,
            max_x: 100.0,
            min_y: 0.0,
            max_y: 0.0,
            min_output: Some(0.5),
            max_output: Some(1.5),
        };

        // f(0) = 2.0, clamped to 1.5
        assert_relative_eq!(curve.evaluate_1d(0.0), 1.5, max_relative = 0.001);
        // f(100) = 2.0 - 10.0 = -8.0, clamped to 0.5
        assert_relative_eq!(curve.evaluate_1d(100.0), 0.5, max_relative = 0.001);
    }

    #[test]
    fn test_input_clamping() {
        let curve = PerformanceCurve {
            name: "InputClamp".to_string(),
            curve_type: CurveType::Linear,
            coefficients: vec![1.0, 0.01],
            min_x: 10.0,
            max_x: 40.0,
            min_y: 0.0,
            max_y: 0.0,
            min_output: None,
            max_output: None,
        };

        // x=0 clamped to min_x=10: f(10) = 1.0 + 0.01*10 = 1.1
        assert_relative_eq!(curve.evaluate_1d(0.0), 1.1, max_relative = 0.001);
        // x=50 clamped to max_x=40: f(40) = 1.0 + 0.01*40 = 1.4
        assert_relative_eq!(curve.evaluate_1d(50.0), 1.4, max_relative = 0.001);
    }
}
