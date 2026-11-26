//! Daylighting controls by the split-flux method, as EnergyPlus
//! `Daylighting:Controls` with Daylighting Method SplitFlux
//! (`DaylightingManager.cc`).
//!
//! For each daylit zone and each hour's sun position of the day, the
//! illuminance at each reference point from each exterior window is found for
//! four standard skies (CIE clear, clear turbid, intermediate, overcast) and
//! for the sun, each as a factor of the exterior horizontal illuminance it
//! would give:
//!
//! - **direct**: the window, divided into elements, is seen from the
//!   reference point; each element contributes the sky luminance in its
//!   direction times its solid angle, the glass's visible transmittance at
//!   that angle and the cosine to the (horizontal) work plane
//!   (`FigureDayltgCoeffsAtPointsForWindowElements`,
//!   `FigureDayltgCoeffsAtPointsForSunPosition`); the sun adds its beam when
//!   the ray from the point to the sun passes through the window;
//! - **internally reflected** (the split flux): the luminous flux through the
//!   window from the sky, ground and sun is split into a downward part, which
//!   first meets the floor and lower walls, and an upward part, which meets
//!   the ceiling and upper walls; the average illuminance it gives after
//!   inter-reflection is `(Φ_down ρ_fw + Φ_up ρ_cw) / (A (1 − ρ̄))`
//!   (`DayltgInterReflectedIllum`, `DayltgAveInteriorReflectance`).
//!
//! Each timestep the factors of the hours either side are interpolated, the
//! sky is a mix of two standard skies picked by the Perez sky clearness and
//! brightness, and the exterior illuminances come from the weather's beam and
//! diffuse irradiance by the Perez luminous efficacy (`DayltgInteriorIllum`,
//! `WeatherManager::DayltgLuminousEfficacy`). The lighting control then sets
//! each reference point's power fraction and the zone's power reduction
//! (`DayltgElecLightingControl`).

use serde::{Deserialize, Serialize};
use std::f64::consts::{FRAC_PI_2, PI};

use openbse_core::ports::SimulationContext;
use openbse_weather::WeatherHour;

use crate::geometry::{newell_normal, polygon_area, Point3D, Vec3};
use crate::heat_balance::BuildingEnvelope;
use crate::solar;
use crate::surface::{BoundaryCondition, SurfaceType};

/// Number of standard sky types.
const N_SKY: usize = 4;
/// Sky types, in order.
const CLEAR: usize = 0;
const CLEAR_TURBID: usize = 1;
const INTERMEDIATE: usize = 2;
const OVERCAST: usize = 3;
/// Sky integration steps for the exterior horizontal illuminance (E+ NTH, NPH).
const NTH: usize = 18;
const NPH: usize = 8;
/// Sky and ground integration steps for the flux through a window (E+ NTHMAX,
/// NPHMAX).
const NTHMAX: usize = 16;
const NPHMAX: usize = 10;
/// Smallest exterior horizontal illuminance a factor is normalised by [lux]
/// (E+ `tmpDFCalc`).
const MIN_HORIZONTAL_ILLUMINANCE: f64 = 0.05;

// ─── Input ───────────────────────────────────────────────────────────────────

/// How the electric lighting responds to daylight (E+ Lighting Control Type).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LightingControl {
    /// Dims continuously to the minimum light output, never below the
    /// minimum input power.
    #[default]
    Continuous,
    /// As continuous, but switches off at the minimum light output.
    ContinuousOff,
    /// Switches in equal steps.
    Stepped,
}

/// A daylighting reference point and the share of the zone's lighting it
/// controls.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReferencePointInput {
    /// Position [m], in the model's coordinates.
    pub x: f64,
    pub y: f64,
    pub z: f64,
    /// Fraction of the zone's lighting this point controls (default 1).
    #[serde(default = "default_fraction")]
    pub fraction_of_zone: f64,
    /// Illuminance setpoint [lux] (default 500, as E+).
    #[serde(default = "default_setpoint")]
    pub illuminance_setpoint: f64,
}

fn default_fraction() -> f64 {
    1.0
}
fn default_setpoint() -> f64 {
    500.0
}
fn default_min_power() -> f64 {
    0.3
}
fn default_min_light() -> f64 {
    0.2
}
fn default_steps() -> u32 {
    1
}

/// Daylighting controls of one zone (E+ `Daylighting:Controls`, SplitFlux).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DaylightingControlInput {
    pub name: String,
    /// The daylit zone. Its lights' power is reduced; its exterior windows
    /// (and skylights) admit the daylight.
    pub zone: String,
    #[serde(default)]
    pub lighting_control: LightingControl,
    /// Lowest input power fraction of continuous dimming (E+ default 0.3).
    #[serde(default = "default_min_power")]
    pub minimum_input_power_fraction: f64,
    /// Lowest light output fraction of continuous dimming (E+ default 0.2).
    #[serde(default = "default_min_light")]
    pub minimum_light_output_fraction: f64,
    /// Number of steps of stepped control (default 1).
    #[serde(default = "default_steps")]
    pub stepped_control_steps: u32,
    pub reference_points: Vec<ReferencePointInput>,
}

impl DaylightingControlInput {
    /// Power fraction of one reference point for the daylight illuminance
    /// `illum` there (E+ `DayltgElecLightingControl`).
    pub fn power_fraction(&self, illum: f64, setpoint: f64) -> f64 {
        let fl = if illum < setpoint {
            (setpoint - illum) / setpoint
        } else {
            0.0
        };
        let (min_light, min_power) = (
            self.minimum_light_output_fraction,
            self.minimum_input_power_fraction,
        );
        match self.lighting_control {
            LightingControl::Continuous | LightingControl::ContinuousOff => {
                if fl <= min_light {
                    if self.lighting_control == LightingControl::ContinuousOff {
                        0.0
                    } else {
                        min_power
                    }
                } else if fl < 1.0 {
                    (fl + (1.0 - fl) * min_power - min_light) / (1.0 - min_light)
                } else {
                    1.0
                }
            }
            LightingControl::Stepped => {
                let steps = self.stepped_control_steps.max(1) as f64;
                if illum < 0.1 {
                    1.0
                } else if illum < setpoint {
                    (((steps * fl) as i64 + 1) as f64 / steps).min(1.0)
                } else {
                    0.0
                }
            }
        }
    }

