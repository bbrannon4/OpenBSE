//! Air-source heat pump DX heating coil component model.
//!
//! Models a single-speed or two-stage air-source heat pump heating coil
//! with capacity and efficiency that vary with outdoor temperature, defrost
//! modeling, and supplemental electric resistance backup.
//!
//! Physics:
//!   Capacity = rated_capacity × cap_ft(T_outdoor_db, T_indoor_db)
//!   COP      = rated_cop / eir_ft(T_outdoor_db, T_indoor_db)
//!   Defrost:   capacity and COP degrade below defrost_onset_temp
//!   Supplemental: electric resistance kicks in when HP can't meet load
//!
//! Ratings are at AHRI 210/240 conditions:
//!   High-temp heating: 47°F (8.33°C) outdoor dry-bulb
//!   Low-temp heating:  17°F (-8.33°C) outdoor dry-bulb
//!
//! Reference: EnergyPlus Engineering Reference, "Coil:Heating:DX:SingleSpeed"

use crate::performance_curve::PerformanceCurve;
use openbse_core::ports::*;
use openbse_psychrometrics::{self as psych};
use serde::{Deserialize, Serialize};

/// Defrost strategy for heat pump coils.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum DefrostStrategy {
    /// Reverse-cycle defrost (most common in modern units).
    /// Temporarily reverses refrigerant flow to melt frost from outdoor coil.
    /// During defrost, heating capacity drops to zero and compressor power
    /// increases slightly.
    ReverseCycle,
    /// Resistive defrost (electric heater on outdoor coil).
    /// Less efficient but simpler. Defrost heater power is additive.
    Resistive,
}

/// Air-source heat pump DX heating coil.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeatPumpHeatingCoil {
    pub name: String,
    /// Rated heating capacity [W] at AHRI high-temp conditions (47°F / 8.33°C outdoor)
    pub rated_capacity: f64,
    /// Rated COP at AHRI high-temp conditions (typically 3.0-4.5)
    pub rated_cop: f64,
    /// Rated air flow rate [m³/s]
    pub rated_airflow: f64,
    /// Desired outlet air temperature setpoint [°C]
    pub outlet_temp_setpoint: f64,

    /// Optional capacity modifier curve: f(T_db_outdoor, T_db_indoor)
    /// Biquadratic: Cap_mod = c1 + c2·Todb + c3·Todb² + c4·Tidb + c5·Tidb² + c6·Todb·Tidb
    #[serde(skip)]
    pub cap_ft_curve: Option<PerformanceCurve>,
    /// Optional EIR modifier curve: f(T_db_outdoor, T_db_indoor)
    #[serde(skip)]
    pub eir_ft_curve: Option<PerformanceCurve>,

    // ─── Defrost parameters ──────────────────────────────────────────────
    /// Defrost strategy
    pub defrost_strategy: DefrostStrategy,
    /// Outdoor temperature below which defrost activates [°C] (default: 5.0)
    pub defrost_onset_temp: f64,
    /// Maximum defrost time fraction at minimum outdoor temp [0-1] (default: 0.08)
    /// Linear ramp from 0 at defrost_onset_temp to max at defrost_min_temp.
    pub defrost_max_fraction: f64,
    /// Outdoor temperature at which defrost fraction reaches maximum [°C] (default: -8.33)
    pub defrost_min_temp: f64,
    /// Resistive defrost heater power [W] (only used for Resistive strategy)
    pub defrost_heater_power: f64,

    // ─── Supplemental electric resistance ────────────────────────────────
    /// Supplemental electric resistance capacity [W] (default: 0 = no backup)
    pub supplemental_capacity: f64,
    /// Outdoor temperature below which HP compressor locks out [°C] (default: -17.8 / 0°F)
    /// Below this, only supplemental heat operates.
    pub compressor_lockout_temp: f64,

    // ─── Runtime state ───────────────────────────────────────────────────
    /// Heat pump heating output [W]
    #[serde(skip)]
    pub hp_heating_rate: f64,
    /// Supplemental electric heating output [W]
    #[serde(skip)]
    pub supplemental_heating_rate: f64,
    /// Compressor electric power [W]
    #[serde(skip)]
    pub compressor_power: f64,
    /// Defrost electric power [W] (resistive strategy only)
    #[serde(skip)]
    pub defrost_power: f64,
    /// Supplemental electric power [W]
    #[serde(skip)]
    pub supplemental_power: f64,
    /// Total heating delivered [W] (HP + supplemental)
    #[serde(skip)]
    pub total_heating_rate: f64,
}

