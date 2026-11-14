//! Glazing systems: layer optics, gap gases and the window face heat balance.
//!
//! A glazing system is one to four glass layers with a gas gap between each
//! pair. Each layer has two faces, and the face temperatures are solved
//! together from their heat balances, as EnergyPlus does
//! (`WindowManager::SolveForWindowTemperatures`). A simple glazing system
//! (U-factor and SHGC) becomes EnergyPlus's equivalent single layer
//! (`MaterialGlass::SetupSimpleWindowGlazingSystem`).
//!
//! Optics follow `WindowManager::InitGlassOpticalCalculations` for
//! spectral-average layers: each layer's properties at ten incidence angles
//! (`TransAndReflAtPhi`), the layers combined with their inter-reflections
//! (`SystemPropertiesAtLambdaAndPhi`), sixth-order fits in cos θ (`W5LsqFit`)
//! and hemispherical averages (`DiffuseAverage`).

const STEFAN_BOLTZMANN: f64 = 5.6704e-8;
const KELVIN: f64 = 273.15;

/// Most glass layers in a glazing system (EnergyPlus's limit).
pub const MAX_LAYERS: usize = 4;
/// Incidence angles of the optical tables: 0°, 10°, …, 90°.
const N_PHI: usize = 10;

// ─── Gases ──────────────────────────────────────────────────────────────────

/// A fill gas: conductivity [W/(m·K)], dynamic viscosity [Pa·s] and specific
/// heat [J/(kg·K)] as quadratics in temperature [K], and molecular weight
/// [kg/kmol]. Coefficients from ISO 15099 Annex B as EnergyPlus holds them
/// (`Material::gases`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Gas {
    pub conductivity: [f64; 3],
    pub viscosity: [f64; 3],
    pub specific_heat: [f64; 3],
    pub molecular_weight: f64,
}

pub const AIR: Gas = Gas {
    conductivity: [2.873e-3, 7.760e-5, 0.0],
    viscosity: [3.723e-6, 4.940e-8, 0.0],
    specific_heat: [1002.737, 1.2324e-2, 0.0],
    molecular_weight: 28.97,
};
pub const ARGON: Gas = Gas {
    conductivity: [2.285e-3, 5.149e-5, 0.0],
    viscosity: [3.379e-6, 6.451e-8, 0.0],
    specific_heat: [521.929, 0.0, 0.0],
    molecular_weight: 39.948,
};
pub const KRYPTON: Gas = Gas {
    conductivity: [9.443e-4, 2.826e-5, 0.0],
    viscosity: [2.213e-6, 7.777e-8, 0.0],
    specific_heat: [248.091, 0.0, 0.0],
    molecular_weight: 83.8,
};
pub const XENON: Gas = Gas {
    conductivity: [4.538e-4, 1.723e-5, 0.0],
    viscosity: [1.069e-6, 7.414e-8, 0.0],
    specific_heat: [158.340, 0.0, 0.0],
    molecular_weight: 131.3,
};

fn quadratic(c: &[f64; 3], t: f64) -> f64 {
    c[0] + c[1] * t + c[2] * t * t
}

/// Gas properties at one temperature.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GasProperties {
    /// Thermal conductivity [W/(m·K)]
    pub conductivity: f64,
    /// Dynamic viscosity [Pa·s]
    pub viscosity: f64,
    /// Specific heat [J/(kg·K)]
    pub specific_heat: f64,
    /// Density [kg/m³]
    pub density: f64,
}

/// The gas in a gap: one gas or a mixture, as mole fractions.
#[derive(Debug, Clone, PartialEq)]
pub struct GasFill {
    pub parts: Vec<(Gas, f64)>,
}

impl GasFill {
    pub fn single(gas: Gas) -> Self {
        Self {
            parts: vec![(gas, 1.0)],
        }
    }

    /// Properties at mean temperature `t` [K], with ISO 15099 §5.3.2 mixing
    /// rules (eqs. 56–68) as EnergyPlus applies them
    /// (`WindowManager::WindowGasConductance`): the gap gas is at 1 × 10⁵ Pa.
    pub fn properties(&self, t: f64) -> GasProperties {
        const PRESSURE: f64 = 1.0e5;
        const GAS_LAW: f64 = 8314.51;
        let n = self.parts.len();
        let con: Vec<f64> = self
            .parts
            .iter()
            .map(|(g, _)| quadratic(&g.conductivity, t))
            .collect();
        let vis: Vec<f64> = self
            .parts
            .iter()
            .map(|(g, _)| quadratic(&g.viscosity, t))
            .collect();
        let cp: Vec<f64> = self
            .parts
            .iter()
            .map(|(g, _)| quadratic(&g.specific_heat, t))
            .collect();
        if n == 1 {
            let w = self.parts[0].0.molecular_weight;
            return GasProperties {
                conductivity: con[0],
                viscosity: vis[0],
                specific_heat: cp[0],
                density: PRESSURE * w / (GAS_LAW * t),
            };
        }
        let w: Vec<f64> = self.parts.iter().map(|(g, _)| g.molecular_weight).collect();
        let x: Vec<f64> = self.parts.iter().map(|(_, f)| *f).collect();
        let mol_mix: f64 = (0..n).map(|i| x[i] * w[i]).sum();
        let cp_mix_m: f64 = (0..n).map(|i| x[i] * cp[i] * w[i]).sum();
        // Monatomic (k′) and internal-energy (k″) parts of each conductivity.
        let k1: Vec<f64> = (0..n).map(|i| 3.75 * GAS_LAW / w[i] * vis[i]).collect();
        let k2: Vec<f64> = (0..n).map(|i| con[i] - k1[i]).collect();
        let (mut mu_mix, mut k1_mix, mut k2_mix) = (0.0, 0.0, 0.0);
        for i in 0..n {
            let (mut mu_dn, mut k1_dn, mut k2_dn) = (1.0, 1.0, 1.0);
            for j in 0..n {
                if i == j {
                    continue;
                }
                let down = 2.0 * 2.0_f64.sqrt() * (1.0 + w[i] / w[j]).sqrt();
                let phi_mu = (1.0 + (vis[i] / vis[j]).sqrt() * (w[j] / w[i]).powf(0.25)).powi(2);
                let psi_k = (1.0 + (k1[i] / k1[j]).sqrt() * (w[i] / w[j]).powf(0.25)).powi(2);
                let psi_term =
                    1.0 + 2.41 * (w[i] - w[j]) * (w[i] - 0.142 * w[j]) / (w[i] + w[j]).powi(2);
                let ratio = x[j] / x[i];
                mu_dn += phi_mu / down * ratio;
                k1_dn += psi_k * psi_term / down * ratio;
                k2_dn += psi_k / down * ratio;
            }
            mu_mix += vis[i] / mu_dn;
            k1_mix += k1[i] / k1_dn;
            k2_mix += k2[i] / k2_dn;
        }
        GasProperties {
            conductivity: k1_mix + k2_mix,
            viscosity: mu_mix,
            specific_heat: cp_mix_m / mol_mix,
            density: PRESSURE * mol_mix / (GAS_LAW * t),
        }
    }
}

/// Nusselt number of a sealed gas cavity, ISO 15099 §5.3.3.3 as EnergyPlus
/// codes it (`WindowManager::NusseltNumber`).
///
/// - `ra`: Rayleigh number of the gap;
/// - `aspect`: cavity height over gap width;
/// - `tilt_deg`: tilt of the window (0° facing up, 90° vertical, 180° facing down);
/// - `heated_from_above`: the upper face of the cavity is the warmer one.
///
/// For a window facing upward (tilt < 90°) the outer face is the upper one,
/// which is the only case EnergyPlus considers; for a window facing downward
/// the caller passes the inner face as the upper one and the supplementary
/// tilt 180° − tilt.
pub fn cavity_nusselt(ra: f64, aspect: f64, tilt_deg: f64, heated_from_above: bool) -> f64 {
    let tilt_r = tilt_deg.to_radians();
    let nu90_1 = if ra <= 1.0e4 {
        1.0 + 1.7596678e-10 * ra.powf(2.2984755) // eq. 51
    } else if ra <= 5.0e4 {
        0.028154 * ra.powf(0.4134) // eq. 50
    } else {
        0.0673838 * ra.powf(1.0 / 3.0) // eq. 49
    };
    let nu90_2 = 0.242 * (ra / aspect).powf(0.272); // eq. 52
    let nu90 = nu90_1.max(nu90_2);
    if heated_from_above {
        return 1.0 + (nu90 - 1.0) * tilt_r.sin(); // eq. 53
    }
    if tilt_deg >= 60.0 {
        let g = if ra >= 0.001 {
            0.5 * (1.0 + (ra / 3160.0).powf(20.6)).powf(-0.1) // eq. 47
        } else {
            0.5
        };
        let nu60_1 = (1.0 + (0.0936 * ra.powf(0.314) / (1.0 + g)).powi(7)).powf(1.0 / 7.0); // eq. 45
        let nu60_2 = (0.104 + 0.175 / aspect) * ra.powf(0.283); // eq. 46
        let nu60 = nu60_1.max(nu60_2);
        ((90.0 - tilt_deg) * nu60 + (tilt_deg - 60.0) * nu90) / 30.0
    } else {
        // eq. 42 (Hollands et al.)
        let cra = ra * tilt_r.cos();
        let a = 1.0 - 1708.0 / cra;
        let b = (cra / 5830.0).powf(0.33333) - 1.0;
        let ang = 1708.0 * (1.8 * tilt_r).sin().powf(1.6);
        1.0 + 1.44 * a.max(0.0) * (1.0 - ang / cra) + b.max(0.0)
    }
}

