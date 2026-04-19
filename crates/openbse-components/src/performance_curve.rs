//! Reusable performance curves for HVAC equipment.
//!
//! Curves modify rated equipment performance as a function of operating
//! conditions (temperatures, flow ratios, etc.).
//!
//! # Variants
//!
//! - **Polynomial** — biquadratic, quadratic, cubic, or linear expression
//! - **TableLookup** — manufacturer tabular data with N-linear interpolation;
//!   no polynomial fitting required
//!
//! Reference: EnergyPlus Engineering Reference, "Performance Curves"

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ─── Independent variable tags ─────────────────────────────────────────────

/// Physical variable an axis represents — determines which runtime value to look up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IndependentVariable {
    OutdoorDryBulb,    // °C
    OutdoorWetBulb,    // °C
    IndoorDryBulb,     // °C
    IndoorWetBulb,     // °C
    EnteringWaterTemp, // °C
    LeavingWaterTemp,  // °C
    PartLoadRatio,     // [0–1]
    AirflowFraction,   // [0–1]
    WaterFlowFraction, // [0–1]
}

impl IndependentVariable {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OutdoorDryBulb => "outdoor_dry_bulb",
            Self::OutdoorWetBulb => "outdoor_wet_bulb",
            Self::IndoorDryBulb => "indoor_dry_bulb",
            Self::IndoorWetBulb => "indoor_wet_bulb",
            Self::EnteringWaterTemp => "entering_water_temp",
            Self::LeavingWaterTemp => "leaving_water_temp",
            Self::PartLoadRatio => "part_load_ratio",
            Self::AirflowFraction => "airflow_fraction",
            Self::WaterFlowFraction => "water_flow_fraction",
        }
    }
}

// ─── Extrapolation mode ────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ExtrapolationMode {
    #[default]
    HoldEdge, // clamp to nearest edge value (safe default)
    Linear, // extrapolate linearly using boundary slope
}

// ─── Table axis ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableAxis {
    pub variable: IndependentVariable,
    /// Breakpoints — must be strictly monotonically increasing.
    pub values: Vec<f64>,
    #[serde(default)]
    pub extrapolation: ExtrapolationMode,
}

// ─── Table data ────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableData {
    pub axes: Vec<TableAxis>,
    /// Flat row-major values. Length must equal the product of all axis lengths.
    /// Axis 0 varies slowest (outermost loop), last axis varies fastest.
    /// YAML may supply either a flat sequence or a nested sequence of rows.
    #[serde(deserialize_with = "deserialize_values")]
    pub values: Vec<f64>,
    #[serde(default)]
    pub output_min: Option<f64>,
    #[serde(default)]
    pub output_max: Option<f64>,
}

/// Accepts either `[f64, ...]` or `[[f64, ...], ...]` in YAML and flattens.
fn deserialize_values<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<f64>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum FlatOrNested {
        Flat(Vec<f64>),
        Nested(Vec<Vec<f64>>),
    }
    match FlatOrNested::deserialize(d)? {
        FlatOrNested::Flat(v) => Ok(v),
        FlatOrNested::Nested(v) => Ok(v.into_iter().flatten().collect()),
    }
}

// ─── Slot validation specs ─────────────────────────────────────────────────
//
// Each slice entry is the set of IndependentVariables allowed for that axis
// position in the named slot.

pub const SLOT_COOLING_CAP_FT: &[&[IndependentVariable]] = &[
    &[
        IndependentVariable::OutdoorDryBulb,
        IndependentVariable::OutdoorWetBulb,
    ],
    &[
        IndependentVariable::IndoorDryBulb,
        IndependentVariable::IndoorWetBulb,
    ],
];

pub const SLOT_COOLING_EIR_FT: &[&[IndependentVariable]] = &[
    &[
        IndependentVariable::OutdoorDryBulb,
        IndependentVariable::OutdoorWetBulb,
    ],
    &[
        IndependentVariable::IndoorDryBulb,
        IndependentVariable::IndoorWetBulb,
    ],
];

