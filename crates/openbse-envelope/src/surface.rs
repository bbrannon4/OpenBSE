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
}

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
}
