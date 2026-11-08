//! Fill a model's lighting and envelope from a code or standard.
//!
//! The model names a standard, edition, method and climate zone in its
//! `standard:` block, and each zone gives its `space_type`. When the model is
//! loaded, [`apply_standards`] adds what the model leaves out:
//!
//! - **Lighting:** one `lights` entry per zone, at the table's LPD. A zone that
//!   any explicit `lights` entry already covers is left alone.
//! - **Envelope:** a construction for each surface with no `construction`,
//!   built from the table for its element, class and category. A surface with
//!   an explicit construction is left alone.
//!
//! Values are resolved at load rather than written into the YAML, so changing
//! the climate zone, method or edition is a one-line edit, and the data stays
//! in one place. The run summary lists every value used with its source table
//! and printed value.

use crate::input::{InputError, ModelInput};
use openbse_envelope::{
    BoundaryCondition, FFactorConstruction, LightsInput, SurfaceType, WindowConstruction,
};
use openbse_standards::constructions::build_opaque;
use openbse_standards::data::Element;
use openbse_standards::{Citation, Derivation, Edition, Library, LightingMethod};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap, HashSet};

/// Issue tracking the C-factor ground method for below-grade walls.
const C_FACTOR_ISSUE: &str = "https://github.com/bbrannon4/OpenBSE/issues/401";

/// Slope [degrees from horizontal] below which fenestration is a skylight.
/// 90.1 defines a skylight as fenestration sloped less than 60° from
/// horizontal.
const SKYLIGHT_MAX_TILT_DEG: f64 = 60.0;

/// The model's `standard:` block.
///
/// ```yaml
/// standard:
///   name: ashrae_90_1
///   edition: "2025"
///   method: prescriptive        # or appendix_g
///   climate_zone: 5B            # default: the model's climate_zone
///   lighting:
///     method: space_by_space    # or building_area
///     schedule: Office Lights
///   envelope:
///     category: nonresidential
///     classes: {wall_above_grade: mass}
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StandardSelection {
    /// Standard key, e.g. `ashrae_90_1`.
    pub name: String,
    /// Edition, e.g. `"2025"`.
    pub edition: String,
    /// Compliance method, e.g. `prescriptive` or `appendix_g`.
    pub method: String,
    /// Climate zone; defaults to the model's top-level `climate_zone`.
    #[serde(default)]
    pub climate_zone: Option<String>,
    /// Fill lighting power from the standard (omit to leave lighting alone).
    #[serde(default)]
    pub lighting: Option<LightingSelection>,
    /// Build constructions from the standard (omit to leave the envelope alone).
    #[serde(default)]
    pub envelope: Option<EnvelopeSelection>,
}

/// How to apply the standard's lighting power.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LightingSelection {
    pub method: LightingMethod,
    /// Building area type for the building area method (zones may override).
    #[serde(default)]
    pub building_area_type: Option<String>,
    /// Schedule for the generated lights (always on if omitted).
    #[serde(default)]
    pub schedule: Option<String>,
    /// Radiant fraction of the generated lights.
    #[serde(default = "default_radiant")]
    pub radiant_fraction: f64,
    /// Return-air fraction of the generated lights.
    #[serde(default)]
    pub return_air_fraction: f64,
    /// Submeter for the generated lights.
    #[serde(default = "default_submeter")]
    pub submeter: String,
}

fn default_radiant() -> f64 {
    0.7
}
fn default_submeter() -> String {
    "General".into()
}

/// How to apply the standard's envelope criteria.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvelopeSelection {
    /// Default space-conditioning category (zones may override with
    /// `envelope_category`). Default: the edition's.
    #[serde(default)]
    pub category: Option<String>,
    /// Assembly class per element, overriding the edition's defaults.
    #[serde(default)]
    pub classes: BTreeMap<Element, String>,
    /// SHGC to use where the table sets no SHGC requirement (`NR`). Needed
    /// only when a window falls under such a row.
    #[serde(default)]
    pub shgc_where_no_requirement: Option<f64>,
}

/// One value the standard supplied.
#[derive(Debug, Clone)]
pub struct AppliedValue {
    /// What received it, e.g. `zone 'Office'` or a construction name.
    pub target: String,
    /// What it is, e.g. `LPD` or `U-factor`.
    pub quantity: String,
    /// Value used, in SI, with unit.
    pub value: String,
    pub citation: Citation,
    /// Extra detail (achieved U, solved insulation, assumptions).
    pub note: Option<String>,
}