    /// The zone's lighting power multiplier for the daylight illuminance at
    /// each reference point: each point's power fraction over its share of
    /// the zone, full power over the rest.
    pub fn power_reduction(&self, illum: &[f64]) -> f64 {
        let mut total = 0.0;
        let mut share = 0.0;
        for (p, &e) in self.reference_points.iter().zip(illum) {
            total += self.power_fraction(e, p.illuminance_setpoint) * p.fraction_of_zone;
            share += p.fraction_of_zone;
        }
        total + (1.0 - share)
    }
}

// ─── Sky ─────────────────────────────────────────────────────────────────────

/// Sun direction: altitude φ, azimuth θ (from east, counter-clockwise toward
/// north, as E+'s `atan2(y, x)`), and the unit vector (x east, y north, z up).
#[derive(Debug, Clone, Copy)]
pub struct Sun {
    pub phi: f64,
    pub theta: f64,
    pub dir: Vec3,
}

impl Sun {
    /// From a unit direction toward the sun.
    pub fn from_dir(dir: Vec3) -> Self {
        Self {
            phi: dir.z.clamp(-1.0, 1.0).asin(),
            theta: dir.y.atan2(dir.x),
            dir,
        }
    }
}

/// Sky luminance in the direction (azimuth `th`, altitude `ph`), relative to
/// the zenith luminance, for a standard sky (E+ `DayltgSkyLuminance`, after
/// Perez et al. 1990 and the CIE skies).
pub fn sky_luminance(sky: usize, sun: &Sun, th: f64, ph: f64) -> f64 {
    let sph = ph.sin().max(0.01);
    let z = FRAC_PI_2 - sun.phi;
    let (sin_phi, cos_phi) = (sun.phi.sin(), sun.phi.cos());
    let cosg = (sph * sin_phi + ph.cos() * cos_phi * (th - sun.theta).cos()).clamp(-1.0, 1.0);
    let g = cosg.acos();
    match sky {
        CLEAR => {
            let z1 = 0.910 + 10.0 * (-3.0 * g).exp() + 0.45 * cosg * cosg;
            let z2 = 1.0 - (-0.32 / sph).exp();
            let z3 = 0.27385 * (0.91 + 10.0 * (-3.0 * z).exp() + 0.45 * sin_phi * sin_phi);
            z1 * z2 / z3
        }
        CLEAR_TURBID => {
            let z1 = 0.856 + 16.0 * (-3.0 * g).exp() + 0.3 * cosg * cosg;
            let z2 = 1.0 - (-0.32 / sph).exp();
            let z3 = 0.27385 * (0.856 + 16.0 * (-3.0 * z).exp() + 0.3 * sin_phi * sin_phi);
            z1 * z2 / z3
        }
        INTERMEDIATE => {
            let z1 = (1.35 * ((3.59 * ph - 0.009).sin() + 2.31) * (2.6 * sun.phi + 0.316).sin()
                + ph
                + 4.799)
                / 2.326;
            let z2 = (-g * 0.563 * ((sun.phi - 0.008) * (ph + 1.059) + 0.812)).exp();
            let z3 = 0.99224 * (2.6 * sun.phi + 0.316).sin() + 2.73852;
            let z4 = (-z * 0.563 * ((sun.phi - 0.008) * 2.6298 + 0.812)).exp();
            z1 * z2 / (z3 * z4)
        }
        _ => (1.0 + 2.0 * sph) / 3.0,
    }
}

/// Exterior horizontal illuminance of each standard sky for a unit zenith
/// luminance, and of the sun for a unit beam normal illuminance (E+
/// `DayltgExtHorizIllum`).
pub fn exterior_horizontal_illuminance(sun: &Sun) -> ([f64; N_SKY], f64) {
    let dth = 2.0 * PI / NTH as f64;
    let dph = FRAC_PI_2 / NPH as f64;
    let mut hi = [0.0; N_SKY];
    for iph in 0..NPH {
        let ph = (iph as f64 + 0.5) * dph;
        let w = ph.sin() * ph.cos();
        for ith in 0..NTH {
            let th = (ith as f64 + 0.5) * dth;
            for (sky, h) in hi.iter_mut().enumerate() {
                *h += sky_luminance(sky, sun, th, ph) * w;
            }
        }
    }
    for h in hi.iter_mut() {
        *h *= dth * dph;
    }
    (hi, sun.phi.sin())
}

/// What the weather gives the daylighting this timestep: the exterior
/// horizontal illuminance from the sky and from the sun's beam [lux] and the
/// Perez sky clearness and brightness (E+ `HISKF`, `HISUNF`, `SkyClearness`,
/// `SkyBrightness`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DaylightWeather {
    pub sky_illuminance: f64,
    pub sun_illuminance: f64,
    pub clearness: f64,
    pub brightness: f64,
}

