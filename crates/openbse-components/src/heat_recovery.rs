//! Air-to-air heat recovery component.
//!
//! Models rotary enthalpy wheels and plate heat exchangers for
//! energy recovery from exhaust air in DOAS systems.
//!
//! Physics: sensible effectiveness approach
//!   Q_sensible = e_s * C_min * (T_exhaust - T_outdoor)
//!   Q_latent   = e_l * m_min * h_fg * (W_exhaust - W_outdoor)
//!
//! Reference: ASHRAE Handbook of Fundamentals, Chapter 26

use openbse_core::ports::*;
use openbse_psychrometrics::{self as psych};
use serde::{Deserialize, Serialize};

/// Heat recovery device type.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum HeatRecoveryType {
    /// Rotary enthalpy wheel -- recovers both sensible and latent
    EnthalpyWheel,
    /// Plate heat exchanger -- sensible only
    PlateHX,
}

/// Air-to-air heat recovery component.
///
/// Transfers heat (and optionally moisture) between an exhaust air stream
/// and the incoming outdoor air stream.  Placed upstream of coils in a
/// DOAS to reduce the heating/cooling load on the primary plant.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeatRecovery {
    pub name: String,
    pub hr_type: HeatRecoveryType,
    /// Sensible effectiveness at 100% airflow [0-1] (typical 0.70-0.85)
    pub sensible_effectiveness: f64,
    /// Latent effectiveness at 100% airflow [0-1] (typical 0.60-0.75, 0 for PlateHX)
    pub latent_effectiveness: f64,
    /// Exhaust air temperature [C] -- represents the building exhaust air stream.
    /// If not connected to actual exhaust, uses a default of 22 C (typical indoor).
    pub exhaust_air_temp: f64,
    /// Exhaust air humidity ratio [kg/kg]
    pub exhaust_air_w: f64,
    /// Parasitic electric power [W] (wheel motor, etc.)
    pub parasitic_power: f64,

    // ─── Runtime state (not serialized) ─────────────────────────────────
    /// Sensible heat recovery rate [W] (positive = heating supply air)
    #[serde(skip)]
    pub sensible_recovery: f64,
    /// Latent heat recovery rate [W] (positive = adding moisture to supply air)
    #[serde(skip)]
    pub latent_recovery: f64,
    /// Electric power consumption [W]
    #[serde(skip)]
    pub electric_power: f64,
}

/// Deadband half-width [C].  When the outdoor air temperature is within
/// this band of the exhaust air temperature the wheel is bypassed because
/// running it would provide negligible benefit (or could even hurt by
/// transferring energy in the wrong direction at the margins).
const BYPASS_DEADBAND: f64 = 1.0;

/// Heat of vaporization of water at ~20 C [J/kg].
const HFG: f64 = 2.454e6;

impl HeatRecovery {
    /// Create a new enthalpy wheel heat recovery unit.
    pub fn enthalpy_wheel(
        name: &str,
        sensible_eff: f64,
        latent_eff: f64,
        parasitic_power: f64,
    ) -> Self {
        Self {
            name: name.to_string(),
            hr_type: HeatRecoveryType::EnthalpyWheel,
            sensible_effectiveness: sensible_eff,
            latent_effectiveness: latent_eff,
            exhaust_air_temp: 22.0,
            exhaust_air_w: 0.008,
            parasitic_power,
            sensible_recovery: 0.0,
            latent_recovery: 0.0,
            electric_power: 0.0,
        }
    }

    /// Create a new plate heat exchanger (sensible-only) heat recovery unit.
    pub fn plate_hx(name: &str, sensible_eff: f64, parasitic_power: f64) -> Self {
        Self {
            name: name.to_string(),
            hr_type: HeatRecoveryType::PlateHX,
            sensible_effectiveness: sensible_eff,
            latent_effectiveness: 0.0, // plate HX has no latent recovery
            exhaust_air_temp: 22.0,
            exhaust_air_w: 0.008,
            parasitic_power,
            sensible_recovery: 0.0,
            latent_recovery: 0.0,
            electric_power: 0.0,
        }
    }

    /// Set exhaust (return) air conditions.  Called each timestep by the
    /// simulation driver once zone temperatures are known.
    pub fn set_exhaust_conditions(&mut self, temp: f64, w: f64) {
        self.exhaust_air_temp = temp;
        self.exhaust_air_w = w;
    }
}

impl AirComponent for HeatRecovery {
    fn name(&self) -> &str {
        &self.name
    }

