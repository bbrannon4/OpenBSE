//! ASHRAE Standard 205 RS0002 — Unitary Cooling Air-Conditioning Equipment.
//!
//! Composition wrapper: an RS0002 file declares a complete packaged
//! unitary system, embedding a full RS0004 (DX refrigerant system) as
//! `dx_system_representation`, plus system-level metadata
//! (`standby_power`, `fan_position`).  The fan itself is supplied
//! separately (as a sibling `assembly_component` of the air loop), since
//! the example RS0002 file doesn't include a fan representation.
//!
//! In OpenBSE, support for `a205_file:` on a cooling coil accepts either
//! a raw RS0004 file or an RS0002 wrapper; in the latter case we extract
//! the inner RS0004 and proceed identically.

use crate::rs0004::Rs0004;
use crate::{detect_format, A205Error, FileFormat};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Rs0002 {
    pub metadata: super::rs0001::Metadata,
    #[serde(default)]
    pub description: Option<serde_json::Value>,
    pub performance: Performance,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Performance {
    #[serde(default)]
    pub standby_power: f64,
    /// `BLOW_THROUGH` (fan upstream of coil) or `DRAW_THROUGH` (fan
    /// downstream).  Informational in OpenBSE — fan placement is
    /// determined by the air loop's equipment order.
    pub fan_position: String,
    pub dx_system_representation: Rs0004,
}

impl Rs0002 {
    pub fn load(path: &Path) -> Result<Self, A205Error> {
        let fmt = detect_format(path)?;
        let bytes = std::fs::read(path)?;
        let obj: Self = match fmt {
            FileFormat::Json => serde_json::from_slice(&bytes)?,
            FileFormat::Cbor => ciborium::from_reader(bytes.as_slice())
                .map_err(|e| A205Error::Cbor(format!("{}", e)))?,
        };
        if obj.metadata.schema != "RS0002" {
            return Err(A205Error::SchemaMismatch {
                expected: "RS0002".into(),
                found: obj.metadata.schema.clone(),
            });
        }
        Ok(obj)
    }

    /// Convenience: consume the wrapper and yield the contained RS0004
    /// DX system representation.
    pub fn into_dx_system(self) -> Rs0004 {
        self.performance.dx_system_representation
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn example_path() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("examples")
            .join("Unitary-Constant-Efficiency.RS0002.a205.json")
    }

    #[test]
    fn loads_unitary_and_extracts_dx() {
        let rs = Rs0002::load(&example_path()).unwrap();
        assert_eq!(rs.metadata.schema, "RS0002");
        assert_eq!(rs.performance.fan_position, "BLOW_THROUGH");
        let dx = rs.into_dx_system();
        assert_eq!(dx.metadata.schema, "RS0004");
    }
}