/// A gas gap between two glass layers.
#[derive(Debug, Clone, PartialEq)]
pub struct Gap {
    /// Width [m]
    pub width: f64,
    pub fill: GasFill,
}

impl Gap {
    /// Convective/conductive conductance of the gap [W/(m²·K)] between faces
    /// at `t_outer` and `t_inner` [K] (`WindowGasConductance` +
    /// `NusseltNumber`), for a window of `height` [m] at `tilt_deg`.
    pub fn conductance(&self, t_outer: f64, t_inner: f64, height: f64, tilt_deg: f64) -> f64 {
        let t_mean = 0.5 * (t_outer + t_inner);
        let p = self.fill.properties(t_mean);
        let pr = p.specific_heat * p.viscosity / p.conductivity;
        let gr = 9.807 * self.width.powi(3) * (t_outer - t_inner).abs() * p.density.powi(2)
            / (t_mean * p.viscosity.powi(2));
        let aspect = height / self.width;
        // The outer face is the cavity's upper face when the window faces
        // up; when it faces down (tilt > 90°) the inner face is.
        let (tilt, heated_from_above) = if tilt_deg <= 90.0 {
            (tilt_deg, t_outer > t_inner)
        } else {
            (180.0 - tilt_deg, t_inner > t_outer)
        };
        cavity_nusselt(gr * pr, aspect, tilt, heated_from_above) * p.conductivity / self.width
    }
}

// ─── Layers and optics ──────────────────────────────────────────────────────

/// One glass layer, with spectral-average properties at normal incidence.
#[derive(Debug, Clone, PartialEq)]
pub struct GlassLayer {
    /// Thickness [m]
    pub thickness: f64,
    /// Conductivity [W/(m·K)]
    pub conductivity: f64,
    pub solar_transmittance: f64,
    pub solar_reflectance_front: f64,
    pub solar_reflectance_back: f64,
    pub visible_transmittance: f64,
    pub visible_reflectance_front: f64,
    pub visible_reflectance_back: f64,
    /// Long-wave emissivity of the outdoor-facing face
    pub emissivity_front: f64,
    /// Long-wave emissivity of the room-facing face
    pub emissivity_back: f64,
    /// Dirt correction on the transmittance of the outermost layer (1 = clean)
    pub dirt_factor: f64,
}

/// Simple glazing ratings that select EnergyPlus's angular curves.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SimpleRating {
    pub u_factor: f64,
    pub shgc: f64,
}

/// Single-layer transmittance and front/back reflectance at incidence cosine
/// `cs` from the normal-incidence values (`WindowManager::TransAndReflAtPhi`).
fn trans_refl_at_phi(
    cs: f64,
    tf0: f64,
    rf0: f64,
    rb0: f64,
    simple: Option<SimpleRating>,
) -> (f64, f64, f64) {
    if let Some(r) = simple {
        let (t, rr) = if cs == 1.0 {
            (1.0, 0.0)
        } else {
            simple_glazing_curves(cs, r.u_factor, r.shgc)
        };
        let tfp = (tf0 * t).clamp(0.0, 1.0);
        let rfp = (rf0 * (1.0 - rr) + rr).min(0.9999 - tfp).max(0.0);
        return (tfp, rfp, rfp);
    }
    if tf0 <= 0.0 {
        return (0.0, rf0, rb0);
    }
    let beta_f = tf0 * tf0 - rf0 * rf0 + 2.0 * rf0 + 1.0;
    let beta_b = tf0 * tf0 - rb0 * rb0 + 2.0 * rb0 + 1.0;
    let r0f = (beta_f - (beta_f * beta_f - 4.0 * (2.0 - rf0) * rf0).sqrt()) / (2.0 * (2.0 - rf0));
    let r0b = (beta_b - (beta_b * beta_b - 4.0 * (2.0 - rb0) * rb0).sqrt()) / (2.0 * (2.0 - rb0));
    let test = if (r0f - r0b).abs() != 0.0 {
        (r0f - r0b).abs() / (r0f + r0b)
    } else {
        0.0
    };
    if test < 0.001 {
        // Uncoated glass: Fresnel interfaces with the index of refraction and
        // absorption implied by the normal-incidence values.
        let abf = if r0f * tf0 != 0.0 {
            (r0f * tf0 / (rf0 - r0f)).ln()
        } else {
            0.0
        };
        let abb = if r0b * tf0 != 0.0 {
            (r0b * tf0 / (rb0 - r0b)).ln()
        } else {
            0.0
        };
        let ngf = (1.0 + r0f.sqrt()) / (1.0 - r0f.sqrt());
        let ngb = (1.0 + r0b.sqrt()) / (1.0 - r0b.sqrt());
        let cgf = (1.0 - (1.0 - cs * cs) / (ngf * ngf)).sqrt();
        let cgb = (1.0 - (1.0 - cs * cs) / (ngb * ngb)).sqrt();
        let refl = |num: f64, den: f64| {
            if num != 0.0 {
                (num / den).powi(2)
            } else {
                0.0
            }
        };
        let rpf1 = refl(ngf * cs - cgf, ngf * cs + cgf);
        let rpf2 = refl(ngf * cgf - cs, ngf * cgf + cs);
        let rpb1 = refl(ngb * cs - cgb, ngb * cs + cgb);
        let rpb2 = refl(ngb * cgb - cs, ngb * cgb + cs);
        let (tpf1, tpf2) = (1.0 - rpf1, 1.0 - rpf2);
        let e1 = if cgf != 0.0 { (-abf / cgf).exp() } else { 0.0 };
        let e2 = if cgf != 0.0 {
            (-2.0 * abf / cgf).exp()
        } else {
            0.0
        };
        let tfp1 = if tpf1 != 0.0 {
            tpf1 * tpf1 * e1 / (1.0 - rpf1 * rpf1 * e2)
        } else {
            0.0
        };
        let tfp2 = if tpf2 != 0.0 {
            tpf2 * tpf2 * e1 / (1.0 - rpf2 * rpf2 * e2)
        } else {
            0.0
        };
        let rfp1 = rpf1 * (1.0 + tfp1 * e1);
        let rfp2 = rpf2 * (1.0 + tfp2 * e1);
        let eb = if abb != 0.0 { (-abb / cgb).exp() } else { 0.0 };
        let rbp1 = rpb1 * (1.0 + tfp1 * eb);
        let rbp2 = rpb2 * (1.0 + tfp2 * eb);
        (
            0.5 * (tfp1 + tfp2),
            0.5 * (rfp1 + rfp2),
            0.5 * (rbp1 + rbp2),
        )
    } else {
        // Coated glass: normalised clear or bronze angular distributions.
        let (t, r) = if tf0 > 0.645 {
            if cs > 0.999 {
                (1.0, 0.0)
            } else if cs < 0.001 {
                (0.0, 1.0)
            } else {
                let t = -0.0015 + (3.355 + (-3.840 + (1.460 + 0.0288 * cs) * cs) * cs) * cs;
                let r = 0.999 + (-0.563 + (2.043 + (-2.532 + 1.054 * cs) * cs) * cs) * cs - t;
                (t, r)
            }
        } else if cs > 0.999 {
            (1.0, 0.0)
        } else if cs < 0.001 {
            (0.0, 1.0)
        } else {
            let t = -0.002 + (2.813 + (-2.341 + (-0.05725 + 0.599 * cs) * cs) * cs) * cs;
            let r = 0.997 + (-1.868 + (6.513 + (-7.862 + 3.225 * cs) * cs) * cs) * cs - t;
            (t, r)
        };
        (tf0 * t, rf0 * (1.0 - r) + r, rb0 * (1.0 - r) + r)
    }
}

