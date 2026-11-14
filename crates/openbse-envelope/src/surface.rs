//! Surface geometry and boundary condition definitions.

use std::collections::HashMap;

use crate::geometry::{self, Point3D, Vec3};
use serde::{Deserialize, Serialize};

/// Surface type classification.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceType {
    Wall,
    Floor,
    Roof,
    Ceiling,
    Window,
}

/// Boundary condition for a surface.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum BoundaryCondition {
    /// Exposed to outdoor air
    #[default]
    Outdoor,
    /// In contact with ground
    Ground,
    /// Perfectly insulated (no heat flow)
    Adiabatic,
    /// Adjacent to another zone (interzone surface)
    Zone(String),
}

/// A building surface (wall, floor, roof, window).
///
/// Supports two geometry modes (backward compatible):
/// 1. **Explicit** (existing): provide `area`, `azimuth`, `tilt` directly
/// 2. **Vertex-based** (new): provide `vertices` and call `resolve_geometry()`
///    to auto-calculate area, azimuth, and tilt from the polygon.
///
/// If both vertices and explicit values are provided, `resolve_geometry()`
/// will overwrite the explicit values with computed ones.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SurfaceInput {
    pub name: String,
    /// Zone this surface belongs to
    pub zone: String,
    /// Surface type
    #[serde(rename = "type")]
    pub surface_type: SurfaceType,
    /// Construction name (opaque or window)
    pub construction: String,
    /// Gross area [m²] (auto-calculated if vertices are provided)
    #[serde(default)]
    pub area: f64,
    /// Azimuth angle [degrees from north, clockwise] (auto-calculated if vertices are provided)
    #[serde(default)]
    pub azimuth: f64,
    /// Tilt angle [degrees from horizontal: 0=face-up, 90=vertical, 180=face-down]
    #[serde(default = "default_tilt")]
    pub tilt: f64,
    /// Boundary condition
    #[serde(default)]
    pub boundary: BoundaryCondition,
    /// Name of the opposite face of an interzone surface (E+ `Outside Boundary
    /// Condition Object`). Optional: surfaces are otherwise paired by coincident
    /// vertices, then by matching area and orientation. Needed only when that
    /// can't tell two surfaces apart (see [`pair_interzone_surfaces`]).
    #[serde(default)]
    pub adjacent_surface: Option<String>,
    /// Parent surface name (for windows)
    #[serde(default)]
    pub parent_surface: Option<String>,
    /// 3D vertex coordinates (CCW from outside). If provided (≥3 vertices),
    /// area, azimuth, and tilt are auto-calculated by `resolve_geometry()`.
    #[serde(default)]
    pub vertices: Option<Vec<Point3D>>,
    /// Window shading devices (overhang, fins). Only applicable to windows.
    /// The engine auto-generates shading polygon vertices from these
    /// simplified parameters relative to the parent window geometry.
    #[serde(default)]
    pub shading: Option<crate::shading::WindowShadingInput>,
    /// Whether exterior surface receives solar radiation (default: true).
    /// Set to false for slab-on-grade floors per ASHRAE 140.
    /// Matches EnergyPlus `NoSun` surface property.
    #[serde(default = "default_true")]
    pub sun_exposure: bool,
    /// Whether exterior surface is exposed to wind-driven convection (default: true).
    /// Set to false for slab-on-grade floors per ASHRAE 140.
    /// When false, only natural convection is used.
    /// Matches EnergyPlus `NoWind` surface property.
    #[serde(default = "default_true")]
    pub wind_exposure: bool,
    /// Exposed perimeter [m] for F-factor ground floor constructions.
    ///
    /// Required when the surface uses an `f_factor_constructions` construction.
    /// The effective U-factor is computed as: U_eff = F × P / A.
    /// Matches EnergyPlus `Construction:FfactorGroundFloor` PerimeterExposed.
    #[serde(default)]
    pub exposed_perimeter: Option<f64>,
    /// Height of the surface along its slope [m]: the window height for the
    /// ISO 15099 interior convection model, and (times the sine of the tilt)
    /// a large opening's vertical extent in the airflow network. If not
    /// provided, taken from the vertices, or sqrt(area) without them.
    #[serde(default)]
    pub height: Option<f64>,
    /// Airflow network overrides for this surface (crack coefficient, Cp, etc.).
    /// Only used when the airflow network is enabled in simulation settings.
    #[serde(default)]
    pub airflow: Option<crate::airflow_network::SurfaceAirflowOverride>,
}

fn default_true() -> bool {
    true
}

fn default_tilt() -> f64 {
    90.0
}

impl SurfaceInput {
    /// Height of a window along its slope [m]: the input value, else the
    /// vertices' vertical extent over the sine of the tilt. A flat window has no
    /// up, so it and a window without vertices take sqrt(area).
    pub fn window_height(&self) -> f64 {
        let square = || self.area.max(0.0).sqrt();
        if let Some(h) = self.height {
            return h;
        }
        let sin_tilt = self.tilt.to_radians().sin();
        match &self.vertices {
            Some(v) if v.len() >= 3 && sin_tilt > 0.05 => {
                let (lo, hi) = v
                    .iter()
                    .fold((f64::MAX, f64::MIN), |(l, h), p| (l.min(p.z), h.max(p.z)));
                (hi - lo) / sin_tilt
            }
            _ => square(),
        }
    }

