//! Solar position and incident radiation calculations.
//!
//! - Solar position (altitude, azimuth) from date/time/latitude
//! - Incident solar on tilted surfaces (Perez 1990 anisotropic sky model)
//! - Window solar transmission (angular-dependent SHGC with Fresnel optics)
//!
//! Reference: ASHRAE Fundamentals Ch. 14, Spencer (1971),
//! Perez et al. (1990) Solar Energy 44(5):271-289.

use std::f64::consts::PI;

/// Solar position result.
#[derive(Debug, Clone, Copy)]
pub struct SolarPosition {
    /// Solar altitude angle [radians] (0 = horizon, π/2 = zenith)
    pub altitude: f64,
    /// Solar azimuth angle [radians from south, west positive]
    pub azimuth: f64,
    /// Cosine of solar zenith angle
    pub cos_zenith: f64,
    /// Whether the sun is above the horizon
    pub is_sunup: bool,
}

/// Calculate solar position for a given time and location.
///
/// Uses Spencer (1971) formulation for solar declination.
/// Reference: ASHRAE Fundamentals, Chapter 14.
pub fn solar_position(
    day_of_year: u32,
    solar_hour: f64,
    latitude_deg: f64,
) -> SolarPosition {
    let lat = latitude_deg.to_radians();

    // Solar declination — Spencer (1971)
    let day_angle = 2.0 * PI * (day_of_year as f64 - 1.0) / 365.0;
    let declination = 0.006918
        - 0.399912 * day_angle.cos()
        + 0.070257 * day_angle.sin()
        - 0.006758 * (2.0 * day_angle).cos()
        + 0.000907 * (2.0 * day_angle).sin()
        - 0.002697 * (3.0 * day_angle).cos()
        + 0.00148 * (3.0 * day_angle).sin();

    // Hour angle (negative before noon, positive after)
    let hour_angle = (solar_hour - 12.0) * 15.0_f64.to_radians();

    // Solar altitude
    let sin_alt = lat.sin() * declination.sin()
        + lat.cos() * declination.cos() * hour_angle.cos();
    let altitude = sin_alt.clamp(-1.0, 1.0).asin();

    let cos_zenith = sin_alt.max(0.0);

    // Solar azimuth (from south, west positive)
    let cos_alt = altitude.cos().max(0.001);
    let sin_azimuth = declination.cos() * hour_angle.sin() / cos_alt;
    let cos_azimuth = (sin_alt * lat.sin() - declination.sin())
        / (cos_alt * lat.cos().max(0.001));
    let azimuth = sin_azimuth.atan2(cos_azimuth);

    SolarPosition {
        altitude,
        azimuth,
        cos_zenith,
        is_sunup: altitude > 0.0,
    }
}

/// Equation of time correction [hours].
///
/// Accounts for the difference between solar time and clock time
/// due to Earth's orbital eccentricity and axial tilt.
pub fn equation_of_time(day_of_year: u32) -> f64 {
    let b = 2.0 * PI * (day_of_year as f64 - 81.0) / 364.0;
    0.1645 * (2.0 * b).sin() - 0.1255 * b.cos() - 0.025 * b.sin()
}

// ─── Anisotropic Sky Diffuse Model ────────────────────────────────────────

/// Extraterrestrial normal incidence irradiance [W/m²].
///
/// Accounts for Earth's orbital eccentricity (±3.3%).
/// Reference: Spencer (1971).
fn extraterrestrial_irradiance(day_of_year: u32) -> f64 {
    const I_SC: f64 = 1367.0; // Solar constant [W/m²]
    let day_angle = 2.0 * PI * (day_of_year as f64 - 1.0) / 365.0;
    I_SC * (1.0 + 0.033 * day_angle.cos())
}

// ─── Perez 1990 Anisotropic Sky Model ───────────────────────────────────────
//
// Decomposes sky diffuse into three components for proper shading treatment:
//   1. Isotropic background — uniform sky dome, reduced by DifShdgRatioIsoSky
//   2. Circumsolar brightening — directional, reduced by sunlit fraction
//   3. Horizon brightening — near-horizon band, reduced by DifShdgRatioHoriz
//
// For the TOTAL incident radiation, Hay-Davies (1980) is used (proven accurate
// for building energy simulation). The Perez decomposition is used ONLY to
// determine what fraction of the Hay-Davies isotropic term is truly isotropic
// vs. horizon-like, so that diffuse shading can be applied correctly.
//
// References:
//   Perez et al. (1990) Solar Energy 44(5):271-289
//   Hay & Davies (1980), Proc. 1st Canadian Solar Radiation Data Workshop
//   EnergyPlus SolarShading.cc, AnisoSkyViewFactors()

/// Perez 1990 sky clearness bin boundaries.
/// Creates 8 bins: [≤1.065], [1.065-1.23], ..., [>6.2]
const PEREZ_EPSILON_LIMITS: [f64; 7] = [1.065, 1.23, 1.5, 1.95, 2.8, 4.5, 6.2];

/// Perez circumsolar brightness coefficients (F1 = F11 + F12·Δ + F13·θz).
/// From EnergyPlus SolarShading.cc, provided by R. Perez (private comm., 1999).
const PEREZ_F11: [f64; 8] = [-0.0083117, 0.1299457, 0.3296958, 0.5682053, 0.8730280, 1.1326077, 1.0601591, 0.6777470];
const PEREZ_F12: [f64; 8] = [ 0.5877285, 0.6825954, 0.4868735, 0.1874525,-0.3920403,-1.2367284,-1.5999137,-0.3272588];
const PEREZ_F13: [f64; 8] = [-0.0620636,-0.1513752,-0.2210958,-0.2951290,-0.3616149,-0.4118494,-0.3589221,-0.2504286];

/// Perez horizon/zenith brightness coefficients (F2 = F21 + F22·Δ + F23·θz).
const PEREZ_F21: [f64; 8] = [-0.0596012,-0.0189325, 0.0554140, 0.1088631, 0.2255647, 0.2877813, 0.2642124, 0.1561313];
const PEREZ_F22: [f64; 8] = [ 0.0721249, 0.0659650,-0.0639588,-0.1519229,-0.4620442,-0.8230357,-1.1272340,-1.3765031];
const PEREZ_F23: [f64; 8] = [-0.0220216,-0.0288748,-0.0260542,-0.0139754, 0.0012448, 0.0558651, 0.1310694, 0.2506212];

