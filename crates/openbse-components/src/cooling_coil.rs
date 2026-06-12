//! DX cooling coil component model.
//!
//! Models a single-speed direct expansion (DX) cooling coil as found in
//! packaged rooftop units and split systems.
//!
//! Simplified steady-state model:
//! - Rated capacity and COP at ARI conditions (35°C outdoor, 26.7°C DB / 19.4°C WB indoor)
//! - Capacity and COP derate with outdoor temperature
//! - Sensible heat ratio (SHR) determines split between sensible and latent cooling
//! - Part-load ratio (PLR) determines fraction of capacity used
//!
//! Reference: EnergyPlus Engineering Reference, "Coil:Cooling:DX:SingleSpeed"

use crate::performance_curve::PerformanceCurve;
use openbse_core::ports::*;
use openbse_psychrometrics::{self as psych};
use serde::{Deserialize, Serialize};

fn default_submeter() -> String {
    "General".to_string()
}

/// DX cooling coil component.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoolingCoilDX {
    pub name: String,
    #[serde(default = "default_submeter")]
    pub submeter: String,
    /// Rated total cooling capacity [W] at ARI conditions
    pub rated_capacity: f64,
    /// Rated COP (coefficient of performance) at ARI conditions
    pub rated_cop: f64,
    /// Rated sensible heat ratio [0-1] at ARI conditions
    pub rated_shr: f64,
    /// Rated air flow rate [m³/s]
    pub rated_airflow: f64,
    /// Desired outlet air temperature setpoint [°C]
    pub outlet_temp_setpoint: f64,

    /// Optional capacity modifier curve: f(T_wb_entering, T_db_outdoor)
    #[serde(skip)]
    pub cap_ft_curve: Option<PerformanceCurve>,
    /// Optional EIR modifier curve: f(T_wb_entering, T_db_outdoor)
    #[serde(skip)]
    pub eir_ft_curve: Option<PerformanceCurve>,
    /// Optional PLF curve: PLF = f(PLR). Maps part-load ratio to part-load
    /// fraction, accounting for cycling losses (compressor on/off).
    /// If None, uses default: PLF = 1 - 0.15 × (1 - PLR).
    #[serde(skip)]
    pub plf_curve: Option<PerformanceCurve>,
    /// Optional capacity modifier curve as f(flow_fraction).
    /// Flow fraction = actual_airflow / rated_airflow.
    #[serde(skip)]
    pub cap_fflow_curve: Option<PerformanceCurve>,
    /// Optional EIR modifier curve as f(flow_fraction).
    #[serde(skip)]
    pub eir_fflow_curve: Option<PerformanceCurve>,
    /// Normalization factor for EIR curve: 1 / eir_curve(19.44, 35).
    /// Per E+ docs, EIR-fT should equal 1.0 at ARI rated conditions
    /// (19.44°C WB entering, 35°C outdoor).  If the curve isn't
    /// normalized, this factor corrects it so rated_COP equals the
    /// actual COP at rated conditions.
    #[serde(skip)]
    eir_normalization: f64,
    /// When true, compute SHR each timestep via apparatus dew point method.
    /// When false, use constant rated_shr (or SHR=1.0 if no latent model).
    pub autocalculate_shr: bool,
    /// Bypass factor derived from rated SHR at ARI conditions.
    /// Computed once, then reused each timestep.
    #[serde(skip)]
    bypass_factor: f64,
    /// Apparatus dew point temperature [°C] at rated conditions.
    #[serde(skip)]
    adp_temp: f64,
    /// Humidity ratio at the apparatus dew point [kg/kg].
    #[serde(skip)]
    adp_w: f64,
    /// Enthalpy at the apparatus dew point [J/kg].
    #[serde(skip)]
    adp_h: f64,
    /// Whether bypass factor has been initialized.
    #[serde(skip)]
    bf_initialized: bool,

    // ─── Runtime state ──────────────────────────────────────────────────
    /// Total cooling rate delivered to air [W] (positive = cooling)
    #[serde(skip)]
    pub cooling_rate: f64,
    /// Sensible cooling rate [W]
    #[serde(skip)]
    pub sensible_cooling_rate: f64,
    /// Electric power consumption [W]
    #[serde(skip)]
    pub power_consumption: f64,
    /// Part-load ratio from last simulate_air call
    #[serde(skip)]
    pub plr: f64,
    /// Runtime fraction from last simulate_air call
    #[serde(skip)]
    pub rtf: f64,
}

