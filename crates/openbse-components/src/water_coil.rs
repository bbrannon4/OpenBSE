//! Water-to-air coil heat transfer, shared by hot-water coils, chilled-water
//! coils and hot-water reheat terminals.
//!
//! The models follow EnergyPlus `WaterCoils.cc`:
//!
//! - **Heating** (`CalcSimpleHeatingCoil`): cross-flow ε-NTU, both streams
//!   unmixed, with the convective resistances following flow and temperature
//!   (`CalcAdjustedCoilUA`, from Wetter 1999).
//! - **Cooling** (`CoolingCoil`, SimpleAnalysis): counterflow. Dry when the
//!   entering dew point is at or below the entering water temperature
//!   (`CoilCompletelyDry`); otherwise an enthalpy-based wet coil
//!   (`CoilCompletelyWet`, `WetCoilOutletCondition`).
//! - **Sizing**: UA is solved so the model itself delivers the design load at
//!   the design state, as E+ does (`InitWaterCoil`, `WaterCoilUA` sizing).
//! - **Control**: water flow is modulated to hold the outlet setpoint, up to
//!   the flow available (`Controller:WaterCoil`).
//!
//! One deliberate difference: when the entering dew point is above the water
//! temperature E+ SimpleAnalysis always takes the wet result, which falls below
//! the dry result on a coil that is mostly dry. Here the larger of the two is
//! taken, which brackets the part-wet coil (Braun 1988; the ASHRAE HVAC 2
//! Toolkit makes the same choice) without a discontinuity at the dew point.

use openbse_psychrometrics as psych;
use serde::{Deserialize, Serialize};

/// Ratio of air-side to water-side convective conductance at the design point
/// (E+ `Coil:Heating:Water` "Rated Ratio for Air and Water Convection" default).
const HEATING_AIR_TO_WATER_CONVECTION: f64 = 0.5;
/// Water-side over air-side UA of a cooling coil (E+ `InitWaterCoil`).
const COOLING_WATER_TO_AIR_UA: f64 = 3.3;
/// Highest effectiveness a design point may ask of the heating coil.
const MAX_DESIGN_EFFECTIVENESS: f64 = 0.99;
/// Air-temperature tolerance of the water-flow control [K]. E+
/// `Controller:WaterCoil` autosizes its tolerance to 0.01 K.
const CONTROL_TOLERANCE: f64 = 0.001;
/// Flow below this is treated as no flow [kg/s] (E+ `MassFlowTolerance`).
const MIN_WATER_FLOW: f64 = 1.0e-9;

/// E+ `CalcAdjustedCoilUA`: air-side temperature sensitivity.
fn air_temp_factor(t_air: f64, t_air_des: f64) -> f64 {
    1.0 + 4.769e-3 * (t_air - t_air_des)
}

/// E+ `CalcAdjustedCoilUA`: water-side temperature sensitivity.
fn water_temp_factor(t_water: f64, t_water_des: f64) -> f64 {
    1.0 + 0.014 / (1.0 + 0.014 * t_water_des) * (t_water - t_water_des)
}

/// Cross-flow effectiveness with both streams unmixed (Incropera Table 11.3,
/// E+ `CalcSimpleHeatingCoil`).
pub fn crossflow_effectiveness(ntu: f64, c_ratio: f64) -> f64 {
    if ntu <= 0.0 {
        return 0.0;
    }
    if c_ratio < 1.0e-6 {
        return 1.0 - (-ntu).exp();
    }
    let eta = ntu.powf(0.22);
    let a = c_ratio * ntu / eta;
    let a = if a > 20.0 {
        eta / c_ratio
    } else {
        eta * (1.0 - (-a).exp()) / c_ratio
    };
    if a > 20.0 {
        1.0
    } else {
        1.0 - (-a).exp()
    }
}

/// Counterflow effectiveness (E+ `CoilOutletStreamCondition`).
pub fn counterflow_effectiveness(ntu: f64, c_ratio: f64) -> f64 {
    if ntu <= 0.0 {
        return 0.0;
    }
    if (c_ratio - 1.0).abs() < 1.0e-9 {
        return ntu / (ntu + 1.0);
    }
    let e = (-(ntu * (1.0 - c_ratio)).min(50.0)).exp();
    (1.0 - e) / (1.0 - c_ratio * e)
}

