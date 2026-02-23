//! Psychrometric property calculations for moist air.
//!
//! All equations match the EnergyPlus implementation (Hyland & Wexler formulation
//! for saturation pressure, ASHRAE relationships for derived properties).
//!
//! Reference: EnergyPlus Engineering Reference, Chapter "Psychrometric Services"
//! Source: github.com/NatLabRockies/EnergyPlus/src/EnergyPlus/Psychrometrics.cc

/// Kelvin offset from Celsius
const KELVIN: f64 = 273.15;

/// Triple point of water in Kelvin
const TRIPLE_POINT_K: f64 = 273.16;

/// Ratio of molecular mass of water to dry air (18.01534 / 28.9645)
const MOL_MASS_RATIO: f64 = 0.621945;

/// Dry air specific heat at constant pressure [J/(kg·K)]
const CP_AIR: f64 = 1.00484e3;

/// Water vapor specific heat at constant pressure [J/(kg·K)]
const CP_VAPOR: f64 = 1.85895e3;

/// Heat of vaporization of water at 0°C [J/kg]
const HFG_WATER: f64 = 2.50094e6;

/// Minimum valid humidity ratio
const W_MIN: f64 = 1.0e-5;

/// Standard atmospheric pressure [Pa]
pub const STD_PRESSURE: f64 = 101325.0;

// ─── Saturation Pressure ─────────────────────────────────────────────────────

/// Saturation pressure of water vapor as a function of temperature.
///
/// Uses Hyland & Wexler (1983) formulation, matching EnergyPlus `PsyPsatFnTemp`.
/// Valid for -100°C to 200°C.
///
/// # Arguments
/// * `t_db` - Dry-bulb temperature [°C]
///
/// # Returns
/// Saturation pressure [Pa]
pub fn psat_fn_temp(t_db: f64) -> f64 {
    let t_kel = t_db + KELVIN;

    if t_kel < 173.15 {
        // Below -100°C: clamp
        return 0.001405102123874164;
    }

    if t_kel > 473.15 {
        // Above 200°C: clamp
        return 1_555_073.745636215;
    }

    if t_kel < TRIPLE_POINT_K {
        // Ice region (Hyland & Wexler over ice)
        const C1: f64 = -5674.5359;
        const C2: f64 = 6.3925247;
        const C3: f64 = -0.9677843e-2;
        const C4: f64 = 0.62215701e-6;
        const C5: f64 = 0.20747825e-8;
        const C6: f64 = -0.9484024e-12;
        const C7: f64 = 4.1635019;

        (C1 / t_kel
            + C2
            + t_kel * (C3 + t_kel * (C4 + t_kel * (C5 + C6 * t_kel)))
            + C7 * t_kel.ln())
        .exp()
    } else {
        // Liquid water region (Hyland & Wexler)
        const C8: f64 = -5800.2206;
        const C9: f64 = 1.3914993;
        const C10: f64 = -0.048640239;
        const C11: f64 = 0.41764768e-4;
        const C12: f64 = -0.14452093e-7;
        const C13: f64 = 6.5459673;

        (C8 / t_kel
            + C9
            + t_kel * (C10 + t_kel * (C11 + t_kel * C12))
            + C13 * t_kel.ln())
        .exp()
    }
}

// ─── Humidity Ratio ──────────────────────────────────────────────────────────

/// Humidity ratio from dry-bulb temperature, relative humidity, and barometric pressure.
///
/// Matches EnergyPlus `PsyWFnTdbRhPb`.
///
/// # Arguments
/// * `t_db` - Dry-bulb temperature [°C]
/// * `rh` - Relative humidity [0.0 to 1.0]
/// * `p_b` - Barometric pressure [Pa]
pub fn w_fn_tdb_rh_pb(t_db: f64, rh: f64, p_b: f64) -> f64 {
    let p_sat = psat_fn_temp(t_db);
    let p_dew = rh * p_sat;
    let w = MOL_MASS_RATIO * p_dew / (p_b - p_dew).max(1.0);
    w.max(W_MIN)
}