impl CoolingCoilDX {
    /// Create a new DX cooling coil.
    ///
    /// # Arguments
    /// * `name` - Component name
    /// * `rated_capacity` - Total cooling capacity at rated conditions [W]
    /// * `rated_cop` - COP at rated conditions (typically 3.0-5.0)
    /// * `rated_shr` - Sensible heat ratio (typically 0.7-0.85)
    /// * `rated_airflow` - Rated air volume flow rate [m³/s]
    /// * `setpoint` - Desired cooling coil outlet temperature [°C]
    pub fn new(
        name: &str,
        rated_capacity: f64,
        rated_cop: f64,
        rated_shr: f64,
        rated_airflow: f64,
        setpoint: f64,
    ) -> Self {
        Self {
            name: name.to_string(),
            submeter: "General".to_string(),
            rated_capacity,
            rated_cop,
            rated_shr,
            rated_airflow,
            outlet_temp_setpoint: setpoint,
            cap_ft_curve: None,
            eir_ft_curve: None,
            plf_curve: None,
            cap_fflow_curve: None,
            eir_fflow_curve: None,
            eir_normalization: 1.0,
            autocalculate_shr: false,
            bypass_factor: 0.0,
            adp_temp: 0.0,
            adp_w: 0.0,
            adp_h: 0.0,
            bf_initialized: false,
            cooling_rate: 0.0,
            sensible_cooling_rate: 0.0,
            power_consumption: 0.0,
            plr: 0.0,
            rtf: 0.0,
        }
    }

    /// Attach performance curves for capacity and EIR modifiers.
    ///
    /// Auto-normalizes the EIR curve so that it evaluates to 1.0 at
    /// ARI rated conditions (19.44 °C entering WB, 35 °C outdoor DB).
    /// This matches the E+ convention where the rated COP directly
    /// represents the COP at rated conditions.
    pub fn with_curves(
        mut self,
        cap_ft: Option<PerformanceCurve>,
        eir_ft: Option<PerformanceCurve>,
    ) -> Self {
        // Compute normalization factor for EIR curve
        if let Some(ref curve) = eir_ft {
            let eir_at_rated = curve.evaluate(19.44, 35.0);
            if eir_at_rated > 0.01 {
                self.eir_normalization = 1.0 / eir_at_rated;
            }
        }
        self.cap_ft_curve = cap_ft;
        self.eir_ft_curve = eir_ft;
        self
    }

    /// Attach a part-load fraction curve: PLF = f(PLR).
    pub fn with_plf_curve(mut self, plf: PerformanceCurve) -> Self {
        self.plf_curve = Some(plf);
        self
    }

    /// Attach flow-fraction modifier curves for capacity and EIR.
    pub fn with_fflow_curves(
        mut self,
        cap_fflow: Option<PerformanceCurve>,
        eir_fflow: Option<PerformanceCurve>,
    ) -> Self {
        self.cap_fflow_curve = cap_fflow;
        self.eir_fflow_curve = eir_fflow;
        self
    }

    /// Enable auto-SHR calculation using the apparatus dew point method.
    pub fn with_autocalculate_shr(mut self, enable: bool) -> Self {
        self.autocalculate_shr = enable;
        self
    }

    /// Calculate available cooling capacity at current conditions.
    ///
    /// Applies cap_ft (biquadratic f(Twb, Todb)) and cap_fflow (f(FF)) modifiers.
    /// At rated conditions (35°C ODB, 19.44°C WB, FF=1), correction is 1.0.
    fn available_capacity(&self, t_outdoor: f64, t_wb_inlet: f64, flow_fraction: f64) -> f64 {
        let ft_mod = if let Some(ref curve) = self.cap_ft_curve {
            curve.evaluate(t_wb_inlet, t_outdoor)
        } else {
            let t_rated = 35.0;
            (1.0 - 0.008 * (t_outdoor - t_rated)).clamp(0.5, 1.05)
        };
        let ff_mod = if let Some(ref curve) = self.cap_fflow_curve {
            curve.evaluate_1d(flow_fraction)
        } else {
            1.0
        };
        self.rated_capacity * ft_mod * ff_mod
    }

    /// Calculate COP at current conditions.
    ///
    /// Applies eir_ft (biquadratic) and eir_fflow (f(FF)) modifiers.
    /// COP = rated_COP / (EIR_ft × EIR_fflow).
    fn available_cop(&self, t_outdoor: f64, t_wb_inlet: f64, flow_fraction: f64) -> f64 {
        let eir_ft_mod = if let Some(ref curve) = self.eir_ft_curve {
            let eir_raw = curve.evaluate(t_wb_inlet, t_outdoor);
            eir_raw * self.eir_normalization
        } else {
            let t_rated = 35.0;
            let c = 1.0 + 0.012 * (t_outdoor - t_rated);
            c.clamp(0.4, 1.10) // Note: higher EIR = worse COP
        };
        let eir_ff_mod = if let Some(ref curve) = self.eir_fflow_curve {
            curve.evaluate_1d(flow_fraction)
        } else {
            1.0
        };
        let eir_total = (eir_ft_mod * eir_ff_mod).max(0.001);
        self.rated_cop / eir_total
    }