/// Find `x` in `[lo, hi]` where the monotonic `f` crosses zero, given
/// `f(lo)` and `f(hi)` have opposite signs (Illinois regula falsi).
fn solve_bracketed(
    mut f: impl FnMut(f64) -> f64,
    mut lo: f64,
    mut f_lo: f64,
    mut hi: f64,
    mut f_hi: f64,
    tol: f64,
) -> f64 {
    let mut side = 0;
    for _ in 0..60 {
        let x = (lo * f_hi - hi * f_lo) / (f_hi - f_lo);
        let fx = f(x);
        if fx.abs() <= tol || (hi - lo).abs() <= 1.0e-9 * hi.abs().max(1.0e-9) {
            return x;
        }
        if (fx > 0.0) == (f_lo > 0.0) {
            lo = x;
            f_lo = fx;
            if side == -1 {
                f_hi *= 0.5;
            }
            side = -1;
        } else {
            hi = x;
            f_hi = fx;
            if side == 1 {
                f_lo *= 0.5;
            }
            side = 1;
        }
    }
    0.5 * (lo + hi)
}

/// Design point of a hot-water coil.
#[derive(Debug, Clone, Copy)]
pub struct HeatingDesign {
    /// Design heat rate [W]
    pub q: f64,
    /// Design air flow [kg/s]
    pub air_flow: f64,
    /// Design entering air temperature [°C]
    pub t_air_in: f64,
    /// Design water flow [kg/s]
    pub water_flow: f64,
    /// Design entering water temperature [°C]
    pub t_water_in: f64,
}

/// A sized hot-water coil.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct HeatingUa {
    /// Overall UA at the design point [W/K]
    pub ua_design: f64,
    /// Whether the design load was within reach of the design temperatures.
    pub reaches_design: bool,
    /// Design water flow [kg/s], the most the coil's control valve passes
    pub water_flow_design: f64,
    air_flow_design: f64,
    t_air_design: f64,
    t_water_design: f64,
    air_convection: f64,
    water_convection: f64,
}

impl HeatingUa {
    /// Solve for the UA at which the coil delivers the design load.
    pub fn size(d: &HeatingDesign, cp_air: f64) -> Self {
        let c_air = d.air_flow * cp_air;
        let c_water = d.water_flow * psych::water_cp(d.t_water_in);
        let c_min = c_air.min(c_water);
        let c_ratio = c_min / c_air.max(c_water).max(1.0e-9);
        let q_max = c_min * (d.t_water_in - d.t_air_in);
        let wanted = if q_max > 0.0 { d.q / q_max } else { 1.0 };
        let reaches_design = wanted < MAX_DESIGN_EFFECTIVENESS;
        let target = wanted.min(MAX_DESIGN_EFFECTIVENESS);
        // Effectiveness rises with NTU, and doubling from a small start brackets
        // it. A cross-flow coil at c_ratio ≈ 1 tops out near 0.99 only at large
        // NTU, so the search stops at a bound that is already a huge coil.
        let mut hi = 1.0;
        while crossflow_effectiveness(hi, c_ratio) < target && hi < 1.0e3 {
            hi *= 2.0;
        }
        let ntu = solve_bracketed(
            |n| crossflow_effectiveness(n, c_ratio) - target,
            0.0,
            -target,
            hi,
            crossflow_effectiveness(hi, c_ratio) - target,
            1.0e-7,
        );
        let ua_design = (ntu * c_min).max(1.0e-6);
        // E+ InitWaterCoil: split UA into its convective parts.
        let r = HEATING_AIR_TO_WATER_CONVECTION;
        let water_convection = ua_design * (r + 1.0) / r;
        Self {
            ua_design,
            reaches_design,
            water_flow_design: d.water_flow,
            air_flow_design: d.air_flow,
            t_air_design: d.t_air_in,
            t_water_design: d.t_water_in,
            air_convection: r * water_convection,
            water_convection,
        }
    }