pub const SLOT_PLF_FPLR: &[&[IndependentVariable]] = &[&[IndependentVariable::PartLoadRatio]];

pub const SLOT_HEATING_CAP_FT: &[&[IndependentVariable]] = &[
    &[
        IndependentVariable::OutdoorDryBulb,
        IndependentVariable::OutdoorWetBulb,
    ],
    &[
        IndependentVariable::IndoorDryBulb,
        IndependentVariable::IndoorWetBulb,
    ],
];

pub const SLOT_HEATING_EIR_FT: &[&[IndependentVariable]] = &[
    &[
        IndependentVariable::OutdoorDryBulb,
        IndependentVariable::OutdoorWetBulb,
    ],
    &[
        IndependentVariable::IndoorDryBulb,
        IndependentVariable::IndoorWetBulb,
    ],
];

pub const SLOT_FAN_POWER_FFLOW: &[&[IndependentVariable]] =
    &[&[IndependentVariable::AirflowFraction]];

// ─── Curve type (polynomial only) ─────────────────────────────────────────

/// Functional form for a Polynomial curve.
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

// ─── Performance curve ─────────────────────────────────────────────────────

/// A reusable performance curve for HVAC equipment.
///
/// Existing YAML files that use the flat polynomial format are deserialized as
/// the `Polynomial` variant. New table-based curves use a nested `table:` key
/// and are deserialized as `TableLookup`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PerformanceCurve {
    /// Manufacturer tabular data with N-linear interpolation.
    TableLookup { name: String, table: TableData },
    /// Classic polynomial curve (biquadratic / quadratic / cubic / linear).
    Polynomial {
        name: String,
        curve_type: CurveType,
        /// Polynomial coefficients (count depends on curve_type).
        coefficients: Vec<f64>,
        #[serde(default = "default_min")]
        min_x: f64,
        #[serde(default = "default_max")]
        max_x: f64,
        #[serde(default = "default_min")]
        min_y: f64,
        #[serde(default = "default_max")]
        max_y: f64,
        #[serde(default)]
        min_output: Option<f64>,
        #[serde(default)]
        max_output: Option<f64>,
    },
}

fn default_min() -> f64 {
    -100.0
}
fn default_max() -> f64 {
    100.0
}

impl PerformanceCurve {
    /// Return the curve name (common to both variants).
    pub fn name(&self) -> &str {
        match self {
            Self::Polynomial { name, .. } | Self::TableLookup { name, .. } => name,
        }
    }

    /// Evaluate a **Polynomial** curve at (x, y).
    ///
    /// For single-variable curves only `x` is used. Inputs are clamped to
    /// [min_x, max_x] / [min_y, max_y]; output is clamped to
    /// [min_output, max_output] when those limits are set.
    ///
    /// # Panics
    /// Panics if called on a `TableLookup` variant — use [`evaluate_table`] instead.
    pub fn evaluate(&self, x: f64, y: f64) -> f64 {
        let Self::Polynomial {
            curve_type,
            coefficients,
            min_x,
            max_x,
            min_y,
            max_y,
            min_output,
            max_output,
            ..
        } = self
        else {
            panic!(
                "PerformanceCurve::evaluate called on TableLookup variant; use evaluate_table()"
            );
        };

        let x = x.clamp(*min_x, *max_x);
        let y = y.clamp(*min_y, *max_y);
        let c = coefficients;

        let result = match curve_type {
            CurveType::Linear => {
                c.get(0).copied().unwrap_or(0.0) + c.get(1).copied().unwrap_or(0.0) * x
            }
            CurveType::Quadratic => {
                c.get(0).copied().unwrap_or(0.0)
                    + c.get(1).copied().unwrap_or(0.0) * x
                    + c.get(2).copied().unwrap_or(0.0) * x * x
            }
            CurveType::Cubic => {
                c.get(0).copied().unwrap_or(0.0)
                    + c.get(1).copied().unwrap_or(0.0) * x
                    + c.get(2).copied().unwrap_or(0.0) * x * x
                    + c.get(3).copied().unwrap_or(0.0) * x * x * x
            }
            CurveType::Biquadratic => {
                c.get(0).copied().unwrap_or(0.0)
                    + c.get(1).copied().unwrap_or(0.0) * x
                    + c.get(2).copied().unwrap_or(0.0) * x * x
                    + c.get(3).copied().unwrap_or(0.0) * y
                    + c.get(4).copied().unwrap_or(0.0) * y * y
                    + c.get(5).copied().unwrap_or(0.0) * x * y
            }
        };

        let result = if let Some(min) = min_output {
            result.max(*min)
        } else {
            result
        };
        if let Some(max) = max_output {
            result.min(*max)
        } else {
            result
        }
    }

