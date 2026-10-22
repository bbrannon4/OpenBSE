//! Spatial thermal comfort grid (#136).
//!
//! Splits a zone's floor into occupant points and gives each point its own
//! mean radiant temperature, from view factors to the zone's real surface
//! polygons, plus a solar MRT increment where a window beam reaches it.
//!
//! **Physics limit:** the zone air node is well mixed, so air temperature and
//! humidity are the same at every point. All spatial variation comes from the
//! radiant field and direct sun. The grid is not a CFD air-temperature field.
//!
//! The surface temperatures fed in come from the heat balance, whose interior
//! long-wave exchange still uses the zone bounding box (#52). Only the
//! occupant's view of those surfaces uses the real polygons.
//!
//! View factors are geometric and constant, so they are computed once per zone;
//! each timestep is then a weighted sum over the zone's surfaces per point.

use std::f64::consts::PI;

use openbse_psychrometrics::comfort;
use rayon::prelude::*;
use thiserror::Error;

use crate::geometry::{newell_normal, polygon_centroid, Point3D, Vec3};
use crate::shading::{build_local_coords, to_local_2d, Point2D};
use crate::surface::SurfaceType;
use crate::zone::ZoneComfortConfig;

/// Long-wave emissivity of the body, matching the zone-level comfort metrics.
const BODY_EMISSIVITY: f64 = 0.95;

/// Horizontal directions sampled for the person models. Eight is enough for
/// the azimuth average to be insensitive to how the room is rotated.
const N_AZIMUTHS: usize = 8;

/// Refuse grids this large; they signal a unit mistake in the spacing, and
/// the view-factor matrix would not fit in memory anyway.
const MAX_CANDIDATE_POINTS: usize = 5_000_000;

const KELVIN: f64 = 273.15;

/// How an occupant point weights the surfaces it sees.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OccupantModel {
    /// ISO 7726 seated person, with plane radiant temperatures averaged over
    /// azimuth.
    #[default]
    Seated,
    /// ISO 7726 standing person, averaged over azimuth.
    Standing,
    /// Uniform sphere (solid-angle view factors), as a globe thermometer sees
    /// the room.
    Sphere,
}

impl OccupantModel {
    /// Height of the radiant centre above the floor [m]: ISO 7726 abdomen
    /// level, 0.6 m seated and 1.1 m standing.
    pub fn default_height(self) -> f64 {
        match self {
            OccupantModel::Standing => 1.1,
            OccupantModel::Seated | OccupantModel::Sphere => 0.6,
        }
    }

    /// `(w_up_down, w_horizontal)` per direction, normalised so that
    /// `2·w_up_down + 4·w_horizontal = 1`. `None` for the sphere.
    fn plane_weights(self) -> Option<(f64, f64)> {
        // ISO 7726 Annex B weights. Front/back and left/right are averaged
        // because a year's occupants face every direction, so no orientation
        // has to be assumed.
        let (up_down, left_right, front_back) = match self {
            OccupantModel::Seated => (0.18, 0.22, 0.30),
            OccupantModel::Standing => (0.08, 0.23, 0.35),
            OccupantModel::Sphere => return None,
        };
        let total = 2.0 * (up_down + left_right + front_back);
        Some((up_down / total, 0.5 * (left_right + front_back) / total))
    }
}

/// Grid layout and occupant settings for one zone.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridSettings {
    /// Distance between neighbouring points [m].
    pub spacing: f64,
    /// Minimum distance from a point to the floor boundary [m]. Nobody sits
    /// against a wall, and a point on the boundary would see that wall as a
    /// half-space.
    pub wall_offset: f64,
    /// Point height above the floor [m]; `None` uses the occupant model's
    /// default.
    pub height: Option<f64>,
    pub occupant: OccupantModel,
}

impl Default for GridSettings {
    fn default() -> Self {
        Self {
            spacing: 0.5,
            wall_offset: 0.5,
            height: None,
            occupant: OccupantModel::default(),
        }
    }
}

/// One surface of the zone enclosure.
#[derive(Debug, Clone)]
pub struct GridSurface {
    /// Polygon vertices. Their winding is not relied on: several repo models
    /// have surfaces wound inward, so every test here works from either side.
    pub vertices: Vec<Point3D>,
    pub surface_type: SurfaceType,
    /// For a window, the index of its host surface in the same list. The host
    /// polygon includes the window opening, so the window's view factor is
    /// subtracted from it.
    pub host: Option<usize>,
}

#[derive(Debug, Error, PartialEq)]
pub enum GridError {
    #[error("comfort grid needs at least one floor surface with vertices")]
    NoFloor,
    #[error("comfort grid: surface {0} has fewer than 3 vertices or zero area")]
    DegenerateSurface(usize),
    #[error("comfort grid: window {window} has an invalid host index {host}")]
    InvalidHost { window: usize, host: usize },
    #[error("comfort grid: {0}")]
    InvalidSettings(String),
    #[error("comfort grid: no points fit inside the floor with the given wall offset")]
    NoPoints,
}

/// A window as a ray target for the direct-sun test.
#[derive(Debug, Clone)]
struct Aperture {
    surface: usize,
    origin: Point3D,
    u: Vec3,
    v: Vec3,
    /// Unit plane normal, either side.
    normal: Vec3,
    outline: Vec<Point2D>,
}

/// Occupant points of one zone with their precomputed view factors.
#[derive(Debug, Clone)]
pub struct ZoneGrid {
    pub points: Vec<Point3D>,
    n_surfaces: usize,
    /// Row-major `[point][surface]`, each row normalised to sum to 1.
    view_factors: Vec<f64>,
    coverage: Vec<f64>,
    apertures: Vec<Aperture>,
}