    /// Vertical extent of the surface [m]: the input `height` times the sine
    /// of the tilt, else the vertices' z-range, else a square of the
    /// surface's area tilted to its slope (sqrt(area) × sin(tilt)).
    pub fn vertical_extent(&self) -> f64 {
        let sin_tilt = self.tilt.to_radians().sin().abs();
        if let Some(h) = self.height {
            return h * sin_tilt;
        }
        match &self.vertices {
            Some(v) if v.len() >= 3 => {
                let (lo, hi) = v
                    .iter()
                    .fold((f64::MAX, f64::MIN), |(l, h), p| (l.min(p.z), h.max(p.z)));
                hi - lo
            }
            _ => self.area.max(0.0).sqrt() * sin_tilt,
        }
    }

    /// Resolve geometry from vertices if present.
    ///
    /// If `vertices` contains ≥3 points, computes area, azimuth, and tilt
    /// from the polygon using Newell's method. Otherwise leaves explicit
    /// values unchanged (backward compatible).
    pub fn resolve_geometry(&mut self) {
        if let Some(ref verts) = self.vertices {
            if verts.len() >= 3 {
                let normal = geometry::newell_normal(verts);
                self.area = normal.magnitude() / 2.0;
                self.azimuth = geometry::azimuth_from_normal(&normal);
                self.tilt = geometry::tilt_from_normal(&normal);
            }
        }
    }
}

/// Two faces of an interzone pair may differ in area by this fraction. Above
/// it the heat one zone loses can't equal what the other gains. Same limit as
/// E+'s "InterZone Surface Areas do not match" (`SurfaceGeometry.cc`).
const INTERZONE_AREA_TOL: f64 = 0.02;
/// Vertices of the two faces coincide within this distance [m].
const INTERZONE_VERTEX_TOL: f64 = 0.01;
/// Faces of a pair point opposite ways within this angle [degrees] when
/// their geometry has to be matched by area and orientation.
const INTERZONE_ANGLE_TOL: f64 = 5.0;

/// One-to-one pairing of interzone surfaces, with what stopped any surface
/// from pairing.
#[derive(Debug, Clone, Default)]
pub struct InterzonePairing {
    /// `pairs[i] = Some(j)`: surfaces i and j are the two faces of one element.
    /// `None` for non-interzone and self-referencing surfaces.
    pub pairs: Vec<Option<usize>>,
    /// Surfaces that can't be paired, and pairs that can't conserve energy.
    pub errors: Vec<String>,
}

/// Gross area [m²]: from the vertices when given (the input `area` is then
/// overwritten by [`SurfaceInput::resolve_geometry`]), else the input value.
fn surface_gross_area(s: &SurfaceInput) -> f64 {
    match &s.vertices {
        Some(v) if v.len() >= 3 => geometry::newell_normal(v).magnitude() / 2.0,
        _ => s.area,
    }
}

/// Azimuth and tilt [degrees], from the vertices when given.
fn surface_orientation(s: &SurfaceInput) -> (f64, f64) {
    match &s.vertices {
        Some(v) if v.len() >= 3 => {
            let n = geometry::newell_normal(v);
            (
                geometry::azimuth_from_normal(&n),
                geometry::tilt_from_normal(&n),
            )
        }
        _ => (s.azimuth, s.tilt),
    }
}

/// Angle [degrees] by which two surfaces fail to face opposite ways. Azimuth
/// counts only for non-horizontal surfaces, where it means something.
fn opposite_facing_error(a: &SurfaceInput, b: &SurfaceInput) -> f64 {
    let (az_a, tilt_a) = surface_orientation(a);
    let (az_b, tilt_b) = surface_orientation(b);
    let tilt_err = ((tilt_a + tilt_b) - 180.0).abs();
    let flat = |t: f64| t < INTERZONE_ANGLE_TOL || t > 180.0 - INTERZONE_ANGLE_TOL;
    if flat(tilt_a) || flat(tilt_b) {
        return tilt_err;
    }
    let d = (az_a - az_b).rem_euclid(360.0);
    tilt_err.max((d - 180.0).abs())
}

/// True when both surfaces have the same vertices, in any order.
fn vertices_coincide(a: &SurfaceInput, b: &SurfaceInput) -> bool {
    let (Some(va), Some(vb)) = (&a.vertices, &b.vertices) else {
        return false;
    };
    va.len() == vb.len()
        && va.iter().all(|p| {
            vb.iter().any(|q| {
                let (dx, dy, dz) = (p.x - q.x, p.y - q.y, p.z - q.z);
                (dx * dx + dy * dy + dz * dz).sqrt() < INTERZONE_VERTEX_TOL
            })
        })
}