    /// Initialize bypass factor and apparatus dew point from rated SHR.
    ///
    /// Uses ARI rated indoor conditions: 26.67°C DB, 19.44°C WB.
    /// Iteratively finds the apparatus dew point (ADP) on the saturation
    /// curve such that the bypass factor (BF) is consistent with the
    /// rated SHR.
    fn initialize_bypass_factor(&mut self) {
        // ARI rated indoor entering conditions
        let t_db_rated = 26.67_f64;
        let t_wb_rated = 19.44_f64;
        let p_b = 101325.0_f64;

        let w_rated = psych::w_fn_tdb_twb_pb(t_db_rated, t_wb_rated, p_b);
        let h_rated = psych::h_fn_tdb_w(t_db_rated, w_rated);

        // The apparatus dew point (ADP) lies on the saturation curve.
        // We search for it by iterating: for a candidate ADP temperature,
        // compute the saturation humidity ratio and enthalpy, then check
        // whether the resulting SHR matches the rated SHR.
        //
        // SHR_rated = (h_in - h_adp) * BF_sensible / (h_in - h_adp)
        // Actually, from the E+ method:
        //   slope_condition_line = (h_in - h_adp) / (t_db_in - t_adp)
        //   SHR = cp_moist / slope  where cp_moist ≈ cp_air(w)
        //
        // Rearranging: slope = cp_moist / SHR
        // And: h_adp = h_in - slope × (t_db_in - t_adp)
        //
        // We find t_adp where h_sat(t_adp) == h_adp from the condition line.

        let shr = self.rated_shr.clamp(0.01, 0.999);
        let cp_moist = psych::cp_air_fn_w(w_rated);
        let slope = cp_moist / shr; // J/kg per °C

        // Search for ADP temperature: where h_sat(t) intersects the condition line
        // h_condition(t) = h_rated - slope × (t_db_rated - t)
        // We need: h_sat(t) = h_condition(t)
        //
        // Search range: from 0°C to the entering dew point
        let t_dp_entering = psych::tdp_fn_w_pb(w_rated, p_b);
        let t_low = -10.0_f64;
        let t_high = t_dp_entering.min(t_db_rated - 1.0);

        // Bisection search
        let mut lo = t_low;
        let mut hi = t_high;
        let mut t_adp = (lo + hi) / 2.0;

        for _ in 0..50 {
            t_adp = (lo + hi) / 2.0;
            let w_sat = psych::w_fn_tdb_rh_pb(t_adp, 1.0, p_b);
            let h_sat = psych::h_fn_tdb_w(t_adp, w_sat);
            let h_cond = h_rated - slope * (t_db_rated - t_adp);

            if (h_sat - h_cond).abs() < 0.1 {
                break;
            }
            if h_sat < h_cond {
                lo = t_adp;
            } else {
                hi = t_adp;
            }
        }

        let w_adp = psych::w_fn_tdb_rh_pb(t_adp, 1.0, p_b);
        let h_adp = psych::h_fn_tdb_w(t_adp, w_adp);

        // Bypass factor: fraction of air that "bypasses" the coil surface
        let dh = h_rated - h_adp;
        let bf = if dh.abs() > 1.0 {
            // BF from enthalpy: h_out = h_adp + BF × (h_in - h_adp)
            // At rated: h_out = h_in - Q_total / m_dot
            //         = h_in - rated_cap / (rated_airflow × rho × (1))
            // But we can compute BF directly from the SHR relationship.
            // For the rated condition, use the airflow to compute h_out:
            let rho_rated = psych::rho_air_fn_pb_tdb_w(p_b, t_db_rated, w_rated);
            let m_dot_rated = self.rated_airflow * rho_rated;
            if m_dot_rated > 0.001 {
                let h_out = h_rated - self.rated_capacity / m_dot_rated;
                ((h_out - h_adp) / dh).clamp(0.0, 0.9)
            } else {
                0.2 // Reasonable default
            }
        } else {
            0.2
        };

        self.adp_temp = t_adp;
        self.adp_w = w_adp;
        self.adp_h = h_adp;
        self.bypass_factor = bf;
        self.bf_initialized = true;
    }
}

impl AirComponent for CoolingCoilDX {
    fn name(&self) -> &str {
        &self.name
    }

    fn component_kind(&self) -> ComponentKind {
        ComponentKind::CoolingCoil
    }