impl ZoneGrid {
    /// Lays out the grid on the zone's floor polygons and computes view
    /// factors from every point to every surface.
    pub fn build(surfaces: &[GridSurface], settings: &GridSettings) -> Result<Self, GridError> {
        validate_settings(settings)?;
        validate_surfaces(surfaces)?;

        let height = settings
            .height
            .unwrap_or_else(|| settings.occupant.default_height());
        let candidates = generate_points(surfaces, settings.spacing, settings.wall_offset, height)?;

        let n_surfaces = surfaces.len();
        let rows: Vec<(Point3D, Vec<f64>, f64)> = candidates
            .into_par_iter()
            .filter_map(|p| {
                let (row, coverage) = view_factor_row(p, surfaces, settings.occupant);
                // A point that sees nothing has no defined MRT; it can only
                // arise from surfaces that do not enclose the floor.
                (coverage > 1e-9).then_some((p, row, coverage))
            })
            .collect();
        if rows.is_empty() {
            return Err(GridError::NoPoints);
        }

        let mut points = Vec::with_capacity(rows.len());
        let mut view_factors = Vec::with_capacity(rows.len() * n_surfaces);
        let mut coverage = Vec::with_capacity(rows.len());
        for (p, row, c) in rows {
            points.push(p);
            view_factors.extend(row);
            coverage.push(c);
        }

        let apertures = surfaces
            .iter()
            .enumerate()
            .filter(|(_, s)| s.surface_type == SurfaceType::Window)
            .map(|(i, s)| Aperture::new(i, &s.vertices))
            .collect();

        Ok(Self {
            points,
            n_surfaces,
            view_factors,
            coverage,
            apertures,
        })
    }

    pub fn n_surfaces(&self) -> usize {
        self.n_surfaces
    }

    /// View factors from one point to each surface, in input order; they sum
    /// to 1. A host wall's entry excludes its windows.
    pub fn view_factors(&self, point: usize) -> &[f64] {
        &self.view_factors[point * self.n_surfaces..(point + 1) * self.n_surfaces]
    }

    /// Share of the point's view covered by surfaces before normalisation.
    /// Below 1 means the enclosure has gaps (missing walls, area-only
    /// surfaces); the gap is then treated as if it were at the temperature of
    /// the surfaces the point does see. Above 1 means a non-convex zone, where
    /// surfaces hidden behind others are counted too, since occlusion is not
    /// traced.
    pub fn coverage(&self, point: usize) -> f64 {
        self.coverage[point]
    }

    /// Lowest coverage over all points, for flagging open enclosures.
    pub fn min_coverage(&self) -> f64 {
        self.coverage.iter().copied().fold(f64::INFINITY, f64::min)
    }

    /// Long-wave mean radiant temperature at a point [°C], from the inside
    /// surface temperatures [°C] in input order.
    pub fn mrt(&self, point: usize, surface_temps: &[f64]) -> f64 {
        debug_assert_eq!(surface_temps.len(), self.n_surfaces);
        let sum: f64 = self
            .view_factors(point)
            .iter()
            .zip(surface_temps)
            .map(|(f, t)| f * (t + KELVIN).powi(4))
            .sum();
        sum.powf(0.25) - KELVIN
    }

    /// Beam irradiance reaching a point through the zone's windows [W/m²
    /// normal to the beam], or 0 when no window lies between the point and
    /// the sun.
    ///
    /// `toward_sun` points from the ground to the sun. `irradiance_by_surface`
    /// is indexed like the input surfaces and holds each window's
    /// [`window_beam_irradiance`]; other entries are ignored.
    pub fn beam_irradiance(
        &self,
        point: usize,
        toward_sun: Vec3,
        irradiance_by_surface: &[f64],
    ) -> f64 {
        let p = self.points[point];
        let s = toward_sun.normalize();
        let mut best: f64 = 0.0;
        for ap in &self.apertures {
            let e = irradiance_by_surface[ap.surface];
            if e <= 0.0 {
                continue;
            }
            // The point is inside the zone, so a ray that reaches the window
            // plane ahead of it is leaving through the glass. That avoids
            // relying on the window's winding to tell inside from outside.
            let denom = s.dot(&ap.normal);
            if denom.abs() <= 1e-9 {
                continue;
            }
            let t = (ap.origin - p).dot(&ap.normal) / denom;
            if t <= 1e-9 {
                continue;
            }
            let hit = Point3D::new(p.x + s.x * t, p.y + s.y * t, p.z + s.z * t);
            let q = to_local_2d(&hit, &ap.origin, &ap.u, &ap.v);
            if point_in_polygon(q.x, q.y, &ap.outline) {
                // Overlapping coplanar windows would otherwise double count.
                best = best.max(e);
            }
        }
        best
    }
}

/// Beam irradiance transmitted through a window [W/m² normal to the beam].
///
/// The heat balance's transmitted beam power already includes the angular
/// transmittance, the window's sunlit fraction and any blind. Dividing by the
/// projected area turns it back into irradiance. External shading therefore
/// dims the whole sun patch instead of moving its edge, which is exact only
/// for unshaded windows.
pub fn window_beam_irradiance(transmitted_beam_w: f64, net_area: f64, cos_incidence: f64) -> f64 {
    if transmitted_beam_w <= 0.0 || net_area <= 0.0 || cos_incidence <= 1e-3 {
        return 0.0;
    }
    transmitted_beam_w / (net_area * cos_incidence)
}

/// Zone air state shared by every point in the zone.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ZoneAirState {
    /// Dry-bulb temperature [°C].
    pub temperature: f64,
    /// Relative humidity [0–1].
    pub relative_humidity: f64,
}

/// Comfort metrics at one occupant point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PointComfort {
    /// Long-wave MRT [°C].
    pub mrt: f64,
    /// Solar increment to MRT from direct beam on the occupant [K].
    pub solar_delta_mrt: f64,
    /// Operative temperature from the solar-corrected MRT [°C].
    pub operative_temperature: f64,
    pub pmv: f64,
    /// [%]
    pub ppd: f64,
}