/// EnergyPlus's simple glazing angular curves A–J and their interpolation
/// over 28 U/SHGC cells (`TransAndReflAtPhi`, simple glazing branch;
/// Arasteh et al. 2009). Returns the normalised (transmittance, reflectance)
/// factors at incidence cosine `cs`.
// EnergyPlus's curve F has a coefficient of 3.14, not π.
#[allow(clippy::approx_constant)]
fn simple_glazing_curves(cs: f64, u: f64, shgc: f64) -> (f64, f64) {
    let (c2, c3, c4) = (cs * cs, cs * cs * cs, cs * cs * cs * cs);
    let t = |a: f64, b: f64, c: f64, d: f64| a * cs + b * c2 + c * c3 + d * c4;
    let ta = t(3.36, -3.85, 1.49, 0.01);
    let tb = t(2.83, -2.42, 0.04, 0.55);
    let tc = t(2.45, -1.58, -0.64, 0.77);
    let td = t(2.85, -2.58, 0.40, 0.35);
    let te = t(1.51, 2.49, -5.87, 2.88);
    let tf = t(1.21, 3.14, -6.37, 3.03);
    let tg = t(1.09, 3.54, -6.84, 3.23);
    let th = t(0.98, 3.83, -7.13, 3.33);
    let ti = t(0.79, 3.93, -6.86, 3.15);
    let tj = t(0.08, 6.02, -8.84, 3.74);
    let r = |a: f64, b: f64, c: f64, d: f64, tr: f64| 1.0 + a * cs + b * c2 + c * c3 + d * c4 - tr;
    let ra = r(-0.70, 2.57, -3.20, 1.33, ta);
    let rb = r(-1.87, 6.50, -7.86, 3.23, tb);
    let rc = r(-2.52, 8.40, -9.86, 3.99, tc);
    let rd = r(-1.85, 6.40, -7.64, 3.11, td);
    let re = r(-1.57, 5.60, -6.82, 2.80, te);
    let rf = r(-3.15, 10.98, -13.14, 5.32, tf);
    let rg = r(-3.25, 11.32, -13.54, 5.49, tg);
    let rh = r(-3.39, 11.70, -13.94, 5.64, th);
    let ri = r(-4.06, 13.55, -15.74, 6.27, ti);
    let rj = r(-4.35, 14.27, -16.32, 6.39, tj);
    let t_fghi = (tf + tg + th + ti) / 4.0;
    let r_fghi = (rf + rg + rh + ri) / 4.0;
    let t_fh = (tf + th) / 2.0;
    let r_fh = (rf + rh) / 2.0;
    let t_bdcd = (tb + td + tc + td) / 4.0;
    let r_bdcd = (rb + rd + rc + rd) / 4.0;
    // Curves as (transmittance, reflectance) pairs.
    let (a, d, e, j) = ((ta, ra), (td, rd), (te, re), (tj, rj));
    let (fghi, fh, bdcd) = ((t_fghi, r_fghi), (t_fh, r_fh), (t_bdcd, r_bdcd));
    let two = |x: f64, x0: f64, x1: f64, f0: (f64, f64), f1: (f64, f64)| {
        let s = (x - x0) / (x1 - x0);
        (f0.0 + s * (f1.0 - f0.0), f0.1 + s * (f1.1 - f0.1))
    };
    // E+ InterpolateBetweenFourValues(U, SHGC, U1, U2, S1, S2, F(U1,S1), F(U1,S2), F(U2,S1), F(U2,S2)).
    #[allow(clippy::too_many_arguments)]
    fn four(
        x: f64,
        y: f64,
        x1: f64,
        x2: f64,
        y1: f64,
        y2: f64,
        f11: (f64, f64),
        f12: (f64, f64),
        f21: (f64, f64),
        f22: (f64, f64),
    ) -> (f64, f64) {
        let d = (x2 - x1) * (y2 - y1);
        let w11 = (x2 - x) * (y2 - y) / d;
        let w21 = (x - x1) * (y2 - y) / d;
        let w12 = (x2 - x) * (y - y1) / d;
        let w22 = (x - x1) * (y - y1) / d;
        (
            f11.0 * w11 + f21.0 * w21 + f12.0 * w12 + f22.0 * w22,
            f11.1 * w11 + f21.1 * w21 + f12.1 * w12 + f22.1 * w22,
        )
    }
    let s = shgc;
    if u < 1.4195 {
        if s > 0.45 {
            e
        } else if s >= 0.35 {
            two(s, 0.35, 0.45, j, e)
        } else {
            j
        }
    } else if u <= 1.7034 {
        let (u1, u2) = (1.4195, 1.7034);
        if s > 0.55 {
            e
        } else if s > 0.5 {
            four(u, s, u1, u2, 0.50, 0.55, e, e, fghi, e)
        } else if s > 0.45 {
            two(u, u1, u2, e, fghi)
        } else if s > 0.35 {
            four(u, s, u1, u2, 0.35, 0.45, j, e, fghi, fghi)
        } else if s > 0.3 {
            two(u, u1, u2, j, fghi)
        } else if s > 0.25 {
            four(u, s, u1, u2, 0.25, 0.3, j, j, fh, fghi)
        } else {
            two(u, u1, u2, j, fh)
        }
    } else if u < 3.4068 {
        if s > 0.55 {
            e
        } else if s >= 0.5 {
            two(s, 0.5, 0.55, fghi, e)
        } else if s > 0.3 {
            fghi
        } else if s >= 0.25 {
            two(s, 0.25, 0.30, fh, fghi)
        } else {
            fh
        }
    } else if u <= 4.5424 {
        let (u1, u2) = (3.4068, 4.5424);
        if s > 0.65 {
            two(u, u1, u2, e, a)
        } else if s > 0.6 {
            four(u, s, u1, u2, 0.6, 0.65, e, e, bdcd, a)
        } else if s > 0.55 {
            two(u, u1, u2, e, bdcd)
        } else if s > 0.5 {
            four(u, s, u1, u2, 0.5, 0.55, fghi, e, bdcd, bdcd)
        } else if s > 0.45 {
            two(u, u1, u2, fghi, bdcd)
        } else if s > 0.3 {
            four(u, s, u1, u2, 0.3, 0.45, fghi, fghi, d, bdcd)
        } else if s > 0.25 {
            four(u, s, u1, u2, 0.25, 0.3, fh, fghi, d, d)
        } else {
            two(u, u1, u2, fh, d)
        }
    } else if s > 0.65 {
        a
    } else if s >= 0.6 {
        two(s, 0.6, 0.65, bdcd, a)
    } else if s > 0.45 {
        bdcd
    } else if s >= 0.3 {
        two(s, 0.3, 0.45, d, bdcd)
    } else {
        d
    }
}

/// Transmittance, front and back reflectance and per-layer absorptance of a
/// stack of layers with their inter-reflections, at one angle
/// (`WindowManager::SystemPropertiesAtLambdaAndPhi`). `layers` are
/// (τ, ρ_front, ρ_back) from the side the light enters.
fn system_properties(layers: &[(f64, f64, f64)]) -> (f64, f64, f64, Vec<f64>) {
    let n = layers.len();
    if n == 1 {
        let (t, rf, rb) = layers[0];
        return (t, rf, rb, vec![1.0 - t - rf]);
    }
    let mut top = [[0.0; MAX_LAYERS]; MAX_LAYERS];
    let mut rfop = [[0.0; MAX_LAYERS]; MAX_LAYERS];
    let mut rbop = [[0.0; MAX_LAYERS]; MAX_LAYERS];
    for (i, &(t, rf, rb)) in layers.iter().enumerate() {
        top[i][i] = t;
        rfop[i][i] = rf;
        rbop[i][i] = rb;
    }
    for i in 0..n - 1 {
        for j in i + 1..n {
            let denom = 1.0 - rfop[j][j] * rbop[i][j - 1];
            if denom == 0.0 {
                top[j][i] = 0.0;
                rfop[j][i] = 1.0;
                rbop[i][j] = 1.0;
            } else {
                top[j][i] = top[j - 1][i] * top[j][j] / denom;
                rfop[j][i] = rfop[j - 1][i] + top[j - 1][i].powi(2) * rfop[j][j] / denom;
                rbop[i][j] = rbop[j][j] + top[j][j].powi(2) * rbop[i][j - 1] / denom;
            }
        }
    }
    let abs = (0..n)
        .map(|j| {
            let (t0, rb0) = if j == 0 {
                (1.0, 0.0)
            } else {
                (top[j - 1][0], rbop[0][j - 1])
            };
            let rf0 = if j == n - 1 { 0.0 } else { rfop[n - 1][j + 1] };
            let af = 1.0 - top[j][j] - rfop[j][j];
            let ab = 1.0 - top[j][j] - rbop[j][j];
            let d1 = 1.0 - rfop[n - 1][j] * rb0;
            let d2 = 1.0 - rbop[0][j] * rf0;
            if d1 == 0.0 || d2 == 0.0 {
                0.0
            } else {
                t0 * af / d1 + top[j][0] * rf0 * ab / d2
            }
        })
        .collect();
    (top[n - 1][0], rfop[n - 1][0], rbop[0][n - 1], abs)
}