    fn simulate_air(&mut self, inlet: &AirPort, ctx: &SimulationContext) -> AirPort {
        if inlet.mass_flow <= 0.0 {
            self.cooling_rate = 0.0;
            self.sensible_cooling_rate = 0.0;
            self.power_consumption = 0.0;
            self.plr = 0.0;
            self.rtf = 0.0;
            return *inlet;
        }

        let cp_air = psych::cp_air_fn_w(inlet.state.w);
        let t_outdoor = ctx.outdoor_air.t_db;

        // Calculate required sensible cooling to reach setpoint
        let q_sensible_required =
            inlet.mass_flow * cp_air * (inlet.state.t_db - self.outlet_temp_setpoint);

        // Only cool, don't heat
        if q_sensible_required <= 0.0 {
            self.cooling_rate = 0.0;
            self.sensible_cooling_rate = 0.0;
            self.power_consumption = 0.0;
            self.plr = 0.0;
            self.rtf = 0.0;
            return *inlet;
        }

        // Entering air wet-bulb temperature (for curve evaluation)
        let t_wb_inlet = psych::twb_fn_tdb_w_pb(inlet.state.t_db, inlet.state.w, inlet.state.p_b);

        // Flow fraction: actual mass flow / rated mass flow
        let flow_fraction = if self.rated_airflow > 0.001 {
            let rho = psych::rho_air_fn_pb_tdb_w(inlet.state.p_b, inlet.state.t_db, inlet.state.w);
            let rated_mass_flow = self.rated_airflow * rho;
            (inlet.mass_flow / rated_mass_flow).clamp(0.0, 1.5)
        } else {
            1.0
        };

        // Available capacity and COP at current conditions (with flow fraction)
        let available_cap = self.available_capacity(t_outdoor, t_wb_inlet, flow_fraction);
        let available_cop = self.available_cop(t_outdoor, t_wb_inlet, flow_fraction);

        // ── Compute SHR and split sensible/latent cooling ──
        let (q_sensible, q_total, outlet_t, outlet_w) = if self.autocalculate_shr {
            // Initialize bypass factor on first call (needs rated_capacity set)
            if !self.bf_initialized && self.rated_capacity > 0.0 {
                self.initialize_bypass_factor();
            }

            // E+ CalcDoe2DXCoil apparatus dew point method: the curve-modified
            // total capacity (Cap-fT × Cap-fFlow) fixes the full-load outlet
            // enthalpy, and the rated bypass factor locates the ADP on the
            // saturation curve from *current* entering conditions. The SHR
            // follows from the ADP — recomputed every timestep, so capacity
            // derates with outdoor temperature and the sensible/latent split
            // tracks the entering air state (dry inlets give a dry coil).
            //
            // Reference: EnergyPlus DXCoils.cc CalcDoe2DXCoil:
            //   hADP = h_in − (h_in − h_out_full) / (1 − BF)
            //   tADP = PsyTsatFnHPb(hADP)
            //   wADP = PsyWFnTdbH(tADP, hADP)
            //   SHR  = min((h(t_in, wADP) − hADP) / (h_in − hADP), 1)
            let h_in = psych::h_fn_tdb_w(inlet.state.t_db, inlet.state.w);
            let m = inlet.mass_flow;
            let h_out_full = h_in - available_cap / m;
            let bf = self.bypass_factor.clamp(0.0, 0.9);
            let h_adp = h_in - (h_in - h_out_full) / (1.0 - bf);
            let t_adp = psych::tsat_fn_h_pb(h_adp, inlet.state.p_b);
            let w_adp = psych::w_fn_tdb_h(t_adp, h_adp);

            // Dry coil when the ADP humidity ratio is at or above the
            // entering humidity ratio (no condensation possible).
            let shr = if w_adp >= inlet.state.w {
                1.0
            } else {
                let h_tin_wadp = psych::h_fn_tdb_w(inlet.state.t_db, w_adp);
                let dh = h_in - h_adp;
                if dh > 1.0 {
                    ((h_tin_wadp - h_adp) / dh).clamp(0.0, 1.0)
                } else {
                    1.0
                }
            };

            let q_sens_full = shr * available_cap;

            // PLR based on sensible load vs sensible capacity
            let plr = if q_sens_full > 0.0 {
                (q_sensible_required / q_sens_full).clamp(0.0, 1.0)
            } else {
                0.0
            };

            // Actual delivered quantities
            let qs = q_sens_full * plr;
            let qt = available_cap * plr;
            let dt = qs / (m * cp_air);
            let out_t = inlet.state.t_db - dt;

            // Full-load outlet humidity per E+: remove the latent portion of
            // the enthalpy change at the entering dry-bulb, then interpolate
            // toward it by PLR (time-averaged cycling outlet).
            let out_w = if shr >= 1.0 {
                inlet.state.w
            } else {
                let h_tin_wout = h_in - (1.0 - shr) * (h_in - h_out_full);
                let w_out_full = psych::w_fn_tdb_h(inlet.state.t_db, h_tin_wout);
                inlet.state.w - plr * (inlet.state.w - w_out_full)
            };

            (qs, qt, out_t, out_w.max(1.0e-5))
        } else {
            // Constant SHR mode: total = sensible / rated_SHR, with the
            // latent portion removed from the airstream. (Previously latent
            // was dropped entirely, under-counting compressor power by
            // ~1/SHR whenever dehumidification would occur.)
            let shr = self.rated_shr.clamp(0.2, 1.0);
            let sens_cap = available_cap * shr;
            let plr = (q_sensible_required / sens_cap).clamp(0.0, 1.0);
            let qs = sens_cap * plr;
            let qt = qs / shr;
            let dt = qs / (inlet.mass_flow * cp_air);
            const H_FG: f64 = 2_501_000.0;
            let out_w = (inlet.state.w - (qt - qs) / (inlet.mass_flow * H_FG)).max(1.0e-5);
            (qs, qt, inlet.state.t_db - dt, out_w)
        };

        // Electric power consumption.
        // Uses total cooling (sensible + latent) for power calculation.
        // Cycling losses (PLF) are applied at the system level in main.rs.
        let plr_power = if available_cap > 0.0 {
            (q_total / available_cap).clamp(0.0, 1.0)
        } else {
            0.0
        };
        self.power_consumption = if available_cop > 0.0 {
            available_cap * plr_power / available_cop
        } else {
            0.0
        };

        self.cooling_rate = q_total;
        self.sensible_cooling_rate = q_sensible;

        // Store PLR and RTF for detailed outputs
        self.plr = plr_power; // plr_power = q_total / available_cap
        let plf = if let Some(ref curve) = self.plf_curve {
            curve.evaluate_1d(self.plr)
        } else {
            1.0 - 0.15 * (1.0 - self.plr)
        };
        self.rtf = if plf > 0.0 { self.plr / plf } else { 0.0 };

        AirPort::new(
            psych::MoistAirState::new(outlet_t, outlet_w, inlet.state.p_b),
            inlet.mass_flow,
        )
    }