/// Pairs interzone surfaces one-to-one, so each face's outside temperature is
/// the inside temperature of the same physical element.
///
/// E+ names the partner in every `BuildingSurface:Detailed` (`Outside Boundary
/// Condition Object`), or generates the reversed surface itself; it rejects a
/// surface whose partner doesn't point back (`SurfaceGeometry.cc`). Here the
/// partner comes, in order, from:
/// 1. an explicit `adjacent_surface`;
/// 2. coincident vertices;
/// 3. equal area and opposite orientation, for surfaces without vertices.
///
/// Whatever can't be paired is an error, never a nearest guess: a surface with
/// no partner would exchange heat with a zone that doesn't receive it. A
/// surface facing its own zone stays unpaired, and its two faces see each other
/// (E+ self-referencing surface), unless it names a partner.
pub fn pair_interzone_surfaces(surfaces: &[SurfaceInput]) -> InterzonePairing {
    let n = surfaces.len();
    let mut pairs: Vec<Option<usize>> = vec![None; n];
    let mut errors = Vec::new();
    let facing = |s: &SurfaceInput| match &s.boundary {
        BoundaryCondition::Zone(z) => Some(z.clone()),
        _ => None,
    };

    for i in 0..n {
        let Some(name) = &surfaces[i].adjacent_surface else {
            continue;
        };
        let Some(other_zone) = facing(&surfaces[i]) else {
            errors.push(format!(
                "Surface '{}' names adjacent_surface '{name}' but its boundary is not a zone",
                surfaces[i].name
            ));
            continue;
        };
        let Some(j) = surfaces.iter().position(|s| s.name == *name) else {
            errors.push(format!(
                "Surface '{}': adjacent_surface '{name}' does not exist",
                surfaces[i].name
            ));
            continue;
        };
        let back_ok = surfaces[j]
            .adjacent_surface
            .as_ref()
            .is_none_or(|back| *back == surfaces[i].name);
        if j == i
            || surfaces[j].zone != other_zone
            || facing(&surfaces[j]).as_ref() != Some(&surfaces[i].zone)
            || !back_ok
        {
            errors.push(format!(
                "Surface '{}' (zone '{}', facing '{other_zone}'): adjacent_surface '{name}' \
                 is in zone '{}' and doesn't point back",
                surfaces[i].name, surfaces[i].zone, surfaces[j].zone
            ));
            continue;
        }
        if pairs[j].is_some_and(|k| k != i) {
            errors.push(format!(
                "Surface '{}' is named as adjacent_surface by more than one surface",
                surfaces[j].name
            ));
            continue;
        }
        pairs[i] = Some(j);
        pairs[j] = Some(i);
    }

    let unmatched = |pairs: &[Option<usize>], i: usize, j: usize| {
        i != j
            && pairs[j].is_none()
            && surfaces[j].adjacent_surface.is_none()
            && surfaces[j].zone == facing(&surfaces[i]).unwrap_or_default()
            && facing(&surfaces[j]).as_ref() == Some(&surfaces[i].zone)
    };
    let needs_partner = |pairs: &[Option<usize>], i: usize| {
        pairs[i].is_none()
            && surfaces[i].adjacent_surface.is_none()
            && facing(&surfaces[i]).is_some_and(|z| z != surfaces[i].zone)
    };

    for i in 0..n {
        if !needs_partner(&pairs, i) {
            continue;
        }
        if let Some(j) = (0..n)
            .find(|&j| unmatched(&pairs, i, j) && vertices_coincide(&surfaces[i], &surfaces[j]))
        {
            pairs[i] = Some(j);
            pairs[j] = Some(i);
        }
    }

    // Area and orientation can leave a tie only between surfaces alike in both,
    // so it needs naming only if they differ in construction.
    for i in 0..n {
        if !needs_partner(&pairs, i) {
            continue;
        }
        let area_i = surface_gross_area(&surfaces[i]);
        let candidates: Vec<usize> = (0..n)
            .filter(|&j| {
                unmatched(&pairs, i, j)
                    && (surface_gross_area(&surfaces[j]) - area_i).abs()
                        <= INTERZONE_AREA_TOL * area_i
                    && opposite_facing_error(&surfaces[i], &surfaces[j]) <= INTERZONE_ANGLE_TOL
            })
            .collect();
        let Some(&j) = candidates.first() else {
            errors.push(format!(
                "Interzone surface '{}' (zone '{}', {area_i:.2} m²) has no matching surface in \
                 zone '{}' that points back with the same area and opposite orientation; check \
                 the boundary zones and geometry, or name the partner with adjacent_surface",
                surfaces[i].name,
                surfaces[i].zone,
                facing(&surfaces[i]).unwrap_or_default(),
            ));
            continue;
        };
        if candidates
            .iter()
            .any(|&k| surfaces[k].construction != surfaces[j].construction)
        {
            errors.push(format!(
                "Interzone surface '{}' (zone '{}') has more than one possible partner with \
                 different constructions; name it with adjacent_surface",
                surfaces[i].name, surfaces[i].zone
            ));
            continue;
        }
        pairs[i] = Some(j);
        pairs[j] = Some(i);
    }

    for i in 0..n {
        let Some(j) = pairs[i] else { continue };
        if i < j {
            let (a, b) = (
                surface_gross_area(&surfaces[i]),
                surface_gross_area(&surfaces[j]),
            );
            if (a - b).abs() > INTERZONE_AREA_TOL * a.max(b) {
                errors.push(format!(
                    "Interzone surfaces '{}' ({a:.2} m²) and '{}' ({b:.2} m²) are paired but \
                     their areas differ, so the heat one zone loses can't equal what the other \
                     gains",
                    surfaces[i].name, surfaces[j].name
                ));
            }
        }
    }
    InterzonePairing { pairs, errors }
}

