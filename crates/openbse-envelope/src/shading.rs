//! Solar shading and shadow calculations.
//!
//! Implements EnergyPlus-derived shadow projection and polygon clipping
//! algorithms to calculate sunlit fractions for building surfaces.
//!
//! Three modes of operation:
//! 1. **Self-shading**: Building surfaces automatically shade each other
//!    (e.g., L-shaped buildings).
//! 2. **Explicit shading surfaces**: User-defined obstructions with 3D vertices
//!    (neighboring buildings, canopies, etc.).
//! 3. **Window overhang/fin**: Simplified syntax that auto-generates shading
//!    geometry from projection depth and extension parameters.
//!
//! Physics: Beam (direct) and circumsolar diffuse components are reduced by
//! sunlit fraction from polygon clipping. Isotropic sky diffuse is reduced
//! by a sky view factor ratio computed via hemisphere sampling, matching
//! EnergyPlus's SkyDifSolarShading() function.
//!
//! Algorithm: Sutherland-Hodgman polygon clipping (1974), adapted from
//! EnergyPlus SolarShading module. Shadow polygons are projected along the
//! sun direction vector onto receiving surface planes, then clipped against
//! the receiving polygon to determine overlap area.
//!
//! Reference: Walton (1983) TARP Reference Manual; Groth & Lokmanhekim (1969).

use serde::{Deserialize, Serialize};
use crate::geometry::{Point3D, Vec3, newell_normal, polygon_area};

// ─── Shading Calculation Mode ────────────────────────────────────────────────

/// Controls how solar shading is calculated.
///
/// Set under `simulation:` in the YAML model file.
///
/// # YAML Example
/// ```yaml
/// simulation:
///   timesteps_per_hour: 4
///   shading_calculation: detailed   # enable geometric shadow calculations
///   start_month: 1
///   end_month: 12
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ShadingCalculation {
    /// No geometric shadow calculations. All outdoor surfaces receive full,
    /// unobstructed solar radiation. This is the default for backward
    /// compatibility and cases without shading geometry.
    #[default]
    Basic,
    /// Full geometric shadow calculations using Sutherland-Hodgman polygon
    /// clipping. Computes sunlit fractions per surface per timestep from:
    /// - Explicit shading surfaces (overhangs, neighboring buildings)
    /// - Window overhang/fin definitions
    /// - Building self-shading (all outdoor surfaces shade each other)
    ///
    /// Only the beam (direct) solar component is reduced; diffuse is unaffected.
    Detailed,
}

// ─── Input Types (YAML) ───────────────────────────────────────────────────────

/// An explicit shading surface defined by 3D vertices.
///
/// Analogous to EnergyPlus `Shading:Site:Detailed` or `Shading:Zone:Detailed`.
/// These surfaces only cast shadows — they have no thermal mass and do not
/// participate in the zone heat balance.
///
/// # YAML Example
/// ```yaml
/// shading_surfaces:
///   - name: Neighboring Building
///     vertices:
///       - {x: 20.0, y: -5.0, z: 0.0}
///       - {x: 20.0, y: 15.0, z: 0.0}
///       - {x: 20.0, y: 15.0, z: 10.0}
///       - {x: 20.0, y: -5.0, z: 10.0}
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShadingSurfaceInput {
    pub name: String,
    /// 3D vertices, counter-clockwise from outside (Newell convention).
    /// Must have ≥ 3 vertices.
    pub vertices: Vec<Point3D>,
    /// Solar transmittance of the shading surface [0-1].
    /// 0.0 = fully opaque (default), 1.0 = fully transparent.
    #[serde(default)]
    pub solar_transmittance: f64,
}

/// Window shading devices: overhang and/or fins.
///
/// Attached to a window via the `shading` field on `SurfaceInput`.
/// The engine auto-generates 3D shading polygon vertices from these
/// simplified parameters relative to the parent window geometry.
///
/// # YAML Example
/// ```yaml
/// surfaces:
///   - name: South Window
///     type: window
///     # ... other fields ...
///     shading:
///       overhang:
///         depth: 1.0
///         offset_above: 0.5
///       left_fin:
///         depth: 0.5
///       right_fin:
///         depth: 0.5
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowShadingInput {
    /// Horizontal overhang above the window.
    #[serde(default)]
    pub overhang: Option<OverhangInput>,
    /// Vertical fin on the left side of the window (when viewed from outside).
    #[serde(default)]
    pub left_fin: Option<FinInput>,
    /// Vertical fin on the right side of the window (when viewed from outside).
    #[serde(default)]
    pub right_fin: Option<FinInput>,
}

/// Horizontal overhang geometry relative to a parent window.
///
/// The overhang is a horizontal rectangle that projects outward from the
/// wall plane, positioned above the window.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OverhangInput {
    /// Projection depth from the wall plane [m].
    pub depth: f64,
    /// Vertical offset above the window top edge [m] (default 0.0 = flush with top).
    #[serde(default)]
    pub offset_above: f64,
    /// How far the overhang extends beyond the left edge of the window [m].
    #[serde(default)]
    pub left_extension: f64,
    /// How far the overhang extends beyond the right edge of the window [m].
    #[serde(default)]
    pub right_extension: f64,
}

/// Vertical fin geometry relative to a parent window.
///
/// A fin is a vertical rectangle that projects outward from the wall plane,
/// positioned at the side of the window.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FinInput {
    /// Projection depth from the wall plane [m].
    pub depth: f64,
    /// How far the fin extends below the window bottom [m] (default 0.0).
    #[serde(default)]
    pub extend_below: f64,
    /// How far the fin extends above the window top [m] (default 0.0).
    #[serde(default)]
    pub extend_above: f64,
}

// ─── Runtime Types ────────────────────────────────────────────────────────────

/// A resolved shading polygon in 3D world coordinates, ready for shadow projection.
#[derive(Debug, Clone)]
pub struct ShadingPolygon {
    pub name: String,
    pub vertices: Vec<Point3D>,
    pub normal: Vec3,
    pub solar_transmittance: f64,
}

/// A 2D point in the local coordinate system of a surface.
#[derive(Debug, Clone, Copy)]
pub struct Point2D {
    pub x: f64,
    pub y: f64,
}

/// Which side of a window a fin is on (viewed from outside).
#[derive(Debug, Clone, Copy)]
pub enum FinSide {
    Left,
    Right,
}

// ─── Core Algorithms ──────────────────────────────────────────────────────────