/// Least-squares fit of c₁x + c₂x² + … + c₆x⁶ through the ten angles
/// (`WindowManager::W5LsqFit`).
fn lsq_fit(x: &[f64; N_PHI], y: &[f64; N_PHI]) -> [f64; 6] {
    let mut d = [[0.0; N_PHI]; 6];
    d[0] = *x;
    for i in 1..6 {
        for m in 0..N_PHI {
            d[i][m] = d[i - 1][m] * x[m];
        }
    }
    let mut a = [[0.0; 6]; 6];
    let mut b = [0.0; 6];
    for i in 0..6 {
        b[i] = (0..N_PHI).map(|m| y[m] * d[i][m]).sum();
        for j in 0..6 {
            a[i][j] = (0..N_PHI).map(|m| d[i][m] * d[j][m]).sum();
        }
    }
    for k in 0..5 {
        for i in k + 1..6 {
            let f = a[i][k] / a[k][k];
            b[i] -= b[k] * f;
            let row_k = a[k];
            for (aij, akj) in a[i][k..].iter_mut().zip(&row_k[k..]) {
                *aij -= akj * f;
            }
        }
    }
    let mut c = [0.0; 6];
    for l in (0..6).rev() {
        let s: f64 = (l + 1..6).map(|j| a[l][j] * c[j]).sum();
        c[l] = (b[l] - s) / a[l][l];
    }
    c
}

/// Evaluates a fit from [`lsq_fit`] at incidence cosine `x`; zero outside
/// [0, 1] (EnergyPlus `POLYF`).
pub fn polyf(x: f64, c: &[f64; 6]) -> f64 {
    if !(0.0..=1.0).contains(&x) {
        return 0.0;
    }
    x * (c[0] + x * (c[1] + x * (c[2] + x * (c[3] + x * (c[4] + x * c[5])))))
}

/// Hemispherical average of a property tabulated at 0°, 10°, …, 90°:
/// ∫ X(φ) sin 2φ dφ by the trapezoidal rule (`WindowManager::DiffuseAverage`).
fn diffuse_average(p: &[f64; N_PHI]) -> f64 {
    let d = 10.0_f64.to_radians();
    let avg: f64 = (0..N_PHI - 1)
        .map(|i| {
            0.5 * d
                * (p[i] * (2.0 * i as f64 * d).sin() + p[i + 1] * (2.0 * (i + 1) as f64 * d).sin())
        })
        .sum();
    avg.max(0.0)
}

/// Solar and visible optics of a glazing system.
#[derive(Debug, Clone, PartialEq)]
pub struct GlazingOptics {
    /// Beam solar transmittance fit in cos θ
    pub tau_beam: [f64; 6],
    /// Beam visible transmittance fit in cos θ
    pub tau_vis_beam: [f64; 6],
    /// Front beam absorptance fit of each layer (outside to inside)
    pub abs_beam: Vec<[f64; 6]>,
    /// Hemispherical solar transmittance (same from either side)
    pub tau_diffuse: f64,
    /// Hemispherical visible transmittance
    pub tau_vis_diffuse: f64,
    /// Hemispherical front (outdoor-side) solar reflectance
    pub rho_diffuse_front: f64,
    /// Hemispherical back (room-side) solar reflectance
    pub rho_diffuse_back: f64,
    /// Hemispherical front absorptance of each layer
    pub abs_diffuse: Vec<f64>,
    /// Hemispherical back (room-side) absorptance of each layer
    pub abs_diffuse_back: Vec<f64>,
}

impl GlazingOptics {
    fn new(layers: &[GlassLayer], simple: Option<SimpleRating>) -> Self {
        let n = layers.len();
        let mut cos_phi = [0.0; N_PHI];
        for (i, c) in cos_phi.iter_mut().enumerate() {
            *c = (i as f64 * 10.0).to_radians().cos();
        }
        let mut tsol = [0.0; N_PHI];
        let mut tvis = [0.0; N_PHI];
        let mut rfsol = [0.0; N_PHI];
        let mut rbsol = [0.0; N_PHI];
        let mut abs_f = vec![[0.0; N_PHI]; n];
        let mut abs_b = vec![[0.0; N_PHI]; n];
        // Dirt reduces the outermost layer's transmittance, from either side.
        let tau = |k: usize, solar: bool| {
            let l = &layers[k];
            let t = if solar {
                l.solar_transmittance
            } else {
                l.visible_transmittance
            };
            if k == 0 {
                t * l.dirt_factor
            } else {
                t
            }
        };
        for (p, &cs) in cos_phi.iter().enumerate() {
            let front: Vec<(f64, f64, f64)> = (0..n)
                .map(|k| {
                    let l = &layers[k];
                    trans_refl_at_phi(
                        cs,
                        tau(k, true),
                        l.solar_reflectance_front,
                        l.solar_reflectance_back,
                        simple,
                    )
                })
                .collect();
            let (t, rf, rb, a) = system_properties(&front);
            tsol[p] = t;
            rfsol[p] = rf;
            rbsol[p] = rb;
            for k in 0..n {
                abs_f[k][p] = a[k];
            }
            let vis: Vec<(f64, f64, f64)> = (0..n)
                .map(|k| {
                    let l = &layers[k];
                    trans_refl_at_phi(
                        cs,
                        tau(k, false),
                        l.visible_reflectance_front,
                        l.visible_reflectance_back,
                        simple,
                    )
                })
                .collect();
            tvis[p] = system_properties(&vis).0;
            // Light from the room: layers in reverse, faces swapped.
            let back: Vec<(f64, f64, f64)> = (0..n)
                .rev()
                .map(|k| {
                    let l = &layers[k];
                    trans_refl_at_phi(
                        cs,
                        tau(k, true),
                        l.solar_reflectance_back,
                        l.solar_reflectance_front,
                        simple,
                    )
                })
                .collect();
            let (_, _, _, ab) = system_properties(&back);
            for k in 0..n {
                abs_b[n - 1 - k][p] = ab[k];
            }
        }
        Self {
            tau_beam: lsq_fit(&cos_phi, &tsol),
            tau_vis_beam: lsq_fit(&cos_phi, &tvis),
            abs_beam: abs_f.iter().map(|a| lsq_fit(&cos_phi, a)).collect(),
            tau_diffuse: diffuse_average(&tsol),
            tau_vis_diffuse: diffuse_average(&tvis),
            rho_diffuse_front: diffuse_average(&rfsol),
            rho_diffuse_back: diffuse_average(&rbsol),
            abs_diffuse: abs_f.iter().map(diffuse_average).collect(),
            abs_diffuse_back: abs_b.iter().map(diffuse_average).collect(),
        }
    }

    /// Beam solar transmittance at incidence cosine `cos_theta`.
    pub fn tau(&self, cos_theta: f64) -> f64 {
        polyf(cos_theta, &self.tau_beam).max(0.0)
    }

    /// Beam solar absorptance of layer `k` at incidence cosine `cos_theta`.
    pub fn abs(&self, k: usize, cos_theta: f64) -> f64 {
        polyf(cos_theta, &self.abs_beam[k]).max(0.0)
    }

    /// Total hemispherical back absorptance of the layers.
    pub fn abs_diffuse_back_total(&self) -> f64 {
        self.abs_diffuse_back.iter().sum()
    }
}

// ─── Simple glazing equivalent layer ────────────────────────────────────────

/// Resistance of the simple glazing equivalent layer [m²·K/W]: the U-factor
/// less EnergyPlus's winter films for that U (`SetupSimpleWindowGlazingSystem`,
/// step 1), at least 0.001.
pub fn simple_layer_resistance(u: f64) -> f64 {
    let r_in = if u < 5.85 {
        1.0 / (0.359073 * u.ln() + 6.949915)
    } else {
        1.0 / (1.788041 * u - 2.886625)
    };
    let r_out = 1.0 / (0.025342 * u + 29.163853);
    (1.0 / u - r_in - r_out).max(0.001)
}