/// Perez (1990) sky clearness, brightness and luminous efficacy of diffuse
/// and beam radiation (E+ `DayltgLuminousEfficacy`), and the exterior
/// horizontal illuminances, from the beam normal and diffuse horizontal
/// irradiance [W/m²], the sun's altitude, the outdoor dew point [°C], the site
/// elevation [m] and the month (1-12). `None` with the sun down.
pub fn daylight_weather(
    beam_normal: f64,
    diffuse_horizontal: f64,
    sun_altitude: f64,
    dew_point: f64,
    elevation: f64,
    month: u32,
) -> Option<DaylightWeather> {
    const A_DIF: [f64; 8] = [
        97.24, 107.22, 104.97, 102.39, 100.71, 106.42, 141.88, 152.23,
    ];
    const B_DIF: [f64; 8] = [-0.46, 1.15, 2.96, 5.59, 5.94, 3.83, 1.90, 0.35];
    const C_DIF: [f64; 8] = [12.00, 0.59, -5.53, -13.95, -22.75, -36.15, -53.24, -45.27];
    const D_DIF: [f64; 8] = [-8.91, -3.95, -8.77, -13.90, -23.74, -28.83, -14.03, -7.98];
    const A_DIR: [f64; 8] = [57.20, 98.99, 109.83, 110.34, 106.36, 107.19, 105.75, 101.18];
    const B_DIR: [f64; 8] = [-4.55, -3.46, -4.90, -5.84, -3.97, -1.25, 0.77, 1.58];
    const C_DIR: [f64; 8] = [-2.98, -1.21, -1.71, -1.99, -1.75, -1.51, -1.26, -1.10];
    const D_DIR: [f64; 8] = [117.12, 12.38, -8.81, -4.56, -6.16, -26.73, -34.44, -8.29];
    const EXTRA_DNI_ILL: [f64; 12] = [
        131153.0, 130613.0, 128992.0, 126816.0, 124731.0, 123240.0, 122652.0, 123120.0, 124576.0,
        126658.0, 128814.0, 130471.0,
    ];
    let sin_alt = sun_altitude.sin();
    if sin_alt <= 1.0e-5 {
        return None;
    }
    let zenith = FRAC_PI_2 - sun_altitude;
    let zeta = 1.041 * zenith.powi(3);
    let clearness =
        ((diffuse_horizontal + beam_normal) / (diffuse_horizontal + 0.0001) + zeta) / (1.0 + zeta);
    let air_mass = (1.0 - 0.1 * elevation / 1000.0)
        / (sin_alt + 0.15 / (sun_altitude.to_degrees() + 3.885).powf(1.253));
    let brightness =
        diffuse_horizontal * 93.73 * air_mass / EXTRA_DNI_ILL[(month.clamp(1, 12) - 1) as usize];
    let bin = [1.065, 1.23, 1.50, 1.95, 2.80, 4.50, 6.20]
        .iter()
        .position(|&edge| clearness <= edge)
        .unwrap_or(7);
    let moisture = (0.07 * dew_point - 0.075).exp();
    let cos_z = zenith.cos();
    let (dif_eff, dir_eff) = if brightness <= 0.0 {
        (0.0, 0.0)
    } else {
        (
            A_DIF[bin] + B_DIF[bin] * moisture + C_DIF[bin] * cos_z + D_DIF[bin] * brightness.ln(),
            (A_DIR[bin]
                + B_DIR[bin] * moisture
                + C_DIR[bin] * (5.73 * zenith - 5.0).exp()
                + D_DIR[bin] * brightness)
                .max(0.0),
        )
    };
    Some(DaylightWeather {
        sky_illuminance: diffuse_horizontal * dif_eff,
        sun_illuminance: beam_normal * sin_alt * dir_eff,
        clearness,
        brightness,
    })
}

/// The two standard skies a timestep's sky is a mix of, and the weight of the
/// first (E+ `DayltgInteriorIllum`).
pub fn sky_mix(clearness: f64, brightness: f64) -> (usize, usize, f64) {
    if clearness > 3.0 {
        (CLEAR, CLEAR_TURBID, ((clearness - 3.0) / 3.0).min(1.0))
    } else if clearness > 1.2 {
        (CLEAR_TURBID, INTERMEDIATE, (clearness - 1.2) / 1.8)
    } else {
        let w = ((clearness - 1.0) / 0.2)
            .max((brightness - 0.05) / 0.4)
            .clamp(0.0, 1.0);
        (INTERMEDIATE, OVERCAST, w)
    }
}

// ─── Geometry ────────────────────────────────────────────────────────────────

/// An exterior window (or skylight) of a daylit zone.
#[derive(Debug, Clone)]
pub struct DaylightWindow {
    pub name: String,
    /// Corner and the two edges spanning it (a parallelogram).
    pub corner: Point3D,
    pub edge_w: Vec3,
    pub edge_h: Vec3,
    pub area: f64,
    /// Outward unit normal.
    pub normal: Vec3,
    /// Beam visible transmittance fit in cos θ (E+ `TransVisBeamCoef`).
    pub tvis: [f64; 6],
    /// Area-weighted visible reflectance of the floor and lower walls, and of
    /// the ceiling and upper walls, seen from this window (E+ `rhoFloorWall`,
    /// `rhoCeilingWall`).
    pub rho_floor_wall: f64,
    pub rho_ceiling_wall: f64,
}

impl DaylightWindow {
    fn centre(&self) -> Point3D {
        add(
            add(self.corner, self.edge_w.scale(0.5)),
            self.edge_h.scale(0.5),
        )
    }

    fn phi_theta(&self) -> (f64, f64) {
        let n = self.normal;
        let phi = n.z.clamp(-1.0, 1.0).asin();
        let theta = if n.x.abs() > 1.0e-5 || n.y.abs() > 1.0e-5 {
            n.y.atan2(n.x)
        } else {
            0.0
        };
        (phi, theta)
    }

    fn tvis_at(&self, cos_b: f64) -> f64 {
        crate::glazing::polyf(cos_b, &self.tvis).max(0.0)
    }
}

fn add(p: Point3D, v: Vec3) -> Point3D {
    Point3D::new(p.x + v.x, p.y + v.y, p.z + v.z)
}

fn sub(a: Point3D, b: Point3D) -> Vec3 {
    Vec3::new(a.x - b.x, a.y - b.y, a.z - b.z)
}

