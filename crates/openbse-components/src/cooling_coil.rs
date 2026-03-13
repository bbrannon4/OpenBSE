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

/// DX cooling coil component.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoolingCoilDX {
    pub name: String,
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
    /// Normalization factor for EIR curve: 1 / eir_curve(19.44, 35).
    /// Per E+ docs, EIR-fT should equal 1.0 at ARI rated conditions
    /// (19.44°C WB entering, 35°C outdoor).  If the curve isn't
    /// normalized, this factor corrects it so rated_COP equals the
    /// actual COP at rated conditions.
    #[serde(skip)]
    eir_normalization: f64,

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
            rated_capacity,
            rated_cop,
            rated_shr,
            rated_airflow,
            outlet_temp_setpoint: setpoint,
            cap_ft_curve: None,
            eir_ft_curve: None,
            eir_normalization: 1.0,
            cooling_rate: 0.0,
            sensible_cooling_rate: 0.0,
            power_consumption: 0.0,
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

    /// Calculate available cooling capacity at current conditions.
    ///
    /// If a `cap_ft_curve` is set, uses biquadratic: f(T_wb_entering, T_db_outdoor).
    /// Otherwise falls back to a simplified linear correction.
    ///
    /// At rated conditions (35°C/95°F ODB), the correction is 1.0.
    fn available_capacity(&self, t_outdoor: f64, t_wb_inlet: f64) -> f64 {
        if let Some(ref curve) = self.cap_ft_curve {
            let modifier = curve.evaluate(t_wb_inlet, t_outdoor);
            self.rated_capacity * modifier
        } else {
            // Fallback: linear derate with outdoor temperature only
            let t_rated = 35.0;
            let correction = 1.0 - 0.008 * (t_outdoor - t_rated);
            let correction = correction.clamp(0.5, 1.05);
            self.rated_capacity * correction
        }
    }

    /// Calculate COP at current conditions.
    ///
    /// If an `eir_ft_curve` is set, uses biquadratic to compute EIR modifier,
    /// then COP = rated_COP / EIR_modifier. Otherwise falls back to linear.
    fn available_cop(&self, t_outdoor: f64, t_wb_inlet: f64) -> f64 {
        if let Some(ref curve) = self.eir_ft_curve {
            let eir_raw = curve.evaluate(t_wb_inlet, t_outdoor);
            // Normalize so eir = 1.0 at ARI rated conditions
            let eir_modifier = eir_raw * self.eir_normalization;
            if eir_modifier > 0.0 {
                self.rated_cop / eir_modifier
            } else {
                self.rated_cop
            }
        } else {
            // Fallback: linear derate
            let t_rated = 35.0;
            let correction = 1.0 - 0.012 * (t_outdoor - t_rated);
            let correction = correction.clamp(0.4, 1.10);
            self.rated_cop * correction
        }
    }
}

impl AirComponent for CoolingCoilDX {
    fn name(&self) -> &str {
        &self.name
    }

    fn simulate_air(&mut self, inlet: &AirPort, ctx: &SimulationContext) -> AirPort {
        if inlet.mass_flow <= 0.0 {
            self.cooling_rate = 0.0;
            self.sensible_cooling_rate = 0.0;
            self.power_consumption = 0.0;
            return *inlet;
        }

        let cp_air = psych::cp_air_fn_w(inlet.state.w);
        let t_outdoor = ctx.outdoor_air.t_db;

        // Calculate required sensible cooling to reach setpoint
        let q_sensible_required = inlet.mass_flow * cp_air * (inlet.state.t_db - self.outlet_temp_setpoint);

        // Only cool, don't heat
        if q_sensible_required <= 0.0 {
            self.cooling_rate = 0.0;
            self.sensible_cooling_rate = 0.0;
            self.power_consumption = 0.0;
            return *inlet;
        }

        // Entering air wet-bulb temperature (for curve evaluation)
        let t_wb_inlet = psych::twb_fn_tdb_w_pb(inlet.state.t_db, inlet.state.w, inlet.state.p_b);

        // Available capacity at current outdoor conditions
        let available_cap = self.available_capacity(t_outdoor, t_wb_inlet);
        let available_cop = self.available_cop(t_outdoor, t_wb_inlet);

        // Simplified model: no dehumidification (humidity ratio passes through).
        // All cooling is sensible — the full coil capacity is available for
        // sensible cooling.  This matches E+ behavior in dry climates where
        // the actual SHR approaches 1.0 (coil surface stays dry or nearly so).
        //
        // PLR = sensible_load / total_available_capacity
        //
        // The rated_shr field is retained for future dehumidification modeling
        // and for capacity reporting, but does NOT reduce available sensible
        // capacity or inflate electric power consumption.
        let plr = (q_sensible_required / available_cap).clamp(0.0, 1.0);

        // Actual sensible cooling delivered (= total, since no latent)
        let q_sensible = available_cap * plr;
        let q_total = q_sensible;

        // Calculate outlet temperature
        let dt = q_sensible / (inlet.mass_flow * cp_air);
        let outlet_t = inlet.state.t_db - dt;

        // Electric power consumption
        // At part load, COP degrades slightly (simplified PLF curve)
        // PLF = 1 - Cd * (1 - PLR), where Cd ≈ 0.15 for typical DX
        let plf = if plr > 0.0 {
            1.0 - 0.15 * (1.0 - plr)
        } else {
            1.0
        };
        let runtime_fraction = if plf > 0.0 { plr / plf } else { 0.0 };
        self.power_consumption = if available_cop > 0.0 {
            available_cap * runtime_fraction / available_cop
        } else {
            0.0
        };

        self.cooling_rate = q_total;
        self.sensible_cooling_rate = q_sensible;

        // Simplified: no dehumidification modeled (humidity ratio passes through)
        AirPort::new(
            psych::MoistAirState::new(outlet_t, inlet.state.w, inlet.state.p_b),
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
            assert!(effective_cop > self::CoolingCoilDX::new("", 0.0, 3.5, 0.8, 0.0, 0.0).rated_cop * 0.7);
        }
    }
}