    fn has_water_side(&self) -> bool {
        false
    }

    fn set_water_inlet(&mut self, _inlet: &WaterPort) {}

    fn water_outlet(&self) -> Option<WaterPort> {
        None
    }

    fn design_air_flow_rate(&self) -> Option<f64> {
        if openbse_core::types::is_autosize(self.rated_airflow) {
            None
        } else {
            Some(self.rated_airflow)
        }
    }

    fn set_design_air_flow_rate(&mut self, flow: f64) {
        self.rated_airflow = flow;
    }

    fn set_setpoint(&mut self, setpoint: f64) {
        self.outlet_temp_setpoint = setpoint;
    }

    fn setpoint(&self) -> Option<f64> {
        Some(self.outlet_temp_setpoint)
    }

    fn nominal_capacity(&self) -> Option<f64> {
        Some(self.rated_capacity)
    }

    fn set_nominal_capacity(&mut self, cap: f64) {
        self.rated_capacity = cap;
    }

    fn power_consumption(&self) -> f64 {
        self.power_consumption
    }

    fn thermal_output(&self) -> f64 {
        // Negative = cooling (convention: positive = heating, negative = cooling)
        -self.cooling_rate
    }

    fn detailed_outputs(&self) -> std::collections::HashMap<String, f64> {
        let mut m = std::collections::HashMap::new();
        m.insert("sensible_load".to_string(), self.sensible_cooling_rate);
        m.insert(
            "latent_load".to_string(),
            self.cooling_rate - self.sensible_cooling_rate,
        );
        m.insert("total_load".to_string(), self.cooling_rate);
        m.insert(
            "cop_operating".to_string(),
            if self.power_consumption > 0.0 {
                self.cooling_rate / self.power_consumption
            } else {
                0.0
            },
        );
        m.insert("plr".to_string(), self.plr);
        m.insert("rtf".to_string(), self.rtf);
        let cycling_loss = if self.plr > 0.0 {
            self.cooling_rate * (1.0 - self.rtf / self.plr)
        } else {
            0.0
        };
        m.insert("cycling_loss".to_string(), cycling_loss);
        m
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_relative_eq;
    use openbse_core::types::{DayType, TimeStep};
    use openbse_psychrometrics::MoistAirState;

    fn make_ctx(t_outdoor: f64) -> SimulationContext {
        SimulationContext {
            timestep: TimeStep {
                month: 7,
                day: 15,
                hour: 14,
                sub_hour: 1,
                timesteps_per_hour: 1,
                sim_time_s: 0.0,
                dt: 3600.0,
            },
            outdoor_air: MoistAirState::from_tdb_rh(t_outdoor, 0.4, 101325.0),
            day_type: DayType::WeatherDay,
            is_sizing: false,
            sizing_internal_gains: SizingInternalGains::Full,
        }
    }

    #[test]
    fn test_dx_coil_cools_to_setpoint() {
        let mut coil = CoolingCoilDX::new("Test DX", 10000.0, 3.5, 0.8, 0.5, 13.0);
        let inlet_state = MoistAirState::from_tdb_rh(25.0, 0.5, 101325.0);
        let inlet = AirPort::new(inlet_state, 0.5);
        let ctx = make_ctx(35.0);

        let outlet = coil.simulate_air(&inlet, &ctx);

        // Should cool significantly
        assert!(outlet.state.t_db < 25.0);
        assert!(coil.cooling_rate > 0.0);
        assert!(coil.power_consumption > 0.0);
    }

    #[test]
    fn test_dx_coil_no_heating() {
        // If inlet is below setpoint, coil should not heat
        let mut coil = CoolingCoilDX::new("Test DX", 10000.0, 3.5, 0.8, 0.5, 13.0);
        let inlet_state = MoistAirState::from_tdb_rh(10.0, 0.5, 101325.0);
        let inlet = AirPort::new(inlet_state, 0.5);
        let ctx = make_ctx(35.0);

        let outlet = coil.simulate_air(&inlet, &ctx);

        assert_relative_eq!(outlet.state.t_db, 10.0, max_relative = 0.001);
        assert_eq!(coil.cooling_rate, 0.0);
        assert_eq!(coil.power_consumption, 0.0);
    }

    #[test]
    fn test_dx_coil_capacity_limited() {
        // Very small coil capacity
        let mut coil = CoolingCoilDX::new("Small DX", 1000.0, 3.5, 0.8, 0.5, 13.0);
        let inlet_state = MoistAirState::from_tdb_rh(35.0, 0.4, 101325.0);
        let inlet = AirPort::new(inlet_state, 1.0);
        let ctx = make_ctx(35.0);

        let outlet = coil.simulate_air(&inlet, &ctx);

        // Should NOT reach setpoint — capacity limited
        assert!(outlet.state.t_db > 13.0);
        assert!(outlet.state.t_db < 35.0);
    }

    #[test]
    fn test_dx_coil_hot_outdoor_derating() {
        let mut coil_normal = CoolingCoilDX::new("DX Normal", 10000.0, 3.5, 0.8, 0.5, 13.0);
        let mut coil_hot = CoolingCoilDX::new("DX Hot", 10000.0, 3.5, 0.8, 0.5, 13.0);

        let inlet_state = MoistAirState::from_tdb_rh(28.0, 0.4, 101325.0);
        let inlet = AirPort::new(inlet_state, 0.5);

        let ctx_normal = make_ctx(35.0);
        let ctx_hot = make_ctx(45.0);

        let out_normal = coil_normal.simulate_air(&inlet, &ctx_normal);
        let out_hot = coil_hot.simulate_air(&inlet, &ctx_hot);

        // At higher outdoor temp, COP is worse → more power for same cooling
        // Also capacity is reduced
        assert!(coil_hot.power_consumption > 0.0);
        // Hot outdoor should deliver less cooling (or same but less efficiently)
        assert!(out_hot.state.t_db >= out_normal.state.t_db - 0.1);
    }

    #[test]
    fn test_dx_coil_cop_calculation() {
        let mut coil = CoolingCoilDX::new("DX COP Test", 10000.0, 3.5, 0.8, 0.5, 13.0);
        let inlet_state = MoistAirState::from_tdb_rh(30.0, 0.4, 101325.0);
        let inlet = AirPort::new(inlet_state, 0.5);
        let ctx = make_ctx(35.0); // Rated conditions

        coil.simulate_air(&inlet, &ctx);

        // At rated conditions, effective COP should be close to rated
        if coil.power_consumption > 0.0 {
            let effective_cop = coil.cooling_rate / coil.power_consumption;
            // Within 20% of rated due to PLR effects
            assert!(
                effective_cop
                    > self::CoolingCoilDX::new("", 0.0, 3.5, 0.8, 0.0, 0.0).rated_cop * 0.7
            );
        }
    }

    #[test]
    fn test_autocalculate_shr_humid_air_dehumidifies() {
        // With humid entering air (w > 0.010 kg/kg), autocalculate_shr should
        // produce an outlet with lower humidity ratio than the inlet.
        let mut coil = CoolingCoilDX::new("DX Dehum", 10000.0, 3.5, 0.75, 0.5, 13.0)
            .with_autocalculate_shr(true);

        // 26°C, ~70% RH → w ≈ 0.015 kg/kg (above ADP of coil)
        let inlet_state = MoistAirState::from_tdb_rh(26.0, 0.70, 101325.0);
        let inlet = AirPort::new(inlet_state, 0.5);
        let ctx = make_ctx(35.0);

        let outlet = coil.simulate_air(&inlet, &ctx);

        // Outlet humidity should be lower (moisture removed by coil)
        assert!(
            outlet.state.w < inlet.state.w,
            "Expected outlet_w ({}) < inlet_w ({})",
            outlet.state.w,
            inlet.state.w
        );
        // Should also be cooling (lower temperature)
        assert!(outlet.state.t_db < inlet.state.t_db);
        assert!(
            coil.cooling_rate > coil.sensible_cooling_rate + 1.0,
            "Expected latent cooling"
        );
    }

    #[test]
    fn test_autocalculate_shr_dry_air_no_dehumidification() {
        // With dry entering air (dewpoint below ADP), the coil should be dry:
        // outlet humidity ratio equals inlet humidity ratio.
        let mut coil = CoolingCoilDX::new("DX Dry", 10000.0, 3.5, 0.75, 0.5, 13.0)
            .with_autocalculate_shr(true);

        // 26°C, 10% RH → very dry, dewpoint well below ADP (~5°C)
        let inlet_state = MoistAirState::from_tdb_rh(26.0, 0.10, 101325.0);
        let inlet = AirPort::new(inlet_state, 0.5);
        let ctx = make_ctx(35.0);

        let outlet = coil.simulate_air(&inlet, &ctx);

        // Dry coil: humidity ratio should pass through unchanged
        assert_relative_eq!(outlet.state.w, inlet.state.w, epsilon = 1e-6);
        // But temperature should still drop (sensible cooling)
        assert!(outlet.state.t_db < inlet.state.t_db);
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Multi-speed DX cooling coil
// ═══════════════════════════════════════════════════════════════════════════

/// One speed stage of a multi-speed DX coil.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DXSpeedStage {
    /// Total cooling capacity at rated conditions [W]
    pub rated_capacity: f64,
    /// COP at rated conditions
    pub rated_cop: f64,
    /// Sensible heat ratio at rated conditions [0-1]
    pub rated_shr: f64,
    /// Rated airflow [m³/s]
    pub rated_airflow: f64,
}

/// Multi-speed DX cooling coil.
///
/// Selects the lowest speed whose capacity meets the load. Operates at full
/// capacity of the selected speed; part-load cycling is applied at the
/// system level (same as single-speed). If load exceeds all speeds, runs
/// at the highest speed with PLR < 1 handled upstream.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoolingCoilDXMultiSpeed {
    pub name: String,
    #[serde(default = "default_submeter")]
    pub submeter: String,
    /// Speed stages ordered from lowest to highest capacity.
    pub speeds: Vec<DXSpeedStage>,
    /// Desired outlet air temperature setpoint [°C]
    pub outlet_temp_setpoint: f64,

    // ─── Runtime state ───────────────────────────────────────────────────
    #[serde(skip)]
    pub cooling_rate: f64,
    #[serde(skip)]
    pub sensible_cooling_rate: f64,
    #[serde(skip)]
    pub power: f64,
    /// Index of the active speed stage (0-based)
    #[serde(skip)]
    pub active_speed: usize,
}

impl CoolingCoilDXMultiSpeed {
    pub fn new(name: &str, speeds: Vec<DXSpeedStage>, setpoint: f64) -> Self {
        Self {
            name: name.to_string(),
            submeter: "General".to_string(),
            speeds,
            outlet_temp_setpoint: setpoint,
            cooling_rate: 0.0,
            sensible_cooling_rate: 0.0,
            power: 0.0,
            active_speed: 0,
        }
    }