/// Normal-incidence solar transmittance of the equivalent layer from SHGC and
/// U (step 4, revised LBNL correlations).
pub fn simple_solar_transmittance(u: f64, shgc: f64) -> f64 {
    let high = if shgc < 0.7206 {
        0.939998 * shgc * shgc + 0.20332 * shgc
    } else {
        1.30415 * shgc - 0.30515
    };
    let low = if shgc <= 0.15 {
        0.41040 * shgc
    } else {
        0.085775 * shgc * shgc + 0.963954 * shgc - 0.084958
    };
    let t = if u > 4.5 {
        high
    } else if u < 3.4 {
        low
    } else {
        (u - 3.4) / (4.5 - 3.4) * (high - low) + low
    };
    t.max(0.0)
}

/// The equivalent layer of a simple glazing system
/// (`MaterialGlass::SetupSimpleWindowGlazingSystem`, steps 1–6).
pub fn simple_equivalent_layer(u: f64, shgc: f64, vt: Option<f64>) -> GlassLayer {
    let r_l = simple_layer_resistance(u);
    let thickness = if 1.0 / r_l > 7.0 {
        0.002
    } else {
        0.05914 - 0.00714 / r_l
    };
    let tsol = simple_solar_transmittance(u, shgc);
    // Solar-condition films, cubic and linear in SHGC − τ. Between U = 3.4
    // and 4.5 they are interpolated as EnergyPlus writes it.
    let d = shgc - tsol;
    let low = (
        1.0 / (199.8208128 * d.powi(3) - 90.639733 * d * d + 19.737055 * d + 6.766575),
        1.0 / (5.763355 * d + 20.541528),
    );
    let high = (
        1.0 / (29.436546 * d.powi(3) - 21.943415 * d * d + 9.945872 * d + 7.426151),
        1.0 / (2.225824 * d + 20.577080),
    );
    let (r_is, r_os) = if u > 4.5 {
        high
    } else if u < 3.4 {
        low
    } else {
        let f = (u - 3.4) / (4.5 - 3.4);
        (f * (low.0 - high.0) + low.0, f * (low.1 - high.1) + low.1)
    };
    let inflow = (r_os + 0.5 * r_l) / (r_os + r_l + r_is);
    let rsol = 1.0 - tsol - d / inflow;
    let (tvis, rvis_f, rvis_b) = match vt {
        Some(tv) => {
            let mut rb = -0.7409 * tv.powi(3) + 1.6531 * tv * tv - 1.2299 * tv + 0.4545;
            if tv + rb >= 1.0 {
                rb = 0.999 - tv;
            }
            let mut rf = -0.0622 * tv.powi(3) + 0.4277 * tv * tv - 0.4169 * tv + 0.2399;
            if tv + rf >= 1.0 {
                rf = 0.999 - tv;
            }
            (tv, rf, rb)
        }
        None => (tsol, rsol, rsol),
    };
    GlassLayer {
        thickness,
        conductivity: thickness / r_l,
        solar_transmittance: tsol,
        solar_reflectance_front: rsol,
        solar_reflectance_back: rsol,
        visible_transmittance: tvis,
        visible_reflectance_front: rvis_f,
        visible_reflectance_back: rvis_b,
        emissivity_front: 0.84,
        emissivity_back: 0.84,
        dirt_factor: 1.0,
    }
}

// ─── Glazing system and its heat balance ────────────────────────────────────

/// A glazing system: its layers, gaps, optics and rated performance.
#[derive(Debug, Clone, PartialEq)]
pub struct Glazing {
    pub layers: Vec<GlassLayer>,
    /// Gap `k` lies between layers `k` and `k + 1`.
    pub gaps: Vec<Gap>,
    pub optics: GlazingOptics,
    /// The ratings of a simple glazing system, `None` for layer-by-layer.
    pub simple: Option<SimpleRating>,
    /// Factor on the inside and outside convection coefficients of an
    /// exterior window. For simple glazing it makes the equivalent layer
    /// reproduce the rated U-factor at NFRC winter conditions, as EnergyPlus
    /// does (`CalcNominalWindowCond`, `CoeffAdjRatio`); 1 otherwise.
    pub film_factor: f64,
    /// Centre-of-glass U-factor at NFRC winter conditions [W/(m²·K)]
    pub nominal_u: f64,
    /// Centre-of-glass SHGC at NFRC summer conditions
    pub nominal_shgc: f64,
}

/// Outside boundary of a window's outer face.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OutsideBoundary {
    /// Convection coefficient [W/(m²·K)], before any film factor
    pub h_conv: f64,
    /// Air temperature [°C]
    pub t_air: f64,
    /// Long-wave irradiance on the face from sky, air and ground [W/m²]
    pub ir_incident: f64,
}

/// How the room-side node(s) exchange heat, re-evaluated each iteration.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RoomCoupling {
    /// The inner glass face sees the room: convection to air at `t_air`
    /// and linearised long-wave exchange to `t_rad` [°C].
    Film {
        h_conv: f64,
        h_rad: f64,
        t_air: f64,
        t_rad: f64,
    },
    /// An interior shade with a vented gap: glass–shade long-wave `h_gs`,
    /// gap convection `h_cv` (each face to the gap air), gap air to the room
    /// `h_vent`, shade convection `h_cs` and long-wave `h_rr` to the room at
    /// `t_air`/`t_rad`, and solar plus radiant gains absorbed by the shade
    /// `q_shade` [W/m²].
    Shade {
        h_gs: f64,
        h_cv: f64,
        h_vent: f64,
        h_cs: f64,
        h_rr: f64,
        t_air: f64,
        t_rad: f64,
        q_shade: f64,
    },
}

/// Result of a face solve.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FaceSolve {
    pub iterations: usize,
    pub converged: bool,
    /// Last room coupling, at the converged temperatures
    pub coupling: RoomCoupling,
    /// Conductance from the inner glass face to the outdoors through the
    /// glazing and outside film, linearised [W/(m²·K)]
    pub u_out: f64,
    /// Temperature step of the last iteration [K]
    pub last_change: f64,
}

/// Iteration limits of the face solve. EnergyPlus stops at a mean face
/// change of 0.02 K; this solve goes on to 1 × 10⁻⁵ K so the window stays
/// settled within the surface–air passes' tolerance.
const MAX_ITERATIONS: usize = 100;
const TOLERANCE: f64 = 1.0e-5;

/// Gauss elimination with partial pivoting on a small dense system.
fn solve_dense(a: &mut [[f64; 10]; 10], b: &mut [f64; 10], n: usize) {
    for k in 0..n {
        let p = (k..n)
            .max_by(|&i, &j| a[i][k].abs().total_cmp(&a[j][k].abs()))
            .unwrap_or(k);
        a.swap(k, p);
        b.swap(k, p);
        for i in k + 1..n {
            let f = a[i][k] / a[k][k];
            if f != 0.0 {
                let row_k = a[k];
                for (aij, akj) in a[i][k..n].iter_mut().zip(&row_k[k..n]) {
                    *aij -= f * akj;
                }
                b[i] -= f * b[k];
            }
        }
    }
    for k in (0..n).rev() {
        let s: f64 = (k + 1..n).map(|j| a[k][j] * b[j]).sum();
        b[k] = (b[k] - s) / a[k][k];
    }
}

impl Glazing {
    /// A simple glazing system from its U-factor, SHGC and optional visible
    /// transmittance.
    pub fn simple(u_factor: f64, shgc: f64, vt: Option<f64>) -> Self {
        let rating = SimpleRating { u_factor, shgc };
        let layer = simple_equivalent_layer(u_factor, shgc, vt);
        let mut g = Self::assemble(vec![layer], vec![], Some(rating));
        g.film_factor = g.rated_film_factor(u_factor);
        g.nominal_u = g.nominal(true, g.film_factor).0;
        g
    }

    /// A layer-by-layer glazing system.
    pub fn layered(layers: Vec<GlassLayer>, gaps: Vec<Gap>) -> Self {
        Self::assemble(layers, gaps, None)
    }

    fn assemble(layers: Vec<GlassLayer>, gaps: Vec<Gap>, simple: Option<SimpleRating>) -> Self {
        let optics = GlazingOptics::new(&layers, simple);
        let mut g = Self {
            layers,
            gaps,
            optics,
            simple,
            film_factor: 1.0,
            nominal_u: 0.0,
            nominal_shgc: 0.0,
        };
        g.nominal_u = g.nominal(true, 1.0).0;
        g.nominal_shgc = g.nominal(false, 1.0).1;
        g
    }

    pub fn n_faces(&self) -> usize {
        2 * self.layers.len()
    }

    /// Conductance of glass layer `k` [W/(m²·K)].
    pub fn layer_conductance(&self, k: usize) -> f64 {
        self.layers[k].conductivity / self.layers[k].thickness
    }

