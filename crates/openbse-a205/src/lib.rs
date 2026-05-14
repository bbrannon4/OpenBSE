//! ASHRAE Standard 205 equipment performance data support.
//!
//! Standard 205 defines a portable data format (CBOR binary, with JSON/YAML
//! equivalents for development) that manufacturers use to publish equipment
//! performance maps.  This crate provides:
//!
//! - Data structures matching the published representation specifications (RS)
//! - A generic N-dimensional linear interpolator over the performance maps
//! - File loading from CBOR (.a205, production) or JSON (.a205.json, dev/test)
//!
//! Currently supports **RS0001 (Chiller)**.  Additional RS modules (RS0002
//! unitary, RS0003 fan, RS0004 DX, RS0005 motor, RS0006 motor drive, RS0007
//! mechanical drive) can be added alongside without restructuring; the
//! interpolator and load machinery are RS-agnostic.
//!
//! ### Extrapolation policy
//!
//! All performance-map lookups in this crate **clamp to the grid edge** when
//! a query point falls outside the published range.  This is conservative
//! and matches the existing table-curve behavior elsewhere in OpenBSE.
//! Standard 205 itself does not mandate a policy; the `operation_state`
//! lookup variable in each map may flag points as UNSUPPORTED, which callers
//! can use to detect out-of-range operation.  See the RS0001 chiller
//! component for how this is surfaced.
//!
//! Reference: ASHRAE Standard 205-2023 (https://data.ashrae.org/standard205/)

pub mod interpolate;
pub mod rs0001;
pub mod rs0002;
pub mod rs0003;
pub mod rs0004;
pub mod rs0005;
pub mod rs0006;
pub mod rs0007;

use std::path::Path;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum A205Error {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON parse error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("CBOR parse error: {0}")]
    Cbor(String),
    #[error("Unsupported Standard 205 schema: expected {expected}, found {found}")]
    SchemaMismatch { expected: String, found: String },
    #[error("{0}")]
    Other(String),
}

/// Detect whether a file is JSON (text-based) or CBOR (binary) by examining
/// its first byte.  JSON files start with whitespace, `{`, or `[`; CBOR map
/// types start with major-type 5 (0xa0-0xbf) per RFC 8949.
pub fn detect_format(path: &Path) -> Result<FileFormat, A205Error> {
    let bytes = std::fs::read(path)?;
    if bytes.is_empty() {
        return Err(A205Error::Other(format!("empty file: {}", path.display())));
    }
    // Skip leading whitespace for text detection
    let first_nonws = bytes
        .iter()
        .find(|&&b| !b.is_ascii_whitespace())
        .copied()
        .unwrap_or(0);
    if first_nonws == b'{' || first_nonws == b'[' {
        Ok(FileFormat::Json)
    } else {
        Ok(FileFormat::Cbor)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileFormat {
    Json,
    Cbor,
}

/// Peek at a Standard 205 file's `metadata.schema` field without fully
/// deserializing the payload.  Used by callers that need to dispatch on
/// the schema ID (e.g. accepting either RS0002 or RS0004 in the same
/// input slot).  Returns the schema string (e.g. `"RS0004"`).
pub fn peek_schema(path: &std::path::Path) -> Result<String, A205Error> {
    let fmt = detect_format(path)?;
    let bytes = std::fs::read(path)?;
    let probe: serde_json::Value = match fmt {
        FileFormat::Json => serde_json::from_slice(&bytes)?,
        FileFormat::Cbor => ciborium::from_reader(bytes.as_slice())
            .map_err(|e| A205Error::Cbor(format!("{}", e)))?,
    };
    Ok(probe
        .get("metadata")
        .and_then(|m| m.get("schema"))
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .to_string())
}
