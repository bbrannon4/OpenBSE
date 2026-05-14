//! ASHRAE Standard 205 RS0003 — Fan Assembly.
//!
//! 2-D performance map of (volumetric flow, static pressure rise) →
//! (impeller rotational speed, shaft power).  Optionally embeds an
//! `assembly_components` list of nested RS0005 (motor) / RS0007 (belt
//! drive) sub-objects, used to compute total grid-side electric power
//! from shaft power.
//!
//! All quantities SI: flow in m³/s standard air, pressure in Pa, speed in
//! rev/s, power in W.
//!
//! ### Assembly components
//!
//! The `assembly_components` field may contain RS0005 motors and RS0007
//! mechanical drives.  When present, the chain from fan shaft back to
//! grid electric is, in series:
//!
//! 1. **Belt drive (RS0007)** — divides shaft power by drive efficiency
//!    and multiplies speed by 1/speed_ratio to recover motor shaft speed.
//! 2. **Motor (RS0005)** — divides motor shaft power by motor efficiency
//!    to get motor electric input.
//! 3. **VFD (RS0006)** — nested inside the motor's `drive_representation`;
//!    divides motor electric by drive efficiency to get grid electric.
//!
//! When `assembly_components` is empty (as in the published example file),
//! the caller is expected to supply a default motor efficiency.