    /// Face temperatures [°C] from a resistance network that ignores absorbed
    /// radiation (`WindowManager::StartingWindowTemps`).
    pub fn starting_faces(&self, h_out: f64, h_in: f64, t_out: f64, t_in: f64) -> Vec<f64> {
        const H_RAD: f64 = 5.3;
        const R_GAP: f64 = 0.21;
        let nf = self.n_faces();
        let mut r = vec![1.0 / (h_out + H_RAD)];
        for k in 0..self.layers.len() {
            r.push(1.0 / self.layer_conductance(k));
            if k + 1 < self.layers.len() {
                r.push(R_GAP);
            }
        }
        r.push(1.0 / (h_in + H_RAD));
        let total: f64 = r.iter().sum();
        let mut dt = t_in - t_out;
        if dt.abs() < 0.5 {
            dt = 2.0;
        }
        let mut sum = 0.0;
        (0..nf)
            .map(|i| {
                sum += r[i];
                sum / total * dt + t_out
            })
            .collect()
    }

    /// Solves the face temperatures [°C] in `faces` (outer face first; for a
    /// shaded window followed by the shade and the gap air), starting from
    /// their values. `absorbed` is each layer's absorbed solar [W/m²], half to
    /// each face; `q_inner` is long-wave radiant gain absorbed by the inner
    /// glass face [W/m²]; `room` gives the room coupling at the current
    /// temperatures. Outside convection is multiplied by the film factor;
    /// `room` applies it to the inside convection.
    ///
    /// Each iteration linearises the long-wave terms at the current face
    /// temperatures (emission ε σ θ³·θ, gap exchange between grey parallel
    /// faces), updates the gap conductances and solves the linear system.
    /// Successive solutions are under-relaxed by ½ once they stop
    /// contracting and by ¼ after 25 iterations, as EnergyPlus relaxes them.
    // The boundary conditions are separate quantities a caller fills in from
    // different sources; a struct for them would only rename the arguments.
    #[allow(clippy::too_many_arguments)]
    pub fn solve_faces(
        &self,
        faces: &mut [f64],
        height: f64,
        tilt_deg: f64,
        outside: &OutsideBoundary,
        absorbed: &[f64],
        q_inner: f64,
        mut room: impl FnMut(&[f64]) -> RoomCoupling,
    ) -> FaceSolve {
        let nf = self.n_faces();
        let m = faces.len();
        let h_out = outside.h_conv * self.film_factor;
        let t_out = outside.t_air + KELVIN;
        let mut theta = [0.0; 10];
        for (t, f) in theta.iter_mut().zip(faces.iter()) {
            *t = f + KELVIN;
        }
        let mut coupling = room(faces);
        let mut relax = 1.0;
        let mut last_change = f64::INFINITY;
        let mut final_change = 0.0;
        let (mut iterations, mut converged) = (0, false);
        let mut h_gap = [0.0; MAX_LAYERS];
        while iterations < MAX_ITERATIONS {
            iterations += 1;
            let mut a = [[0.0; 10]; 10];
            let mut b = [0.0; 10];
            for k in 0..self.layers.len() {
                let (f, g) = (2 * k, 2 * k + 1);
                let s = self.layer_conductance(k);
                a[f][f] += s;
                a[f][g] -= s;
                a[g][g] += s;
                a[g][f] -= s;
                b[f] += 0.5 * absorbed.get(k).copied().unwrap_or(0.0);
                b[g] += 0.5 * absorbed.get(k).copied().unwrap_or(0.0);
            }
            // Outer face: convection and long-wave with the outdoors.
            let e0 = self.layers[0].emissivity_front;
            a[0][0] += e0 * STEFAN_BOLTZMANN * theta[0].powi(3) + h_out;
            b[0] += outside.ir_incident * e0 + h_out * t_out;
            // Gaps: gas conduction/convection and grey-body long-wave.
            for (k, gap) in self.gaps.iter().enumerate() {
                let (f, g) = (2 * k + 1, 2 * k + 2);
                let (ef, eg) = (
                    self.layers[k].emissivity_back,
                    self.layers[k + 1].emissivity_front,
                );
                let hg = gap.conductance(theta[f], theta[g], height, tilt_deg);
                h_gap[k] = hg;
                let den = 1.0 - (1.0 - ef) * (1.0 - eg);
                let hr_f = ef * eg * STEFAN_BOLTZMANN * theta[f].powi(3) / den;
                let hr_g = ef * eg * STEFAN_BOLTZMANN * theta[g].powi(3) / den;
                a[f][f] += hg + hr_f;
                a[f][g] += -hg - hr_g;
                a[g][f] += -hg - hr_f;
                a[g][g] += hg + hr_g;
            }
            // Inner glass face and any shade.
            let last = nf - 1;
            b[last] += q_inner;
            match coupling {
                RoomCoupling::Film {
                    h_conv,
                    h_rad,
                    t_air,
                    t_rad,
                } => {
                    a[last][last] += h_conv + h_rad;
                    b[last] += h_conv * (t_air + KELVIN) + h_rad * (t_rad + KELVIN);
                }
                RoomCoupling::Shade {
                    h_gs,
                    h_cv,
                    h_vent,
                    h_cs,
                    h_rr,
                    t_air,
                    t_rad,
                    q_shade,
                } => {
                    let (sh, ga) = (nf, nf + 1);
                    a[last][last] += h_gs + h_cv;
                    a[last][sh] -= h_gs;
                    a[last][ga] -= h_cv;
                    a[sh][sh] += h_gs + h_cv + h_cs + h_rr;
                    a[sh][last] -= h_gs;
                    a[sh][ga] -= h_cv;
                    b[sh] += h_cs * (t_air + KELVIN) + h_rr * (t_rad + KELVIN) + q_shade;
                    a[ga][ga] += 2.0 * h_cv + h_vent;
                    a[ga][last] -= h_cv;
                    a[ga][sh] -= h_cv;
                    b[ga] += h_vent * (t_air + KELVIN);
                }
            }
            // Long-wave emission of the outer face is ε σ θ⁴ = (ε σ θ³)·θ, so
            // its coefficient stays on the diagonal; the gap terms above are
            // the same exchange written as differences.
            solve_dense(&mut a, &mut b, m);
            let change = (0..m)
                .map(|i| (b[i] - theta[i]).abs())
                .fold(0.0_f64, f64::max);
            if change > last_change && relax > 0.5 {
                relax = 0.5;
            }
            if iterations > MAX_ITERATIONS / 4 {
                relax = 0.25;
            }
            for i in 0..m {
                theta[i] += relax * (b[i] - theta[i]);
            }
            for (f, t) in faces.iter_mut().zip(theta.iter()) {
                *f = t - KELVIN;
            }
            coupling = room(faces);
            final_change = relax * change;
            if final_change < TOLERANCE {
                converged = true;
                break;
            }
            last_change = change;
        }
        // Linearised conductance from the inner glass face to the outdoors.
        let mut r = 1.0
            / (h_out + 4.0 * self.layers[0].emissivity_front * STEFAN_BOLTZMANN * theta[0].powi(3));
        for k in 0..self.layers.len() {
            r += 1.0 / self.layer_conductance(k);
        }
        for (k, hg) in h_gap.iter().take(self.gaps.len()).enumerate() {
            let (ef, eg) = (
                self.layers[k].emissivity_back,
                self.layers[k + 1].emissivity_front,
            );
            let tm = 0.5 * (theta[2 * k + 1] + theta[2 * k + 2]);
            let hr =
                4.0 * STEFAN_BOLTZMANN * tm.powi(3) * ef * eg / (1.0 - (1.0 - ef) * (1.0 - eg));
            r += 1.0 / (hg + hr);
        }
        FaceSolve {
            iterations,
            converged,
            coupling,
            u_out: 1.0 / r,
            last_change: final_change,
        }
    }