/// Humidity ratio from dew-point temperature and barometric pressure.
///
/// Matches EnergyPlus `PsyWFnTdpPb`.
pub fn w_fn_tdp_pb(t_dp: f64, p_b: f64) -> f64 {
    let p_sat = psat_fn_temp(t_dp);
    let w = MOL_MASS_RATIO * p_sat / (p_b - p_sat).max(1.0);
    w.max(W_MIN)
}

/// Humidity ratio from dry-bulb temperature, wet-bulb temperature, and barometric pressure.
///
/// Matches EnergyPlus `PsyWFnTdbTwbPb`.
pub fn w_fn_tdb_twb_pb(t_db: f64, t_wb: f64, p_b: f64) -> f64 {
    let w_star = w_fn_tdb_rh_pb(t_wb, 1.0, p_b);

    let w = if t_wb >= 0.0 {
        ((2501.0 - 2.326 * t_wb) * w_star - 1.006 * (t_db - t_wb))
            / (2501.0 + 1.86 * t_db - 4.186 * t_wb)
    } else {
        ((2830.0 - 0.24 * t_wb) * w_star - 1.006 * (t_db - t_wb))
            / (2830.0 + 1.86 * t_db - 2.1 * t_wb)
    };
    w.max(W_MIN)
}

/// Humidity ratio from dry-bulb temperature and enthalpy.
///
/// Matches EnergyPlus `PsyWFnTdbH`.
pub fn w_fn_tdb_h(t_db: f64, h: f64) -> f64 {
    let w = (h - CP_AIR * t_db) / (HFG_WATER + CP_VAPOR * t_db);
    w.max(W_MIN)
}

// ─── Enthalpy ────────────────────────────────────────────────────────────────

/// Moist air enthalpy from dry-bulb temperature and humidity ratio.
///
/// h = Cp_air * T + W * (Hfg + Cp_vapor * T)
///
/// Matches EnergyPlus `PsyHFnTdbW`.
///
/// # Returns
/// Enthalpy [J/kg]
pub fn h_fn_tdb_w(t_db: f64, w: f64) -> f64 {
    CP_AIR * t_db + w.max(W_MIN) * (HFG_WATER + CP_VAPOR * t_db)
}

/// Moist air enthalpy from dry-bulb, relative humidity, and barometric pressure.
///
/// Matches EnergyPlus `PsyHFnTdbRhPb`.
pub fn h_fn_tdb_rh_pb(t_db: f64, rh: f64, p_b: f64) -> f64 {
    let w = w_fn_tdb_rh_pb(t_db, rh, p_b);
    h_fn_tdb_w(t_db, w)
}

// ─── Temperature ─────────────────────────────────────────────────────────────

/// Dry-bulb temperature from enthalpy and humidity ratio.
///
/// Matches EnergyPlus `PsyTdbFnHW`.
pub fn tdb_fn_h_w(h: f64, w: f64) -> f64 {
    let w = w.max(W_MIN);
    (h - HFG_WATER * w) / (CP_AIR + CP_VAPOR * w)
}

/// Dew-point temperature from humidity ratio and barometric pressure.
///
/// Matches EnergyPlus `PsyTdpFnWPb`.
/// Iteratively inverts the saturation pressure relationship.
pub fn tdp_fn_w_pb(w: f64, p_b: f64) -> f64 {
    let w = w.max(W_MIN);
    let p_vap = p_b * w / (MOL_MASS_RATIO + w);
    tsat_fn_press(p_vap)
}

/// Dew-point temperature from dry-bulb, wet-bulb, and barometric pressure.
///
/// Matches EnergyPlus `PsyTdpFnTdbTwbPb`.
pub fn tdp_fn_tdb_twb_pb(t_db: f64, t_wb: f64, p_b: f64) -> f64 {
    let w = w_fn_tdb_twb_pb(t_db, t_wb, p_b);
    tdp_fn_w_pb(w, p_b)
}

