//! Data-file formats under `data/standards/`.
//!
//! Each standard edition is a directory `<standard>/<edition>/` holding an
//! `edition.yaml` manifest and one file per source table. Every struct denies
//! unknown keys, so a misspelt key in a data file fails the validation test
//! instead of being dropped.
//!
//! What is generic and what is standard-specific:
//! - generic (in code): the envelope elements a surface can be ([`Element`]),
//!   the two lighting methods, and the criteria a table can give (U, C, F,
//!   SHGC, VT);
//! - per standard (in data): methods, climate zones, envelope categories,
//!   assembly classes, space types, defaults and air films.
//!
//! So a later edition, Title 24 or another code is new data, not new code.

use crate::units::{Quantity, UnitSystem};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// How a table file's values were obtained.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Derivation {
    /// As printed in an edition of the standard in this unit system.
    Printed,
    /// Converted exactly from the I-P edition's printed values. The SI
    /// edition rounds its own values, so these can differ from it in the last
    /// digit; replace them with the SI edition's values when available.
    ConvertedFromIp,
}

/// Envelope element a table row applies to. Shared by every standard: this is
/// what the importer matches surfaces against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Element {
    Roof,
    WallAboveGrade,
    WallBelowGrade,
    /// Floor over outdoor air or an unconditioned space.
    Floor,
    SlabOnGrade,
    OpaqueDoor,
    VerticalFenestration,
    Skylight,
}

impl Element {
    pub fn key(self) -> &'static str {
        match self {
            Element::Roof => "roof",
            Element::WallAboveGrade => "wall_above_grade",
            Element::WallBelowGrade => "wall_below_grade",
            Element::Floor => "floor",
            Element::SlabOnGrade => "slab_on_grade",
            Element::OpaqueDoor => "opaque_door",
            Element::VerticalFenestration => "vertical_fenestration",
            Element::Skylight => "skylight",
        }
    }

    pub fn is_fenestration(self) -> bool {
        matches!(self, Element::VerticalFenestration | Element::Skylight)
    }

    /// The single criterion an opaque element's row must give.
    pub fn opaque_criterion(self) -> Option<OpaqueCriterionKind> {
        match self {
            Element::WallBelowGrade => Some(OpaqueCriterionKind::CFactor),
            Element::SlabOnGrade => Some(OpaqueCriterionKind::FFactor),
            Element::VerticalFenestration | Element::Skylight => None,
            _ => Some(OpaqueCriterionKind::UFactor),
        }
    }
}

/// Which performance criterion an opaque row carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpaqueCriterionKind {
    /// Assembly U-factor including air films.
    UFactor,
    /// C-factor: wall conductance excluding soil and air films.
    CFactor,
    /// F-factor: slab heat loss per length of exposed perimeter.
    FFactor,
}

impl OpaqueCriterionKind {
    pub fn quantity(self) -> Quantity {
        match self {
            OpaqueCriterionKind::UFactor | OpaqueCriterionKind::CFactor => Quantity::UFactor,
            OpaqueCriterionKind::FFactor => Quantity::FFactor,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            OpaqueCriterionKind::UFactor => "U-factor",
            OpaqueCriterionKind::CFactor => "C-factor",
            OpaqueCriterionKind::FFactor => "F-factor",
        }
    }
}

/// How a project applies lighting power.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LightingMethod {
    /// One LPD per space type (90.1 space-by-space; Title 24 area category).
    SpaceBySpace,
    /// One LPD for the whole building type (90.1 building area; Title 24
    /// complete building).
    BuildingArea,
}

impl LightingMethod {
    pub fn table_kind(self) -> TableKind {
        match self {
            LightingMethod::SpaceBySpace => TableKind::LightingSpaceBySpace,
            LightingMethod::BuildingArea => TableKind::LightingBuildingArea,
        }
    }
}

/// What a table file holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TableKind {
    LightingSpaceBySpace,
    LightingBuildingArea,
    Envelope,
}

/// A table cell: a number, or `NR` where the standard sets no requirement.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Criterion {
    Value(f64),
    NoRequirement(NoRequirement),
}

/// The `NR` marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NoRequirement {
    #[serde(rename = "NR")]
    Nr,
}

impl Criterion {
    pub fn value(self) -> Option<f64> {
        match self {
            Criterion::Value(v) => Some(v),
            Criterion::NoRequirement(_) => None,
        }
    }
}

/// A lighting control requirement cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlRequirement {
    /// Required.
    Req,
    /// One of the "additional control" options, group 1.
    Add1,
    /// One of the "additional control" options, group 2.
    Add2,
    /// Not required.
    No,
}