    /// Centre-of-glass U-factor [W/(m²·K)] at NFRC winter conditions
    /// (`winter`) and SHGC at NFRC summer conditions, with convection
    /// coefficients scaled by `factor` (`WindowManager::CalcNominalWindowCond`,
    /// `WindowTempsForNominalCond`, `EvalNominalWindowCond`): vertical, 1 m
    /// tall, sky at the outdoor temperature.
    pub fn nominal(&self, winter: bool, factor: f64) -> (f64, f64) {
        let (t_in, t_out, h_out, solar) = if winter {
            (21.0, -18.0, 26.0, 0.0)
        } else {
            (24.0, 32.0, 15.0, 783.0)
        };
        let h_out = h_out * factor;
        let abs_norm: Vec<f64> = (0..self.layers.len())
            .map(|k| self.optics.abs(k, 1.0))
            .collect();
        let absorbed: Vec<f64> = abs_norm.iter().map(|a| a * solar).collect();
        let outside = OutsideBoundary {
            h_conv: h_out,
            t_air: t_out,
            ir_incident: STEFAN_BOLTZMANN * (t_out + KELVIN).powi(4),
        };
        let mut faces = self.starting_faces(h_out, 3.2, t_out, t_in);
        let nf = self.n_faces();
        let e_in = self.layers[nf / 2 - 1].emissivity_back;
        let mut h_in_prev: Option<f64> = None;
        // The nominal solve's own film-factor is applied through `factor`.
        let unscaled = Glazing {
            film_factor: 1.0,
            ..self.clone()
        };
        // Gap aspect ratio for an unknown window height: 1.524 m (5 ft), as E+.
        unscaled.solve_faces(&mut faces, 1.524, 90.0, &outside, &absorbed, 0.0, |f| {
            let t_s = f[nf - 1];
            let mut h = nominal_inside_convection(t_s, t_in);
            if let Some(p) = h_in_prev {
                h = 0.5 * (p + h);
            }
            h_in_prev = Some(h);
            // Exact linearisation of ε σ (θ⁴ − T_in⁴) about the face.
            let (a, b) = (t_s + KELVIN, t_in + KELVIN);
            RoomCoupling::Film {
                h_conv: h * factor,
                h_rad: e_in * STEFAN_BOLTZMANN * (a * a + b * b) * (a + b),
                t_air: t_in,
                t_rad: t_in,
            }
        });
        let h_in = h_in_prev.unwrap_or(3.2) * factor;
        let th: Vec<f64> = faces.iter().map(|f| f + KELVIN).collect();
        let h_out_rad = self.layers[0].emissivity_front
            * STEFAN_BOLTZMANN
            * 0.5
            * (t_out + KELVIN + th[0]).powi(3);
        let r_out = 1.0 / (h_out_rad + h_out);
        let h_in_rad = e_in * STEFAN_BOLTZMANN * 0.5 * (t_in + KELVIN + th[nf - 1]).powi(3);
        let r_in = 1.0 / (h_in_rad + h_in);
        // Resistance from the outdoors to the middle of each layer.
        let mut r_mid = Vec::with_capacity(self.layers.len());
        let mut r_bare = 0.0;
        for k in 0..self.layers.len() {
            let rk = 1.0 / self.layer_conductance(k);
            r_mid.push(r_out + r_bare + 0.5 * rk);
            r_bare += rk;
            if let Some(gap) = self.gaps.get(k) {
                let (ef, eg) = (
                    self.layers[k].emissivity_back,
                    self.layers[k + 1].emissivity_front,
                );
                let (t1, t2) = (th[2 * k + 1], th[2 * k + 2]);
                let hg = gap.conductance(t1, t2, 1.524, 90.0);
                let hr = ef * eg * STEFAN_BOLTZMANN * 0.5 * (t1 + t2).powi(3)
                    / (1.0 - (1.0 - ef) * (1.0 - eg));
                r_bare += 1.0 / (hg + hr);
            }
        }
        let r_tot = r_out + r_bare + r_in;
        let shgc = self.optics.tau(1.0)
            + abs_norm
                .iter()
                .zip(&r_mid)
                .map(|(a, r)| a * r / r_tot)
                .sum::<f64>();
        (1.0 / r_tot, shgc)
    }

    /// Film factor that makes the nominal U-factor match the rated one within
    /// 0.01 W/(m²·K), by bisection on [1, 2] (EnergyPlus `CoeffAdjRatio`).
    fn rated_film_factor(&self, u_rated: f64) -> f64 {
        let mut u = self.nominal(true, 1.0).0;
        let (mut lo, mut hi, mut factor) = (1.0, 2.0, 1.0);
        let mut iter = 0;
        while (u_rated - u).abs() > 0.01 && iter < 100 {
            factor = 0.5 * (lo + hi);
            u = self.nominal(true, factor).0;
            if u < u_rated {
                lo = factor;
            } else {
                hi = factor;
            }
            iter += 1;
        }
        factor
    }
}

