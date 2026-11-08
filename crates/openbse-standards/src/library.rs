//! Loading, validating and looking up standards data.
//!
//! The engine reads each table's SI file. The file printed in the edition's
//! own unit system (`source_units`) is the record of what the standard says;
//! it is paired with the SI file row by row so that every value used can be
//! cited with its printed number and unit.

use crate::data::*;
use crate::units::{Quantity, UnitSystem};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

/// One table: the SI file the engine uses and the printed source it came from.
#[derive(Debug, Clone)]
pub struct Table {
    /// Path under `data/standards/` of the SI file.
    pub si_path: String,
    /// Path of the printed source file (same as `si_path` for an SI edition).
    pub source_path: String,
    pub si: TableFile,
    pub source: TableFile,
}

/// A standard edition with its tables.
#[derive(Debug, Clone)]
pub struct Edition {
    pub manifest: EditionManifest,
    pub tables: Vec<Table>,
}

/// All embedded standards data.
#[derive(Debug, Clone)]
pub struct Library {
    pub editions: Vec<Edition>,
    pub typical: TypicalAssemblies,
}

/// Where a value came from, for the run report.
#[derive(Debug, Clone, PartialEq)]
pub struct Citation {
    /// e.g. `ASHRAE 90.1-2025`
    pub standard: String,
    /// e.g. `5.5-5`
    pub table: String,
    /// Row as the table names it, with the category or band where relevant.
    pub row: String,
    /// The value as printed, with its unit.
    pub printed: String,
    /// How the SI value was obtained.
    pub derivation: Derivation,
}

impl std::fmt::Display for Citation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} Table {}, {}", self.standard, self.table, self.row)
    }
}

/// A lighting row found for a space or building type.
#[derive(Debug, Clone)]
pub struct LightingHit<'a> {
    pub table: &'a Table,
    pub si: &'a LightingRow,
    pub source: &'a LightingRow,
}

impl LightingHit<'_> {
    /// LPD [W/m²] for a space of the given height [m].
    pub fn lpd_si(&self, height_m: f64) -> f64 {
        self.si.lpd.unwrap_or(0.0) + self.si.lpd_per_height.unwrap_or(0.0) * height_m
    }

    pub fn needs_height(&self) -> bool {
        self.si.lpd_per_height.is_some()
    }
}

/// A criterion value found for an envelope element.
#[derive(Debug, Clone)]
pub struct EnvelopeHit {
    /// SI value, or `None` where the table says `NR`.
    pub si: Option<f64>,
    pub citation: Citation,
}

/// The fenestration criteria found for an element.
#[derive(Debug, Clone)]
pub struct FenestrationHit {
    pub u_factor: EnvelopeHit,
    pub shgc: EnvelopeHit,
    /// Visible transmittance, where the table gives it.
    pub vt: Option<EnvelopeHit>,
    /// Minimum VT/SHGC ratio, where the table gives it.
    pub vt_shgc_min: Option<EnvelopeHit>,
}

static EMBEDDED: OnceLock<Result<Library, Vec<String>>> = OnceLock::new();

mod embedded {
    include!(concat!(env!("OUT_DIR"), "/embedded.rs"));
}

/// Every embedded data file: (path under `data/standards/`, contents).
pub fn embedded_files() -> &'static [(&'static str, &'static str)] {
    embedded::FILES
}

