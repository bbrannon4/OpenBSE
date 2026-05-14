//! ASHRAE Standard 205 RS0006 — Electronic Motor Drive (VFD).
//!
//! Efficiency lookup over (output_power, output_frequency).  All SI:
//! power W, frequency Hz.

use crate::interpolate::{Axis, NdGrid};
use crate::{detect_format, A205Error, FileFormat};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Rs0006 {
    pub metadata: super::rs0001::Metadata,
    #[serde(default)]
    pub description: Option<serde_json::Value>,
    pub performance: Performance,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Performance {
    pub maximum_power: f64,
    #[serde(default)]
    pub standby_power: f64,
    /// PASSIVE_COOLED, ACTIVE_AIR_COOLED, ACTIVE_LIQUID_COOLED — metadata only.
    #[serde(default)]
    pub cooling_method: Option<String>,
    pub performance_map: PerformanceMap,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PerformanceMap {
    pub grid_variables: GridVars,
    pub lookup_variables: LookupVars,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GridVars {
    pub output_power: Vec<f64>,
    pub output_frequency: Vec<f64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LookupVars {
    pub efficiency: Vec<f64>,
    #[serde(default)]
    pub operation_state: Option<Vec<String>>,
}

impl Rs0006 {
    pub fn load(path: &Path) -> Result<Self, A205Error> {
        let fmt = detect_format(path)?;
        let bytes = std::fs::read(path)?;
        let obj: Self = match fmt {
            FileFormat::Json => serde_json::from_slice(&bytes)?,
            FileFormat::Cbor => ciborium::from_reader(bytes.as_slice())
                .map_err(|e| A205Error::Cbor(format!("{}", e)))?,
        };
        if obj.metadata.schema != "RS0006" {
            return Err(A205Error::SchemaMismatch {
                expected: "RS0006".into(),
                found: obj.metadata.schema.clone(),
            });
        }
        Ok(obj)
    }
}

pub struct ElectronicDriveEfficiency {
    grid: NdGrid,
    efficiency: Vec<f64>,
    pub maximum_power: f64,
    pub standby_power: f64,
}

impl ElectronicDriveEfficiency {
    pub fn new(rs: &Rs0006) -> Result<Self, A205Error> {
        let g = &rs.performance.performance_map.grid_variables;
        let axes = vec![
            Axis::new("output_power", g.output_power.clone())
                .map_err(|e| A205Error::Other(format!("{}", e)))?,
            Axis::new("output_frequency", g.output_frequency.clone())
                .map_err(|e| A205Error::Other(format!("{}", e)))?,
        ];
        let grid = NdGrid::new(axes);
        Ok(Self {
            grid,
            efficiency: rs
                .performance
                .performance_map
                .lookup_variables
                .efficiency
                .clone(),
            maximum_power: rs.performance.maximum_power,
            standby_power: rs.performance.standby_power,
        })
    }

    pub fn efficiency_at(&self, output_power_w: f64, output_frequency_hz: f64) -> f64 {
        self.grid
            .interp(&self.efficiency, &[output_power_w, output_frequency_hz])
            .unwrap_or(0.95)
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
            .join("Drive-Constant-Efficiency.RS0006.a205.json")
    }

    #[test]
    fn loads_drive() {
        let rs = Rs0006::load(&example()).unwrap();
        assert_eq!(rs.metadata.schema, "RS0006");
        let eff = ElectronicDriveEfficiency::new(&rs).unwrap();
        assert_relative_eq!(eff.efficiency_at(5000.0, 60.0), 0.985, epsilon = 1e-6);
    }
}