/// Why [`orient_surfaces`] reversed a surface's vertex order.
#[derive(Debug, Clone, PartialEq)]
pub enum WindingFix {
    /// Every other vertex of the zone lay on the side the normal pointed to,
    /// so the normal pointed into the zone.
    FacedIntoZone,
    /// A window's normal opposed its parent surface's (named here).
    OpposedParent(String),
}

impl WindingFix {
    /// Warning text for a surface this fix was applied to.
    pub fn message(&self, surface: &str) -> String {
        match self {
            WindingFix::FacedIntoZone => format!(
                "Surface '{surface}': vertices are wound so it faces into its zone; \
                 the engine reverses them. List vertices counter-clockwise as seen from outside."
            ),
            WindingFix::OpposedParent(parent) => format!(
                "Window '{surface}': vertices face the opposite way to parent surface \
                 '{parent}'; the engine reverses them. List vertices counter-clockwise as \
                 seen from outside."
            ),
        }
    }
}

/// Distance [m] within which a zone vertex counts as lying in a surface's
/// plane. The repo models carry four decimals, so 1 mm clears rounding while
/// staying far below any real room dimension.
const PLANE_TOL: f64 = 1e-3;

/// Reverses the vertices of surfaces wound into their zone, so that
/// `resolve_geometry` derives the right azimuth and tilt.
///
/// A surface wound the wrong way faces the opposite direction for solar, sky
/// exchange, convection and the interior box view factors. This test trusts no
/// vertex order. When every vertex of a zone lies on one side of a surface's
/// plane, the zone is on that side whatever the winding says. Surfaces with
/// zone vertices on both sides (re-entrant walls of non-convex zones) are left
/// alone; a window on one is checked against its parent instead.
///
/// Returns each reversed surface's name and the reason.
pub fn orient_surfaces(surfaces: &mut [SurfaceInput]) -> Vec<(String, WindingFix)> {
    let mut decided = vec![false; surfaces.len()];
    let into_zone: Vec<usize> = {
        let mut zone_points: HashMap<&str, Vec<Point3D>> = HashMap::new();
        for s in surfaces.iter() {
            if s.surface_type == SurfaceType::Window {
                continue;
            }
            if let Some(v) = polygon(s) {
                zone_points
                    .entry(s.zone.as_str())
                    .or_default()
                    .extend_from_slice(v);
            }
        }
        let mut out = Vec::new();
        for (i, s) in surfaces.iter().enumerate() {
            let (Some(v), Some(pts)) = (polygon(s), zone_points.get(s.zone.as_str())) else {
                continue;
            };
            if let Some(zone_in_front) = zone_side(v, pts) {
                decided[i] = true;
                if zone_in_front {
                    out.push(i);
                }
            }
        }
        out
    };

    let mut fixes = Vec::new();
    for i in into_zone {
        if let Some(v) = surfaces[i].vertices.as_mut() {
            reverse_winding(v);
        }
        fixes.push((surfaces[i].name.clone(), WindingFix::FacedIntoZone));
    }

    // Parent normals are read after the fixes above, so a window is matched to
    // its corrected wall.
    let normals: HashMap<String, Vec3> = surfaces
        .iter()
        .filter_map(|s| polygon(s).map(|v| (s.name.clone(), geometry::newell_normal(v))))
        .collect();
    for (i, s) in surfaces.iter_mut().enumerate() {
        if decided[i] || s.surface_type != SurfaceType::Window {
            continue;
        }
        let Some(parent) = s.parent_surface.clone() else {
            continue;
        };
        let (Some(v), Some(parent_normal)) = (polygon(s), normals.get(&parent)) else {
            continue;
        };
        if geometry::newell_normal(v).dot(parent_normal) < 0.0 {
            if let Some(v) = s.vertices.as_mut() {
                reverse_winding(v);
            }
            fixes.push((s.name.clone(), WindingFix::OpposedParent(parent)));
        }
    }
    fixes
}

fn polygon(s: &SurfaceInput) -> Option<&[Point3D]> {
    s.vertices
        .as_deref()
        .filter(|v| v.len() >= 3 && geometry::newell_normal(v).magnitude() > 1e-12)
}

/// `Some(true)` when the zone lies on the side the polygon's normal points
/// to, `Some(false)` when it lies behind, `None` when it is on both sides or
/// entirely in the plane.
fn zone_side(poly: &[Point3D], zone_points: &[Point3D]) -> Option<bool> {
    let n = geometry::newell_normal(poly).normalize();
    let c = geometry::polygon_centroid(poly);
    let (mut front, mut back) = (false, false);
    for p in zone_points {
        let d = (*p - c).dot(&n);
        if d > PLANE_TOL {
            front = true;
        } else if d < -PLANE_TOL {
            back = true;
        }
    }
    match (front, back) {
        (true, false) => Some(true),
        (false, true) => Some(false),
        _ => None,
    }
}

/// Reverses a polygon's winding. A vertical polygon is then rotated to start
/// at its lowest edge, so a window still reads [BL, BR, TR, TL] from outside,
/// which overhang and fin generation rely on.
fn reverse_winding(v: &mut [Point3D]) {
    v.reverse();
    let n = geometry::newell_normal(v);
    if n.z.abs() > 0.1 * n.magnitude() {
        return;
    }
    let len = v.len();
    let edge_height = |i: usize| v[i].z + v[(i + 1) % len].z;
    let lowest = (0..len)
        .min_by(|&a, &b| edge_height(a).total_cmp(&edge_height(b)))
        .unwrap_or(0);
    v.rotate_left(lowest);
}