/// Inside convection at nominal conditions: ISO 15099 eq. 135 for a 1 m
/// vertical window in dry air at 101 325 Pa (`WindowTempsForNominalCond`).
fn nominal_inside_convection(t_surface: f64, t_air: f64) -> f64 {
    let t_film_k = t_air + KELVIN + 0.25 * (t_surface - t_air);
    let rho = 101325.0 / (287.042 * t_film_k);
    let lambda = 2.873e-3 + 7.76e-5 * t_film_k;
    let mu = 3.723e-6 + 4.94e-8 * t_film_k;
    let cp = 1002.737 + 1.2324e-2 * t_film_k;
    let ra = rho * rho * 9.81 * cp * (t_surface - t_air).abs() / (t_film_k * mu * lambda);
    0.56 * ra.powf(0.25) * lambda
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clear_3mm() -> GlassLayer {
        GlassLayer {
            thickness: 0.003,
            conductivity: 1.0,
            solar_transmittance: 0.834,
            solar_reflectance_front: 0.075,
            solar_reflectance_back: 0.075,
            visible_transmittance: 0.899,
            visible_reflectance_front: 0.083,
            visible_reflectance_back: 0.083,
            emissivity_front: 0.84,
            emissivity_back: 0.84,
            dirt_factor: 1.0,
        }
    }

    #[test]
    fn single_gas_properties_follow_the_iso_15099_coefficients() {
        let p = GasFill::single(AIR).properties(283.15);
        assert!((p.conductivity - (2.873e-3 + 7.76e-5 * 283.15)).abs() < 1e-12);
        assert!((p.density - 1.0e5 * 28.97 / (8314.51 * 283.15)).abs() < 1e-12);
    }

    #[test]
    fn mixture_of_one_gas_with_itself_is_that_gas() {
        let a = GasFill::single(ARGON).properties(290.0);
        let m = GasFill {
            parts: vec![(ARGON, 0.4), (ARGON, 0.6)],
        }
        .properties(290.0);
        assert!((a.conductivity - m.conductivity).abs() < 1e-12);
        assert!((a.viscosity - m.viscosity).abs() < 1e-15);
        assert!((a.specific_heat - m.specific_heat).abs() < 1e-9);
    }

    #[test]
    fn argon_air_mixture_conductivity_lies_between_its_parts() {
        let t = 283.15;
        let ar = GasFill::single(ARGON).properties(t).conductivity;
        let air = GasFill::single(AIR).properties(t).conductivity;
        let mix = GasFill {
            parts: vec![(ARGON, 0.9), (AIR, 0.1)],
        }
        .properties(t)
        .conductivity;
        assert!(mix > ar && mix < air, "{ar} < {mix} < {air}");
    }

    #[test]
    fn nusselt_is_one_for_conduction_regime_and_rises_with_rayleigh() {
        assert!((cavity_nusselt(10.0, 100.0, 90.0, false) - 1.0).abs() < 1e-3);
        let n1 = cavity_nusselt(1.0e4, 100.0, 90.0, false);
        let n2 = cavity_nusselt(1.0e5, 100.0, 90.0, false);
        assert!(n2 > n1 && n1 > 1.0);
        // Heated from above, a horizontal cavity only conducts.
        assert!((cavity_nusselt(1.0e5, 100.0, 0.0, true) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn single_layer_optics_reproduce_window_7_clear_glass() {
        // WINDOW 7 ID 15 (CLEAR_3): Tsol and Abs1 at 0°…80°, hemispherical.
        let g = Glazing::layered(vec![clear_3mm()], vec![]);
        let tsol = [
            0.834, 0.833, 0.831, 0.827, 0.818, 0.797, 0.749, 0.637, 0.389,
        ];
        for (i, t) in tsol.iter().enumerate() {
            let cs = (i as f64 * 10.0).to_radians().cos();
            assert!((g.optics.tau(cs) - t).abs() < 0.005, "{i}0°");
        }
        assert!((g.optics.tau_diffuse - 0.753).abs() < 0.005);
        assert!((g.optics.abs_diffuse[0] - 0.101).abs() < 0.005);
    }

    #[test]
    fn double_clear_optics_match_inter_reflection_and_window_7() {
        let g = Glazing::layered(
            vec![clear_3mm(), clear_3mm()],
            vec![Gap {
                width: 0.012,
                fill: GasFill::single(AIR),
            }],
        );
        // Two identical layers: τ = τ₁²/(1 − ρ₁²) at normal incidence.
        let tau = 0.834_f64.powi(2) / (1.0 - 0.075_f64.powi(2));
        assert!((g.optics.tau(1.0) - tau).abs() < 1e-3);
        // WINDOW 7 ID 23 integrates the measured spectra (Tsol 0.703, Abs
        // 0.096 / 0.072, hemispherical 0.601 / 0.110 / 0.073); spectral
        // averages agree within half a percent of incident solar.
        assert!((g.optics.tau(1.0) - 0.703).abs() < 0.005);
        assert!((g.optics.abs(0, 1.0) - 0.096).abs() < 0.005);
        assert!((g.optics.abs(1, 1.0) - 0.072).abs() < 0.005);
        assert!((g.optics.tau_diffuse - 0.601).abs() < 0.006);
        assert!((g.optics.abs_diffuse[0] - 0.110).abs() < 0.005);
        assert!((g.optics.abs_diffuse[1] - 0.073).abs() < 0.006);
    }

    fn layer(
        t: f64,
        tau: f64,
        rf: f64,
        rb: f64,
        tv: f64,
        rvf: f64,
        rvb: f64,
        eb: f64,
    ) -> GlassLayer {
        GlassLayer {
            thickness: t,
            solar_transmittance: tau,
            solar_reflectance_front: rf,
            solar_reflectance_back: rb,
            visible_transmittance: tv,
            visible_reflectance_front: rvf,
            visible_reflectance_back: rvb,
            emissivity_back: eb,
            ..clear_3mm()
        }
    }

    /// Nominal U-factor, SHGC, solar and visible transmittance against the
    /// `WindowConstruction` lines of EnergyPlus 25.2's eio for the same
    /// layers (CLEAR_3 and the CSR42_3 low-e of WINDOW 7 ID 36); input in
    /// `tests/eplus/window_constructions.idf`.
    #[test]
    fn layered_systems_match_energyplus_nominal_performance() {
        let clear = clear_3mm();
        let lowe = layer(0.0032, 0.452, 0.359, 0.397, 0.714, 0.207, 0.148, 0.047);
        let gap = |w: f64, fill: GasFill| Gap { width: w, fill };
        let mix = GasFill {
            parts: vec![(ARGON, 0.9), (AIR, 0.1)],
        };
        let cases = [
            (
                Glazing::layered(vec![clear.clone()], vec![]),
                [5.906, 0.859, 0.834, 0.899],
            ),
            (
                Glazing::layered(
                    vec![clear.clone(), clear.clone()],
                    vec![gap(0.012, GasFill::single(AIR))],
                ),
                [2.740, 0.760, 0.700, 0.814],
            ),
            (
                Glazing::layered(
                    vec![lowe.clone(), clear.clone()],
                    vec![gap(0.012, GasFill::single(ARGON))],
                ),
                [1.380, 0.438, 0.390, 0.650],
            ),
            (
                Glazing::layered(
                    vec![lowe.clone(), clear.clone(), clear.clone()],
                    vec![gap(0.0127, mix.clone()), gap(0.0127, mix)],
                ),
                [1.063, 0.407, 0.334, 0.595],
            ),
            (
                Glazing::layered(
                    vec![clear.clone(), lowe, clear.clone(), clear],
                    vec![
                        gap(0.008, GasFill::single(KRYPTON)),
                        gap(0.008, GasFill::single(XENON)),
                        gap(0.008, GasFill::single(AIR)),
                    ],
                ),
                [0.734, 0.378, 0.287, 0.547],
            ),
        ];
        for (i, (g, e)) in cases.iter().enumerate() {
            let got = [
                g.nominal_u,
                g.nominal_shgc,
                g.optics.tau(1.0),
                polyf(1.0, &g.optics.tau_vis_beam),
            ];
            for k in 0..4 {
                assert!(
                    (got[k] - e[k]).abs() < 0.0025,
                    "case {i} item {k}: {} vs E+ {}",
                    got[k],
                    e[k]
                );
            }
        }
    }

    /// The simple glazing equivalent layer and its nominal performance
    /// against EnergyPlus 25.2's eio (`WindowMaterial:Glazing` and
    /// `WindowConstruction` lines); input in `tests/eplus/window_constructions.idf`.
    #[test]
    fn simple_glazing_matches_energyplus_equivalent_layer() {
        // (U, SHGC) → thickness, conductivity, τ, ρ, nominal SHGC, film ratio
        let cases = [
            (
                (1.19, 0.44),
                [4.83785e-2, 7.29166e-2, 0.35579, 0.45826, 0.441, 1.000],
            ),
            (
                (2.0, 0.40),
                [3.72957e-2, 0.11410, 0.31435, 0.48237, 0.401, 1.000],
            ),
            (
                (3.0, 0.60),
                [1.53297e-2, 9.40613e-2, 0.52429, 0.27736, 0.601, 1.000],
            ),
            (
                (5.8, 0.82),
                [2.0e-3, 0.31299, 0.76425, 3.98275e-2, 0.823, 1.004],
            ),
            (
                (3.9, 0.52),
                [2.0e-3, 2.27782e-2, 0.40331, 0.26405, 0.523, 1.000],
            ),
            (
                (1.6, 0.27),
                [4.32833e-2, 9.61247e-2, 0.18156, 0.61613, 0.270, 1.000],
            ),
        ];
        for ((u, shgc), e) in cases {
            let g = Glazing::simple(u, shgc, None);
            let l = &g.layers[0];
            let rel = |a: f64, b: f64| ((a - b) / b).abs();
            assert!(rel(l.thickness, e[0]) < 1e-4, "U {u}");
            assert!(rel(l.conductivity, e[1]) < 1e-4, "U {u}");
            assert!((l.solar_transmittance - e[2]).abs() < 1e-5, "U {u}");
            assert!((l.solar_reflectance_front - e[3]).abs() < 1e-5, "U {u}");
            assert!(
                (g.nominal_shgc - e[4]).abs() < 0.0015,
                "U {u}: {}",
                g.nominal_shgc
            );
            assert!(
                (g.film_factor - e[5]).abs() < 0.0015,
                "U {u}: {}",
                g.film_factor
            );
        }
    }

    #[test]
    fn simple_glazing_reproduces_its_rated_u_factor() {
        for (u, shgc) in [(1.19, 0.44), (2.0, 0.4), (3.0, 0.6), (5.8, 0.82)] {
            let g = Glazing::simple(u, shgc, None);
            assert!((g.nominal_u - u).abs() <= 0.011, "U {u}: {}", g.nominal_u);
            assert!(g.film_factor >= 1.0 && g.film_factor <= 2.0);
        }
    }

    /// The converged faces conserve energy: absorbed solar plus the radiant
    /// gain on the inner face leaves through the outer and inner faces, and
    /// each gap passes the same heat from face to face.
    #[test]
    fn face_solve_conserves_energy_in_a_tilted_triple() {
        let lowe = layer(0.0032, 0.452, 0.359, 0.397, 0.714, 0.207, 0.148, 0.047);
        let mix = GasFill {
            parts: vec![(ARGON, 0.9), (AIR, 0.1)],
        };
        let g = Glazing::layered(
            vec![lowe, clear_3mm(), clear_3mm()],
            vec![
                Gap {
                    width: 0.0127,
                    fill: mix.clone(),
                },
                Gap {
                    width: 0.0127,
                    fill: mix,
                },
            ],
        );
        let out = OutsideBoundary {
            h_conv: 12.0,
            t_air: -5.0,
            ir_incident: 250.0,
        };
        let absorbed = [120.0, 25.0, 15.0];
        let (t_z, t_r, h_c, h_r, q_in) = (21.0, 19.0, 2.5, 4.6, 8.0);
        let mut faces = g.starting_faces(12.0, 3.0, -5.0, 21.0);
        let res = g.solve_faces(&mut faces, 1.5, 30.0, &out, &absorbed, q_in, |_| {
            RoomCoupling::Film {
                h_conv: h_c,
                h_rad: h_r,
                t_air: t_z,
                t_rad: t_r,
            }
        });
        assert!(res.converged, "{res:?}");
        let th: Vec<f64> = faces.iter().map(|f| f + KELVIN).collect();
        let e0 = g.layers[0].emissivity_front;
        let q_outer = h_c_out(&g, &out) * (th[0] - (out.t_air + KELVIN))
            + e0 * (STEFAN_BOLTZMANN * th[0].powi(4) - out.ir_incident);
        let q_room = h_c * (faces[5] - t_z) + h_r * (faces[5] - t_r);
        let source: f64 = absorbed.iter().sum::<f64>() + q_in;
        assert!(
            (q_outer + q_room - source).abs() < 1e-3 * source,
            "out {q_outer} + room {q_room} vs {source}"
        );
        // Through layer 3 (inner): conduction plus its own absorbed solar
        // reaches the room.
        let cond3 = g.layer_conductance(2) * (faces[4] - faces[5]);
        assert!((cond3 + 0.5 * absorbed[2] + q_in - q_room).abs() < 1e-3 * source);
    }

    fn h_c_out(g: &Glazing, out: &OutsideBoundary) -> f64 {
        out.h_conv * g.film_factor
    }
}