/// Calculate the sunlit fraction for a receiving surface.
///
/// Projects all casting polygons onto the receiving surface along the sun
/// direction, clips the shadows against the receiving polygon using
/// Sutherland-Hodgman, and computes the fraction that remains sunlit.
///
/// Only the beam (direct) solar component should be multiplied by this fraction.
/// Diffuse radiation is unaffected.
///
/// # Arguments
/// * `recv_verts` — Vertices of the receiving surface (3D, ≥3 points)
/// * `recv_normal` — Outward unit normal of the receiving surface
/// * `casters` — All potential shadow-casting polygons (excluding self)
/// * `sun_dir` — Unit vector pointing from sun toward ground
///
/// # Returns
/// Sunlit fraction in [0.0, 1.0]. 1.0 = fully sunlit, 0.0 = fully shaded.
pub fn calculate_sunlit_fraction(
    recv_verts: &[Point3D],
    recv_normal: &Vec3,
    casters: &[&ShadingPolygon],
    sun_dir: &Vec3,
) -> f64 {
    let recv_area = polygon_area(recv_verts);
    if recv_area < 1e-6 {
        return 1.0;
    }

    // If the sun is behind the receiving surface (beam wouldn't hit it anyway),
    // the surface is effectively fully shaded from beam perspective.
    // sun_dir points FROM sun → if dot(sun_dir, normal) >= 0, sun is behind surface.
    if sun_dir.dot(recv_normal) >= 0.0 {
        return 0.0;
    }

    // Filter out coplanar casters (cannot cast meaningful shadows on the receiver).
    // This prevents parent walls from shadowing their child windows.
    let valid_casters: Vec<&ShadingPolygon> = casters.iter()
        .filter(|casting| {
            let normals_parallel = casting.normal.dot(recv_normal).abs() > 0.99;
            if normals_parallel {
                let avg_plane_dist: f64 = casting.vertices.iter()
                    .map(|v| {
                        let dx = v.x - recv_verts[0].x;
                        let dy = v.y - recv_verts[0].y;
                        let dz = v.z - recv_verts[0].z;
                        let d = Vec3::new(dx, dy, dz);
                        d.dot(recv_normal).abs()
                    })
                    .sum::<f64>() / casting.vertices.len() as f64;
                avg_plane_dist >= 0.01 // Keep only non-coplanar
            } else {
                true // Non-parallel normals → definitely not coplanar
            }
        })
        .copied()
        .collect();

    if valid_casters.is_empty() {
        return 1.0;
    }

    // Use point-sampling approach to correctly handle overlapping shadows
    // from multiple casters (e.g., overhang + fins on the same window).
    // A point is either in shadow or not, regardless of how many casters
    // shade it — this avoids the double-counting bug of area summation.
    //
    // For a single caster, this is equivalent to (but slightly less precise
    // than) the polygon clipping approach. For multiple casters with
    // potential overlap, this gives the correct union of shadows.
    let (origin, u_axis, v_axis) = build_local_coords(recv_verts, recv_normal);
    let recv_2d: Vec<Point2D> = recv_verts.iter()
        .map(|v| to_local_2d(v, &origin, &u_axis, &v_axis))
        .collect();

    // Pre-project all casters onto the receiver plane in 2D
    let mut projected_casters_2d: Vec<(Vec<Point2D>, f64)> = Vec::new();
    for casting in &valid_casters {
        let projected = match project_polygon_onto_plane(
            &casting.vertices,
            &recv_verts[0],
            recv_normal,
            sun_dir,
        ) {
            Some(p) => p,
            None => continue,
        };
        let cast_2d: Vec<Point2D> = projected.iter()
            .map(|v| to_local_2d(v, &origin, &u_axis, &v_axis))
            .collect();
        if cast_2d.len() >= 3 {
            // Clip to receiver polygon first
            let clipped = sutherland_hodgman_clip(&cast_2d, &recv_2d);
            if clipped.len() >= 3 {
                projected_casters_2d.push((clipped, casting.solar_transmittance));
            }
        }
    }

    if projected_casters_2d.is_empty() {
        return 1.0;
    }

    // For single opaque caster, use exact polygon area (no overlap possible)
    if projected_casters_2d.len() == 1 && projected_casters_2d[0].1 == 0.0 {
        let shadow_area = polygon_area_2d(&projected_casters_2d[0].0);
        return (1.0 - shadow_area / recv_area).clamp(0.0, 1.0);
    }

    // Multiple casters or semi-transparent: use point sampling
    // 8×8 grid is accurate (verified: 32×32 gives same results within 7 kWh)
    const N_SAMPLES: usize = 8;
    let mut u_min = f64::MAX;
    let mut u_max = f64::MIN;
    let mut v_min = f64::MAX;
    let mut v_max = f64::MIN;
    for p in &recv_2d {
        u_min = u_min.min(p.x);
        u_max = u_max.max(p.x);
        v_min = v_min.min(p.y);
        v_max = v_max.max(p.y);
    }

    let mut n_total = 0u32;
    let mut n_sunlit = 0u32;
    for iu in 0..N_SAMPLES {
        let u_frac = (iu as f64 + 0.5) / N_SAMPLES as f64;
        let u_val = u_min + u_frac * (u_max - u_min);
        for iv in 0..N_SAMPLES {
            let v_frac = (iv as f64 + 0.5) / N_SAMPLES as f64;
            let v_val = v_min + v_frac * (v_max - v_min);
            let pt = Point2D { x: u_val, y: v_val };
            if !point_in_polygon_2d(&pt, &recv_2d) {
                continue;
            }
            n_total += 1;
            // Check if ANY caster's projected shadow covers this point
            let in_shadow = projected_casters_2d.iter().any(|(shadow, transmittance)| {
                *transmittance == 0.0 && point_in_polygon_2d(&pt, shadow)
            });
            if !in_shadow {
                n_sunlit += 1;
            }
        }
    }

    if n_total == 0 {
        return 1.0;
    }

    n_sunlit as f64 / n_total as f64
}

// ─── Diffuse Sky Shading (Hemisphere Sampling) ──────────────────────────────────

