//! Vertex-based geometry calculations for building surfaces.
//!
//! Coordinate system: right-hand, Z-up, Y-north, X-east.
//! Vertex order: counter-clockwise from outside (Newell convention).
//!
//! Provides:
//! - `Point3D`: 3D point with serde support for YAML vertices
//! - `Vec3`: internal vector math type
//! - Newell's method for polygon normals and areas
//! - Azimuth and tilt from surface normal
//! - Zone volume via divergence theorem
//! - Zone floor area from floor polygon areas

use serde::{Deserialize, Serialize};

/// A 3D point in building coordinates (X=East, Y=North, Z=Up).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Point3D {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl Point3D {
    pub fn new(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }
}

/// 3D vector for internal math operations.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vec3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl Vec3 {
    pub fn new(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }

    pub fn magnitude(&self) -> f64 {
        (self.x * self.x + self.y * self.y + self.z * self.z).sqrt()
    }

    pub fn dot(&self, other: &Vec3) -> f64 {
        self.x * other.x + self.y * other.y + self.z * other.z
    }

    /// Cross product: self × other.
    pub fn cross(&self, other: &Vec3) -> Vec3 {
        Vec3 {
            x: self.y * other.z - self.z * other.y,
            y: self.z * other.x - self.x * other.z,
            z: self.x * other.y - self.y * other.x,
        }
    }

    /// Scalar multiplication.
    pub fn scale(&self, s: f64) -> Vec3 {
        Vec3 { x: self.x * s, y: self.y * s, z: self.z * s }
    }

    pub fn normalize(&self) -> Vec3 {
        let m = self.magnitude();
        if m < 1.0e-15 {
            Vec3::new(0.0, 0.0, 1.0) // degenerate → up
        } else {
            Vec3::new(self.x / m, self.y / m, self.z / m)
        }
    }
}

impl std::ops::Sub for Vec3 {
    type Output = Vec3;
    fn sub(self, rhs: Vec3) -> Vec3 {
        Vec3::new(self.x - rhs.x, self.y - rhs.y, self.z - rhs.z)
    }
}

impl std::ops::Add for Vec3 {
    type Output = Vec3;
    fn add(self, rhs: Vec3) -> Vec3 {
        Vec3::new(self.x + rhs.x, self.y + rhs.y, self.z + rhs.z)
    }
}

impl std::ops::Sub for Point3D {
    type Output = Vec3;
    fn sub(self, rhs: Point3D) -> Vec3 {
        Vec3::new(self.x - rhs.x, self.y - rhs.y, self.z - rhs.z)
    }
}

/// Compute the outward normal of a polygon using Newell's method.
///
/// Vertices must be ordered counter-clockwise when viewed from outside.
/// The magnitude of the returned vector equals twice the polygon area.
///
/// Reference: Newell (1972), adapted from EnergyPlus SurfaceGeometry.cc.
pub fn newell_normal(vertices: &[Point3D]) -> Vec3 {
    let n = vertices.len();
    if n < 3 {
        return Vec3::new(0.0, 0.0, 0.0);
    }

    let mut nx = 0.0;
    let mut ny = 0.0;
    let mut nz = 0.0;

    for i in 0..n {
        let j = (i + 1) % n;
        let vi = &vertices[i];
        let vj = &vertices[j];
        nx += (vi.y - vj.y) * (vi.z + vj.z);
        ny += (vi.z - vj.z) * (vi.x + vj.x);
        nz += (vi.x - vj.x) * (vi.y + vj.y);
    }

    Vec3::new(nx, ny, nz)
}

/// Compute polygon area from vertices [m²].
///
/// Uses the magnitude of the Newell normal divided by 2.
pub fn polygon_area(vertices: &[Point3D]) -> f64 {
    newell_normal(vertices).magnitude() / 2.0
}

/// Compute azimuth angle from a surface normal [degrees].
///
/// Azimuth = degrees from north, clockwise: 0°=North, 90°=East, 180°=South, 270°=West.
/// For the normal vector (Nx, Ny, Nz):
///   azimuth = atan2(Nx, Ny)  mapped to [0, 360)
///
/// Note: Ny is the north component, Nx is the east component.
pub fn azimuth_from_normal(normal: &Vec3) -> f64 {
    let n = normal.normalize();
    // Project onto XY plane
    let mut az = n.x.atan2(n.y).to_degrees();
    if az < 0.0 {
        az += 360.0;
    }
    az
}

/// Compute tilt angle from a surface normal [degrees].
///
/// Tilt = angle from horizontal (Z-up):
///   0° = face-up (horizontal roof/floor facing sky)
///   90° = vertical wall
///   180° = face-down (floor facing ground)
///
/// tilt = acos(Nz / |N|)
pub fn tilt_from_normal(normal: &Vec3) -> f64 {
    let n = normal.normalize();
    n.z.clamp(-1.0, 1.0).acos().to_degrees()
}

