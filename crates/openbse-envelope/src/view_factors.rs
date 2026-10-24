//! Interior long-wave exchange between the surfaces of a zone.
//!
//! Direct view factors come from the surfaces' own polygons when the zone has
//! them (contour integration, exact for a convex zone), otherwise from
//! EnergyPlus's area-weighted approximation. Internal mass has no geometry, so
//! it sees and is seen in proportion to area, as in EnergyPlus. The factors are
//! then made reciprocal and complete with EnergyPlus's `FixViewFactors`, and
//! turned into grey-body interchange factors (Hottel's script-F, EnergyPlus
//! `CalcScriptF`). With those the net long-wave gain of each surface is
//!
//!   Q_i = Σ_j G_ij σ (T_j⁴ − T_i⁴),  G_ij = G_ji,
//!
//! which sums to zero over the zone: no radiation is lost.

use crate::geometry::{newell_normal, Point3D, Vec3};

/// How a surface takes part in EnergyPlus's approximate view factors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RadSurfaceKind {
    /// Floors do not see other floors.
    Floor,
    /// Walls, ceilings, roofs and windows.
    Other,
    /// Internal mass: no geometry; sees and is seen by everything.
    InternalMass,
}

/// One surface of a zone, as the long-wave exchange sees it.
#[derive(Debug, Clone)]
pub struct RadSurface {
    /// Exchange area [m²]: the net area for a surface with subsurfaces.
    pub area: f64,
    /// Polygon (outward normal, E+ vertex order); for a parent surface this is
    /// the gross outline including its subsurfaces.
    pub polygon: Option<Vec<Point3D>>,
    /// Outward azimuth and tilt [degrees], for the approximate factors.
    pub azimuth: f64,
    pub tilt: f64,
    pub kind: RadSurfaceKind,
    /// Index (in the same list) of the surface this one is cut out of.
    pub parent: Option<usize>,
}

/// Direct view factors F[i][j] (from i to j), reciprocal and complete.
pub fn direct_view_factors(surfaces: &[RadSurface]) -> Vec<Vec<f64>> {
    let n = surfaces.len();
    let areas: Vec<f64> = surfaces.iter().map(|s| s.area.max(1e-9)).collect();
    let geometric = surfaces.iter().all(|s| {
        s.kind == RadSurfaceKind::InternalMass || s.polygon.as_ref().is_some_and(|p| p.len() >= 3)
    });
    let mut f = if geometric {
        geometric_view_factors(surfaces, &areas)
    } else {
        approximate_view_factors(surfaces, &areas)
    };
    if n > 1 {
        fix_view_factors(&areas, &mut f);
    }
    f
}

/// EnergyPlus `CalcApproximateViewFactors`: F_ij = A_j / Σ(areas i can see),
/// where no surface sees itself, floors do not see floors, and surfaces facing
/// within 10° of each other do not see each other unless one is a floor or
/// internal mass.
pub fn approximate_view_factors(surfaces: &[RadSurface], areas: &[f64]) -> Vec<Vec<f64>> {
    let n = surfaces.len();
    let mut f = vec![vec![0.0; n]; n];
    for (i, row) in f.iter_mut().enumerate() {
        let seen: Vec<usize> = (0..n).filter(|&j| sees_approx(surfaces, i, j)).collect();
        let total: f64 = seen.iter().map(|&j| areas[j]).sum();
        if total > 0.0 {
            for j in seen {
                row[j] = areas[j] / total;
            }
        }
    }
    f
}

fn sees_approx(s: &[RadSurface], i: usize, j: usize) -> bool {
    use RadSurfaceKind::*;
    if i == j || (s[i].kind == Floor && s[j].kind == Floor) {
        return false;
    }
    const SAME_ANGLE: f64 = 10.0;
    let d_az = (s[i].azimuth - s[j].azimuth).abs();
    s[i].kind == InternalMass
        || s[j].kind == InternalMass
        || s[i].kind == Floor
        || s[j].kind == Floor
        || (d_az > SAME_ANGLE && d_az < 360.0 - SAME_ANGLE)
        || (s[i].tilt - s[j].tilt).abs() > SAME_ANGLE
}