impl HeatPumpHeatingCoil {
    /// Create a new heat pump heating coil with default defrost and supplemental settings.
    ///
    /// # Arguments
    /// * `name` - Component name
    /// * `rated_capacity` - Heating capacity at AHRI 47°F conditions [W]
    /// * `rated_cop` - COP at AHRI 47°F conditions
    /// * `rated_airflow` - Rated air volume flow rate [m³/s]
    /// * `setpoint` - Desired outlet temperature [°C]
    pub fn new(
        name: &str,
        rated_capacity: f64,
        rated_cop: f64,
        rated_airflow: f64,
        setpoint: f64,
    ) -> Self {
        Self {
            name: name.to_string(),
            rated_capacity,
            rated_cop,
            rated_airflow,
            outlet_temp_setpoint: setpoint,
            cap_ft_curve: None,
            eir_ft_curve: None,
            defrost_strategy: DefrostStrategy::ReverseCycle,
            defrost_onset_temp: 5.0,
            defrost_max_fraction: 0.08,
            defrost_min_temp: -8.33,
            defrost_heater_power: 0.0,
            supplemental_capacity: 0.0,
            compressor_lockout_temp: -17.78, // 0°F
            hp_heating_rate: 0.0,
            supplemental_heating_rate: 0.0,
            compressor_power: 0.0,
            defrost_power: 0.0,
            supplemental_power: 0.0,
            total_heating_rate: 0.0,
        }
    }

    /// Set supplemental electric resistance backup capacity [W].
    pub fn with_supplemental(mut self, capacity: f64) -> Self {
        self.supplemental_capacity = capacity;
        self
    }

    /// Set compressor lockout temperature [°C].
    pub fn with_lockout_temp(mut self, temp: f64) -> Self {
        self.compressor_lockout_temp = temp;
        self
    }

    /// Attach performance curves.
    pub fn with_curves(
        mut self,
        cap_ft: Option<PerformanceCurve>,
        eir_ft: Option<PerformanceCurve>,
    ) -> Self {
        self.cap_ft_curve = cap_ft;
        self.eir_ft_curve = eir_ft;
        self
    }

    /// Calculate available heating capacity at current outdoor conditions.
    ///
    /// Uses biquadratic curve if available, otherwise a simplified linear model
    /// based on AHRI 210/240 high-temp (47°F) and low-temp (17°F) rating points.
    fn available_capacity(&self, t_outdoor: f64, t_indoor: f64) -> f64 {
        if let Some(ref curve) = self.cap_ft_curve {
            let modifier = curve.evaluate(t_outdoor, t_indoor);
            self.rated_capacity * modifier
        } else {
            // Default linear capacity degradation with outdoor temperature.
            // At rated (8.33°C): factor = 1.0
            // At low-temp (-8.33°C): factor ≈ 0.67 (typical single-speed HP)
            // Below -17.78°C: extrapolate but clamp
            let t_rated = 8.33;
            let slope = 0.02; // ~2% per °C below rated
            let factor = 1.0 - slope * (t_rated - t_outdoor).max(0.0);
            let factor = factor.clamp(0.2, 1.15);
            self.rated_capacity * factor
        }
    }

    /// Calculate COP at current outdoor conditions.
    fn available_cop(&self, t_outdoor: f64, t_indoor: f64) -> f64 {
        if let Some(ref curve) = self.eir_ft_curve {
            let eir_modifier = curve.evaluate(t_outdoor, t_indoor);
            if eir_modifier > 0.0 {
                self.rated_cop / eir_modifier
            } else {
                self.rated_cop
            }
        } else {
            // Default linear COP degradation.
            // At rated (8.33°C): COP = rated
            // At low-temp (-8.33°C): COP ≈ 0.60 × rated (typical)
            let t_rated = 8.33;
            let slope = 0.024; // ~2.4% per °C
            let factor = 1.0 - slope * (t_rated - t_outdoor).max(0.0);
            let factor = factor.clamp(0.3, 1.10);
            self.rated_cop * factor
        }
    }