/// Compute the centroid of a polygon.
pub fn polygon_centroid(vertices: &[Point3D]) -> Point3D {
    let n = vertices.len() as f64;
    if n < 1.0 {
        return Point3D::new(0.0, 0.0, 0.0);
    }
    let sx: f64 = vertices.iter().map(|v| v.x).sum();
    let sy: f64 = vertices.iter().map(|v| v.y).sum();
    let sz: f64 = vertices.iter().map(|v| v.z).sum();
    Point3D::new(sx / n, sy / n, sz / n)
}

/// Compute zone volume from a closed set of surface polygons using the divergence theorem.
///
/// V = (1/3) * |Σ (N_hat · P) * A|  for each face
///
/// where N_hat is the outward unit normal, P is any vertex of the face,
/// and A is the face area.
///
/// Requires surfaces to form a closed volume with outward-pointing normals.
pub fn zone_volume_from_surfaces(surface_verts: &[&[Point3D]]) -> f64 {
    let mut volume = 0.0;
    for verts in surface_verts {
        if verts.len() < 3 {
            continue;
        }
        let normal = newell_normal(verts);
        let n_hat = normal.normalize();
        let area = normal.magnitude() / 2.0;

        // Use centroid as the reference point for better numerical stability
        let centroid = polygon_centroid(verts);
        let p = Vec3::new(centroid.x, centroid.y, centroid.z);
        volume += n_hat.dot(&p) * area;
    }
    (volume / 3.0).abs()
}

/// Sum the areas of floor surface polygons.
///
/// Computes the area of each polygon using Newell's method (correct for
/// arbitrary planar polygons, including non-convex shapes).  The caller
/// is expected to pre-filter to floor-type surfaces; this function sums
/// all provided polygon areas regardless of normal direction, so both
/// upward-facing (tilt ≈ 0°) and downward-facing (tilt ≈ 180°) vertex
/// winding orders are handled correctly.
pub fn zone_floor_area(all_surface_verts: &[&[Point3D]]) -> f64 {
    let mut total_area = 0.0;
    for verts in all_surface_verts {
        if verts.len() < 3 {
            continue;
        }
        total_area += polygon_area(verts);
    }
    total_area
}

// ────────────────────────────────────────────────────────────────────────────
// Cardinal direction classification and wall/window area utilities
// ────────────────────────────────────────────────────────────────────────────

/// Cardinal direction for surface orientation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CardinalDirection {
    North,
    East,
    South,
    West,
}

impl std::fmt::Display for CardinalDirection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CardinalDirection::North => write!(f, "North"),
            CardinalDirection::East => write!(f, "East"),
            CardinalDirection::South => write!(f, "South"),
            CardinalDirection::West => write!(f, "West"),
        }
    }
}

/// Classify an azimuth angle (degrees from north, clockwise) into a cardinal direction.
///
/// Uses 90° sectors centered on each cardinal:
///   - North: 315°–360° and 0°–45°
///   - East:  45°–135°
///   - South: 135°–225°
///   - West:  225°–315°
pub fn azimuth_to_cardinal(azimuth: f64) -> CardinalDirection {
    let az = ((azimuth % 360.0) + 360.0) % 360.0; // normalize to [0, 360)
    if az >= 315.0 || az < 45.0 {
        CardinalDirection::North
    } else if az < 135.0 {
        CardinalDirection::East
    } else if az < 225.0 {
        CardinalDirection::South
    } else {
        CardinalDirection::West
    }
}

/// Wall and window areas by cardinal direction.
#[derive(Debug, Clone, Default)]
pub struct EnvelopeAreas {
    /// Gross wall area [m²] by direction (N, E, S, W)
    pub wall_area: [f64; 4],
    /// Window area [m²] by direction (N, E, S, W)
    pub window_area: [f64; 4],
}

impl EnvelopeAreas {
    /// Index for a cardinal direction in the arrays.
    fn dir_index(dir: CardinalDirection) -> usize {
        match dir {
            CardinalDirection::North => 0,
            CardinalDirection::East  => 1,
            CardinalDirection::South => 2,
            CardinalDirection::West  => 3,
        }
    }

    /// Total gross wall area (all orientations).
    pub fn total_wall_area(&self) -> f64 {
        self.wall_area.iter().sum()
    }

    /// Total window area (all orientations).
    pub fn total_window_area(&self) -> f64 {
        self.window_area.iter().sum()
    }

    /// Window-to-wall ratio for a specific direction.
    /// Returns 0.0 if there is no wall area in that direction.
    pub fn wwr(&self, dir: CardinalDirection) -> f64 {
        let i = Self::dir_index(dir);
        if self.wall_area[i] > 0.0 {
            self.window_area[i] / self.wall_area[i]
        } else {
            0.0
        }
    }