/// A window from its polygon (three or four vertices, counter-clockwise seen
/// from outside). Four vertices are taken as a parallelogram on vertices 2–4
/// (E+ `W1`–`W3`); a triangle as the parallelogram's half.
pub fn window_geometry(verts: &[Point3D]) -> Option<(Point3D, Vec3, Vec3, Vec3, f64)> {
    if verts.len() < 3 {
        return None;
    }
    let normal = newell_normal(verts).normalize();
    let area = polygon_area(verts);
    let (w3, w2, w1) = if verts.len() >= 4 {
        (verts[1], verts[2], verts[3])
    } else {
        (verts[1], verts[2], verts[0])
    };
    Some((w2, sub(w3, w2), sub(w1, w2), normal, area))
}

/// A zone surface as the inter-reflection sees it: net area, visible
/// reflectance of its inside face, tilt [deg], and the base surface it
/// belongs to.
#[derive(Debug, Clone)]
pub struct InteriorSurface {
    pub area: f64,
    pub reflectance: f64,
    pub tilt: f64,
    /// Index of this surface's base surface (itself for a base surface).
    pub base: usize,
}

/// Area-weighted interior visible reflectance and each window's floor/wall
/// and ceiling/wall reflectances (E+ `DayltgAveInteriorReflectance`).
/// `windows` gives each window's base surface index and centre height;
/// `floor_z` is the zone's floor level (E+ zone origin z). Returns (total
/// area, average reflectance, per-window (ρ_fw, ρ_cw)).
pub fn interior_reflectances(
    surfaces: &[InteriorSurface],
    windows: &[(usize, f64)],
    floor_z: f64,
    floor_area: f64,
    volume: f64,
) -> (f64, f64, Vec<(f64, f64)>) {
    // Floor, wall, ceiling classes as E+: tilt > 10° and < 170° is a wall.
    let class = |tilt: f64| {
        if tilt >= 170.0 {
            0
        } else if tilt > 10.0 {
            1
        } else {
            2
        }
    };
    let mut ar = [0.0; 3];
    let mut arh = [0.0; 3];
    let (mut a_tot, mut arh_tot) = (0.0, 0.0);
    for s in surfaces {
        a_tot += s.area;
        arh_tot += s.area * s.reflectance;
        ar[class(s.tilt)] += s.area;
        arh[class(s.tilt)] += s.area * s.reflectance;
    }
    let rho_avg = if a_tot > 0.0 { arh_tot / a_tot } else { 0.0 };
    let per_window = windows
        .iter()
        .map(|&(base, win_z)| {
            // The window's base surface and everything on it are left out
            // (E+ `EnclAreaMinusThisSurf`); its class is by 45°/135°.
            let base_tilt = surfaces.get(base).map_or(90.0, |s| s.tilt);
            let base_class = if base_tilt >= 135.0 {
                0
            } else if base_tilt > 45.0 {
                1
            } else {
                2
            };
            let (mut a_b, mut arh_b) = (0.0, 0.0);
            for s in surfaces.iter().filter(|s| s.base == base) {
                a_b += s.area;
                arh_b += s.area * s.reflectance;
            }
            let mut ap = ar;
            let mut arhp = arh;
            ap[base_class] -= a_b;
            arhp[base_class] -= arh_b;
            let eta = ((win_z - floor_z) * floor_area / volume.max(1e-9)).clamp(0.0, 1.0);
            let rho_cw = (arhp[1] * (1.0 - eta) + arhp[2]) / (ap[1] * (1.0 - eta) + ap[2] + 1.0e-5);
            let rho_fw = (arhp[1] * eta + arhp[0]) / (ap[1] * eta + ap[0] + 1.0e-9);
            (rho_fw, rho_cw)
        })
        .collect();
    (a_tot, rho_avg, per_window)
}

// ─── Daylight factors ────────────────────────────────────────────────────────

/// Illuminance factors at one point from one window for one sun position:
/// per standard sky (illuminance over the sky's exterior horizontal
/// illuminance) and for the sun (over the beam's).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Factors {
    pub sky: [f64; N_SKY],
    pub sun: f64,
}

/// Number of window elements each way for a point at distance `alf` from the
/// window plane (E+ `FigureDayltgCoeffsAtPointsSetupForWindow`).
fn element_counts(width: f64, height: f64, alf: f64) -> (usize, usize) {
    let (mut nx, mut ny) = (40usize, 40usize);
    if alf > 0.1524 {
        nx = 1 + (4.0 * width / alf) as usize;
        ny = 1 + (4.0 * height / alf) as usize;
    }
    (nx.min(40), ny.min(40))
}

/// Whether the ray from `p` in direction `d` passes through the window.
fn ray_hits_window(win: &DaylightWindow, p: Point3D, d: Vec3) -> bool {
    let denom = win.normal.dot(&d);
    if denom.abs() < 1e-12 {
        return false;
    }
    let t = win.normal.dot(&sub(win.corner, p)) / denom;
    if t <= 0.0 {
        return false;
    }
    let hit = add(p, d.scale(t));
    let r = sub(hit, win.corner);
    let (ww, hh) = (win.edge_w.dot(&win.edge_w), win.edge_h.dot(&win.edge_h));
    let a = r.dot(&win.edge_w) / ww;
    let b = r.dot(&win.edge_h) / hh;
    (0.0..=1.0).contains(&a) && (0.0..=1.0).contains(&b)
}