/// Saturation temperature from pressure.
///
/// Uses Newton-Raphson iteration matching EnergyPlus `PsyTsatFnPb`.
pub fn tsat_fn_press(press: f64) -> f64 {
    if press >= 1_555_000.0 {
        return 200.0;
    }
    if press <= 0.0017 {
        return -100.0;
    }

    // Initial guess from approximate inversion
    let mut t_sat = if press > 611.0 && press < 611.25 {
        0.0
    } else if press <= 611.0 {
        // Ice region: rough initial guess
        -30.0
    } else {
        // Above triple point: start with boiling approximation
        100.0 * (press / 101325.0).ln() / (100.0_f64.ln())
    };

    // Clamp initial guess
    t_sat = t_sat.clamp(-100.0, 200.0);

    // Newton-Raphson iteration
    for _ in 0..50 {
        let p_sat = psat_fn_temp(t_sat);
        let error = press - p_sat;
        if error.abs() < 0.1 {
            break;
        }
        // Numerical derivative
        let dp = psat_fn_temp(t_sat + 0.001) - p_sat;
        if dp.abs() < 1.0e-20 {
            break;
        }
        let dt = error / (dp / 0.001);
        t_sat += dt.clamp(-10.0, 10.0);
        t_sat = t_sat.clamp(-100.0, 200.0);
    }
    t_sat
}

/// Wet-bulb temperature from dry-bulb, humidity ratio, and barometric pressure.
///
/// Iterative solver matching EnergyPlus `PsyTwbFnTdbWPb`.
pub fn twb_fn_tdb_w_pb(t_db: f64, w: f64, p_b: f64) -> f64 {
    let w = w.max(W_MIN);

    // Check if air is already saturated
    let w_sat = w_fn_tdb_rh_pb(t_db, 1.0, p_b);
    if w >= w_sat {
        return t_db;
    }

    // Initial guess: start at dew point and iterate upward
    let t_dp = tdp_fn_w_pb(w, p_b);
    let mut t_wb = (t_db + t_dp) / 2.0;

    for _ in 0..100 {
        let w_star = w_fn_tdb_rh_pb(t_wb, 1.0, p_b);

        let w_calc = if t_wb >= 0.0 {
            ((2501.0 - 2.326 * t_wb) * w_star - 1.006 * (t_db - t_wb))
                / (2501.0 + 1.86 * t_db - 4.186 * t_wb)
        } else {
            ((2830.0 - 0.24 * t_wb) * w_star - 1.006 * (t_db - t_wb))
                / (2830.0 + 1.86 * t_db - 2.1 * t_wb)
        };

        let error = w - w_calc;
        if error.abs() < 1.0e-6 {
            break;
        }

        // Numerical derivative for Newton step
        let t_wb2 = t_wb + 0.001;
        let w_star2 = w_fn_tdb_rh_pb(t_wb2, 1.0, p_b);
        let w_calc2 = if t_wb2 >= 0.0 {
            ((2501.0 - 2.326 * t_wb2) * w_star2 - 1.006 * (t_db - t_wb2))
                / (2501.0 + 1.86 * t_db - 4.186 * t_wb2)
        } else {
            ((2830.0 - 0.24 * t_wb2) * w_star2 - 1.006 * (t_db - t_wb2))
                / (2830.0 + 1.86 * t_db - 2.1 * t_wb2)
        };

        let dw_dt = (w_calc2 - w_calc) / 0.001;
        if dw_dt.abs() < 1.0e-20 {
            break;
        }
        t_wb += error / dw_dt;
        t_wb = t_wb.clamp(t_dp, t_db);
    }

    t_wb.min(t_db)
}

// ─── Density & Specific Volume ───────────────────────────────────────────────

/// Moist air density from barometric pressure, dry-bulb temperature, and humidity ratio.
///
/// Uses ideal gas law matching EnergyPlus `PsyRhoAirFnPbTdbW`.
///
/// # Returns
/// Air density [kg/m³]
pub fn rho_air_fn_pb_tdb_w(p_b: f64, t_db: f64, w: f64) -> f64 {
    let w = w.max(W_MIN);
    // Gas constant for dry air = 287.055 J/(kg·K)
    // Factor (1 + W) / (1 + 1.6078 * W) accounts for moisture
    p_b / (287.055 * (t_db + KELVIN) * (1.0 + 1.6078 * w) / (1.0 + w))
}