/// Compute the diffuse sky shading ratio for a surface.
///
/// Samples the upper hemisphere using a 6×24 grid (144 sky patches, matching
/// EnergyPlus SkyDifSolarShading()) and determines what fraction of isotropic
/// sky diffuse radiation reaches the surface after accounting for obstruction
/// by shading surfaces (overhangs, fins, detached shading).
///
/// Uses point-sampling on the receiver surface (5×5 grid = 25 sample points)
/// instead of polygon-based sunlit fraction to correctly handle overlapping
/// shadows from multiple casters (e.g., overhang + fins on the same window).
///
/// Returns a ratio in [0.0, 1.0]:
///   1.0 = no diffuse shading (full sky exposure)
///   < 1.0 = some sky patches blocked by shading surfaces
///
/// Reference: EnergyPlus SolarShading.cc, SkyDifSolarShading() function.
pub fn compute_diffuse_sky_shading_ratio(
    recv_verts: &[Point3D],
    recv_normal: &Vec3,
    casters: &[&ShadingPolygon],
) -> f64 {
    if recv_verts.len() < 3 || casters.is_empty() {
        return 1.0;
    }

    // Build local 2D coordinate system on the receiver
    let (origin, u_axis, v_axis) = build_local_coords(recv_verts, recv_normal);

    // Generate sample points on the receiver surface (5×5 grid)
    const N_SAMPLES_U: usize = 5;
    const N_SAMPLES_V: usize = 5;
    let recv_2d: Vec<Point2D> = recv_verts.iter()
        .map(|v| to_local_2d(v, &origin, &u_axis, &v_axis))
        .collect();

    // Compute bounding box of receiver in 2D
    let mut u_min = f64::MAX;
    let mut u_max = f64::MIN;
    let mut v_min = f64::MAX;
    let mut v_max = f64::MIN;
    for p in &recv_2d {
        u_min = u_min.min(p.x);
        u_max = u_max.max(p.x);
        v_min = v_min.min(p.y);
        v_max = v_max.max(p.y);
    }

    // Generate sample points within receiver polygon
    let mut sample_points_3d: Vec<Point3D> = Vec::new();
    for iu in 0..N_SAMPLES_U {
        let u_frac = (iu as f64 + 0.5) / N_SAMPLES_U as f64;
        let u_val = u_min + u_frac * (u_max - u_min);
        for iv in 0..N_SAMPLES_V {
            let v_frac = (iv as f64 + 0.5) / N_SAMPLES_V as f64;
            let v_val = v_min + v_frac * (v_max - v_min);
            let p2d = Point2D { x: u_val, y: v_val };
            // Check if point is inside the receiver polygon
            if point_in_polygon_2d(&p2d, &recv_2d) {
                // Convert back to 3D
                let p3d = Point3D::new(
                    origin.x + u_val * u_axis.x + v_val * v_axis.x,
                    origin.y + u_val * u_axis.y + v_val * v_axis.y,
                    origin.z + u_val * u_axis.z + v_val * v_axis.z,
                );
                sample_points_3d.push(p3d);
            }
        }
    }

    if sample_points_3d.is_empty() {
        return 1.0;
    }

    let n_samples = sample_points_3d.len() as f64;

    // Hemisphere sampling grid (matches E+ SolarShading.cc lines 142-148)
    const N_PHI: usize = 6;    // Altitude angle steps
    const N_THETA: usize = 24; // Azimuth angle steps
    let d_phi = std::f64::consts::FRAC_PI_2 / N_PHI as f64; // ~15°
    let d_theta = 2.0 * std::f64::consts::PI / N_THETA as f64; // ~15°
    let phi_min = 0.5 * d_phi; // 7.5° (avoid horizon)

    let mut with_shading = 0.0_f64;
    let mut without_shading = 0.0_f64;

    for i_phi in 0..N_PHI {
        let phi = phi_min + i_phi as f64 * d_phi;
        let cos_phi = phi.cos();
        let sin_phi = phi.sin();

        for i_theta in 0..N_THETA {
            let theta = i_theta as f64 * d_theta;
            let cos_theta = theta.cos();
            let sin_theta = theta.sin();

            // Direction FROM ground TOWARD this sky patch
            let toward_sky = Vec3::new(
                cos_phi * cos_theta,
                cos_phi * sin_theta,
                sin_phi,
            );

            // Cosine of incidence on the receiving surface
            let cos_aoi = toward_sky.dot(recv_normal);
            if cos_aoi <= 0.0 {
                continue; // Patch is behind the surface
            }

            // Isotropic sky radiance weight: cos(phi) * dPhi * dTheta * cos_aoi
            // cos(altitude) is the correct Jacobian for the solid angle element
            // dΩ = cos(φ)·dφ·dθ in altitude-azimuth coordinates. cos_aoi accounts
            // for the projected solid angle on the receiving surface.
            let weight = cos_phi * d_phi * d_theta * cos_aoi;
            without_shading += weight;

            // For each sample point, check if ANY caster blocks the ray
            // toward this sky patch. This naturally handles overlapping shadows.
            let mut n_sunlit = 0.0_f64;
            for pt in &sample_points_3d {
                let blocked = casters.iter().any(|caster| {
                    ray_intersects_polygon(pt, &toward_sky, &caster.vertices, &caster.normal)
                });
                if !blocked {
                    n_sunlit += 1.0;
                }
            }
            let sunlit_frac = n_sunlit / n_samples;
            with_shading += weight * sunlit_frac;
        }
    }

    if without_shading > 1e-10 {
        (with_shading / without_shading).clamp(0.0, 1.0)
    } else {
        1.0
    }
}

/// Test if a ray from `origin` in direction `dir` intersects a convex polygon.
///
/// Uses the Möller–Trumbore approach: first intersect ray with the polygon's
/// plane, then check if the intersection point is inside the polygon.
fn ray_intersects_polygon(
    origin: &Point3D,
    dir: &Vec3,
    poly_verts: &[Point3D],
    poly_normal: &Vec3,
) -> bool {
    if poly_verts.len() < 3 {
        return false;
    }

    // Check ray is heading toward the polygon's front or back face
    let denom = poly_normal.dot(dir);
    if denom.abs() < 1e-10 {
        return false; // Ray parallel to polygon plane
    }

    // Compute distance to intersection with polygon plane
    let d = Vec3::new(
        poly_verts[0].x - origin.x,
        poly_verts[0].y - origin.y,
        poly_verts[0].z - origin.z,
    );
    let t = poly_normal.dot(&d) / denom;

    if t < 0.001 {
        return false; // Intersection behind origin or too close
    }

    // Compute intersection point
    let hit = Point3D::new(
        origin.x + t * dir.x,
        origin.y + t * dir.y,
        origin.z + t * dir.z,
    );

    // Check if hit point is inside the polygon using cross product test
    point_in_polygon_3d(&hit, poly_verts, poly_normal)
}