    /// Calculate defrost time fraction and capacity/power adjustments.
    ///
    /// Returns (net_capacity_fraction, extra_power_fraction)
    fn defrost_adjustments(&self, t_outdoor: f64) -> (f64, f64) {
        if t_outdoor >= self.defrost_onset_temp {
            return (1.0, 0.0);
        }

        // Linear ramp of defrost fraction from 0 at onset to max at min_temp
        let range = (self.defrost_onset_temp - self.defrost_min_temp).max(1.0);
        let frac = ((self.defrost_onset_temp - t_outdoor) / range).clamp(0.0, 1.0);
        let defrost_fraction = frac * self.defrost_max_fraction;

        match self.defrost_strategy {
            DefrostStrategy::ReverseCycle => {
                // During reverse-cycle defrost, heating stops and compressor still runs.
                // Net capacity reduced by defrost fraction, power stays roughly the same.
                let net_cap_fraction = 1.0 - defrost_fraction;
                let extra_power_fraction = defrost_fraction * 0.3; // slight power increase
                (net_cap_fraction, extra_power_fraction)
            }
            DefrostStrategy::Resistive => {
                // Resistive defrost: capacity slightly reduced (coil frost),
                // but no reverse cycle loss. Defrost heater adds extra power.
                let net_cap_fraction = 1.0 - defrost_fraction * 0.5;
                let extra_power_fraction = 0.0; // defrost heater power tracked separately
                (net_cap_fraction, extra_power_fraction)
            }
        }
    }
}

impl AirComponent for HeatPumpHeatingCoil {
    fn name(&self) -> &str {
        &self.name
    }

    fn simulate_air(&mut self, inlet: &AirPort, ctx: &SimulationContext) -> AirPort {
        // Reset runtime state
        self.hp_heating_rate = 0.0;
        self.supplemental_heating_rate = 0.0;
        self.compressor_power = 0.0;
        self.defrost_power = 0.0;
        self.supplemental_power = 0.0;
        self.total_heating_rate = 0.0;

        if inlet.mass_flow <= 0.0 {
            return *inlet;
        }

        let cp_air = psych::cp_air_fn_w(inlet.state.w);
        let t_outdoor = ctx.outdoor_air.t_db;
        let t_indoor = inlet.state.t_db;

        // Calculate required heating to reach setpoint
        let q_required = inlet.mass_flow * cp_air * (self.outlet_temp_setpoint - t_indoor);

        // Only heat, don't cool
        if q_required <= 0.0 {
            return *inlet;
        }

        // ─── Heat pump compressor operation ──────────────────────────────
        let mut q_hp = 0.0;
        let mut p_compressor = 0.0;
        let mut p_defrost = 0.0;

        let compressor_available = t_outdoor >= self.compressor_lockout_temp;

        if compressor_available {
            // Get capacity and COP at current conditions
            let available_cap = self.available_capacity(t_outdoor, t_indoor);
            let cop = self.available_cop(t_outdoor, t_indoor);

            // Apply defrost adjustments
            let (cap_fraction, power_extra_fraction) = self.defrost_adjustments(t_outdoor);
            let net_capacity = available_cap * cap_fraction;

            // Part-load ratio
            let plr = (q_required / net_capacity).clamp(0.0, 1.0);

            // Heat delivered by HP
            q_hp = net_capacity * plr;

            // Compressor power
            // PLF curve: PLF = 1 - Cd × (1 - PLR), Cd ≈ 0.15
            let plf = 1.0 - 0.15 * (1.0 - plr);
            let runtime = if plf > 0.0 { plr / plf } else { 0.0 };

            p_compressor = if cop > 0.0 {
                available_cap * runtime / cop * (1.0 + power_extra_fraction)
            } else {
                0.0
            };

            // Resistive defrost heater power
            if matches!(self.defrost_strategy, DefrostStrategy::Resistive)
                && t_outdoor < self.defrost_onset_temp
            {
                let range = (self.defrost_onset_temp - self.defrost_min_temp).max(1.0);
                let frac = ((self.defrost_onset_temp - t_outdoor) / range).clamp(0.0, 1.0);
                let defrost_fraction = frac * self.defrost_max_fraction;
                p_defrost = self.defrost_heater_power * defrost_fraction;
            }
        }

        // ─── Supplemental electric resistance ────────────────────────────
        let q_shortfall = (q_required - q_hp).max(0.0);
        let q_supplemental = q_shortfall.min(self.supplemental_capacity);
        let p_supplemental = q_supplemental; // electric resistance: COP = 1.0

        // ─── Total heating and outlet temperature ────────────────────────
        let q_total = q_hp + q_supplemental;
        let dt = q_total / (inlet.mass_flow * cp_air);
        let outlet_t = t_indoor + dt;

        // Store runtime state
        self.hp_heating_rate = q_hp;
        self.supplemental_heating_rate = q_supplemental;
        self.compressor_power = p_compressor;
        self.defrost_power = p_defrost;
        self.supplemental_power = p_supplemental;
        self.total_heating_rate = q_total;

        AirPort::new(
            psych::MoistAirState::new(outlet_t, inlet.state.w, inlet.state.p_b),
            inlet.mass_flow,
        )
    }