    /// Convenience wrapper for single-variable polynomial curves.
    ///
    /// # Panics
    /// See [`evaluate`].
    pub fn evaluate_1d(&self, x: f64) -> f64 {
        self.evaluate(x, 0.0)
    }

    /// Evaluate a **TableLookup** curve given named runtime variable values.
    ///
    /// Axis order in the YAML does not need to match any specific order —
    /// each axis declares which [`IndependentVariable`] it represents and the
    /// correct column is picked from `inputs` at runtime.
    ///
    /// # Panics
    /// Panics if called on a `Polynomial` variant — use [`evaluate`] instead.
    pub fn evaluate_table(&self, inputs: &HashMap<IndependentVariable, f64>) -> f64 {
        let Self::TableLookup { table, .. } = self else {
            panic!("PerformanceCurve::evaluate_table called on Polynomial variant; use evaluate()");
        };
        let raw = interp_nd(&table.axes, &table.values, inputs, 0, 0);
        let raw = if let Some(min) = table.output_min {
            raw.max(min)
        } else {
            raw
        };
        if let Some(max) = table.output_max {
            raw.min(max)
        } else {
            raw
        }
    }
}

// ─── N-linear interpolation ────────────────────────────────────────────────

/// Recursively interpolate over `axes[dim..]`, starting at `offset` in the
/// flat row-major `values` slice.
fn interp_nd(
    axes: &[TableAxis],
    values: &[f64],
    inputs: &HashMap<IndependentVariable, f64>,
    dim: usize,
    offset: usize,
) -> f64 {
    if dim == axes.len() {
        return values.get(offset).copied().unwrap_or(0.0);
    }

    let axis = &axes[dim];
    // Product of all subsequent axis lengths gives the stride for this dimension.
    let stride: usize = axes[dim + 1..].iter().map(|a| a.values.len()).product();

    let x = inputs.get(&axis.variable).copied().unwrap_or(0.0);
    let (lo, hi, t) = find_bracket(axis, x);

    let lo_val = interp_nd(axes, values, inputs, dim + 1, offset + lo * stride);
    let hi_val = interp_nd(axes, values, inputs, dim + 1, offset + hi * stride);

    lo_val + t * (hi_val - lo_val)
}

/// Find the bounding indices and interpolation fraction `t ∈ [0, 1]` (or
/// extrapolated beyond that range) for a sorted axis breakpoint list.
fn find_bracket(axis: &TableAxis, x: f64) -> (usize, usize, f64) {
    let vs = &axis.values;
    let n = vs.len();

    if n <= 1 {
        return (0, 0, 0.0);
    }

    if x <= vs[0] {
        return match axis.extrapolation {
            ExtrapolationMode::HoldEdge => (0, 0, 0.0),
            ExtrapolationMode::Linear => {
                let t = (x - vs[0]) / (vs[1] - vs[0]);
                (0, 1, t) // t < 0 → extrapolates below
            }
        };
    }

    if x >= vs[n - 1] {
        return match axis.extrapolation {
            ExtrapolationMode::HoldEdge => (n - 1, n - 1, 0.0),
            ExtrapolationMode::Linear => {
                let t = (x - vs[n - 2]) / (vs[n - 1] - vs[n - 2]);
                (n - 2, n - 1, t) // t > 1 → extrapolates above
            }
        };
    }

    // Interior: first index where vs[i] > x.
    let hi = vs.partition_point(|&v| v <= x);
    let lo = hi - 1;
    let t = (x - vs[lo]) / (vs[hi] - vs[lo]);
    (lo, hi, t)
}