/// Comfort metrics at a point, using the same correlations as the zone-level
/// outputs so the two are directly comparable.
pub fn point_comfort(
    mrt: f64,
    beam_irradiance: f64,
    solar_altitude: f64,
    air: ZoneAirState,
    cfg: &ZoneComfortConfig,
) -> PointComfort {
    let solar_delta_mrt = comfort::solar_mrt_correction(
        beam_irradiance,
        solar_altitude,
        mrt,
        cfg.solar_absorptivity,
        BODY_EMISSIVITY,
    );
    let mrt_eff = mrt + solar_delta_mrt;
    let pmv = comfort::pmv(
        air.temperature,
        mrt_eff,
        cfg.air_velocity,
        air.relative_humidity,
        cfg.metabolic_rate,
        cfg.clothing,
    );
    PointComfort {
        mrt,
        solar_delta_mrt,
        operative_temperature: comfort::operative_temperature(air.temperature, mrt_eff),
        pmv,
        ppd: comfort::ppd(pmv),
    }
}

/// Thermal sensation class of a point against a PMV limit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sensation {
    Cold,
    Comfortable,
    Hot,
}

/// Effective-MRT thresholds where PMV crosses ±limit for one zone air state.
///
/// Within a zone and hour everything in PMV except MRT is shared, and PMV
/// rises monotonically with MRT. Solving for the two crossings once lets an
/// annual pass classify every point with two comparisons instead of an
/// iterative PMV call per point and hour.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PmvBand {
    /// Points with a lower effective MRT have PMV < −limit.
    pub cold_below: f64,
    /// Points with a higher effective MRT have PMV > +limit.
    pub hot_above: f64,
}

impl PmvBand {
    /// Effective MRT range searched [°C]; beyond it every point is classed
    /// by the end it falls past.
    const MRT_RANGE: (f64, f64) = (-50.0, 120.0);

    pub fn new(air: ZoneAirState, cfg: &ZoneComfortConfig, limit: f64) -> Self {
        let pmv_at = |mrt: f64| {
            comfort::pmv(
                air.temperature,
                mrt,
                cfg.air_velocity,
                air.relative_humidity,
                cfg.metabolic_rate,
                cfg.clothing,
            )
        };
        Self {
            cold_below: mrt_where_pmv_reaches(-limit, pmv_at),
            hot_above: mrt_where_pmv_reaches(limit, pmv_at),
        }
    }

    pub fn classify(&self, effective_mrt: f64) -> Sensation {
        if effective_mrt < self.cold_below {
            Sensation::Cold
        } else if effective_mrt > self.hot_above {
            Sensation::Hot
        } else {
            Sensation::Comfortable
        }
    }
}