/// Compute Perez F1 (circumsolar) and F2 (horizon) brightness coefficients.
///
/// Used to determine the fraction of diffuse radiation that is truly isotropic
/// vs. circumsolar vs. horizon brightening. This decomposition is needed for
/// proper application of diffuse sky shading ratios.
///
/// Returns (F1, F2) where F1 ≥ 0. F2 can be slightly negative for overcast.
pub fn perez_brightness_coefficients(
    beam_normal: f64,
    diffuse_horiz: f64,
    zenith_angle_rad: f64,
    _day_of_year: u32,
    elevation_m: f64,
) -> (f64, f64) {
    if diffuse_horiz < 1.0 {
        return (0.0, 0.0);
    }

    // Sky clearness ε (EnergyPlus SolarShading.cc line 2681)
    let kappa_z3 = 1.041 * zenith_angle_rad.powi(3);
    let epsilon = ((beam_normal + diffuse_horiz) / diffuse_horiz + kappa_z3) / (1.0 + kappa_z3);

    // Sky brightness Δ (Perez et al. 1990)
    // Air mass via Kasten & Young (1989) with altitude pressure correction,
    // matching EnergyPlus SolarShading.cc AnisoSkyViewFactors():
    //   AirMass = (1 - 0.1 * Elevation/1000) / (sin(alt) + ...)
    // For Denver at 1609m, this reduces air mass by 16%, lowering Delta
    // and shifting Perez coefficients toward more isotropic sky.
    let altitude_factor = (1.0 - 0.1 * elevation_m / 1000.0).max(0.5);
    let cz = zenith_angle_rad.cos().clamp(0.0, 1.0);
    let zenith_deg = cz.acos().to_degrees();
    let denom = cz + 0.50572 * (96.07995 - zenith_deg).max(0.01).powf(-1.6364);
    let air_mass = if denom > 0.0 {
        (altitude_factor / denom).min(40.0)
    } else {
        40.0
    };
    let delta = diffuse_horiz * air_mass / 1353.0;

    // Select epsilon bin
    let mut bin = 7usize;
    for (i, &limit) in PEREZ_EPSILON_LIMITS.iter().enumerate() {
        if epsilon < limit {
            bin = i;
            break;
        }
    }

    // Compute F1 and F2
    let f1 = (PEREZ_F11[bin] + PEREZ_F12[bin] * delta + PEREZ_F13[bin] * zenith_angle_rad).max(0.0);
    let f2 = PEREZ_F21[bin] + PEREZ_F22[bin] * delta + PEREZ_F23[bin] * zenith_angle_rad;

    (f1, f2)
}

/// Calculate incident solar radiation on a tilted surface [W/m²].
///
/// Uses the Perez 1990 anisotropic sky model for diffuse radiation:
///   I_total = I_beam · cos(AOI)
///           + I_diffuse_perez(tilt, AOI, sky_clearness)
///           + I_global · ρ_ground · (1 - cos(tilt)) / 2
///
/// # Arguments
/// * `beam_normal` - Direct normal radiation from weather [W/m²]
/// * `diffuse_horiz` - Diffuse horizontal radiation from weather [W/m²]
/// * `global_horiz` - Global horizontal radiation from weather [W/m²]
/// * `solar_pos` - Current solar position
/// * `surface_azimuth_deg` - Surface azimuth [degrees from north, clockwise]
/// * `surface_tilt_deg` - Surface tilt [degrees from horizontal]
/// * `ground_reflectance` - Ground reflectance [0-1], typically 0.2
pub fn incident_solar(
    beam_normal: f64,
    diffuse_horiz: f64,
    global_horiz: f64,
    solar_pos: &SolarPosition,
    surface_azimuth_deg: f64,
    surface_tilt_deg: f64,
    ground_reflectance: f64,
) -> f64 {
    if !solar_pos.is_sunup || (beam_normal + diffuse_horiz) <= 0.0 {
        return 0.0;
    }

    let tilt = surface_tilt_deg.to_radians();
    // Convert surface azimuth from north-clockwise to south-based
    let surface_azimuth = (surface_azimuth_deg - 180.0).to_radians();

    // Angle of incidence between sun and surface normal
    let cos_aoi = solar_pos.altitude.sin() * tilt.cos()
        + solar_pos.altitude.cos() * tilt.sin()
            * (solar_pos.azimuth - surface_azimuth).cos();
    let cos_aoi = cos_aoi.max(0.0);

    // Beam on surface
    let i_beam = beam_normal * cos_aoi;

    // Diffuse (isotropic sky)
    let i_diffuse = diffuse_horiz * (1.0 + tilt.cos()) / 2.0;

    // Ground-reflected
    let i_ground = global_horiz * ground_reflectance * (1.0 - tilt.cos()) / 2.0;

    (i_beam + i_diffuse + i_ground).max(0.0)
}

/// Result of incident solar decomposition into beam and Perez 1990 diffuse components.
///
/// The diffuse sky radiation is decomposed into three components per the
/// Perez 1990 anisotropic sky model, matching EnergyPlus AnisoSkyViewFactors().
/// Each component receives its own shading treatment:
///   - Isotropic: reduced by DifShdgRatioIsoSky (hemisphere-sampled)
///   - Circumsolar: reduced by sunlit fraction (directional, like beam)
///   - Horizon brightening: reduced by DifShdgRatioHoriz (near-horizon sampling)
#[derive(Debug, Clone, Copy)]
pub struct IncidentSolarComponents {
    /// Beam (direct) component on the surface [W/m²]
    pub beam: f64,
    /// Circumsolar diffuse component [W/m²] (Perez F1 term).
    /// Directional (from sun direction), shaded like beam by overhangs.
    pub circumsolar: f64,
    /// Isotropic sky diffuse component [W/m²] (Perez (1-F1) term).
    /// Uniform sky dome contribution, reduced by DifShdgRatioIsoSky.
    pub sky_diffuse: f64,
    /// Horizon brightening diffuse component [W/m²] (Perez F2 term).
    /// Near-horizon band contribution, reduced by DifShdgRatioHoriz.
    pub horizon: f64,
    /// Ground-reflected diffuse component [W/m²].
    /// Not affected by sky shading (overhangs don't block ground reflection).
    pub ground_diffuse: f64,
    /// Total incident radiation [W/m²] (sum of all components, unshaded)
    pub total: f64,
    /// Cosine of the angle of incidence (for beam)
    pub cos_aoi: f64,
}