    /// Overall window-to-wall ratio (all directions combined).
    pub fn total_wwr(&self) -> f64 {
        let tw = self.total_wall_area();
        if tw > 0.0 {
            self.total_window_area() / tw
        } else {
            0.0
        }
    }

    /// Add a wall area for the given direction.
    pub fn add_wall(&mut self, dir: CardinalDirection, area: f64) {
        self.wall_area[Self::dir_index(dir)] += area;
    }

    /// Add a window area for the given direction.
    pub fn add_window(&mut self, dir: CardinalDirection, area: f64) {
        self.window_area[Self::dir_index(dir)] += area;
    }
}

// ────────────────────────────────────────────────────────────────────────────
// Interior view factors for rectangular box zones
// ────────────────────────────────────────────────────────────────────────────

/// Box face indices for interior view factor model.
pub const FACE_FLOOR: usize = 0;
pub const FACE_CEILING: usize = 1;
pub const FACE_SOUTH: usize = 2;
pub const FACE_NORTH: usize = 3;
pub const FACE_EAST: usize = 4;
pub const FACE_WEST: usize = 5;

/// View factor between two identical aligned parallel rectangles.
///
/// Each rectangle has dimensions `a × b`, separated by distance `c`.
/// Uses Hottel's analytical formula (Incropera, Table 13.1, Configuration 1).
///
/// # Returns
/// F₁₂ ∈ [0, 1]: fraction of radiation leaving one rectangle that reaches the other.
pub fn vf_parallel_rectangles(a: f64, b: f64, c: f64) -> f64 {
    if a <= 0.0 || b <= 0.0 || c <= 0.0 {
        return 0.0;
    }
    let x = a / c;
    let y = b / c;
    let x2 = x * x;
    let y2 = y * y;
    let pi = std::f64::consts::PI;

    let term1 = ((1.0 + x2) * (1.0 + y2) / (1.0 + x2 + y2)).sqrt().ln();
    let term2 = x * (1.0 + y2).sqrt() * (x / (1.0 + y2).sqrt()).atan();
    let term3 = y * (1.0 + x2).sqrt() * (y / (1.0 + x2).sqrt()).atan();
    let term4 = x * x.atan();
    let term5 = y * y.atan();

    let f = (2.0 / (pi * x * y)) * (term1 + term2 + term3 - term4 - term5);
    f.clamp(0.0, 1.0)
}

/// View factor from one perpendicular rectangle to another, sharing a common edge.
///
/// Rectangle 1 has dimensions `common × depth` (the "from" surface).
/// Rectangle 2 has dimensions `common × height` (the "to" surface).
/// They share the edge of length `common` and are at 90° to each other.
///
/// Uses the analytical formula from Incropera, Table 13.1, Configuration 2.
///
/// # Arguments
/// * `common` — Length of shared edge [m]
/// * `depth` — Dimension of Rectangle 1 perpendicular to shared edge [m]
/// * `height` — Dimension of Rectangle 2 perpendicular to shared edge [m]
///
/// # Returns
/// F₁→₂ ∈ [0, 1]: fraction of radiation leaving Rectangle 1 that reaches Rectangle 2.
pub fn vf_perpendicular_rectangles(common: f64, depth: f64, height: f64) -> f64 {
    if common <= 0.0 || depth <= 0.0 || height <= 0.0 {
        return 0.0;
    }
    let h = depth / common;
    let w = height / common;
    let h2 = h * h;
    let w2 = w * w;
    let pi = std::f64::consts::PI;

    let term1 = h * (1.0 / h).atan();
    let term2 = w * (1.0 / w).atan();
    let term3 = (h2 + w2).sqrt() * (1.0 / (h2 + w2).sqrt()).atan();

    // Logarithmic term: ln[A × B^h² × C^w²]
    // where A = (1+h²)(1+w²)/(1+h²+w²)
    //       B = h²(1+h²+w²)/((1+h²)(h²+w²))
    //       C = w²(1+h²+w²)/((1+w²)(h²+w²))
    let a_val = (1.0 + h2) * (1.0 + w2) / (1.0 + h2 + w2);
    let b_val = h2 * (1.0 + h2 + w2) / ((1.0 + h2) * (h2 + w2));
    let c_val = w2 * (1.0 + h2 + w2) / ((1.0 + w2) * (h2 + w2));
    let log_term = 0.25 * (a_val.ln() + h2 * b_val.ln() + w2 * c_val.ln());

    let f = (1.0 / (pi * h)) * (term1 + term2 - term3 + log_term);
    f.clamp(0.0, 1.0)
}