/// Check if a 3D point lies inside a convex polygon (assumed planar).
///
/// Uses the cross-product winding test: the point is inside if it's on
/// the same side of all edges.
fn point_in_polygon_3d(point: &Point3D, verts: &[Point3D], normal: &Vec3) -> bool {
    let n = verts.len();
    for i in 0..n {
        let j = (i + 1) % n;
        let edge = Vec3::new(
            verts[j].x - verts[i].x,
            verts[j].y - verts[i].y,
            verts[j].z - verts[i].z,
        );
        let to_point = Vec3::new(
            point.x - verts[i].x,
            point.y - verts[i].y,
            point.z - verts[i].z,
        );
        let cross = edge.cross(&to_point);
        if cross.dot(normal) < -1e-6 {
            return false; // Point is outside this edge
        }
    }
    true
}

/// Check if a 2D point lies inside a polygon (winding number test).
fn point_in_polygon_2d(point: &Point2D, polygon: &[Point2D]) -> bool {
    let n = polygon.len();
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        if ((polygon[i].y > point.y) != (polygon[j].y > point.y))
            && (point.x < (polygon[j].x - polygon[i].x) * (point.y - polygon[i].y)
                / (polygon[j].y - polygon[i].y) + polygon[i].x)
        {
            inside = !inside;
        }
        j = i;
    }
    inside
}

// ─── Diffuse Horizon Shading (Near-Horizon Band) ─────────────────────────────

/// Compute the diffuse horizon shading ratio for a surface.
///
/// Samples rays near the horizon (altitude 0-10°) in the forward hemisphere
/// and determines what fraction is blocked by shading devices.
/// Horizontal overhangs generally don't block horizon rays (ratio ≈ 1.0),
/// but vertical fins do block horizon rays from their side directions.
///
/// Returns a ratio in [0.0, 1.0]:
///   1.0 = full horizon exposure
///   < 1.0 = some horizon directions blocked by shading surfaces
///
/// Reference: EnergyPlus DifShdgRatioHoriz in SolarShading.cc.
pub fn compute_diffuse_horizon_shading_ratio(
    recv_verts: &[Point3D],
    recv_normal: &Vec3,
    casters: &[&ShadingPolygon],
) -> f64 {
    if recv_verts.len() < 3 || casters.is_empty() {
        return 1.0;
    }

    // Build local coordinate system and sample points (same as sky ratio)
    let (origin, u_axis, v_axis) = build_local_coords(recv_verts, recv_normal);
    let recv_2d: Vec<Point2D> = recv_verts.iter()
        .map(|v| to_local_2d(v, &origin, &u_axis, &v_axis))
        .collect();

    // Compute bounding box
    let mut u_min = f64::MAX;
    let mut u_max = f64::MIN;
    let mut v_min = f64::MAX;
    let mut v_max = f64::MIN;
    for p in &recv_2d {
        u_min = u_min.min(p.x);
        u_max = u_max.max(p.x);
        v_min = v_min.min(p.y);
        v_max = v_max.max(p.y);
    }

    // Generate sample points (3×3 grid for horizon — less resolution needed)
    const N_SAMPLES: usize = 3;
    let mut sample_points_3d: Vec<Point3D> = Vec::new();
    for iu in 0..N_SAMPLES {
        let u_frac = (iu as f64 + 0.5) / N_SAMPLES as f64;
        let u_val = u_min + u_frac * (u_max - u_min);
        for iv in 0..N_SAMPLES {
            let v_frac = (iv as f64 + 0.5) / N_SAMPLES as f64;
            let v_val = v_min + v_frac * (v_max - v_min);
            let p2d = Point2D { x: u_val, y: v_val };
            if point_in_polygon_2d(&p2d, &recv_2d) {
                let p3d = Point3D::new(
                    origin.x + u_val * u_axis.x + v_val * v_axis.x,
                    origin.y + u_val * u_axis.y + v_val * v_axis.y,
                    origin.z + u_val * u_axis.z + v_val * v_axis.z,
                );
                sample_points_3d.push(p3d);
            }
        }
    }

    if sample_points_3d.is_empty() {
        return 1.0;
    }

    let n_samples = sample_points_3d.len() as f64;

    // Sample only horizon band: altitude 0-10° in 2 steps, 24 azimuth steps
    const N_PHI: usize = 2;
    const N_THETA: usize = 24;
    let d_phi = (10.0_f64).to_radians() / N_PHI as f64; // 5° steps
    let d_theta = 2.0 * std::f64::consts::PI / N_THETA as f64;
    let phi_min = 0.5 * d_phi;

    let mut with_shading = 0.0_f64;
    let mut without_shading = 0.0_f64;

    for i_phi in 0..N_PHI {
        let phi = phi_min + i_phi as f64 * d_phi;
        let cos_phi = phi.cos();
        let sin_phi = phi.sin();

        for i_theta in 0..N_THETA {
            let theta = i_theta as f64 * d_theta;
            let toward_sky = Vec3::new(
                cos_phi * theta.cos(),
                cos_phi * theta.sin(),
                sin_phi,
            );

            let cos_aoi = toward_sky.dot(recv_normal);
            if cos_aoi <= 0.0 {
                continue;
            }

            // Horizon band weight: cos(phi) * dPhi * dTheta * cos_aoi
            // Same Jacobian as sky ratio for consistency.
            let weight = cos_phi * d_phi * d_theta * cos_aoi;
            without_shading += weight;

            let mut n_sunlit = 0.0_f64;
            for pt in &sample_points_3d {
                let blocked = casters.iter().any(|caster| {
                    ray_intersects_polygon(pt, &toward_sky, &caster.vertices, &caster.normal)
                });
                if !blocked {
                    n_sunlit += 1.0;
                }
            }
            let sunlit_frac = n_sunlit / n_samples;
            with_shading += weight * sunlit_frac;
        }
    }

    if without_shading > 1e-10 {
        (with_shading / without_shading).clamp(0.0, 1.0)
    } else {
        1.0
    }
}

// ─── Sutherland-Hodgman Polygon Clipping ──────────────────────────────────────

