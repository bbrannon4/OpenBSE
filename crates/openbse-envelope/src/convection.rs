//! Interior and exterior convection coefficient models.
//!
//! Interior: TARP natural convection (Walton 1983), matching E+ CalcASHRAETARPNatural.
//! Exterior: DOE-2 combined model (natural + forced via MoWiTT).
//! Reference: EnergyPlus ConvectionCoefficients.cc, Walton (1983).

use crate::material::Roughness;

/// Pure natural convection coefficient [W/(m²·K)].
///
/// Exact match to E+'s `CalcASHRAETARPNatural(Tsurf, Tamb, cosTilt)`.
///
/// The caller is responsible for the sign of `cos_tilt`:
///   - **Interior surfaces:** pass `-cos(tilt)` (E+ negates cosTilt for interior)
///   - **Exterior surfaces:** pass `cos(tilt)` directly (E+ uses exterior cosTilt as-is)
///
/// Stability logic (E+ ConvectionCoefficients.cc:1935):
///   - `same_sign(DeltaTemp, cosTilt)` → **unstable** (enhanced convection)
///   - `opposite_sign(DeltaTemp, cosTilt)` → **stable** (reduced convection)
///   - `cosTilt == 0` or `DeltaTemp == 0` → **vertical** wall formula
///
/// Coefficients (ConvectionCoefficients.hh:394–456):
///   - Vertical:  h = 1.31 · |ΔT|^(1/3)
///   - Unstable:  h = 9.482 · |ΔT|^(1/3) / (7.238 - |cosΣ|)
///   - Stable:    h = 1.810 · |ΔT|^(1/3) / (1.382 + |cosΣ|)
///
/// Minimum: 0.1 W/(m²·K) (E+ `LowHConvLimit`, DataHeatBalance.hh:1790).
fn calc_natural_convection(delta_t: f64, cos_tilt: f64) -> f64 {
    let abs_dt = delta_t.abs().max(0.001);
    let dt_third = abs_dt.powf(1.0 / 3.0);
    let abs_cos = cos_tilt.abs();

    // Near-vertical surfaces or zero ΔT → vertical wall formula.
    // E+ uses exact equality (cosTilt == 0.0); we use small epsilon for
    // floating-point imprecision from cos(90°).
    if abs_cos < 0.01 || delta_t.abs() < 0.001 {
        return (1.31 * dt_third).max(0.1);
    }

    // E+ stability: same sign → unstable (enhanced), opposite → stable (reduced)
    let is_unstable = (delta_t > 0.0 && cos_tilt > 0.0) || (delta_t < 0.0 && cos_tilt < 0.0);

    let h = if is_unstable {
        9.482 * dt_third / (7.238 - abs_cos)
    } else {
        1.810 * dt_third / (1.382 + abs_cos)
    };

    h.max(0.1)
}

/// Interior convection coefficient [W/(m²·K)].
///
/// Calls `calc_natural_convection` with **negated** cosTilt, matching E+:
/// ```cpp
/// CalcASHRAETARPNatural(SurfaceTemperature, ZoneMeanAirTemperature, -surface.CosTilt);
/// ```
///
/// `tilt_deg` is from the **outside face** perspective (0°=roof, 90°=wall, 180°=floor).
pub fn interior_convection(t_surface: f64, t_zone: f64, tilt_deg: f64) -> f64 {
    let cos_tilt = tilt_deg.to_radians().cos();
    // Interior: negate cosTilt (E+ ConvectionCoefficients.cc:1959)
    calc_natural_convection(t_surface - t_zone, -cos_tilt)
}

