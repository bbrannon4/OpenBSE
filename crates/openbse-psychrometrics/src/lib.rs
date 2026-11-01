//! Psychrometric property calculations for moist air and thermal comfort indices.
//!
//! All equations match the EnergyPlus implementation (Hyland & Wexler formulation
//! for saturation pressure, ASHRAE relationships for derived properties).
//!
//! Reference: EnergyPlus Engineering Reference, Chapter "Psychrometric Services"
//! Source: github.com/NREL/EnergyPlus/src/EnergyPlus/Psychrometrics.cc

pub mod comfort;

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

        (C8 / t_kel + C9 + t_kel * (C10 + t_kel * (C11 + t_kel * C12)) + C13 * t_kel.ln()).exp()
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
/// ASHRAE *Fundamentals* Ch. 1 Eq. 33 and 35, with an ice branch. A deliberate
/// difference from E+ `PsyWFnTdbTwbPb`, which uses other constants and no ice
/// branch: this is the exact inverse of ASHRAE's wet-bulb, which E+ also uses in
/// `PsyTwbFnTdbWPb`.
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

/// Enthalpy of water vapour at `t_db` [°C], in J/kg, E+ `PsyHgAirFnWTdb`: the heat
/// that one kilogram of moisture adds to or removes from the air.
pub fn hg_air_fn_tdb(t_db: f64) -> f64 {
    HFG_WATER + CP_VAPOR * t_db
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

/// Saturation temperature from enthalpy and barometric pressure.
///
/// Finds the temperature on the saturation curve where the saturated moist
/// air enthalpy equals `h`. Matches EnergyPlus `PsyTsatFnHPb` (used to
/// locate the apparatus dew point in DX coil models).
pub fn tsat_fn_h_pb(h: f64, p_b: f64) -> f64 {
    // h_sat(T) is monotonically increasing — bisection over the E+ range.
    let mut lo = -50.0_f64;
    let mut hi = 100.0_f64;
    let h_sat = |t: f64| h_fn_tdb_w(t, w_fn_tdb_rh_pb(t, 1.0, p_b));

    if h <= h_sat(lo) {
        return lo;
    }
    if h >= h_sat(hi) {
        return hi;
    }
    for _ in 0..60 {
        let mid = 0.5 * (lo + hi);
        if h_sat(mid) < h {
            lo = mid;
        } else {
            hi = mid;
        }
        if hi - lo < 1.0e-5 {
            break;
        }
    }
    0.5 * (lo + hi)
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
    // E+ PsyRhoAirFnPbTdbW: rho = pb / (287 · T_K · (1 + 1.6077687·W)).
    // Note this is the dry-air partial density (kg dry air per m³ of moist
    // air), not total moist-air density (which would carry an extra (1+W)
    // factor, ~+1% at W=0.01). E+ uses this form consistently for
    // volumetric-to-mass flow conversions, and E+ math is the reference
    // standard — flows and capacities calibrate against it.
    p_b / (287.0 * (t_db + KELVIN) * (1.0 + 1.6077687 * w))
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

/// E+ converts a design chilled- or condenser-water volume flow to mass at this
/// temperature [°C] (`CWInitConvTemp`, `InitConvTemp`).
pub const COLD_WATER_CONVERSION_TEMP: f64 = 5.05;

/// E+ converts a design hot-water volume flow to mass at this temperature [°C]
/// (`HWInitConvTemp`).
pub const HOT_WATER_CONVERSION_TEMP: f64 = 60.0;

/// Water specific heat [J/(kg·K)] every 5 °C from 0 °C, E+ `DefaultWaterCpData`.
const WATER_CP: [f64; 26] = [
    4217.0, 4198.0, 4191.0, 4185.0, 4181.0, 4179.0, 4180.0, 4180.0, 4180.0, 4180.0, 4181.0, 4183.0,
    4185.0, 4188.0, 4192.0, 4196.0, 4200.0, 4203.0, 4208.0, 4213.0, 4218.0, 4223.0, 4228.0, 4233.0,
    4238.0, 4243.0,
];

/// Water density [kg/m³] every 5 °C from 0 °C, E+ `DefaultWaterRhoData`.
const WATER_RHO: [f64; 21] = [
    999.8, 999.9, 999.7, 999.1, 998.2, 997.0, 995.6, 994.0, 992.2, 990.2, 988.0, 985.7, 983.2,
    980.5, 977.7, 974.8, 971.8, 968.6, 965.3, 961.9, 958.3,
];

/// Linear interpolation in a table every 5 °C from 0 °C, holding the end values.
fn water_table(table: &[f64], t: f64) -> f64 {
    let x = (t / 5.0).clamp(0.0, (table.len() - 1) as f64);
    let i = (x as usize).min(table.len() - 2);
    table[i] + (x - i as f64) * (table[i + 1] - table[i])
}

/// Specific heat of liquid water at `t` [°C], in J/(kg·K).
pub fn water_cp(t: f64) -> f64 {
    water_table(&WATER_CP, t)
}

/// Density of liquid water at `t` [°C], in kg/m³.
pub fn water_rho(t: f64) -> f64 {
    water_table(&WATER_RHO, t)
}

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
            cp: water_cp(temp),
        }
    }

    /// Heat capacity rate [W/K]
    pub fn capacity_rate(&self) -> f64 {
        self.mass_flow * self.cp
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn water_properties_match_the_eplus_table() {
        assert_eq!(water_cp(0.0), 4217.0);
        assert_eq!(water_cp(35.0), 4180.0);
        assert_eq!(water_cp(80.0), 4200.0);
        assert_eq!(water_rho(0.0), 999.8);
        assert_eq!(water_rho(80.0), 971.8);
        // Linear in between, flat beyond the ends.
        assert_relative_eq!(water_rho(82.5), 0.5 * (971.8 + 968.6), max_relative = 1e-12);
        assert_relative_eq!(water_rho(5.05), 999.9 - 0.2 * 0.01, max_relative = 1e-12);
        assert_eq!(water_rho(150.0), 958.3);
        assert_eq!(water_cp(-5.0), 4217.0);
        assert_eq!(FluidState::water(82.0, 1.0).cp, water_cp(82.0));
    }

    #[test]
    fn air_density_and_vapour_enthalpy_match_eplus() {
        // E+ PsyRhoAirFnPbTdbW and PsyHgAirFnWTdb, by hand.
        let rho = 101_325.0 / (287.0 * 293.15 * (1.0 + 1.6077687 * 0.01));
        assert_relative_eq!(
            rho_air_fn_pb_tdb_w(101_325.0, 20.0, 0.01),
            rho,
            max_relative = 1e-12
        );
        assert_eq!(hg_air_fn_tdb(22.0), 2_500_940.0 + 1858.95 * 22.0);
    }
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
    fn test_tsat_fn_h_pb_round_trip() {
        // h of saturated air at 10°C should invert back to 10°C
        for t in [0.0, 5.0, 10.0, 15.0, 20.0, 30.0] {
            let w_sat = w_fn_tdb_rh_pb(t, 1.0, 101325.0);
            let h = h_fn_tdb_w(t, w_sat);
            let t_back = tsat_fn_h_pb(h, 101325.0);
            assert_relative_eq!(t_back, t, epsilon = 0.01);
        }
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
