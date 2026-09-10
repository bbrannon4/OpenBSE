//! Thermal comfort indices: PMV/PPD (Fanger ISO 7730 / ASHRAE 55) and solar MRT correction.
//!
//! References:
//! - ISO 7730:2005 — Ergonomics of the thermal environment: PMV, PPD
//! - ASHRAE Standard 55-2023 — Thermal Environmental Conditions for Human Occupancy
//! - ASHRAE 55-2023 Annex C — Solar radiation correction to MRT
//! - Walkenhorst et al. (2004) — Projected area factor for standing person

use crate::psat_fn_temp;

const SIGMA: f64 = 5.6704e-8; // Stefan-Boltzmann [W/(m²·K⁴)]

/// Metabolic rate [W/m²] per met unit (1 met = 58.15 W/m²).
pub const W_PER_MET: f64 = 58.15;

/// Clothing insulation [m²·K/W] per clo unit (1 clo = 0.155 m²·K/W).
pub const M2K_PER_CLO: f64 = 0.155;

/// Predicted Mean Vote per ISO 7730 / ASHRAE 55 Fanger model.
///
/// # Arguments
/// * `t_air`  — Air (dry-bulb) temperature [°C]
/// * `t_mrt`  — Mean radiant temperature [°C] (area-weighted surface temp)
/// * `v_air`  — Relative air velocity [m/s] (0.1 m/s is a still-room default)
/// * `rh`     — Relative humidity [0–1]
/// * `met`    — Metabolic rate [met] (1 met = 58.15 W/m²)
/// * `clo`    — Clothing insulation [clo] (1 clo = 0.155 m²·K/W)
///
/// # Returns
/// PMV on the ASHRAE comfort scale (−3 cold … 0 neutral … +3 hot).
pub fn pmv(t_air: f64, t_mrt: f64, v_air: f64, rh: f64, met: f64, clo: f64) -> f64 {
    let mw = met * W_PER_MET; // metabolic rate minus external work [W/m²]
    let i_cl = clo * M2K_PER_CLO; // clothing thermal resistance [m²K/W]

    let f_cl = if i_cl <= 0.078 {
        1.00 + 1.290 * i_cl
    } else {
        1.05 + 0.645 * i_cl
    };

    let p_a = rh.clamp(0.0, 1.0) * psat_fn_temp(t_air); // vapor pressure [Pa]

    // Clothing surface temperature (iterate to convergence)
    let t_cl = clothing_surface_temp(mw, i_cl, f_cl, t_air, t_mrt, v_air);

    let h_c = convective_coeff(t_cl, t_air, v_air);

    // ISO 7730 Eq. (1) thermal load components
    // The 0.42·(M−58.15) sweat-evaporation term is clamped at 0: ISO 7730 leaves
    // it unclamped, but below ~1 met it would otherwise contribute a spurious
    // negative loss. Harmless at met ≥ 1 (the validated range); intentional guard.
    let heat_loss = 3.05e-3 * (5733.0 - 6.99 * mw - p_a)
        + 0.42 * (mw - 58.15).max(0.0)
        + 1.7e-5 * mw * (5867.0 - p_a)
        + 0.0014 * mw * (34.0 - t_air)
        + 3.96e-8 * f_cl * ((t_cl + 273.15).powi(4) - (t_mrt + 273.15).powi(4))
        + f_cl * h_c * (t_cl - t_air);

    let thermal_load = mw - heat_loss;

    (0.303 * (-0.036 * mw).exp() + 0.028) * thermal_load
}

/// Predicted Percentage Dissatisfied from PMV.
///
/// ISO 7730 Eq. (2). Valid for PMV in [−3, +3]; clamped outside that range.
pub fn ppd(pmv_val: f64) -> f64 {
    let p = pmv_val.clamp(-3.0, 3.0);
    100.0 - 95.0 * (-0.03353 * p.powi(4) - 0.2179 * p.powi(2)).exp()
}

/// Operative temperature [°C].
///
/// Simple equal-weight average, valid when air velocity ≤ 0.2 m/s.
/// At higher velocities the weighting shifts toward air temperature.
pub fn operative_temperature(t_air: f64, t_mrt: f64) -> f64 {
    (t_air + t_mrt) / 2.0
}

/// Solar MRT correction — direct-beam simplification of ASHRAE 55-2023 Annex C
/// (SolarCal).
///
/// Returns the increment ΔT_mrt [K] to add to the long-wave MRT when the occupant
/// is in direct beam solar radiation:
///
/// ```text
/// ΔT_mrt = α_sw · f_p · DNI / (ε_lw · σ · 4 · T_mrt_K³)
/// ```
///
/// This implements only the **direct-beam** term of the SolarCal model. The full
/// Annex C model additionally accounts for diffuse-sky and ground-reflected
/// short-wave radiation (via the sky-vision factor `f_svv`), the beam-exposure
/// fraction `f_bes`, and the effective radiating-area fraction `f_eff` (~0.70).
/// Adding those terms is tracked as a follow-up; the direct term dominates for an
/// occupant in a sunbeam.
///
/// # Arguments
/// * `dni`        — Direct normal irradiance (beam from sun) [W/m²]
/// * `solar_alt`  — Solar altitude angle [radians]
/// * `t_mrt`      — Long-wave mean radiant temperature [°C]
/// * `alpha_sw`   — Short-wave absorptivity of body/clothing (SolarCal default 0.67)
/// * `epsilon_lw` — Long-wave emissivity of body (default 0.95)
///
/// Returns 0.0 when the sun is below the horizon.
pub fn solar_mrt_correction(
    dni: f64,
    solar_alt: f64,
    t_mrt: f64,
    alpha_sw: f64,
    epsilon_lw: f64,
) -> f64 {
    if solar_alt <= 0.0 || dni <= 0.0 {
        return 0.0;
    }
    let fp = projected_area_factor(solar_alt);
    let i_eff = dni * fp;
    let t_mrt_k = t_mrt + 273.15;
    alpha_sw * i_eff / (epsilon_lw * SIGMA * 4.0 * t_mrt_k.powi(3))
}