/// MRT at which a monotonically rising PMV reaches `target`, or ∓∞ when the
/// whole search range lies above or below it.
fn mrt_where_pmv_reaches(target: f64, pmv_at: impl Fn(f64) -> f64) -> f64 {
    let (mut lo, mut hi) = PmvBand::MRT_RANGE;
    if pmv_at(lo) >= target {
        return f64::NEG_INFINITY;
    }
    if pmv_at(hi) < target {
        return f64::INFINITY;
    }
    // 40 halvings of a 170 K range resolve well below 1e-9 K.
    for _ in 0..40 {
        let mid = 0.5 * (lo + hi);
        if pmv_at(mid) < target {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    0.5 * (lo + hi)
}

// ─── Validation ─────────────────────────────────────────────────────────────

fn validate_settings(settings: &GridSettings) -> Result<(), GridError> {
    if !(settings.spacing.is_finite() && settings.spacing > 0.0) {
        return Err(GridError::InvalidSettings(format!(
            "spacing must be positive, got {}",
            settings.spacing
        )));
    }
    if !(settings.wall_offset.is_finite() && settings.wall_offset >= 0.0) {
        return Err(GridError::InvalidSettings(format!(
            "wall_offset must be zero or positive, got {}",
            settings.wall_offset
        )));
    }
    if let Some(h) = settings.height {
        if !(h.is_finite() && h > 0.0) {
            return Err(GridError::InvalidSettings(format!(
                "height must be positive, got {h}"
            )));
        }
    }
    Ok(())
}

fn validate_surfaces(surfaces: &[GridSurface]) -> Result<(), GridError> {
    for (i, s) in surfaces.iter().enumerate() {
        if s.vertices.len() < 3 || newell_normal(&s.vertices).magnitude() < 1e-12 {
            return Err(GridError::DegenerateSurface(i));
        }
        if let Some(h) = s.host {
            let valid =
                h < surfaces.len() && h != i && surfaces[h].surface_type != SurfaceType::Window;
            if !valid {
                return Err(GridError::InvalidHost { window: i, host: h });
            }
        }
    }
    Ok(())
}

// ─── Grid layout ────────────────────────────────────────────────────────────

/// Candidate points on a regular grid over all floor polygons.
///
/// The grid is aligned with the longest edge of the largest floor polygon, so
/// rotated buildings get rows parallel to their walls, and it is centred on
/// the floor's extent so a symmetric room gets a symmetric grid.
fn generate_points(
    surfaces: &[GridSurface],
    spacing: f64,
    wall_offset: f64,
    height: f64,
) -> Result<Vec<Point3D>, GridError> {
    let floors: Vec<&[Point3D]> = surfaces
        .iter()
        .filter(|s| s.surface_type == SurfaceType::Floor)
        .map(|s| s.vertices.as_slice())
        .collect();
    if floors.is_empty() {
        return Err(GridError::NoFloor);
    }

    let (origin, axis) = grid_frame(&floors);
    let to_local = |p: &Point3D| {
        let (dx, dy) = (p.x - origin.0, p.y - origin.1);
        (dx * axis.0 + dy * axis.1, -dx * axis.1 + dy * axis.0)
    };
    let outlines: Vec<Vec<(f64, f64)>> = floors
        .iter()
        .map(|f| f.iter().map(&to_local).collect())
        .collect();
    let boundary = boundary_edges(&outlines);

    let (mut min_u, mut max_u, mut min_v, mut max_v) = (
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
    );
    for &(u, v) in outlines.iter().flatten() {
        min_u = min_u.min(u);
        max_u = max_u.max(u);
        min_v = min_v.min(v);
        max_v = max_v.max(v);
    }
    let n_u = ((max_u - min_u) / spacing).floor() as usize + 1;
    let n_v = ((max_v - min_v) / spacing).floor() as usize + 1;
    if n_u.saturating_mul(n_v) > MAX_CANDIDATE_POINTS {
        return Err(GridError::InvalidSettings(format!(
            "spacing {spacing} m gives more than {MAX_CANDIDATE_POINTS} candidate points"
        )));
    }
    let start_u = 0.5 * (min_u + max_u) - 0.5 * (n_u - 1) as f64 * spacing;
    let start_v = 0.5 * (min_v + max_v) - 0.5 * (n_v - 1) as f64 * spacing;

    // Small tolerance so points exactly at the offset distance are kept
    // regardless of rounding in the frame rotation.
    let min_dist = wall_offset - 1e-9;
    let mut points = Vec::new();
    for j in 0..n_v {
        let v = start_v + j as f64 * spacing;
        for i in 0..n_u {
            let u = start_u + i as f64 * spacing;
            let Some(floor_idx) = outlines
                .iter()
                .position(|poly| point_in_polygon_xy(u, v, poly))
            else {
                continue;
            };
            let clear = boundary
                .iter()
                .all(|&(a, b)| distance_to_segment((u, v), a, b) >= min_dist);
            if !clear {
                continue;
            }
            let x = origin.0 + u * axis.0 - v * axis.1;
            let y = origin.1 + u * axis.1 + v * axis.0;
            if let Some(z) = floor_z(floors[floor_idx], x, y) {
                points.push(Point3D::new(x, y, z + height));
            }
        }
    }
    if points.is_empty() {
        return Err(GridError::NoPoints);
    }
    Ok(points)
}

/// Origin and unit x-y direction of the grid's first axis.
fn grid_frame(floors: &[&[Point3D]]) -> ((f64, f64), (f64, f64)) {
    let largest = floors
        .iter()
        .max_by(|a, b| {
            newell_normal(a)
                .z
                .abs()
                .total_cmp(&newell_normal(b).z.abs())
        })
        .expect("floors is non-empty");
    // Each edge direction is folded into one half-plane, and ties between
    // equally long edges go to the smallest angle. With the origin at the
    // bounding-box corner, the frame (and so the point order) then does not
    // depend on where the vertex list starts or which way it winds.
    let n = largest.len();
    let edges: Vec<(f64, f64, f64)> = (0..n)
        .map(|i| {
            let (a, b) = (largest[i], largest[(i + 1) % n]);
            let (mut dx, mut dy) = (b.x - a.x, b.y - a.y);
            if dx < -1e-12 || (dx.abs() <= 1e-12 && dy < 0.0) {
                (dx, dy) = (-dx, -dy);
            }
            (dx.hypot(dy), dx, dy)
        })
        .collect();
    let longest = edges.iter().map(|e| e.0).fold(0.0, f64::max);
    let axis = edges
        .iter()
        .filter(|e| e.0 >= longest * (1.0 - 1e-9))
        .min_by(|a, b| a.2.atan2(a.1).total_cmp(&b.2.atan2(b.1)))
        .map(|&(len, dx, dy)| (dx / len, dy / len))
        .unwrap_or((1.0, 0.0));
    let min_x = largest.iter().map(|p| p.x).fold(f64::INFINITY, f64::min);
    let min_y = largest.iter().map(|p| p.y).fold(f64::INFINITY, f64::min);
    ((min_x, min_y), axis)
}

type Edge2 = ((f64, f64), (f64, f64));

/// Floor polygon edges that are not shared with another floor polygon of the
/// same zone. Shared edges are seams inside the room, not walls, so points
/// are not pushed away from them.
fn boundary_edges(outlines: &[Vec<(f64, f64)>]) -> Vec<Edge2> {
    let edges_of = |poly: &Vec<(f64, f64)>| -> Vec<Edge2> {
        (0..poly.len())
            .map(|i| (poly[i], poly[(i + 1) % poly.len()]))
            .collect()
    };
    let all: Vec<Vec<Edge2>> = outlines.iter().map(edges_of).collect();
    let same = |p: (f64, f64), q: (f64, f64)| (p.0 - q.0).abs() < 1e-6 && (p.1 - q.1).abs() < 1e-6;
    let mut out = Vec::new();
    for (pi, edges) in all.iter().enumerate() {
        for &(a, b) in edges {
            let shared = all.iter().enumerate().any(|(qi, other)| {
                qi != pi
                    && other
                        .iter()
                        .any(|&(c, d)| (same(a, d) && same(b, c)) || (same(a, c) && same(b, d)))
            });
            if !shared {
                out.push((a, b));
            }
        }
    }
    out
}

/// Height of a floor polygon's plane at (x, y), or `None` for a near-vertical
/// polygon that has no meaningful floor height.
fn floor_z(floor: &[Point3D], x: f64, y: f64) -> Option<f64> {
    let n = newell_normal(floor);
    if n.z.abs() < 1e-6 * n.magnitude() {
        return None;
    }
    let c = polygon_centroid(floor);
    Some(c.z - (n.x * (x - c.x) + n.y * (y - c.y)) / n.z)
}

fn point_in_polygon_xy(x: f64, y: f64, poly: &[(f64, f64)]) -> bool {
    let n = poly.len();
    let mut inside = false;
    let mut j = n - 1;
    for i in 0..n {
        let (xi, yi) = poly[i];
        let (xj, yj) = poly[j];
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

fn point_in_polygon(x: f64, y: f64, poly: &[Point2D]) -> bool {
    let xy: Vec<(f64, f64)> = poly.iter().map(|p| (p.x, p.y)).collect();
    point_in_polygon_xy(x, y, &xy)
}

fn distance_to_segment(p: (f64, f64), a: (f64, f64), b: (f64, f64)) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len2 = dx * dx + dy * dy;
    let t = if len2 < 1e-24 {
        0.0
    } else {
        (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / len2).clamp(0.0, 1.0)
    };
    (p.0 - (a.0 + t * dx)).hypot(p.1 - (a.1 + t * dy))
}

// ─── View factors ───────────────────────────────────────────────────────────

impl Aperture {
    fn new(surface: usize, vertices: &[Point3D]) -> Self {
        let normal = newell_normal(vertices).normalize();
        let (origin, u, v) = build_local_coords(vertices, &normal);
        let outline = vertices
            .iter()
            .map(|p| to_local_2d(p, &origin, &u, &v))
            .collect();
        Self {
            surface,
            origin,
            u,
            v,
            normal,
            outline,
        }
    }
}

/// Raw view factors from `p` to every surface (host walls net of their
/// windows), normalised to sum to 1, and the coverage before normalisation.
///
/// There is no back-face culling: it would need trustworthy winding, which
/// the repo models do not all have, and in a convex zone every surface faces
/// the point anyway.
fn view_factor_row(p: Point3D, surfaces: &[GridSurface], model: OccupantModel) -> (Vec<f64>, f64) {
    let mut row: Vec<f64> = surfaces
        .iter()
        .map(|s| occupant_view_factor(p, &s.vertices, model))
        .collect();
    for (i, s) in surfaces.iter().enumerate() {
        if let Some(h) = s.host {
            row[h] -= row[i];
        }
    }
    for f in &mut row {
        // A window drawn slightly outside its host would push the host below
        // zero; the overlap is not a real negative view.
        *f = f.max(0.0);
    }
    let coverage: f64 = row.iter().sum();
    if coverage > 1e-9 {
        for f in &mut row {
            *f /= coverage;
        }
    }
    (row, coverage)
}

fn occupant_view_factor(p: Point3D, poly: &[Point3D], model: OccupantModel) -> f64 {
    let Some((w_up_down, w_horizontal)) = model.plane_weights() else {
        return solid_angle(p, poly) / (4.0 * PI);
    };
    let up = plane_view_factor(p, poly, Vec3::new(0.0, 0.0, 1.0));
    let down = plane_view_factor(p, poly, Vec3::new(0.0, 0.0, -1.0));
    let horizontal: f64 = (0..N_AZIMUTHS)
        .map(|k| {
            let a = 2.0 * PI * k as f64 / N_AZIMUTHS as f64;
            plane_view_factor(p, poly, Vec3::new(a.cos(), a.sin(), 0.0))
        })
        .sum();
    // The four ISO directions' weight is spread evenly over the samples.
    w_up_down * (up + down) + w_horizontal * 4.0 / N_AZIMUTHS as f64 * horizontal
}

/// Solid angle subtended by a planar polygon at `p` [sr].
///
/// Van Oosterom & Strackee (1983) per fan triangle. The signed sum is correct
/// for non-convex polygons as well, because the triangles that fall outside
/// the polygon cancel.
fn solid_angle(p: Point3D, poly: &[Point3D]) -> f64 {
    let r0 = poly[0] - p;
    let mut total = 0.0;
    for i in 1..poly.len() - 1 {
        let (a, b, c) = (r0, poly[i] - p, poly[i + 1] - p);
        let (la, lb, lc) = (a.magnitude(), b.magnitude(), c.magnitude());
        let num = a.dot(&b.cross(&c));
        let den = la * lb * lc + a.dot(&b) * lc + a.dot(&c) * lb + b.dot(&c) * la;
        total += 2.0 * num.atan2(den);
    }
    total.abs()
}

/// View factor from a differential plane element at `p` facing `normal` to a
/// polygon, by Lambert's contour integral over the part of the polygon in
/// front of the element.
fn plane_view_factor(p: Point3D, poly: &[Point3D], normal: Vec3) -> f64 {
    let clipped = clip_to_front(p, poly, normal);
    let n = clipped.len();
    if n < 3 {
        return 0.0;
    }
    let mut sum = 0.0;
    for k in 0..n {
        let a = clipped[k] - p;
        let b = clipped[(k + 1) % n] - p;
        let c = a.cross(&b);
        let cm = c.magnitude();
        if cm < 1e-12 {
            continue;
        }
        let theta = cm.atan2(a.dot(&b));
        sum += theta * normal.dot(&c) / cm;
    }
    (sum / (2.0 * PI)).abs()
}

/// The part of a polygon on the front side of the plane through `p` with the
/// given normal (Sutherland–Hodgman against a single plane). The contour
/// integral is only valid for the visible half-space.
fn clip_to_front(p: Point3D, poly: &[Point3D], normal: Vec3) -> Vec<Point3D> {
    let dist = |q: &Point3D| (*q - p).dot(&normal);
    let n = poly.len();
    let mut out = Vec::with_capacity(n + 2);
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        let (da, db) = (dist(&a), dist(&b));
        if da >= 0.0 {
            out.push(a);
        }
        if (da >= 0.0) != (db >= 0.0) {
            let t = da / (da - db);
            out.push(Point3D::new(
                a.x + t * (b.x - a.x),
                a.y + t * (b.y - a.y),
                a.z + t * (b.z - a.z),
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_relative_eq;

    fn p(x: f64, y: f64, z: f64) -> Point3D {
        Point3D::new(x, y, z)
    }

    fn surface(vertices: Vec<Point3D>, surface_type: SurfaceType) -> GridSurface {
        GridSurface {
            vertices,
            surface_type,
            host: None,
        }
    }

    /// Box room `lx × ly × lz` with its corner at the origin. Order: floor,
    /// ceiling, south, north, east, west.
    fn box_room(lx: f64, ly: f64, lz: f64) -> Vec<GridSurface> {
        use SurfaceType::*;
        vec![
            surface(
                vec![p(0., 0., 0.), p(0., ly, 0.), p(lx, ly, 0.), p(lx, 0., 0.)],
                Floor,
            ),
            surface(
                vec![p(0., 0., lz), p(lx, 0., lz), p(lx, ly, lz), p(0., ly, lz)],
                Ceiling,
            ),
            surface(
                vec![p(0., 0., 0.), p(lx, 0., 0.), p(lx, 0., lz), p(0., 0., lz)],
                Wall,
            ),
            surface(
                vec![p(lx, ly, 0.), p(0., ly, 0.), p(0., ly, lz), p(lx, ly, lz)],
                Wall,
            ),
            surface(
                vec![p(lx, 0., 0.), p(lx, ly, 0.), p(lx, ly, lz), p(lx, 0., lz)],
                Wall,
            ),
            surface(
                vec![p(0., ly, 0.), p(0., 0., 0.), p(0., 0., lz), p(0., ly, lz)],
                Wall,
            ),
        ]
    }

    const SOUTH: usize = 2;

    /// Adds a window on the south wall spanning x ∈ [x0, x1], z ∈ [z0, z1].
    fn with_south_window(
        mut room: Vec<GridSurface>,
        x0: f64,
        x1: f64,
        z0: f64,
        z1: f64,
    ) -> Vec<GridSurface> {
        room.push(GridSurface {
            vertices: vec![p(x0, 0., z0), p(x1, 0., z0), p(x1, 0., z1), p(x0, 0., z1)],
            surface_type: SurfaceType::Window,
            host: Some(SOUTH),
        });
        room
    }

    fn settings(occupant: OccupantModel) -> GridSettings {
        GridSettings {
            occupant,
            ..GridSettings::default()
        }
    }

    const MODELS: [OccupantModel; 3] = [
        OccupantModel::Seated,
        OccupantModel::Standing,
        OccupantModel::Sphere,
    ];

    #[test]
    fn reversed_winding_gives_identical_results() {
        let room = with_south_window(box_room(6.0, 5.0, 3.0), 1.0, 5.0, 1.0, 2.0);
        let flipped: Vec<GridSurface> = room
            .iter()
            .map(|s| GridSurface {
                vertices: s.vertices.iter().rev().copied().collect(),
                ..s.clone()
            })
            .collect();
        let a = ZoneGrid::build(&room, &GridSettings::default()).unwrap();
        let b = ZoneGrid::build(&flipped, &GridSettings::default()).unwrap();
        assert_eq!(a.points, b.points);
        let alt = 45.0_f64.to_radians();
        let toward_sun = Vec3::new(0.0, -alt.cos(), alt.sin());
        let mut irr = vec![0.0; room.len()];
        irr[6] = 600.0;
        for i in 0..a.points.len() {
            for (fa, fb) in a.view_factors(i).iter().zip(b.view_factors(i)) {
                assert_relative_eq!(fa, fb, epsilon = 1e-12);
            }
            assert_eq!(
                a.beam_irradiance(i, toward_sun, &irr),
                b.beam_irradiance(i, toward_sun, &irr)
            );
        }
    }

    #[test]
    fn iso_weights_sum_to_one() {
        for model in [OccupantModel::Seated, OccupantModel::Standing] {
            let (ud, h) = model.plane_weights().unwrap();
            assert_relative_eq!(2.0 * ud + 4.0 * h, 1.0, epsilon = 1e-12);
        }
    }

    #[test]
    fn plane_view_factor_matches_parallel_square_formula() {
        // Element facing up at the centre below a 2a × 2a square at height c:
        // four times the corner formula of Hamilton & Morgan (1952).
        let (a, c) = (1.5, 2.0);
        let sq = vec![p(-a, -a, c), p(a, -a, c), p(a, a, c), p(-a, a, c)];
        let f = plane_view_factor(p(0., 0., 0.), &sq, Vec3::new(0., 0., 1.));
        let x = a / c;
        let s = (1.0 + x * x).sqrt();
        let corner = (2.0 * x / s * (x / s).atan()) / (2.0 * PI);
        assert_relative_eq!(f, 4.0 * corner, epsilon = 1e-12);
    }

    #[test]
    fn solid_angle_matches_rectangle_formula() {
        let (a, b, c) = (1.0, 2.0, 1.5);
        let rect = vec![p(-a, -b, c), p(a, -b, c), p(a, b, c), p(-a, b, c)];
        let omega = solid_angle(p(0., 0., 0.), &rect);
        let expected = 4.0 * (a * b / ((a * a + c * c) * (b * b + c * c)).sqrt()).asin();
        assert_relative_eq!(omega, expected, epsilon = 1e-12);
    }

    #[test]
    fn view_factors_sum_to_one_in_closed_room() {
        let room = with_south_window(box_room(6.0, 5.0, 3.0), 1.0, 5.0, 0.9, 2.4);
        for model in MODELS {
            let grid = ZoneGrid::build(&room, &settings(model)).unwrap();
            for i in 0..grid.points.len() {
                assert_relative_eq!(grid.coverage(i), 1.0, epsilon = 1e-9);
                let sum: f64 = grid.view_factors(i).iter().sum();
                assert_relative_eq!(sum, 1.0, epsilon = 1e-12);
            }
        }
    }

    #[test]
    fn uniform_surfaces_give_uniform_mrt() {
        let room = with_south_window(box_room(6.0, 5.0, 3.0), 1.0, 5.0, 0.9, 2.4);
        let temps = vec![23.4; room.len()];
        for model in MODELS {
            let grid = ZoneGrid::build(&room, &settings(model)).unwrap();
            for i in 0..grid.points.len() {
                assert_relative_eq!(grid.mrt(i, &temps), 23.4, epsilon = 1e-9);
            }
        }
    }

    #[test]
    fn symmetric_room_gives_mirrored_mrt() {
        // Window centred on the south wall: results mirror about x = 3.
        let room = with_south_window(box_room(6.0, 5.0, 3.0), 1.5, 4.5, 0.9, 2.4);
        let mut temps = vec![21.0; room.len()];
        temps[SOUTH] = 18.0;
        *temps.last_mut().unwrap() = 5.0;
        let grid = ZoneGrid::build(&room, &settings(OccupantModel::Seated)).unwrap();
        for i in 0..grid.points.len() {
            let a = grid.points[i];
            let j = grid
                .points
                .iter()
                .position(|b| (b.x - (6.0 - a.x)).abs() < 1e-9 && (b.y - a.y).abs() < 1e-9)
                .expect("grid is mirror-symmetric");
            assert_relative_eq!(grid.mrt(i, &temps), grid.mrt(j, &temps), epsilon = 1e-9);
        }
    }

    #[test]
    fn point_near_cold_window_has_lower_mrt_than_centre() {
        let room = with_south_window(box_room(6.0, 5.0, 3.0), 1.0, 5.0, 0.9, 2.4);
        let mut temps = vec![21.0; room.len()];
        *temps.last_mut().unwrap() = 2.0;
        let grid = ZoneGrid::build(&room, &settings(OccupantModel::Seated)).unwrap();
        let nearest = |x: f64, y: f64| {
            (0..grid.points.len())
                .min_by(|&i, &j| {
                    let d = |k: usize| (grid.points[k].x - x).hypot(grid.points[k].y - y);
                    d(i).total_cmp(&d(j))
                })
                .unwrap()
        };
        let by_window = grid.mrt(nearest(3.0, 0.5), &temps);
        let centre = grid.mrt(nearest(3.0, 2.5), &temps);
        let far = grid.mrt(nearest(3.0, 4.5), &temps);
        assert!(by_window < centre && centre < far);
        assert!(
            by_window < 21.0 - 1.0,
            "window should cool the MRT noticeably"
        );
    }

    #[test]
    fn grid_average_mrt_is_close_to_area_weighted_mrt() {
        // The zone-level output weights surfaces by area; the grid weights them
        // by view. For a compact room the two agree to within a few tenths.
        let room = with_south_window(box_room(5.0, 5.0, 3.0), 1.0, 4.0, 0.9, 2.4);
        let mut temps = vec![21.0; room.len()];
        *temps.last_mut().unwrap() = 5.0;
        temps[SOUTH] = 19.0;
        let areas: Vec<f64> = room
            .iter()
            .map(|s| crate::geometry::polygon_area(&s.vertices))
            .collect();
        let window_area = *areas.last().unwrap();
        let mut net = areas.clone();
        net[SOUTH] -= window_area;
        let zone_mrt =
            net.iter().zip(&temps).map(|(a, t)| a * t).sum::<f64>() / net.iter().sum::<f64>();

        let grid = ZoneGrid::build(&room, &settings(OccupantModel::Sphere)).unwrap();
        let mean = (0..grid.points.len())
            .map(|i| grid.mrt(i, &temps))
            .sum::<f64>()
            / grid.points.len() as f64;
        assert!(
            (mean - zone_mrt).abs() < 0.5,
            "grid mean {mean:.3} vs area-weighted {zone_mrt:.3}"
        );
    }

    #[test]
    fn hosted_window_splits_its_wall_view_factor() {
        let plain = box_room(6.0, 5.0, 3.0);
        let glazed = with_south_window(plain.clone(), 1.0, 5.0, 0.9, 2.4);
        let s = settings(OccupantModel::Seated);
        let a = ZoneGrid::build(&plain, &s).unwrap();
        let b = ZoneGrid::build(&glazed, &s).unwrap();
        assert_eq!(a.points, b.points);
        for i in 0..a.points.len() {
            let fb = b.view_factors(i);
            assert_relative_eq!(a.view_factors(i)[SOUTH], fb[SOUTH] + fb[6], epsilon = 1e-12);
            assert!(fb[6] > 0.0);
        }
    }

    #[test]
    fn open_enclosure_reports_coverage_below_one() {
        let mut room = box_room(6.0, 5.0, 3.0);
        room.remove(1); // ceiling
        let grid = ZoneGrid::build(&room, &settings(OccupantModel::Sphere)).unwrap();
        assert!(grid.min_coverage() < 0.9);
        for i in 0..grid.points.len() {
            let sum: f64 = grid.view_factors(i).iter().sum();
            assert_relative_eq!(sum, 1.0, epsilon = 1e-12);
        }
    }

    #[test]
    fn seated_weights_floor_and_ceiling_more_than_standing() {
        let room = box_room(6.0, 6.0, 3.0);
        let at = |model| {
            let s = GridSettings {
                height: Some(0.8),
                ..settings(model)
            };
            let g = ZoneGrid::build(&room, &s).unwrap();
            let f = g.view_factors(g.points.len() / 2);
            f[0] + f[1]
        };
        assert!(at(OccupantModel::Seated) > at(OccupantModel::Standing));
    }

    #[test]
    fn grid_layout_on_rectangle() {
        let room = box_room(4.0, 3.0, 3.0);
        let grid = ZoneGrid::build(&room, &GridSettings::default()).unwrap();
        // x ∈ {0.5..3.5} (7) × y ∈ {0.5..2.5} (5), all at seated height.
        assert_eq!(grid.points.len(), 35);
        for pt in &grid.points {
            assert!(pt.x >= 0.5 - 1e-9 && pt.x <= 3.5 + 1e-9);
            assert!(pt.y >= 0.5 - 1e-9 && pt.y <= 2.5 + 1e-9);
            assert_relative_eq!(pt.z, 0.6, epsilon = 1e-12);
        }
    }

    #[test]
    fn grid_skips_notch_of_l_shaped_floor() {
        // L shape: 6 × 6 with the north-east 3 × 3 quadrant removed. Vertices
        // run clockwise from above (counter-clockwise from below).
        let floor = surface(
            vec![
                p(0., 0., 0.),
                p(0., 6., 0.),
                p(3., 6., 0.),
                p(3., 3., 0.),
                p(6., 3., 0.),
                p(6., 0., 0.),
            ],
            SurfaceType::Floor,
        );
        let pts = generate_points(&[floor], 0.5, 0.5, 0.6).unwrap();
        assert!(!pts.is_empty());
        for pt in &pts {
            let in_notch = pt.x > 2.5 + 1e-9 && pt.y > 2.5 + 1e-9;
            assert!(!in_notch, "point {pt:?} is in or too close to the notch");
        }
    }

    #[test]
    fn seam_between_floor_polygons_is_not_offset() {
        let whole = surface(
            vec![p(0., 0., 0.), p(0., 3., 0.), p(4., 3., 0.), p(4., 0., 0.)],
            SurfaceType::Floor,
        );
        let west = surface(
            vec![p(0., 0., 0.), p(0., 3., 0.), p(2., 3., 0.), p(2., 0., 0.)],
            SurfaceType::Floor,
        );
        let east = surface(
            vec![p(2., 0., 0.), p(2., 3., 0.), p(4., 3., 0.), p(4., 0., 0.)],
            SurfaceType::Floor,
        );
        let one = generate_points(&[whole], 0.5, 0.5, 0.6).unwrap();
        let two = generate_points(&[west, east], 0.5, 0.5, 0.6).unwrap();
        assert_eq!(one.len(), two.len());
    }

    #[test]
    fn rotated_room_keeps_rows_parallel_to_walls() {
        let room = box_room(4.0, 3.0, 3.0);
        let (s, c) = 30.0_f64.to_radians().sin_cos();
        let rotated: Vec<GridSurface> = room
            .iter()
            .map(|surf| GridSurface {
                vertices: surf
                    .vertices
                    .iter()
                    .map(|v| p(v.x * c - v.y * s, v.x * s + v.y * c, v.z))
                    .collect(),
                ..surf.clone()
            })
            .collect();
        let a = ZoneGrid::build(&room, &GridSettings::default()).unwrap();
        let b = ZoneGrid::build(&rotated, &GridSettings::default()).unwrap();
        assert_eq!(a.points.len(), b.points.len());
    }

    #[test]
    fn sun_patch_follows_beam_geometry() {
        // Sun due south at 45°: a ray from (x, y, 0.6) reaches the wall y = 0
        // at z = 0.6 + y, so it passes the window (z ∈ [1, 2]) for y ∈ [0.4, 1.4].
        let room = with_south_window(box_room(6.0, 5.0, 3.0), 1.0, 5.0, 1.0, 2.0);
        let grid = ZoneGrid::build(&room, &settings(OccupantModel::Seated)).unwrap();
        let alt = 45.0_f64.to_radians();
        let toward_sun = Vec3::new(0.0, -alt.cos(), alt.sin());
        let mut irr = vec![0.0; room.len()];
        irr[6] = 600.0;
        let mut lit = 0;
        for (i, pt) in grid.points.iter().enumerate() {
            let e = grid.beam_irradiance(i, toward_sun, &irr);
            let z_hit = 0.6 + pt.y;
            let inside = (1.0..=5.0).contains(&pt.x) && (1.0..=2.0).contains(&z_hit);
            let margin = (pt.x - 1.0)
                .abs()
                .min((pt.x - 5.0).abs())
                .min((z_hit - 1.0).abs())
                .min((z_hit - 2.0).abs());
            if margin < 1e-6 {
                continue;
            }
            assert_eq!(e > 0.0, inside, "point {pt:?}");
            if inside {
                assert_eq!(e, 600.0);
                lit += 1;
            }
        }
        assert!(lit > 0);

        let from_north = Vec3::new(0.0, alt.cos(), alt.sin());
        for i in 0..grid.points.len() {
            assert_eq!(grid.beam_irradiance(i, from_north, &irr), 0.0);
        }
    }

    #[test]
    fn window_beam_irradiance_undoes_projection() {
        assert_relative_eq!(window_beam_irradiance(300.0, 2.0, 0.5), 300.0);
        assert_eq!(window_beam_irradiance(300.0, 2.0, 0.0), 0.0);
        assert_eq!(window_beam_irradiance(0.0, 2.0, 0.5), 0.0);
    }

    #[test]
    fn point_comfort_matches_zone_correlations() {
        let cfg = ZoneComfortConfig::default();
        let air = ZoneAirState {
            temperature: 22.0,
            relative_humidity: 0.45,
        };
        let shade = point_comfort(20.0, 0.0, 0.6, air, &cfg);
        assert_eq!(shade.solar_delta_mrt, 0.0);
        assert_relative_eq!(shade.operative_temperature, 21.0);
        let sun = point_comfort(20.0, 500.0, 0.6, air, &cfg);
        assert!(sun.solar_delta_mrt > 1.0);
        assert!(sun.pmv > shade.pmv);
        assert_relative_eq!(sun.ppd, comfort::ppd(sun.pmv));
    }

    #[test]
    fn pmv_band_agrees_with_direct_pmv() {
        let cfg = ZoneComfortConfig::default();
        for t_air in [16.0, 22.0, 28.0] {
            let air = ZoneAirState {
                temperature: t_air,
                relative_humidity: 0.5,
            };
            let band = PmvBand::new(air, &cfg, 0.5);
            assert!(band.cold_below < band.hot_above);
            let mut mrt = 0.0;
            while mrt <= 50.0 {
                let pmv = comfort::pmv(
                    t_air,
                    mrt,
                    cfg.air_velocity,
                    0.5,
                    cfg.metabolic_rate,
                    cfg.clothing,
                );
                let near_edge = (pmv.abs() - 0.5).abs() < 1e-6;
                if !near_edge {
                    let expected = if pmv < -0.5 {
                        Sensation::Cold
                    } else if pmv > 0.5 {
                        Sensation::Hot
                    } else {
                        Sensation::Comfortable
                    };
                    assert_eq!(band.classify(mrt), expected, "t_air {t_air}, mrt {mrt}");
                }
                mrt += 0.25;
            }
        }
    }

    #[test]
    fn rejects_bad_input() {
        let room = box_room(4.0, 3.0, 3.0);
        let bad = GridSettings {
            spacing: 0.0,
            ..GridSettings::default()
        };
        assert!(matches!(
            ZoneGrid::build(&room, &bad),
            Err(GridError::InvalidSettings(_))
        ));
        let walls_only: Vec<GridSurface> = room[2..].to_vec();
        assert_eq!(
            ZoneGrid::build(&walls_only, &GridSettings::default()).unwrap_err(),
            GridError::NoFloor
        );
        let tiny = box_room(0.8, 0.8, 3.0);
        assert_eq!(
            ZoneGrid::build(&tiny, &GridSettings::default()).unwrap_err(),
            GridError::NoPoints
        );
        let mut bad_host = with_south_window(room, 1.0, 3.0, 1.0, 2.0);
        bad_host[6].host = Some(6);
        assert_eq!(
            ZoneGrid::build(&bad_host, &GridSettings::default()).unwrap_err(),
            GridError::InvalidHost { window: 6, host: 6 }
        );
    }
}