    fn simulate_air(&mut self, inlet: &AirPort, _ctx: &SimulationContext) -> AirPort {
        // Zero flow -- nothing to do
        if inlet.mass_flow <= 0.0 {
            self.sensible_recovery = 0.0;
            self.latent_recovery = 0.0;
            self.electric_power = 0.0;
            return *inlet;
        }

        let t_oa = inlet.state.t_db;
        let w_oa = inlet.state.w;
        let t_exh = self.exhaust_air_temp;
        let w_exh = self.exhaust_air_w;

        // ── Bypass check ────────────────────────────────────────────────
        // If OA is already close to exhaust temperature, bypass the wheel.
        if (t_oa - t_exh).abs() < BYPASS_DEADBAND {
            self.sensible_recovery = 0.0;
            self.latent_recovery = 0.0;
            self.electric_power = 0.0;
            return *inlet;
        }

        // ── Sensible recovery ───────────────────────────────────────────
        // Using the effectiveness-NTU approach with C_min on the supply side
        // (conservative: assumes balanced or supply-limited flow).
        let cp_air = psych::cp_air_fn_w(w_oa);
        let c_supply = inlet.mass_flow * cp_air; // [W/K]

        // Q_sensible = epsilon_s * C_min * (T_exhaust - T_outdoor)
        // Positive Q means heating the outdoor air (winter), negative means
        // cooling (summer).
        let q_sensible = self.sensible_effectiveness * c_supply * (t_exh - t_oa);

        let outlet_t = t_oa + q_sensible / c_supply;

        // ── Latent recovery (enthalpy wheel only) ───────────────────────
        let latent_eff = match self.hr_type {
            HeatRecoveryType::EnthalpyWheel => self.latent_effectiveness,
            HeatRecoveryType::PlateHX => 0.0,
        };

        let delta_w = latent_eff * (w_exh - w_oa);
        let outlet_w = (w_oa + delta_w).max(psych::w_fn_tdb_rh_pb(-100.0, 0.0, inlet.state.p_b));
        let q_latent = inlet.mass_flow * HFG * delta_w;

        // ── Store runtime state ─────────────────────────────────────────
        self.sensible_recovery = q_sensible;
        self.latent_recovery = q_latent;
        self.electric_power = self.parasitic_power;

        AirPort::new(
            psych::MoistAirState::new(outlet_t, outlet_w, inlet.state.p_b),
            inlet.mass_flow,
        )
    }

    fn design_air_flow_rate(&self) -> Option<f64> {
        None // Heat recovery does not set air flow rate
    }

    fn power_consumption(&self) -> f64 {
        self.electric_power
    }