// ─── Slot validation ───────────────────────────────────────────────────────

/// Validate that a `TableLookup` curve's axes are compatible with a named slot.
///
/// `allowed_per_axis[i]` is the set of [`IndependentVariable`]s that axis `i`
/// may represent. Returns `Ok(())` for `Polynomial` curves (no validation
/// needed) and for valid `TableLookup` curves. Returns `Err` with a
/// human-readable message on mismatch.
pub fn validate_table_axes(
    curve: &PerformanceCurve,
    slot_name: &str,
    allowed_per_axis: &[&[IndependentVariable]],
) -> Result<(), String> {
    let table = match curve {
        PerformanceCurve::Polynomial { .. } => return Ok(()),
        PerformanceCurve::TableLookup { table, .. } => table,
    };

    if table.axes.len() != allowed_per_axis.len() {
        return Err(format!(
            "{}: expected {} axis/axes, got {}",
            slot_name,
            allowed_per_axis.len(),
            table.axes.len()
        ));
    }

    for (i, (axis, allowed)) in table.axes.iter().zip(allowed_per_axis.iter()).enumerate() {
        if !allowed.contains(&axis.variable) {
            let allowed_names: Vec<&str> = allowed.iter().map(|v| v.as_str()).collect();
            return Err(format!(
                "{}: axis {} must be one of [{}], got {}",
                slot_name,
                i,
                allowed_names.join(", "),
                axis.variable.as_str()
            ));
        }
    }

    Ok(())
}