/// Direct illuminance factors at reference point `p` from `win` for the sun
/// at `sun`, before normalisation: per sky the illuminance for unit zenith
/// luminance, and the sun's for unit beam normal illuminance.
fn direct_illuminance(win: &DaylightWindow, p: Point3D, sun: &Sun) -> Factors {
    let mut f = Factors::default();
    let width = win.edge_w.magnitude();
    let height = win.edge_h.magnitude();
    let alf = win.normal.dot(&sub(win.centre(), p)).abs();
    let (nx, ny) = element_counts(width, height, alf);
    let (ux, uy) = (
        win.edge_w.scale(1.0 / nx as f64),
        win.edge_h.scale(1.0 / ny as f64),
    );
    let da = win.area / (nx * ny) as f64;
    for ix in 0..nx {
        for iy in 0..ny {
            let el = add(
                add(win.corner, ux.scale(ix as f64 + 0.5)),
                uy.scale(iy as f64 + 0.5),
            );
            let v = sub(el, p);
            let dis = v.magnitude();
            if dis <= 0.0 {
                continue;
            }
            let ray = v.scale(1.0 / dis);
            let cos_b = win.normal.dot(&ray);
            if cos_b <= 0.0 || ray.z <= 0.0 {
                continue;
            }
            let domega = da * cos_b / (dis * dis);
            let ph = ray.z.asin();
            let th = if ray.x.abs() > 1.0e-5 || ray.y.abs() > 1.0e-5 {
                ray.y.atan2(ray.x)
            } else {
                0.0
            };
            let tv = win.tvis_at(cos_b);
            for (sky, e) in f.sky.iter_mut().enumerate() {
                *e += sky_luminance(sky, sun, th, ph) * domega * ray.z * tv;
            }
        }
    }
    // The sun's beam, when the ray from the point to the sun crosses the window.
    let cos_i = win.normal.dot(&sun.dir);
    if cos_i > 0.0 && sun.dir.z > 0.0 && ray_hits_window(win, p, sun.dir) {
        f.sun = sun.dir.z * win.tvis_at(cos_i);
    }
    f
}

/// Internally reflected illuminance factors from `win`, the same at every
/// point of the zone, before normalisation (E+ `DayltgInterReflectedIllum`,
/// bare window, no obstructions).
fn reflected_illuminance(
    win: &DaylightWindow,
    sun: &Sun,
    hor_sky: &[f64; N_SKY],
    hor_sun: f64,
    total_area: f64,
    rho_avg: f64,
    ground_reflectance: f64,
) -> Factors {
    let (phi_w, theta_w) = win.phi_theta();
    let ph_min = (-FRAC_PI_2).max(phi_w - FRAC_PI_2);
    let ph_max = FRAC_PI_2.min(phi_w + FRAC_PI_2);
    let dph = (ph_max - ph_min) / NPHMAX as f64;
    let mut down = Factors::default();
    let mut up = Factors::default();
    for iph in 0..NPHMAX {
        let ph = ph_min + (iph as f64 + 0.5) * dph;
        let (sph, cph) = (ph.sin(), ph.cos());
        let (th_min, th_max) = if phi_w >= 0.0 {
            if ph >= FRAC_PI_2 - phi_w {
                (-PI, PI)
            } else {
                let a = (-(ph.tan()) * phi_w.tan()).clamp(-1.0, 1.0).acos().abs();
                (theta_w - a, theta_w + a)
            }
        } else if ph <= -phi_w - FRAC_PI_2 {
            (-PI, PI)
        } else {
            let a = (-(ph.tan()) * phi_w.tan()).clamp(-1.0, 1.0).acos().abs();
            (theta_w - a, theta_w + a)
        };
        let dth = (th_max - th_min) / NTHMAX as f64;
        let da = cph * dth * dph;
        for ith in 0..NTHMAX {
            let th = th_min + (ith as f64 + 0.5) * dth;
            let cos_b = sph * phi_w.sin() + cph * phi_w.cos() * (th - theta_w).cos();
            if cos_b < 0.0 {
                continue;
            }
            let tv = win.tvis_at(cos_b);
            let mut zsk = [0.0; N_SKY];
            let mut zsu = 0.0;
            if ph > 0.0 {
                for (sky, z) in zsk.iter_mut().enumerate() {
                    *z = sky_luminance(sky, sun, th, ph) * cos_b * da;
                }
            } else {
                for (sky, z) in zsk.iter_mut().enumerate() {
                    *z = hor_sky[sky] * ground_reflectance / PI * cos_b * da;
                }
                zsu = hor_sun * ground_reflectance / PI * cos_b * da;
            }
            // Light from above the horizon goes down first, from the ground up.
            let target = if ph > 0.0 { &mut down } else { &mut up };
            for (t, z) in target.sky.iter_mut().zip(zsk) {
                *t += z * tv;
            }
            target.sun += zsu * tv;
        }
    }
    // The sun's beam on the window goes down.
    let cos_sun = sun.dir.dot(&win.normal);
    if cos_sun > 0.0 && sun.dir.z > 0.0 {
        down.sun += cos_sun * win.tvis_at(cos_sun);
    }
    let k = win.area / (total_area * (1.0 - rho_avg)).max(1e-9);
    let mut f = Factors::default();
    for sky in 0..N_SKY {
        f.sky[sky] = (down.sky[sky] * win.rho_floor_wall + up.sky[sky] * win.rho_ceiling_wall) * k;
    }
    f.sun = (down.sun * win.rho_floor_wall + up.sun * win.rho_ceiling_wall) * k;
    f
}

// ─── Runtime ─────────────────────────────────────────────────────────────────

/// A zone's daylighting controls with their geometry and the day's factors.
#[derive(Debug, Clone)]
pub struct DaylightingControl {
    pub input: DaylightingControlInput,
    pub zone_index: usize,
    pub windows: Vec<DaylightWindow>,
    pub total_area: f64,
    pub rho_avg: f64,
    /// Ground visible reflectance (E+ `GndReflectanceForDayltg`).
    pub ground_reflectance: f64,
    /// Day of year the factors are for.
    day: Option<u32>,
    /// Per hour 1-24 (index 0 unused): per window, per reference point.
    factors: Vec<Vec<Vec<Factors>>>,
    /// Per hour: exterior horizontal illuminance of each standard sky.
    hor_sky: Vec<[f64; N_SKY]>,
    /// Daylight illuminance at each reference point this timestep [lux].
    pub illuminance: Vec<f64>,
    /// The zone's lighting power multiplier this timestep.
    pub power_reduction: f64,
}