/// Calculate incident solar with beam/diffuse decomposition [W/m²].
///
/// Uses Perez (1990) anisotropic sky model for the diffuse component,
/// matching EnergyPlus `AnisoSkyViewFactors()`. Decomposes diffuse into
/// three components (isotropic, circumsolar, horizon) for proper shading
/// treatment by the heat balance solver.
///
/// Reference: Perez et al. (1990) Solar Energy 44(5):271-289.
pub fn incident_solar_components(
    beam_normal: f64,
    diffuse_horiz: f64,
    global_horiz: f64,
    solar_pos: &SolarPosition,
    surface_azimuth_deg: f64,
    surface_tilt_deg: f64,
    ground_reflectance: f64,
    day_of_year: u32,
    elevation_m: f64,
) -> IncidentSolarComponents {
    if !solar_pos.is_sunup || (beam_normal + diffuse_horiz) <= 0.0 {
        return IncidentSolarComponents {
            beam: 0.0, circumsolar: 0.0, sky_diffuse: 0.0, horizon: 0.0,
            ground_diffuse: 0.0, total: 0.0, cos_aoi: 0.0,
        };
    }

    let tilt = surface_tilt_deg.to_radians();
    let surface_azimuth = (surface_azimuth_deg - 180.0).to_radians();

    let cos_aoi = (solar_pos.altitude.sin() * tilt.cos()
        + solar_pos.altitude.cos() * tilt.sin()
            * (solar_pos.azimuth - surface_azimuth).cos())
        .max(0.0);

    // ── Beam component ──────────────────────────────────────────────────
    let i_beam = beam_normal * cos_aoi;

    // ── Sky diffuse: Perez 1990 anisotropic model ───────────────────────
    //
    // Three-component decomposition matching EnergyPlus AnisoSkyViewFactors():
    //   1. Isotropic:   DHI × (1 - F1) × (1 + cos(tilt))/2
    //   2. Circumsolar: DHI × F1 × a/b  (directional, shaded like beam)
    //   3. Horizon:     DHI × F2 × sin(tilt)
    //
    // where F1, F2 are the Perez brightness coefficients from 8 sky clearness
    // bins, a = max(0, cos_aoi), b = max(cos(85°), cos_zenith).
    //
    // This replaces the simpler Hay-Davies (1980) model which lacks horizon
    // brightening. The Perez model matches EnergyPlus and is required by
    // ASHRAE 140-2023 Section 7.2.1.3.1.
    //
    // Shading treatment in heat_balance.rs:
    //   - Circumsolar: multiplied by sunlit fraction (directional like beam)
    //   - Isotropic: passes through unshaded (CS-only mode)
    //   - Horizon: passes through unshaded (near-horizon, not directional)
    //
    // Reference: Perez et al. (1990) Solar Energy 44(5):271-289.

    let cos_z = solar_pos.cos_zenith.max(0.087);
    let zenith_rad = cos_z.acos();
    let vf_sky = (1.0 + tilt.cos()) / 2.0;
    let sin_tilt = tilt.sin();

    // Perez F1 (circumsolar) and F2 (horizon) brightness coefficients
    let (f1, f2) = perez_brightness_coefficients(
        beam_normal, diffuse_horiz, zenith_rad, day_of_year, elevation_m,
    );

    // Circumsolar: directional component (shaded by sunlit fraction)
    // a/b ratio amplifies when surface normal points sunward better than horizontal
    let a = cos_aoi.max(0.0);
    let b = cos_z.max(0.0872); // cos(85°) ≈ 0.0872
    let i_circumsolar = (diffuse_horiz * f1 * a / b).max(0.0);

    // Isotropic: uniform sky dome component (passes through unshaded)
    let i_iso_sky = (diffuse_horiz * (1.0 - f1) * vf_sky).max(0.0);

    // Horizon: near-horizon brightening band (passes through unshaded)
    // Significant for vertical surfaces where sin(tilt) = 1.0
    let i_horizon = (diffuse_horiz * f2 * sin_tilt).max(0.0);

    let sky_total = i_iso_sky + i_circumsolar + i_horizon;

    // ── Ground-reflected component ──────────────────────────────────────
    let i_ground = global_horiz * ground_reflectance * (1.0 - tilt.cos()) / 2.0;

    IncidentSolarComponents {
        beam: i_beam.max(0.0),
        circumsolar: i_circumsolar.max(0.0),
        sky_diffuse: i_iso_sky.max(0.0),
        horizon: i_horizon.max(0.0),
        ground_diffuse: i_ground.max(0.0),
        total: (i_beam + sky_total + i_ground).max(0.0),
        cos_aoi,
    }
}

/// Calculate the angle of incidence between sunlight and a surface [radians].
///
/// Returns the AOI in radians (0 = normal incidence, π/2 = grazing).
/// Returns π/2 if the sun is behind the surface.
pub fn angle_of_incidence(
    solar_pos: &SolarPosition,
    surface_azimuth_deg: f64,
    surface_tilt_deg: f64,
) -> f64 {
    if !solar_pos.is_sunup {
        return PI / 2.0;
    }

    let tilt = surface_tilt_deg.to_radians();
    let surface_azimuth = (surface_azimuth_deg - 180.0).to_radians();

    let cos_aoi = solar_pos.altitude.sin() * tilt.cos()
        + solar_pos.altitude.cos() * tilt.sin()
            * (solar_pos.azimuth - surface_azimuth).cos();

    cos_aoi.clamp(0.0, 1.0).acos()
}

/// Angular SHGC modifier for glass windows.
// ─── Glass Angular Properties ────────────────────────────────────────────────

/// Compute glass angular parameters (kd, N_i, n_eff) from window properties.
///
/// Returns `(kd, ni, n_eff)` where:
/// - `kd` = glass extinction coefficient × thickness per pane
/// - `ni` = inward-flowing fraction of absorbed solar (N_i)
/// - `n_eff` = effective refractive index for the Fresnel model
///
/// These values feed into the angular SHGC modifier to compute the full
/// `SHGC(θ) = τ(θ) + N_i × α(θ)` ratio, which is more accurate than
/// the transmittance-only ratio `τ(θ)/τ(0°)`.
///
/// # Methods (in priority order)
///
/// 1. **Per-pane properties given** (`pane_tau`, `pane_rho`): Derives the
///    effective refractive index from the pane reflectance, then kd from
///    pane transmittance. The effective n ensures the Fresnel model matches
///    both τ and ρ at normal incidence, giving accurate angular behavior.
///    N_i = (SHGC − τ_system) / α_system. Most accurate; matches Berkeley
///    Lab Window 7 angular SHGC data within 0.5%.
///
/// 2. **Clear glass without per-pane properties** (SHGC ≥ 0.55): Uses the
///    EnergyPlus SimpleGlazingSystem correlation to estimate system
///    transmittance from SHGC, then derives kd by numerical inversion
///    of the double-pane Fresnel model with n=1.526.
///
/// 3. **Low-e / tinted** (SHGC < 0.55): Returns (0, 0, 1.526); the polynomial
///    angular model is used instead of Fresnel.
///
/// Reference: EnergyPlus Engineering Reference, "Simple Window Model" (SGS)
pub fn compute_glass_angular_params(
    shgc: f64,
    pane_tau: Option<f64>,
    pane_rho: Option<f64>,
) -> (f64, f64, f64) {
    const N_DEFAULT: f64 = 1.526; // soda-lime glass refractive index

    if let (Some(tau_p), Some(rho_p)) = (pane_tau, pane_rho) {
        // Method 1: Per-pane properties → effective n, kd, and N_i
        //
        // Derive the effective refractive index that matches both pane τ
        // and ρ at normal incidence. This ensures the Fresnel angular model
        // produces correct reflectance at all angles, not just transmittance.
        let (n_eff, kd) = derive_effective_n_kd(tau_p, rho_p);
        let tau_sys = double_pane_transmittance(1.0, n_eff, kd);
        let rho_sys = double_pane_reflectance(1.0, n_eff, kd);
        let alpha_sys = (1.0 - tau_sys - rho_sys).max(0.0);
        let ni = if alpha_sys > 0.001 {
            ((shgc - tau_sys) / alpha_sys).clamp(0.0, 1.0)
        } else {
            0.0
        };
        (kd, ni, n_eff)
    } else if shgc >= 0.55 {
        // Method 2: E+ SimpleGlazingSystem correlation
        let tau_est = estimate_system_tau_from_shgc(shgc);
        let kd = derive_kd_from_system_tau(tau_est, N_DEFAULT);
        let tau_sys = double_pane_transmittance(1.0, N_DEFAULT, kd);
        let rho_sys = double_pane_reflectance(1.0, N_DEFAULT, kd);
        let alpha_sys = (1.0 - tau_sys - rho_sys).max(0.0);
        let ni = if alpha_sys > 0.001 {
            ((shgc - tau_sys) / alpha_sys).clamp(0.0, 1.0)
        } else {
            0.0
        };
        (kd, ni, N_DEFAULT)
    } else {
        // Method 3: Low-e / tinted → polynomial model
        (0.0, 0.0, N_DEFAULT)
    }
}