/// Exterior natural convection coefficient [W/(m²·K)].
///
/// Same TARP natural convection as interior, but WITHOUT negating cosTilt.
/// Used for NoWind surfaces (EnergyPlus `NoWind` surface property).
///
/// For exterior surfaces, E+ uses cosTilt directly (not negated).
/// This gives correct stability for exterior faces:
///   - Warm roof exterior (face up) → unstable (enhanced)
///   - Cool roof exterior (face up, night) → stable (reduced)
///   - Warm floor exterior (face down) → stable
///
/// The previous code incorrectly used `interior_convection()` for NoWind
/// surfaces, which negates cosTilt. For a floor exterior (cos_tilt = -1),
/// this gave wrong stability: warm-in-winter floor appeared unstable
/// instead of stable, over-predicting convection by ~2x.
pub fn exterior_natural_convection(t_surface: f64, t_outdoor: f64, tilt_deg: f64) -> f64 {
    let cos_tilt = tilt_deg.to_radians().cos();
    // Exterior: do NOT negate cosTilt (E+ uses exterior cosTilt as-is)
    calc_natural_convection(t_surface - t_outdoor, cos_tilt)
}

/// Compute wind speed at a given height above ground using the power-law
/// wind profile, matching E+ `SetSurfaceWindSpeedAt()` (DataSurfaces.cc:635-660).
///
/// Formula: V(z) = V_met * (δ_met / z_met)^α_met * (z / δ_site)^α_site
///
/// For default suburban terrain and standard 10m met station:
///   WeatherFileWindModCoeff = (370/10)^0.22 = 1.5863
///   V(z) = V_met * 1.5863 * (z / 370)^0.22
///
/// Parameters from E+ DataEnvironment.hh:
///   - `weather_wind_mod_coeff`: = (δ_met/z_met)^α_met, default 1.5863
///   - `site_wind_exp`: power law exponent for site terrain, default 0.22 (suburban)
///   - `site_wind_bl_height`: boundary layer height [m], default 370.0 (suburban)
///
/// Reference: 2005 ASHRAE Fundamentals, Chapter 16, Equation 4.
pub fn wind_speed_at_height(
    wind_speed_met: f64,
    height_m: f64,
    weather_wind_mod_coeff: f64,
    site_wind_exp: f64,
    site_wind_bl_height: f64,
) -> f64 {
    let z = height_m.max(0.1); // Avoid zero/negative height
    wind_speed_met * weather_wind_mod_coeff * (z / site_wind_bl_height).powf(site_wind_exp)
}

/// Weather file wind modification coefficient.
///
/// Converts from met station (assumed open/country terrain at 10m) to
/// free-stream wind. Constant regardless of site terrain.
///   WMC = (δ_met / z_met)^α_met = (270/10)^0.14 = 1.5863
///
/// Reference: E+ DataEnvironment.hh, HeatBalanceManager.cc.
pub const DEFAULT_WEATHER_WIND_MOD_COEFF: f64 = 1.5863;

/// Site terrain classification for wind profile calculations.
///
/// Maps to wind profile exponent (α) and boundary layer height (δ) used
/// in the power-law wind profile: V(z) = V_met × WMC × (z/δ)^α.
///
/// Values match EnergyPlus HeatBalanceManager.cc (lines 569-598).
/// Reference: 2005 ASHRAE Fundamentals, Chapter 16.
#[derive(Debug, Clone, Copy, PartialEq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Terrain {
    /// Open terrain, flat unobstructed areas, weather stations.
    /// ASHRAE 140 specifies this terrain type.
    /// α = 0.14, δ = 270 m
    Country,
    /// Residential areas, light suburban development.
    /// α = 0.22, δ = 370 m
    #[default]
    Suburbs,
    /// Urban areas with tall buildings.
    /// α = 0.33, δ = 460 m
    City,
    /// Unobstructed ocean or large lake exposure.
    /// α = 0.10, δ = 210 m
    Ocean,
}

impl Terrain {
    /// Wind profile exponent for this terrain type.
    pub fn wind_exp(self) -> f64 {
        match self {
            Terrain::Country => 0.14,
            Terrain::Suburbs => 0.22,
            Terrain::City => 0.33,
            Terrain::Ocean => 0.10,
        }
    }