/// Sutherland-Hodgman polygon clipping: clip `subject` against `clip` polygon.
///
/// Both polygons are in 2D. The clip polygon must be convex (all standard
/// building surfaces are rectangular). Returns the intersection polygon,
/// or an empty vector if there is no overlap.
///
/// Reference: Sutherland & Hodgman (1974), as used in EnergyPlus SolarShading.cc.
pub fn sutherland_hodgman_clip(
    subject: &[Point2D],
    clip: &[Point2D],
) -> Vec<Point2D> {
    if subject.is_empty() || clip.len() < 3 {
        return vec![];
    }

    let mut output = subject.to_vec();
    let n_clip = clip.len();

    for i in 0..n_clip {
        if output.is_empty() {
            return vec![];
        }

        let edge_start = clip[i];
        let edge_end = clip[(i + 1) % n_clip];

        let input = std::mem::take(&mut output);
        let n_input = input.len();

        for j in 0..n_input {
            let current = input[j];
            let previous = input[(j + n_input - 1) % n_input];

            let curr_inside = is_inside(current, edge_start, edge_end);
            let prev_inside = is_inside(previous, edge_start, edge_end);

            if curr_inside {
                if !prev_inside {
                    // Entering: add intersection then current
                    if let Some(p) = line_intersect_2d(previous, current, edge_start, edge_end) {
                        output.push(p);
                    }
                }
                output.push(current);
            } else if prev_inside {
                // Leaving: add intersection only
                if let Some(p) = line_intersect_2d(previous, current, edge_start, edge_end) {
                    output.push(p);
                }
            }
        }
    }

    output
}

/// Check if a point is on the "inside" (left) side of a directed edge.
/// For a CCW-wound clip polygon, "inside" is to the left of each edge.
fn is_inside(point: Point2D, edge_start: Point2D, edge_end: Point2D) -> bool {
    let cross = (edge_end.x - edge_start.x) * (point.y - edge_start.y)
              - (edge_end.y - edge_start.y) * (point.x - edge_start.x);
    cross >= -1e-10 // On left side or on edge (with tiny tolerance)
}

/// Line-line intersection for Sutherland-Hodgman clipping.
///
/// Computes the intersection of line segment (p1→p2) with line (p3→p4).
fn line_intersect_2d(
    p1: Point2D, p2: Point2D,
    p3: Point2D, p4: Point2D,
) -> Option<Point2D> {
    let denom = (p1.x - p2.x) * (p3.y - p4.y) - (p1.y - p2.y) * (p3.x - p4.x);
    if denom.abs() < 1e-12 {
        return None; // Parallel
    }
    let t = ((p1.x - p3.x) * (p3.y - p4.y) - (p1.y - p3.y) * (p3.x - p4.x)) / denom;
    Some(Point2D {
        x: p1.x + t * (p2.x - p1.x),
        y: p1.y + t * (p2.y - p1.y),
    })
}

/// Area of a 2D polygon using the shoelace formula.
pub fn polygon_area_2d(poly: &[Point2D]) -> f64 {
    let n = poly.len();
    if n < 3 { return 0.0; }
    let mut area = 0.0;
    for i in 0..n {
        let j = (i + 1) % n;
        area += poly[i].x * poly[j].y;
        area -= poly[j].x * poly[i].y;
    }
    (area / 2.0).abs()
}

// ─── Shadow Projection ───────────────────────────────────────────────────────

/// Project a 3D point onto a plane along the sun direction vector.
///
/// Plane defined by `plane_point` and unit `plane_normal`.
/// Ray: `point + t * sun_dir`. Solves for t where the ray hits the plane.
///
/// Returns `None` if the ray is parallel to the plane or the shadow
/// falls behind the receiving surface (t < 0).
fn project_point_onto_plane(
    point: &Point3D,
    plane_point: &Point3D,
    plane_normal: &Vec3,
    sun_dir: &Vec3,
) -> Option<Point3D> {
    let denom = sun_dir.dot(plane_normal);
    if denom.abs() < 1e-10 {
        return None; // Sun direction parallel to surface
    }
    let diff = Vec3::new(
        plane_point.x - point.x,
        plane_point.y - point.y,
        plane_point.z - point.z,
    );
    let t = diff.dot(plane_normal) / denom;
    if t < -1e-6 {
        return None; // Shadow falls behind the surface (casting polygon is behind)
    }
    Some(Point3D::new(
        point.x + t * sun_dir.x,
        point.y + t * sun_dir.y,
        point.z + t * sun_dir.z,
    ))
}

/// Project an entire casting polygon onto the receiving surface plane.
///
/// Returns the projected polygon vertices in 3D (all on the receiving plane),
/// or `None` if any vertex cannot be projected.
fn project_polygon_onto_plane(
    casting_verts: &[Point3D],
    plane_point: &Point3D,
    plane_normal: &Vec3,
    sun_dir: &Vec3,
) -> Option<Vec<Point3D>> {
    let mut projected = Vec::with_capacity(casting_verts.len());
    for v in casting_verts {
        match project_point_onto_plane(v, plane_point, plane_normal, sun_dir) {
            Some(p) => projected.push(p),
            None => return None,
        }
    }
    Some(projected)
}

// ─── Local 2D Coordinate System ──────────────────────────────────────────────

/// Build a local 2D coordinate system on a planar surface.
///
/// Returns `(origin, u_axis, v_axis)` where `u` and `v` are orthonormal
/// vectors in the plane of the surface:
/// - `u`: along the first edge of the polygon
/// - `v`: perpendicular to both normal and u (in-plane)
fn build_local_coords(vertices: &[Point3D], normal: &Vec3) -> (Point3D, Vec3, Vec3) {
    let origin = vertices[0];
    // u-axis: along first edge, normalized
    let edge = Vec3::new(
        vertices[1].x - vertices[0].x,
        vertices[1].y - vertices[0].y,
        vertices[1].z - vertices[0].z,
    );
    let u = edge.normalize();
    // v-axis: normal × u → perpendicular to both, lies in the surface plane
    let v = normal.cross(&u).normalize();
    (origin, u, v)
}

/// Transform a 3D point (assumed on the surface plane) to 2D local coordinates.
fn to_local_2d(point: &Point3D, origin: &Point3D, u: &Vec3, v: &Vec3) -> Point2D {
    let d = Vec3::new(
        point.x - origin.x,
        point.y - origin.y,
        point.z - origin.z,
    );
    Point2D { x: d.dot(u), y: d.dot(v) }
}

// ─── Overhang / Fin Vertex Generation ─────────────────────────────────────────

