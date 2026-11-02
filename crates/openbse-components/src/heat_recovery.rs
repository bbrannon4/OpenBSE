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

fn default_submeter() -> String {
    "General".to_string()
}

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
    #[serde(default = "default_submeter")]
    pub submeter: String,
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
    /// Exhaust-to-supply mass flow ratio [0-1+] (default 1.0 = balanced).
    /// Real systems usually exhaust less than they supply (building
    /// pressurization); the exhaust stream then limits recovery:
    /// Q = ε · C_min · ΔT with C_min = min(C_supply, ratio·C_supply).
    #[serde(default = "default_exhaust_flow_ratio")]
    pub exhaust_flow_ratio: f64,
    /// Sensible effectiveness when cooling the outdoor air (outdoor warmer
    /// than exhaust) [0-1]. `None` uses `sensible_effectiveness`, which then
    /// applies in both seasons (E+ has separate heating/cooling inputs).
    #[serde(default)]
    pub cooling_sensible_effectiveness: Option<f64>,

    // ─── Runtime state (not serialized) ─────────────────────────────────
    /// Supply-outlet temperature setpoint [°C] for this timestep, as E+
    /// `Supply Air Outlet Temperature Control = Yes` with an
    /// `OutdoorAirPretreat` setpoint: recovery is throttled so the outlet
    /// does not cross the setpoint, and stops when it would move the outlet
    /// away from it. `None` runs at full effectiveness.
    #[serde(skip)]
    pub supply_outlet_setpoint: Option<f64>,
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