    /// Atmospheric boundary layer height [m] for this terrain type.
    pub fn wind_bl_height(self) -> f64 {
        match self {
            Terrain::Country => 270.0,
            Terrain::Suburbs => 370.0,
            Terrain::City => 460.0,
            Terrain::Ocean => 210.0,
        }
    }
}

/// Default terrain for backward compatibility.
pub const DEFAULT_SITE_WIND_EXP: f64 = 0.22;
pub const DEFAULT_SITE_WIND_BL_HEIGHT: f64 = 370.0;

/// Determine if a surface is windward relative to the wind direction.
///
/// Matches E+ ConvectionCoefficients.cc logic:
///   - Horizontal surfaces (|cosTilt| >= 0.98): always windward
///   - Other surfaces: windward if |wind_dir - azimuth| <= 100°
///
/// `wind_dir_deg`: meteorological wind direction [degrees from north, 0-360]
/// `azimuth_deg`: surface outward-normal azimuth [degrees from north, 0-360]
/// `cos_tilt`: cosine of surface tilt angle
fn is_windward(wind_dir_deg: f64, azimuth_deg: f64, cos_tilt: f64) -> bool {
    // Horizontal surfaces are always windward
    if cos_tilt.abs() >= 0.98 {
        return true;
    }
    // For non-horizontal: check angle between wind direction and surface azimuth
    let mut angle_diff = (wind_dir_deg - azimuth_deg).abs();
    if angle_diff > 180.0 {
        angle_diff = 360.0 - angle_diff;
    }
    angle_diff <= 100.0
}

/// Exterior convection coefficient [W/(m²·K)].
///
/// DOE-2 combined model (E+ ConvectionCoefficients.cc:624–635):
///   1. Natural: `Hn = CalcASHRAETARPNatural(TSurf, TAir, surface.CosTilt)` (NOT negated)
///   2. Forced: windward or leeward based on wind direction vs surface azimuth
///      - Windward: `HfSmooth = 3.26 * V^0.89` (MoWiTT windward)
///      - Leeward:  `HfSmooth = 3.55 * V^0.617` (MoWiTT leeward)
///      - `HcSmooth = sqrt(Hn² + HfSmooth²)`
///      - `Hf = Rf * (HcSmooth - Hn)`  (roughness multiplier)
///   3. Total: `HExt = Hn + Hf`
///
/// Wind speed should already be adjusted to surface height via `wind_speed_at_height()`.
///
/// Reference: MoWiTT (Yazdanian & Klems 1994), DOE-2 (LBL 1994).
pub fn exterior_convection(
    t_surface: f64,
    t_outdoor: f64,
    wind_speed: f64,
    tilt_deg: f64,
    roughness: Roughness,
) -> f64 {
    exterior_convection_full(t_surface, t_outdoor, wind_speed, tilt_deg, roughness, 0.0, 0.0)
}

/// Full exterior convection with windward/leeward distinction.
///
/// Same as `exterior_convection` but uses wind direction and surface azimuth
/// to select windward vs leeward MoWiTT coefficients.
pub fn exterior_convection_full(
    t_surface: f64,
    t_outdoor: f64,
    wind_speed: f64,
    tilt_deg: f64,
    roughness: Roughness,
    wind_dir_deg: f64,
    azimuth_deg: f64,
) -> f64 {
    let cos_tilt = tilt_deg.to_radians().cos();
    let delta_t = t_surface - t_outdoor;

    // Exterior: use cosTilt directly (NOT negated), matching E+ line 633
    let h_natural = calc_natural_convection(delta_t, cos_tilt);

    // MoWiTT forced convection for smooth glass
    // Windward: a=3.26, b=0.89; Leeward: a=3.55, b=0.617
    let v = wind_speed.max(0.0);
    let windward = is_windward(wind_dir_deg, azimuth_deg, cos_tilt);
    let hf_glass = if v > 0.01 {
        if windward {
            3.26 * v.powf(0.89)
        } else {
            3.55 * v.powf(0.617)
        }
    } else {
        0.0
    };

    // Combined smooth-surface coefficient
    let hc_glass = (h_natural * h_natural + hf_glass * hf_glass).sqrt();

    // DOE-2 roughness correction: Hf = Rf * (HcSmooth - Hn)
    let rf = roughness_multiplier(roughness);
    let hc = h_natural + rf * (hc_glass - h_natural);

    hc.max(0.1)
}

