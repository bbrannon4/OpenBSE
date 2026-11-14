//! Solar position and incident radiation calculations.
//!
//! - Solar position (altitude, azimuth) from date/time/latitude
//! - Incident solar on tilted surfaces (Perez 1990 anisotropic sky model)
//! - Window solar transmission (angular-dependent SHGC with Fresnel optics)
//!
//! Reference: ASHRAE Fundamentals Ch. 14, Spencer (1971),
//! Perez et al. (1990) Solar Energy 44(5):271-289.

use std::f64::consts::PI;

/// The sun counts as up from this cos(zenith), E+ `SunIsUpValue`.
const SUN_IS_UP_COS_ZENITH: f64 = 1e-5;

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
pub fn solar_position(day_of_year: u32, solar_hour: f64, latitude_deg: f64) -> SolarPosition {
    let lat = latitude_deg.to_radians();

    // Solar declination — Spencer (1971)
    let day_angle = 2.0 * PI * (day_of_year as f64 - 1.0) / 365.0;
    let declination = 0.006918 - 0.399912 * day_angle.cos() + 0.070257 * day_angle.sin()
        - 0.006758 * (2.0 * day_angle).cos()
        + 0.000907 * (2.0 * day_angle).sin()
        - 0.002697 * (3.0 * day_angle).cos()
        + 0.00148 * (3.0 * day_angle).sin();

    // Hour angle (negative before noon, positive after)
    let hour_angle = (solar_hour - 12.0) * 15.0_f64.to_radians();

    // Solar altitude
    let sin_alt = lat.sin() * declination.sin() + lat.cos() * declination.cos() * hour_angle.cos();
    let altitude = sin_alt.clamp(-1.0, 1.0).asin();

    let cos_zenith = sin_alt.max(0.0);

    // Solar azimuth (from south, west positive)
    let cos_alt = altitude.cos().max(0.001);
    let sin_azimuth = declination.cos() * hour_angle.sin() / cos_alt;
    let cos_azimuth = (sin_alt * lat.sin() - declination.sin()) / (cos_alt * lat.cos().max(0.001));
    let azimuth = sin_azimuth.atan2(cos_azimuth);

    SolarPosition {
        altitude,
        azimuth,
        cos_zenith,
        // E+ `DataEnvironment::SunIsUpValue`: no solar below this cos(zenith).
        is_sunup: cos_zenith >= SUN_IS_UP_COS_ZENITH,
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

/// Local solar hour from clock time, applying the longitude (standard
/// meridian) correction and the equation of time.
///
///   solar_hour = clock_hour + (longitude/15 − time_zone) + EOT
///
/// Conventions match EPW headers: `longitude_deg` is east-positive
/// (Denver ≈ −104.65), `time_zone_hr` is hours from UTC (Denver = −7,
/// standard meridian = 15 × time_zone). A site east of its meridian
/// sees solar noon before clock noon, so the correction is positive.
pub fn local_solar_hour(
    clock_hour: f64,
    time_zone_hr: f64,
    longitude_deg: f64,
    day_of_year: u32,
) -> f64 {
    clock_hour + (longitude_deg / 15.0 - time_zone_hr) + equation_of_time(day_of_year)
}

// ─── Anisotropic Sky Diffuse Model ────────────────────────────────────────

// ─── Perez 1990 Anisotropic Sky Model ───────────────────────────────────────
//
// Decomposes sky diffuse into three components for proper shading treatment:
//   1. Isotropic background — uniform sky dome, reduced by DifShdgRatioIsoSky
//   2. Circumsolar brightening — directional, reduced by sunlit fraction
//   3. Horizon brightening — near-horizon band, reduced by DifShdgRatioHoriz
//
// References:
//   Perez et al. (1990) Solar Energy 44(5):271-289
//   EnergyPlus SolarShading.cc, AnisoSkyViewFactors()

/// Perez 1990 sky clearness bin boundaries.
/// Creates 8 bins: [≤1.065], [1.065-1.23], ..., [>6.2]
const PEREZ_EPSILON_LIMITS: [f64; 7] = [1.065, 1.23, 1.5, 1.95, 2.8, 4.5, 6.2];

/// Perez circumsolar brightness coefficients (F1 = F11 + F12·Δ + F13·θz).
/// From EnergyPlus SolarShading.cc, provided by R. Perez (private comm., 1999).
const PEREZ_F11: [f64; 8] = [
    -0.0083117, 0.1299457, 0.3296958, 0.5682053, 0.8730280, 1.1326077, 1.0601591, 0.6777470,
];
const PEREZ_F12: [f64; 8] = [
    0.5877285, 0.6825954, 0.4868735, 0.1874525, -0.3920403, -1.2367284, -1.5999137, -0.3272588,
];
const PEREZ_F13: [f64; 8] = [
    -0.0620636, -0.1513752, -0.2210958, -0.2951290, -0.3616149, -0.4118494, -0.3589221, -0.2504286,
];

/// Perez horizon/zenith brightness coefficients (F2 = F21 + F22·Δ + F23·θz).
const PEREZ_F21: [f64; 8] = [
    -0.0596012, -0.0189325, 0.0554140, 0.1088631, 0.2255647, 0.2877813, 0.2642124, 0.1561313,
];
const PEREZ_F22: [f64; 8] = [
    0.0721249, 0.0659650, -0.0639588, -0.1519229, -0.4620442, -0.8230357, -1.1272340, -1.3765031,
];
const PEREZ_F23: [f64; 8] = [
    -0.0220216, -0.0288748, -0.0260542, -0.0139754, 0.0012448, 0.0558651, 0.1310694, 0.2506212,
];

/// Compute Perez F1 (circumsolar) and F2 (horizon) brightness coefficients.
///
/// Used to determine the fraction of diffuse radiation that is truly isotropic
/// vs. circumsolar vs. horizon brightening. This decomposition is needed for
/// proper application of diffuse sky shading ratios.
///
/// Returns (F1, F2) where F1 ≥ 0. F2 is negative under many overcast skies. As in
/// E+, the zenith angle is the true one, which must be below 90° (sun up).
pub fn perez_brightness_coefficients(
    beam_normal: f64,
    diffuse_horiz: f64,
    zenith_angle_rad: f64,
    _day_of_year: u32,
    elevation_m: f64,
) -> (f64, f64) {
    if diffuse_horiz <= 0.0 {
        return (0.0, 0.0);
    }

    // Sky clearness ε (EnergyPlus SolarShading.cc line 2681)
    let kappa_z3 = 1.041 * zenith_angle_rad.powi(3);
    let epsilon = ((beam_normal + diffuse_horiz) / diffuse_horiz + kappa_z3) / (1.0 + kappa_z3);

    // Sky brightness Δ (Perez et al. 1990)
    // Air mass with altitude pressure correction, matching EnergyPlus
    // SolarShading.cc AnisoSkyViewFactors() exactly:
    //   AirMassH = (1 - 0.1 * Elevation/1000)
    //   AirMass = AirMassH / cos(z)                        for z <= 75°
    //   AirMass = AirMassH / (cos(z) + 0.15*(93.9-z)^-1.253)  for z > 75°
    let air_mass_h = 1.0 - 0.1 * elevation_m / 1000.0;
    let cz = zenith_angle_rad.cos();
    let zenith_deg = zenith_angle_rad.to_degrees();
    let air_mass = if zenith_deg <= 75.0 {
        air_mass_h / cz
    } else {
        air_mass_h / (cz + 0.15 * (93.9 - zenith_deg).powf(-1.253))
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
        + solar_pos.altitude.cos() * tilt.sin() * (solar_pos.azimuth - surface_azimuth).cos();
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
    /// Uniform sky dome contribution, reduced by DifShdgRatioIsoSky. May be
    /// slightly negative when F1 exceeds 1.
    pub sky_diffuse: f64,
    /// Horizon brightening diffuse component [W/m²] (Perez F2 term).
    /// Near-horizon band contribution, reduced by DifShdgRatioHoriz. Negative
    /// when F2 is, as under many overcast skies.
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
            beam: 0.0,
            circumsolar: 0.0,
            sky_diffuse: 0.0,
            horizon: 0.0,
            ground_diffuse: 0.0,
            total: 0.0,
            cos_aoi: 0.0,
        };
    }

    let tilt = surface_tilt_deg.to_radians();
    let surface_azimuth = (surface_azimuth_deg - 180.0).to_radians();

    let cos_aoi = (solar_pos.altitude.sin() * tilt.cos()
        + solar_pos.altitude.cos() * tilt.sin() * (solar_pos.azimuth - surface_azimuth).cos())
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
    // bins, a = max(0, cos_aoi), b = max(cos(85°), cos_zenith). The 85° limit
    // applies only to this denominator; the coefficients use the true zenith.
    //
    // F2 is often negative under overcast skies, so the horizon term may darken
    // the surface. E+ limits only the sum of the three, after shading, at zero
    // (`SurfAnisoSkyMult`): see `sky_diffuse_after_shading`.

    let cos_z = solar_pos.cos_zenith;
    let zenith_rad = cos_z.acos();
    let vf_sky = (1.0 + tilt.cos()) / 2.0;
    let sin_tilt = tilt.sin();

    // Perez F1 (circumsolar) and F2 (horizon) brightness coefficients
    let (f1, f2) = perez_brightness_coefficients(
        beam_normal,
        diffuse_horiz,
        zenith_rad,
        day_of_year,
        elevation_m,
    );

    // Circumsolar: directional component (shaded by sunlit fraction).
    // With the sun within 5° of the horizon, an almost flat surface takes a
    // factor of 1, E+'s guard against the capped denominator.
    const COS_85_DEG: f64 = 0.0871557;
    let mut circumsolar_factor = cos_aoi / cos_z.max(COS_85_DEG);
    if circumsolar_factor > 0.0 && cos_z < COS_85_DEG && surface_tilt_deg < 2.0 {
        circumsolar_factor = 1.0;
    }
    let i_circumsolar = diffuse_horiz * f1 * circumsolar_factor;

    // Isotropic: uniform sky dome component
    let i_iso_sky = diffuse_horiz * (1.0 - f1) * vf_sky;

    // Horizon: near-horizon brightening band, significant for vertical surfaces
    let i_horizon = diffuse_horiz * f2 * sin_tilt;

    let sky_total = sky_diffuse_after_shading(i_iso_sky, i_circumsolar, i_horizon, 1.0, 1.0, 1.0);

    // ── Ground-reflected component ──────────────────────────────────────
    let i_ground = global_horiz * ground_reflectance * (1.0 - tilt.cos()) / 2.0;

    IncidentSolarComponents {
        beam: i_beam.max(0.0),
        circumsolar: i_circumsolar,
        sky_diffuse: i_iso_sky,
        horizon: i_horizon,
        ground_diffuse: i_ground.max(0.0),
        total: (i_beam + sky_total + i_ground).max(0.0),
        cos_aoi,
    }
}

/// Sky diffuse on a surface after shading each Perez component, limited only as
/// a whole at zero, E+ `SurfAnisoSkyMult`. The horizon component can be negative.
pub fn sky_diffuse_after_shading(
    isotropic: f64,
    circumsolar: f64,
    horizon: f64,
    isotropic_ratio: f64,
    sunlit_fraction: f64,
    horizon_ratio: f64,
) -> f64 {
    (isotropic * isotropic_ratio + circumsolar * sunlit_fraction + horizon * horizon_ratio).max(0.0)
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
        + solar_pos.altitude.cos() * tilt.sin() * (solar_pos.azimuth - surface_azimuth).cos();

    cos_aoi.clamp(0.0, 1.0).acos()
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
    let toward_sun =
        crate::geometry::Vec3::new(-alt.cos() * azi.sin(), -alt.cos() * azi.cos(), alt.sin());

    // Negate to get direction FROM sun TOWARD ground
    toward_sun.scale(-1.0).normalize()
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_relative_eq;

    /// Almanac check for the solar-time longitude correction (NOAA solar
    /// calculator reference). Boston (42.36°N, −71.06°E, UTC−5) sits 3.94°
    /// east of the −75° standard meridian, so solar noon arrives ~15.8 min
    /// before clock noon (plus EOT ≈ −8 min on the March equinox): NOAA
    /// gives solar noon ≈ 11:52 EST on Mar 20 (day 79).
    ///
    /// The pre-fix formula `(time_zone − longitude/15)` had the meridian
    /// correction sign flipped, putting solar noon at ~12:24 EST — over
    /// 30 minutes wrong. Guard against regression.
    #[test]
    fn test_local_solar_hour_boston_noaa() {
        let lon = -71.06;
        let tz = -5.0;
        let doy = 79; // March 20

        // At clock 11:52.4 EST (11.873 h), local solar hour should be 12.0.
        let sh = local_solar_hour(11.873, tz, lon, doy);
        assert!(
            (sh - 12.0).abs() < 2.0 / 60.0,
            "solar hour at NOAA solar noon should be 12.0 ± 2 min, got {:.4}",
            sh
        );

        // Sun position at solar noon: due south (azimuth ≈ 0 from south),
        // altitude ≈ 90 − latitude + declination ≈ 47.2° on the equinox.
        let pos = solar_position(doy, sh, 42.36);
        assert!(
            pos.azimuth.to_degrees().abs() < 1.5,
            "azimuth at solar noon should be ~0° from south, got {:.2}°",
            pos.azimuth.to_degrees()
        );
        assert!(
            (pos.altitude.to_degrees() - 47.2).abs() < 0.6,
            "noon altitude should be ~47.2°, got {:.2}°",
            pos.altitude.to_degrees()
        );
    }

    /// A site exactly on its standard meridian needs only the EOT correction.
    #[test]
    fn test_local_solar_hour_on_meridian() {
        let sh = local_solar_hour(12.0, -7.0, -105.0, 172);
        let eot = equation_of_time(172);
        assert_relative_eq!(sh, 12.0 + eot, max_relative = 1e-12);
    }

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

    /// E+ `AnisoSkyViewFactors` in closed form for checking the code.
    fn eplus_f1_f2(dni: f64, dhi: f64, z: f64, elevation: f64) -> (f64, f64) {
        let deg = z.to_degrees();
        let m_h = 1.0 - 0.1 * elevation / 1000.0;
        let m = if deg <= 75.0 {
            m_h / z.cos()
        } else {
            m_h / (z.cos() + 0.15 * (93.9 - deg).powf(-1.253))
        };
        let k = 1.041 * z.powi(3);
        let eps = ((dni + dhi) / dhi + k) / (1.0 + k);
        let delta = dhi * m / 1353.0;
        let limits = [1.065, 1.23, 1.5, 1.95, 2.8, 4.5, 6.2];
        let b = limits.iter().position(|&l| eps < l).unwrap_or(7);
        (
            (PEREZ_F11[b] + PEREZ_F12[b] * delta + PEREZ_F13[b] * z).max(0.0),
            PEREZ_F21[b] + PEREZ_F22[b] * delta + PEREZ_F23[b] * z,
        )
    }

    fn sun_at_zenith(deg: f64) -> SolarPosition {
        let z = deg.to_radians();
        SolarPosition {
            altitude: PI / 2.0 - z,
            azimuth: 0.0,
            cos_zenith: z.cos(),
            is_sunup: true,
        }
    }

    #[test]
    fn perez_uses_the_true_zenith_at_low_sun() {
        // At 88° the old cap would have evaluated everything at 85°.
        let sun = sun_at_zenith(88.0);
        let (f1, f2) = eplus_f1_f2(100.0, 50.0, 88f64.to_radians(), 1600.0);
        let (dhi, dni) = (50.0, 100.0);
        let c = incident_solar_components(dni, dhi, 0.0, &sun, 180.0, 90.0, 0.2, 200, 1600.0);
        let cos_aoi = c.cos_aoi;
        assert!((c.sky_diffuse - dhi * (1.0 - f1) * 0.5).abs() < 1e-9);
        assert!((c.horizon - dhi * f2).abs() < 1e-9);
        assert!((c.circumsolar - dhi * f1 * cos_aoi / 0.0871557).abs() < 1e-9);
    }

    #[test]
    fn negative_horizon_term_lowers_the_sum_and_the_sum_stops_at_zero() {
        // Overcast and low sun: F2 is negative. E+ keeps it in the sum.
        let (dni, dhi) = (0.0, 200.0);
        let sun = sun_at_zenith(40.0);
        let (f1, f2) = eplus_f1_f2(dni, dhi, 40f64.to_radians(), 0.0);
        assert!(f2 < 0.0, "test needs an overcast bin: F2 = {f2}");
        let c = incident_solar_components(dni, dhi, 0.0, &sun, 180.0, 90.0, 0.2, 200, 0.0);
        assert!(c.horizon < 0.0);
        let cs = dhi * f1 * c.cos_aoi / 40f64.to_radians().cos();
        let expected = dhi * (1.0 - f1) * 0.5 + cs + dhi * f2;
        assert!((c.total - expected).abs() < 1e-9, "{} {expected}", c.total);
        // An isotropic part shaded away leaves only the negative term: zero.
        assert_eq!(
            sky_diffuse_after_shading(10.0, 0.0, -5.0, 0.0, 1.0, 1.0),
            0.0
        );
        assert_eq!(
            sky_diffuse_after_shading(10.0, 2.0, -5.0, 1.0, 1.0, 1.0),
            7.0
        );
    }

    #[test]
    fn flat_surface_circumsolar_factor_is_one_with_a_very_low_sun() {
        let sun = sun_at_zenith(87.0);
        let (f1, _) = eplus_f1_f2(50.0, 40.0, 87f64.to_radians(), 0.0);
        assert!(f1 > 0.0);
        let flat = incident_solar_components(50.0, 40.0, 0.0, &sun, 180.0, 1.0, 0.2, 200, 0.0);
        assert!((flat.circumsolar - 40.0 * f1).abs() < 1e-9);
        let tilted = incident_solar_components(50.0, 40.0, 0.0, &sun, 180.0, 3.0, 0.2, 200, 0.0);
        let factor = tilted.circumsolar / (40.0 * f1);
        assert!((factor - tilted.cos_aoi / 0.0871557).abs() < 1e-9);
    }

    #[test]
    fn sun_up_follows_cos_zenith_not_a_refraction_band() {
        // Solstice noon is up; half a degree below the horizon is not.
        assert!(solar_position(172, 12.0, 40.0).is_sunup);
        let t = (0..2400)
            .map(|h| h as f64 / 100.0)
            .find(|&h| {
                let p = solar_position(172, h, 40.0);
                p.altitude < -0.5_f64.to_radians() && p.altitude > -0.8_f64.to_radians()
            })
            .expect("a time in the band");
        assert!(!solar_position(172, t, 40.0).is_sunup);
    }

    #[test]
    fn test_incident_solar_horizontal_surface() {
        // Horizontal surface (tilt=0) should receive close to global horizontal
        let pos = solar_position(172, 12.0, 40.0); // Summer solstice, noon, 40°N
        let i = incident_solar(
            800.0, // beam normal
            200.0, // diffuse horizontal
            900.0, // global horizontal
            &pos, 0.0, // azimuth irrelevant for horizontal
            0.0, // horizontal
            0.2,
        );
        // For horizontal surface: beam*cos(zenith) + diffuse*(1+1)/2 + 0
        // = 800*cos_zenith + 200
        let expected = 800.0 * pos.cos_zenith + 200.0;
        assert_relative_eq!(i, expected, max_relative = 0.01);
    }
}