    fn ua(&self, air_flow: f64, t_air: f64, water_flow: f64, t_water: f64) -> f64 {
        let air = air_temp_factor(t_air, self.t_air_design)
            * (air_flow / self.air_flow_design).powf(0.8)
            * self.air_convection;
        let water = water_temp_factor(t_water, self.t_water_design)
            * (water_flow / self.water_flow_design).powf(0.85)
            * self.water_convection;
        if air > 0.0 && water > 0.0 {
            1.0 / (1.0 / water + 1.0 / air)
        } else {
            self.ua_design
        }
    }

    /// Heat rate [W] at a given water flow.
    pub fn heat_rate(
        &self,
        air_flow: f64,
        cp_air: f64,
        t_air: f64,
        water_flow: f64,
        t_water: f64,
    ) -> f64 {
        if air_flow <= 0.0 || water_flow <= MIN_WATER_FLOW || t_water <= t_air {
            return 0.0;
        }
        let c_air = air_flow * cp_air;
        let c_water = water_flow * psych::water_cp(t_water);
        let c_min = c_air.min(c_water);
        let ntu = self.ua(air_flow, t_air, water_flow, t_water) / c_min;
        crossflow_effectiveness(ntu, c_min / c_air.max(c_water)) * c_min * (t_water - t_air)
    }

    /// Water flow that delivers `q_wanted`, up to `water_available`. Returns the
    /// heat delivered [W] and the water flow [kg/s].
    pub fn control(
        &self,
        q_wanted: f64,
        air_flow: f64,
        cp_air: f64,
        t_air: f64,
        water_available: f64,
        t_water: f64,
    ) -> (f64, f64) {
        let max_flow = water_available.min(self.water_flow_design);
        if q_wanted <= 0.0 || max_flow <= MIN_WATER_FLOW {
            return (0.0, 0.0);
        }
        let q_full = self.heat_rate(air_flow, cp_air, t_air, max_flow, t_water);
        if q_full <= q_wanted {
            return if q_full > 0.0 {
                (q_full, max_flow)
            } else {
                (0.0, 0.0)
            };
        }
        let tol = (CONTROL_TOLERANCE * air_flow * cp_air).max(1.0e-3);
        let flow = solve_bracketed(
            |m| self.heat_rate(air_flow, cp_air, t_air, m, t_water) - q_wanted,
            0.0,
            -q_wanted,
            max_flow,
            q_full - q_wanted,
            tol,
        );
        // The setpoint is what the valve holds; this leaves the air at it exactly.
        (q_wanted, flow.max(MIN_WATER_FLOW))
    }
}

/// Design point of a chilled-water coil.
#[derive(Debug, Clone, Copy)]
pub struct CoolingDesign {
    /// Design total cooling rate [W]
    pub q_total: f64,
    /// Design air flow [kg/s]
    pub air_flow: f64,
    /// Design entering air temperature [°C]
    pub t_air_in: f64,
    /// Design entering air humidity ratio [kg/kg]
    pub w_air_in: f64,
    /// Design water flow [kg/s]
    pub water_flow: f64,
    /// Design entering water temperature [°C]
    pub t_water_in: f64,
    /// Barometric pressure [Pa]
    pub p_b: f64,
}

/// A chilled-water coil's result at one operating point.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CoolingResult {
    pub t_air_out: f64,
    pub w_air_out: f64,
    /// Total cooling [W], the air's enthalpy drop
    pub q_total: f64,
    /// Sensible cooling [W]
    pub q_sensible: f64,
}

/// A sized chilled-water coil.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct CoolingUa {
    /// External (air-side) UA at the design point [W/K]
    pub ua_external_design: f64,
    /// Internal (water-side) UA at the design point [W/K]
    pub ua_internal_design: f64,
    /// Whether the design load was within reach of the design temperatures.
    pub reaches_design: bool,
    /// Design water flow [kg/s], the most the coil's control valve passes
    pub water_flow_design: f64,
    air_flow_design: f64,
    t_air_design: f64,
    t_water_design: f64,
}

