//! ASHRAE Standard 205 RS0005 — Electric Motor.
//!
//! Efficiency and power factor as functions of shaft power and shaft
//! rotational speed.  May optionally embed an RS0006 drive (VFD) as
//! `drive_representation`.

use crate::interpolate::{Axis, NdGrid};
use crate::rs0006::Rs0006;
use crate::{detect_format, A205Error, FileFormat};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Rs0005 {
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
    /// Pole count of the motor.  Used to estimate VFD output frequency
    /// from shaft rotational speed:  `freq = shaft_speed × poles / 2`.
    #[serde(default)]
    pub number_of_poles: u32,
    /// Optional nested RS0006 drive.  When present, the motor's electric
    /// input is itself fed by a VFD whose efficiency must be folded in to
    /// compute total grid-side electric power.
    #[serde(default)]
    pub drive_representation: Option<Rs0006>,
    pub performance_map: PerformanceMap,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PerformanceMap {
    pub grid_variables: GridVars,
    pub lookup_variables: LookupVars,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GridVars {
    /// Mechanical shaft output power [W]
    pub shaft_power: Vec<f64>,
    /// Shaft rotational speed [rev/s]
    pub shaft_rotational_speed: Vec<f64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LookupVars {
    pub efficiency: Vec<f64>,
    #[serde(default)]
    pub power_factor: Option<Vec<f64>>,
    #[serde(default)]
    pub operation_state: Option<Vec<String>>,
}

impl Rs0005 {
    pub fn load(path: &Path) -> Result<Self, A205Error> {
        let fmt = detect_format(path)?;
        let bytes = std::fs::read(path)?;
        let obj: Self = match fmt {
            FileFormat::Json => serde_json::from_slice(&bytes)?,
            FileFormat::Cbor => ciborium::from_reader(bytes.as_slice())
                .map_err(|e| A205Error::Cbor(format!("{}", e)))?,
        };
        if obj.metadata.schema != "RS0005" {
            return Err(A205Error::SchemaMismatch {
                expected: "RS0005".into(),
                found: obj.metadata.schema.clone(),
            });
        }
        Ok(obj)
    }
}

pub struct MotorEfficiency {
    grid: NdGrid,
    efficiency: Vec<f64>,
    pub number_of_poles: u32,
}

impl MotorEfficiency {
    pub fn new(rs: &Rs0005) -> Result<Self, A205Error> {
        let g = &rs.performance.performance_map.grid_variables;
        let axes = vec![
            Axis::new("shaft_power", g.shaft_power.clone())
                .map_err(|e| A205Error::Other(format!("{}", e)))?,
            Axis::new("shaft_rotational_speed", g.shaft_rotational_speed.clone())
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
            number_of_poles: rs.performance.number_of_poles,
        })
    }

    pub fn efficiency_at(&self, shaft_power_w: f64, shaft_speed_rev_per_s: f64) -> f64 {
        self.grid
            .interp(&self.efficiency, &[shaft_power_w, shaft_speed_rev_per_s])
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
            .join("Motor-Constant-Efficiency.RS0005.a205.json")
    }

    #[test]
    fn loads_motor() {
        let rs = Rs0005::load(&example()).unwrap();
        assert_eq!(rs.metadata.schema, "RS0005");
        assert_eq!(rs.performance.number_of_poles, 4);
        // The example file embeds a nested RS0006 drive
        assert!(rs.performance.drive_representation.is_some());
        let eff = MotorEfficiency::new(&rs).unwrap();
        assert_relative_eq!(eff.efficiency_at(5000.0, 30.0), 0.92, epsilon = 1e-6);
    }
}