// ─── Edition manifest ───────────────────────────────────────────────────────

/// `edition.yaml`: what the edition defines and the choices OpenBSE makes
/// when applying it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EditionManifest {
    /// Machine key of the standard, e.g. `ashrae_90_1`.
    pub standard: String,
    /// Edition, e.g. `"2025"`.
    pub edition: String,
    /// Short name used in citations, e.g. `ASHRAE 90.1-2025`.
    pub citation_name: String,
    /// Unit system the source tables are printed in.
    pub source_units: UnitSystem,
    /// Compliance paths with tables, e.g. `prescriptive`, `appendix_g`.
    pub methods: Vec<String>,
    /// Every climate zone the edition defines.
    pub climate_zones: Vec<String>,
    /// Envelope space-conditioning categories, e.g. `nonresidential`.
    pub envelope_categories: Vec<String>,
    /// Air-film resistances the assembly U-factors include.
    pub air_films: AirFilms,
    /// Per-method defaults the project can override.
    pub defaults: BTreeMap<String, MethodDefaults>,
    /// Typical assembly (from `typical_assemblies.yaml`) built for each
    /// opaque class, with the films its U-factor assumes.
    pub assemblies: BTreeMap<Element, BTreeMap<String, AssemblyRef>>,
}

/// Which typical assembly an opaque class is built from.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssemblyRef {
    /// Id in `typical_assemblies.yaml`.
    pub assembly: String,
    pub outside_film: FilmKind,
    pub inside_film: FilmKind,
    /// Where the edition states the films for this class.
    pub citation: String,
}

/// An air-film condition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilmKind {
    Exterior,
    Semiexterior,
    InteriorVertical,
    InteriorHeatFlowUp,
    InteriorHeatFlowDown,
}

/// Air-film resistances, in the edition's source units.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AirFilms {
    /// Where the edition gives them.
    pub citation: String,
    /// Exterior film, surfaces exposed to wind.
    pub exterior: f64,
    /// Semiexterior film, surfaces facing a ventilated attic, crawlspace or
    /// garage.
    pub semiexterior: f64,
    /// Interior film, vertical surface (horizontal heat flow).
    pub interior_vertical: f64,
    /// Interior film, horizontal surface, heat flow up (roofs).
    pub interior_heat_flow_up: f64,
    /// Interior film, horizontal surface, heat flow down (floors).
    pub interior_heat_flow_down: f64,
}

impl AirFilms {
    /// Resistance of one film, in the edition's source units.
    pub fn get(&self, kind: FilmKind) -> f64 {
        match kind {
            FilmKind::Exterior => self.exterior,
            FilmKind::Semiexterior => self.semiexterior,
            FilmKind::InteriorVertical => self.interior_vertical,
            FilmKind::InteriorHeatFlowUp => self.interior_heat_flow_up,
            FilmKind::InteriorHeatFlowDown => self.interior_heat_flow_down,
        }
    }
}

/// Defaults for one method.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MethodDefaults {
    /// Category for zones that don't name one.
    pub envelope_category: String,
    /// Assembly class per element.
    pub classes: BTreeMap<Element, String>,
}

// ─── Table files ────────────────────────────────────────────────────────────

/// One source table.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TableFile {
    pub standard: String,
    pub edition: String,
    /// Table number as printed, e.g. `9.6.1` or `G3.4-5`.
    pub table: String,
    /// What the table covers, in our own words.
    pub title: String,
    pub kind: TableKind,
    /// One of the manifest's `methods`.
    pub method: String,
    /// Unit system of this file's values.
    pub units: UnitSystem,
    pub derivation: Derivation,
    /// Envelope tables: the climate zones the table covers.
    #[serde(default)]
    pub climate_zones: Vec<String>,
    #[serde(default)]
    pub lighting: Vec<LightingRow>,
    #[serde(default)]
    pub opaque: Vec<OpaqueRow>,
    #[serde(default)]
    pub fenestration: Vec<FenestrationRow>,
}