impl DaylightingControl {
    pub fn new(
        input: DaylightingControlInput,
        zone_index: usize,
        windows: Vec<DaylightWindow>,
        total_area: f64,
        rho_avg: f64,
        ground_reflectance: f64,
    ) -> Self {
        let n = input.reference_points.len();
        Self {
            input,
            zone_index,
            windows,
            total_area,
            rho_avg,
            ground_reflectance,
            day: None,
            factors: Vec::new(),
            hor_sky: Vec::new(),
            illuminance: vec![0.0; n],
            power_reduction: 1.0,
        }
    }

    /// The day's factors for each hour's sun position at the end of the hour
    /// (E+ `CalcDayltgCoefficients` with `SurfSunCosHourly`); `sun_at(hour)`
    /// gives the sun direction then, `None` with the sun down.
    pub fn compute_day(&mut self, day: u32, sun_at: impl Fn(u32) -> Option<Vec3>) {
        if self.day == Some(day) {
            return;
        }
        self.day = Some(day);
        let nw = self.windows.len();
        let np = self.input.reference_points.len();
        self.factors = vec![vec![vec![Factors::default(); np]; nw]; 25];
        self.hor_sky = vec![[0.0; N_SKY]; 25];
        for hour in 1..=24u32 {
            let Some(dir) = sun_at(hour) else { continue };
            if dir.z < 1.0e-5 {
                continue;
            }
            let sun = Sun::from_dir(dir);
            let (hor_sky, hor_sun) = exterior_horizontal_illuminance(&sun);
            self.hor_sky[hour as usize] = hor_sky;
            for (iw, win) in self.windows.iter().enumerate() {
                let refl = reflected_illuminance(
                    win,
                    &sun,
                    &hor_sky,
                    hor_sun,
                    self.total_area,
                    self.rho_avg,
                    self.ground_reflectance,
                );
                for (ip, rp) in self.input.reference_points.iter().enumerate() {
                    let p = Point3D::new(rp.x, rp.y, rp.z);
                    let dir_f = direct_illuminance(win, p, &sun);
                    let f = &mut self.factors[hour as usize][iw][ip];
                    for (sky, &h) in hor_sky.iter().enumerate() {
                        f.sky[sky] = if h > MIN_HORIZONTAL_ILLUMINANCE {
                            (dir_f.sky[sky] + refl.sky[sky]) / h
                        } else {
                            0.0
                        };
                    }
                    f.sun = if hor_sun > MIN_HORIZONTAL_ILLUMINANCE {
                        (dir_f.sun + refl.sun) / (hor_sun + 0.0001)
                    } else {
                        0.0
                    };
                }
            }
        }
    }

    /// Daylight illuminance at each reference point and the zone's lighting
    /// power multiplier this timestep (E+ `DayltgInteriorIllum`,
    /// `DayltgElecLightingControl`): the factors of this hour (1-24) and the
    /// one before, weighted `weight_now`; `weather` `None` with the sun down.
    pub fn update(&mut self, hour: u32, weight_now: f64, weather: Option<DaylightWeather>) {
        let np = self.input.reference_points.len();
        self.illuminance = vec![0.0; np];
        if let (Some(w), false) = (weather, self.factors.is_empty()) {
            let h = hour.clamp(1, 24) as usize;
            let hp = if h == 1 { 24 } else { h - 1 };
            let wp = 1.0 - weight_now;
            let (s1, s2, wt) = sky_mix(w.clearness, w.brightness);
            let hs = |s: usize| weight_now * self.hor_sky[h][s] + wp * self.hor_sky[hp][s] + 0.001;
            let (h1, h2) = (hs(s1), hs(s2));
            let sky_fac = w.sky_illuminance / ((1.0 - wt) * h2 + wt * h1);
            for iw in 0..self.windows.len() {
                for ip in 0..np {
                    let fc = self.factors[h][iw][ip];
                    let fp = self.factors[hp][iw][ip];
                    let df = |s: usize| weight_now * fc.sky[s] + wp * fp.sky[s];
                    let dsun = weight_now * fc.sun + wp * fp.sun;
                    self.illuminance[ip] += dsun * w.sun_illuminance
                        + sky_fac * (df(s1) * wt * h1 + df(s2) * (1.0 - wt) * h2);
                }
            }
        }
        self.power_reduction = self.input.power_reduction(&self.illuminance);
    }
}

// ─── Envelope ────────────────────────────────────────────────────────────────

/// Visible reflectance of an opaque construction's inside face: one minus the
/// inside layer's visible absorptance (E+ `ReflectVisDiffBack`), 0.3 for
/// constructions without layers (E+'s default absorptance, 0.7).
fn inside_visible_reflectance(env: &BuildingEnvelope, construction: &str) -> f64 {
    let absorptance = env
        .constructions
        .get(construction)
        .and_then(|c| c.inside_material())
        .and_then(|m| env.materials.get(m))
        .map_or(0.7, |m| m.visible_absorptance);
    1.0 - absorptance
}

/// Name of the surface a zone's `k`-th internal mass (1-based) becomes.
pub(crate) fn internal_mass_name(zone: &str, k: usize) -> String {
    format!("{zone} IntMass {k}")
}

impl BuildingEnvelope {
    /// Set up the daylighting controls: each control's zone, its exterior
    /// windows and the zone's interior reflectances. Problems (an unknown
    /// zone, a window without vertices or glazing) are reported when the
    /// envelope initialises.
    pub fn set_daylighting(&mut self, inputs: &[DaylightingControlInput]) {
        let mut controls = Vec::new();
        for input in inputs {
            match self.build_daylighting(input) {
                Ok(c) => controls.push(c),
                Err(e) => self.geometry_errors.push(e),
            }
        }
        self.daylighting = controls;
    }