/// Roughness multiplier for forced convection.
/// Based on EnergyPlus ConvectionCoefficients.cc (DOE-2 method).
fn roughness_multiplier(roughness: Roughness) -> f64 {
    match roughness {
        Roughness::VeryRough    => 2.17,
        Roughness::Rough        => 1.67,
        Roughness::MediumRough  => 1.52,
        Roughness::MediumSmooth => 1.13,
        Roughness::Smooth       => 1.11,
        Roughness::VerySmooth   => 1.00,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_relative_eq;

    // ══════════════════════════════════════════════════════════════════════
    // Interior convection unit tests
    //
    // Each test traces through E+'s CalcASHRAETARPNatural algorithm by hand
    // to compute the expected value.
    //
    // E+ algorithm (ConvectionCoefficients.cc:1898–1944):
    //   DeltaTemp = Tsurf - Tamb
    //   if DeltaTemp == 0 or cosTilt == 0:  → 1.31 * |ΔT|^(1/3)          (vertical)
    //   if same_sign(DeltaTemp, cosTilt):   → 9.482 * |ΔT|^(1/3) / (7.238 - |cos|) (unstable)
    //   else:                               → 1.810 * |ΔT|^(1/3) / (1.382 + |cos|) (stable)
    //
    // For interior surfaces, E+ passes -surface.CosTilt (negated).
    // Our function takes outside-face tilt_deg and handles the sign internally
    // via: is_stable = (delta_t * cos_tilt) >= 0
    //
    // This is equivalent because:
    //   E+ enhanced (unstable) = same_sign(DeltaT, -cosTilt_outside)
    //                          = opposite_sign(DeltaT, cosTilt_outside)
    //                          = product < 0 → is_stable = false ✓
    // ══════════════════════════════════════════════════════════════════════

    #[test]
    fn test_interior_vertical_wall_dt5() {
        // Vertical wall (tilt=90°), T_surf=25, T_zone=20, ΔT=5
        // E+: cosTilt=0 → vertical branch → h = 1.31 * 5^(1/3)
        let h = interior_convection(25.0, 20.0, 90.0);
        let expected = 1.31 * 5.0_f64.powf(1.0 / 3.0); // 2.239
        assert_relative_eq!(h, expected, max_relative = 0.01);
    }

    #[test]
    fn test_interior_vertical_wall_dt10() {
        // Vertical wall, T_surf=30, T_zone=20, ΔT=10
        // h = 1.31 * 10^(1/3) = 1.31 * 2.154 = 2.822
        let h = interior_convection(30.0, 20.0, 90.0);
        let expected = 1.31 * 10.0_f64.powf(1.0 / 3.0);
        assert_relative_eq!(h, expected, max_relative = 0.01);
    }

    #[test]
    fn test_interior_vertical_wall_dt0_clamps_to_minimum() {
        // Vertical wall, T_surf = T_zone = 20, ΔT=0
        // E+: DeltaTemp==0 → vertical → h = 1.31 * 0^(1/3) = 0.0, clamped to 0.1
        // Our code: dt = max(|0|, 0.001) → h = 1.31 * 0.001^(1/3) = 0.131
        // Both result in a very small value ≥ 0.1
        let h = interior_convection(20.0, 20.0, 90.0);
        assert!(h >= 0.1, "h={} should be >= 0.1 (minimum)", h);
        assert!(h < 0.2, "h={} should be small for zero ΔT", h);
    }

    #[test]
    fn test_interior_warm_floor_unstable() {
        // Floor: tilt=180° (outside face down), cos_tilt = -1.0
        // Interior face is UP. Surface warmer than air → buoyant plumes → UNSTABLE
        //
        // E+ trace: passes cosTilt_interior = -(-1) = +1
        //   DeltaTemp = 25-20 = +5, cosTilt_passed = +1 → same sign → unstable
        //   h = 9.482 * 5^(1/3) / (7.238 - 1.0)
        //     = 9.482 * 1.7100 / 6.238
        //     = 16.214 / 6.238 = 2.599
        //
        // Our code: delta_t=+5, cos_tilt=-1 → product=-5 < 0 → is_stable=false → unstable ✓
        let h = interior_convection(25.0, 20.0, 180.0);
        let expected = 9.482 * 5.0_f64.powf(1.0 / 3.0) / (7.238 - 1.0);
        assert_relative_eq!(h, expected, max_relative = 0.01);
    }

    #[test]
    fn test_interior_cool_floor_stable() {
        // Floor: tilt=180°, cos_tilt = -1.0
        // Interior face UP. Surface cooler than air → stratified → STABLE
        //
        // E+ trace: passes cosTilt_interior = +1
        //   DeltaTemp = 18-22 = -4, cosTilt_passed = +1 → different signs → stable
        //   h = 1.810 * 4^(1/3) / (1.382 + 1.0)
        //     = 1.810 * 1.5874 / 2.382
        //     = 2.873 / 2.382 = 1.206
        //
        // Our code: delta_t=-4, cos_tilt=-1 → product=+4 > 0 → is_stable=true → stable ✓
        let h = interior_convection(18.0, 22.0, 180.0);
        let expected = 1.810 * 4.0_f64.powf(1.0 / 3.0) / (1.382 + 1.0);
        assert_relative_eq!(h, expected, max_relative = 0.01);
    }

    #[test]
    fn test_interior_warm_ceiling_stable() {
        // Roof/ceiling: tilt=0° (outside face up), cos_tilt = +1.0
        // Interior face is DOWN. Surface warmer than air → warm air trapped → STABLE
        //
        // E+ trace: passes cosTilt_interior = -(+1) = -1
        //   DeltaTemp = 30-22 = +8, cosTilt_passed = -1 → different signs → stable
        //   h = 1.810 * 8^(1/3) / (1.382 + 1.0)
        //     = 1.810 * 2.0000 / 2.382
        //     = 3.620 / 2.382 = 1.520
        //
        // Our code: delta_t=+8, cos_tilt=+1 → product=+8 > 0 → is_stable=true → stable ✓
        let h = interior_convection(30.0, 22.0, 0.0);
        let expected = 1.810 * 8.0_f64.powf(1.0 / 3.0) / (1.382 + 1.0);
        assert_relative_eq!(h, expected, max_relative = 0.01);
    }

    #[test]
    fn test_interior_cool_ceiling_unstable() {
        // Roof/ceiling: tilt=0°, cos_tilt = +1.0
        // Interior face DOWN. Surface cooler than air → cold air sinks → UNSTABLE
        //
        // E+ trace: passes cosTilt_interior = -1
        //   DeltaTemp = 18-24 = -6, cosTilt_passed = -1 → same sign → unstable
        //   h = 9.482 * 6^(1/3) / (7.238 - 1.0)
        //     = 9.482 * 1.8171 / 6.238
        //     = 17.229 / 6.238 = 2.762
        //
        // Our code: delta_t=-6, cos_tilt=+1 → product=-6 < 0 → is_stable=false → unstable ✓
        let h = interior_convection(18.0, 24.0, 0.0);
        let expected = 9.482 * 6.0_f64.powf(1.0 / 3.0) / (7.238 - 1.0);
        assert_relative_eq!(h, expected, max_relative = 0.01);
    }

    #[test]
    fn test_interior_tilted_45deg_stable() {
        // Tilted surface: tilt=45° (like a sloped roof), cos_tilt = 0.707
        // Interior face faces down-and-inward. Surface warmer than air.
        //
        // E+ trace: passes cosTilt_interior = -0.707
        //   DeltaTemp = 28-22 = +6, cosTilt_passed = -0.707 → different signs → stable
        //   h = 1.810 * 6^(1/3) / (1.382 + 0.707)
        //     = 1.810 * 1.8171 / 2.089
        //     = 3.289 / 2.089 = 1.574
        //
        // Our code: delta_t=+6, cos_tilt=+0.707 → product > 0 → is_stable=true → stable ✓
        let h = interior_convection(28.0, 22.0, 45.0);
        let cos45 = 45.0_f64.to_radians().cos();
        let expected = 1.810 * 6.0_f64.powf(1.0 / 3.0) / (1.382 + cos45.abs());
        assert_relative_eq!(h, expected, max_relative = 0.01);
    }

    #[test]
    fn test_interior_tilted_45deg_unstable() {
        // Same tilted surface, but now cool surface with warm air above.
        //
        // E+ trace: passes cosTilt_interior = -0.707
        //   DeltaTemp = 18-24 = -6, cosTilt_passed = -0.707 → same sign → unstable
        //   h = 9.482 * 6^(1/3) / (7.238 - 0.707)
        //     = 9.482 * 1.8171 / 6.531
        //     = 17.229 / 6.531 = 2.638
        //
        // Our code: delta_t=-6, cos_tilt=+0.707 → product < 0 → is_stable=false → unstable ✓
        let h = interior_convection(18.0, 24.0, 45.0);
        let cos45 = 45.0_f64.to_radians().cos();
        let expected = 9.482 * 6.0_f64.powf(1.0 / 3.0) / (7.238 - cos45.abs());
        assert_relative_eq!(h, expected, max_relative = 0.01);
    }

    #[test]
    fn test_interior_minimum_clamp() {
        // Very small ΔT on a floor → should still be ≥ 0.1
        let h = interior_convection(20.001, 20.0, 180.0);
        assert!(h >= 0.1, "h={} must be >= 0.1 minimum", h);
    }

    #[test]
    fn test_interior_symmetry_sign_of_dt() {
        // For vertical walls, swapping which side is warmer should give same h
        // (vertical formula only depends on |ΔT|)
        let h1 = interior_convection(25.0, 20.0, 90.0);
        let h2 = interior_convection(20.0, 25.0, 90.0);
        assert_relative_eq!(h1, h2, max_relative = 0.001);
    }

    // ══════════════════════════════════════════════════════════════════════
    // calc_natural_convection unit tests (the core E+-matching function)
    // ══════════════════════════════════════════════════════════════════════

    #[test]
    fn test_natural_vertical() {
        // Vertical: cosTilt=0 → h = 1.31 * |ΔT|^(1/3)
        let h = calc_natural_convection(5.0, 0.0);
        let expected = 1.31 * 5.0_f64.powf(1.0 / 3.0);
        assert_relative_eq!(h, expected, max_relative = 0.01);
    }

    #[test]
    fn test_natural_unstable_positive() {
        // DeltaT=+5, cosTilt=+1 → same sign → unstable
        let h = calc_natural_convection(5.0, 1.0);
        let expected = 9.482 * 5.0_f64.powf(1.0 / 3.0) / (7.238 - 1.0);
        assert_relative_eq!(h, expected, max_relative = 0.01);
    }

    #[test]
    fn test_natural_stable_opposite_signs() {
        // DeltaT=-5, cosTilt=+1 → opposite signs → stable
        let h = calc_natural_convection(-5.0, 1.0);
        let expected = 1.810 * 5.0_f64.powf(1.0 / 3.0) / (1.382 + 1.0);
        assert_relative_eq!(h, expected, max_relative = 0.01);
    }

    #[test]
    fn test_natural_unstable_both_negative() {
        // DeltaT=-6, cosTilt=-1 → same sign → unstable
        let h = calc_natural_convection(-6.0, -1.0);
        let expected = 9.482 * 6.0_f64.powf(1.0 / 3.0) / (7.238 - 1.0);
        assert_relative_eq!(h, expected, max_relative = 0.01);
    }

    // ══════════════════════════════════════════════════════════════════════
    // Exterior convection unit tests
    //
    // E+ DOE-2 exterior algorithm (ConvectionCoefficients.cc:624-635):
    //   Hf = CalcDOE2Windward(TSurf, TAir, surface.CosTilt, WindSpeed, Roughness)
    //      = Rf * (sqrt(Hn² + HfSmooth²) - Hn)
    //   Hn = CalcASHRAETARPNatural(TSurf, TAir, surface.CosTilt)  // NOT negated
    //   HExt = Hn + Hf
    //
    // For exterior, cosTilt is NOT negated. So stability is:
    //   same_sign(DeltaT, cosTilt) → unstable (enhanced)
    //   opposite_sign → stable (reduced)
    // ══════════════════════════════════════════════════════════════════════

    #[test]
    fn test_exterior_zero_wind_vertical_wall() {
        // Vertical wall, no wind: exterior h = natural h (vertical formula)
        // tilt=90°, cos≈0 → vertical branch for both interior and exterior
        let h_ext = exterior_convection(25.0, 20.0, 0.0, 90.0, Roughness::MediumRough);
        let h_int = interior_convection(25.0, 20.0, 90.0);
        // Both use vertical formula → same result
        assert_relative_eq!(h_ext, h_int, max_relative = 0.01);
    }

    #[test]
    fn test_exterior_warm_roof_no_wind_unstable() {
        // Roof exterior: tilt=0°, cos=+1. Surface warmer than air.
        // Exterior face is UP, warm → buoyant plumes → UNSTABLE.
        //
        // E+ trace: cosTilt=+1 (not negated for exterior)
        //   DeltaTemp = 40-20 = +20, cosTilt = +1 → same sign → unstable
        //   h = 9.482 * 20^(1/3) / (7.238 - 1.0)
        //     = 9.482 * 2.7144 / 6.238
        //     = 25.738 / 6.238 = 4.126
        //
        // This was previously broken: interior_convection gave stable (2.06) instead!
        let h = exterior_convection(40.0, 20.0, 0.0, 0.0, Roughness::MediumRough);
        let expected = 9.482 * 20.0_f64.powf(1.0 / 3.0) / (7.238 - 1.0);
        assert_relative_eq!(h, expected, max_relative = 0.01);
    }

    #[test]
    fn test_exterior_cool_roof_no_wind_stable() {
        // Roof exterior: tilt=0°, cos=+1. Surface cooler than air (night radiative cooling).
        // Exterior face UP, cool → stratified → STABLE.
        //
        // E+ trace: DeltaTemp = 5-10 = -5, cosTilt = +1 → opposite signs → stable
        //   h = 1.810 * 5^(1/3) / (1.382 + 1.0)
        //     = 1.810 * 1.7100 / 2.382
        //     = 3.095 / 2.382 = 1.299
        let h = exterior_convection(5.0, 10.0, 0.0, 0.0, Roughness::MediumRough);
        let expected = 1.810 * 5.0_f64.powf(1.0 / 3.0) / (1.382 + 1.0);
        assert_relative_eq!(h, expected, max_relative = 0.01);
    }

    #[test]
    fn test_exterior_warm_roof_with_wind() {
        // Same warm roof but with wind=3 m/s, MediumRough.
        // DOE-2 formula: HExt = Hn + Rf * (sqrt(Hn² + HfSmooth²) - Hn)
        //
        // Hn = 9.482 * 20^(1/3) / (7.238 - 1.0) = 4.126  (unstable, exterior)
        // HfSmooth = 3.26 * 3.0^0.89 = 3.26 * 2.7316 = 8.905
        // HcSmooth = sqrt(4.126² + 8.905²) = sqrt(17.024 + 79.299) = sqrt(96.323) = 9.815
        // Hf = 1.52 * (9.815 - 4.126) = 1.52 * 5.689 = 8.647
        // HExt = 4.126 + 8.647 = 12.773
        let h = exterior_convection(40.0, 20.0, 3.0, 0.0, Roughness::MediumRough);

        let hn = 9.482 * 20.0_f64.powf(1.0 / 3.0) / (7.238 - 1.0);
        let hf_smooth = 3.26 * 3.0_f64.powf(0.89);
        let hc_smooth = (hn * hn + hf_smooth * hf_smooth).sqrt();
        let rf = 1.52; // MediumRough
        let expected = hn + rf * (hc_smooth - hn);

        assert_relative_eq!(h, expected, max_relative = 0.01);
    }

    #[test]
    fn test_exterior_wall_with_wind() {
        // Vertical wall: tilt=90°, T_surf=25, T_air=20, wind=5 m/s, MediumRough
        // Vertical → Hn = 1.31 * 5^(1/3) = 2.239
        // HfSmooth = 3.26 * 5^0.89 = 3.26 * 4.3174 = 14.075
        // HcSmooth = sqrt(2.239² + 14.075²) = sqrt(5.013 + 198.106) = sqrt(203.119) = 14.252
        // Hf = 1.52 * (14.252 - 2.239) = 1.52 * 12.013 = 18.260
        // HExt = 2.239 + 18.260 = 20.499
        let h = exterior_convection(25.0, 20.0, 5.0, 90.0, Roughness::MediumRough);

        let hn = 1.31 * 5.0_f64.powf(1.0 / 3.0);
        let hf_smooth = 3.26 * 5.0_f64.powf(0.89);
        let hc_smooth = (hn * hn + hf_smooth * hf_smooth).sqrt();
        let rf = 1.52;
        let expected = hn + rf * (hc_smooth - hn);

        assert_relative_eq!(h, expected, max_relative = 0.01);
    }

    #[test]
    fn test_exterior_increases_with_wind() {
        let h_calm = exterior_convection(25.0, 20.0, 0.5, 90.0, Roughness::MediumRough);
        let h_windy = exterior_convection(25.0, 20.0, 10.0, 90.0, Roughness::MediumRough);
        assert!(h_windy > h_calm);
    }

    #[test]
    fn test_exterior_roughness_effect() {
        // Higher roughness → higher h_ext (same conditions)
        let h_smooth = exterior_convection(25.0, 20.0, 5.0, 90.0, Roughness::VerySmooth);
        let h_rough = exterior_convection(25.0, 20.0, 5.0, 90.0, Roughness::VeryRough);
        assert!(h_rough > h_smooth, "VeryRough h={} should exceed VerySmooth h={}", h_rough, h_smooth);

        // VerySmooth Rf=1.0, so forced increment is unchanged:
        // hc = hn + 1.0 * (hc_glass - hn) = hc_glass
        let hn = 1.31 * 5.0_f64.powf(1.0 / 3.0);
        let hf_smooth = 3.26 * 5.0_f64.powf(0.89);
        let hc_glass = (hn * hn + hf_smooth * hf_smooth).sqrt();
        assert_relative_eq!(h_smooth, hc_glass, max_relative = 0.01);
    }

    #[test]
    fn test_exterior_interior_differ_for_roof() {
        // For horizontal surfaces, interior and exterior stability differ.
        // Warm surface, cool air. Roof tilt=0°.
        //
        // Exterior (face UP, warm → unstable): h = 9.482 * |ΔT|^(1/3) / (7.238 - 1.0)
        // Interior (face DOWN, warm → stable):  h = 1.810 * |ΔT|^(1/3) / (1.382 + 1.0)
        //
        // These should differ by about 2x.
        let h_ext = exterior_convection(30.0, 20.0, 0.0, 0.0, Roughness::MediumRough);
        let h_int = interior_convection(30.0, 20.0, 0.0);

        // Exterior unstable: 9.482 / 6.238 = 1.520 coefficient
        // Interior stable:   1.810 / 2.382 = 0.760 coefficient
        // Ratio should be about 2.0
        let ratio = h_ext / h_int;
        assert!(ratio > 1.8 && ratio < 2.2,
            "Exterior/interior ratio={:.2} should be ~2.0 for warm horizontal roof", ratio);
    }
}