impl Library {
    /// The data compiled into this build.
    pub fn embedded() -> Result<&'static Library, String> {
        EMBEDDED
            .get_or_init(|| Library::from_files(embedded::FILES))
            .as_ref()
            .map_err(|errs| {
                format!(
                    "embedded standards data is invalid:\n  {}",
                    errs.join("\n  ")
                )
            })
    }

    /// Parse and validate a set of data files. Collects every problem.
    pub fn from_files(files: &[(&str, &str)]) -> Result<Library, Vec<String>> {
        let mut errors = Vec::new();
        let mut typical = None;
        let mut manifests: BTreeMap<String, EditionManifest> = BTreeMap::new();
        let mut tables: BTreeMap<String, Vec<(String, TableFile)>> = BTreeMap::new();
        for (path, text) in files {
            let parts: Vec<&str> = path.split('/').collect();
            match parts.as_slice() {
                ["typical_assemblies.yaml"] => match serde_yaml::from_str(text) {
                    Ok(t) => typical = Some(t),
                    Err(e) => errors.push(format!("{path}: {e}")),
                },
                [std, ed, "edition.yaml"] => match serde_yaml::from_str::<EditionManifest>(text) {
                    Ok(m) => {
                        if m.standard != *std || m.edition != *ed {
                            errors.push(format!(
                                "{path}: names {}/{} but sits in {std}/{ed}",
                                m.standard, m.edition
                            ));
                        }
                        manifests.insert(format!("{std}/{ed}"), m);
                    }
                    Err(e) => errors.push(format!("{path}: {e}")),
                },
                [std, ed, _units, file] if file.starts_with("table_") => {
                    match serde_yaml::from_str::<TableFile>(text) {
                        Ok(t) => tables
                            .entry(format!("{std}/{ed}"))
                            .or_default()
                            .push((path.to_string(), t)),
                        Err(e) => errors.push(format!("{path}: {e}")),
                    }
                }
                [.., f] if f.ends_with(".yaml") => {
                    errors.push(format!("{path}: not a recognised data file location"))
                }
                _ => {}
            }
        }
        let Some(typical) = typical else {
            errors.push("typical_assemblies.yaml is missing".into());
            return Err(errors);
        };
        let mut editions = Vec::new();
        for (key, manifest) in manifests {
            let files = tables.remove(&key).unwrap_or_default();
            match pair_tables(&manifest, files) {
                Ok(t) => editions.push(Edition {
                    manifest,
                    tables: t,
                }),
                Err(mut e) => errors.append(&mut e),
            }
        }
        for key in tables.keys() {
            errors.push(format!("{key}: tables without an edition.yaml"));
        }
        let lib = Library { editions, typical };
        errors.extend(lib.validate());
        if errors.is_empty() {
            Ok(lib)
        } else {
            Err(errors)
        }
    }

    /// Find an edition.
    pub fn edition(&self, standard: &str, edition: &str) -> Result<&Edition, String> {
        self.editions
            .iter()
            .find(|e| e.manifest.standard == standard && e.manifest.edition == edition)
            .ok_or_else(|| {
                let have: Vec<String> = self
                    .editions
                    .iter()
                    .map(|e| format!("{} {}", e.manifest.standard, e.manifest.edition))
                    .collect();
                format!(
                    "no data for standard '{standard}' edition '{edition}' (available: {})",
                    have.join(", ")
                )
            })
    }

    pub fn assembly(&self, id: &str) -> Option<&AssemblyDef> {
        self.typical.assemblies.iter().find(|a| a.id == id)
    }

    pub fn material(&self, id: &str) -> Option<&MaterialDef> {
        self.typical.materials.iter().find(|m| m.id == id)
    }

    /// Semantic checks beyond the file format.
    fn validate(&self) -> Vec<String> {
        let mut errs = Vec::new();
        self.validate_typical(&mut errs);
        for ed in &self.editions {
            validate_edition(self, ed, &mut errs);
        }
        errs
    }

    fn validate_typical(&self, errs: &mut Vec<String>) {
        let mut ids = BTreeSet::new();
        for m in &self.typical.materials {
            if !ids.insert(m.id.as_str()) {
                errs.push(format!(
                    "typical_assemblies.yaml: material '{}' twice",
                    m.id
                ));
            }
            if !(m.conductivity > 0.0 && m.density > 0.0 && m.specific_heat > 0.0) {
                errs.push(format!(
                    "typical_assemblies.yaml: material '{}' needs positive k, ρ and cp",
                    m.id
                ));
            }
        }
        let mut aids = BTreeSet::new();
        for a in &self.typical.assemblies {
            if !aids.insert(a.id.as_str()) {
                errs.push(format!(
                    "typical_assemblies.yaml: assembly '{}' twice",
                    a.id
                ));
            }
            let n_ins = a
                .layers
                .iter()
                .filter(|l| matches!(l, LayerDef::Insulation(_)))
                .count();
            if n_ins != 1 {
                errs.push(format!(
                    "typical_assemblies.yaml: assembly '{}' must have exactly one insulation layer",
                    a.id
                ));
            }
            for l in &a.layers {
                if let LayerDef::Material {
                    material,
                    thickness,
                } = l
                {
                    if self.material(material).is_none() {
                        errs.push(format!(
                            "typical_assemblies.yaml: assembly '{}' uses unknown material '{material}'",
                            a.id
                        ));
                    }
                    if *thickness <= 0.0 {
                        errs.push(format!(
                            "typical_assemblies.yaml: assembly '{}' has a non-positive thickness",
                            a.id
                        ));
                    }
                }
            }
        }
    }
}