// ─── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_relative_eq;

    // ── Helpers ─────────────────────────────────────────────────────────────

    fn make_poly(
        curve_type: CurveType,
        coefficients: Vec<f64>,
        min_output: Option<f64>,
        max_output: Option<f64>,
    ) -> PerformanceCurve {
        PerformanceCurve::Polynomial {
            name: "test".to_string(),
            curve_type,
            coefficients,
            min_x: -100.0,
            max_x: 100.0,
            min_y: -100.0,
            max_y: 100.0,
            min_output,
            max_output,
        }
    }

    fn plf_1d(extrap: ExtrapolationMode) -> PerformanceCurve {
        PerformanceCurve::TableLookup {
            name: "PLF 1D".to_string(),
            table: TableData {
                axes: vec![TableAxis {
                    variable: IndependentVariable::PartLoadRatio,
                    values: vec![0.0, 0.5, 1.0],
                    extrapolation: extrap,
                }],
                values: vec![0.85, 0.925, 1.0],
                output_min: None,
                output_max: None,
            },
        }
    }

    fn inputs1(var: IndependentVariable, v: f64) -> HashMap<IndependentVariable, f64> {
        let mut m = HashMap::new();
        m.insert(var, v);
        m
    }

    fn inputs2(
        v1: IndependentVariable,
        x1: f64,
        v2: IndependentVariable,
        x2: f64,
    ) -> HashMap<IndependentVariable, f64> {
        let mut m = HashMap::new();
        m.insert(v1, x1);
        m.insert(v2, x2);
        m
    }

    // ── TableLookup: 1D ─────────────────────────────────────────────────────

    #[test]
    fn test_1d_linear_interpolation() {
        let curve = plf_1d(ExtrapolationMode::HoldEdge);
        // midpoint between 0.0→0.85 and 0.5→0.925: t=0.5
        let expected = 0.85 + 0.5 * (0.925 - 0.85);
        assert_relative_eq!(
            curve.evaluate_table(&inputs1(IndependentVariable::PartLoadRatio, 0.25)),
            expected,
            max_relative = 1e-9
        );
    }

    #[test]
    fn test_1d_hold_edge_extrapolation() {
        let curve = plf_1d(ExtrapolationMode::HoldEdge);
        assert_relative_eq!(
            curve.evaluate_table(&inputs1(IndependentVariable::PartLoadRatio, -0.5)),
            0.85,
            max_relative = 1e-9
        );
        assert_relative_eq!(
            curve.evaluate_table(&inputs1(IndependentVariable::PartLoadRatio, 1.5)),
            1.0,
            max_relative = 1e-9
        );
    }

    #[test]
    fn test_1d_linear_extrapolation() {
        let curve = plf_1d(ExtrapolationMode::Linear);
        // x=-0.5, bracket [0,0.5], t = (-0.5-0.0)/(0.5-0.0) = -1.0
        // result = 0.85 + (-1.0)*(0.925-0.85) = 0.775
        assert_relative_eq!(
            curve.evaluate_table(&inputs1(IndependentVariable::PartLoadRatio, -0.5)),
            0.775,
            max_relative = 1e-9
        );
    }

    // ── TableLookup: 2D ─────────────────────────────────────────────────────

    fn cap_3x3() -> PerformanceCurve {
        // axis 0: ODB [25, 30, 35]  axis 1: IWB [15, 19, 23]
        // row-major:
        //   (25,15)=1.10  (25,19)=1.00  (25,23)=0.90
        //   (30,15)=1.00  (30,19)=0.95  (30,23)=0.85
        //   (35,15)=0.90  (35,19)=0.87  (35,23)=0.75
        PerformanceCurve::TableLookup {
            name: "Cap 3x3".to_string(),
            table: TableData {
                axes: vec![
                    TableAxis {
                        variable: IndependentVariable::OutdoorDryBulb,
                        values: vec![25.0, 30.0, 35.0],
                        extrapolation: ExtrapolationMode::HoldEdge,
                    },
                    TableAxis {
                        variable: IndependentVariable::IndoorWetBulb,
                        values: vec![15.0, 19.0, 23.0],
                        extrapolation: ExtrapolationMode::HoldEdge,
                    },
                ],
                values: vec![1.10, 1.00, 0.90, 1.00, 0.95, 0.85, 0.90, 0.87, 0.75],
                output_min: None,
                output_max: None,
            },
        }
    }

    #[test]
    fn test_2d_exact_grid_point() {
        let curve = cap_3x3();
        let inp = inputs2(
            IndependentVariable::OutdoorDryBulb,
            30.0,
            IndependentVariable::IndoorWetBulb,
            19.0,
        );
        assert_relative_eq!(curve.evaluate_table(&inp), 0.95, max_relative = 1e-9);
    }

    #[test]
    fn test_2d_bilinear_interpolation() {
        let curve = cap_3x3();
        // ODB=27.5 t=0.5 in [25,30]; IWB=17.0 t=0.5 in [15,19]
        // lo_odb (25): lerp(1.10, 1.00, 0.5) = 1.05
        // hi_odb (30): lerp(1.00, 0.95, 0.5) = 0.975
        // result: lerp(1.05, 0.975, 0.5) = 1.0125
        let inp = inputs2(
            IndependentVariable::OutdoorDryBulb,
            27.5,
            IndependentVariable::IndoorWetBulb,
            17.0,
        );
        assert_relative_eq!(curve.evaluate_table(&inp), 1.0125, max_relative = 1e-9);
    }

    #[test]
    fn test_2d_axis_order_independence() {
        // Same 2x2 table, axes declared in opposite order — must give identical result.
        let natural = PerformanceCurve::TableLookup {
            name: "natural".to_string(),
            table: TableData {
                axes: vec![
                    TableAxis {
                        variable: IndependentVariable::OutdoorDryBulb,
                        values: vec![25.0, 30.0],
                        extrapolation: ExtrapolationMode::HoldEdge,
                    },
                    TableAxis {
                        variable: IndependentVariable::IndoorWetBulb,
                        values: vec![15.0, 19.0],
                        extrapolation: ExtrapolationMode::HoldEdge,
                    },
                ],
                // (25,15)=1.10  (25,19)=1.00  (30,15)=0.95  (30,19)=0.85
                values: vec![1.10, 1.00, 0.95, 0.85],
                output_min: None,
                output_max: None,
            },
        };
        let reversed = PerformanceCurve::TableLookup {
            name: "reversed".to_string(),
            table: TableData {
                axes: vec![
                    TableAxis {
                        variable: IndependentVariable::IndoorWetBulb,
                        values: vec![15.0, 19.0],
                        extrapolation: ExtrapolationMode::HoldEdge,
                    },
                    TableAxis {
                        variable: IndependentVariable::OutdoorDryBulb,
                        values: vec![25.0, 30.0],
                        extrapolation: ExtrapolationMode::HoldEdge,
                    },
                ],
                // axis 0 (IWB) slowest: (15,25)=1.10  (15,30)=0.95  (19,25)=1.00  (19,30)=0.85
                values: vec![1.10, 0.95, 1.00, 0.85],
                output_min: None,
                output_max: None,
            },
        };
        let inp = inputs2(
            IndependentVariable::OutdoorDryBulb,
            27.5,
            IndependentVariable::IndoorWetBulb,
            17.0,
        );
        assert_relative_eq!(
            natural.evaluate_table(&inp),
            reversed.evaluate_table(&inp),
            max_relative = 1e-9
        );
    }

    #[test]
    fn test_output_clamp_table() {
        let curve = PerformanceCurve::TableLookup {
            name: "Clamped".to_string(),
            table: TableData {
                axes: vec![TableAxis {
                    variable: IndependentVariable::PartLoadRatio,
                    values: vec![0.0, 1.0],
                    extrapolation: ExtrapolationMode::HoldEdge,
                }],
                values: vec![0.5, 2.0],
                output_min: Some(0.0),
                output_max: Some(1.5),
            },
        };
        // Raw at PLR=1.0 → 2.0, clamped to 1.5
        assert_relative_eq!(
            curve.evaluate_table(&inputs1(IndependentVariable::PartLoadRatio, 1.0)),
            1.5,
            max_relative = 1e-9
        );
        // Raw at PLR=0.0 → 0.5, no clamp needed
        assert_relative_eq!(
            curve.evaluate_table(&inputs1(IndependentVariable::PartLoadRatio, 0.0)),
            0.5,
            max_relative = 1e-9
        );
    }

    // ── Slot validation ─────────────────────────────────────────────────────

    #[test]
    fn test_validate_axes_ok() {
        let curve = PerformanceCurve::TableLookup {
            name: "CapFT".to_string(),
            table: TableData {
                axes: vec![
                    TableAxis {
                        variable: IndependentVariable::OutdoorWetBulb,
                        values: vec![15.0, 20.0, 25.0],
                        extrapolation: ExtrapolationMode::HoldEdge,
                    },
                    TableAxis {
                        variable: IndependentVariable::IndoorWetBulb,
                        values: vec![15.0, 19.0],
                        extrapolation: ExtrapolationMode::HoldEdge,
                    },
                ],
                values: vec![1.0, 0.9, 0.95, 0.85, 0.90, 0.80],
                output_min: None,
                output_max: None,
            },
        };
        assert!(validate_table_axes(&curve, "cooling_cap_ft", SLOT_COOLING_CAP_FT).is_ok());
    }

    #[test]
    fn test_validate_axes_wrong_variable() {
        let curve = PerformanceCurve::TableLookup {
            name: "Bad".to_string(),
            table: TableData {
                axes: vec![
                    TableAxis {
                        variable: IndependentVariable::PartLoadRatio, // wrong for cooling_cap_ft
                        values: vec![0.0, 1.0],
                        extrapolation: ExtrapolationMode::HoldEdge,
                    },
                    TableAxis {
                        variable: IndependentVariable::IndoorWetBulb,
                        values: vec![15.0, 19.0],
                        extrapolation: ExtrapolationMode::HoldEdge,
                    },
                ],
                values: vec![1.0, 0.9, 0.95, 0.85],
                output_min: None,
                output_max: None,
            },
        };
        let err = validate_table_axes(&curve, "cooling_cap_ft", SLOT_COOLING_CAP_FT).unwrap_err();
        assert!(err.contains("axis 0"), "expected 'axis 0' in: {err}");
        assert!(
            err.contains("part_load_ratio"),
            "expected variable name in: {err}"
        );
    }

    #[test]
    fn test_validate_axes_wrong_count() {
        let curve = PerformanceCurve::TableLookup {
            name: "OneAxis".to_string(),
            table: TableData {
                axes: vec![TableAxis {
                    variable: IndependentVariable::OutdoorDryBulb,
                    values: vec![25.0, 35.0],
                    extrapolation: ExtrapolationMode::HoldEdge,
                }],
                values: vec![1.0, 0.9],
                output_min: None,
                output_max: None,
            },
        };
        let err = validate_table_axes(&curve, "cooling_cap_ft", SLOT_COOLING_CAP_FT).unwrap_err();
        assert!(err.contains("expected 2"), "expected axis count in: {err}");
    }

    #[test]
    fn test_validate_polynomial_skipped() {
        let curve = make_poly(CurveType::Quadratic, vec![0.85, 0.15, 0.0], None, None);
        // Polynomial variants always pass slot validation.
        assert!(validate_table_axes(&curve, "plf_fplr", SLOT_PLF_FPLR).is_ok());
    }

    // ── Polynomial variants (unchanged behaviour) ────────────────────────────

    #[test]
    fn test_biquadratic_at_rated() {
        let curve = PerformanceCurve::Polynomial {
            name: "DX Cap fT".to_string(),
            curve_type: CurveType::Biquadratic,
            coefficients: vec![
                0.942587793,
                0.009543347,
                0.000683770,
                -0.011042676,
                0.000005249,
                -0.000009720,
            ],
            min_x: 12.78,
            max_x: 23.89,
            min_y: 18.33,
            max_y: 46.11,
            min_output: None,
            max_output: None,
        };
        // At rated: EWB=19.44, ODB=35.0 → modifier ≈ 1.0
        assert_relative_eq!(curve.evaluate(19.44, 35.0), 1.0, max_relative = 0.05);
    }

    #[test]
    fn test_biquadratic_hot_outdoor() {
        let curve = PerformanceCurve::Polynomial {
            name: "DX Cap fT".to_string(),
            curve_type: CurveType::Biquadratic,
            coefficients: vec![
                0.942587793,
                0.009543347,
                0.000683770,
                -0.011042676,
                0.000005249,
                -0.000009720,
            ],
            min_x: 12.78,
            max_x: 23.89,
            min_y: 18.33,
            max_y: 46.11,
            min_output: None,
            max_output: None,
        };
        let mod_35 = curve.evaluate(19.44, 35.0);
        let mod_45 = curve.evaluate(19.44, 45.0);
        assert!(mod_45 < mod_35, "Capacity should decrease at higher ODB");
    }

    #[test]
    fn test_quadratic() {
        let curve = make_poly(CurveType::Quadratic, vec![0.85, 0.15, 0.0], None, None);
        assert_relative_eq!(curve.evaluate_1d(1.0), 1.0, max_relative = 0.001);
        assert_relative_eq!(curve.evaluate_1d(0.0), 0.85, max_relative = 0.001);
    }

    #[test]
    fn test_linear() {
        let curve = PerformanceCurve::Polynomial {
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
        assert_relative_eq!(curve.evaluate_1d(35.0), 0.65, max_relative = 0.001);
        assert_relative_eq!(curve.evaluate_1d(0.0), 1.0, max_relative = 0.001);
    }

    #[test]
    fn test_output_clamping() {
        let curve = PerformanceCurve::Polynomial {
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
        assert_relative_eq!(curve.evaluate_1d(0.0), 1.5, max_relative = 0.001);
        assert_relative_eq!(curve.evaluate_1d(100.0), 0.5, max_relative = 0.001);
    }

    #[test]
    fn test_input_clamping() {
        let curve = PerformanceCurve::Polynomial {
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
        // x=0 clamped to 10 → 1.1
        assert_relative_eq!(curve.evaluate_1d(0.0), 1.1, max_relative = 0.001);
        // x=50 clamped to 40 → 1.4
        assert_relative_eq!(curve.evaluate_1d(50.0), 1.4, max_relative = 0.001);
    }
}