/// View factors from the polygons, with subsurfaces cut out of their parents
/// and internal mass folded in by area.
fn geometric_view_factors(surfaces: &[RadSurface], areas: &[f64]) -> Vec<Vec<f64>> {
    let n = surfaces.len();
    let poly: Vec<Option<&[Point3D]>> = surfaces
        .iter()
        .map(|s| {
            s.polygon
                .as_deref()
                .filter(|_| s.kind != RadSurfaceKind::InternalMass)
        })
        .collect();

    // A·F between the polygons as drawn (parents at their gross outline).
    let mut af_poly = vec![vec![0.0; n]; n];
    for i in 0..n {
        for j in (i + 1)..n {
            if let (Some(a), Some(b)) = (poly[i], poly[j]) {
                let af = polygon_af(a, b);
                af_poly[i][j] = af;
                af_poly[j][i] = af;
            }
        }
    }

    // Net exchange surfaces: a parent is its outline less its subsurfaces, so
    // by additivity AF(net i → net j) = AF(i → j) − AF(children of i → j)
    // − AF(i → children of j) + AF(children of i → children of j).
    let children: Vec<Vec<usize>> = (0..n)
        .map(|p| (0..n).filter(|&c| surfaces[c].parent == Some(p)).collect())
        .collect();
    let with_children = |i: usize| -> Vec<(usize, f64)> {
        std::iter::once((i, 1.0))
            .chain(children[i].iter().map(|&c| (c, -1.0)))
            .collect()
    };
    let mut f = vec![vec![0.0; n]; n];
    for i in 0..n {
        if poly[i].is_none() {
            continue;
        }
        for j in 0..n {
            if i == j || poly[j].is_none() {
                continue;
            }
            let mut af = 0.0;
            for &(a, sa) in &with_children(i) {
                for &(b, sb) in &with_children(j) {
                    if a != b {
                        af += sa * sb * af_poly[a][b];
                    }
                }
            }
            f[i][j] = (af / areas[i]).max(0.0);
        }
    }

    // Internal mass takes the share of each surface's view that the
    // approximate method gives it, and sees everything in proportion to area.
    let mass: Vec<usize> = (0..n)
        .filter(|&i| surfaces[i].kind == RadSurfaceKind::InternalMass)
        .collect();
    if !mass.is_empty() {
        let mass_area: f64 = mass.iter().map(|&m| areas[m]).sum();
        for i in 0..n {
            if surfaces[i].kind == RadSurfaceKind::InternalMass {
                let total: f64 = (0..n).filter(|&j| j != i).map(|j| areas[j]).sum();
                for j in 0..n {
                    f[i][j] = if j == i || total <= 0.0 {
                        0.0
                    } else {
                        areas[j] / total
                    };
                }
            } else {
                let seen: f64 = (0..n)
                    .filter(|&j| sees_approx(surfaces, i, j))
                    .map(|j| areas[j])
                    .sum();
                let share = if seen > 0.0 { mass_area / seen } else { 0.0 };
                for j in 0..n {
                    if surfaces[j].kind == RadSurfaceKind::InternalMass {
                        f[i][j] = areas[j] / seen.max(1e-9);
                    } else {
                        f[i][j] *= 1.0 - share;
                    }
                }
            }
        }
    }
    f
}