/// Generate shading polygon vertices for a horizontal overhang above a window.
///
/// The overhang is a rectangle projecting outward from the wall plane.
///
/// # Arguments
/// * `window_verts` — Window vertices, CCW from outside: [BL, BR, TR, TL]
/// * `overhang` — Overhang geometry parameters
/// * `wall_outward` — Unit outward normal of the parent wall
///
/// # Returns
/// 4 vertices of the overhang polygon (CCW from below / outside).
pub fn generate_overhang_vertices(
    window_verts: &[Point3D],
    overhang: &OverhangInput,
    wall_outward: &Vec3,
) -> Vec<Point3D> {
    assert!(window_verts.len() >= 4, "Window must have at least 4 vertices");

    let bl = window_verts[0];
    let br = window_verts[1];
    let tr = window_verts[2];
    let tl = window_verts[3];

    // Window local directions
    let horizontal = Vec3::new(br.x - bl.x, br.y - bl.y, br.z - bl.z).normalize();
    let vertical = Vec3::new(tl.x - bl.x, tl.y - bl.y, tl.z - bl.z).normalize();
    let outward = wall_outward.normalize();

    // Overhang base edge: at offset_above above window top, extending beyond window edges
    let base_left = Point3D::new(
        tl.x + vertical.x * overhang.offset_above - horizontal.x * overhang.left_extension,
        tl.y + vertical.y * overhang.offset_above - horizontal.y * overhang.left_extension,
        tl.z + vertical.z * overhang.offset_above - horizontal.z * overhang.left_extension,
    );
    let base_right = Point3D::new(
        tr.x + vertical.x * overhang.offset_above + horizontal.x * overhang.right_extension,
        tr.y + vertical.y * overhang.offset_above + horizontal.y * overhang.right_extension,
        tr.z + vertical.z * overhang.offset_above + horizontal.z * overhang.right_extension,
    );

    // Overhang outer edge: base + depth in outward direction
    let outer_left = Point3D::new(
        base_left.x + outward.x * overhang.depth,
        base_left.y + outward.y * overhang.depth,
        base_left.z + outward.z * overhang.depth,
    );
    let outer_right = Point3D::new(
        base_right.x + outward.x * overhang.depth,
        base_right.y + outward.y * overhang.depth,
        base_right.z + outward.z * overhang.depth,
    );

    // CCW from outside (looking up at the overhang from below)
    // The overhang's outward normal should point downward (visible from below).
    // Winding: base_left → outer_left → outer_right → base_right
    vec![base_left, outer_left, outer_right, base_right]
}

/// Generate shading polygon vertices for a vertical fin beside a window.
///
/// # Arguments
/// * `window_verts` — Window vertices, CCW from outside: [BL, BR, TR, TL]
/// * `fin` — Fin geometry parameters
/// * `wall_outward` — Unit outward normal of the parent wall
/// * `side` — Which side of the window the fin is on
///
/// # Returns
/// 4 vertices of the fin polygon (CCW from the fin's outer face).
pub fn generate_fin_vertices(
    window_verts: &[Point3D],
    fin: &FinInput,
    wall_outward: &Vec3,
    side: FinSide,
) -> Vec<Point3D> {
    assert!(window_verts.len() >= 4, "Window must have at least 4 vertices");

    let bl = window_verts[0];
    let br = window_verts[1];
    let tr = window_verts[2];
    let tl = window_verts[3];

    let vertical = Vec3::new(tl.x - bl.x, tl.y - bl.y, tl.z - bl.z).normalize();
    let outward = wall_outward.normalize();

    // Fin base edge: on the wall, at the side of the window
    let (base_bottom, base_top) = match side {
        FinSide::Left => {
            let bottom = Point3D::new(
                bl.x - vertical.x * fin.extend_below,
                bl.y - vertical.y * fin.extend_below,
                bl.z - vertical.z * fin.extend_below,
            );
            let top = Point3D::new(
                tl.x + vertical.x * fin.extend_above,
                tl.y + vertical.y * fin.extend_above,
                tl.z + vertical.z * fin.extend_above,
            );
            (bottom, top)
        }
        FinSide::Right => {
            let bottom = Point3D::new(
                br.x - vertical.x * fin.extend_below,
                br.y - vertical.y * fin.extend_below,
                br.z - vertical.z * fin.extend_below,
            );
            let top = Point3D::new(
                tr.x + vertical.x * fin.extend_above,
                tr.y + vertical.y * fin.extend_above,
                tr.z + vertical.z * fin.extend_above,
            );
            (bottom, top)
        }
    };

    // Fin outer edge: base + depth in outward direction
    let outer_bottom = Point3D::new(
        base_bottom.x + outward.x * fin.depth,
        base_bottom.y + outward.y * fin.depth,
        base_bottom.z + outward.z * fin.depth,
    );
    let outer_top = Point3D::new(
        base_top.x + outward.x * fin.depth,
        base_top.y + outward.y * fin.depth,
        base_top.z + outward.z * fin.depth,
    );

    // CCW from outside (facing the fin from outside the building)
    match side {
        FinSide::Left => {
            // Left fin is viewed from the left side; outward normal faces left
            vec![base_bottom, base_top, outer_top, outer_bottom]
        }
        FinSide::Right => {
            // Right fin: outward normal faces right
            vec![outer_bottom, outer_top, base_top, base_bottom]
        }
    }
}

// ─── Shading Polygon Resolution ──────────────────────────────────────────────

/// Resolve explicit shading surface inputs into runtime shading polygons.
pub fn resolve_shading_surfaces(inputs: &[ShadingSurfaceInput]) -> Vec<ShadingPolygon> {
    inputs.iter().filter_map(|ss| {
        if ss.vertices.len() < 3 {
            return None;
        }
        let normal = newell_normal(&ss.vertices).normalize();
        Some(ShadingPolygon {
            name: ss.name.clone(),
            vertices: ss.vertices.clone(),
            normal,
            solar_transmittance: ss.solar_transmittance,
        })
    }).collect()
}

/// Create a ShadingPolygon from a building surface (for self-shading).
///
/// All outdoor building surfaces with vertices are potential shadow casters.
pub fn surface_to_shading_polygon(name: &str, vertices: &[Point3D]) -> Option<ShadingPolygon> {
    if vertices.len() < 3 {
        return None;
    }
    let normal = newell_normal(vertices).normalize();
    Some(ShadingPolygon {
        name: format!("self:{}", name),
        vertices: vertices.to_vec(),
        normal,
        solar_transmittance: 0.0,
    })
}