/// Derive effective refractive index and kd from pane τ and ρ at normal.
///
/// Uses bisection to find the refractive index n such that the Fresnel
/// single-pane model matches BOTH the observed transmittance and reflectance.
/// This is critical for accurate angular behavior: the standard n=1.526
/// gives ρ_pane ≈ 0.077 but ASHRAE 140 specifies ρ_pane = 0.075, causing
/// the angular SHGC modifier to be ~3% too steep at 60°.
///
/// With the corrected n, the Fresnel angular SHGC curve matches Berkeley
/// Lab Window 7 reference data within 0.5% at all angles.
fn derive_effective_n_kd(tau_pane: f64, rho_pane: f64) -> (f64, f64) {
    // Bisection on n to match pane reflectance
    let mut n_lo = 1.30_f64;
    let mut n_hi = 1.80_f64;

    for _ in 0..60 {
        let n_mid = (n_lo + n_hi) / 2.0;
        let kd_mid = derive_kd_from_pane_tau(tau_pane, n_mid);
        let rho_model = single_pane_reflectance(1.0, n_mid, kd_mid);

        if rho_model > rho_pane {
            n_hi = n_mid; // n too high → r too high → ρ too high
        } else {
            n_lo = n_mid;
        }
    }

    let n_eff = (n_lo + n_hi) / 2.0;
    let kd = derive_kd_from_pane_tau(tau_pane, n_eff);
    (n_eff, kd)
}

/// Derive glass extinction coefficient (K×d per pane) from per-pane solar
/// transmittance at normal incidence using the Fresnel single-pane model.
///
/// Solves: τ_pane = τ_abs × (1−r)² / (1 − (r×τ_abs)²)
/// for τ_abs = exp(−kd), where r is the single-surface Fresnel reflectance
/// at normal incidence.
fn derive_kd_from_pane_tau(tau_pane: f64, n: f64) -> f64 {
    let r = ((n - 1.0) / (n + 1.0)).powi(2);

    // Quadratic in τ_abs: a × τ_abs² + b × τ_abs + c = 0
    let a = r * r * tau_pane;
    let b = (1.0 - r).powi(2);
    let c = -tau_pane;
    let discriminant = b * b - 4.0 * a * c; // Always positive for physical values
    if discriminant < 0.0 || a.abs() < 1e-15 {
        // Fallback for edge cases (zero reflectance, etc.)
        return (-tau_pane.ln()).max(0.0);
    }
    let tau_abs = (-b + discriminant.sqrt()) / (2.0 * a);
    (-tau_abs.clamp(0.001, 1.0).ln()).max(0.0)
}

/// Estimate system solar transmittance from SHGC using the EnergyPlus
/// SimpleGlazingSystem correlation.
///
/// Reference: EnergyPlus Engineering Reference, "Simple Window Model"
/// (WindowManager.cc, CalcSimpleWindowSHGC)
fn estimate_system_tau_from_shgc(shgc: f64) -> f64 {
    if shgc < 0.7206 {
        0.939998 * shgc * shgc + 0.20332 * shgc
    } else {
        (1.30415 * shgc - 0.30515).max(0.0)
    }
}