/// A_a·F_ab between two planar polygons (outward normals; each radiates to
/// the side its normal points away from), by the contour integral
///   A_a F_ab = (1/2π) ∮∮ ln r  dr_a·dr_b.
/// Each polygon is first clipped to the part in front of the other, so a
/// pair that can only partly see each other (non-convex zone) still gives the
/// factor between the visible parts; obstruction by third surfaces is ignored.
pub fn polygon_af(a: &[Point3D], b: &[Point3D]) -> f64 {
    let na = inward_normal(a);
    let nb = inward_normal(b);
    let (Some(na), Some(nb)) = (na, nb) else {
        return 0.0;
    };
    let a_vis = clip_in_front(a, b[0], nb);
    let b_vis = clip_in_front(b, a[0], na);
    if a_vis.len() < 3 || b_vis.len() < 3 {
        return 0.0;
    }
    if crate::geometry::polygon_area(&a_vis) < 1e-10
        || crate::geometry::polygon_area(&b_vis) < 1e-10
    {
        return 0.0;
    }
    let mut sum = 0.0;
    for p in 0..a_vis.len() {
        let p0 = a_vis[p];
        let p1 = a_vis[(p + 1) % a_vis.len()];
        for q in 0..b_vis.len() {
            let q0 = b_vis[q];
            let q1 = b_vis[(q + 1) % b_vis.len()];
            sum += edge_pair_integral(p0, p1, q0, q1);
        }
    }
    (sum / (2.0 * std::f64::consts::PI)).max(0.0)
}

fn inward_normal(poly: &[Point3D]) -> Option<Vec3> {
    let n = newell_normal(poly);
    let m = n.magnitude();
    (m > 1e-12).then(|| n.scale(-1.0 / m))
}

/// The part of `poly` on the front side (along `normal`) of the plane through
/// `origin`. Points on the plane are kept, so a shared edge survives.
fn clip_in_front(poly: &[Point3D], origin: Point3D, normal: Vec3) -> Vec<Point3D> {
    const TOL: f64 = 1e-9;
    let d = |p: &Point3D| (*p - origin).dot(&normal);
    let mut out = Vec::with_capacity(poly.len() + 2);
    for k in 0..poly.len() {
        let cur = poly[k];
        let nxt = poly[(k + 1) % poly.len()];
        let (dc, dn) = (d(&cur), d(&nxt));
        if dc >= -TOL {
            out.push(cur);
        }
        if (dc > TOL && dn < -TOL) || (dc < -TOL && dn > TOL) {
            let t = dc / (dc - dn);
            out.push(Point3D::new(
                cur.x + t * (nxt.x - cur.x),
                cur.y + t * (nxt.y - cur.y),
                cur.z + t * (nxt.z - cur.z),
            ));
        }
    }
    out
}

fn sub(a: Point3D, b: Point3D) -> Vec3 {
    a - b
}

fn lerp(a: Point3D, b: Point3D, t: f64) -> Point3D {
    Point3D::new(
        a.x + t * (b.x - a.x),
        a.y + t * (b.y - a.y),
        a.z + t * (b.z - a.z),
    )
}

/// (û_p·û_q) ∫∫ ln|P(s) − Q(t)| ds dt over two edges.
fn edge_pair_integral(p0: Point3D, p1: Point3D, q0: Point3D, q1: Point3D) -> f64 {
    let dp = sub(p1, p0);
    let dq = sub(q1, q0);
    let (lp, lq) = (dp.magnitude(), dq.magnitude());
    if lp < 1e-12 || lq < 1e-12 {
        return 0.0;
    }
    let (u, v) = (dp.scale(1.0 / lp), dq.scale(1.0 / lq));
    let cos = u.dot(&v);
    if cos.abs() < 1e-12 {
        return 0.0;
    }
    // Collinear edges: the integrand is singular along the overlap, but the
    // one-dimensional integral has a closed form.
    let off = sub(q0, p0);
    let perp = off - u.scale(off.dot(&u));
    if u.cross(&v).magnitude() < 1e-9 && perp.magnitude() < 1e-9 {
        let t0 = off.dot(&u);
        let t1 = t0 + lq * cos;
        let (lo, hi) = if t0 < t1 { (t0, t1) } else { (t1, t0) };
        return cos * collinear_log_integral(lp, lo, hi);
    }
    cos * log_integral(p0, p1, q0, q1, 0)
}