/// Pair each SI table with its printed source table.
fn pair_tables(
    manifest: &EditionManifest,
    files: Vec<(String, TableFile)>,
) -> Result<Vec<Table>, Vec<String>> {
    let mut errs = Vec::new();
    let unit_dir = |u: UnitSystem| match u {
        UnitSystem::Ip => "ip",
        UnitSystem::Si => "si",
    };
    let mut by_table: BTreeMap<String, Vec<(String, TableFile)>> = BTreeMap::new();
    for (path, t) in files {
        let dir = path.rsplit('/').nth(1).unwrap_or("");
        if dir != unit_dir(t.units) {
            errs.push(format!("{path}: units {:?} but sits in '{dir}/'", t.units));
        }
        let expected_name = format!("table_{}.yaml", t.table);
        if !path.ends_with(&expected_name) {
            errs.push(format!(
                "{path}: holds table {} so should be named {expected_name}",
                t.table
            ));
        }
        if t.standard != manifest.standard || t.edition != manifest.edition {
            errs.push(format!("{path}: names {} {}", t.standard, t.edition));
        }
        by_table.entry(t.table.clone()).or_default().push((path, t));
    }
    let mut out = Vec::new();
    for (table, mut list) in by_table {
        let si = list.iter().position(|(_, t)| t.units == UnitSystem::Si);
        let Some(si) = si else {
            errs.push(format!(
                "{} {} table {table}: no SI file",
                manifest.standard, manifest.edition
            ));
            continue;
        };
        let (si_path, si) = list.remove(si);
        let (source_path, source) = if manifest.source_units == UnitSystem::Si {
            (si_path.clone(), si.clone())
        } else {
            match list
                .into_iter()
                .find(|(_, t)| t.units == manifest.source_units)
            {
                Some(s) => s,
                None => {
                    errs.push(format!("{si_path}: no printed source file"));
                    continue;
                }
            }
        };
        if source.derivation != Derivation::Printed {
            errs.push(format!(
                "{source_path}: the source file must be `derivation: printed`"
            ));
        }
        out.push(Table {
            si_path,
            source_path,
            si,
            source,
        });
    }
    if errs.is_empty() {
        Ok(out)
    } else {
        Err(errs)
    }
}

fn is_snake_key(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        && s.chars().next().is_some_and(|c| c.is_ascii_lowercase())
}

/// Tolerance on SI vs converted source: exact conversion keeps 7 significant
/// figures; an SI edition rounds its printed values.
fn si_tolerance(d: Derivation) -> f64 {
    match d {
        Derivation::ConvertedFromIp => 1e-6,
        Derivation::Printed => 0.03,
    }
}

fn check_pair(
    errs: &mut Vec<String>,
    t: &Table,
    what: &str,
    q: Quantity,
    si: Option<f64>,
    src: Option<f64>,
    src_units: UnitSystem,
) {
    match (si, src) {
        (None, None) => {}
        (Some(a), Some(b)) => {
            if !a.is_finite() || a < 0.0 {
                errs.push(format!("{}: {what} = {a} is not a valid value", t.si_path));
            }
            let expect = q.to_si(b, src_units);
            let tol = si_tolerance(t.si.derivation) * expect.abs().max(1e-12);
            if (a - expect).abs() > tol {
                errs.push(format!(
                    "{}: {what} = {a} but {} gives {b} → {expect:.7}",
                    t.si_path, t.source_path
                ));
            }
        }
        _ => errs.push(format!(
            "{}: {what} is given in one of the SI and source files but not the other",
            t.si_path
        )),
    }
}

fn crit(c: Option<Criterion>) -> Option<f64> {
    c.and_then(|c| c.value())
}

fn crit_pair_nr(
    errs: &mut Vec<String>,
    t: &Table,
    what: &str,
    a: Option<Criterion>,
    b: Option<Criterion>,
) {
    let nr = |c: Option<Criterion>| matches!(c, Some(Criterion::NoRequirement(_)));
    if nr(a) != nr(b) {
        errs.push(format!(
            "{}: {what} is NR in one of the SI and source files only",
            t.si_path
        ));
    }
}

