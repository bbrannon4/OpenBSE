//! ASHRAE Standard 205 RS0001 — Chiller representation specification.
//!
//! All quantities are SI: temperatures in **kelvin** (K), pressures in pascals
//! (Pa), volumetric flow in cubic metres per second (m³/s), power in watts (W).
//! Callers must convert their domain units (e.g. °C) before querying the maps.
//!
//! Reference: https://data.ashrae.org/standard205/assets/schema/RS0001.schema.json
//!
//! ### Schema coverage
//!
//! This module covers the fields needed to drive a chiller simulation:
//! `metadata`, `performance.condenser_type`,
//! `performance.compressor_speed_control_type`,
//! `performance.cycling_degradation_coefficient`,
//! `performance.scaling`, and the cooling and standby performance maps.
//! Pressure-drop maps, fouling factors, fluid composition, and AHRI rating
//! data are deserialized into permissive types but not yet consumed.

use crate::interpolate::{Axis, NdGrid};
use crate::{detect_format, A205Error, FileFormat};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Rs0001 {
    pub metadata: Metadata,
    #[serde(default)]
    pub description: Option<Description>,
    pub performance: Performance,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Metadata {
    pub data_model: String,
    pub schema: String,
    pub schema_version: String,
    #[serde(default)]
    pub description: String,
    pub id: String,
    #[serde(default)]
    pub data_timestamp: String,
    #[serde(default)]
    pub data_version: u32,
    #[serde(default)]
    pub data_source: String,
    #[serde(default)]
    pub disclaimer: String,
    #[serde(default)]
    pub notes: String,
}

/// Optional descriptive block — product info, rating data.  Not consumed by
/// the simulation but kept around for round-tripping and reporting.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Description {
    #[serde(default)]
    pub product_information: Option<serde_json::Value>,
    #[serde(default)]
    pub rating_ahri_550_590: Option<serde_json::Value>,
    #[serde(default)]
    pub rating_ahri_551_591: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CondenserType {
    Air,
    Liquid,
    Evaporative,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SpeedControl {
    Continuous,
    Discrete,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Performance {
    pub condenser_type: CondenserType,
    pub compressor_speed_control_type: SpeedControl,
    #[serde(default)]
    pub cycling_degradation_coefficient: f64,
    #[serde(default)]
    pub evaporator_fouling_factor: f64,
    #[serde(default)]
    pub condenser_fouling_factor: f64,
    /// Fluid composition (not yet consumed).
    #[serde(default)]
    pub evaporator_liquid_type: Option<serde_json::Value>,
    #[serde(default)]
    pub condenser_liquid_type: Option<serde_json::Value>,
    pub performance_map_cooling: CoolingMap,
    #[serde(default)]
    pub performance_map_standby: Option<StandbyMap>,
    /// Pressure-drop maps (not yet consumed).
    #[serde(default)]
    pub performance_map_evaporator_liquid_pressure_differential: Option<serde_json::Value>,
    #[serde(default)]
    pub performance_map_condenser_liquid_pressure_differential: Option<serde_json::Value>,
    #[serde(default)]
    pub scaling: Option<Scaling>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
pub struct Scaling {
    /// Maximum allowable scaling factor relative to the published map.
    pub maximum: f64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CoolingMap {
    pub grid_variables: CoolingGridVars,
    pub lookup_variables: CoolingLookupVars,
}

/// Grid axes for the cooling performance map.  Which axes are populated
/// depends on `condenser_type`:
///   - Air / Evaporative: condenser_air_* and ambient_pressure
///   - Liquid:            condenser_liquid_*
///
/// `compressor_sequence_number` is always present and indexes loading
/// stages (1 = minimum unloading, N = full load).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CoolingGridVars {
    pub evaporator_liquid_volumetric_flow_rate: Vec<f64>,
    /// Leaving chilled water temperature [K]
    pub evaporator_liquid_leaving_temperature: Vec<f64>,

    // Air/Evaporative condenser axes
    #[serde(default)]
    pub condenser_air_entering_drybulb_temperature: Option<Vec<f64>>,
    #[serde(default)]
    pub condenser_air_entering_relative_humidity: Option<Vec<f64>>,
    #[serde(default)]
    pub condenser_air_entering_wetbulb_temperature: Option<Vec<f64>>,
    #[serde(default)]
    pub ambient_pressure: Option<Vec<f64>>,

    // Liquid condenser axes
    #[serde(default)]
    pub condenser_liquid_volumetric_flow_rate: Option<Vec<f64>>,
    #[serde(default)]
    pub condenser_liquid_entering_temperature: Option<Vec<f64>>,

    pub compressor_sequence_number: Vec<f64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CoolingLookupVars {
    pub input_power: Vec<f64>,
    pub net_evaporator_capacity: Vec<f64>,
    #[serde(default)]
    pub net_condenser_capacity: Option<Vec<f64>>,
    #[serde(default)]
    pub condenser_air_volumetric_flow_rate: Option<Vec<f64>>,
    #[serde(default)]
    pub oil_cooler_heat: Option<Vec<f64>>,
    #[serde(default)]
    pub auxiliary_heat: Option<Vec<f64>>,
    #[serde(default)]
    pub operation_state: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct StandbyMap {
    pub grid_variables: StandbyGridVars,
    pub lookup_variables: StandbyLookupVars,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct StandbyGridVars {
    pub environment_dry_bulb_temperature: Vec<f64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct StandbyLookupVars {
    pub input_power: Vec<f64>,
}

// ─── Loading ────────────────────────────────────────────────────────────────

impl Rs0001 {
    /// Load from a file, auto-detecting JSON vs CBOR.
    pub fn load(path: &Path) -> Result<Self, A205Error> {
        let fmt = detect_format(path)?;
        let bytes = std::fs::read(path)?;
        let obj: Self = match fmt {
            FileFormat::Json => serde_json::from_slice(&bytes)?,
            FileFormat::Cbor => ciborium::from_reader(bytes.as_slice())
                .map_err(|e| A205Error::Cbor(format!("{}", e)))?,
        };
        if obj.metadata.schema != "RS0001" {
            return Err(A205Error::SchemaMismatch {
                expected: "RS0001".into(),
                found: obj.metadata.schema.clone(),
            });
        }
        Ok(obj)
    }
}

// ─── Cooling-map interpolator ──────────────────────────────────────────────
//
// The cooling map has six axes for air/evaporative condensers and five for
// liquid.  Callers don't generally want to deal with that variability, so
// `CoolingInterpolator` builds an N-d grid using whichever axes are present
// in the file, and exposes a stable query API.

/// Inputs needed to query the cooling map.  All in SI units (K, m³/s, Pa, -).
/// `condenser_temp` is the dry-bulb for air-cooled or entering-liquid temp
/// for water-cooled.  Unused fields for the file's condenser type are ignored.
#[derive(Debug, Clone, Copy)]
pub struct CoolingQuery {
    pub evap_volumetric_flow: f64,
    pub evap_leaving_temp_k: f64,
    pub condenser_temp_k: f64,
    pub condenser_air_rh: f64,
    pub condenser_liquid_flow: f64,
    pub ambient_pressure_pa: f64,
    /// Real-valued compressor sequence number (1..N).  Callers translate
    /// part-load ratio to a sequence number externally.
    pub compressor_sequence: f64,
}

/// Result of a cooling-map query.
#[derive(Debug, Clone, Copy)]
pub struct CoolingResult {
    pub input_power: f64,
    pub net_evaporator_capacity: f64,
    pub net_condenser_capacity: Option<f64>,
    /// True when the query falls inside the published grid bounds.  When
    /// false, returned values are edge-clamped extrapolations and should
    /// be used cautiously.
    pub in_range: bool,
}

pub struct CoolingInterpolator {
    grid: NdGrid,
    axis_order: Vec<AxisKind>,
    input_power: Vec<f64>,
    net_evaporator_capacity: Vec<f64>,
    net_condenser_capacity: Option<Vec<f64>>,
    /// Min/max along each numeric query dimension for in-range checks.
    bounds: Vec<(f64, f64)>,
    /// Number of compressor sequence steps (length of sequence axis).
    pub n_sequence_steps: usize,
    /// Compressor sequence axis range (min, max).
    pub sequence_range: (f64, f64),
}

#[derive(Debug, Clone, Copy)]
enum AxisKind {
    EvapFlow,
    EvapLeavingTemp,
    CondenserAirDrybulb,
    CondenserAirRH,
    CondenserAirWetbulb,
    AmbientPressure,
    CondenserLiquidFlow,
    CondenserLiquidEntering,
    CompressorSequence,
}

impl CoolingInterpolator {
    pub fn new(map: &CoolingMap) -> Result<Self, A205Error> {
        let g = &map.grid_variables;
        let l = &map.lookup_variables;

        // Build axes in the same order they appear in the file's grid_variables
        // map.  Standard 205 doesn't strictly mandate row-major axis order, but
        // every example file we've inspected uses the order: evap_flow,
        // evap_leaving_temp, condenser_*, [air_rh, ambient_pressure],
        // compressor_sequence.  We replicate that ordering.
        let mut axes: Vec<Axis> = Vec::new();
        let mut kinds: Vec<AxisKind> = Vec::new();
        let mut bounds: Vec<(f64, f64)> = Vec::new();

        let push = |name: &str,
                    vals: &[f64],
                    kind: AxisKind,
                    axes: &mut Vec<Axis>,
                    kinds: &mut Vec<AxisKind>,
                    bounds: &mut Vec<(f64, f64)>|
         -> Result<(), A205Error> {
            let axis =
                Axis::new(name, vals.to_vec()).map_err(|e| A205Error::Other(format!("{}", e)))?;
            let lo = *vals.first().unwrap();
            let hi = *vals.last().unwrap();
            axes.push(axis);
            kinds.push(kind);
            bounds.push((lo, hi));
            Ok(())
        };

        push(
            "evaporator_liquid_volumetric_flow_rate",
            &g.evaporator_liquid_volumetric_flow_rate,
            AxisKind::EvapFlow,
            &mut axes,
            &mut kinds,
            &mut bounds,
        )?;
        push(
            "evaporator_liquid_leaving_temperature",
            &g.evaporator_liquid_leaving_temperature,
            AxisKind::EvapLeavingTemp,
            &mut axes,
            &mut kinds,
            &mut bounds,
        )?;

        // Condenser axes (variable set)
        if let Some(v) = &g.condenser_liquid_volumetric_flow_rate {
            push(
                "condenser_liquid_volumetric_flow_rate",
                v,
                AxisKind::CondenserLiquidFlow,
                &mut axes,
                &mut kinds,
                &mut bounds,
            )?;
        }
        if let Some(v) = &g.condenser_liquid_entering_temperature {
            push(
                "condenser_liquid_entering_temperature",
                v,
                AxisKind::CondenserLiquidEntering,
                &mut axes,
                &mut kinds,
                &mut bounds,
            )?;
        }
        if let Some(v) = &g.condenser_air_entering_drybulb_temperature {
            push(
                "condenser_air_entering_drybulb_temperature",
                v,
                AxisKind::CondenserAirDrybulb,
                &mut axes,
                &mut kinds,
                &mut bounds,
            )?;
        }
        if let Some(v) = &g.condenser_air_entering_wetbulb_temperature {
            push(
                "condenser_air_entering_wetbulb_temperature",
                v,
                AxisKind::CondenserAirWetbulb,
                &mut axes,
                &mut kinds,
                &mut bounds,
            )?;
        }
        if let Some(v) = &g.condenser_air_entering_relative_humidity {
            push(
                "condenser_air_entering_relative_humidity",
                v,
                AxisKind::CondenserAirRH,
                &mut axes,
                &mut kinds,
                &mut bounds,
            )?;
        }
        if let Some(v) = &g.ambient_pressure {
            push(
                "ambient_pressure",
                v,
                AxisKind::AmbientPressure,
                &mut axes,
                &mut kinds,
                &mut bounds,
            )?;
        }

        let n_seq = g.compressor_sequence_number.len();
        let seq_range = (
            *g.compressor_sequence_number.first().unwrap(),
            *g.compressor_sequence_number.last().unwrap(),
        );
        push(
            "compressor_sequence_number",
            &g.compressor_sequence_number,
            AxisKind::CompressorSequence,
            &mut axes,
            &mut kinds,
            &mut bounds,
        )?;

        let grid = NdGrid::new(axes);
        grid.validate(l.input_power.len())
            .map_err(|e| A205Error::Other(format!("input_power: {}", e)))?;
        grid.validate(l.net_evaporator_capacity.len())
            .map_err(|e| A205Error::Other(format!("net_evaporator_capacity: {}", e)))?;
        if let Some(v) = &l.net_condenser_capacity {
            grid.validate(v.len())
                .map_err(|e| A205Error::Other(format!("net_condenser_capacity: {}", e)))?;
        }

        Ok(Self {
            grid,
            axis_order: kinds,
            input_power: l.input_power.clone(),
            net_evaporator_capacity: l.net_evaporator_capacity.clone(),
            net_condenser_capacity: l.net_condenser_capacity.clone(),
            bounds,
            n_sequence_steps: n_seq,
            sequence_range: seq_range,
        })
    }

    fn build_query(&self, q: &CoolingQuery) -> Vec<f64> {
        self.axis_order
            .iter()
            .map(|k| match k {
                AxisKind::EvapFlow => q.evap_volumetric_flow,
                AxisKind::EvapLeavingTemp => q.evap_leaving_temp_k,
                AxisKind::CondenserAirDrybulb => q.condenser_temp_k,
                AxisKind::CondenserAirWetbulb => q.condenser_temp_k,
                AxisKind::CondenserAirRH => q.condenser_air_rh,
                AxisKind::AmbientPressure => q.ambient_pressure_pa,
                AxisKind::CondenserLiquidFlow => q.condenser_liquid_flow,
                AxisKind::CondenserLiquidEntering => q.condenser_temp_k,
                AxisKind::CompressorSequence => q.compressor_sequence,
            })
            .collect()
    }

    pub fn query(&self, q: &CoolingQuery) -> Result<CoolingResult, A205Error> {
        let qv = self.build_query(q);
        let mut in_range = true;
        for (val, (lo, hi)) in qv.iter().zip(self.bounds.iter()) {
            if *val < *lo - 1e-9 || *val > *hi + 1e-9 {
                in_range = false;
                break;
            }
        }
        let input_power = self
            .grid
            .interp(&self.input_power, &qv)
            .map_err(|e| A205Error::Other(format!("{}", e)))?;
        let net_evap = self
            .grid
            .interp(&self.net_evaporator_capacity, &qv)
            .map_err(|e| A205Error::Other(format!("{}", e)))?;
        let net_cond = if let Some(v) = &self.net_condenser_capacity {
            Some(
                self.grid
                    .interp(v, &qv)
                    .map_err(|e| A205Error::Other(format!("{}", e)))?,
            )
        } else {
            None
        };
        Ok(CoolingResult {
            input_power,
            net_evaporator_capacity: net_evap,
            net_condenser_capacity: net_cond,
            in_range,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_relative_eq;

    fn example_path() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("examples")
            .join("RS0001_AppJ_CurveSetA.a205.json")
    }

    #[test]
    fn loads_curve_set_a() {
        let rs = Rs0001::load(&example_path()).expect("load curve set A");
        assert_eq!(rs.metadata.schema, "RS0001");
        assert!(matches!(rs.performance.condenser_type, CondenserType::Air));
        assert!(matches!(
            rs.performance.compressor_speed_control_type,
            SpeedControl::Continuous
        ));
    }

    #[test]
    fn cooling_interpolator_at_full_load() {
        // Curve Set A:
        //   evap_flow = [0.01130153653374901]
        //   evap_leaving_temp = [275.37, 281.67, 287.96, 294.26]  (K)
        //   cond_air_drybulb = [285.93, 299.00, 312.08, 325.15]   (K)
        //   cond_air_rh = [0.4]
        //   ambient_pressure = [101325.0]
        //   compressor_sequence_number = [1, 2, 3, 4]
        // Lookup arrays are 1*4*4*1*1*4 = 64 elements, row-major.
        // The first element of input_power is at indices (0,0,0,0,0,0):
        //   evap_leaving=275.37, cond=285.93, seq=1 → 14192.03 W
        // The 4th element (..., seq=4) is the max-loading point at the
        // coldest condenser & coldest CHW: 65032.20 W.
        let rs = Rs0001::load(&example_path()).expect("load");
        let interp = CoolingInterpolator::new(&rs.performance.performance_map_cooling).unwrap();

        // Full-load (seq=4), at the first interior grid point
        let q = CoolingQuery {
            evap_volumetric_flow: 0.01130153653374901,
            evap_leaving_temp_k: 275.37222222222226,
            condenser_temp_k: 285.9277777777778,
            condenser_air_rh: 0.4,
            condenser_liquid_flow: 0.0,
            ambient_pressure_pa: 101325.0,
            compressor_sequence: 4.0,
        };
        let r = interp.query(&q).unwrap();
        assert!(r.in_range);
        assert_relative_eq!(r.input_power, 65032.20311497696, epsilon = 1e-6);
        assert_relative_eq!(r.net_evaporator_capacity, 242483.9307795564, epsilon = 1e-6);
    }

    #[test]
    fn cooling_interpolator_edge_clamp() {
        let rs = Rs0001::load(&example_path()).expect("load");
        let interp = CoolingInterpolator::new(&rs.performance.performance_map_cooling).unwrap();
        // Way out of range — should clamp to edge and report in_range = false
        let q = CoolingQuery {
            evap_volumetric_flow: 0.01130153653374901,
            evap_leaving_temp_k: 100.0, // far below grid min
            condenser_temp_k: 285.9277777777778,
            condenser_air_rh: 0.4,
            condenser_liquid_flow: 0.0,
            ambient_pressure_pa: 101325.0,
            compressor_sequence: 4.0,
        };
        let r = interp.query(&q).unwrap();
        assert!(!r.in_range);
        // Edge clamp should give the same value as at the first grid point
        assert_relative_eq!(r.input_power, 65032.20311497696, epsilon = 1e-6);
    }
}