    /// Run the single-speed physics for one stage, return outlet AirPort and
    /// (total_cooling_W, sensible_W, power_W).
    fn simulate_stage(
        stage: &DXSpeedStage,
        inlet: &AirPort,
        setpoint: f64,
        t_outdoor: f64,
    ) -> (AirPort, f64, f64, f64) {
        // Capacity derate with outdoor temperature (same defaults as single-speed)
        let cap_factor = (1.0 - 0.008 * (t_outdoor - 35.0)).clamp(0.5, 1.05);
        let capacity = stage.rated_capacity * cap_factor;
        let cp = psych::cp_air_fn_w(inlet.state.w);
        let load_sensible = (inlet.mass_flow * cp * (inlet.state.t_db - setpoint)).max(0.0);
        let sens_capacity = capacity * stage.rated_shr;
        // Sensible delivery limited to the setpoint load; total and latent
        // scale with the same PLR so the outlet state, reported total, and
        // power stay mutually consistent.
        let plr = if sens_capacity > 0.0 {
            (load_sensible / sens_capacity).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let sensible = sens_capacity * plr;
        let total = capacity * plr;
        let latent = total - sensible;
        let t_out = inlet.state.t_db - sensible / (inlet.mass_flow * cp).max(1e-6);
        const H_FG: f64 = 2_501_000.0;
        let w_out = (inlet.state.w - latent / (inlet.mass_flow * H_FG)).max(1.0e-5);
        let eir_factor = (1.0 + 0.012 * (t_outdoor - 35.0)).clamp(0.5, 1.10);
        let power = (capacity / stage.rated_cop) * eir_factor * plr;
        let outlet = AirPort::new(
            psych::MoistAirState::new(t_out, w_out, inlet.state.p_b),
            inlet.mass_flow,
        );
        (outlet, total, sensible, power)
    }
}

impl AirComponent for CoolingCoilDXMultiSpeed {
    fn name(&self) -> &str {
        &self.name
    }