/// Specific volume from dry-bulb, humidity ratio, and barometric pressure.
///
/// Matches EnergyPlus `PsyVFnTdbWPb`.
///
/// # Returns
/// Specific volume [m³/kg]
pub fn v_fn_tdb_w_pb(t_db: f64, w: f64, p_b: f64) -> f64 {
    let w = w.max(W_MIN);
    // 1.59473e2 = 287.055 * 5/9 (unit conversion factor)
    1.59473e2 * (1.0 + 1.6078 * w) * (1.8 * t_db + 492.0) / p_b
}

// ─── Specific Heat ───────────────────────────────────────────────────────────

/// Moist air specific heat from humidity ratio.
///
/// Cp = Cp_dry_air + W * Cp_water_vapor
///
/// Matches EnergyPlus `PsyCpAirFnW`.
///
/// # Returns
/// Specific heat [J/(kg·K)]
pub fn cp_air_fn_w(w: f64) -> f64 {
    CP_AIR + w.max(W_MIN) * CP_VAPOR
}

// ─── Relative Humidity ───────────────────────────────────────────────────────

/// Relative humidity from dry-bulb temperature, humidity ratio, and barometric pressure.
///
/// Matches EnergyPlus `PsyRhFnTdbWPb`.
///
/// # Returns
/// Relative humidity [0.0 to 1.0]
pub fn rh_fn_tdb_w_pb(t_db: f64, w: f64, p_b: f64) -> f64 {
    let p_sat = psat_fn_temp(t_db);
    let w = w.max(W_MIN);
    let p_vap = p_b * w / (MOL_MASS_RATIO + w);
    let rh = p_vap / p_sat;
    rh.clamp(0.0, 1.0)
}

// ─── Convenience Structures ──────────────────────────────────────────────────

/// Complete moist air state at a point.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MoistAirState {
    /// Dry-bulb temperature [°C]
    pub t_db: f64,
    /// Humidity ratio [kg_water/kg_dry_air]
    pub w: f64,
    /// Enthalpy [J/kg]
    pub h: f64,
    /// Barometric pressure [Pa]
    pub p_b: f64,
}

impl MoistAirState {
    /// Create a new moist air state from dry-bulb, humidity ratio, and pressure.
    pub fn new(t_db: f64, w: f64, p_b: f64) -> Self {
        let w = w.max(W_MIN);
        Self {
            t_db,
            w,
            h: h_fn_tdb_w(t_db, w),
            p_b,
        }
    }

    /// Create from dry-bulb, relative humidity, and pressure.
    pub fn from_tdb_rh(t_db: f64, rh: f64, p_b: f64) -> Self {
        let w = w_fn_tdb_rh_pb(t_db, rh, p_b);
        Self::new(t_db, w, p_b)
    }

    /// Relative humidity [0.0 to 1.0]
    pub fn rh(&self) -> f64 {
        rh_fn_tdb_w_pb(self.t_db, self.w, self.p_b)
    }

    /// Wet-bulb temperature [°C]
    pub fn t_wb(&self) -> f64 {
        twb_fn_tdb_w_pb(self.t_db, self.w, self.p_b)
    }

    /// Dew-point temperature [°C]
    pub fn t_dp(&self) -> f64 {
        tdp_fn_w_pb(self.w, self.p_b)
    }

    /// Air density [kg/m³]
    pub fn rho(&self) -> f64 {
        rho_air_fn_pb_tdb_w(self.p_b, self.t_db, self.w)
    }

    /// Specific volume [m³/kg]
    pub fn v(&self) -> f64 {
        v_fn_tdb_w_pb(self.t_db, self.w, self.p_b)
    }

    /// Specific heat [J/(kg·K)]
    pub fn cp(&self) -> f64 {
        cp_air_fn_w(self.w)
    }
}

// ─── Water/Fluid Properties ──────────────────────────────────────────────────

/// Specific heat of liquid water [J/(kg·K)]
pub const CP_WATER: f64 = 4180.0;