use crate::interpolate::{Axis, NdGrid};
use crate::rs0005::{MotorEfficiency, Rs0005};
use crate::rs0006::ElectronicDriveEfficiency;
use crate::rs0007::{DriveEfficiency, Rs0007};
use crate::{detect_format, A205Error, FileFormat};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Rs0003 {
    pub metadata: super::rs0001::Metadata,
    #[serde(default)]
    pub description: Option<serde_json::Value>,
    pub performance: Performance,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Performance {
    /// Design (nominal) volumetric flow rate of standard air [m³/s]
    pub nominal_standard_air_volumetric_flow_rate: f64,
    #[serde(default)]
    pub is_enclosed: bool,
    /// Fraction of motor / drive losses that end up as heat in the air
    /// stream (0 = all losses leave the air, 1 = all losses heat the air).
    /// JSON files sometimes encode this as integer 0 or 1; serde will
    /// promote either to f64.
    #[serde(default)]
    pub heat_loss_fraction: f64,
    pub maximum_impeller_rotational_speed: f64,
    pub minimum_impeller_rotational_speed: f64,
    pub operation_speed_control_type: String,
    pub installation_speed_control_type: String,
    #[serde(default)]
    pub stability_curve: Option<serde_json::Value>,
    pub performance_map: PerformanceMap,
    /// Optional motor / drive sub-components.  Each entry is one of the
    /// nested RS types (RS0005 motor, RS0007 belt drive).  Unknown types
    /// are ignored.
    #[serde(default)]
    pub assembly_components: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PerformanceMap {
    pub grid_variables: GridVars,
    pub lookup_variables: LookupVars,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GridVars {
    /// Volumetric flow rate of standard air [m³/s]
    pub standard_air_volumetric_flow_rate: Vec<f64>,
    /// Fan static pressure rise [Pa]
    pub static_pressure_difference: Vec<f64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LookupVars {
    /// Impeller rotational speed [rev/s]
    pub impeller_rotational_speed: Vec<f64>,
    /// Shaft input power [W]
    pub shaft_power: Vec<f64>,
    #[serde(default)]
    pub operation_state: Option<Vec<String>>,
}

impl Rs0003 {
    pub fn load(path: &Path) -> Result<Self, A205Error> {
        let fmt = detect_format(path)?;
        let bytes = std::fs::read(path)?;
        let obj: Self = match fmt {
            FileFormat::Json => serde_json::from_slice(&bytes)?,
            FileFormat::Cbor => ciborium::from_reader(bytes.as_slice())
                .map_err(|e| A205Error::Cbor(format!("{}", e)))?,
        };
        if obj.metadata.schema != "RS0003" {
            return Err(A205Error::SchemaMismatch {
                expected: "RS0003".into(),
                found: obj.metadata.schema.clone(),
            });
        }
        Ok(obj)
    }

    /// Extract any nested motor (RS0005) and belt drive (RS0007) from
    /// `assembly_components`.  Returns the first of each type, since the
    /// example schema supports at most one of each on a fan.
    pub fn assembly_motor_and_drive(&self) -> (Option<Rs0005>, Option<Rs0007>) {
        let mut motor: Option<Rs0005> = None;
        let mut drive: Option<Rs0007> = None;
        for v in &self.performance.assembly_components {
            let schema = v
                .get("metadata")
                .and_then(|m| m.get("schema"))
                .and_then(|s| s.as_str())
                .unwrap_or("");
            match schema {
                "RS0005" => {
                    if motor.is_none() {
                        motor = serde_json::from_value::<Rs0005>(v.clone()).ok();
                    }
                }
                "RS0007" => {
                    if drive.is_none() {
                        drive = serde_json::from_value::<Rs0007>(v.clone()).ok();
                    }
                }
                _ => { /* ignore other sub-components for now */ }
            }
        }
        (motor, drive)
    }
}

// ─── Fan performance-map interpolator ──────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct FanQuery {
    pub volumetric_flow_m3_s: f64,
    pub static_pressure_pa: f64,
}

#[derive(Debug, Clone, Copy)]
pub struct FanResult {
    pub impeller_speed_rev_s: f64,
    pub shaft_power_w: f64,
    pub in_range: bool,
}

pub struct FanInterpolator {
    grid: NdGrid,
    impeller_speed: Vec<f64>,
    shaft_power: Vec<f64>,
    bounds: Vec<(f64, f64)>,
}

impl FanInterpolator {
    pub fn new(map: &PerformanceMap) -> Result<Self, A205Error> {
        let g = &map.grid_variables;
        let flow_axis = Axis::new(
            "standard_air_volumetric_flow_rate",
            g.standard_air_volumetric_flow_rate.clone(),
        )
        .map_err(|e| A205Error::Other(format!("{}", e)))?;
        let p_axis = Axis::new(
            "static_pressure_difference",
            g.static_pressure_difference.clone(),
        )
        .map_err(|e| A205Error::Other(format!("{}", e)))?;
        let bounds = vec![
            (
                *g.standard_air_volumetric_flow_rate.first().unwrap(),
                *g.standard_air_volumetric_flow_rate.last().unwrap(),
            ),
            (
                *g.static_pressure_difference.first().unwrap(),
                *g.static_pressure_difference.last().unwrap(),
            ),
        ];
        let grid = NdGrid::new(vec![flow_axis, p_axis]);
        let l = &map.lookup_variables;
        grid.validate(l.impeller_rotational_speed.len())
            .map_err(|e| A205Error::Other(format!("impeller_rotational_speed: {}", e)))?;
        grid.validate(l.shaft_power.len())
            .map_err(|e| A205Error::Other(format!("shaft_power: {}", e)))?;
        Ok(Self {
            grid,
            impeller_speed: l.impeller_rotational_speed.clone(),
            shaft_power: l.shaft_power.clone(),
            bounds,
        })
    }

    pub fn query(&self, q: &FanQuery) -> Result<FanResult, A205Error> {
        let qv = [q.volumetric_flow_m3_s, q.static_pressure_pa];
        let mut in_range = true;
        for (val, (lo, hi)) in qv.iter().zip(self.bounds.iter()) {
            if *val < *lo - 1e-9 || *val > *hi + 1e-9 {
                in_range = false;
                break;
            }
        }
        let speed = self
            .grid
            .interp(&self.impeller_speed, &qv)
            .map_err(|e| A205Error::Other(format!("{}", e)))?;
        let shaft = self
            .grid
            .interp(&self.shaft_power, &qv)
            .map_err(|e| A205Error::Other(format!("{}", e)))?;
        Ok(FanResult {
            impeller_speed_rev_s: speed,
            shaft_power_w: shaft.max(0.0),
            in_range,
        })
    }
}

// ─── Composite efficiency chain (shaft → grid) ─────────────────────────────

/// Composes the optional belt drive (RS0007), motor (RS0005), and VFD
/// (RS0006 nested inside the motor) into a single function:
/// `grid_electric_power = chain.grid_electric_power(fan_shaft_w, fan_speed)`.
///
/// When a stage is absent, that stage's efficiency is 1.0.  When the
/// motor itself is absent, `fallback_motor_efficiency` is used for the
/// shaft → electric conversion.
pub struct FanEfficiencyChain {
    pub motor: Option<MotorEfficiency>,
    pub motor_poles: u32,
    pub drive: Option<DriveEfficiency>,
    pub vfd: Option<ElectronicDriveEfficiency>,
    pub fallback_motor_efficiency: f64,
}

impl FanEfficiencyChain {
    pub fn from_assembly(
        rs0003: &Rs0003,
        fallback_motor_efficiency: f64,
    ) -> Result<Self, A205Error> {
        Self::from_assembly_with_overrides(rs0003, fallback_motor_efficiency, None, None, None)
    }

    /// Same as [`from_assembly`], but with optional user-supplied
    /// overrides for the motor, mechanical drive, and electronic drive
    /// (VFD).  Each override, when present, **replaces** the
    /// corresponding sub-component from the RS0003 file's
    /// `assembly_components` list (or from the motor's nested
    /// `drive_representation` for the VFD).  Overrides are independent —
    /// supplying only a motor override leaves the file's belt drive
    /// (if any) intact.
    pub fn from_assembly_with_overrides(
        rs0003: &Rs0003,
        fallback_motor_efficiency: f64,
        motor_override: Option<crate::rs0005::Rs0005>,
        drive_override: Option<Rs0007>,
        vfd_override: Option<crate::rs0006::Rs0006>,
    ) -> Result<Self, A205Error> {
        let (file_motor, file_drive) = rs0003.assembly_motor_and_drive();
        let motor_rs = motor_override.or(file_motor);
        let drive_rs = drive_override.or(file_drive);
        // VFD precedence: standalone override > nested-in-motor (after
        // override choice) > none.
        let nested_vfd = motor_rs
            .as_ref()
            .and_then(|m| m.performance.drive_representation.clone());
        let vfd_rs = vfd_override.or(nested_vfd);

        let motor = motor_rs.as_ref().map(MotorEfficiency::new).transpose()?;
        let motor_poles = motor_rs
            .as_ref()
            .map(|r| r.performance.number_of_poles)
            .unwrap_or(4);
        let vfd = vfd_rs
            .as_ref()
            .map(ElectronicDriveEfficiency::new)
            .transpose()?;
        let drive = drive_rs.as_ref().map(DriveEfficiency::new).transpose()?;
        Ok(Self {
            motor,
            motor_poles,
            drive,
            vfd,
            fallback_motor_efficiency: fallback_motor_efficiency.clamp(0.05, 1.0),
        })
    }

    /// Compute grid-side electric power given fan shaft power [W] and
    /// fan impeller rotational speed [rev/s].
    pub fn grid_electric_power(&self, fan_shaft_w: f64, fan_speed_rev_s: f64) -> f64 {
        if fan_shaft_w <= 0.0 {
            return 0.0;
        }
        // 1) Belt drive: motor shaft power = fan shaft / drive_eff
        let (motor_shaft_w, motor_shaft_speed) = match &self.drive {
            Some(d) => {
                let eff = d.efficiency_at(fan_shaft_w);
                let motor_w = fan_shaft_w / eff;
                // motor speed is higher when speed_ratio < 1
                let speed = if d.speed_ratio > 1e-6 {
                    fan_speed_rev_s / d.speed_ratio
                } else {
                    fan_speed_rev_s
                };
                (motor_w, speed)
            }
            None => (fan_shaft_w, fan_speed_rev_s),
        };
        // 2) Motor: electric input = shaft / motor_eff
        let motor_eff = match &self.motor {
            Some(m) => m.efficiency_at(motor_shaft_w, motor_shaft_speed),
            None => self.fallback_motor_efficiency,
        };
        let motor_electric_w = motor_shaft_w / motor_eff.max(1e-3);
        // 3) VFD: grid electric = motor_electric / vfd_eff
        match &self.vfd {
            Some(v) => {
                // Convert shaft speed to VFD output frequency.
                // For a synchronous motor at no slip: freq = shaft_speed × poles / 2.
                let freq_hz = motor_shaft_speed * (self.motor_poles as f64) / 2.0;
                let vfd_eff = v.efficiency_at(motor_electric_w, freq_hz);
                motor_electric_w / vfd_eff.max(1e-3)
            }
            None => motor_electric_w,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_relative_eq;

    fn example_path() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("examples")
            .join("Fan-Continuous.RS0003.a205.json")
    }

    #[test]
    fn loads_fan() {
        let rs = Rs0003::load(&example_path()).unwrap();
        assert_eq!(rs.metadata.schema, "RS0003");
        assert!(rs.performance.nominal_standard_air_volumetric_flow_rate > 0.0);
        // The published example has an empty assembly_components list.
        assert!(rs.performance.assembly_components.is_empty());
    }

    #[test]
    fn fan_map_query_recovers_first_corner() {
        let rs = Rs0003::load(&example_path()).unwrap();
        let g = &rs.performance.performance_map.grid_variables;
        let lk = &rs.performance.performance_map.lookup_variables;
        let interp = FanInterpolator::new(&rs.performance.performance_map).unwrap();
        let q = FanQuery {
            volumetric_flow_m3_s: g.standard_air_volumetric_flow_rate[0],
            static_pressure_pa: g.static_pressure_difference[0],
        };
        let r = interp.query(&q).unwrap();
        assert!(r.in_range);
        assert_relative_eq!(
            r.impeller_speed_rev_s,
            lk.impeller_rotational_speed[0],
            epsilon = 1e-6
        );
        assert_relative_eq!(r.shaft_power_w, lk.shaft_power[0], epsilon = 1e-6);
    }

    #[test]
    fn efficiency_chain_empty_assembly_uses_fallback() {
        let rs = Rs0003::load(&example_path()).unwrap();
        let chain = FanEfficiencyChain::from_assembly(&rs, 0.9).unwrap();
        let grid = chain.grid_electric_power(1000.0, 20.0);
        // With no motor / drive in the assembly, grid = shaft / 0.9
        assert_relative_eq!(grid, 1000.0 / 0.9, epsilon = 1e-6);
    }
}
