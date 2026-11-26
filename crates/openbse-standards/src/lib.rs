//! Code and standard reference data, and the constructions built from it.
//!
//! Data lives in `data/standards/` (one file per source table, with the
//! printed values and an exact SI conversion) and is compiled into the
//! binary, so a model that names a standard needs no data path at run time.
//! See `data/standards/README.md` for the file format and the Engineering
//! Reference chapter "Code and standard reference data" for the method.

pub mod constructions;
pub mod data;
pub mod library;
pub mod units;

pub use data::{Derivation, Element, LightingMethod};
pub use library::{Citation, Edition, Library};