#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_relative_eq;

    // ── Sutherland-Hodgman Clipping Tests ──────────────────────────────────

    #[test]
    fn test_clip_square_overlap() {
        // Two overlapping unit squares: one at (0,0)-(1,1), one at (0.5,0.5)-(1.5,1.5)
        let subject = vec![
            Point2D { x: 0.5, y: 0.5 },
            Point2D { x: 1.5, y: 0.5 },
            Point2D { x: 1.5, y: 1.5 },
            Point2D { x: 0.5, y: 1.5 },
        ];
        let clip = vec![
            Point2D { x: 0.0, y: 0.0 },
            Point2D { x: 1.0, y: 0.0 },
            Point2D { x: 1.0, y: 1.0 },
            Point2D { x: 0.0, y: 1.0 },
        ];
        let result = sutherland_hodgman_clip(&subject, &clip);
        let area = polygon_area_2d(&result);
        assert_relative_eq!(area, 0.25, max_relative = 0.01); // 0.5 × 0.5
    }

    #[test]
    fn test_clip_no_overlap() {
        let subject = vec![
            Point2D { x: 2.0, y: 2.0 },
            Point2D { x: 3.0, y: 2.0 },
            Point2D { x: 3.0, y: 3.0 },
            Point2D { x: 2.0, y: 3.0 },
        ];
        let clip = vec![
            Point2D { x: 0.0, y: 0.0 },
            Point2D { x: 1.0, y: 0.0 },
            Point2D { x: 1.0, y: 1.0 },
            Point2D { x: 0.0, y: 1.0 },
        ];
        let result = sutherland_hodgman_clip(&subject, &clip);
        assert!(result.is_empty() || polygon_area_2d(&result) < 1e-10);
    }

    #[test]
    fn test_clip_full_containment() {
        // Subject fully inside clip → result = subject
        let subject = vec![
            Point2D { x: 0.25, y: 0.25 },
            Point2D { x: 0.75, y: 0.25 },
            Point2D { x: 0.75, y: 0.75 },
            Point2D { x: 0.25, y: 0.75 },
        ];
        let clip = vec![
            Point2D { x: 0.0, y: 0.0 },
            Point2D { x: 1.0, y: 0.0 },
            Point2D { x: 1.0, y: 1.0 },
            Point2D { x: 0.0, y: 1.0 },
        ];
        let result = sutherland_hodgman_clip(&subject, &clip);
        let area = polygon_area_2d(&result);
        assert_relative_eq!(area, 0.25, max_relative = 0.01);
    }

    #[test]
    fn test_clip_half_overlap() {
        // Subject covers right half of clip
        let subject = vec![
            Point2D { x: 0.5, y: 0.0 },
            Point2D { x: 2.0, y: 0.0 },
            Point2D { x: 2.0, y: 1.0 },
            Point2D { x: 0.5, y: 1.0 },
        ];
        let clip = vec![
            Point2D { x: 0.0, y: 0.0 },
            Point2D { x: 1.0, y: 0.0 },
            Point2D { x: 1.0, y: 1.0 },
            Point2D { x: 0.0, y: 1.0 },
        ];
        let result = sutherland_hodgman_clip(&subject, &clip);
        let area = polygon_area_2d(&result);
        assert_relative_eq!(area, 0.5, max_relative = 0.01);
    }

    #[test]
    fn test_clip_triangle_in_square() {
        let subject = vec![
            Point2D { x: 0.5, y: 0.0 },
            Point2D { x: 1.0, y: 0.5 },
            Point2D { x: 0.5, y: 1.0 },
        ];
        let clip = vec![
            Point2D { x: 0.0, y: 0.0 },
            Point2D { x: 1.0, y: 0.0 },
            Point2D { x: 1.0, y: 1.0 },
            Point2D { x: 0.0, y: 1.0 },
        ];
        let result = sutherland_hodgman_clip(&subject, &clip);
        let area = polygon_area_2d(&result);
        // Triangle: base=1.0 (from y=0 to y=1), height=0.5 (from x=0.5 to x=1.0)
        assert_relative_eq!(area, 0.25, max_relative = 0.01);
    }

    // ── Polygon Area 2D Tests ─────────────────────────────────────────────

    #[test]
    fn test_area_2d_unit_square() {
        let sq = vec![
            Point2D { x: 0.0, y: 0.0 },
            Point2D { x: 1.0, y: 0.0 },
            Point2D { x: 1.0, y: 1.0 },
            Point2D { x: 0.0, y: 1.0 },
        ];
        assert_relative_eq!(polygon_area_2d(&sq), 1.0, max_relative = 0.001);
    }

    #[test]
    fn test_area_2d_triangle() {
        let tri = vec![
            Point2D { x: 0.0, y: 0.0 },
            Point2D { x: 4.0, y: 0.0 },
            Point2D { x: 0.0, y: 3.0 },
        ];
        assert_relative_eq!(polygon_area_2d(&tri), 6.0, max_relative = 0.001);
    }

    // ── Shadow Projection Tests ──────────────────────────────────────────

    #[test]
    fn test_project_point_straight_down() {
        // Sun directly overhead → project straight down onto floor
        let sun_dir = Vec3::new(0.0, 0.0, -1.0);
        let plane_point = Point3D::new(0.0, 0.0, 0.0);
        let plane_normal = Vec3::new(0.0, 0.0, 1.0); // Floor facing up

        let point = Point3D::new(3.0, 4.0, 5.0);
        let projected = project_point_onto_plane(&point, &plane_point, &plane_normal, &sun_dir);
        assert!(projected.is_some());
        let p = projected.unwrap();
        assert_relative_eq!(p.x, 3.0, epsilon = 0.001);
        assert_relative_eq!(p.y, 4.0, epsilon = 0.001);
        assert_relative_eq!(p.z, 0.0, epsilon = 0.001);
    }

    #[test]
    fn test_project_point_45_degree() {
        // Sun at 45° from south → projects onto vertical south wall
        let sun_dir = Vec3::new(0.0, 1.0, -1.0).normalize(); // from south-above toward north-below
        let plane_point = Point3D::new(0.0, 0.0, 0.0);
        let plane_normal = Vec3::new(0.0, -1.0, 0.0); // South wall facing south

        let point = Point3D::new(4.0, -1.0, 2.0); // Overhang 1m south, 2m high
        let projected = project_point_onto_plane(&point, &plane_point, &plane_normal, &sun_dir);
        assert!(projected.is_some());
        let p = projected.unwrap();
        // At 45°, moving 1m south = 1m in y = 1m drop in z
        // Point at y=-1 projected to y=0 plane: t = 1/cos(45°) * something
        // sun_dir·normal = (0)(0) + (1)(-1) + (-1)(0) = -1/√2 ≈ -0.707
        // diff·normal = (0-4)(0) + (0-(-1))(-1) + (0-2)(0) = -1
        // t = -1 / (-0.707) ≈ 1.414
        // projected_y = -1 + 1.414 * 0.707 ≈ 0.0 ✓
        // projected_z = 2 + 1.414 * (-0.707) ≈ 1.0
        assert_relative_eq!(p.y, 0.0, epsilon = 0.01);
        assert_relative_eq!(p.z, 1.0, epsilon = 0.01);
    }

    // ── Sunlit Fraction Tests ─────────────────────────────────────────────

    #[test]
    fn test_sunlit_no_casters() {
        // No casting surfaces → fully sunlit
        let recv = vec![
            Point3D::new(0.0, 0.0, 0.0),
            Point3D::new(8.0, 0.0, 0.0),
            Point3D::new(8.0, 0.0, 2.7),
            Point3D::new(0.0, 0.0, 2.7),
        ];
        let normal = newell_normal(&recv).normalize();
        let sun_dir = Vec3::new(0.0, 1.0, -1.0).normalize(); // from south-above

        let sf = calculate_sunlit_fraction(&recv, &normal, &[], &sun_dir);
        assert_relative_eq!(sf, 1.0, epsilon = 0.001);
    }

    #[test]
    fn test_sunlit_with_overhang() {
        // South wall (2m tall, 8m wide) with 1m overhang at top
        // Sun at 45° altitude from due south
        let recv = vec![
            Point3D::new(0.0, 0.0, 0.0),
            Point3D::new(8.0, 0.0, 0.0),
            Point3D::new(8.0, 0.0, 2.0),
            Point3D::new(0.0, 0.0, 2.0),
        ];
        let recv_normal = Vec3::new(0.0, -1.0, 0.0); // facing south

        // Horizontal overhang: 8m wide, 1m deep, at z=2.0 (top of wall)
        let overhang_verts = vec![
            Point3D::new(0.0, 0.0,  2.0),
            Point3D::new(0.0, -1.0, 2.0),
            Point3D::new(8.0, -1.0, 2.0),
            Point3D::new(8.0, 0.0,  2.0),
        ];
        let ovh = ShadingPolygon {
            name: "overhang".into(),
            vertices: overhang_verts,
            normal: Vec3::new(0.0, 0.0, -1.0), // faces down
            solar_transmittance: 0.0,
        };

        // Sun at 45° from due south → shadow drops 1m down the wall
        // (overhang depth 1m × tan(90°-45°) = 1m shadow height)
        // Wall is 2m tall → 1m shaded, 1m sunlit → sunlit_fraction ≈ 0.5
        let sun_dir = Vec3::new(0.0, 1.0, -1.0).normalize();

        let casters: Vec<&ShadingPolygon> = vec![&ovh];
        let sf = calculate_sunlit_fraction(&recv, &recv_normal, &casters, &sun_dir);
        assert_relative_eq!(sf, 0.5, max_relative = 0.05);
    }

    #[test]
    fn test_sunlit_sun_behind_surface() {
        // Sun from north hitting a south-facing wall → sun is behind → sunlit = 0
        let recv = vec![
            Point3D::new(0.0, 0.0, 0.0),
            Point3D::new(8.0, 0.0, 0.0),
            Point3D::new(8.0, 0.0, 2.7),
            Point3D::new(0.0, 0.0, 2.7),
        ];
        let normal = Vec3::new(0.0, -1.0, 0.0); // facing south
        let sun_dir = Vec3::new(0.0, -1.0, -1.0).normalize(); // from north

        let sf = calculate_sunlit_fraction(&recv, &normal, &[], &sun_dir);
        assert_relative_eq!(sf, 0.0, epsilon = 0.001);
    }

    // ── Overhang Vertex Generation Tests ─────────────────────────────────

    #[test]
    fn test_generate_overhang_south_window() {
        // South-facing window: 3m wide × 2m tall on south wall (y=0)
        let window_verts = vec![
            Point3D::new(0.5, 0.0, 0.2),  // BL
            Point3D::new(3.5, 0.0, 0.2),  // BR
            Point3D::new(3.5, 0.0, 2.2),  // TR
            Point3D::new(0.5, 0.0, 2.2),  // TL
        ];
        let wall_normal = Vec3::new(0.0, -1.0, 0.0); // south-facing

        let ovh = OverhangInput {
            depth: 1.0,
            offset_above: 0.5,
            left_extension: 0.0,
            right_extension: 0.0,
        };

        let verts = generate_overhang_vertices(&window_verts, &ovh, &wall_normal);
        assert_eq!(verts.len(), 4);

        // Base should be at z = 2.2 + 0.5 = 2.7 (window top + offset)
        // Outer edge should be at y = 0 - 1.0 = -1.0 (depth outward from south wall)
        for v in &verts {
            assert_relative_eq!(v.z, 2.7, epsilon = 0.01);
        }
        // Check that overhang extends from x=0.5 to x=3.5 (no extension)
        let min_x = verts.iter().map(|v| v.x).fold(f64::INFINITY, f64::min);
        let max_x = verts.iter().map(|v| v.x).fold(f64::NEG_INFINITY, f64::max);
        assert_relative_eq!(min_x, 0.5, epsilon = 0.01);
        assert_relative_eq!(max_x, 3.5, epsilon = 0.01);
    }

    #[test]
    fn test_generate_overhang_case610() {
        // Case 610: full 8m wide overhang, 1m deep, at z=2.7 (0.5m above window top at z=2.2)
        // Two windows side by side, but overhang spans full wall
        // We'd add via shading_surfaces directly for Case 610, but test the generation
        let window_verts = vec![
            Point3D::new(0.0, 0.0, 0.0),  // full south wall bottom
            Point3D::new(8.0, 0.0, 0.0),
            Point3D::new(8.0, 0.0, 2.2),  // to window top
            Point3D::new(0.0, 0.0, 2.2),
        ];
        let wall_normal = Vec3::new(0.0, -1.0, 0.0);

        let ovh = OverhangInput {
            depth: 1.0,
            offset_above: 0.5,
            left_extension: 0.0,
            right_extension: 0.0,
        };

        let verts = generate_overhang_vertices(&window_verts, &ovh, &wall_normal);
        // All vertices at z=2.7
        for v in &verts {
            assert_relative_eq!(v.z, 2.7, epsilon = 0.01);
        }
        // y range: 0.0 (wall) to -1.0 (1m outward from south wall)
        let min_y = verts.iter().map(|v| v.y).fold(f64::INFINITY, f64::min);
        let max_y = verts.iter().map(|v| v.y).fold(f64::NEG_INFINITY, f64::max);
        assert_relative_eq!(min_y, -1.0, epsilon = 0.01);
        assert_relative_eq!(max_y, 0.0, epsilon = 0.01);
    }
}
