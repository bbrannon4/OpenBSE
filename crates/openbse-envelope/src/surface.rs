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
    /// Window height [m] for ISO 15099 interior convection model.
    /// If not provided, estimated as sqrt(area) for windows.
    #[serde(default)]
    pub height: Option<f64>,
    /// Window width [m] for ISO 15099 interior convection model.
    /// If not provided, estimated as sqrt(area) for windows.
    #[serde(default)]
    pub width: Option<f64>,
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
    /// Solar absorbed by window glazing that flows into the zone [W]
    /// (fraction of glass absorptance × incident; adds to zone heat balance)
    pub absorbed_solar_inside_window: f64,
    /// Solar absorptance of window glazing (total, both sides combined)
    pub window_solar_absorptance: f64,
    /// Fraction of absorbed window solar that goes inward (into zone)
    pub window_inside_absorbed_fraction: f64,
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
    /// Glass-only conductance [W/(m²·K)] — U-factor with standard films removed.
    /// Used for windows when applying dynamic exterior/interior film coefficients.
    /// Pre-computed as: 1/(1/U_overall - 1/h_e_std - 1/h_i_std)
    pub u_glass: f64,
    /// Window SHGC
    pub shgc: f64,
    /// Ratio τ_sol / SHGC: fraction of total window solar gain (SHGC × I)
    /// that is directly transmitted to interior surfaces. The remainder
    /// (1 − ratio) × SHGC × I is absorbed by the glass and re-emitted
    /// inward. Computed from E+ SimpleGlazingSystem correlations or from
    /// per-pane optical properties when available.
    pub solar_transmittance_ratio: f64,
    /// Glass extinction coefficient × thickness per pane (K·d) for Fresnel
    /// angular SHGC model. Pre-computed from per-pane glass properties or
    /// estimated from SHGC via the E+ SimpleGlazingSystem correlation.
    pub glass_kd: f64,
    /// Inward-flowing fraction of absorbed solar (N_i).
    /// Used in SHGC(θ) = τ(θ) + N_i × α(θ) angular modifier computation.
    pub glass_ni: f64,
    /// Effective refractive index for the Fresnel angular SHGC model.
    /// Derived from per-pane τ and ρ to match both transmittance and reflectance
    /// at normal incidence. Falls back to 1.526 (soda-lime glass) when per-pane
    /// properties are not provided.
    pub glass_n: f64,
    /// E+ SimpleGlazingSystem angular model (LBNL-2804E curves + layer properties).
    /// Present for SGS windows (no per-pane optical data); None for detailed windows.
    pub sgs_model: Option<crate::solar::SgsAngularModel>,
    // ── First-principles window gap thermal model ─────────────────────
    /// Rated glass-only conductance [W/(m²·K)] — NFRC-derived value.
    /// Used as an upper cap for the dynamic gap model to prevent the
    /// window from becoming more conductive than its NFRC rating at
    /// extreme temperatures (radiation through the gap scales as T³).
    /// Equal to u_glass when no gap model is used.
    pub u_glass_rated: f64,
    /// Gap width [m] for dynamic ISO 15099 window thermal model.
    /// When > 0, u_glass is recomputed each timestep from pane + gap
    /// properties instead of using the fixed NFRC film-stripped value.
    pub gap_width: f64,
    /// Pane thickness [m] for gap thermal model.
    pub pane_thickness: f64,
    /// Pane thermal conductivity [W/(m·K)] for gap thermal model.
    pub pane_conductivity: f64,
    /// Glass emissivity for gap inter-pane radiation exchange.
    pub gap_emissivity: f64,
    /// Glass emissivity for interior and exterior LW radiation exchange.
    /// Defaults to 0.84 (uncoated soda-lime glass) when not specified.
    pub glass_emissivity: f64,
    /// Hemispherical diffuse back-transmittance of window glazing system.
    /// Used for interior cavity solar back-out calculation. Represents the
    /// fraction of diffuse interior-reflected solar that transmits back
    /// through the window to the exterior.
    pub diffuse_back_transmittance: f64,
    /// Hemispherical diffuse back-absorptance of window glazing system.
    /// α_back = 1 - τ_back - ρ_back. The outward-flowing portion
    /// (α_back × (1 - N_i)) also exits the zone and must be included
    /// in the cavity back-out correction.
    pub diffuse_back_absorptance: f64,
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
    /// Box face index for interior view factor model.
    /// 0=floor, 1=ceiling, 2=south, 3=north, 4=east, 5=west.
    /// `None` for surfaces in zones without view factor support.
    pub box_face: Option<usize>,
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
    /// True when a shading control is configured for this window. Distinguishes
    /// "no blind" from a legitimately configured blackout blind whose
    /// `blind_shgc` is 0.0 (#106).
    pub blind_configured: bool,
    /// True when interior blind/shade is currently deployed for this window.
    /// Updated each timestep by the blind control logic before solar gains are computed.
    pub blind_deployed: bool,
    /// SHGC of the window+blind assembly when the blind is deployed.
    /// Only meaningful when `blind_configured` is true; 0.0 is a valid value
    /// (fully opaque blackout blind).
    pub blind_shgc: f64,
    /// U-factor [W/(m²·K)] of the window+blind assembly when deployed.
    /// Zero falls back to the base window U-factor.
    pub blind_u_factor: f64,
}

#[cfg(test)]
mod tests {
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