/// What [`apply_standards`] did, for the run summary.
#[derive(Debug, Clone, Default)]
pub struct StandardsReport {
    /// e.g. `ASHRAE 90.1-2025, prescriptive, climate zone 5B`.
    pub header: String,
    pub applied: Vec<AppliedValue>,
    /// Explicit model inputs that were kept in place of the standard's.
    pub kept_explicit: Vec<String>,
    /// Things left for the model to supply, and why.
    pub notes: Vec<String>,
}

impl StandardsReport {
    /// True if any value used was converted from another unit system.
    pub fn any_converted(&self) -> bool {
        self.applied
            .iter()
            .any(|a| a.citation.derivation == Derivation::ConvertedFromIp)
    }
}

/// Apply the model's `standard:` block. Returns `None` if there is none.
pub fn apply_standards(model: &mut ModelInput) -> Result<Option<StandardsReport>, InputError> {
    let Some(sel) = model.standard.clone() else {
        return Ok(None);
    };
    let lib = Library::embedded().map_err(InputError::ValidationError)?;
    let err = |e: String| InputError::ValidationError(format!("standard: {e}"));
    let ed = lib.edition(&sel.name, &sel.edition).map_err(err)?;
    if !ed.manifest.methods.contains(&sel.method) {
        return Err(err(format!(
            "{} has no method '{}' (valid: {})",
            ed.manifest.citation_name,
            sel.method,
            ed.manifest.methods.join(", ")
        )));
    }
    let cz = match sel.climate_zone.as_ref().or(model.climate_zone.as_ref()) {
        Some(cz) => Some(ed.climate_zone(cz).map_err(err)?),
        None => None,
    };
    let mut report = StandardsReport {
        header: format!(
            "{}, {}{}",
            ed.manifest.citation_name,
            sel.method,
            cz.as_ref()
                .map(|c| format!(", climate zone {c}"))
                .unwrap_or_default()
        ),
        ..Default::default()
    };
    let mut errors = Vec::new();
    if let Some(ls) = &sel.lighting {
        apply_lighting(model, ed, &sel.method, ls, &mut report, &mut errors);
    }
    if let Some(es) = &sel.envelope {
        match &cz {
            Some(cz) => apply_envelope(
                model,
                lib,
                ed,
                &sel.method,
                cz,
                es,
                &mut report,
                &mut errors,
            ),
            None => errors.push(
                "the envelope needs a climate zone: set standard.climate_zone or climate_zone"
                    .into(),
            ),
        }
    }
    if errors.is_empty() {
        Ok(Some(report))
    } else {
        Err(InputError::ValidationError(format!(
            "standard {}:\n  {}",
            report.header,
            errors.join("\n  ")
        )))
    }
}

/// Zones named by any explicit `lights` entry, directly or through a group.
fn zones_with_explicit_lights(model: &ModelInput) -> HashSet<String> {
    let groups: HashMap<&str, &Vec<String>> = model
        .zone_groups
        .iter()
        .map(|g| (g.name.as_str(), &g.zones))
        .collect();
    let mut out = HashSet::new();
    for l in &model.lights {
        for z in &l.zones {
            match groups.get(z.as_str()) {
                Some(members) => out.extend(members.iter().cloned()),
                None => {
                    out.insert(z.clone());
                }
            }
        }
    }
    out
}

/// Zone height [m]: volume / floor area when both are given, otherwise the
/// vertical extent of the zone's surface vertices.
fn zone_height(model: &ModelInput, zone: &str) -> Option<f64> {
    let z = model.zones.iter().find(|z| z.name == zone)?;
    if z.volume > 0.0 && z.floor_area > 0.0 {
        return Some(z.volume / z.floor_area);
    }
    let (lo, hi) = model
        .surfaces
        .iter()
        .filter(|s| s.zone == zone)
        .filter_map(|s| s.vertices.as_ref())
        .flatten()
        .fold((f64::MAX, f64::MIN), |(l, h), p| (l.min(p.z), h.max(p.z)));
    (hi > lo).then_some(hi - lo)
}