/// Runtime surface state used during simulation.
#[derive(Debug, Clone)]
pub struct SurfaceState {
    pub input: SurfaceInput,
    pub is_window: bool,
    /// Net area (gross minus child window areas) [m²]
    pub net_area: f64,
    /// Outside surface temperature [°C]
    pub temp_outside: f64,
    /// Inside surface temperature [°C]
    pub temp_inside: f64,
    /// Inside convective heat flux [W/m²] (positive = into zone)
    pub q_conv_inside: f64,
    /// Inside convection coefficient [W/(m²·K)]
    pub h_conv_inside: f64,
    /// Inside net longwave radiation heat flux [W/m²] (positive = into surface)
    /// Computed as h_rad × (T_mrt − T_surface).
    pub q_rad_inside: f64,
    /// Inside linearized radiation coefficient [W/(m²·K)]
    pub h_rad_inside: f64,
    /// Outside convection coefficient [W/(m²·K)]
    pub h_conv_outside: f64,
    /// Incident solar radiation on this surface [W/m²]
    pub incident_solar: f64,
    /// Absorbed solar on outside face [W/m²] (opaque) or absorbed by glass exterior [W]
    pub absorbed_solar_outside: f64,
    /// Transmitted solar through window [W]
    pub transmitted_solar: f64,
    /// Beam component of transmitted solar [W]
    pub transmitted_solar_beam: f64,
    /// Cosine of the beam's angle of incidence on the outside face (0 when
    /// the sun is behind the surface or down). Kept so a window's transmitted
    /// beam power can be turned back into beam irradiance.
    pub cos_incidence: f64,
    /// Diffuse component of transmitted solar [W]
    pub transmitted_solar_diffuse: f64,
    /// Outside solar absorptance
    pub solar_absorptance_outside: f64,
    /// Outside thermal absorptance (emissivity) for exterior LW radiation
    pub thermal_absorptance_outside: f64,
    /// Inside thermal absorptance (emissivity) for interior LW radiation
    pub thermal_absorptance_inside: f64,
    /// Inside solar absorptance
    pub solar_absorptance_inside: f64,
    /// Surface roughness
    pub roughness: crate::material::Roughness,
    /// Construction U-factor [W/(m²·K)] (used for windows and simple conduction)
    pub u_factor: f64,
    /// Window SHGC: the rated value of a simple glazing system, or the
    /// nominal value computed for one described by its panes.
    pub shgc: f64,
    /// Glazing system of a window (`None` for opaque surfaces).
    pub glazing: Option<std::sync::Arc<crate::glazing::Glazing>>,
    /// Simple glazing equivalent of the window+blind assembly rating
    /// (`blind_shgc`, `blind_u_factor`), used while that blind is deployed.
    pub blind_glazing: Option<std::sync::Arc<crate::glazing::Glazing>>,
    /// Face temperatures of the active glazing [°C], outer face first; with
    /// a shade deployed they are followed by the shade and the gap air.
    /// Empty until the first window solve.
    pub window_faces: Vec<f64>,
    /// Solar absorbed by each glass layer [W], from outdoors and from the
    /// room's reflected diffuse.
    pub window_layer_solar: [f64; crate::glazing::MAX_LAYERS],
    /// Cosine of tilt angle
    pub cos_tilt: f64,
    /// Sine of tilt angle
    pub sin_tilt: f64,
    /// Centroid height above ground [m] for wind speed profile adjustment.
    /// Estimated from zone geometry when vertex data is not available.
    pub centroid_height: f64,
    /// Diffuse sky shading ratio [0.0-1.0]: fraction of isotropic sky diffuse
    /// radiation that reaches this surface, accounting for obstruction by
    /// shading surfaces (overhangs, fins, detached shading).
    /// 1.0 = fully exposed to sky dome, < 1.0 = partially obstructed.
    /// Computed once during initialization using hemisphere sampling.
    /// Matches EnergyPlus DifShdgRatioIsoSky.
    pub diffuse_sky_shading_ratio: f64,
    /// Diffuse horizon shading ratio [0.0-1.0]: fraction of horizon-band
    /// diffuse radiation that reaches this surface.
    /// Separate from sky ratio because horizontal overhangs don't block
    /// horizon brightening, but vertical fins do.
    /// Matches EnergyPlus DifShdgRatioHoriz.
    pub diffuse_horizon_shading_ratio: f64,
    /// Position of this surface in its zone's long-wave exchange
    /// (`ZoneRadExchange::surfaces`).
    pub rad_index: Option<usize>,
    /// Monthly ground temperatures [°C] for F-factor ground floors.
    /// When `Some`, these override the building-level ground temperature model
    /// for this surface. Matches EnergyPlus `Site:GroundTemperature:FCfactorMethod`.
    pub f_factor_ground_temps: Option<[f64; 12]>,
    /// Conduction heat flux into zone from inside face of surface [W] (total, not per m²).
    /// Stored from the CTF apply_ctf() result each timestep.
    pub q_cond_inside: f64,
    /// Conduction heat flux from outside into surface [W] (total, not per m²).
    /// Stored from the CTF apply_ctf() result each timestep.
    pub q_cond_outside: f64,