/// Density of water at ~20°C [kg/m³]
pub const RHO_WATER: f64 = 998.2;

/// Water/fluid state for plant-side calculations.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FluidState {
    /// Temperature [°C]
    pub temp: f64,
    /// Mass flow rate [kg/s]
    pub mass_flow: f64,
    /// Specific heat [J/(kg·K)]
    pub cp: f64,
}

impl FluidState {
    /// Create a new water fluid state.
    pub fn water(temp: f64, mass_flow: f64) -> Self {
        Self {
            temp,
            mass_flow,
            cp: CP_WATER,
        }
    }

    /// Heat capacity rate [W/K]
    pub fn capacity_rate(&self) -> f64 {
        self.mass_flow * self.cp
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_relative_eq;

    #[test]
    fn test_psat_at_100c() {
        // Saturation pressure at 100°C should be ~101325 Pa (1 atm)
        let p = psat_fn_temp(100.0);
        assert_relative_eq!(p, 101325.0, max_relative = 0.002);
    }

    #[test]
    fn test_psat_at_0c() {
        // Saturation pressure at 0°C should be ~611 Pa
        let p = psat_fn_temp(0.0);
        assert_relative_eq!(p, 611.0, max_relative = 0.005);
    }

    #[test]
    fn test_psat_at_20c() {
        // ~2338 Pa at 20°C
        let p = psat_fn_temp(20.0);
        assert_relative_eq!(p, 2338.0, max_relative = 0.01);
    }

    #[test]
    fn test_enthalpy_round_trip() {
        let t_db = 25.0;
        let w = 0.010;
        let h = h_fn_tdb_w(t_db, w);
        let t_back = tdb_fn_h_w(h, w);
        assert_relative_eq!(t_back, t_db, max_relative = 1.0e-10);
    }

    #[test]
    fn test_humidity_ratio_from_rh() {
        // At 20°C, 50% RH, 101325 Pa: W should be approximately 0.00726
        let w = w_fn_tdb_rh_pb(20.0, 0.50, 101325.0);
        assert_relative_eq!(w, 0.00726, max_relative = 0.02);
    }

    #[test]
    fn test_rh_round_trip() {
        let t_db = 30.0;
        let rh_in = 0.60;
        let p_b = 101325.0;
        let w = w_fn_tdb_rh_pb(t_db, rh_in, p_b);
        let rh_out = rh_fn_tdb_w_pb(t_db, w, p_b);
        assert_relative_eq!(rh_out, rh_in, max_relative = 0.001);
    }

    #[test]
    fn test_moist_air_state() {
        let state = MoistAirState::from_tdb_rh(25.0, 0.50, 101325.0);
        assert_relative_eq!(state.t_db, 25.0, max_relative = 1.0e-10);
        assert!(state.w > 0.009 && state.w < 0.011);
        assert_relative_eq!(state.rh(), 0.50, max_relative = 0.001);
    }

    #[test]
    fn test_tsat_round_trip() {
        // Saturation temp at 101325 Pa should be ~100°C
        let t = tsat_fn_press(101325.0);
        assert_relative_eq!(t, 100.0, max_relative = 0.001);
    }

    #[test]
    fn test_wet_bulb_at_saturation() {
        // At saturation (RH=1.0), wet-bulb equals dry-bulb
        let t_db = 25.0;
        let p_b = 101325.0;
        let w_sat = w_fn_tdb_rh_pb(t_db, 1.0, p_b);
        let t_wb = twb_fn_tdb_w_pb(t_db, w_sat, p_b);
        assert_relative_eq!(t_wb, t_db, max_relative = 0.001);
    }

    #[test]
    fn test_density_at_standard_conditions() {
        // Air density at 20°C, 101325 Pa, dry air: ~1.204 kg/m³
        let rho = rho_air_fn_pb_tdb_w(101325.0, 20.0, 0.0);
        assert_relative_eq!(rho, 1.204, max_relative = 0.01);
    }

    #[test]
    fn test_cp_dry_air() {
        let cp = cp_air_fn_w(0.0);
        assert_relative_eq!(cp, CP_AIR, max_relative = 0.001);
    }
}