fn default_exhaust_flow_ratio() -> f64 {
    1.0
}

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
            submeter: "General".to_string(),
            hr_type: HeatRecoveryType::EnthalpyWheel,
            sensible_effectiveness: sensible_eff,
            latent_effectiveness: latent_eff,
            exhaust_air_temp: 22.0,
            exhaust_air_w: 0.008,
            parasitic_power,
            exhaust_flow_ratio: 1.0,
            cooling_sensible_effectiveness: None,
            supply_outlet_setpoint: None,
            sensible_recovery: 0.0,
            latent_recovery: 0.0,
            electric_power: 0.0,
        }
    }

    /// Create a new plate heat exchanger (sensible-only) heat recovery unit.
    pub fn plate_hx(name: &str, sensible_eff: f64, parasitic_power: f64) -> Self {
        Self {
            name: name.to_string(),
            submeter: "General".to_string(),
            hr_type: HeatRecoveryType::PlateHX,
            sensible_effectiveness: sensible_eff,
            latent_effectiveness: 0.0, // plate HX has no latent recovery
            exhaust_air_temp: 22.0,
            exhaust_air_w: 0.008,
            parasitic_power,
            exhaust_flow_ratio: 1.0,
            cooling_sensible_effectiveness: None,
            supply_outlet_setpoint: None,
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

    /// Fraction of full recovery to run [0-1] against the supply-outlet
    /// setpoint, as E+ `CalcAirToAirGenericHeatExch` does with
    /// `ControlToTemperatureSetPoint`: throttle so the outlet lands on the
    /// setpoint when full recovery would cross it, run fully when it only
    /// moves the outlet toward it, and stop when it would move it away.
    fn control_fraction(&self, t_in: f64, full_outlet_t: f64) -> f64 {
        let Some(sp) = self.supply_outlet_setpoint else {
            return 1.0;
        };
        if (t_in - full_outlet_t).abs() < 1e-12 {
            return 0.0;
        }
        let crosses = (full_outlet_t > sp && t_in < sp) || (t_in > sp && full_outlet_t < sp);
        let toward = (t_in < full_outlet_t && full_outlet_t < sp)
            || (t_in > full_outlet_t && full_outlet_t > sp);
        if crosses {
            ((t_in - sp) / (t_in - full_outlet_t)).abs().clamp(0.0, 1.0)
        } else if toward {
            1.0
        } else {
            0.0
        }
    }
}

impl AirComponent for HeatRecovery {
    fn name(&self) -> &str {
        &self.name
    }

    fn component_kind(&self) -> ComponentKind {
        ComponentKind::HeatRecovery
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
        // Effectiveness-NTU: recovery scales with the LIMITING capacity
        // rate. With exhaust flow below supply flow (typical — building
        // pressurization), the exhaust stream limits what can be recovered:
        //   Q = ε_s · C_min · (T_exhaust − T_outdoor)
        //   C_min = min(C_supply, ratio · C_supply)
        let cp_air = psych::cp_air_fn_w(w_oa);
        let c_supply = inlet.mass_flow * cp_air; // [W/K]
        let flow_ratio = self.exhaust_flow_ratio.clamp(0.0, 1.0);
        let c_min = c_supply * flow_ratio;

        // Positive Q means heating the outdoor air (winter), negative means
        // cooling (summer).
        let sensible_eff = if t_oa > t_exh {
            self.cooling_sensible_effectiveness
                .unwrap_or(self.sensible_effectiveness)
        } else {
            self.sensible_effectiveness
        };
        let full_outlet_t = t_oa + sensible_eff * c_min * (t_exh - t_oa) / c_supply;
        let control = self.control_fraction(t_oa, full_outlet_t);

        let q_sensible = control * sensible_eff * c_min * (t_exh - t_oa);
        let outlet_t = t_oa + q_sensible / c_supply;

        // ── Latent recovery (enthalpy wheel only) ───────────────────────
        // Throttling a rotary wheel slows it, which cuts latent transfer by
        // the same fraction (E+ scales both effectivenesses).
        let latent_eff = match self.hr_type {
            HeatRecoveryType::EnthalpyWheel => self.latent_effectiveness,
            HeatRecoveryType::PlateHX => 0.0,
        };

        let delta_w = control * latent_eff * flow_ratio * (w_exh - w_oa);
        let outlet_w = (w_oa + delta_w).max(psych::w_fn_tdb_rh_pb(-100.0, 0.0, inlet.state.p_b));
        let q_latent =
            inlet.mass_flow * openbse_psychrometrics::hg_air_fn_tdb(inlet.state.t_db) * delta_w;

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

    fn set_exhaust_conditions(&mut self, temp: f64, w: f64) {
        self.exhaust_air_temp = temp;
        self.exhaust_air_w = w;
    }

    /// Supply-outlet temperature setpoint; a non-finite value (NaN) clears
    /// it, so the unit runs at full effectiveness.
    fn set_setpoint(&mut self, setpoint: f64) {
        self.supply_outlet_setpoint = setpoint.is_finite().then_some(setpoint);
    }

    fn setpoint(&self) -> Option<f64> {
        self.supply_outlet_setpoint
    }

    fn report_outputs(&self, out: &mut dyn FnMut(&str, f64)) {
        out("sensible_load", self.sensible_recovery);
        out("latent_load", self.latent_recovery);
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

    /// A wheel with 0.70 heating / 0.75 cooling sensible effectiveness and a
    /// 22 °C exhaust, run on 1 kg/s of outdoor air at `t_oa`.
    fn controlled_outlet(t_oa: f64, setpoint: f64) -> (f64, HeatRecovery) {
        let mut hr = HeatRecovery::enthalpy_wheel("ERV", 0.70, 0.60, 100.0);
        hr.cooling_sensible_effectiveness = Some(0.75);
        hr.set_exhaust_conditions(22.0, 0.008);
        hr.set_setpoint(setpoint);
        let inlet = AirPort::new(MoistAirState::from_tdb_rh(t_oa, 0.5, 101325.0), 1.0);
        let outlet = hr.simulate_air(&inlet, &make_ctx(t_oa));
        (outlet.state.t_db, hr)
    }

    #[test]
    fn supply_outlet_control_matches_energyplus() {
        // No setpoint (NaN clears it): full recovery, 0.70 × 32 K = 22.4 K.
        let (t, _) = controlled_outlet(-10.0, f64::NAN);
        assert_relative_eq!(t, -10.0 + 0.70 * 32.0, max_relative = 1e-9);

        // Full recovery would overshoot a 5 °C setpoint: throttle onto it,
        // latent transfer scaled by the same fraction.
        let (t, hr) = controlled_outlet(-10.0, 5.0);
        assert_relative_eq!(t, 5.0, max_relative = 1e-9);
        let (_, full) = controlled_outlet(-10.0, f64::NAN);
        let fraction = 15.0 / 22.4;
        assert_relative_eq!(
            hr.latent_recovery,
            fraction * full.latent_recovery,
            max_relative = 1e-9
        );

        // Recovery only moves the outlet toward a 20 °C setpoint: run fully.
        let (t, _) = controlled_outlet(-10.0, 20.0);
        assert_relative_eq!(t, 12.4, max_relative = 1e-9);

        // Outdoor air already above the setpoint: heating it would move it
        // away, so the wheel stops (but stays on, drawing its power).
        let (t, hr) = controlled_outlet(-10.0, -15.0);
        assert_relative_eq!(t, -10.0, max_relative = 1e-9);
        assert_eq!(hr.sensible_recovery, 0.0);
        assert_relative_eq!(hr.electric_power, 100.0);

        // Summer: cooling effectiveness, 0.75 × 10 K.
        let (t, _) = controlled_outlet(32.0, f64::NAN);
        assert_relative_eq!(t, 32.0 - 0.75 * 10.0, max_relative = 1e-9);
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