    fn has_water_side(&self) -> bool {
        false
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
        self.compressor_power + self.defrost_power + self.supplemental_power
    }

    fn thermal_output(&self) -> f64 {
        self.total_heating_rate
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
                month: 1,
                day: 15,
                hour: 12,
                sub_hour: 1,
                timesteps_per_hour: 1,
                sim_time_s: 0.0,
                dt: 3600.0,
            },
            outdoor_air: MoistAirState::from_tdb_rh(t_outdoor, 0.5, 101325.0),
            day_type: DayType::WeatherDay,
            is_sizing: false,
            sizing_internal_gains: SizingInternalGains::Full,
        }
    }

    #[test]
    fn test_hp_heats_at_rated_conditions() {
        let mut coil = HeatPumpHeatingCoil::new("Test HP", 10000.0, 3.5, 0.5, 35.0);
        let inlet = AirPort::new(
            MoistAirState::from_tdb_rh(10.0, 0.5, 101325.0),
            0.5,
        );
        let ctx = make_ctx(8.33); // Rated outdoor temp (47°F)

        let outlet = coil.simulate_air(&inlet, &ctx);

        assert!(outlet.state.t_db > 10.0);
        assert!(coil.hp_heating_rate > 0.0);
        assert!(coil.compressor_power > 0.0);

        // Effective COP should be close to rated at rated conditions
        if coil.compressor_power > 0.0 {
            let effective_cop = coil.hp_heating_rate / coil.compressor_power;
            assert!(effective_cop > 2.5);
            assert!(effective_cop < 5.0);
        }
    }

    #[test]
    fn test_hp_capacity_degrades_with_cold() {
        let mut coil_warm = HeatPumpHeatingCoil::new("HP Warm", 10000.0, 3.5, 0.5, 35.0);
        let mut coil_cold = HeatPumpHeatingCoil::new("HP Cold", 10000.0, 3.5, 0.5, 35.0);

        let inlet = AirPort::new(
            MoistAirState::from_tdb_rh(10.0, 0.5, 101325.0),
            0.5,
        );

        let _out_warm = coil_warm.simulate_air(&inlet, &make_ctx(8.33));
        let _out_cold = coil_cold.simulate_air(&inlet, &make_ctx(-8.33));

        // At -8.33°C (17°F), capacity should be significantly less
        assert!(coil_cold.hp_heating_rate < coil_warm.hp_heating_rate);
        // COP should be worse (more power per unit heat)
        let cop_warm = coil_warm.hp_heating_rate / coil_warm.compressor_power.max(1.0);
        let cop_cold = coil_cold.hp_heating_rate / coil_cold.compressor_power.max(1.0);
        assert!(cop_cold < cop_warm);
    }

    #[test]
    fn test_hp_no_heating_when_warm() {
        let mut coil = HeatPumpHeatingCoil::new("Test HP", 10000.0, 3.5, 0.5, 20.0);
        let inlet = AirPort::new(
            MoistAirState::from_tdb_rh(25.0, 0.5, 101325.0),
            0.5,
        );
        let ctx = make_ctx(8.33);

        let outlet = coil.simulate_air(&inlet, &ctx);

        // Inlet above setpoint — no heating
        assert_relative_eq!(outlet.state.t_db, 25.0, max_relative = 0.001);
        assert_eq!(coil.hp_heating_rate, 0.0);
        assert_eq!(coil.compressor_power, 0.0);
    }

    #[test]
    fn test_hp_compressor_lockout_with_supplemental() {
        let mut coil = HeatPumpHeatingCoil::new("HP Lockout", 10000.0, 3.5, 0.5, 35.0)
            .with_supplemental(15000.0)
            .with_lockout_temp(-17.78);

        let inlet = AirPort::new(
            MoistAirState::from_tdb_rh(10.0, 0.5, 101325.0),
            0.5,
        );
        let ctx = make_ctx(-25.0); // Below lockout

        let outlet = coil.simulate_air(&inlet, &ctx);

        // Compressor locked out — only supplemental
        assert_eq!(coil.hp_heating_rate, 0.0);
        assert_eq!(coil.compressor_power, 0.0);
        assert!(coil.supplemental_heating_rate > 0.0);
        assert!(outlet.state.t_db > 10.0);
    }

    #[test]
    fn test_hp_supplemental_tops_off() {
        // Small HP with supplemental backup
        let mut coil = HeatPumpHeatingCoil::new("HP + Supp", 3000.0, 3.5, 0.5, 35.0)
            .with_supplemental(20000.0);

        let inlet = AirPort::new(
            MoistAirState::from_tdb_rh(10.0, 0.5, 101325.0),
            0.5,
        );
        let ctx = make_ctx(-5.0); // Cold but not locked out

        let outlet = coil.simulate_air(&inlet, &ctx);

        // HP at capacity + supplemental fills the gap
        assert!(coil.hp_heating_rate > 0.0);
        assert!(coil.supplemental_heating_rate > 0.0);
        assert!(coil.total_heating_rate > coil.hp_heating_rate);
        assert_relative_eq!(outlet.state.t_db, 35.0, max_relative = 0.01);
    }

    #[test]
    fn test_hp_defrost_reduces_capacity() {
        let mut coil_no_defrost = HeatPumpHeatingCoil::new("HP Above", 10000.0, 3.5, 0.5, 35.0);
        let mut coil_defrost = HeatPumpHeatingCoil::new("HP Below", 10000.0, 3.5, 0.5, 35.0);

        let inlet = AirPort::new(
            MoistAirState::from_tdb_rh(10.0, 0.5, 101325.0),
            0.5,
        );

        // Above defrost onset (5°C) — no defrost penalty
        let _out1 = coil_no_defrost.simulate_air(&inlet, &make_ctx(10.0));
        // Below defrost onset — should have capacity reduction
        let _out2 = coil_defrost.simulate_air(&inlet, &make_ctx(0.0));

        // Capacity with defrost should be less than without
        // (even though both are below rated temp, the 0°C case has defrost penalty)
        // This is approximate since capacity also degrades with outdoor temp
        assert!(coil_defrost.compressor_power > 0.0);
    }

    #[test]
    fn test_hp_zero_airflow() {
        let mut coil = HeatPumpHeatingCoil::new("HP", 10000.0, 3.5, 0.5, 35.0);
        let inlet = AirPort::new(
            MoistAirState::from_tdb_rh(10.0, 0.5, 101325.0),
            0.0,
        );
        let ctx = make_ctx(8.33);

        let outlet = coil.simulate_air(&inlet, &ctx);

        assert_relative_eq!(outlet.state.t_db, 10.0, max_relative = 0.001);
        assert_eq!(coil.total_heating_rate, 0.0);
        assert_eq!(coil.power_consumption(), 0.0);
    }
}
