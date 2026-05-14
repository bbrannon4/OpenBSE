//! ASHRAE Standard 205 RS0007 — Mechanical Drive (belt drive).
//!
//! Smallest of the Standard 205 representation specifications.  Maps
//! efficiency to the output mechanical power.  `speed_ratio` is the gear
//! ratio (output rotational speed / input rotational speed).
//!
//! All quantities SI: power in W, rotational speed in rev/s.

use crate::interpolate::{Axis, NdGrid};
use crate::{detect_format, A205Error, FileFormat};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Rs0007 {
    pub metadata: super::rs0001::Metadata,
    #[serde(default)]
    pub description: Option<serde_json::Value>,
    pub performance: Performance,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Performance {
    /// Output rotational speed / input rotational speed.
    pub speed_ratio: f64,
    pub performance_map: PerformanceMap,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PerformanceMap {
    pub grid_variables: GridVars,
    pub lookup_variables: LookupVars,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GridVars {
    /// Mechanical power delivered to the driven shaft [W]
    pub output_power: Vec<f64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LookupVars {
    pub efficiency: Vec<f64>,
    #[serde(default)]
    pub operation_state: Option<Vec<String>>,
}

impl Rs0007 {
    pub fn load(path: &Path) -> Result<Self, A205Error> {
        let fmt = detect_format(path)?;
        let bytes = std::fs::read(path)?;
        let obj: Self = match fmt {
            FileFormat::Json => serde_json::from_slice(&bytes)?,
            FileFormat::Cbor => ciborium::from_reader(bytes.as_slice())
                .map_err(|e| A205Error::Cbor(format!("{}", e)))?,
        };
        if obj.metadata.schema != "RS0007" {
            return Err(A205Error::SchemaMismatch {
                expected: "RS0007".into(),
                found: obj.metadata.schema.clone(),
            });
        }
        Ok(obj)
    }
}

/// Efficiency interpolator over output power.
pub struct DriveEfficiency {
    grid: NdGrid,
    efficiency: Vec<f64>,
    pub speed_ratio: f64,
}

impl DriveEfficiency {
    pub fn new(rs: &Rs0007) -> Result<Self, A205Error> {
        let axis = Axis::new(
            "output_power",
            rs.performance
                .performance_map
                .grid_variables
                .output_power
                .clone(),
        )
        .map_err(|e| A205Error::Other(format!("{}", e)))?;
        let grid = NdGrid::new(vec![axis]);
        Ok(Self {
            grid,
            efficiency: rs
                .performance
                .performance_map
                .lookup_variables
                .efficiency
                .clone(),
            speed_ratio: rs.performance.speed_ratio,
        })
    }

    /// Efficiency at the given output shaft power [W].  Clamps to [1e-3, 1].
    pub fn efficiency_at(&self, output_power_w: f64) -> f64 {
        self.grid
            .interp(&self.efficiency, &[output_power_w])
            .unwrap_or(0.9)
            .clamp(1e-3, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_relative_eq;

    fn example() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("examples")
            .join("Belt-Drive-Constant-Efficiency.RS0007.a205.json")
    }

    #[test]
    fn loads_belt_drive() {
        let rs = Rs0007::load(&example()).unwrap();
        assert_eq!(rs.metadata.schema, "RS0007");
        assert_relative_eq!(rs.performance.speed_ratio, 0.25, epsilon = 1e-9);
        let eff = DriveEfficiency::new(&rs).unwrap();
        assert_relative_eq!(eff.efficiency_at(5000.0), 0.985, epsilon = 1e-6);
    }
}
