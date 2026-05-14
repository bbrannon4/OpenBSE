//! Generic N-dimensional linear interpolation over rectilinear grids.
//!
//! Standard 205 performance maps are organized as a list of grid-variable
//! axes (each a strictly increasing 1-D array) and a flat row-major
//! "lookup" array whose length equals the product of the axis lengths.
//! Axis 0 varies slowest (outermost loop), the last axis varies fastest.
//!
//! Out-of-range queries are clamped to the nearest grid edge (see crate
//! docs).  Axes with a single point are degenerate and contribute no
//! interpolation; the query value along that axis is ignored.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum InterpError {
    #[error("axis '{name}' values must be strictly increasing")]
    AxisNotMonotonic { name: String },
    #[error(
        "lookup array length {got} does not match grid product {expected} \
         (axes: {axes:?})"
    )]
    SizeMismatch {
        got: usize,
        expected: usize,
        axes: Vec<usize>,
    },
    #[error("axis '{name}' is empty")]
    EmptyAxis { name: String },
    #[error("number of query values ({got}) does not match number of axes ({expected})")]
    WrongQueryArity { got: usize, expected: usize },
}

/// A single axis of a performance map grid.
#[derive(Debug, Clone)]
pub struct Axis {
    pub name: String,
    /// Strictly increasing breakpoint values along this axis.
    pub values: Vec<f64>,
}

impl Axis {
    pub fn new(name: impl Into<String>, values: Vec<f64>) -> Result<Self, InterpError> {
        let name = name.into();
        if values.is_empty() {
            return Err(InterpError::EmptyAxis { name });
        }
        for w in values.windows(2) {
            if w[1] <= w[0] {
                return Err(InterpError::AxisNotMonotonic { name });
            }
        }
        Ok(Self { name, values })
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// Find the (lower_index, upper_index, fraction) for `x`, clamping to
    /// the grid edges.  For single-point axes returns (0, 0, 0.0).
    fn locate(&self, x: f64) -> (usize, usize, f64) {
        let n = self.values.len();
        if n == 1 {
            return (0, 0, 0.0);
        }
        if x <= self.values[0] {
            return (0, 0, 0.0);
        }
        if x >= self.values[n - 1] {
            return (n - 1, n - 1, 0.0);
        }
        // Binary search for upper index
        let upper = self.values.partition_point(|&v| v <= x);
        let lower = upper - 1;
        let lo = self.values[lower];
        let hi = self.values[upper];
        let frac = (x - lo) / (hi - lo);
        (lower, upper, frac)
    }
}

/// An N-dimensional rectilinear grid with a single flat row-major lookup
/// array.  Multiple lookup variables sharing the same grid are stored
/// separately and indexed via [`NdGrid::interp`].
#[derive(Debug, Clone)]
pub struct NdGrid {
    pub axes: Vec<Axis>,
    /// Cached strides for row-major indexing.  `strides[i]` is the index
    /// offset when stepping one position along axis `i`.
    strides: Vec<usize>,
    pub total_size: usize,
}

impl NdGrid {
    pub fn new(axes: Vec<Axis>) -> Self {
        let n = axes.len();
        let mut strides = vec![1usize; n];
        // Row-major: last axis has stride 1, others compounded right-to-left
        for i in (0..n.saturating_sub(1)).rev() {
            strides[i] = strides[i + 1] * axes[i + 1].len();
        }
        let total_size = axes.iter().map(|a| a.len()).product::<usize>().max(1);
        Self {
            axes,
            strides,
            total_size,
        }
    }

    /// Validate that a lookup array matches this grid's size.
    pub fn validate(&self, lookup_len: usize) -> Result<(), InterpError> {
        if lookup_len != self.total_size {
            return Err(InterpError::SizeMismatch {
                got: lookup_len,
                expected: self.total_size,
                axes: self.axes.iter().map(|a| a.len()).collect(),
            });
        }
        Ok(())
    }