fn apply_lighting(
    model: &mut ModelInput,
    ed: &Edition,
    method: &str,
    ls: &LightingSelection,
    report: &mut StandardsReport,
    errors: &mut Vec<String>,
) {
    let explicit = zones_with_explicit_lights(model);
    let mut new_lights = Vec::new();
    for zone in &model.zones {
        if explicit.contains(&zone.name) {
            report
                .kept_explicit
                .push(format!("zone '{}': lights given in the model", zone.name));
            continue;
        }
        let key = match ls.method {
            LightingMethod::SpaceBySpace => zone
                .space_type
                .as_ref()
                .and_then(|s| s.for_method(method))
                .map(str::to_string),
            LightingMethod::BuildingArea => zone
                .building_area_type
                .clone()
                .or_else(|| ls.building_area_type.clone()),
        };
        let Some(key) = key else {
            let what = match ls.method {
                LightingMethod::SpaceBySpace => format!("a space_type for method '{method}'"),
                LightingMethod::BuildingArea => "a building_area_type".into(),
            };
            if zone.conditioned {
                errors.push(format!(
                    "zone '{}' has no {what} and no explicit lights",
                    zone.name
                ));
            } else {
                report.notes.push(format!(
                    "zone '{}' (unconditioned) has no {what}: no lights added",
                    zone.name
                ));
            }
            continue;
        };
        let hit = match ed.lighting(method, ls.method, &key) {
            Ok(h) => h,
            Err(e) => {
                errors.push(format!("zone '{}': {e}", zone.name));
                continue;
            }
        };
        let height = if hit.needs_height() {
            match zone_height(model, &zone.name) {
                Some(h) => h,
                None => {
                    errors.push(format!(
                        "zone '{}': space type '{key}' sets LPD by height, but the zone's height \
                         is unknown (give volume and floor_area, or surface vertices)",
                        zone.name
                    ));
                    continue;
                }
            }
        } else {
            0.0
        };
        let lpd = hit.lpd_si(height);
        let src = hit.source;
        let unit = |q: openbse_standards::units::Quantity| q.unit(ed.manifest.source_units);
        let printed = match (src.lpd, src.lpd_per_height) {
            (Some(a), None) => format!(
                "{a} {}",
                unit(openbse_standards::units::Quantity::LightingPowerDensity)
            ),
            (a, Some(b)) => format!(
                "{} + {b} {}",
                a.unwrap_or(0.0),
                unit(openbse_standards::units::Quantity::LpdPerHeight)
            ),
            (None, None) => "-".into(),
        };
        report.applied.push(AppliedValue {
            target: format!("zone '{}'", zone.name),
            quantity: "LPD".into(),
            value: format!("{lpd:.3} W/m²"),
            citation: Citation {
                standard: ed.manifest.citation_name.clone(),
                table: hit.table.si.table.clone(),
                row: format!("{} ({key})", src.name),
                printed,
                derivation: hit.table.si.derivation,
            },
            note: hit
                .needs_height()
                .then(|| format!("zone height {height:.2} m")),
        });
        new_lights.push(LightsInput {
            name: format!(
                "{} lights ({} Table {})",
                zone.name, ed.manifest.citation_name, hit.table.si.table
            ),
            zones: vec![zone.name.clone()],
            submeter: ls.submeter.clone(),
            power: 0.0,
            watts_per_area: Some(lpd),
            radiant_fraction: ls.radiant_fraction,
            return_air_fraction: ls.return_air_fraction,
            schedule: ls.schedule.clone(),
        });
    }
    model.lights.extend(new_lights);
}