impl CoolingUa {
    /// Solve for the UA at which the coil delivers the design total load
    /// (E+ `InitWaterCoil`, its `SolveRoot` on `CoolingCoil`).
    pub fn size(d: &CoolingDesign) -> Self {
        let mut coil = Self {
            ua_external_design: 0.0,
            ua_internal_design: 0.0,
            reaches_design: true,
            water_flow_design: d.water_flow,
            air_flow_design: d.air_flow,
            t_air_design: d.t_air_in,
            t_water_design: d.t_water_in,
        };
        let delivered = |ua: f64, coil: &mut Self| {
            coil.ua_external_design = ua;
            coil.ua_internal_design = ua * COOLING_WATER_TO_AIR_UA;
            coil.perform(
                d.air_flow,
                d.t_air_in,
                d.w_air_in,
                d.p_b,
                d.water_flow,
                d.t_water_in,
            )
            .q_total
                - d.q_total
        };
        // A very large coil at the design flows; its estimate starts well above
        // any real UA, and heat transfer rises with UA.
        let estimate = d.q_total / (d.t_air_in - d.t_water_in).max(1.0);
        let lo = 0.01 * estimate;
        let hi = 1000.0 * estimate;
        let e_lo = delivered(lo, &mut coil);
        let e_hi = delivered(hi, &mut coil);
        let ua = if e_hi <= 0.0 {
            coil.reaches_design = false;
            hi
        } else if e_lo >= 0.0 {
            lo
        } else {
            // Heat transfer saturates with UA, so search on its logarithm.
            let log_ua = solve_bracketed(
                |x| delivered(x.exp(), &mut coil) / d.q_total,
                lo.ln(),
                e_lo / d.q_total,
                hi.ln(),
                e_hi / d.q_total,
                1.0e-5,
            );
            log_ua.exp()
        };
        coil.ua_external_design = ua;
        coil.ua_internal_design = ua * COOLING_WATER_TO_AIR_UA;
        coil
    }

    /// External and internal UA at this state (E+ `CalcAdjustedCoilUA`).
    fn adjusted_ua(&self, air_flow: f64, t_air: f64, water_flow: f64, t_water: f64) -> (f64, f64) {
        let external = air_temp_factor(t_air, self.t_air_design)
            * (air_flow / self.air_flow_design).powf(0.8)
            * self.ua_external_design;
        let internal = water_temp_factor(t_water, self.t_water_design)
            * (water_flow / self.water_flow_design).powf(0.85)
            * self.ua_internal_design;
        if external > 0.0 && internal > 0.0 {
            (external, internal)
        } else {
            (self.ua_external_design, self.ua_internal_design)
        }
    }

    /// E+ `CoilCompletelyDry`.
    fn dry(
        air_flow: f64,
        t_air: f64,
        w_air: f64,
        water_flow: f64,
        t_water: f64,
        ua_total: f64,
    ) -> CoolingResult {
        let cp_air = psych::cp_air_fn_w(w_air);
        let c_air = air_flow * cp_air;
        let c_water = water_flow * psych::water_cp(t_water);
        let c_min = c_air.min(c_water);
        let eff = counterflow_effectiveness(ua_total / c_min, c_min / c_air.max(c_water));
        let q = (eff * c_min * (t_air - t_water)).max(0.0);
        CoolingResult {
            t_air_out: t_air - q / c_air,
            w_air_out: w_air,
            q_total: q,
            q_sensible: q,
        }
    }