    /// Interpolate `lookup` at query point `q`.  `q.len()` must equal the
    /// number of axes.  Out-of-range coordinates clamp to grid edges.
    pub fn interp(&self, lookup: &[f64], q: &[f64]) -> Result<f64, InterpError> {
        if q.len() != self.axes.len() {
            return Err(InterpError::WrongQueryArity {
                got: q.len(),
                expected: self.axes.len(),
            });
        }
        self.validate(lookup.len())?;

        let n = self.axes.len();
        let mut indices: Vec<(usize, usize, f64)> = Vec::with_capacity(n);
        for (axis, &x) in self.axes.iter().zip(q.iter()) {
            indices.push(axis.locate(x));
        }

        // N-linear interpolation: sum over the 2^n corners of the bounding box,
        // weighted by the product of (1-frac) and frac across axes.
        // For degenerate axes (lower==upper), one corner with weight 1.
        let mut result = 0.0;
        let n_corners = 1usize << n;
        for mask in 0..n_corners {
            let mut weight = 1.0;
            let mut flat = 0usize;
            let mut skip = false;
            for (i, &(lo, hi, frac)) in indices.iter().enumerate() {
                let high_bit = (mask >> i) & 1 == 1;
                let idx = if high_bit { hi } else { lo };
                // For degenerate axes (lo == hi), only count the corner once
                if high_bit && lo == hi {
                    skip = true;
                    break;
                }
                let w = if lo == hi {
                    1.0
                } else if high_bit {
                    frac
                } else {
                    1.0 - frac
                };
                weight *= w;
                flat += idx * self.strides[i];
            }
            if skip {
                continue;
            }
            result += weight * lookup[flat];
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_relative_eq;

    #[test]
    fn test_1d_linear() {
        let axis = Axis::new("x", vec![0.0, 1.0, 2.0]).unwrap();
        let grid = NdGrid::new(vec![axis]);
        let values = vec![0.0, 10.0, 30.0];
        assert_relative_eq!(grid.interp(&values, &[0.5]).unwrap(), 5.0);
        assert_relative_eq!(grid.interp(&values, &[1.5]).unwrap(), 20.0);
        // Edge clamping
        assert_relative_eq!(grid.interp(&values, &[-1.0]).unwrap(), 0.0);
        assert_relative_eq!(grid.interp(&values, &[10.0]).unwrap(), 30.0);
    }

    #[test]
    fn test_2d_bilinear() {
        // 2x2 grid, exact corners
        let ax = Axis::new("x", vec![0.0, 10.0]).unwrap();
        let ay = Axis::new("y", vec![0.0, 10.0]).unwrap();
        let grid = NdGrid::new(vec![ax, ay]);
        // Row-major: [x0y0, x0y1, x1y0, x1y1]
        let v = vec![0.0, 10.0, 20.0, 30.0];
        assert_relative_eq!(grid.interp(&v, &[0.0, 0.0]).unwrap(), 0.0);
        assert_relative_eq!(grid.interp(&v, &[10.0, 10.0]).unwrap(), 30.0);
        assert_relative_eq!(grid.interp(&v, &[5.0, 5.0]).unwrap(), 15.0);
        assert_relative_eq!(grid.interp(&v, &[5.0, 0.0]).unwrap(), 10.0);
    }

    #[test]
    fn test_degenerate_axis() {
        // One axis is a single point - should be ignored
        let ax = Axis::new("x", vec![5.0]).unwrap();
        let ay = Axis::new("y", vec![0.0, 10.0]).unwrap();
        let grid = NdGrid::new(vec![ax, ay]);
        let v = vec![0.0, 20.0]; // 1 * 2 = 2
        assert_relative_eq!(grid.interp(&v, &[5.0, 5.0]).unwrap(), 10.0);
        assert_relative_eq!(grid.interp(&v, &[99.0, 5.0]).unwrap(), 10.0);
    }

    #[test]
    fn test_3d_trilinear() {
        // 2x2x2 unit cube, corner values 0..7 row-major
        let ax = Axis::new("x", vec![0.0, 1.0]).unwrap();
        let ay = Axis::new("y", vec![0.0, 1.0]).unwrap();
        let az = Axis::new("z", vec![0.0, 1.0]).unwrap();
        let grid = NdGrid::new(vec![ax, ay, az]);
        let v: Vec<f64> = (0..8).map(|i| i as f64).collect();
        // Center should be mean of all 8 corners = 3.5
        assert_relative_eq!(grid.interp(&v, &[0.5, 0.5, 0.5]).unwrap(), 3.5);
    }

    #[test]
    fn test_non_monotonic_rejected() {
        assert!(Axis::new("x", vec![1.0, 0.0]).is_err());
        assert!(Axis::new("x", vec![1.0, 1.0]).is_err());
    }
}