    fn build_daylighting(
        &self,
        input: &DaylightingControlInput,
    ) -> Result<DaylightingControl, String> {
        let ctx = format!("Daylighting control '{}'", input.name);
        let &zi = self
            .zone_index
            .get(&input.zone)
            .ok_or_else(|| format!("{ctx}: zone '{}' does not exist.", input.zone))?;
        if input.reference_points.is_empty() {
            return Err(format!("{ctx}: it needs at least one reference point."));
        }
        let zone = &self.zones[zi];
        // The zone's walls, floors, roofs, windows and doors; E+ leaves
        // internal mass out of the inter-reflection.
        let internal_mass: Vec<String> = (1..=zone.input.internal_mass.len())
            .map(|k| internal_mass_name(&input.zone, k))
            .collect();
        let in_zone: Vec<usize> = (0..self.surfaces.len())
            .filter(|&i| self.surfaces[i].input.zone == input.zone)
            .filter(|&i| !internal_mass.contains(&self.surfaces[i].input.name))
            .collect();
        let local = |name: &str| {
            in_zone
                .iter()
                .position(|&i| self.surfaces[i].input.name == name)
        };
        let mut interior = Vec::with_capacity(in_zone.len());
        for (k, &i) in in_zone.iter().enumerate() {
            let s = &self.surfaces[i];
            let reflectance = match (&s.glazing, s.is_window) {
                (Some(g), true) => g.optics.rho_vis_diffuse_back,
                (None, true) => 0.0,
                _ => inside_visible_reflectance(self, &s.input.construction),
            };
            let base = s
                .input
                .parent_surface
                .as_deref()
                .and_then(local)
                .unwrap_or(k);
            interior.push(InteriorSurface {
                area: s.net_area,
                reflectance,
                tilt: s.input.tilt,
                base,
            });
        }
        // The zone's floor level (E+ uses the zone origin's height): its
        // lowest floor vertex, else its lowest vertex.
        let lowest = |floors_only: bool| {
            in_zone
                .iter()
                .map(|&i| &self.surfaces[i].input)
                .filter(|s| !floors_only || s.surface_type == SurfaceType::Floor)
                .filter_map(|s| s.vertices.as_ref())
                .flat_map(|v| v.iter().map(|p| p.z))
                .fold(f64::INFINITY, f64::min)
        };
        let floor_z = [lowest(true), lowest(false), 0.0]
            .into_iter()
            .find(|z| z.is_finite())
            .unwrap_or(0.0);
        let mut windows = Vec::new();
        let mut window_bases = Vec::new();
        for (k, &i) in in_zone.iter().enumerate() {
            let s = &self.surfaces[i];
            if !s.is_window || s.input.boundary != BoundaryCondition::Outdoor {
                continue;
            }
            let verts = s.input.vertices.as_deref().unwrap_or(&[]);
            let (corner, edge_w, edge_h, normal, area) =
                window_geometry(verts).ok_or_else(|| {
                    format!(
                        "{ctx}: window '{}' needs vertices for the daylight calculation.",
                        s.input.name
                    )
                })?;
            let glazing = s
                .glazing
                .as_ref()
                .ok_or_else(|| format!("{ctx}: window '{}' has no glazing.", s.input.name))?;
            let centre_z = verts.iter().map(|p| p.z).sum::<f64>() / verts.len() as f64;
            window_bases.push((interior[k].base, centre_z));
            windows.push(DaylightWindow {
                name: s.input.name.clone(),
                corner,
                edge_w,
                edge_h,
                area,
                normal,
                tvis: glazing.optics.tau_vis_beam,
                rho_floor_wall: 0.0,
                rho_ceiling_wall: 0.0,
            });
        }
        let (total_area, rho_avg, per_window) = interior_reflectances(
            &interior,
            &window_bases,
            floor_z,
            zone.input.floor_area,
            zone.input.volume,
        );
        for (w, (fw, cw)) in windows.iter_mut().zip(per_window) {
            w.rho_floor_wall = fw;
            w.rho_ceiling_wall = cw;
        }
        Ok(DaylightingControl::new(
            input.clone(),
            zi,
            windows,
            total_area,
            rho_avg,
            self.ground_reflectance,
        ))
    }