    // ── Dynamic blind state (#104) ────────────────────────────────────────
    /// True when a shading control is configured for this window, a blackout
    /// blind (`blind_shgc` 0) included (#106).
    pub blind_configured: bool,
    /// True when interior blind/shade is currently deployed for this window.
    /// Updated each timestep by the blind control logic before solar gains are computed.
    pub blind_deployed: bool,
    /// Physical interior shade layer (#144), copied from the shading control.
    /// When present and deployed, the window is solved as glass + shade.
    pub shade: Option<crate::material::InteriorShade>,
    /// Inner glass face temperature [°C]. Equal to `temp_inside` unless a
    /// shade layer is down, when `temp_inside` is the shade (the face the
    /// room sees) and this is the glass behind it.
    pub temp_glass_inside: f64,
    /// Linearised conductance from an unshaded window's inner glass face to
    /// the outdoors [W/(m²·K)], from its last solve; 0 for opaque surfaces
    /// and for windows behind a deployed shade.
    pub window_u_out: f64,
    /// Solar absorbed by the deployed shade layer [W].
    pub shade_absorbed_solar: f64,
}

#[cfg(test)]
mod tests {
    #[test]
    fn vertical_extent_from_height_then_vertices_then_area() {
        let mut door = iz("d", "a", "b", 4.0, 180.0, "g");
        assert!(
            (door.vertical_extent() - 2.0).abs() < 1e-12,
            "square of the area"
        );
        door.vertices = Some(vec![
            Point3D {
                x: 0.0,
                y: 0.0,
                z: 0.0,
            },
            Point3D {
                x: 0.9,
                y: 0.0,
                z: 0.0,
            },
            Point3D {
                x: 0.9,
                y: 0.0,
                z: 2.1,
            },
            Point3D {
                x: 0.0,
                y: 0.0,
                z: 2.1,
            },
        ]);
        assert!((door.vertical_extent() - 2.1).abs() < 1e-12);
        door.height = Some(2.0);
        assert!((door.vertical_extent() - 2.0).abs() < 1e-12);
        let mut roof = iz("r", "a", "b", 4.0, 0.0, "g");
        roof.tilt = 0.0;
        assert!(
            roof.vertical_extent().abs() < 1e-12,
            "a flat surface has no extent"
        );
    }

    #[test]
    fn window_height_comes_from_input_then_vertices_then_area() {
        let mut w = iz("w", "a", "b", 6.0, 180.0, "g");
        assert!((w.window_height() - 6.0f64.sqrt()).abs() < 1e-12);
        w.vertices = Some(vec![
            Point3D {
                x: 0.0,
                y: 0.0,
                z: 0.2,
            },
            Point3D {
                x: 3.0,
                y: 0.0,
                z: 0.2,
            },
            Point3D {
                x: 3.0,
                y: 0.0,
                z: 2.2,
            },
            Point3D {
                x: 0.0,
                y: 0.0,
                z: 2.2,
            },
        ]);
        assert!((w.window_height() - 2.0).abs() < 1e-12);
        w.tilt = 0.0;
        assert!((w.window_height() - 6.0f64.sqrt()).abs() < 1e-12);
        w.height = Some(1.5);
        assert_eq!(w.window_height(), 1.5);
    }

    fn iz(
        name: &str,
        zone: &str,
        other: &str,
        area: f64,
        azimuth: f64,
        cons: &str,
    ) -> SurfaceInput {
        SurfaceInput {
            name: name.into(),
            zone: zone.into(),
            surface_type: SurfaceType::Wall,
            construction: cons.into(),
            area,
            azimuth,
            tilt: 90.0,
            boundary: BoundaryCondition::Zone(other.into()),
            adjacent_surface: None,
            parent_surface: None,
            vertices: None,
            shading: None,
            sun_exposure: false,
            wind_exposure: false,
            exposed_perimeter: None,
            height: None,
            airflow: None,
        }
    }

    #[test]
    fn wall_and_door_between_two_zones_pair_with_their_own_partners() {
        // SingleFamily's garage: every surface used to pair with the first
        // match, so each door's outside face saw the other zone's wall.
        let surfaces = vec![
            iz("wall a", "a", "b", 13.84, 180.0, "w"),
            iz("door a", "a", "b", 1.95, 180.0, "d"),
            iz("door b", "b", "a", 1.95, 0.0, "d"),
            iz("wall b", "b", "a", 13.84, 0.0, "w"),
        ];
        let p = pair_interzone_surfaces(&surfaces);
        assert!(p.errors.is_empty(), "{:?}", p.errors);
        assert_eq!(p.pairs, vec![Some(3), Some(2), Some(1), Some(0)]);
    }