/// Which envelope element a surface is, or why it isn't one.
fn classify(
    s: &openbse_envelope::SurfaceInput,
    parent: Option<&openbse_envelope::SurfaceInput>,
) -> Result<Element, &'static str> {
    use BoundaryCondition as B;
    match (&s.surface_type, &s.boundary) {
        (SurfaceType::Window, _) => {
            let tilt = if s.vertices.is_some() || parent.is_none() {
                s.tilt
            } else {
                parent.map(|p| p.tilt).unwrap_or(s.tilt)
            };
            if tilt < SKYLIGHT_MAX_TILT_DEG {
                Ok(Element::Skylight)
            } else {
                Ok(Element::VerticalFenestration)
            }
        }
        (SurfaceType::Wall, B::Outdoor) => Ok(Element::WallAboveGrade),
        (SurfaceType::Wall, B::Ground) => Ok(Element::WallBelowGrade),
        (SurfaceType::Roof | SurfaceType::Ceiling, B::Outdoor) => Ok(Element::Roof),
        (SurfaceType::Floor, B::Outdoor) => Ok(Element::Floor),
        (SurfaceType::Floor, B::Ground) => Ok(Element::SlabOnGrade),
        (_, B::Zone(_)) => Err("interzone surfaces are not part of the standard's envelope tables"),
        (_, B::Adiabatic) => {
            Err("adiabatic surfaces are not part of the standard's envelope tables")
        }
        _ => Err("this surface type and boundary have no envelope table row"),
    }
}

/// Window-to-wall and skylight-to-roof ratios [%] over exterior surfaces of
/// the zones the envelope is applied to.
fn area_ratios(model: &ModelInput, zones: &HashSet<String>) -> (f64, f64) {
    let resolved: Vec<openbse_envelope::SurfaceInput> = model
        .surfaces
        .iter()
        .filter(|s| zones.contains(&s.zone))
        .map(|s| {
            let mut s = s.clone();
            s.resolve_geometry();
            s
        })
        .collect();
    let by_name: HashMap<&str, &openbse_envelope::SurfaceInput> =
        resolved.iter().map(|s| (s.name.as_str(), s)).collect();
    let (mut wall, mut roof, mut win, mut sky) = (0.0, 0.0, 0.0, 0.0);
    for s in &resolved {
        let parent = s
            .parent_surface
            .as_deref()
            .and_then(|p| by_name.get(p).copied());
        match classify(s, parent) {
            Ok(Element::WallAboveGrade) => wall += s.area,
            Ok(Element::Roof) => roof += s.area,
            Ok(Element::VerticalFenestration) => win += s.area,
            Ok(Element::Skylight) => sky += s.area,
            _ => {}
        }
    }
    let pct = |a: f64, b: f64| if b > 0.0 { 100.0 * a / b } else { 0.0 };
    (pct(win, wall), pct(sky, roof))
}