/// Walkenhorst (2004) projected area factor for a standing person,
/// averaged over all body orientations relative to the sun.
///
/// # Arguments
/// * `altitude` — Solar altitude [radians]
///
/// Returns f_p in [0, 0.308].
pub fn projected_area_factor(altitude: f64) -> f64 {
    // Walkenhorst polynomial approximation, standing person, azimuth-averaged.
    // fp = 0.308 × cos(0.998·alt − 0.0875·alt²)
    let a = altitude.clamp(0.0, std::f64::consts::FRAC_PI_2);
    0.308 * (0.998 * a - 0.0875 * a * a).cos()
}

// ─── Internal helpers ────────────────────────────────────────────────────────

fn convective_coeff(t_cl: f64, t_air: f64, v_air: f64) -> f64 {
    let natural = 2.38 * (t_cl - t_air).abs().powf(0.25);
    let forced = 12.1 * v_air.max(0.0).sqrt();
    natural.max(forced)
}

fn clothing_surface_temp(mw: f64, i_cl: f64, f_cl: f64, t_air: f64, t_mrt: f64, v_air: f64) -> f64 {
    // ISO 7730 Eq. (A5) with 50/50 damping to prevent oscillation.
    let mut t_cl = 35.7 - 0.028 * mw;
    for _ in 0..150 {
        let h_c = convective_coeff(t_cl, t_air, v_air);
        let t_cl_new = 35.7
            - 0.028 * mw
            - i_cl
                * (3.96e-8 * f_cl * ((t_cl + 273.15).powi(4) - (t_mrt + 273.15).powi(4))
                    + f_cl * h_c * (t_cl - t_air));
        let t_cl_damp = 0.5 * (t_cl + t_cl_new);
        if (t_cl_damp - t_cl).abs() < 0.00015 {
            return t_cl_damp;
        }
        t_cl = t_cl_damp;
    }
    t_cl
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_relative_eq;

    #[test]
    fn test_pmv_neutral_conditions() {
        // 24°C air/mrt, 50% RH, 0.1 m/s, 1.2 met, 0.5 clo → PMV ≈ −0.22 (near-neutral).
        // Reference: pythermalcomfort / ISO 7730 Table A.1. Pinned with a tight
        // tolerance so the test actually guards the formula (#110).
        let p = pmv(24.0, 24.0, 0.1, 0.5, 1.2, 0.5);
        assert_relative_eq!(p, -0.22, epsilon = 0.05);
    }

    #[test]
    fn test_pmv_hot() {
        // Hot conditions should give positive PMV
        let p = pmv(30.0, 32.0, 0.1, 0.5, 1.2, 0.5);
        assert!(p > 1.5, "Expected hot PMV, got {p:.3}");
    }

    #[test]
    fn test_pmv_cold() {
        // Cold conditions should give negative PMV
        let p = pmv(16.0, 14.0, 0.1, 0.5, 1.2, 1.0);
        assert!(p < -1.0, "Expected cold PMV, got {p:.3}");
    }

    #[test]
    fn test_ppd_at_neutral_pmv() {
        // At PMV=0, PPD should be ~5%
        let d = ppd(0.0);
        assert_relative_eq!(d, 5.0, epsilon = 0.1);
    }

    #[test]
    fn test_ppd_at_pmv_2() {
        // At PMV=±2, PPD should be ~77%
        let d = ppd(2.0);
        assert!(d > 70.0 && d < 85.0, "PPD {d:.1} not near 77%");
    }

    #[test]
    fn test_solar_mrt_no_sun() {
        assert_eq!(solar_mrt_correction(800.0, -0.1, 22.0, 0.57, 0.95), 0.0);
        assert_eq!(solar_mrt_correction(0.0, 0.5, 22.0, 0.57, 0.95), 0.0);
    }

    #[test]
    fn test_solar_mrt_correction_plausible() {
        // 45° sun altitude, 600 W/m² DNI, 22°C MRT → expect ΔT_mrt ≈ 5–15 K
        let dt = solar_mrt_correction(600.0, 45_f64.to_radians(), 22.0, 0.57, 0.95);
        assert!(
            dt > 4.0 && dt < 20.0,
            "ΔT_mrt {dt:.1} outside plausible range"
        );
    }

    #[test]
    fn test_projected_area_factor_bounds() {
        // f_p should be in [0, 0.308]
        for alt_deg in [0, 15, 30, 45, 60, 75, 90] {
            let fp = projected_area_factor((alt_deg as f64).to_radians());
            assert!(
                fp >= 0.0 && fp <= 0.31,
                "fp {fp:.3} out of range at {alt_deg}°"
            );
        }
    }
}
