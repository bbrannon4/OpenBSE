//! ASHRAE Standard 205 RS0004 — Air-to-Air Direct Expansion Refrigerant System.
//!
//! Covers packaged DX cooling units (rooftop equipment, split systems,
//! residential AC).  The cooling map is a 6-axis grid that returns gross
//! total capacity, gross sensible capacity, and gross compressor power —
//! "gross" meaning before fan effects.
//!
//! All quantities are SI: temperatures in **kelvin** (K), pressures in
//! pascals (Pa), mass flow in kg/s, power in watts (W).
//!
//! Reference: <https://data.ashrae.org/standard205/assets/schema/RS0004.schema.json>
//!
//! ### A note on example-file unit bugs
//!
//! ASHRAE's `DX-Constant-Efficiency.RS0004.a205.json` example has the
//! pressure axis values `[81.273, 101.325]` which appear to be in kPa
//! rather than Pa.  The schema mandates Pa; the residential example file
//! (`residential-dx.RS0004.json`) is correct.  This crate trusts the
//! schema's stated unit and edge-clamps when the query falls outside the
//! axis range (logging `in_range = false`).

use crate::interpolate::{Axis, NdGrid};
use crate::{detect_format, A205Error, FileFormat};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Rs0004 {
    pub metadata: super::rs0001::Metadata,
    #[serde(default)]
    pub description: Option<serde_json::Value>,
    pub performance: Performance,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Performance {
    pub compressor_speed_control_type: super::rs0001::SpeedControl,
    #[serde(default)]
    pub cycling_degradation_coefficient: f64,
    pub performance_map_cooling: CoolingMap,
    #[serde(default)]
    pub performance_map_standby: Option<StandbyMap>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CoolingMap {
    pub grid_variables: CoolingGridVars,
    pub lookup_variables: CoolingLookupVars,
}

/// All six axes are always present in RS0004 cooling maps.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CoolingGridVars {
    pub outdoor_coil_entering_dry_bulb_temperature: Vec<f64>,
    pub indoor_coil_entering_relative_humidity: Vec<f64>,
    pub indoor_coil_entering_dry_bulb_temperature: Vec<f64>,
    pub indoor_coil_air_mass_flow_rate: Vec<f64>,
    pub compressor_sequence_number: Vec<f64>,
    pub ambient_absolute_air_pressure: Vec<f64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CoolingLookupVars {
    /// Gross total cooling capacity [W] — sensible + latent, fan effects excluded
    pub gross_total_capacity: Vec<f64>,
    /// Gross sensible cooling capacity [W] — fan effects excluded
    pub gross_sensible_capacity: Vec<f64>,
    /// Gross compressor electric power [W] — does not include indoor fan
    pub gross_power: Vec<f64>,
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
    pub outdoor_coil_environment_dry_bulb_temperature: Vec<f64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct StandbyLookupVars {
    pub gross_power: Vec<f64>,
}

// ─── Loading ────────────────────────────────────────────────────────────────

impl Rs0004 {
    /// Load from a file, auto-detecting JSON vs CBOR.
    pub fn load(path: &Path) -> Result<Self, A205Error> {
        let fmt = detect_format(path)?;
        let bytes = std::fs::read(path)?;
        let obj: Self = match fmt {
            FileFormat::Json => serde_json::from_slice(&bytes)?,
            FileFormat::Cbor => ciborium::from_reader(bytes.as_slice())
                .map_err(|e| A205Error::Cbor(format!("{}", e)))?,
        };
        if obj.metadata.schema != "RS0004" {
            return Err(A205Error::SchemaMismatch {
                expected: "RS0004".into(),
                found: obj.metadata.schema.clone(),
            });
        }
        Ok(obj)
    }
}

// ─── Cooling-map interpolator ──────────────────────────────────────────────

/// Inputs needed to query the DX cooling map.  All SI units (K, kg/s, Pa).
#[derive(Debug, Clone, Copy)]
pub struct DxCoolingQuery {
    pub outdoor_db_k: f64,
    pub indoor_rh: f64,
    pub indoor_db_k: f64,
    pub indoor_mass_flow_kg_s: f64,
    /// Real-valued compressor sequence number (1..N).
    pub compressor_sequence: f64,
    pub ambient_pressure_pa: f64,
}

#[derive(Debug, Clone, Copy)]
pub struct DxCoolingResult {
    pub gross_total_capacity: f64,
    pub gross_sensible_capacity: f64,
    pub gross_power: f64,
    /// True when the query falls inside the published grid bounds.
    pub in_range: bool,
}

pub struct DxCoolingInterpolator {
    grid: NdGrid,
    gross_total: Vec<f64>,
    gross_sensible: Vec<f64>,
    gross_power: Vec<f64>,
    bounds: Vec<(f64, f64)>,
    pub n_sequence_steps: usize,
    pub sequence_range: (f64, f64),
}

impl DxCoolingInterpolator {
    pub fn new(map: &CoolingMap) -> Result<Self, A205Error> {
        let g = &map.grid_variables;
        let mk = |name: &str, vals: &[f64]| -> Result<(Axis, (f64, f64)), A205Error> {
            let axis =
                Axis::new(name, vals.to_vec()).map_err(|e| A205Error::Other(format!("{}", e)))?;
            Ok((axis, (*vals.first().unwrap(), *vals.last().unwrap())))
        };

        // Axis order matches the order they appear in the file's
        // `grid_variables` map (and the row-major lookup data).
        let pairs = [
            mk(
                "outdoor_coil_entering_dry_bulb_temperature",
                &g.outdoor_coil_entering_dry_bulb_temperature,
            )?,
            mk(
                "indoor_coil_entering_relative_humidity",
                &g.indoor_coil_entering_relative_humidity,
            )?,
            mk(
                "indoor_coil_entering_dry_bulb_temperature",
                &g.indoor_coil_entering_dry_bulb_temperature,
            )?,
            mk(
                "indoor_coil_air_mass_flow_rate",
                &g.indoor_coil_air_mass_flow_rate,
            )?,
            mk("compressor_sequence_number", &g.compressor_sequence_number)?,
            mk(
                "ambient_absolute_air_pressure",
                &g.ambient_absolute_air_pressure,
            )?,
        ];

        let n_seq = g.compressor_sequence_number.len();
        let seq_range = (
            *g.compressor_sequence_number.first().unwrap(),
            *g.compressor_sequence_number.last().unwrap(),
        );

        let axes: Vec<Axis> = pairs.iter().map(|(a, _)| a.clone()).collect();
        let bounds: Vec<(f64, f64)> = pairs.iter().map(|(_, b)| *b).collect();
        let grid = NdGrid::new(axes);
        let l = &map.lookup_variables;
        grid.validate(l.gross_total_capacity.len())
            .map_err(|e| A205Error::Other(format!("gross_total_capacity: {}", e)))?;
        grid.validate(l.gross_sensible_capacity.len())
            .map_err(|e| A205Error::Other(format!("gross_sensible_capacity: {}", e)))?;
        grid.validate(l.gross_power.len())
            .map_err(|e| A205Error::Other(format!("gross_power: {}", e)))?;

        Ok(Self {
            grid,
            gross_total: l.gross_total_capacity.clone(),
            gross_sensible: l.gross_sensible_capacity.clone(),
            gross_power: l.gross_power.clone(),
            bounds,
            n_sequence_steps: n_seq,
            sequence_range: seq_range,
        })
    }

    fn build_query(&self, q: &DxCoolingQuery) -> [f64; 6] {
        [
            q.outdoor_db_k,
            q.indoor_rh,
            q.indoor_db_k,
            q.indoor_mass_flow_kg_s,
            q.compressor_sequence,
            q.ambient_pressure_pa,
        ]
    }

    pub fn query(&self, q: &DxCoolingQuery) -> Result<DxCoolingResult, A205Error> {
        let qv = self.build_query(q);
        let mut in_range = true;
        for (val, (lo, hi)) in qv.iter().zip(self.bounds.iter()) {
            if *val < *lo - 1e-9 || *val > *hi + 1e-9 {
                in_range = false;
                break;
            }
        }
        let gt = self
            .grid
            .interp(&self.gross_total, &qv)
            .map_err(|e| A205Error::Other(format!("{}", e)))?;
        let gs = self
            .grid
            .interp(&self.gross_sensible, &qv)
            .map_err(|e| A205Error::Other(format!("{}", e)))?;
        let gp = self
            .grid
            .interp(&self.gross_power, &qv)
            .map_err(|e| A205Error::Other(format!("{}", e)))?;
        // Clamp sensible <= total (some example files publish sensible > total
        // at low entering RH due to slightly inconsistent rating procedures).
        let gs_clamped = gs.min(gt).max(0.0);
        Ok(DxCoolingResult {
            gross_total_capacity: gt.max(0.0),
            gross_sensible_capacity: gs_clamped,
            gross_power: gp.max(0.0),
            in_range,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_relative_eq;

    fn residential_path() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("examples")
            .join("RS0004_Residential.RS0004.a205.json")
    }

    #[test]
    fn loads_residential() {
        let rs = Rs0004::load(&residential_path()).expect("load residential");
        assert_eq!(rs.metadata.schema, "RS0004");
        // Residential file: 2-stage discrete compressor
        assert!(matches!(
            rs.performance.compressor_speed_control_type,
            super::super::rs0001::SpeedControl::Discrete
        ));
        assert_eq!(
            rs.performance
                .performance_map_cooling
                .grid_variables
                .compressor_sequence_number
                .len(),
            2
        );
    }

    #[test]
    fn interp_at_first_grid_point_matches_lookup_array() {
        // The residential file's first lookup point sits at all axis-zero
        // coordinates.  Confirm we can recover it exactly.
        let rs = Rs0004::load(&residential_path()).unwrap();
        let g = &rs.performance.performance_map_cooling.grid_variables;
        let lk = &rs.performance.performance_map_cooling.lookup_variables;
        let interp = DxCoolingInterpolator::new(&rs.performance.performance_map_cooling).unwrap();
        let q = DxCoolingQuery {
            outdoor_db_k: g.outdoor_coil_entering_dry_bulb_temperature[0],
            indoor_rh: g.indoor_coil_entering_relative_humidity[0],
            indoor_db_k: g.indoor_coil_entering_dry_bulb_temperature[0],
            indoor_mass_flow_kg_s: g.indoor_coil_air_mass_flow_rate[0],
            compressor_sequence: g.compressor_sequence_number[0],
            ambient_pressure_pa: g.ambient_absolute_air_pressure[0],
        };
        let r = interp.query(&q).unwrap();
        assert!(r.in_range);
        // First point in the residential file
        assert_relative_eq!(
            r.gross_total_capacity,
            lk.gross_total_capacity[0],
            epsilon = 1e-6
        );
        // Sensible clamped to total when raw sensible > total (file quirk at RH=0)
        let expected_sensible = lk.gross_sensible_capacity[0].min(lk.gross_total_capacity[0]);
        assert_relative_eq!(r.gross_sensible_capacity, expected_sensible, epsilon = 1e-6);
        assert_relative_eq!(r.gross_power, lk.gross_power[0], epsilon = 1e-6);
    }

    #[test]
    fn edge_clamp_flags_out_of_range() {
        let rs = Rs0004::load(&residential_path()).unwrap();
        let interp = DxCoolingInterpolator::new(&rs.performance.performance_map_cooling).unwrap();
        let q = DxCoolingQuery {
            outdoor_db_k: 10.0, // Way below grid min (286 K)
            indoor_rh: 0.5,
            indoor_db_k: 297.0,
            indoor_mass_flow_kg_s: 0.5,
            compressor_sequence: 2.0,
            ambient_pressure_pa: 101325.0,
        };
        let r = interp.query(&q).unwrap();
        assert!(!r.in_range);
        // Edge-clamped values are still positive and self-consistent
        assert!(r.gross_total_capacity > 0.0);
        assert!(r.gross_sensible_capacity <= r.gross_total_capacity);
        assert!(r.gross_power > 0.0);
    }
}