fn validate_edition(lib: &Library, ed: &Edition, errs: &mut Vec<String>) {
    let m = &ed.manifest;
    let name = format!("{}/{}", m.standard, m.edition);
    let zones: BTreeSet<&str> = m.climate_zones.iter().map(|s| s.as_str()).collect();
    let cats: BTreeSet<&str> = m.envelope_categories.iter().map(|s| s.as_str()).collect();
    for (method, d) in &m.defaults {
        if !m.methods.contains(method) {
            errs.push(format!(
                "{name}/edition.yaml: defaults for unknown method '{method}'"
            ));
        }
        if !cats.contains(d.envelope_category.as_str()) {
            errs.push(format!(
                "{name}/edition.yaml: default category '{}' is not an envelope category",
                d.envelope_category
            ));
        }
    }
    for (el, classes) in &m.assemblies {
        for (class, r) in classes {
            match lib.assembly(&r.assembly) {
                None => errs.push(format!(
                    "{name}/edition.yaml: {}/{class} uses unknown assembly '{}'",
                    el.key(),
                    r.assembly
                )),
                Some(a) if a.element != *el => errs.push(format!(
                    "{name}/edition.yaml: {}/{class} uses assembly '{}' built for {}",
                    el.key(),
                    r.assembly,
                    a.element.key()
                )),
                _ => {}
            }
        }
    }

    for t in &ed.tables {
        let s = &t.si;
        if !m.methods.contains(&s.method) {
            errs.push(format!("{}: unknown method '{}'", t.si_path, s.method));
        }
        if s.kind != t.source.kind
            || s.method != t.source.method
            || s.climate_zones != t.source.climate_zones
        {
            errs.push(format!(
                "{}: kind, method or climate zones differ from {}",
                t.si_path, t.source_path
            ));
        }
        for z in &s.climate_zones {
            if !zones.contains(z.as_str()) {
                errs.push(format!("{}: unknown climate zone '{z}'", t.si_path));
            }
        }
        match s.kind {
            TableKind::LightingSpaceBySpace | TableKind::LightingBuildingArea => {
                validate_lighting_table(t, m.source_units, errs)
            }
            TableKind::Envelope => validate_envelope_table(t, m, &cats, errs),
        }
    }

    // Coverage: each method has both lighting kinds, and each climate zone
    // exactly one envelope table per method.
    for method in &m.methods {
        for kind in [
            TableKind::LightingSpaceBySpace,
            TableKind::LightingBuildingArea,
        ] {
            if !ed
                .tables
                .iter()
                .any(|t| t.si.kind == kind && &t.si.method == method)
            {
                errs.push(format!("{name}: no {kind:?} table for method '{method}'"));
            }
        }
        for z in &zones {
            let n = ed
                .tables
                .iter()
                .filter(|t| {
                    t.si.kind == TableKind::Envelope
                        && &t.si.method == method
                        && t.si.climate_zones.iter().any(|c| c == z)
                })
                .count();
            if n != 1 {
                errs.push(format!(
                    "{name}: climate zone {z} has {n} envelope tables for method '{method}' (need 1)"
                ));
            }
        }
        // Lighting keys unique across the tables of one kind and method.
        for kind in [
            TableKind::LightingSpaceBySpace,
            TableKind::LightingBuildingArea,
        ] {
            let mut seen = BTreeSet::new();
            for t in ed
                .tables
                .iter()
                .filter(|t| t.si.kind == kind && &t.si.method == method)
            {
                for r in &t.si.lighting {
                    for k in std::iter::once(&r.key).chain(&r.aliases) {
                        if !seen.insert(k.clone()) {
                            errs.push(format!("{name}: {method} lighting key '{k}' appears twice"));
                        }
                    }
                }
            }
        }
        // Defaults name classes the tables have.
        if let Some(d) = m.defaults.get(method) {
            for (el, class) in &d.classes {
                let found = ed
                    .tables
                    .iter()
                    .filter(|t| &t.si.method == method)
                    .any(|t| {
                        t.si.opaque
                            .iter()
                            .any(|r| r.element == *el && &r.class == class)
                            || t.si
                                .fenestration
                                .iter()
                                .any(|r| r.element == *el && &r.class == class)
                    });
                if !found {
                    errs.push(format!(
                        "{name}: default {method} class '{class}' for {} is in no table",
                        el.key()
                    ));
                }
            }
        }
    }
}