/// ∫_0^L ∫_lo^hi ln|s − τ| dτ ds, exactly.
fn collinear_log_integral(l: f64, lo: f64, hi: f64) -> f64 {
    // G'' = ln|x| with G(x) = x²/2·ln|x| − 3x²/4, G(0) = 0.
    let g = |x: f64| {
        if x.abs() < 1e-300 {
            0.0
        } else {
            0.5 * x * x * x.abs().ln() - 0.75 * x * x
        }
    };
    g(l - lo) - g(l - hi) - g(-lo) + g(-hi)
}

/// ∫∫ ln|P(s) − Q(t)| ds dt (unit-speed parameters) by Gauss–Legendre,
/// splitting the longer edge while the edges are close relative to their
/// length, which isolates the logarithmic singularity where they meet.
fn log_integral(p0: Point3D, p1: Point3D, q0: Point3D, q1: Point3D, depth: u32) -> f64 {
    let lp = sub(p1, p0).magnitude();
    let lq = sub(q1, q0).magnitude();
    let dist = segment_distance(p0, p1, q0, q1);
    if dist >= lp.max(lq) || depth >= 40 {
        return gauss_log(p0, p1, q0, q1, lp, lq);
    }
    if lp >= lq {
        let m = lerp(p0, p1, 0.5);
        log_integral(p0, m, q0, q1, depth + 1) + log_integral(m, p1, q0, q1, depth + 1)
    } else {
        let m = lerp(q0, q1, 0.5);
        log_integral(p0, p1, q0, m, depth + 1) + log_integral(p0, p1, m, q1, depth + 1)
    }
}

const GAUSS_X: [f64; 8] = [
    -0.960_289_856_497_536_3,
    -0.796_666_477_413_626_7,
    -0.525_532_409_916_329,
    -0.183_434_642_495_649_8,
    0.183_434_642_495_649_8,
    0.525_532_409_916_329,
    0.796_666_477_413_626_7,
    0.960_289_856_497_536_3,
];
const GAUSS_W: [f64; 8] = [
    0.101_228_536_290_376_3,
    0.222_381_034_453_374_5,
    0.313_706_645_877_887_3,
    0.362_683_783_378_362,
    0.362_683_783_378_362,
    0.313_706_645_877_887_3,
    0.222_381_034_453_374_5,
    0.101_228_536_290_376_3,
];

fn gauss_log(p0: Point3D, p1: Point3D, q0: Point3D, q1: Point3D, lp: f64, lq: f64) -> f64 {
    let mut sum = 0.0;
    for (xi, wi) in GAUSS_X.iter().zip(GAUSS_W) {
        let p = lerp(p0, p1, 0.5 * (xi + 1.0));
        for (xj, wj) in GAUSS_X.iter().zip(GAUSS_W) {
            let q = lerp(q0, q1, 0.5 * (xj + 1.0));
            let r = sub(p, q).magnitude().max(1e-15);
            sum += wi * wj * r.ln();
        }
    }
    sum * 0.25 * lp * lq
}