    /// Daylight illuminance and each daylit zone's lighting multiplier this
    /// timestep. Runs once per timestep, whichever of the timestep's
    /// calculations asks first.
    pub fn update_daylighting(&mut self, ctx: &SimulationContext, weather: &WeatherHour) {
        if self.daylighting.is_empty() {
            return;
        }
        let ts = &ctx.timestep;
        let doy = ts.day_of_year();
        let key = (doy, ts.hour, ts.sub_hour, ctx.is_sizing);
        if self.daylighting_step == Some(key) {
            return;
        }
        self.daylighting_step = Some(key);
        let (tz, lon, lat) = (self.time_zone, self.longitude, self.latitude);
        let sun_at = |clock_hour: f64| {
            let pos =
                solar::solar_position(doy, solar::local_solar_hour(clock_hour, tz, lon, doy), lat);
            // `sun_direction_vector` points from the sun to the ground.
            pos.is_sunup
                .then(|| solar::sun_direction_vector(&pos).scale(-1.0))
        };
        let now = solar::solar_position(
            doy,
            solar::local_solar_hour(ts.sun_hour(), tz, lon, doy),
            lat,
        );
        let light = daylight_weather(
            weather.direct_normal_rad,
            weather.diffuse_horiz_rad,
            if now.is_sunup { now.altitude } else { -1.0 },
            weather.dew_point,
            self.elevation,
            ts.month,
        );
        let weight_now = ts.sub_hour as f64 / ts.timesteps_per_hour.max(1) as f64;
        for z in &mut self.zones {
            z.daylight_lights_factor = 1.0;
        }
        for c in &mut self.daylighting {
            c.compute_day(doy, |h| sun_at(h as f64));
            c.update(ts.hour, weight_now, light);
            self.zones[c.zone_index].daylight_lights_factor *= c.power_reduction;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sun_at(alt_deg: f64, az_from_east_deg: f64) -> Sun {
        let (a, t) = (alt_deg.to_radians(), az_from_east_deg.to_radians());
        Sun::from_dir(Vec3::new(a.cos() * t.cos(), a.cos() * t.sin(), a.sin()))
    }

    /// Every sky's luminance is normalised to 1 at the zenith, and the CIE
    /// overcast sky's horizontal illuminance is 7π/9 of the zenith luminance.
    #[test]
    fn skies_are_normalised_to_the_zenith() {
        let sun = sun_at(40.0, 30.0);
        for sky in 0..N_SKY {
            let l = sky_luminance(sky, &sun, 0.3, FRAC_PI_2 - 1e-9);
            assert!((l - 1.0).abs() < 0.02, "sky {sky}: {l}");
        }
        let (hi, hsun) = exterior_horizontal_illuminance(&sun);
        assert!(
            (hi[OVERCAST] - 7.0 * PI / 9.0).abs() < 0.02,
            "{}",
            hi[OVERCAST]
        );
        assert!((hsun - 40f64.to_radians().sin()).abs() < 1e-12);
    }

    /// E+ `DayltgElecLightingControl`: ContinuousOff switches off at the
    /// minimum light output (daylight 80 % of the setpoint), continuous holds
    /// the minimum power there, and dims linearly above.
    #[test]
    fn lighting_control_follows_eplus() {
        let ctl = |c| DaylightingControlInput {
            name: "c".into(),
            zone: "z".into(),
            lighting_control: c,
            minimum_input_power_fraction: 0.2,
            minimum_light_output_fraction: 0.2,
            stepped_control_steps: 3,
            reference_points: vec![
                ReferencePointInput {
                    x: 0.0,
                    y: 0.0,
                    z: 0.0,
                    fraction_of_zone: 0.25,
                    illuminance_setpoint: 538.0,
                },
                ReferencePointInput {
                    x: 0.0,
                    y: 0.0,
                    z: 0.0,
                    fraction_of_zone: 0.25,
                    illuminance_setpoint: 538.0,
                },
            ],
        };
        let off = ctl(LightingControl::ContinuousOff);
        assert_eq!(off.power_fraction(450.0, 538.0), 0.0);
        assert_eq!(
            ctl(LightingControl::Continuous).power_fraction(450.0, 538.0),
            0.2
        );
        assert_eq!(off.power_fraction(0.0, 538.0), 1.0);
        // Half the setpoint: FL = 0.5, FP = (0.5 + 0.5·0.2 − 0.2)/0.8 = 0.5.
        assert!((off.power_fraction(269.0, 538.0) - 0.5).abs() < 1e-12);
        // Two points at a quarter of the zone each, both off: half the power.
        assert!((off.power_reduction(&[600.0, 600.0]) - 0.5).abs() < 1e-12);
        let st = ctl(LightingControl::Stepped);
        assert!((st.power_fraction(269.0, 538.0) - 2.0 / 3.0).abs() < 1e-12);
    }

    /// A skylight straight above a point: the direct overcast illuminance for
    /// unit zenith luminance is about L·τ·A/d² for a small skylight.
    #[test]
    fn direct_illuminance_under_a_skylight() {
        let win = DaylightWindow {
            name: "sky".into(),
            corner: Point3D::new(-0.5, -0.5, 5.0),
            edge_w: Vec3::new(1.0, 0.0, 0.0),
            edge_h: Vec3::new(0.0, 1.0, 0.0),
            area: 1.0,
            normal: Vec3::new(0.0, 0.0, 1.0),
            tvis: [0.5, 0.0, 0.0, 0.0, 0.0, 0.0],
            rho_floor_wall: 0.3,
            rho_ceiling_wall: 0.5,
        };
        let sun = sun_at(60.0, 90.0);
        let f = direct_illuminance(&win, Point3D::new(0.0, 0.0, 0.0), &sun);
        let expect = 1.0 * 0.5 * 1.0 / 25.0;
        assert!(
            (f.sky[OVERCAST] - expect).abs() / expect < 0.02,
            "{}",
            f.sky[OVERCAST]
        );
        // The sun at 60° does not shine through the skylight onto the point.
        assert_eq!(f.sun, 0.0);
        let overhead = sun_at(89.9, 90.0);
        assert!(direct_illuminance(&win, Point3D::new(0.0, 0.0, 0.0), &overhead).sun > 0.49);
    }

    /// Perez sky clearness of an overcast sky is about 1 and the sky is the
    /// overcast/intermediate mix; a clear sky has high clearness.
    #[test]
    fn perez_clearness_and_efficacy() {
        let alt = 50f64.to_radians();
        let overcast = daylight_weather(0.0, 200.0, alt, 5.0, 1726.0, 6).unwrap();
        assert!((overcast.clearness - 1.0).abs() < 1e-3);
        assert!(
            overcast.sky_illuminance > 200.0 * 100.0 && overcast.sky_illuminance < 200.0 * 140.0
        );
        let clear = daylight_weather(850.0, 100.0, alt, 5.0, 1726.0, 6).unwrap();
        assert!(clear.clearness > 6.0);
        assert_eq!(sky_mix(clear.clearness, clear.brightness).0, CLEAR);
        assert!(daylight_weather(0.0, 0.0, -0.1, 0.0, 0.0, 1).is_none());
    }
}
