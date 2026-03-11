//! Surface geometry and boundary condition definitions.

use serde::{Deserialize, Serialize};
use crate::geometry::{self, Point3D};

/// Surface type classification.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
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
#[serde(rename_all = "snake_case")]
pub enum BoundaryCondition {
    /// Exposed to outdoor air
    Outdoor,
    /// In contact with ground
    Ground,
    /// Perfectly insulated (no heat flow)
    Adiabatic,
    /// Adjacent to another zone (interzone surface)
    Zone(String),
}

impl Default for BoundaryCondition {
    fn default() -> Self {
        BoundaryCondition::Outdoor
    }
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
}

fn default_true() -> bool { true }

fn default_tilt() -> f64 { 90.0 }

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
    /// Outside thermal absorptance
    pub thermal_absorptance_outside: f64,
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
}