/// Shortest distance between two segments.
fn segment_distance(p0: Point3D, p1: Point3D, q0: Point3D, q1: Point3D) -> f64 {
    let d1 = sub(p1, p0);
    let d2 = sub(q1, q0);
    let r = sub(p0, q0);
    let (a, e, f) = (d1.dot(&d1), d2.dot(&d2), d2.dot(&r));
    let (s, t);
    if a <= 1e-24 && e <= 1e-24 {
        return r.magnitude();
    }
    if a <= 1e-24 {
        s = 0.0;
        t = (f / e).clamp(0.0, 1.0);
    } else {
        let c = d1.dot(&r);
        if e <= 1e-24 {
            t = 0.0;
            s = (-c / a).clamp(0.0, 1.0);
        } else {
            let b = d1.dot(&d2);
            let denom = a * e - b * b;
            let mut s0 = if denom > 1e-24 {
                ((b * f - c * e) / denom).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let mut t0 = (b * s0 + f) / e;
            if t0 < 0.0 {
                t0 = 0.0;
                s0 = (-c / a).clamp(0.0, 1.0);
            } else if t0 > 1.0 {
                t0 = 1.0;
                s0 = ((b - c) / a).clamp(0.0, 1.0);
            }
            s = s0;
            t = t0;
        }
    }
    let cp = lerp(p0, p1, s);
    let cq = lerp(q0, q1, t);
    sub(cp, cq).magnitude()
}

/// EnergyPlus `FixViewFactors`: average A_i F_ij with A_j F_ji for
/// reciprocity, then rescale rows towards completeness, repeating until the
/// factors settle. A surface larger than all the others together gets a
/// self-view, as in EnergyPlus.
pub fn fix_view_factors(areas: &[f64], f: &mut [Vec<f64>]) {
    const PRIMARY_CONVERGENCE: f64 = 0.001;
    const DIFFERENCE_CONVERGENCE: f64 = 0.00001;
    let n = areas.len();
    let total_area: f64 = areas.iter().sum();
    let sum_f = |m: &[Vec<f64>]| m.iter().flatten().sum::<f64>();
    let original_check = (sum_f(f) - n as f64).abs();

    let mut fixed: Vec<Vec<f64>> = f.to_vec();
    let largest = areas.iter().cloned().fold(0.0, f64::max);
    if largest > 0.99 * (total_area - largest) && n > 3 {
        if let Some(k) = areas.iter().position(|&a| a == largest) {
            fixed[k][k] = (1.2 * largest / total_area).min(0.9);
        }
    }
    let mut af: Vec<Vec<f64>> = (0..n)
        .map(|i| (0..n).map(|j| fixed[i][j] * areas[i]).collect())
        .collect();
    symmetrize(&mut af);

    let to_f = |af: &mut [Vec<f64>]| -> Vec<Vec<f64>> {
        let mut out = vec![vec![0.0; n]; n];
        for i in 0..n {
            for j in 0..n {
                out[i][j] = af[i][j] / areas[i];
                if out[i][j].abs() < 1e-10 {
                    out[i][j] = 0.0;
                    af[i][j] = 0.0;
                }
            }
        }
        out
    };

    if n <= 3 {
        // Too few surfaces for an enclosure: reciprocity only, capped so no
        // row exceeds unity.
        let mut out = to_f(&mut af);
        let max_row = out
            .iter()
            .map(|r| r.iter().sum::<f64>())
            .fold(0.0, f64::max);
        if sum_f(&out) > n as f64 + 0.01 && max_row > 1.0 {
            for row in out.iter_mut() {
                for v in row.iter_mut() {
                    *v /= max_row;
                }
            }
        }
        f.clone_from_slice(&out);
        return;
    }

    let mut converge_old = 10.0;
    let mut out = to_f(&mut af);
    for iteration in 1..=400 {
        for i in 0..n {
            let row: f64 = af[i].iter().sum();
            let coefficient = if row.abs() > 1e-10 {
                areas[i] / row
            } else {
                1.0
            };
            for v in af[i].iter_mut() {
                *v *= coefficient;
            }
        }
        symmetrize(&mut af);
        out = to_f(&mut af);
        let converge_new = (sum_f(&out) - n as f64).abs();
        if (converge_old - converge_new).abs() < DIFFERENCE_CONVERGENCE
            || converge_new <= PRIMARY_CONVERGENCE
            || iteration == 400
        {
            if converge_new < original_check || converge_new < PRIMARY_CONVERGENCE {
                f.clone_from_slice(&out);
            }
            return;
        }
        converge_old = converge_new;
    }
    f.clone_from_slice(&out);
}

// Each step writes the (i, j) and (j, i) pair, which needs both indices.
#[allow(clippy::needless_range_loop)]
fn symmetrize(af: &mut [Vec<f64>]) {
    let n = af.len();
    for i in 0..n {
        for j in (i + 1)..n {
            let avg = 0.5 * (af[i][j] + af[j][i]);
            af[i][j] = avg;
            af[j][i] = avg;
        }
    }
}

/// Grey-body interchange areas G (Hottel's script-F times area, EnergyPlus
/// `CalcScriptF`): Q_i = Σ_j G_ij σ (T_j⁴ − T_i⁴), with G symmetric and
/// G_ii = 0. From the radiosities J = ε E + (1 − ε) F J:
///   G_ij = A_i ε_i / (1 − ε_i) · R_ij,  R = (I − (1 − ε) F)⁻¹ ε.
pub fn interchange_areas(areas: &[f64], f: &[Vec<f64>], emissivity: &[f64]) -> Vec<Vec<f64>> {
    const MAX_EMISSIVITY: f64 = 0.99999;
    let n = areas.len();
    let eps: Vec<f64> = emissivity
        .iter()
        .map(|&e| e.clamp(1e-6, MAX_EMISSIVITY))
        .collect();
    // M = I − diag(1 − ε) F
    let mut m = vec![vec![0.0; n]; n];
    for i in 0..n {
        for j in 0..n {
            m[i][j] = -(1.0 - eps[i]) * f[i][j];
        }
        m[i][i] += 1.0;
    }
    let Some(minv) = invert(m) else {
        return vec![vec![0.0; n]; n];
    };
    let mut g = vec![vec![0.0; n]; n];
    for i in 0..n {
        let fac = areas[i] * eps[i] / (1.0 - eps[i]);
        for j in 0..n {
            if i != j {
                g[i][j] = fac * minv[i][j] * eps[j];
            }
        }
    }
    symmetrize(&mut g);
    g
}

/// Inverse by Gauss–Jordan elimination with partial pivoting.
fn invert(mut a: Vec<Vec<f64>>) -> Option<Vec<Vec<f64>>> {
    let n = a.len();
    let mut inv: Vec<Vec<f64>> = (0..n)
        .map(|i| (0..n).map(|j| if i == j { 1.0 } else { 0.0 }).collect())
        .collect();
    for col in 0..n {
        let pivot = (col..n).max_by(|&x, &y| a[x][col].abs().total_cmp(&a[y][col].abs()))?;
        if a[pivot][col].abs() < 1e-14 {
            return None;
        }
        a.swap(col, pivot);
        inv.swap(col, pivot);
        let p = a[col][col];
        for j in 0..n {
            a[col][j] /= p;
            inv[col][j] /= p;
        }
        for r in 0..n {
            if r != col {
                let k = a[r][col];
                if k != 0.0 {
                    for j in 0..n {
                        a[r][j] -= k * a[col][j];
                        inv[r][j] -= k * inv[col][j];
                    }
                }
            }
        }
    }
    Some(inv)
}

/// Linearised exchange for surface `i` at temperature `t_i`, with the other
/// surfaces at `temps` [°C]: returns (h, T_mrt) with h·(T_mrt − T_i) equal to
/// the exact net long-wave gain per unit area, Σ_j G_ij σ (T_j⁴ − T_i⁴) / A_i.
pub fn linearised_exchange(
    g_row: &[f64],
    area: f64,
    i: usize,
    t_i: f64,
    temps: &[f64],
) -> (f64, f64) {
    const SIGMA: f64 = 5.6704e-8;
    let ti = t_i + 273.15;
    let (mut h, mut ht) = (0.0, 0.0);
    for (j, &gij) in g_row.iter().enumerate() {
        if j == i || gij == 0.0 {
            continue;
        }
        let tj = temps[j] + 273.15;
        let w = gij * SIGMA * (ti * ti + tj * tj) * (ti + tj);
        h += w;
        ht += w * temps[j];
    }
    if h <= 0.0 || area <= 0.0 {
        return (0.0, t_i);
    }
    (h / area, ht / h)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(x: f64, y: f64, z: f64) -> Point3D {
        Point3D::new(x, y, z)
    }

    /// Faces of an L×W×H box with outward normals (E+ vertex order).
    fn box_faces(l: f64, w: f64, h: f64) -> Vec<Vec<Point3D>> {
        vec![
            vec![p(0., 0., 0.), p(0., w, 0.), p(l, w, 0.), p(l, 0., 0.)], // floor (down)
            vec![p(0., 0., h), p(l, 0., h), p(l, w, h), p(0., w, h)],     // ceiling (up)
            vec![p(0., 0., h), p(0., 0., 0.), p(l, 0., 0.), p(l, 0., h)], // south (−y)
            vec![p(l, w, h), p(l, w, 0.), p(0., w, 0.), p(0., w, h)],     // north (+y)
            vec![p(0., w, h), p(0., w, 0.), p(0., 0., 0.), p(0., 0., h)], // west (−x)
            vec![p(l, 0., h), p(l, 0., 0.), p(l, w, 0.), p(l, w, h)],     // east (+x)
        ]
    }

    #[test]
    fn parallel_unit_squares_match_the_closed_form() {
        let faces = box_faces(1.0, 1.0, 1.0);
        // Floor to ceiling of a unit cube: 0.19982.
        let af = polygon_af(&faces[0], &faces[1]);
        assert!((af - 0.199_825).abs() < 1e-5, "{af}");
    }

    #[test]
    fn perpendicular_squares_sharing_an_edge_match_the_closed_form() {
        let faces = box_faces(1.0, 1.0, 1.0);
        let af = polygon_af(&faces[0], &faces[2]);
        assert!((af - 0.200_044).abs() < 1e-5, "{af}");
    }

    #[test]
    fn box_view_factors_agree_with_the_rectangle_formulas() {
        let (l, w, h) = (8.0, 6.0, 2.7);
        let faces = box_faces(l, w, h);
        let vf = crate::geometry::compute_box_view_factors(l, w, h);
        // geometry's face order: floor, ceiling, south, north, east, west.
        let order = [0usize, 1, 2, 3, 5, 4];
        for (a, &fa) in order.iter().enumerate() {
            let area = crate::geometry::polygon_area(&faces[fa]);
            for (b, &fb) in order.iter().enumerate() {
                if a == b {
                    continue;
                }
                let f = polygon_af(&faces[fa], &faces[fb]) / area;
                assert!((f - vf[a][b]).abs() < 1e-4, "{a}->{b}: {f} vs {}", vf[a][b]);
            }
        }
    }

    #[test]
    fn coplanar_surfaces_do_not_see_each_other() {
        let wall = vec![p(0., 0., 2.), p(0., 0., 0.), p(4., 0., 0.), p(4., 0., 2.)];
        let window = vec![
            p(1., 0., 1.5),
            p(1., 0., 0.5),
            p(3., 0., 0.5),
            p(3., 0., 1.5),
        ];
        assert_eq!(polygon_af(&wall, &window), 0.0);
    }

    fn rad(
        poly: Vec<Point3D>,
        kind: RadSurfaceKind,
        parent: Option<usize>,
        area: f64,
    ) -> RadSurface {
        let n = crate::geometry::newell_normal(&poly).normalize();
        RadSurface {
            area,
            azimuth: crate::geometry::azimuth_from_normal(&n),
            tilt: crate::geometry::tilt_from_normal(&n),
            polygon: Some(poly),
            kind,
            parent,
        }
    }

    fn room_with_window_and_mass() -> Vec<RadSurface> {
        let (l, w, h) = (10.0, 5.0, 3.0);
        let faces = box_faces(l, w, h);
        let window = vec![
            p(2., 0., 2.5),
            p(2., 0., 0.5),
            p(8., 0., 0.5),
            p(8., 0., 2.5),
        ];
        let mut s: Vec<RadSurface> = faces
            .into_iter()
            .enumerate()
            .map(|(k, f)| {
                let area = crate::geometry::polygon_area(&f) - if k == 2 { 12.0 } else { 0.0 };
                let kind = if k == 0 {
                    RadSurfaceKind::Floor
                } else {
                    RadSurfaceKind::Other
                };
                rad(f, kind, None, area)
            })
            .collect();
        s.push(rad(window, RadSurfaceKind::Other, Some(2), 12.0));
        s.push(RadSurface {
            area: 80.0,
            polygon: None,
            azimuth: 0.0,
            tilt: 90.0,
            kind: RadSurfaceKind::InternalMass,
            parent: None,
        });
        s
    }

    #[test]
    fn view_factors_are_reciprocal_and_complete() {
        let s = room_with_window_and_mass();
        let f = direct_view_factors(&s);
        for i in 0..s.len() {
            let row: f64 = f[i].iter().sum();
            assert!((row - 1.0).abs() < 2e-3, "row {i} sums to {row}");
            for j in 0..s.len() {
                let (a, b) = (s[i].area * f[i][j], s[j].area * f[j][i]);
                assert!(
                    (a - b).abs() < 1e-6 * s[i].area.max(1.0),
                    "{i},{j}: {a} vs {b}"
                );
            }
        }
        // The window sees nothing of its own wall.
        assert!(f[6][2] < 1e-9 && f[2][6] < 1e-9);
    }

    #[test]
    fn interchange_conserves_energy() {
        let s = room_with_window_and_mass();
        let f = direct_view_factors(&s);
        let areas: Vec<f64> = s.iter().map(|x| x.area).collect();
        let eps = [0.9, 0.9, 0.9, 0.9, 0.9, 0.9, 0.84, 0.9];
        let g = interchange_areas(&areas, &f, &eps);
        let temps = [18.0, 22.0, 15.0, 21.0, 20.0, 20.5, 8.0, 21.5];
        let total: f64 = (0..s.len())
            .map(|i| {
                let (h, t_mrt) = linearised_exchange(&g[i], areas[i], i, temps[i], &temps);
                h * (t_mrt - temps[i]) * areas[i]
            })
            .sum();
        assert!(total.abs() < 1e-6, "net long-wave {total} W");
        // The cold window gains, everything else loses.
        let (h, t_mrt) = linearised_exchange(&g[6], areas[6], 6, temps[6], &temps);
        assert!(h * (t_mrt - temps[6]) > 0.0);
    }

    #[test]
    fn black_surfaces_exchange_by_view_factor() {
        let faces = box_faces(1.0, 1.0, 1.0);
        let s: Vec<RadSurface> = faces
            .into_iter()
            .enumerate()
            .map(|(k, f)| {
                let kind = if k == 0 {
                    RadSurfaceKind::Floor
                } else {
                    RadSurfaceKind::Other
                };
                rad(f, kind, None, 1.0)
            })
            .collect();
        let f = direct_view_factors(&s);
        let g = interchange_areas(&[1.0; 6], &f, &[0.99999; 6]);
        assert!(
            (g[0][1] - f[0][1]).abs() < 1e-3,
            "{} vs {}",
            g[0][1],
            f[0][1]
        );
    }

    #[test]
    fn approximate_factors_follow_energyplus_rules() {
        let mut s = room_with_window_and_mass();
        s[0].polygon = None; // force the approximate method
        let areas: Vec<f64> = s.iter().map(|x| x.area).collect();
        let f = approximate_view_factors(&s, &areas);
        // The south wall and its window face the same way: no exchange.
        assert_eq!(f[2][6], 0.0);
        // Everything sees the internal mass.
        assert!(f[1][7] > 0.0 && f[7][0] > 0.0);
    }
}