    /// E+ `CoilCompletelyWet` and `WetCoilOutletCondition`.
    #[allow(clippy::too_many_arguments)]
    fn wet(
        air_flow: f64,
        t_air: f64,
        w_air: f64,
        p_b: f64,
        water_flow: f64,
        t_water: f64,
        ua_external: f64,
        ua_internal: f64,
    ) -> CoolingResult {
        let h_in = psych::h_fn_tdb_w(t_air, w_air);
        let h_sat_water = psych::h_fn_tdb_w(t_water, psych::w_fn_tdp_pb(t_water, p_b));
        let t_dp = psych::tdp_fn_w_pb(w_air, p_b);
        // Slope of the saturation line between the water and the dew point.
        let cp_sat = (psych::h_fn_tdb_w(t_dp, psych::w_fn_tdp_pb(t_dp, p_b)) - h_sat_water)
            / (t_dp - t_water).max(1.0e-6);
        let cp_dry = psych::cp_air_fn_w(0.0);
        let ua_enthalpy = 1.0 / (cp_sat / ua_internal + cp_dry / ua_external);
        let c_water = water_flow * psych::water_cp(t_water) / cp_sat;
        let c_min = air_flow.min(c_water);
        let eff = counterflow_effectiveness(ua_enthalpy / c_min, c_min / air_flow.max(c_water));
        let q = (eff * c_min * (h_in - h_sat_water)).max(0.0);
        let h_out = h_in - q / air_flow;

        // Outlet state: the air moves toward the surface condition.
        let cp_air = psych::cp_air_fn_w(w_air);
        let c_air = air_flow * cp_air;
        let surface_eff = (1.0 - (-ua_external / c_air).exp()).max(1.0e-9);
        let h_surface = h_in - (h_in - h_out) / surface_eff;
        let t_surface = psych::tsat_fn_h_pb(h_surface, p_b);
        let (mut t_out, mut w_out) = if t_dp - t_surface > 0.1 {
            let t = t_air - (t_air - t_surface) * surface_eff;
            (t, psych::w_fn_tdb_h(t, h_out))
        } else {
            (psych::tdb_fn_h_w(h_out, w_air), w_air)
        };
        if w_out > w_air {
            w_out = w_air;
            t_out = psych::tdb_fn_h_w(h_out, w_air);
        }
        // The air's state cannot pass saturation; hold its enthalpy (and so the
        // load) and move it onto the saturation line.
        if w_out > psych::w_fn_tdp_pb(t_out, p_b) {
            t_out = psych::tsat_fn_h_pb(h_out, p_b);
            w_out = psych::w_fn_tdb_h(t_out, h_out).min(w_air);
        }
        CoolingResult {
            t_air_out: t_out,
            w_air_out: w_out,
            q_total: air_flow * (h_in - psych::h_fn_tdb_w(t_out, w_out)),
            q_sensible: c_air * (t_air - t_out),
        }
    }

    /// The coil's performance at a given water flow.
    pub fn perform(
        &self,
        air_flow: f64,
        t_air: f64,
        w_air: f64,
        p_b: f64,
        water_flow: f64,
        t_water: f64,
    ) -> CoolingResult {
        let idle = CoolingResult {
            t_air_out: t_air,
            w_air_out: w_air,
            ..Default::default()
        };
        if air_flow <= 0.0 || water_flow <= MIN_WATER_FLOW || t_water >= t_air {
            return idle;
        }
        let (external, internal) = self.adjusted_ua(air_flow, t_air, water_flow, t_water);
        let ua_total = 1.0 / (1.0 / external + 1.0 / internal);
        let dry = Self::dry(air_flow, t_air, w_air, water_flow, t_water, ua_total);
        if psych::tdp_fn_w_pb(w_air, p_b) <= t_water {
            return dry;
        }
        let wet = Self::wet(
            air_flow, t_air, w_air, p_b, water_flow, t_water, external, internal,
        );
        if wet.q_total >= dry.q_total {
            wet
        } else {
            dry
        }
    }