/// A lighting table row.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LightingRow {
    /// Stable machine key, kept across editions while the meaning holds.
    pub key: String,
    /// Space or building type, as the table names it.
    pub name: String,
    /// Lighting power density, in the file's units. Absent only where the
    /// table gives the density per unit of space height alone.
    #[serde(default)]
    pub lpd: Option<f64>,
    /// Density added per unit of space height (W/ft² per ft, or W/m² per m),
    /// for tall spaces such as atria: LPD = lpd + lpd_per_height × height.
    #[serde(default)]
    pub lpd_per_height: Option<f64>,
    /// Former keys that resolve to this row (renames between editions).
    #[serde(default)]
    pub aliases: Vec<String>,
    /// Room cavity ratio threshold as printed: a number, `NA`, or a rule.
    #[serde(default)]
    pub rcr: Option<NumberOrText>,
    /// Appendix G occupancy sensor reduction, percent, or `None` as printed.
    #[serde(default)]
    pub occupancy_sensor_reduction_percent: Option<NumberOrText>,
    /// Lighting control requirements, keyed by control.
    #[serde(default)]
    pub controls: BTreeMap<String, ControlRequirement>,
}

/// A cell that is a number or printed text.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum NumberOrText {
    Number(f64),
    Text(String),
}

/// An opaque envelope row.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpaqueRow {
    pub element: Element,
    /// Assembly class key, e.g. `mass`, `steel_framed`.
    pub class: String,
    /// Class as the table names it.
    pub name: String,
    /// Subset of the table's climate zones this row applies to (all if empty).
    #[serde(default)]
    pub climate_zones: Vec<String>,
    /// Criteria per envelope category.
    pub criteria: BTreeMap<String, OpaqueCriteria>,
}

/// Opaque criteria for one category: exactly the one the element needs.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpaqueCriteria {
    #[serde(default)]
    pub u_factor: Option<Criterion>,
    #[serde(default)]
    pub c_factor: Option<Criterion>,
    #[serde(default)]
    pub f_factor: Option<Criterion>,
}

impl OpaqueCriteria {
    pub fn get(&self, kind: OpaqueCriterionKind) -> Option<Criterion> {
        match kind {
            OpaqueCriterionKind::UFactor => self.u_factor,
            OpaqueCriterionKind::CFactor => self.c_factor,
            OpaqueCriterionKind::FFactor => self.f_factor,
        }
    }
}

/// A fenestration row.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FenestrationRow {
    pub element: Element,
    /// Product or framing class key, e.g. `fixed`, `operable`, `all`.
    pub class: String,
    pub name: String,
    /// Subset of the table's climate zones this row applies to (all if empty).
    #[serde(default)]
    pub climate_zones: Vec<String>,
    /// Window-to-wall (or skylight-to-roof) ratio band the row applies to,
    /// in percent as printed: `[low, high]`, with `high` null for an
    /// open-ended band.
    #[serde(default)]
    pub area_ratio_percent: Option<(f64, Option<f64>)>,
    pub criteria: BTreeMap<String, FenestrationCriteria>,
}

/// Fenestration criteria for one category.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FenestrationCriteria {
    /// Whole-product U-factor (includes frame and air films).
    pub u_factor: Criterion,
    pub shgc: Criterion,
    /// Minimum visible transmittance / SHGC ratio.
    #[serde(default)]
    pub vt_shgc_min: Option<Criterion>,
    /// Visible transmittance, where the table gives it directly.
    #[serde(default)]
    pub vt: Option<Criterion>,
}

// ─── Typical assemblies ─────────────────────────────────────────────────────

/// `typical_assemblies.yaml`: layered assemblies OpenBSE builds to meet a
/// target U-factor. These are modelling choices, not standard data, so they
/// sit outside every edition directory and any standard can map to them.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TypicalAssemblies {
    pub materials: Vec<MaterialDef>,
    pub assemblies: Vec<AssemblyDef>,
}

/// A material used by typical assemblies, in SI.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MaterialDef {
    pub id: String,
    /// W/(m·K)
    pub conductivity: f64,
    /// kg/m³
    pub density: f64,
    /// J/(kg·K)
    pub specific_heat: f64,
    #[serde(default)]
    pub solar_absorptance: Option<f64>,
    #[serde(default)]
    pub thermal_absorptance: Option<f64>,
    #[serde(default)]
    pub roughness: Option<openbse_envelope::material::Roughness>,
    /// Where the properties come from.
    pub source: String,
}

/// A typical layered assembly.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssemblyDef {
    pub id: String,
    pub element: Element,
    pub description: String,
    /// Layers outside to inside; exactly one is `insulation`, the layer
    /// whose resistance is solved to meet the target.
    pub layers: Vec<LayerDef>,
}

/// One assembly layer: a material at a fixed thickness, or the solved
/// insulation layer.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum LayerDef {
    Material { material: String, thickness: f64 },
    Insulation(InsulationMarker),
}

/// The `insulation` marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InsulationMarker {
    Insulation,
}