#[allow(clippy::too_many_arguments)]
fn apply_envelope(
    model: &mut ModelInput,
    lib: &Library,
    ed: &Edition,
    method: &str,
    cz: &str,
    es: &EnvelopeSelection,
    report: &mut StandardsReport,
    errors: &mut Vec<String>,
) {
    let default_cat = es
        .category
        .clone()
        .or_else(|| ed.default_category(method).map(str::to_string))
        .unwrap_or_default();
    let zone_info: HashMap<String, (bool, Option<String>)> = model
        .zones
        .iter()
        .map(|z| (z.name.clone(), (z.conditioned, z.envelope_category.clone())))
        .collect();
    // Zones whose envelope the standard covers: conditioned, or given a
    // category explicitly (e.g. a semiheated zone with no cooling).
    let covered: HashSet<String> = zone_info
        .iter()
        .filter(|(_, (cond, cat))| *cond || cat.is_some())
        .map(|(n, _)| n.clone())
        .collect();
    let (wwr, srr) = area_ratios(model, &covered);

    let existing: HashSet<String> = model
        .constructions
        .iter()
        .map(|c| c.name.clone())
        .chain(model.simple_constructions.iter().map(|c| c.name.clone()))
        .chain(model.f_factor_constructions.iter().map(|c| c.name.clone()))
        .chain(model.window_constructions.iter().map(|c| c.name.clone()))
        .collect();
    let mut built: HashMap<(Element, String, String), String> = HashMap::new();
    let mut assigned: BTreeMap<String, usize> = BTreeMap::new();
    let mut below_grade = Vec::new();

    let parents: HashMap<String, openbse_envelope::SurfaceInput> = model
        .surfaces
        .iter()
        .map(|s| (s.name.clone(), s.clone()))
        .collect();
    for i in 0..model.surfaces.len() {
        let s = &model.surfaces[i];
        if !s.construction.is_empty() {
            continue;
        }
        let Some((_, zone_cat)) = zone_info.get(&s.zone) else {
            continue; // validation reports the unknown zone
        };
        if !covered.contains(&s.zone) {
            report.notes.push(format!(
                "surface '{}': zone '{}' is unconditioned and has no envelope_category, so no \
                 construction was built",
                s.name, s.zone
            ));
            continue;
        }
        let parent = s.parent_surface.as_ref().and_then(|p| parents.get(p));
        let mut sr = s.clone();
        sr.resolve_geometry();
        let element = match classify(&sr, parent) {
            Ok(e) => e,
            Err(why) => {
                report.notes.push(format!(
                    "surface '{}': no construction built ({why})",
                    s.name
                ));
                continue;
            }
        };
        let category = zone_cat.clone().unwrap_or_else(|| default_cat.clone());
        let Some(class) = es
            .classes
            .get(&element)
            .map(|c| c.as_str())
            .or_else(|| ed.default_class(method, element))
            .map(str::to_string)
        else {
            errors.push(format!(
                "no default class for {} under {method}",
                element.key()
            ));
            continue;
        };
        if element == Element::WallBelowGrade {
            below_grade.push(s.name.clone());
            continue;
        }
        if element == Element::SlabOnGrade && s.exposed_perimeter.is_none() {
            errors.push(format!(
                "surface '{}': a slab-on-grade floor uses the F-factor method and needs \
                 exposed_perimeter",
                s.name
            ));
            continue;
        }
        let key = (element, class.clone(), category.clone());
        let name = match built.get(&key) {
            Some(n) => n.clone(),
            None => {
                let name = format!(
                    "{} {method} {cz} {category} {} {class}",
                    ed.manifest.citation_name,
                    element.key()
                );
                if existing.contains(&name) {
                    report.kept_explicit.push(format!(
                        "construction '{name}': defined in the model, used in place of the standard's"
                    ));
                } else {
                    let ratio = if element == Element::Skylight {
                        srr
                    } else {
                        wwr
                    };
                    if let Err(e) = build_construction(
                        model, lib, ed, method, cz, element, &class, &category, &name, ratio, es,
                        report,
                    ) {
                        errors.push(e);
                        continue;
                    }
                }
                built.insert(key, name.clone());
                name
            }
        };
        *assigned.entry(name.clone()).or_default() += 1;
        model.surfaces[i].construction = name;
    }
    for a in report.applied.iter_mut() {
        if let Some(n) = assigned.get(&a.target) {
            a.target = format!(
                "{} ({n} surface{})",
                a.target,
                if *n == 1 { "" } else { "s" }
            );
        }
    }
    if !below_grade.is_empty() {
        errors.push(format!(
            "below-grade walls need a C-factor ground method, which OpenBSE does not have yet \
             ({C_FACTOR_ISSUE}); give these surfaces an explicit construction: {}",
            below_grade.join(", ")
        ));
    }
    if wwr > 0.0 || srr > 0.0 {
        report.notes.push(format!(
            "window-to-wall ratio {wwr:.1}%, skylight-to-roof ratio {srr:.1}% (exterior surfaces \
             of the zones the envelope covers)"
        ));
    }
}