    #[test]
    fn coincident_vertices_decide_between_equal_areas() {
        let pts = |v: &[(f64, f64)]| {
            v.iter()
                .flat_map(|&(x, y)| [(x, y, 0.0), (x, y, 3.0)])
                .map(|(x, y, z)| Point3D { x, y, z })
                .collect::<Vec<_>>()
        };
        let mut a1 = iz("a1", "a", "b", 0.0, 0.0, "w");
        let mut a2 = iz("a2", "a", "b", 0.0, 0.0, "w");
        let mut b1 = iz("b1", "b", "a", 0.0, 0.0, "w");
        let mut b2 = iz("b2", "b", "a", 0.0, 0.0, "w");
        a1.vertices = Some(pts(&[(0.0, 0.0), (4.0, 0.0)]));
        a2.vertices = Some(pts(&[(8.0, 0.0), (12.0, 0.0)]));
        b1.vertices = Some(pts(&[(12.0, 0.0), (8.0, 0.0)]));
        b2.vertices = Some(pts(&[(4.0, 0.0), (0.0, 0.0)]));
        let p = pair_interzone_surfaces(&[a1, a2, b1, b2]);
        assert!(p.errors.is_empty(), "{:?}", p.errors);
        assert_eq!(p.pairs, vec![Some(3), Some(2), Some(1), Some(0)]);
    }

    #[test]
    fn surface_without_a_partner_is_an_error() {
        // A wall pointing at the wrong zone (found in LargeOffice) and two
        // walls claiming one partner (found in doe_retail_standalone).
        let p = pair_interzone_surfaces(&[iz("x", "a", "b", 10.0, 0.0, "w")]);
        assert_eq!(p.errors.len(), 1);
        let p = pair_interzone_surfaces(&[
            iz("x1", "a", "b", 10.0, 180.0, "w"),
            iz("x2", "a", "b", 10.0, 180.0, "w"),
            iz("y", "b", "a", 10.0, 0.0, "w"),
        ]);
        assert_eq!(p.errors.len(), 1, "{:?}", p.errors);
        assert_eq!(p.pairs[2], Some(0));
    }

    #[test]
    fn paired_faces_of_different_areas_are_an_error() {
        let mut a = iz("a", "a", "b", 10.0, 0.0, "w");
        a.adjacent_surface = Some("b".into());
        let p = pair_interzone_surfaces(&[a, iz("b", "b", "a", 20.0, 180.0, "w")]);
        assert_eq!(p.errors.len(), 1, "{:?}", p.errors);
    }

    #[test]
    fn explicit_partner_settles_a_tie_between_different_constructions() {
        let surfaces = vec![
            iz("a1", "a", "b", 10.0, 180.0, "w1"),
            iz("a2", "a", "b", 10.0, 180.0, "w2"),
            iz("b1", "b", "a", 10.0, 0.0, "w1"),
            iz("b2", "b", "a", 10.0, 0.0, "w2"),
        ];
        assert!(!pair_interzone_surfaces(&surfaces).errors.is_empty());
        let mut named = surfaces;
        named[0].adjacent_surface = Some("b1".into());
        named[1].adjacent_surface = Some("b2".into());
        let p = pair_interzone_surfaces(&named);
        assert!(p.errors.is_empty(), "{:?}", p.errors);
        assert_eq!(p.pairs, vec![Some(2), Some(3), Some(0), Some(1)]);
    }

    #[test]
    fn self_referencing_surfaces_stay_unpaired() {
        let p = pair_interzone_surfaces(&[
            iz("f", "a", "a", 10.0, 0.0, "w"),
            iz("c", "a", "a", 10.0, 180.0, "w"),
        ]);
        assert!(p.errors.is_empty());
        assert_eq!(p.pairs, vec![None, None]);
    }

    use super::*;

    type V = (f64, f64, f64);

    fn surf(name: &str, zone: &str, ty: &str, verts: &[V], parent: Option<&str>) -> SurfaceInput {
        let vs: Vec<String> = verts
            .iter()
            .map(|(x, y, z)| format!("{{x: {x}, y: {y}, z: {z}}}"))
            .collect();
        let parent = parent
            .map(|p| format!("parent_surface: {p}\n"))
            .unwrap_or_default();
        let yaml = format!(
            "name: {name}\nzone: {zone}\ntype: {ty}\nconstruction: C\n{parent}vertices: [{}]\n",
            vs.join(", ")
        );
        serde_yaml::from_str(&yaml).unwrap()
    }

    /// 6 × 5 × 3 box, wound counter-clockwise from outside, with a window on
    /// the south wall listed [BL, BR, TR, TL].
    fn room() -> Vec<SurfaceInput> {
        vec![
            surf(
                "floor",
                "Z",
                "floor",
                &[(0., 0., 0.), (0., 5., 0.), (6., 5., 0.), (6., 0., 0.)],
                None,
            ),
            surf(
                "ceiling",
                "Z",
                "roof",
                &[(0., 0., 3.), (6., 0., 3.), (6., 5., 3.), (0., 5., 3.)],
                None,
            ),
            surf(
                "south",
                "Z",
                "wall",
                &[(0., 0., 0.), (6., 0., 0.), (6., 0., 3.), (0., 0., 3.)],
                None,
            ),
            surf(
                "north",
                "Z",
                "wall",
                &[(6., 5., 0.), (0., 5., 0.), (0., 5., 3.), (6., 5., 3.)],
                None,
            ),
            surf(
                "east",
                "Z",
                "wall",
                &[(6., 0., 0.), (6., 5., 0.), (6., 5., 3.), (6., 0., 3.)],
                None,
            ),
            surf(
                "west",
                "Z",
                "wall",
                &[(0., 5., 0.), (0., 0., 0.), (0., 0., 3.), (0., 5., 3.)],
                None,
            ),
            surf(
                "window",
                "Z",
                "window",
                &[(1., 0., 1.), (5., 0., 1.), (5., 0., 2.), (1., 0., 2.)],
                Some("south"),
            ),
        ]
    }