    fn thermal_output(&self) -> f64 {
        // Total recovery = sensible + latent
        self.sensible_recovery + self.latent_recovery
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
    fn test_winter_preheating() {
        // Very cold outdoor air (-15 C), warm exhaust (22 C).
        // Enthalpy wheel should preheat outdoor air significantly.
        let mut hr = HeatRecovery::enthalpy_wheel("Winter ERV", 0.80, 0.70, 200.0);
        hr.set_exhaust_conditions(22.0, 0.008);

        let inlet_state = MoistAirState::from_tdb_rh(-15.0, 0.80, 101325.0);
        let rho = inlet_state.rho();
        let mass_flow = 0.5 * rho; // 0.5 m^3/s
        let inlet = AirPort::new(inlet_state, mass_flow);
        let ctx = make_ctx(-15.0);

        let outlet = hr.simulate_air(&inlet, &ctx);

        // With 80% sensible effectiveness and dT of 37 C,
        // expected temperature rise = 0.80 * 37 = 29.6 C
        // outlet ~ -15 + 29.6 = 14.6 C
        let expected_outlet_t = -15.0 + 0.80 * (22.0 - (-15.0));
        assert_relative_eq!(outlet.state.t_db, expected_outlet_t, max_relative = 0.01);

        // Outlet should be much warmer than inlet
        assert!(outlet.state.t_db > 10.0);
        assert!(outlet.state.t_db < 22.0);

        // Sensible recovery should be positive (heating)
        assert!(hr.sensible_recovery > 0.0);

        // Parasitic power should be active
        assert_relative_eq!(hr.electric_power, 200.0, max_relative = 0.001);
    }

    #[test]
    fn test_summer_precooling() {
        // Hot outdoor air (35 C), cooler exhaust (24 C).
        // The wheel should pre-cool the outdoor air.
        let mut hr = HeatRecovery::enthalpy_wheel("Summer ERV", 0.75, 0.65, 150.0);
        hr.set_exhaust_conditions(24.0, 0.009);

        let inlet_state = MoistAirState::from_tdb_rh(35.0, 0.40, 101325.0);
        let rho = inlet_state.rho();
        let mass_flow = 0.5 * rho;
        let inlet = AirPort::new(inlet_state, mass_flow);
        let ctx = make_ctx(35.0);

        let outlet = hr.simulate_air(&inlet, &ctx);

        // With 75% effectiveness and dT of -11 C,
        // expected dT = 0.75 * (24 - 35) = -8.25 C
        // outlet ~ 35 - 8.25 = 26.75 C
        let expected_outlet_t = 35.0 + 0.75 * (24.0 - 35.0);
        assert_relative_eq!(outlet.state.t_db, expected_outlet_t, max_relative = 0.01);

        // Outlet should be cooler than inlet
        assert!(outlet.state.t_db < 35.0);
        assert!(outlet.state.t_db > 24.0);

        // Sensible recovery should be negative (cooling)
        assert!(hr.sensible_recovery < 0.0);
    }

    #[test]
    fn test_plate_hx_no_latent_recovery() {
        // Plate HX should only do sensible recovery, no moisture transfer.
        let mut hr = HeatRecovery::plate_hx("Plate HX", 0.75, 100.0);
        hr.set_exhaust_conditions(22.0, 0.012); // exhaust is more humid

        let inlet_state = MoistAirState::from_tdb_rh(-5.0, 0.80, 101325.0);
        let rho = inlet_state.rho();
        let mass_flow = 0.4 * rho;
        let inlet = AirPort::new(inlet_state, mass_flow);
        let ctx = make_ctx(-5.0);

        let outlet = hr.simulate_air(&inlet, &ctx);

        // Sensible recovery should occur (preheating)
        assert!(outlet.state.t_db > -5.0);
        assert!(hr.sensible_recovery > 0.0);

        // Latent recovery must be zero for plate HX
        assert_relative_eq!(hr.latent_recovery, 0.0, epsilon = 1.0e-10);

        // Humidity ratio should be unchanged
        assert_relative_eq!(outlet.state.w, inlet.state.w, max_relative = 0.001);
    }

    #[test]
    fn test_zero_flow_returns_inlet_unchanged() {
        let mut hr = HeatRecovery::enthalpy_wheel("Zero Flow", 0.80, 0.70, 200.0);
        let inlet_state = MoistAirState::from_tdb_rh(-10.0, 0.50, 101325.0);
        let inlet = AirPort::new(inlet_state, 0.0);
        let ctx = make_ctx(-10.0);

        let outlet = hr.simulate_air(&inlet, &ctx);

        assert_relative_eq!(outlet.state.t_db, inlet.state.t_db, max_relative = 0.001);
        assert_relative_eq!(outlet.state.w, inlet.state.w, max_relative = 0.001);
        assert_eq!(hr.sensible_recovery, 0.0);
        assert_eq!(hr.latent_recovery, 0.0);
        assert_eq!(hr.electric_power, 0.0);
    }

    #[test]
    fn test_bypass_when_oa_close_to_exhaust() {
        // When outdoor air is within the deadband of exhaust temperature,
        // the wheel bypasses -- no energy transfer, no parasitic power.
        let mut hr = HeatRecovery::enthalpy_wheel("Bypass Test", 0.80, 0.70, 200.0);
        hr.set_exhaust_conditions(22.0, 0.008);

        // OA at 21.5 C -- within 1 C of exhaust (22 C)
        let inlet_state = MoistAirState::from_tdb_rh(21.5, 0.50, 101325.0);
        let rho = inlet_state.rho();
        let mass_flow = 0.5 * rho;
        let inlet = AirPort::new(inlet_state, mass_flow);
        let ctx = make_ctx(21.5);

        let outlet = hr.simulate_air(&inlet, &ctx);

        // Temperature should be unchanged (bypass)
        assert_relative_eq!(outlet.state.t_db, inlet.state.t_db, max_relative = 0.001);
        assert_eq!(hr.sensible_recovery, 0.0);
        assert_eq!(hr.latent_recovery, 0.0);
        // Parasitic power should be zero in bypass mode
        assert_eq!(hr.electric_power, 0.0);
    }
}