fn validate_lighting_table(t: &Table, src_units: UnitSystem, errs: &mut Vec<String>) {
    let s = &t.si;
    if s.lighting.is_empty()
        || !s.opaque.is_empty()
        || !s.fenestration.is_empty()
        || !s.climate_zones.is_empty()
    {
        errs.push(format!(
            "{}: a lighting table holds only `lighting` rows",
            t.si_path
        ));
    }
    if s.lighting.len() != t.source.lighting.len() {
        errs.push(format!(
            "{}: row count differs from {}",
            t.si_path, t.source_path
        ));
        return;
    }
    for (a, b) in s.lighting.iter().zip(&t.source.lighting) {
        if !is_snake_key(&a.key) {
            errs.push(format!("{}: key '{}' is not snake_case", t.si_path, a.key));
        }
        if a.key != b.key || a.name != b.name || a.aliases != b.aliases {
            errs.push(format!(
                "{}: row '{}' does not match {}",
                t.si_path, a.key, t.source_path
            ));
            continue;
        }
        if a.lpd.is_none() && a.lpd_per_height.is_none() {
            errs.push(format!("{}: row '{}' has no LPD", t.si_path, a.key));
        }
        let what = format!("row '{}' lpd", a.key);
        check_pair(
            errs,
            t,
            &what,
            Quantity::LightingPowerDensity,
            a.lpd,
            b.lpd,
            src_units,
        );
        let what = format!("row '{}' lpd_per_height", a.key);
        check_pair(
            errs,
            t,
            &what,
            Quantity::LpdPerHeight,
            a.lpd_per_height,
            b.lpd_per_height,
            src_units,
        );
        if a.rcr != b.rcr
            || a.occupancy_sensor_reduction_percent != b.occupancy_sensor_reduction_percent
            || a.controls != b.controls
        {
            errs.push(format!(
                "{}: row '{}' dimensionless fields differ from {}",
                t.si_path, a.key, t.source_path
            ));
        }
        if a.lpd.is_some_and(|v| v > 100.0) {
            errs.push(format!(
                "{}: row '{}' LPD {} W/m² is implausible",
                t.si_path,
                a.key,
                a.lpd.unwrap()
            ));
        }
    }
}