#[allow(clippy::too_many_arguments)]
fn build_construction(
    model: &mut ModelInput,
    lib: &Library,
    ed: &Edition,
    method: &str,
    cz: &str,
    element: Element,
    class: &str,
    category: &str,
    name: &str,
    ratio_percent: f64,
    es: &EnvelopeSelection,
    report: &mut StandardsReport,
) -> Result<(), String> {
    if element.is_fenestration() {
        let hit = ed.fenestration(method, cz, element, class, category, ratio_percent)?;
        let u = hit.u_factor.si.ok_or_else(|| {
            format!(
                "{}: no U-factor requirement (NR); give an explicit window construction",
                hit.u_factor.citation
            )
        })?;
        let (shgc, shgc_note) = match hit.shgc.si {
            Some(v) => (v, None),
            None => match es.shgc_where_no_requirement {
                Some(v) => (
                    v,
                    Some("table gives NR; envelope.shgc_where_no_requirement used".to_string()),
                ),
                None => {
                    return Err(format!(
                        "{} is NR; set standard.envelope.shgc_where_no_requirement or give an \
                         explicit window construction",
                        hit.shgc.citation
                    ))
                }
            },
        };
        model.window_constructions.push(WindowConstruction {
            name: name.to_string(),
            u_factor: u,
            shgc,
            pane_solar_transmittance: None,
            pane_solar_reflectance: None,
            num_panes: None,
            gap_width: None,
            pane_conductivity: None,
            pane_thickness: None,
            glass_emissivity: None,
            shading_control: None,
        });
        report.applied.push(AppliedValue {
            target: name.to_string(),
            quantity: "U-factor".into(),
            value: format!("{u:.4} W/(m²·K)"),
            citation: hit.u_factor.citation,
            note: None,
        });
        report.applied.push(AppliedValue {
            target: name.to_string(),
            quantity: "SHGC".into(),
            value: format!("{shgc:.3}"),
            citation: hit.shgc.citation,
            note: shgc_note,
        });
        // The engine has no daylighting model, so VT is reported, not used.
        for (q, h) in [("VT", hit.vt), ("VT/SHGC min", hit.vt_shgc_min)] {
            if let Some(h) = h {
                report.applied.push(AppliedValue {
                    target: name.to_string(),
                    quantity: q.into(),
                    value: h
                        .si
                        .map(|v| format!("{v:.3}"))
                        .unwrap_or_else(|| "NR".into()),
                    citation: h.citation,
                    note: Some("reported only: no daylighting model uses it".into()),
                });
            }
        }
        return Ok(());
    }

    let hit = ed.opaque(method, cz, element, class, category)?;
    if element == Element::SlabOnGrade {
        let f = hit
            .si
            .ok_or_else(|| format!("{}: NR; give an explicit construction", hit.citation))?;
        model.f_factor_constructions.push(FFactorConstruction {
            name: name.to_string(),
            f_factor: f,
            solar_absorptance: 0.7,
            thermal_absorptance: 0.9,
            ground_temperatures: None,
        });
        report.applied.push(AppliedValue {
            target: name.to_string(),
            quantity: "F-factor".into(),
            value: format!("{f:.4} W/(m·K)"),
            citation: hit.citation,
            note: None,
        });
        return Ok(());
    }

    let aref = ed
        .assembly_ref(element, class)
        .ok_or_else(|| format!("{} class '{class}' has no typical assembly", element.key()))?;
    let assembly = lib
        .assembly(&aref.assembly)
        .ok_or_else(|| format!("typical assembly '{}' is missing", aref.assembly))?;
    let r_films = ed.film_si(aref.outside_film) + ed.film_si(aref.inside_film);
    let b = build_opaque(name, assembly, &lib.typical.materials, hit.si, r_films)?;
    for m in b.materials {
        if !model.materials.iter().any(|x| x.name == m.name) {
            model.materials.push(m);
        }
    }
    model.constructions.push(b.construction);
    let mut note = format!(
        "{} assembly; insulation R {:.3} m²·K/W; U with films {:.4} W/(m²·K) (films per {})",
        aref.assembly, b.insulation_r, b.u_with_films, aref.citation
    );
    if let Some(n) = b.note {
        note.push_str("; ");
        note.push_str(&n);
    }
    report.applied.push(AppliedValue {
        target: name.to_string(),
        quantity: "U-factor".into(),
        value: hit
            .si
            .map(|v| format!("{v:.4} W/(m²·K)"))
            .unwrap_or_else(|| "NR".into()),
        citation: hit.citation,
        note: Some(note),
    });
    Ok(())
}

/// Plain-text lines for the run summary.
pub fn report_lines(r: &StandardsReport) -> Vec<String> {
    let mut out = vec![format!("  {}", r.header)];
    for a in &r.applied {
        out.push(format!("  {} — {}: {}", a.target, a.quantity, a.value));
        // Dimensionless values (SHGC, VT, NR) print without a unit and need
        // no conversion.
        let converted = a.citation.derivation == Derivation::ConvertedFromIp
            && a.citation.printed.contains(' ');
        out.push(format!(
            "      from {} (printed {}{})",
            a.citation,
            a.citation.printed,
            if converted {
                ", converted exactly to SI"
            } else {
                ""
            }
        ));
        if let Some(n) = &a.note {
            out.push(format!("      {n}"));
        }
    }
    for k in &r.kept_explicit {
        out.push(format!("  kept: {k}"));
    }
    for n in &r.notes {
        out.push(format!("  note: {n}"));
    }
    out
}