    fn reversed(s: &SurfaceInput) -> SurfaceInput {
        let mut s = s.clone();
        s.vertices.as_mut().unwrap().reverse();
        s
    }

    fn resolved(mut s: SurfaceInput) -> SurfaceInput {
        s.resolve_geometry();
        s
    }

    #[test]
    fn correct_room_is_left_alone() {
        let mut surfaces = room();
        assert!(orient_surfaces(&mut surfaces).is_empty());
    }

    #[test]
    fn reversed_floor_and_ceiling_are_fixed() {
        let mut surfaces = room();
        surfaces[0] = reversed(&surfaces[0]);
        surfaces[1] = reversed(&surfaces[1]);
        let fixes = orient_surfaces(&mut surfaces);
        assert_eq!(
            fixes,
            vec![
                ("floor".to_string(), WindingFix::FacedIntoZone),
                ("ceiling".to_string(), WindingFix::FacedIntoZone),
            ]
        );
        assert!((resolved(surfaces[0].clone()).tilt - 180.0).abs() < 1e-9);
        assert!(resolved(surfaces[1].clone()).tilt.abs() < 1e-9);
    }

    #[test]
    fn reversed_window_faces_out_and_starts_bottom_left() {
        let mut surfaces = room();
        surfaces[6] = reversed(&surfaces[6]);
        let fixes = orient_surfaces(&mut surfaces);
        assert_eq!(
            fixes,
            vec![("window".to_string(), WindingFix::FacedIntoZone)]
        );
        let w = resolved(surfaces[6].clone());
        assert!((w.azimuth - 180.0).abs() < 1e-9);
        let v = w.vertices.unwrap();
        // Seen from the south, bottom-left is the west end.
        assert_eq!((v[0].x, v[0].z), (1.0, 1.0));
        assert_eq!((v[1].x, v[1].z), (5.0, 1.0));
        assert_eq!((v[2].x, v[2].z), (5.0, 2.0));
    }

    #[test]
    fn open_zone_is_still_oriented() {
        // A sliver zone with only a floor, a ceiling and one wall, like the
        // large office data-center zones.
        let mut surfaces = vec![reversed(&room()[0]), room()[1].clone(), room()[2].clone()];
        let fixes = orient_surfaces(&mut surfaces);
        assert_eq!(
            fixes,
            vec![("floor".to_string(), WindingFix::FacedIntoZone)]
        );
    }

    /// L-shaped zone: 6 × 6 with the north-east 3 × 3 quadrant cut out.
    fn l_room() -> Vec<SurfaceInput> {
        let h = 3.0;
        let outline = [(0., 0.), (6., 0.), (6., 3.), (3., 3.), (3., 6.), (0., 6.)];
        let mut out = vec![
            surf(
                "floor",
                "L",
                "floor",
                &outline
                    .iter()
                    .rev()
                    .map(|&(x, y)| (x, y, 0.))
                    .collect::<Vec<_>>(),
                None,
            ),
            surf(
                "ceiling",
                "L",
                "roof",
                &outline.iter().map(|&(x, y)| (x, y, h)).collect::<Vec<_>>(),
                None,
            ),
        ];
        // Walls follow the counter-clockwise outline, so each is wound
        // counter-clockwise from outside.
        for i in 0..outline.len() {
            let (a, b) = (outline[i], outline[(i + 1) % outline.len()]);
            out.push(surf(
                &format!("wall{i}"),
                "L",
                "wall",
                &[(a.0, a.1, 0.), (b.0, b.1, 0.), (b.0, b.1, h), (a.0, a.1, h)],
                None,
            ));
        }
        out
    }

    #[test]
    fn re_entrant_walls_are_skipped_but_windows_follow_their_parent() {
        let mut surfaces = l_room();
        assert!(orient_surfaces(&mut surfaces).is_empty());

        // wall3 runs (3,3) → (3,6) and faces east into the notch; zone
        // vertices lie on both sides of its plane, so only the parent check
        // can catch a reversed window on it.
        let mut surfaces = l_room();
        surfaces.push(surf(
            "notch window",
            "L",
            "window",
            &[(3., 5., 1.), (3., 4., 1.), (3., 4., 2.), (3., 5., 2.)],
            Some("wall3"),
        ));
        let fixes = orient_surfaces(&mut surfaces);
        assert_eq!(
            fixes,
            vec![(
                "notch window".to_string(),
                WindingFix::OpposedParent("wall3".to_string())
            )]
        );
        let w = resolved(surfaces.last().unwrap().clone());
        assert!((w.azimuth - 90.0).abs() < 1e-9);
    }

    #[test]
    fn zones_do_not_see_each_other() {
        // Two rooms side by side: the shared wall's plane would split the
        // combined vertex set, so each zone must be tested on its own.
        let mut surfaces = room();
        for s in room() {
            let mut s = s;
            s.zone = "Z2".to_string();
            s.name = format!("{}2", s.name);
            if let Some(p) = s.parent_surface.as_mut() {
                p.push('2');
            }
            for v in s.vertices.as_mut().unwrap() {
                v.x += 6.0;
            }
            surfaces.push(s);
        }
        assert!(orient_surfaces(&mut surfaces).is_empty());
    }
}