/// Classify a surface into one of 6 box faces based on tilt and azimuth.
///
/// Returns face index: 0=floor, 1=ceiling, 2=south, 3=north, 4=east, 5=west.
/// Returns `None` if the surface geometry doesn't clearly map to a box face.
pub fn classify_box_face(tilt: f64, azimuth: f64) -> Option<usize> {
    if tilt > 150.0 {
        Some(FACE_FLOOR)
    } else if tilt < 30.0 {
        Some(FACE_CEILING)
    } else {
        // Vertical (or near-vertical) wall: classify by azimuth
        let az = ((azimuth % 360.0) + 360.0) % 360.0;
        if az >= 135.0 && az < 225.0 {
            Some(FACE_SOUTH)
        } else if az >= 315.0 || az < 45.0 {
            Some(FACE_NORTH)
        } else if az >= 45.0 && az < 135.0 {
            Some(FACE_EAST)
        } else {
            Some(FACE_WEST)
        }
    }
}

/// Compute the 6×6 face-to-face view factor matrix for a rectangular box zone.
///
/// # Arguments
/// * `length` — Box X-dimension (east-west extent) [m]
/// * `width` — Box Y-dimension (north-south extent) [m]
/// * `height` — Box Z-dimension (vertical) [m]
///
/// # Returns
/// 6×6 matrix where `vf[i][j]` = view factor from face i to face j.
/// Indices: 0=floor, 1=ceiling, 2=south, 3=north, 4=east, 5=west.
///
/// Row sums are 1.0 (enclosure) and reciprocity A_i·F_ij = A_j·F_ji holds.
pub fn compute_box_view_factors(length: f64, width: f64, height: f64) -> [[f64; 6]; 6] {
    let mut vf = [[0.0_f64; 6]; 6];

    // ── Parallel face pairs (opposite faces) ──────────────────────────
    let f_fc = vf_parallel_rectangles(length, width, height);   // floor↔ceiling
    let f_sn = vf_parallel_rectangles(length, height, width);   // south↔north
    let f_ew = vf_parallel_rectangles(width, height, length);   // east↔west

    vf[FACE_FLOOR][FACE_CEILING] = f_fc;
    vf[FACE_CEILING][FACE_FLOOR] = f_fc;
    vf[FACE_SOUTH][FACE_NORTH] = f_sn;
    vf[FACE_NORTH][FACE_SOUTH] = f_sn;
    vf[FACE_EAST][FACE_WEST] = f_ew;
    vf[FACE_WEST][FACE_EAST] = f_ew;

    // ── Adjacent face pairs (sharing an edge) ─────────────────────────
    //
    // vf_perpendicular_rectangles(common, depth, height):
    //   F from rect (common × depth) to rect (common × height)

    // Floor/Ceiling (L×W) ↔ South/North (L×H): common edge = L
    let f_floor_south = vf_perpendicular_rectangles(length, width, height);
    let f_south_floor = vf_perpendicular_rectangles(length, height, width);

    vf[FACE_FLOOR][FACE_SOUTH] = f_floor_south;
    vf[FACE_FLOOR][FACE_NORTH] = f_floor_south;
    vf[FACE_CEILING][FACE_SOUTH] = f_floor_south;
    vf[FACE_CEILING][FACE_NORTH] = f_floor_south;

    vf[FACE_SOUTH][FACE_FLOOR] = f_south_floor;
    vf[FACE_SOUTH][FACE_CEILING] = f_south_floor;
    vf[FACE_NORTH][FACE_FLOOR] = f_south_floor;
    vf[FACE_NORTH][FACE_CEILING] = f_south_floor;

    // Floor/Ceiling (L×W) ↔ East/West (W×H): common edge = W
    let f_floor_east = vf_perpendicular_rectangles(width, length, height);
    let f_east_floor = vf_perpendicular_rectangles(width, height, length);

    vf[FACE_FLOOR][FACE_EAST] = f_floor_east;
    vf[FACE_FLOOR][FACE_WEST] = f_floor_east;
    vf[FACE_CEILING][FACE_EAST] = f_floor_east;
    vf[FACE_CEILING][FACE_WEST] = f_floor_east;

    vf[FACE_EAST][FACE_FLOOR] = f_east_floor;
    vf[FACE_EAST][FACE_CEILING] = f_east_floor;
    vf[FACE_WEST][FACE_FLOOR] = f_east_floor;
    vf[FACE_WEST][FACE_CEILING] = f_east_floor;

    // South/North (L×H) ↔ East/West (W×H): common edge = H
    let f_south_east = vf_perpendicular_rectangles(height, length, width);
    let f_east_south = vf_perpendicular_rectangles(height, width, length);

    vf[FACE_SOUTH][FACE_EAST] = f_south_east;
    vf[FACE_SOUTH][FACE_WEST] = f_south_east;
    vf[FACE_NORTH][FACE_EAST] = f_south_east;
    vf[FACE_NORTH][FACE_WEST] = f_south_east;

    vf[FACE_EAST][FACE_SOUTH] = f_east_south;
    vf[FACE_EAST][FACE_NORTH] = f_east_south;
    vf[FACE_WEST][FACE_SOUTH] = f_east_south;
    vf[FACE_WEST][FACE_NORTH] = f_east_south;

    vf
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_relative_eq;

    // Helper: 8×6 rectangle in XY plane at z=0, CCW from above → normal = (0,0,-1) = floor
    fn floor_rect_8x6() -> Vec<Point3D> {
        vec![
            Point3D::new(0.0, 0.0, 0.0),
            Point3D::new(8.0, 0.0, 0.0),
            Point3D::new(8.0, 6.0, 0.0),
            Point3D::new(0.0, 6.0, 0.0),
        ]
    }

    // Helper: 8×6 rectangle in XY plane at z=2.7
    // For a roof, the outward normal points UP (+Z), so tilt = 0°.
    // "CCW from outside" for a roof means CCW when viewed from above.
    fn roof_rect_8x6() -> Vec<Point3D> {
        vec![
            Point3D::new(0.0, 0.0, 2.7),
            Point3D::new(8.0, 0.0, 2.7),
            Point3D::new(8.0, 6.0, 2.7),
            Point3D::new(0.0, 6.0, 2.7),
        ]
    }

    #[test]
    fn test_rectangle_area() {
        let verts = floor_rect_8x6();
        let area = polygon_area(&verts);
        assert_relative_eq!(area, 48.0, max_relative = 0.001);
    }

    #[test]
    fn test_triangle_area() {
        let verts = vec![
            Point3D::new(0.0, 0.0, 0.0),
            Point3D::new(4.0, 0.0, 0.0),
            Point3D::new(0.0, 3.0, 0.0),
        ];
        let area = polygon_area(&verts);
        assert_relative_eq!(area, 6.0, max_relative = 0.001);
    }

    #[test]
    fn test_newell_normal_horizontal_floor() {
        // Floor: CCW from outside (below) → normal pointing down (-Z)
        let verts = floor_rect_8x6();
        let n = newell_normal(&verts);
        // CCW from above makes normal +Z, but floor viewed from outside
        // (underneath) would be CW from above → -Z.
        // Actually: floor_rect_8x6 is CCW from above → normal = +Z (ceiling)
        // For a floor, vertices should be CW from above = CCW from below.
        assert!(n.z.abs() > 0.0);
    }

    #[test]
    fn test_south_wall_normal() {
        // South wall: face outside = face south → normal = (0, -1, 0)
        // CCW from outside (looking north at the wall from the south):
        // bottom-left(0,0,0), bottom-right(8,0,0), top-right(8,0,2.7), top-left(0,0,2.7)
        let verts = vec![
            Point3D::new(0.0, 0.0, 0.0),
            Point3D::new(8.0, 0.0, 0.0),
            Point3D::new(8.0, 0.0, 2.7),
            Point3D::new(0.0, 0.0, 2.7),
        ];
        let n = newell_normal(&verts).normalize();
        assert_relative_eq!(n.x, 0.0, epsilon = 0.001);
        assert!(n.y < -0.99); // pointing south (negative Y)
        assert_relative_eq!(n.z, 0.0, epsilon = 0.001);
    }

    #[test]
    fn test_south_wall_azimuth_tilt() {
        // South-facing wall: azimuth = 180°, tilt = 90°
        let verts = vec![
            Point3D::new(0.0, 0.0, 0.0),
            Point3D::new(8.0, 0.0, 0.0),
            Point3D::new(8.0, 0.0, 2.7),
            Point3D::new(0.0, 0.0, 2.7),
        ];
        let n = newell_normal(&verts);
        let az = azimuth_from_normal(&n);
        let tilt = tilt_from_normal(&n);
        assert_relative_eq!(az, 180.0, max_relative = 0.01);
        assert_relative_eq!(tilt, 90.0, max_relative = 0.01);
    }

    #[test]
    fn test_north_wall_azimuth() {
        // North-facing wall: normal = (0, +1, 0), azimuth = 0°
        // CCW from outside (looking south at the wall from the north):
        let verts = vec![
            Point3D::new(8.0, 6.0, 0.0),
            Point3D::new(0.0, 6.0, 0.0),
            Point3D::new(0.0, 6.0, 2.7),
            Point3D::new(8.0, 6.0, 2.7),
        ];
        let n = newell_normal(&verts);
        let az = azimuth_from_normal(&n);
        assert_relative_eq!(az, 0.0, epsilon = 0.1);
    }

    #[test]
    fn test_east_wall_azimuth() {
        // East-facing wall: normal = (+1, 0, 0), azimuth = 90°
        // CCW from outside (looking west at the wall from the east):
        let verts = vec![
            Point3D::new(8.0, 0.0, 0.0),
            Point3D::new(8.0, 6.0, 0.0),
            Point3D::new(8.0, 6.0, 2.7),
            Point3D::new(8.0, 0.0, 2.7),
        ];
        let n = newell_normal(&verts);
        let az = azimuth_from_normal(&n);
        assert_relative_eq!(az, 90.0, max_relative = 0.01);
    }

    #[test]
    fn test_west_wall_azimuth() {
        // West-facing wall: normal = (-1, 0, 0), azimuth = 270°
        let verts = vec![
            Point3D::new(0.0, 6.0, 0.0),
            Point3D::new(0.0, 0.0, 0.0),
            Point3D::new(0.0, 0.0, 2.7),
            Point3D::new(0.0, 6.0, 2.7),
        ];
        let n = newell_normal(&verts);
        let az = azimuth_from_normal(&n);
        assert_relative_eq!(az, 270.0, max_relative = 0.01);
    }

    #[test]
    fn test_roof_tilt_zero() {
        // Roof: normal points up (+Z), tilt = 0°
        let verts = roof_rect_8x6();
        let n = newell_normal(&verts);
        let tilt = tilt_from_normal(&n);
        assert_relative_eq!(tilt, 0.0, epsilon = 0.1);
    }

    #[test]
    fn test_floor_tilt_180() {
        // Floor: normal points down (-Z), tilt = 180°
        // CCW from outside (below): viewed from below, CW from above
        let verts = vec![
            Point3D::new(0.0, 6.0, 0.0),
            Point3D::new(8.0, 6.0, 0.0),
            Point3D::new(8.0, 0.0, 0.0),
            Point3D::new(0.0, 0.0, 0.0),
        ];
        let n = newell_normal(&verts);
        let tilt = tilt_from_normal(&n);
        assert_relative_eq!(tilt, 180.0, epsilon = 0.1);
    }

    #[test]
    fn test_south_wall_area() {
        // 8m wide × 2.7m tall = 21.6 m²
        let verts = vec![
            Point3D::new(0.0, 0.0, 0.0),
            Point3D::new(8.0, 0.0, 0.0),
            Point3D::new(8.0, 0.0, 2.7),
            Point3D::new(0.0, 0.0, 2.7),
        ];
        let area = polygon_area(&verts);
        assert_relative_eq!(area, 21.6, max_relative = 0.001);
    }

    #[test]
    fn test_box_volume_8x6x2_7() {
        // ASHRAE 140 Case 600: 8m × 6m × 2.7m = 129.6 m³
        //
        // All surfaces with outward-pointing normals (CCW from outside):

        // Floor: normal down (-Z). CCW from below = CW from above.
        let floor = vec![
            Point3D::new(0.0, 6.0, 0.0),
            Point3D::new(8.0, 6.0, 0.0),
            Point3D::new(8.0, 0.0, 0.0),
            Point3D::new(0.0, 0.0, 0.0),
        ];

        // Roof: normal up (+Z). CCW from above.
        let roof = vec![
            Point3D::new(0.0, 0.0, 2.7),
            Point3D::new(8.0, 0.0, 2.7),
            Point3D::new(8.0, 6.0, 2.7),
            Point3D::new(0.0, 6.0, 2.7),
        ];

        // South wall: normal south (-Y). CCW from south.
        let south = vec![
            Point3D::new(0.0, 0.0, 0.0),
            Point3D::new(8.0, 0.0, 0.0),
            Point3D::new(8.0, 0.0, 2.7),
            Point3D::new(0.0, 0.0, 2.7),
        ];

        // North wall: normal north (+Y). CCW from north.
        let north = vec![
            Point3D::new(8.0, 6.0, 0.0),
            Point3D::new(0.0, 6.0, 0.0),
            Point3D::new(0.0, 6.0, 2.7),
            Point3D::new(8.0, 6.0, 2.7),
        ];

        // East wall: normal east (+X). CCW from east.
        let east = vec![
            Point3D::new(8.0, 0.0, 0.0),
            Point3D::new(8.0, 6.0, 0.0),
            Point3D::new(8.0, 6.0, 2.7),
            Point3D::new(8.0, 0.0, 2.7),
        ];

        // West wall: normal west (-X). CCW from west.
        let west = vec![
            Point3D::new(0.0, 6.0, 0.0),
            Point3D::new(0.0, 0.0, 0.0),
            Point3D::new(0.0, 0.0, 2.7),
            Point3D::new(0.0, 6.0, 2.7),
        ];

        let surfaces: Vec<&[Point3D]> = vec![
            &floor[..], &roof[..], &south[..], &north[..], &east[..], &west[..],
        ];
        let vol = zone_volume_from_surfaces(&surfaces);
        assert_relative_eq!(vol, 129.6, max_relative = 0.01);
    }

    #[test]
    fn test_zone_floor_area() {
        // Floor polygon (downward-facing normal — CW winding from above)
        let floor_down = vec![
            Point3D::new(0.0, 6.0, 0.0),
            Point3D::new(8.0, 6.0, 0.0),
            Point3D::new(8.0, 0.0, 0.0),
            Point3D::new(0.0, 0.0, 0.0),
        ];
        // Floor polygon (upward-facing normal — CCW winding from above)
        // Both winding orders should produce correct area.
        let floor_up = vec![
            Point3D::new(0.0, 0.0, 0.0),
            Point3D::new(8.0, 0.0, 0.0),
            Point3D::new(8.0, 6.0, 0.0),
            Point3D::new(0.0, 6.0, 0.0),
        ];

        // Single floor surface
        let surfaces: Vec<&[Point3D]> = vec![&floor_down[..]];
        let area = zone_floor_area(&surfaces);
        assert_relative_eq!(area, 48.0, max_relative = 0.001);

        // Floor with opposite winding order — same area
        let surfaces: Vec<&[Point3D]> = vec![&floor_up[..]];
        let area = zone_floor_area(&surfaces);
        assert_relative_eq!(area, 48.0, max_relative = 0.001);

        // Multiple floor surfaces (e.g., L-shaped zone with two floor polygons)
        let floor_2 = vec![
            Point3D::new(8.0, 0.0, 0.0),
            Point3D::new(12.0, 0.0, 0.0),
            Point3D::new(12.0, 3.0, 0.0),
            Point3D::new(8.0, 3.0, 0.0),
        ];
        let surfaces: Vec<&[Point3D]> = vec![&floor_down[..], &floor_2[..]];
        let area = zone_floor_area(&surfaces);
        assert_relative_eq!(area, 48.0 + 12.0, max_relative = 0.001);
    }

    #[test]
    fn test_polygon_centroid() {
        let verts = vec![
            Point3D::new(0.0, 0.0, 0.0),
            Point3D::new(8.0, 0.0, 0.0),
            Point3D::new(8.0, 6.0, 0.0),
            Point3D::new(0.0, 6.0, 0.0),
        ];
        let c = polygon_centroid(&verts);
        assert_relative_eq!(c.x, 4.0, epsilon = 0.001);
        assert_relative_eq!(c.y, 3.0, epsilon = 0.001);
        assert_relative_eq!(c.z, 0.0, epsilon = 0.001);
    }

    // ── View factor tests ──────────────────────────────────────────────

    #[test]
    fn test_vf_parallel_unit_cube() {
        // Unit cube: two 1×1 squares separated by 1
        // Known value: F ≈ 0.1998
        let f = vf_parallel_rectangles(1.0, 1.0, 1.0);
        assert_relative_eq!(f, 0.1998, epsilon = 0.001);
    }

    #[test]
    fn test_vf_perpendicular_unit_cube() {
        // Unit cube: two 1×1 squares sharing an edge of length 1
        // Known value: F ≈ 0.2000
        let f = vf_perpendicular_rectangles(1.0, 1.0, 1.0);
        assert_relative_eq!(f, 0.2000, epsilon = 0.001);
    }

    #[test]
    fn test_vf_cube_row_sum() {
        // For a cube, each face sees 5 other faces.
        // F(parallel) + 4×F(perpendicular) should equal 1.0.
        let f_par = vf_parallel_rectangles(1.0, 1.0, 1.0);
        let f_perp = vf_perpendicular_rectangles(1.0, 1.0, 1.0);
        let sum = f_par + 4.0 * f_perp;
        assert_relative_eq!(sum, 1.0, epsilon = 0.002);
    }

    #[test]
    fn test_vf_box_ashrae140_row_sums() {
        // ASHRAE 140 room: 8×6×2.7m
        // Each row of the VF matrix should sum to 1.0 (enclosure rule).
        let vf = compute_box_view_factors(8.0, 6.0, 2.7);
        for face in 0..6 {
            let row_sum: f64 = vf[face].iter().sum();
            assert_relative_eq!(row_sum, 1.0, epsilon = 0.003);
        }
    }

    #[test]
    fn test_vf_box_reciprocity() {
        // A_i × F_ij = A_j × F_ji for all pairs.
        let (l, w, h) = (8.0, 6.0, 2.7);
        let vf = compute_box_view_factors(l, w, h);
        let areas = [l * w, l * w, l * h, l * h, w * h, w * h];
        for i in 0..6 {
            for j in 0..6 {
                let lhs = areas[i] * vf[i][j];
                let rhs = areas[j] * vf[j][i];
                assert_relative_eq!(lhs, rhs, epsilon = 0.01);
            }
        }
    }

    #[test]
    fn test_vf_ashrae140_geometry() {
        // In the wide, shallow ASHRAE 140 room (8×6×2.7m):
        // - Floor sees the large ceiling (48m²) most (~0.49)
        // - South wall sees floor much more than north wall
        //   (floor is large and adjacent; north wall is far away)
        let vf = compute_box_view_factors(8.0, 6.0, 2.7);

        // Floor→Ceiling dominant (large parallel surface directly above)
        let f_floor_ceiling = vf[FACE_FLOOR][FACE_CEILING];
        assert!(f_floor_ceiling > 0.45,
            "Floor→Ceiling ({:.4}) should be >0.45 for 8×6×2.7 room", f_floor_ceiling);

        // South wall→Floor > South wall→North (adjacent floor is close and large)
        let f_south_floor = vf[FACE_SOUTH][FACE_FLOOR];
        let f_south_north = vf[FACE_SOUTH][FACE_NORTH];
        assert!(f_south_floor > f_south_north,
            "South→Floor ({:.4}) should exceed South→North ({:.4})",
            f_south_floor, f_south_north);

        // South wall sees floor and ceiling equally (symmetric)
        let f_south_ceiling = vf[FACE_SOUTH][FACE_CEILING];
        assert_relative_eq!(f_south_floor, f_south_ceiling, epsilon = 0.001);
    }

    #[test]
    fn test_classify_box_face() {
        assert_eq!(classify_box_face(180.0, 0.0), Some(FACE_FLOOR));
        assert_eq!(classify_box_face(0.0, 0.0), Some(FACE_CEILING));
        assert_eq!(classify_box_face(90.0, 180.0), Some(FACE_SOUTH));
        assert_eq!(classify_box_face(90.0, 0.0), Some(FACE_NORTH));
        assert_eq!(classify_box_face(90.0, 90.0), Some(FACE_EAST));
        assert_eq!(classify_box_face(90.0, 270.0), Some(FACE_WEST));
    }

    #[test]
    fn test_azimuth_to_cardinal() {
        // Exact cardinal directions
        assert_eq!(azimuth_to_cardinal(0.0), CardinalDirection::North);
        assert_eq!(azimuth_to_cardinal(90.0), CardinalDirection::East);
        assert_eq!(azimuth_to_cardinal(180.0), CardinalDirection::South);
        assert_eq!(azimuth_to_cardinal(270.0), CardinalDirection::West);

        // Boundary cases (edges of sectors)
        assert_eq!(azimuth_to_cardinal(44.9), CardinalDirection::North);
        assert_eq!(azimuth_to_cardinal(45.0), CardinalDirection::East);
        assert_eq!(azimuth_to_cardinal(134.9), CardinalDirection::East);
        assert_eq!(azimuth_to_cardinal(135.0), CardinalDirection::South);
        assert_eq!(azimuth_to_cardinal(224.9), CardinalDirection::South);
        assert_eq!(azimuth_to_cardinal(225.0), CardinalDirection::West);
        assert_eq!(azimuth_to_cardinal(314.9), CardinalDirection::West);
        assert_eq!(azimuth_to_cardinal(315.0), CardinalDirection::North);

        // Wrap-around
        assert_eq!(azimuth_to_cardinal(360.0), CardinalDirection::North);
        assert_eq!(azimuth_to_cardinal(720.0), CardinalDirection::North);
        assert_eq!(azimuth_to_cardinal(-90.0), CardinalDirection::West);
    }

    #[test]
    fn test_envelope_areas_wwr() {
        let mut ea = EnvelopeAreas::default();
        ea.add_wall(CardinalDirection::South, 100.0);
        ea.add_window(CardinalDirection::South, 40.0);
        ea.add_wall(CardinalDirection::North, 100.0);
        ea.add_window(CardinalDirection::North, 20.0);
        ea.add_wall(CardinalDirection::East, 50.0);
        ea.add_wall(CardinalDirection::West, 50.0);
        ea.add_window(CardinalDirection::West, 10.0);

        assert_relative_eq!(ea.wwr(CardinalDirection::South), 0.40);
        assert_relative_eq!(ea.wwr(CardinalDirection::North), 0.20);
        assert_relative_eq!(ea.wwr(CardinalDirection::East), 0.0);
        assert_relative_eq!(ea.wwr(CardinalDirection::West), 0.20);

        assert_relative_eq!(ea.total_wall_area(), 300.0);
        assert_relative_eq!(ea.total_window_area(), 70.0);
        assert_relative_eq!(ea.total_wwr(), 70.0 / 300.0, max_relative = 1e-10);
    }
}