    fn component_kind(&self) -> ComponentKind {
        ComponentKind::CoolingCoil
    }

    fn simulate_air(&mut self, inlet: &AirPort, ctx: &SimulationContext) -> AirPort {
        if inlet.mass_flow <= 0.0 || self.speeds.is_empty() {
            self.cooling_rate = 0.0;
            self.sensible_cooling_rate = 0.0;
            self.power = 0.0;
            return *inlet;
        }

        let t_outdoor = ctx.outdoor_air.t_db;
        let cp = psych::cp_air_fn_w(inlet.state.w);
        let load_needed = inlet.mass_flow * cp * (inlet.state.t_db - self.outlet_temp_setpoint);

        if load_needed <= 0.0 {
            self.cooling_rate = 0.0;
            self.sensible_cooling_rate = 0.0;
            self.power = 0.0;
            return *inlet;
        }

        // Select lowest speed that meets the sensible load; fall back to highest
        let selected = self
            .speeds
            .iter()
            .enumerate()
            .find(|(_, s)| {
                let cap_factor = (1.0 - 0.008 * (t_outdoor - 35.0)).clamp(0.5, 1.05);
                s.rated_capacity * cap_factor * s.rated_shr >= load_needed
            })
            .map(|(i, _)| i)
            .unwrap_or(self.speeds.len() - 1);

        self.active_speed = selected;
        let stage = &self.speeds[selected];
        let (outlet, total, sensible, power) =
            Self::simulate_stage(stage, inlet, self.outlet_temp_setpoint, t_outdoor);

        self.cooling_rate = total;
        self.sensible_cooling_rate = sensible;
        self.power = power;
        outlet
    }