fn validate_envelope_table(
    t: &Table,
    m: &EditionManifest,
    cats: &BTreeSet<&str>,
    errs: &mut Vec<String>,
) {
    let s = &t.si;
    let src_units = m.source_units;
    if s.climate_zones.is_empty() || !s.lighting.is_empty() {
        errs.push(format!(
            "{}: an envelope table needs climate_zones and no lighting rows",
            t.si_path
        ));
    }
    if s.opaque.len() != t.source.opaque.len()
        || s.fenestration.len() != t.source.fenestration.len()
    {
        errs.push(format!(
            "{}: row count differs from {}",
            t.si_path, t.source_path
        ));
        return;
    }
    let row_zones_ok = |zs: &Vec<String>| zs.iter().all(|z| s.climate_zones.contains(z));
    for (a, b) in s.opaque.iter().zip(&t.source.opaque) {
        let id = format!("{}/{}", a.element.key(), a.class);
        if a.element != b.element
            || a.class != b.class
            || a.name != b.name
            || a.climate_zones != b.climate_zones
        {
            errs.push(format!(
                "{}: opaque row {id} does not match {}",
                t.si_path, t.source_path
            ));
            continue;
        }
        if !is_snake_key(&a.class) || !row_zones_ok(&a.climate_zones) {
            errs.push(format!(
                "{}: opaque row {id} has a bad class key or climate zone",
                t.si_path
            ));
        }
        let Some(kind) = a.element.opaque_criterion() else {
            errs.push(format!(
                "{}: {id} is fenestration listed as opaque",
                t.si_path
            ));
            continue;
        };
        if !m
            .assemblies
            .get(&a.element)
            .is_some_and(|c| c.contains_key(&a.class))
            && !matches!(a.element, Element::SlabOnGrade | Element::WallBelowGrade)
        {
            errs.push(format!(
                "{}: {id} has no typical assembly in edition.yaml",
                t.si_path
            ));
        }
        for (cat, c) in &a.criteria {
            if !cats.contains(cat.as_str()) {
                errs.push(format!("{}: {id} unknown category '{cat}'", t.si_path));
            }
            let others = [
                OpaqueCriterionKind::UFactor,
                OpaqueCriterionKind::CFactor,
                OpaqueCriterionKind::FFactor,
            ]
            .into_iter()
            .filter(|k| *k != kind && c.get(*k).is_some())
            .count();
            if c.get(kind).is_none() || others > 0 {
                errs.push(format!(
                    "{}: {id} {cat} must give exactly a {}",
                    t.si_path,
                    kind.label()
                ));
            }
            let src = b.criteria.get(cat);
            let what = format!("{id} {cat} {}", kind.label());
            crit_pair_nr(errs, t, &what, c.get(kind), src.and_then(|s| s.get(kind)));
            check_pair(
                errs,
                t,
                &what,
                kind.quantity(),
                crit(c.get(kind)),
                src.and_then(|s| crit(s.get(kind))),
                src_units,
            );
        }
        if a.criteria.keys().collect::<Vec<_>>() != b.criteria.keys().collect::<Vec<_>>() {
            errs.push(format!(
                "{}: {id} categories differ from {}",
                t.si_path, t.source_path
            ));
        }
    }
    for (a, b) in s.fenestration.iter().zip(&t.source.fenestration) {
        let id = format!("{}/{}", a.element.key(), a.class);
        if a.element != b.element
            || a.class != b.class
            || a.area_ratio_percent != b.area_ratio_percent
            || a.climate_zones != b.climate_zones
        {
            errs.push(format!(
                "{}: fenestration row {id} does not match {}",
                t.si_path, t.source_path
            ));
            continue;
        }
        if !a.element.is_fenestration()
            || !is_snake_key(&a.class)
            || !row_zones_ok(&a.climate_zones)
        {
            errs.push(format!("{}: fenestration row {id} is malformed", t.si_path));
        }
        for (cat, c) in &a.criteria {
            if !cats.contains(cat.as_str()) {
                errs.push(format!("{}: {id} unknown category '{cat}'", t.si_path));
            }
            let Some(src) = b.criteria.get(cat) else {
                errs.push(format!(
                    "{}: {id} {cat} missing from {}",
                    t.si_path, t.source_path
                ));
                continue;
            };
            let what = format!("{id} {cat} U-factor");
            crit_pair_nr(errs, t, &what, Some(c.u_factor), Some(src.u_factor));
            check_pair(
                errs,
                t,
                &what,
                Quantity::UFactor,
                c.u_factor.value(),
                src.u_factor.value(),
                src_units,
            );
            for (label, x, y) in [
                ("SHGC", Some(c.shgc), Some(src.shgc)),
                ("VT", c.vt, src.vt),
                ("VT/SHGC", c.vt_shgc_min, src.vt_shgc_min),
            ] {
                let what = format!("{id} {cat} {label}");
                crit_pair_nr(errs, t, &what, x, y);
                check_pair(
                    errs,
                    t,
                    &what,
                    Quantity::Dimensionless,
                    crit(x),
                    crit(y),
                    src_units,
                );
            }
            for v in [crit(Some(c.shgc)), crit(c.vt)].into_iter().flatten() {
                if !(0.0..=1.0).contains(&v) {
                    errs.push(format!("{}: {id} {cat} SHGC/VT {v} outside 0–1", t.si_path));
                }
            }
        }
    }
}

// ─── Lookups ────────────────────────────────────────────────────────────────

impl Edition {
    /// Normalise a climate zone to one the edition defines: case and spaces
    /// are ignored, and a zone without subzones accepts a letter (`7A` → `7`).
    pub fn climate_zone(&self, cz: &str) -> Result<String, String> {
        let norm = cz.trim().to_ascii_uppercase();
        let zones = &self.manifest.climate_zones;
        if zones.contains(&norm) {
            return Ok(norm);
        }
        let digits: String = norm.chars().take_while(|c| c.is_ascii_digit()).collect();
        if !digits.is_empty() && zones.contains(&digits) {
            return Ok(digits);
        }
        Err(format!(
            "climate zone '{cz}' is not defined by {} (valid: {})",
            self.manifest.citation_name,
            zones.join(", ")
        ))
    }

    fn check_method(&self, method: &str) -> Result<(), String> {
        if self.manifest.methods.iter().any(|m| m == method) {
            Ok(())
        } else {
            Err(format!(
                "{} has no method '{method}' (valid: {})",
                self.manifest.citation_name,
                self.manifest.methods.join(", ")
            ))
        }
    }