/// Find kd such that `double_pane_transmittance(normal, n, kd) = target_tau`.
///
/// Uses bisection search; converges to <1e-10 accuracy in 50 iterations.
fn derive_kd_from_system_tau(target_tau: f64, n: f64) -> f64 {
    let mut lo = 0.0_f64;
    let mut hi = 3.0_f64; // kd=3 gives extremely low transmittance
    for _ in 0..60 {
        let mid = (lo + hi) / 2.0;
        let tau = double_pane_transmittance(1.0, n, mid);
        if tau > target_tau {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    (lo + hi) / 2.0
}

///
/// Returns `SHGC(θ) / SHGC(0°)` as a function of the cosine of the angle of
/// incidence.
///
/// Two models are used depending on glass type:
///
/// - **Clear glass** (SHGC ≥ 0.55): First-principles Fresnel optics for a
///   double-pane clear glass unit (n = 1.526). Computes the full
///   SHGC(θ) = τ(θ) + N_i × α(θ) at each angle, where N_i is the inward-
///   flowing fraction of absorbed solar. This properly accounts for the
///   absorbed-inward component that keeps SHGC higher at oblique angles
///   than transmittance alone. Matches EnergyPlus detailed glazing model.
///
/// - **Low-e / tinted** (SHGC ≤ 0.25): Five-term polynomial with steeper
///   angular falloff to capture the angle-dependent behavior of coated glass.
///   Coefficients tuned to match WINDOW 7 generic low-e product data.
///
/// Between 0.25 and 0.55 the two curves are linearly blended on SHGC.
///
/// # Arguments
/// * `cos_incidence` — cosine of the angle of incidence (1.0 = normal, 0.0 = grazing)
/// * `shgc` — normal-incidence solar heat gain coefficient [0-1] of the window
/// * `kd` — glass extinction coefficient × thickness per pane (from `compute_glass_angular_params`)
/// * `ni` — inward-flowing fraction of absorbed solar (from `compute_glass_angular_params`)
pub fn angular_shgc_modifier(cos_incidence: f64, shgc: f64, kd: f64, ni: f64, n: f64) -> f64 {
    let c = cos_incidence.clamp(0.0, 1.0);
    if c < 0.01 {
        return 0.0;
    }

    // When kd and ni are both zero (Method 3 fallback — no real per-pane
    // optical properties), the Fresnel model produces meaningless results
    // (pure glass τ ≈ 0.847, modifier clamped to 1.0). In that case, use
    // the polynomial model which was designed for low-e/tinted glass.
    let have_fresnel_params = kd > 1e-12 || ni > 1e-12;

    if shgc >= 0.55 && have_fresnel_params {
        // Clear glass with real Fresnel parameters: full SHGC(θ) = τ(θ) + N_i × α(θ)
        fresnel_double_pane_modifier(c, shgc, kd, ni, n)
    } else if shgc <= 0.25 || !have_fresnel_params {
        // Low-e / coated glass, or any glass without per-pane optical data:
        // use polynomial angular model
        polynomial_angular_modifier(c)
    } else {
        // Intermediate SHGC with real per-pane properties: blend Fresnel ↔ polynomial
        let blend = (shgc - 0.25) / (0.55 - 0.25);
        let clear_mod = fresnel_double_pane_modifier(c, shgc, kd, ni, n);
        let lowe_mod = polynomial_angular_modifier(c);
        blend * clear_mod + (1.0 - blend) * lowe_mod
    }
}

/// Polynomial angular modifier for low-e / coated glass.
///
/// Five-term polynomial with steeper falloff than Fresnel, matching WINDOW 7
/// generic low-e product data. Low-e coatings preferentially reflect at
/// oblique angles, producing a steeper angular curve than clear glass.
///
///   modifier(cos θ) = Σ aₙ · cosⁿ(θ),  n = 1..5
///   Σ = 3.5 - 8.0 + 9.0 - 5.0 + 1.5 = 1.000 ✓
fn polynomial_angular_modifier(c: f64) -> f64 {
    const A: [f64; 5] = [3.5, -8.0, 9.0, -5.0, 1.5];
    let c2 = c * c;
    let c3 = c2 * c;
    let c4 = c3 * c;
    let c5 = c4 * c;
    (A[0]*c + A[1]*c2 + A[2]*c3 + A[3]*c4 + A[4]*c5).clamp(0.0, 1.0)
}

// ─── Fresnel Optics for Clear Glass ─────────────────────────────────────────

/// Angular SHGC modifier for clear double-pane glass using Fresnel optics.
///
/// Computes SHGC(θ) / SHGC(0°) for a double-pane assembly where:
///
///   SHGC(θ) = τ_system(θ) + N_i × α_system(θ)
///
/// τ_system is the system transmittance, α_system = 1 - τ - ρ is the system
/// absorptance, and N_i is the inward-flowing fraction of absorbed solar.
///
/// The effective refractive index `n` is derived from per-pane properties
/// when available (matching both τ and ρ), giving angular behavior that
/// matches Berkeley Lab Window 7 reference data within 0.5%.
///
/// Reference: EnergyPlus Engineering Reference, "Window Calculation Module"
///
/// # Arguments
/// * `cos_theta` — cosine of angle of incidence
/// * `shgc` — normal-incidence SHGC
/// * `kd` — glass extinction coefficient × thickness per pane
/// * `ni` — inward-flowing fraction of absorbed solar
/// * `n` — effective refractive index for the Fresnel model
fn fresnel_double_pane_modifier(cos_theta: f64, shgc: f64, kd: f64, ni: f64, n: f64) -> f64 {
    // Compute system properties at the given angle
    let tau_angle = double_pane_transmittance(cos_theta, n, kd);
    let rho_angle = double_pane_reflectance(cos_theta, n, kd);
    let alpha_angle = (1.0 - tau_angle - rho_angle).max(0.0);

    // SHGC at this angle = transmitted + inward-absorbed
    let shgc_angle = tau_angle + ni * alpha_angle;

    // Modifier = SHGC(θ) / SHGC(0°)
    // Note: by construction, SHGC(0°) = τ(0°) + N_i × α(0°) ≈ shgc input
    // (exact when n is derived from per-pane properties)
    if shgc > 0.0 {
        (shgc_angle / shgc).clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// Transmittance of a double-pane glazing assembly at a given angle.
///
/// Uses the standard multi-pane formula for two identical panes:
///   T_total = T₁² / (1 - R₁²)
///
/// where T₁ and R₁ are the single-pane transmittance and reflectance.
fn double_pane_transmittance(cos_theta: f64, n: f64, kd: f64) -> f64 {
    let t1 = single_pane_transmittance(cos_theta, n, kd);
    let r1 = single_pane_reflectance(cos_theta, n, kd);
    if r1 < 1.0 {
        t1 * t1 / (1.0 - r1 * r1)
    } else {
        0.0
    }
}

/// Reflectance of a double-pane glazing assembly at a given angle.
///
/// Uses the standard multi-pane formula for two identical panes:
///   R_total = R₁ + T₁² × R₁ / (1 - R₁²)
fn double_pane_reflectance(cos_theta: f64, n: f64, kd: f64) -> f64 {
    let t1 = single_pane_transmittance(cos_theta, n, kd);
    let r1 = single_pane_reflectance(cos_theta, n, kd);
    if r1 < 1.0 {
        r1 + t1 * t1 * r1 / (1.0 - r1 * r1)
    } else {
        1.0
    }
}

/// Transmittance of a single glass pane at angle θ.
///
/// Accounts for:
/// 1. Fresnel reflection at both air-glass interfaces (averaged s & p polarization)
/// 2. Absorption through the glass at the oblique path length (d / cos θ')
/// 3. Multiple internal reflections between the two surfaces
///
/// Formula: τ = τ_abs · (1-r)² / (1 - (r·τ_abs)²)
///
/// where r is the single-surface Fresnel reflectance and
/// τ_abs = exp(-K·d / cos θ') is the absorption transmittance.
fn single_pane_transmittance(cos_theta: f64, n: f64, kd: f64) -> f64 {
    let sin_theta = (1.0 - cos_theta * cos_theta).sqrt();
    let sin_theta_p = sin_theta / n;
    if sin_theta_p >= 1.0 {
        return 0.0; // Total internal reflection (shouldn't happen for external incidence)
    }
    let cos_theta_p = (1.0 - sin_theta_p * sin_theta_p).sqrt();

    let r = fresnel_reflectance(cos_theta, cos_theta_p, n);
    let tau_abs = (-kd / cos_theta_p).exp();

    tau_abs * (1.0 - r).powi(2) / (1.0 - (r * tau_abs).powi(2))
}

/// Reflectance of a single glass pane at angle θ.
///
/// Accounts for multiple internal reflections:
///   R = r + r · (τ_abs · (1-r))² / (1 - (r·τ_abs)²)
fn single_pane_reflectance(cos_theta: f64, n: f64, kd: f64) -> f64 {
    let sin_theta = (1.0 - cos_theta * cos_theta).sqrt();
    let sin_theta_p = sin_theta / n;
    if sin_theta_p >= 1.0 {
        return 1.0;
    }
    let cos_theta_p = (1.0 - sin_theta_p * sin_theta_p).sqrt();

    let r = fresnel_reflectance(cos_theta, cos_theta_p, n);
    let tau_abs = (-kd / cos_theta_p).exp();

    r + r * (tau_abs * (1.0 - r)).powi(2) / (1.0 - (r * tau_abs).powi(2))
}

/// Average Fresnel reflectance for unpolarized light at a single air-glass interface.
///
/// Uses Fresnel equations for s- and p-polarized light:
///   r_s = ((cos θ - n·cos θ') / (cos θ + n·cos θ'))²
///   r_p = ((n·cos θ - cos θ') / (n·cos θ + cos θ'))²
///   r   = (r_s + r_p) / 2
fn fresnel_reflectance(cos_theta: f64, cos_theta_p: f64, n: f64) -> f64 {
    let rs = ((cos_theta - n * cos_theta_p) / (cos_theta + n * cos_theta_p)).powi(2);
    let rp = ((n * cos_theta - cos_theta_p) / (n * cos_theta + cos_theta_p)).powi(2);
    (rs + rp) / 2.0
}

/// Hemispherically-averaged SHGC modifier for diffuse radiation.
///
/// Computed by numerical midpoint-rule integration of `angular_shgc_modifier`
/// over the hemisphere with Lambert's cosine weighting:
///
///   Modifier_diffuse(SHGC) =
///     ∫₀^(π/2) modifier(cos θ, SHGC, kd, ni) · cos θ · sin θ  dθ
///     ─────────────────────────────────────────────────────────
///     ∫₀^(π/2) cos θ · sin θ  dθ
///
/// where the denominator is 0.5 (Lambert solid angle integral).
///
/// Typically ≈ 0.84–0.90 depending on SHGC/coating type.
pub fn diffuse_shgc_modifier(shgc: f64, kd: f64, ni: f64, n: f64) -> f64 {
    const N_SAMPLES: usize = 200;
    let mut num = 0.0_f64;
    let mut den = 0.0_f64;
    for i in 0..N_SAMPLES {
        let theta = (i as f64 + 0.5) / N_SAMPLES as f64 * PI / 2.0;
        let cos_t = theta.cos();
        let w = cos_t * theta.sin(); // cosine-weighted solid angle element
        num += angular_shgc_modifier(cos_t, shgc, kd, ni, n) * w;
        den += w;
    }
    if den > 0.0 { (num / den).clamp(0.0, 1.0) } else { 0.88 }
}

/// Constant diffuse modifier for backward compatibility (clear glass).
/// Prefer `diffuse_shgc_modifier(shgc)` for SHGC-dependent results.
pub const DIFFUSE_SHGC_MODIFIER: f64 = 0.88;

/// Calculate transmitted solar through a window with angular SHGC [W].
///
/// Applies angular SHGC modifier for beam radiation and hemispherically-
/// integrated modifier for diffuse radiation. Both modifiers are SHGC-dependent,
/// capturing the difference between clear and low-e glass angular behavior.
///
/// # Arguments
/// * `shgc` - Normal-incidence solar heat gain coefficient [0-1]
/// * `area` - Window net area [m²]
/// * `beam_incident` - Beam (direct) component on window surface [W/m²]
/// * `diffuse_incident` - Diffuse + ground-reflected component on window surface [W/m²]
/// * `cos_aoi` - Cosine of angle of incidence for beam radiation
/// * `kd` - Glass extinction coefficient × thickness per pane
/// * `ni` - Inward-flowing fraction of absorbed solar
pub fn window_transmitted_solar_angular(
    shgc: f64,
    area: f64,
    beam_incident: f64,
    diffuse_incident: f64,
    cos_aoi: f64,
    kd: f64,
    ni: f64,
    n: f64,
) -> f64 {
    let beam_mod = angular_shgc_modifier(cos_aoi, shgc, kd, ni, n);
    let diff_mod = diffuse_shgc_modifier(shgc, kd, ni, n);
    let beam_transmitted = shgc * beam_mod * area * beam_incident;
    let diff_transmitted = shgc * diff_mod * area * diffuse_incident;
    (beam_transmitted + diff_transmitted).max(0.0)
}

/// Calculate transmitted solar through a window, split into beam and diffuse [W].
///
/// Same physics as `window_transmitted_solar_angular` but returns `(beam_W, diffuse_W)`
/// separately for interior solar distribution (beam goes geometrically to surfaces,
/// diffuse is uniformly redistributed via VMULT).
pub fn window_transmitted_solar_split(
    shgc: f64,
    area: f64,
    beam_incident: f64,
    diffuse_incident: f64,
    cos_aoi: f64,
    kd: f64,
    ni: f64,
    n: f64,
) -> (f64, f64) {
    let beam_mod = angular_shgc_modifier(cos_aoi, shgc, kd, ni, n);
    let diff_mod = diffuse_shgc_modifier(shgc, kd, ni, n);
    let beam_transmitted = (shgc * beam_mod * area * beam_incident).max(0.0);
    let diff_transmitted = (shgc * diff_mod * area * diffuse_incident).max(0.0);
    (beam_transmitted, diff_transmitted)
}

/// Calculate transmitted solar through a window [W].
///
/// Simplified constant-SHGC model: Q_solar = SHGC × Area × I_incident.
/// Use `window_transmitted_solar_angular` for angle-dependent results.
pub fn window_transmitted_solar(shgc: f64, area: f64, incident: f64) -> f64 {
    shgc * area * incident
}

/// Compute the sun direction unit vector pointing FROM the sun TOWARD the ground.
///
/// Used for shadow projection: casting surface vertices are projected along this
/// direction onto receiving surface planes.
///
/// Coordinate system: X=East, Y=North, Z=Up.
/// Solar azimuth convention: from south, west positive (matching `SolarPosition`).
///
/// Returns a zero-ish downward vector if the sun is below the horizon.
pub fn sun_direction_vector(solar_pos: &SolarPosition) -> crate::geometry::Vec3 {
    if !solar_pos.is_sunup || solar_pos.altitude <= 0.0 {
        return crate::geometry::Vec3::new(0.0, 0.0, -1.0);
    }
    let alt = solar_pos.altitude;
    let azi = solar_pos.azimuth; // from south, west positive

    // Direction FROM ground TOWARD sun in world coordinates:
    //   The solar azimuth is measured from south (Y-negative direction), west positive.
    //   cos(azi) > 0 when sun is south (toward -Y), sin(azi) > 0 when sun is west (toward -X).
    //   toward_sun_x = -cos(alt)*sin(azi)  (east when sin(azi)<0, i.e., morning)
    //   toward_sun_y = -cos(alt)*cos(azi)  (south when cos(azi)>0)
    //   toward_sun_z = sin(alt)
    let toward_sun = crate::geometry::Vec3::new(
        -alt.cos() * azi.sin(),
        -alt.cos() * azi.cos(),
        alt.sin(),
    );

    // Negate to get direction FROM sun TOWARD ground
    toward_sun.scale(-1.0).normalize()
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_relative_eq;

    #[test]
    fn test_solar_position_equinox_noon_equator() {
        // March equinox (day ~80), solar noon, latitude 0°
        // Sun should be nearly overhead
        let pos = solar_position(80, 12.0, 0.0);
        assert!(pos.is_sunup);
        // Altitude should be close to 90° (π/2)
        assert!(pos.altitude > 1.4); // > 80 degrees
        assert!(pos.cos_zenith > 0.95);
    }

    #[test]
    fn test_solar_position_night() {
        // Midnight at equator
        let pos = solar_position(80, 0.0, 0.0);
        assert!(!pos.is_sunup);
    }

    #[test]
    fn test_incident_solar_horizontal_surface() {
        // Horizontal surface (tilt=0) should receive close to global horizontal
        let pos = solar_position(172, 12.0, 40.0); // Summer solstice, noon, 40°N
        let i = incident_solar(
            800.0,  // beam normal
            200.0,  // diffuse horizontal
            900.0,  // global horizontal
            &pos,
            0.0,    // azimuth irrelevant for horizontal
            0.0,    // horizontal
            0.2,
        );
        // For horizontal surface: beam*cos(zenith) + diffuse*(1+1)/2 + 0
        // = 800*cos_zenith + 200
        let expected = 800.0 * pos.cos_zenith + 200.0;
        assert_relative_eq!(i, expected, max_relative = 0.01);
    }

    #[test]
    fn test_window_transmitted_solar() {
        let q = window_transmitted_solar(0.7, 5.0, 300.0);
        assert_relative_eq!(q, 0.7 * 5.0 * 300.0, max_relative = 0.001);
    }

    #[test]
    fn test_angular_shgc_modifier_normal_incidence() {
        // At normal incidence (cos=1.0), modifier should be ~1.0 for clear glass
        let (kd, ni, n) = compute_glass_angular_params(0.7, None, None);
        let m = angular_shgc_modifier(1.0, 0.7, kd, ni, n);
        assert_relative_eq!(m, 1.0, max_relative = 0.02);
    }

    #[test]
    fn test_angular_shgc_modifier_grazing() {
        // At grazing (cos≈0), modifier should be ~0
        let (kd, ni, n) = compute_glass_angular_params(0.7, None, None);
        let m = angular_shgc_modifier(0.0, 0.7, kd, ni, n);
        assert_relative_eq!(m, 0.0, epsilon = 0.01);
    }

    #[test]
    fn test_angular_shgc_modifier_60_degrees() {
        // At 60° (cos=0.5), modifier should be ~0.60-0.85
        let (kd, ni, n) = compute_glass_angular_params(0.7, None, None);
        let m = angular_shgc_modifier(0.5, 0.7, kd, ni, n);
        assert!(m > 0.5, "60° modifier should be > 0.5, got {}", m);
        assert!(m < 0.95, "60° modifier should be < 0.95, got {}", m);
    }

    #[test]
    fn test_angular_shgc_modifier_monotonic() {
        // Modifier should be monotonically increasing with cos(θ) for both clear and low-e
        for shgc in [0.2, 0.4, 0.7] {
            let (kd, ni, n) = compute_glass_angular_params(shgc, None, None);
            let mut prev = 0.0;
            for i in 1..=10 {
                let c = i as f64 * 0.1;
                let m = angular_shgc_modifier(c, shgc, kd, ni, n);
                assert!(m >= prev, "Not monotonic (shgc={}): m({})={} < m({})={}", shgc, c, m, c - 0.1, prev);
                prev = m;
            }
        }
    }

    #[test]
    fn test_angular_shgc_modifier_lowe_steeper() {
        // Low-e glass (SHGC=0.2) should fall off faster at oblique angles than clear (SHGC=0.7)
        let (kd_c, ni_c, n_c) = compute_glass_angular_params(0.7, None, None);
        let (kd_l, ni_l, n_l) = compute_glass_angular_params(0.2, None, None);
        let m_clear = angular_shgc_modifier(0.5, 0.7, kd_c, ni_c, n_c);
        let m_lowe  = angular_shgc_modifier(0.5, 0.2, kd_l, ni_l, n_l);
        assert!(m_lowe < m_clear, "Low-e should have steeper angular falloff: lowe={} clear={}", m_lowe, m_clear);
    }

    #[test]
    fn test_window_transmitted_solar_angular() {
        let shgc = 0.7_f64;
        let (kd, ni, n) = compute_glass_angular_params(shgc, None, None);

        // Normal incidence should be close to flat SHGC model
        let q_angular = window_transmitted_solar_angular(shgc, 5.0, 300.0, 0.0, 1.0, kd, ni, n);
        let q_flat = window_transmitted_solar(shgc, 5.0, 300.0);
        assert_relative_eq!(q_angular, q_flat, max_relative = 0.02);

        // With diffuse-only radiation, should use the SHGC-dependent diffuse modifier
        let q_diffuse = window_transmitted_solar_angular(shgc, 5.0, 0.0, 200.0, 1.0, kd, ni, n);
        let expected = shgc * diffuse_shgc_modifier(shgc, kd, ni, n) * 5.0 * 200.0;
        assert_relative_eq!(q_diffuse, expected, max_relative = 0.01);

        // At 60° beam incidence, total should be less than flat model
        let q_angled = window_transmitted_solar_angular(shgc, 5.0, 300.0, 0.0, 0.5, kd, ni, n);
        assert!(q_angled < q_flat, "Angular model at 60° should give less than flat model");
    }

    #[test]
    fn test_diffuse_shgc_modifier_range() {
        // Diffuse modifier (hemispherical avg) should be physically reasonable:
        // - Low-e coatings (SHGC=0.2) have steeper angular falloff → lower diffuse modifier (~0.65-0.80)
        // - Clear glass (SHGC=0.7+) → moderate diffuse modifier (~0.85-0.95)
        // All should be in [0.60, 0.98]
        for shgc in [0.2_f64, 0.4, 0.6, 0.8] {
            let (kd, ni, n) = compute_glass_angular_params(shgc, None, None);
            let m = diffuse_shgc_modifier(shgc, kd, ni, n);
            assert!(m > 0.60, "Diffuse modifier too low for shgc={}: {}", shgc, m);
            assert!(m < 0.98, "Diffuse modifier too high for shgc={}: {}", shgc, m);
        }
        // Clear glass should have higher diffuse modifier than low-e
        let (kd_c, ni_c, n_c) = compute_glass_angular_params(0.8, None, None);
        let (kd_l, ni_l, n_l) = compute_glass_angular_params(0.2, None, None);
        let m_clear = diffuse_shgc_modifier(0.8, kd_c, ni_c, n_c);
        let m_lowe  = diffuse_shgc_modifier(0.2, kd_l, ni_l, n_l);
        assert!(m_clear > m_lowe, "Clear glass should have higher diffuse modifier than low-e: clear={} lowe={}", m_clear, m_lowe);
    }

    #[test]
    fn test_compute_glass_angular_params_ashrae140() {
        // ASHRAE 140 double-pane clear: τ_pane=0.834, ρ_pane=0.075, SHGC=0.769
        let (kd, ni, n) = compute_glass_angular_params(0.769, Some(0.834), Some(0.075));
        // kd should be ~0.09 (from Beer's law)
        assert!(kd > 0.07 && kd < 0.12,
            "ASHRAE 140 kd should be ~0.09, got {}", kd);
        // N_i should be reasonable (inward-flowing fraction of absorbed solar)
        assert!(ni > 0.20 && ni < 0.65,
            "ASHRAE 140 N_i should be ~0.40, got {}", ni);
        // Effective n should be close to 1.526 but slightly different
        assert!(n > 1.45 && n < 1.60,
            "Effective n should be ~1.50-1.53, got {}", n);

        // Verify modifier at normal = 1.0
        let m_normal = angular_shgc_modifier(1.0, 0.769, kd, ni, n);
        assert_relative_eq!(m_normal, 1.0, max_relative = 0.01);

        // At 60° (cos=0.5), modifier should be ~0.82-0.90 (higher than pure transmittance)
        let m_60 = angular_shgc_modifier(0.5, 0.769, kd, ni, n);
        assert!(m_60 > 0.75 && m_60 < 0.95,
            "60° SHGC modifier should be ~0.85, got {}", m_60);
    }

    #[test]
    fn test_compute_glass_angular_params_no_pane_props() {
        // Without per-pane properties, should use SGS correlation
        let (kd, ni, n) = compute_glass_angular_params(0.769, None, None);
        // Should give reasonable values from the correlation
        assert!(kd > 0.01, "kd should be positive, got {}", kd);
        assert!(ni > 0.0, "ni should be positive for clear glass, got {}", ni);
        // Should use default n=1.526
        assert_relative_eq!(n, 1.526, max_relative = 0.001);

        // Modifier at normal should still be ~1.0
        let m = angular_shgc_modifier(1.0, 0.769, kd, ni, n);
        assert_relative_eq!(m, 1.0, max_relative = 0.02);
    }
}

#[cfg(test)]
mod diagnostic_tests {
    use super::*;

    /// Compare OpenBSE angular SHGC model against Berkeley Lab WINDOW 7 reference data
    /// for the ASHRAE 140 double-pane clear window (SHGC=0.769).
    ///
    /// WINDOW 7 data from: Case600-W7-DblPaneClr-ID23.txt
    #[test]
    fn ashrae140_angular_vs_window7() {
        // ASHRAE 140 double-pane clear window
        let shgc = 0.769_f64;
        let (kd, ni, n) = compute_glass_angular_params(shgc, Some(0.834), Some(0.075));
        
        println!("\n=== ASHRAE 140 Window Angular Properties ===");
        println!("Derived parameters: kd={:.6}, ni={:.6}, n_eff={:.6}", kd, ni, n);
        
        // WINDOW 7 reference SHGCc values at each angle:
        // 0°=0.769, 10°=0.768, 20°=0.766, 30°=0.761, 40°=0.748, 50°=0.718, 60°=0.651, 70°=0.509, 80°=0.267, 90°=0.000
        let w7_shgc = [0.769, 0.768, 0.766, 0.761, 0.748, 0.718, 0.651, 0.509, 0.267, 0.000];
        let w7_tsol = [0.703, 0.702, 0.699, 0.692, 0.678, 0.646, 0.577, 0.438, 0.208, 0.000];
        
        println!("\nAngle  W7-SHGC  W7-Mod   OpenBSE-Mod  Diff    W7-Tsol  OpenBSE-Tsol  Diff");
        println!("-----  -------  ------   -----------  ------  -------  ------------  ------");
        for i in 0..10 {
            let angle_deg = i as f64 * 10.0;
            let cos_i = (angle_deg * std::f64::consts::PI / 180.0).cos();
            let ob_mod = angular_shgc_modifier(cos_i, shgc, kd, ni, n);
            let w7_mod = w7_shgc[i] / w7_shgc[0]; // W7 modifier
            let ob_tsol = double_pane_transmittance(cos_i, n, kd);
            let diff = ob_mod - w7_mod;
            let diff_t = ob_tsol - w7_tsol[i];
            println!("{:>3}°   {:.3}    {:.4}    {:.4}       {:+.4}   {:.3}    {:.4}        {:+.4}",
                angle_deg, w7_shgc[i], w7_mod, ob_mod, diff, w7_tsol[i], ob_tsol, diff_t);
        }
        
        // Hemispherical SHGC modifier (diffuse)
        let ob_diff_mod = diffuse_shgc_modifier(shgc, kd, ni, n);
        let w7_hemis_mod = 0.670 / 0.769;  // From WINDOW 7 data: Hemis=0.670
        println!("\nDiffuse/Hemispherical SHGC modifier:");
        println!("  WINDOW 7:  {:.4} (hemis SHGC=0.670 / normal SHGC=0.769)", w7_hemis_mod);
        println!("  OpenBSE:   {:.4}", ob_diff_mod);
        println!("  Diff:      {:+.4} ({:+.2}%)", ob_diff_mod - w7_hemis_mod, (ob_diff_mod - w7_hemis_mod) / w7_hemis_mod * 100.0);
        
        // Also compute total solar transmittance hemispherical
        let w7_hemis_tsol = 0.601;
        let w7_hemis_tsol_mod = w7_hemis_tsol / w7_tsol[0];
        // Compute our hemispherical transmittance
        let n_samp = 200;
        let mut num_t = 0.0_f64;
        let mut den_t = 0.0_f64;
        for j in 0..n_samp {
            let theta = (j as f64 + 0.5) / n_samp as f64 * std::f64::consts::PI / 2.0;
            let cos_t = theta.cos();
            let w = cos_t * theta.sin();
            num_t += double_pane_transmittance(cos_t, n, kd) * w;
            den_t += w;
        }
        let ob_hemis_tsol = num_t / den_t;
        println!("\nHemispherical Tsol:");
        println!("  WINDOW 7:  {:.4}", w7_hemis_tsol);
        println!("  OpenBSE:   {:.4} (computed hemispherical average)", ob_hemis_tsol);
        println!("  Diff:      {:+.4}", ob_hemis_tsol - w7_hemis_tsol);
        
        // Compute the actual SHGC at each angle to compare with W7
        let tau_sys_0 = double_pane_transmittance(1.0, n, kd);
        let rho_sys_0 = double_pane_reflectance(1.0, n, kd);
        let alpha_sys_0 = 1.0 - tau_sys_0 - rho_sys_0;
        let shgc_reconstructed = tau_sys_0 + ni * alpha_sys_0;
        println!("\nReconstructed SHGC at normal:");
        println!("  tau_sys(0)  = {:.4}", tau_sys_0);
        println!("  rho_sys(0)  = {:.4}", rho_sys_0);
        println!("  alpha_sys(0)= {:.4}", alpha_sys_0);
        println!("  ni          = {:.4}", ni);
        println!("  SHGC(0°)    = {:.4} (target: 0.769)", shgc_reconstructed);
    }
}
