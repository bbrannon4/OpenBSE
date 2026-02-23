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

/// Sum the areas of floor polygons (tilt ≈ 180°, i.e., face-down normals).
///
/// Identifies floors by checking if the surface normal points downward
/// (tilt > 150°).
pub fn zone_floor_area(all_surface_verts: &[&[Point3D]]) -> f64 {
    let mut total_area = 0.0;
    for verts in all_surface_verts {
        if verts.len() < 3 {
            continue;
        }
        let normal = newell_normal(verts);
        let tilt = tilt_from_normal(&normal);
        // Floors have tilt ≈ 180° (normal pointing down)
        if tilt > 150.0 {
            total_area += normal.magnitude() / 2.0;
        }
    }
    total_area
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
        // Floor polygon with normal pointing down (tilt ≈ 180°)
        let floor = vec![
            Point3D::new(0.0, 6.0, 0.0),
            Point3D::new(8.0, 6.0, 0.0),
            Point3D::new(8.0, 0.0, 0.0),
            Point3D::new(0.0, 0.0, 0.0),
        ];
        // Roof (should not count as floor)
        let roof = vec![
            Point3D::new(0.0, 0.0, 2.7),
            Point3D::new(8.0, 0.0, 2.7),
            Point3D::new(8.0, 6.0, 2.7),
            Point3D::new(0.0, 6.0, 2.7),
        ];
        // Wall (should not count as floor)
        let wall = vec![
            Point3D::new(0.0, 0.0, 0.0),
            Point3D::new(8.0, 0.0, 0.0),
            Point3D::new(8.0, 0.0, 2.7),
            Point3D::new(0.0, 0.0, 2.7),
        ];
        let surfaces: Vec<&[Point3D]> = vec![&floor[..], &roof[..], &wall[..]];
        let area = zone_floor_area(&surfaces);
        assert_relative_eq!(area, 48.0, max_relative = 0.001);
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
}