    /// Find a lighting row by space or building type key (or alias).
    pub fn lighting(
        &self,
        method: &str,
        lighting: LightingMethod,
        key: &str,
    ) -> Result<LightingHit<'_>, String> {
        self.check_method(method)?;
        let kind = lighting.table_kind();
        let tables = || {
            self.tables
                .iter()
                .filter(move |t| t.si.kind == kind && t.si.method == method)
        };
        for t in tables() {
            for (i, r) in t.si.lighting.iter().enumerate() {
                if r.key == key || r.aliases.iter().any(|a| a == key) {
                    return Ok(LightingHit {
                        table: t,
                        si: r,
                        source: &t.source.lighting[i],
                    });
                }
            }
        }
        let keys: Vec<&str> = tables()
            .flat_map(|t| t.si.lighting.iter().map(|r| r.key.as_str()))
            .collect();
        let close: Vec<&str> = keys
            .iter()
            .copied()
            .filter(|k| {
                let stem = key.split('_').next().unwrap_or(key);
                k.contains(stem)
            })
            .collect();
        let tables: Vec<String> = tables().map(|t| t.si.table.clone()).collect();
        Err(format!(
            "'{key}' is not a {} type in {} Table {}{}",
            match lighting {
                LightingMethod::SpaceBySpace => "space",
                LightingMethod::BuildingArea => "building area",
            },
            self.manifest.citation_name,
            tables.join(" / "),
            if close.is_empty() {
                format!(
                    " ({} keys are listed in data/standards/README.md)",
                    keys.len()
                )
            } else {
                format!("; similar keys: {}", close.join(", "))
            }
        ))
    }

    /// The envelope table for a method and climate zone.
    pub fn envelope_table(&self, method: &str, cz: &str) -> Result<&Table, String> {
        self.check_method(method)?;
        let cz = self.climate_zone(cz)?;
        self.tables
            .iter()
            .find(|t| {
                t.si.kind == TableKind::Envelope
                    && t.si.method == method
                    && t.si.climate_zones.contains(&cz)
            })
            .ok_or_else(|| format!("no {method} envelope table for climate zone {cz}"))
    }

    fn citation(&self, t: &Table, row: String, printed: String) -> Citation {
        Citation {
            standard: self.manifest.citation_name.clone(),
            table: t.si.table.clone(),
            row,
            printed,
            derivation: t.si.derivation,
        }
    }

    fn printed(&self, v: Option<Criterion>, q: Quantity) -> String {
        match v {
            Some(Criterion::Value(x)) => {
                let unit = q.unit(self.manifest.source_units);
                if unit == "-" {
                    format!("{x}")
                } else {
                    format!("{x} {unit}")
                }
            }
            Some(Criterion::NoRequirement(_)) => "NR".into(),
            None => "-".into(),
        }
    }

    /// The opaque criterion for an element, class and category.
    pub fn opaque(
        &self,
        method: &str,
        cz: &str,
        element: Element,
        class: &str,
        category: &str,
    ) -> Result<EnvelopeHit, String> {
        let t = self.envelope_table(method, cz)?;
        let cz = self.climate_zone(cz)?;
        let kind = element
            .opaque_criterion()
            .ok_or_else(|| format!("{} is not an opaque element", element.key()))?;
        let applies = |zs: &Vec<String>| zs.is_empty() || zs.contains(&cz);
        let idx =
            t.si.opaque
                .iter()
                .position(|r| r.element == element && r.class == class && applies(&r.climate_zones))
                .ok_or_else(|| {
                    let have: Vec<&str> =
                        t.si.opaque
                            .iter()
                            .filter(|r| r.element == element)
                            .map(|r| r.class.as_str())
                            .collect();
                    format!(
                        "{} Table {} has no {} class '{class}' (classes: {})",
                        self.manifest.citation_name,
                        t.si.table,
                        element.key(),
                        have.join(", ")
                    )
                })?;
        let (si_row, src_row) = (&t.si.opaque[idx], &t.source.opaque[idx]);
        let si = si_row.criteria.get(category).ok_or_else(|| {
            format!(
                "{} Table {} has no '{category}' column (categories: {})",
                self.manifest.citation_name,
                t.si.table,
                self.manifest.envelope_categories.join(", ")
            )
        })?;
        let src = &src_row.criteria[category];
        Ok(EnvelopeHit {
            si: crit(si.get(kind)),
            citation: self.citation(
                t,
                format!(
                    "{}: {}, {category}, {}",
                    element.key(),
                    src_row.name,
                    kind.label()
                ),
                self.printed(src.get(kind), kind.quantity()),
            ),
        })
    }

    /// The fenestration criteria for an element, class and category. `ratio`
    /// is the window-to-wall (or skylight-to-roof) ratio in percent, used by
    /// tables banded by it.
    pub fn fenestration(
        &self,
        method: &str,
        cz: &str,
        element: Element,
        class: &str,
        category: &str,
        ratio_percent: f64,
    ) -> Result<FenestrationHit, String> {
        let t = self.envelope_table(method, cz)?;
        let cz = self.climate_zone(cz)?;
        let applies = |r: &FenestrationRow| {
            r.element == element
                && r.class == class
                && (r.climate_zones.is_empty() || r.climate_zones.contains(&cz))
        };
        // Bands are printed to one decimal (0–10.0, 10.1–20.0, …): a ratio
        // belongs to the first band whose upper bound it does not exceed.
        let candidates: Vec<usize> = (0..t.si.fenestration.len())
            .filter(|&i| applies(&t.si.fenestration[i]))
            .collect();
        if candidates.is_empty() {
            let have: Vec<&str> =
                t.si.fenestration
                    .iter()
                    .filter(|r| r.element == element)
                    .map(|r| r.class.as_str())
                    .collect();
            return Err(format!(
                "{} Table {} has no {} class '{class}' (classes: {})",
                self.manifest.citation_name,
                t.si.table,
                element.key(),
                have.join(", ")
            ));
        }
        let idx = candidates
            .iter()
            .copied()
            .find(|&i| match t.si.fenestration[i].area_ratio_percent {
                None | Some((_, None)) => true,
                Some((_, Some(hi))) => ratio_percent <= hi + 0.05,
            })
            .ok_or_else(|| {
                format!(
                    "{} Table {}: {} area ratio {ratio_percent:.1}% is above every band the table gives",
                    self.manifest.citation_name,
                    t.si.table,
                    element.key()
                )
            })?;
        let (si_row, src_row) = (&t.si.fenestration[idx], &t.source.fenestration[idx]);
        let si = si_row.criteria.get(category).ok_or_else(|| {
            format!(
                "{} Table {} has no '{category}' column",
                self.manifest.citation_name, t.si.table
            )
        })?;
        let src = &src_row.criteria[category];
        let band = match src_row.area_ratio_percent {
            Some((lo, Some(hi))) => format!(" ({lo}–{hi}%)"),
            Some((lo, None)) => format!(" ({lo}%+)"),
            None => String::new(),
        };
        let row = |label: &str| {
            format!(
                "{}: {}{band}, {category}, {label}",
                element.key(),
                src_row.name
            )
        };
        let hit =
            |label: &str, si: Option<Criterion>, src: Option<Criterion>, q: Quantity| EnvelopeHit {
                si: crit(si),
                citation: self.citation(t, row(label), self.printed(src, q)),
            };
        Ok(FenestrationHit {
            u_factor: hit(
                "U-factor",
                Some(si.u_factor),
                Some(src.u_factor),
                Quantity::UFactor,
            ),
            shgc: hit(
                "SHGC",
                Some(si.shgc),
                Some(src.shgc),
                Quantity::Dimensionless,
            ),
            vt: si
                .vt
                .map(|v| hit("VT", Some(v), src.vt, Quantity::Dimensionless)),
            vt_shgc_min: si
                .vt_shgc_min
                .map(|v| hit("VT/SHGC", Some(v), src.vt_shgc_min, Quantity::Dimensionless)),
        })
    }

    /// Air film resistance [m²·K/W].
    pub fn film_si(&self, kind: FilmKind) -> f64 {
        Quantity::Resistance.to_si(
            self.manifest.air_films.get(kind),
            self.manifest.source_units,
        )
    }

    /// The typical assembly and films for an opaque class.
    pub fn assembly_ref(&self, element: Element, class: &str) -> Option<&AssemblyRef> {
        self.manifest.assemblies.get(&element)?.get(class)
    }

    /// Default assembly class for an element under a method.
    pub fn default_class(&self, method: &str, element: Element) -> Option<&str> {
        self.manifest
            .defaults
            .get(method)?
            .classes
            .get(&element)
            .map(|s| s.as_str())
    }

    /// Default envelope category under a method.
    pub fn default_category(&self, method: &str) -> Option<&str> {
        self.manifest
            .defaults
            .get(method)
            .map(|d| d.envelope_category.as_str())
    }
}