    fn set_setpoint(&mut self, setpoint: f64) {
        self.outlet_temp_setpoint = setpoint;
    }

    fn power_consumption(&self) -> f64 {
        self.power
    }
}

#[cfg(test)]
mod multispeed_tests {
    use super::*;
    use approx::assert_relative_eq;
    use openbse_core::ports::SizingInternalGains;
    use openbse_core::types::{DayType, TimeStep};
    use openbse_psychrometrics::MoistAirState;

    fn make_ctx(t_outdoor: f64) -> SimulationContext {
        SimulationContext {
            timestep: TimeStep {
                month: 7,
                day: 15,
                hour: 14,
                sub_hour: 1,
                timesteps_per_hour: 1,
                sim_time_s: 0.0,
                dt: 3600.0,
            },
            outdoor_air: MoistAirState::from_tdb_rh(t_outdoor, 0.30, 101325.0),
            day_type: DayType::WeatherDay,
            is_sizing: false,
            sizing_internal_gains: SizingInternalGains::Full,
        }
    }

    fn make_coil() -> CoolingCoilDXMultiSpeed {
        CoolingCoilDXMultiSpeed::new(
            "MS DX",
            vec![
                DXSpeedStage {
                    rated_capacity: 5_000.0,
                    rated_cop: 4.0,
                    rated_shr: 0.75,
                    rated_airflow: 0.2,
                },
                DXSpeedStage {
                    rated_capacity: 10_000.0,
                    rated_cop: 3.5,
                    rated_shr: 0.75,
                    rated_airflow: 0.4,
                },
            ],
            13.0,
        )
    }

    #[test]
    fn test_low_load_uses_speed1() {
        let mut coil = make_coil();
        // Small mass flow → small load → speed 1 sufficient
        let inlet = AirPort::new(MoistAirState::from_tdb_rh(20.0, 0.5, 101325.0), 0.1);
        let ctx = make_ctx(35.0);
        let _out = coil.simulate_air(&inlet, &ctx);
        assert_eq!(
            coil.active_speed, 0,
            "Low load should use speed 1 (index 0)"
        );
        assert!(coil.cooling_rate > 0.0);
        assert!(coil.power > 0.0);
    }

    #[test]
    fn test_high_load_uses_speed2() {
        let mut coil = make_coil();
        // Large mass flow + warm inlet → load exceeds speed 1 → speed 2
        let inlet = AirPort::new(MoistAirState::from_tdb_rh(30.0, 0.5, 101325.0), 1.0);
        let ctx = make_ctx(35.0);
        let _out = coil.simulate_air(&inlet, &ctx);
        assert_eq!(
            coil.active_speed, 1,
            "High load should use speed 2 (index 1)"
        );
        assert!(
            coil.cooling_rate > 5_000.0,
            "Should deliver more than speed-1 capacity"
        );
    }

    #[test]
    fn test_no_cooling_above_setpoint() {
        let mut coil = make_coil();
        // Inlet already at setpoint — no cooling needed
        let inlet = AirPort::new(MoistAirState::from_tdb_rh(13.0, 0.5, 101325.0), 0.5);
        let ctx = make_ctx(35.0);
        let out = coil.simulate_air(&inlet, &ctx);
        assert_eq!(coil.cooling_rate, 0.0);
        assert_eq!(coil.power, 0.0);
        assert_relative_eq!(out.state.t_db, inlet.state.t_db, epsilon = 0.01);
    }

    #[test]
    fn test_zero_flow_no_operation() {
        let mut coil = make_coil();
        let inlet = AirPort::new(MoistAirState::from_tdb_rh(25.0, 0.5, 101325.0), 0.0);
        let ctx = make_ctx(35.0);
        coil.simulate_air(&inlet, &ctx);
        assert_eq!(coil.power, 0.0);
    }
}