    /// Water flow that holds the outlet at `t_setpoint`, up to `water_available`.
    /// Returns the coil's result and the water flow [kg/s].
    pub fn control(
        &self,
        t_setpoint: f64,
        air_flow: f64,
        air: &psych::MoistAirState,
        water_available: f64,
        t_water: f64,
    ) -> (CoolingResult, f64) {
        let (t_air, w_air, p_b) = (air.t_db, air.w, air.p_b);
        let idle = CoolingResult {
            t_air_out: t_air,
            w_air_out: w_air,
            ..Default::default()
        };
        let max_flow = water_available.min(self.water_flow_design);
        if t_air <= t_setpoint || max_flow <= MIN_WATER_FLOW || t_water >= t_air {
            return (idle, 0.0);
        }
        let full = self.perform(air_flow, t_air, w_air, p_b, max_flow, t_water);
        if full.t_air_out >= t_setpoint - CONTROL_TOLERANCE {
            return (full, max_flow);
        }
        let flow = solve_bracketed(
            |m| {
                self.perform(air_flow, t_air, w_air, p_b, m, t_water)
                    .t_air_out
                    - t_setpoint
            },
            0.0,
            t_air - t_setpoint,
            max_flow,
            full.t_air_out - t_setpoint,
            CONTROL_TOLERANCE,
        )
        .max(MIN_WATER_FLOW);
        (
            self.perform(air_flow, t_air, w_air, p_b, flow, t_water),
            flow,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_relative_eq;

    const P: f64 = 101_325.0;

    /// E+ rated point of `Coil:Heating:Water`.
    fn rated_heating(q: f64) -> (HeatingDesign, f64) {
        let cp_air = psych::cp_air_fn_w(0.008);
        (
            HeatingDesign {
                q,
                air_flow: q / (cp_air * (32.2 - 16.6)),
                t_air_in: 16.6,
                water_flow: q / (psych::water_cp(82.2) * (82.2 - 71.1)),
                t_water_in: 82.2,
            },
            cp_air,
        )
    }

    #[test]
    fn crossflow_effectiveness_matches_the_correlation() {
        // Both-unmixed values at Cr = 0.5 (Incropera Table 11.3).
        for (ntu, eff) in [(0.5, 0.352), (1.0, 0.545), (2.0, 0.739), (4.0, 0.877)] {
            assert_relative_eq!(crossflow_effectiveness(ntu, 0.5), eff, epsilon = 0.002);
        }
        // Limits: Cr → 0, NTU → 0.
        assert_relative_eq!(
            crossflow_effectiveness(2.0, 0.0),
            1.0 - (-2.0_f64).exp(),
            epsilon = 1e-9
        );
        assert_eq!(crossflow_effectiveness(0.0, 0.5), 0.0);
    }

    #[test]
    fn counterflow_effectiveness_limits() {
        assert_relative_eq!(counterflow_effectiveness(1.0, 1.0), 0.5, epsilon = 1e-9);
        assert_relative_eq!(
            counterflow_effectiveness(2.0, 0.0),
            1.0 - (-2.0_f64).exp(),
            epsilon = 1e-9
        );
    }

    #[test]
    fn heating_ua_reproduces_the_design_load() {
        let (d, cp_air) = rated_heating(10_000.0);
        let coil = HeatingUa::size(&d, cp_air);
        assert!(coil.reaches_design);
        assert_relative_eq!(
            coil.heat_rate(d.air_flow, cp_air, d.t_air_in, d.water_flow, d.t_water_in),
            d.q,
            max_relative = 1e-4
        );
    }

    #[test]
    fn heating_capacity_follows_water_temperature_and_flow() {
        let (d, cp_air) = rated_heating(10_000.0);
        let coil = HeatingUa::size(&d, cp_air);
        let q = |m, t| coil.heat_rate(d.air_flow, cp_air, d.t_air_in, m, t);
        let q60 = q(d.water_flow, 60.0);
        let q40 = q(d.water_flow, 40.0);
        // Capacity scales close to the entering temperature difference.
        assert!(q60 < d.q && q40 < q60, "{q40} {q60}");
        let linear = d.q * (60.0 - 16.6) / (82.2 - 16.6);
        assert_relative_eq!(q60, linear, max_relative = 0.05);
        // Starving the coil lowers the output, but less than in proportion:
        // the coil's characteristic concave valve curve.
        let half = q(0.5 * d.water_flow, d.t_water_in);
        assert!(half < d.q && half > 0.5 * d.q, "{half}");
    }

    #[test]
    fn heating_control_meets_the_load_on_less_water_and_conserves_energy() {
        let (d, cp_air) = rated_heating(10_000.0);
        let coil = HeatingUa::size(&d, cp_air);
        let (q, m) = coil.control(
            5_000.0,
            d.air_flow,
            cp_air,
            d.t_air_in,
            d.water_flow,
            d.t_water_in,
        );
        assert_relative_eq!(q, 5_000.0);
        assert!(
            m < 0.5 * d.water_flow,
            "half load on {m} of {}",
            d.water_flow
        );
        assert_relative_eq!(
            coil.heat_rate(d.air_flow, cp_air, d.t_air_in, m, d.t_water_in),
            q,
            max_relative = 1e-3
        );
        // An over-large demand gets the full-flow output, on design water.
        let (q, m) = coil.control(
            1.0e6,
            d.air_flow,
            cp_air,
            d.t_air_in,
            2.0 * d.water_flow,
            d.t_water_in,
        );
        assert_relative_eq!(q, d.q, max_relative = 1e-3);
        assert_relative_eq!(m, d.water_flow);
    }

    #[test]
    fn heating_water_colder_than_air_or_no_water_does_nothing() {
        let (d, cp_air) = rated_heating(10_000.0);
        let coil = HeatingUa::size(&d, cp_air);
        assert_eq!(
            coil.control(5_000.0, d.air_flow, cp_air, 30.0, d.water_flow, 25.0),
            (0.0, 0.0)
        );
        assert_eq!(
            coil.control(5_000.0, d.air_flow, cp_air, 16.6, 0.0, 82.2),
            (0.0, 0.0)
        );
    }

    #[test]
    fn heating_design_beyond_reach_is_flagged_not_silently_met() {
        // 60 °C water cannot bring this air flow to the 100 K rise asked of it.
        let cp_air = psych::cp_air_fn_w(0.008);
        let d = HeatingDesign {
            q: 100_000.0,
            air_flow: 1.0,
            t_air_in: 10.0,
            water_flow: 5.0,
            t_water_in: 60.0,
        };
        let coil = HeatingUa::size(&d, cp_air);
        assert!(!coil.reaches_design);
        let q = coil.heat_rate(d.air_flow, cp_air, d.t_air_in, d.water_flow, d.t_water_in);
        assert!(q < 1.0 * cp_air * 50.0, "{q}");
    }

    fn typical_cooling() -> CoolingDesign {
        // 1 kg/s from 26 °C, 50% RH toward 13 °C on 6.7 °C water, 5.6 K rise.
        let w = psych::w_fn_tdb_rh_pb(26.0, 0.5, P);
        let q_total = psych::h_fn_tdb_w(26.0, w) - psych::h_fn_tdb_w(13.0, 0.0085);
        CoolingDesign {
            q_total,
            air_flow: 1.0,
            t_air_in: 26.0,
            w_air_in: w,
            water_flow: q_total / (psych::water_cp(6.7) * 5.6),
            t_water_in: 6.7,
            p_b: P,
        }
    }

    fn at_design(c: &CoolingUa, d: &CoolingDesign, m: f64, t: f64) -> CoolingResult {
        c.perform(d.air_flow, d.t_air_in, d.w_air_in, d.p_b, m, t)
    }

    #[test]
    fn cooling_ua_reproduces_the_design_load_with_e_plus_split() {
        let d = typical_cooling();
        let coil = CoolingUa::size(&d);
        assert!(coil.reaches_design);
        assert_relative_eq!(
            coil.ua_internal_design / coil.ua_external_design,
            3.3,
            epsilon = 1e-9
        );
        let r = at_design(&coil, &d, d.water_flow, d.t_water_in);
        assert_relative_eq!(r.q_total, d.q_total, max_relative = 1e-3);
    }

    #[test]
    fn cooling_wet_coil_leaves_air_short_of_saturation_and_closes_energy() {
        let d = typical_cooling();
        let coil = CoolingUa::size(&d);
        let r = at_design(&coil, &d, d.water_flow, d.t_water_in);
        // A real coil leaves the air short of saturation.
        let rh = psych::rh_fn_tdb_w_pb(r.t_air_out, r.w_air_out, P);
        assert!(rh < 0.995 && rh > 0.7, "RH {rh}");
        assert!(r.w_air_out < d.w_air_in);
        assert!(r.q_total > r.q_sensible);
        // The total is the air's enthalpy drop.
        let drop = d.air_flow
            * (psych::h_fn_tdb_w(d.t_air_in, d.w_air_in)
                - psych::h_fn_tdb_w(r.t_air_out, r.w_air_out));
        assert_relative_eq!(r.q_total, drop, max_relative = 1e-9);
    }

    #[test]
    fn cooling_dry_air_keeps_its_moisture() {
        let d = typical_cooling();
        let coil = CoolingUa::size(&d);
        let w = 0.003; // dew point ≈ −2 °C, below the water
        let r = coil.perform(d.air_flow, 26.0, w, P, d.water_flow, 6.7);
        assert_eq!(r.w_air_out, w);
        assert_relative_eq!(r.q_total, r.q_sensible);
        assert!(r.t_air_out < 26.0 && r.t_air_out > 6.7);
    }

    #[test]
    fn cooling_mostly_dry_coil_is_continuous_across_the_dew_point() {
        let d = typical_cooling();
        let coil = CoolingUa::size(&d);
        let w_dp = psych::w_fn_tdp_pb(6.7, P);
        let below = coil.perform(d.air_flow, 26.0, 0.999 * w_dp, P, d.water_flow, 6.7);
        let above = coil.perform(d.air_flow, 26.0, 1.001 * w_dp, P, d.water_flow, 6.7);
        assert_relative_eq!(below.q_total, above.q_total, max_relative = 0.01);
    }

    #[test]
    fn cooling_capacity_follows_water_temperature_and_flow() {
        let d = typical_cooling();
        let coil = CoolingUa::size(&d);
        let full = at_design(&coil, &d, d.water_flow, 6.7).q_total;
        let warm = at_design(&coil, &d, d.water_flow, 12.0).q_total;
        let starved = at_design(&coil, &d, 0.25 * d.water_flow, 6.7).q_total;
        assert!(warm < 0.8 * full, "{warm} {full}");
        assert!(starved < full && starved > 0.25 * full, "{starved} {full}");
        // The warm loop also dehumidifies less.
        let sens_ratio = |r: CoolingResult| r.q_sensible / r.q_total;
        assert!(
            sens_ratio(at_design(&coil, &d, d.water_flow, 12.0))
                > sens_ratio(at_design(&coil, &d, d.water_flow, 6.7))
        );
    }

    #[test]
    fn cooling_control_holds_the_setpoint_by_water_flow() {
        let d = typical_cooling();
        let coil = CoolingUa::size(&d);
        let (r, m) = coil.control(
            18.0,
            d.air_flow,
            &psych::MoistAirState::new(26.0, d.w_air_in, P),
            d.water_flow,
            6.7,
        );
        assert_relative_eq!(r.t_air_out, 18.0, epsilon = 2.0 * CONTROL_TOLERANCE);
        assert!(m < d.water_flow && m > 0.0);
        // Beyond its reach it runs flat out on the water available.
        let limited = 0.1 * d.water_flow;
        let (r, m) = coil.control(
            13.0,
            d.air_flow,
            &psych::MoistAirState::new(26.0, d.w_air_in, P),
            limited,
            6.7,
        );
        assert!(r.t_air_out > 13.0);
        assert_relative_eq!(m, limited);
        // Air at or below the setpoint, or water warmer than the air: no flow.
        let (r, m) = coil.control(
            13.0,
            d.air_flow,
            &psych::MoistAirState::new(12.0, 0.006, P),
            d.water_flow,
            6.7,
        );
        assert_eq!((r.q_total, m), (0.0, 0.0));
        let (r, m) = coil.control(
            13.0,
            d.air_flow,
            &psych::MoistAirState::new(26.0, 0.006, P),
            d.water_flow,
            30.0,
        );
        assert_eq!((r.q_total, m), (0.0, 0.0));
    }
}
