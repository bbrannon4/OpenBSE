//! Ground-source heat pump (GSHP) component.
//!
//! Models a self-contained ground-source heat pump that exchanges heat with the
//! ground via a buried loop. Entering water temperature (EWT) is computed from
//! a ground temperature model each timestep — no external plant loop needed.
//!
//! Physics (mirrors WSHP):
//!   Cooling: Q = rated_cap × PLR × cap_mod; W = Q / (COP × eir_mod)
//!   Heating: Q = rated_cap × PLR × cap_mod; W = Q / (COP × eir_mod)
//!
//! Ground temperature is computed from one of three sources:
//!   Auto        — Kusuda-Achenbach sinusoidal model at loop_depth
//!   EpwMonthly  — EPW header monthly temps (falls back to Auto if unavailable)
//!   Monthly     — User-specified monthly table (12 values)
//!
//! Reference: EnergyPlus Engineering Reference, "HeatPump:WaterToAir:EquationFit"

use crate::performance_curve::PerformanceCurve;
use openbse_core::ports::*;
use openbse_psychrometrics as psych;
use serde::{Deserialize, Serialize};

fn default_submeter() -> String {
    "General".to_string()
}
fn default_gshp_cop_cooling() -> f64 {
    4.5
}
fn default_gshp_cop_heating() -> f64 {
    4.0
}
fn default_loop_depth() -> f64 {
    1.5
}

/// Selects how the GSHP determines the ground loop entering water temperature.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GroundTempSource {
    /// Kusuda-Achenbach sinusoidal model derived from weather-file annual stats.
    /// Uses `loop_depth` [m] (1.5 m for horizontal, deeper for vertical boreholes).
    Auto,
    /// EPW header monthly ground temps at 0.5 m depth when available; falls
    /// back to `Auto` if the weather file contains no ground temperature data.
    EpwMonthly,
    /// User-specified monthly ground temperatures [°C], January through December.
    Monthly([f64; 12]),
}

impl Default for GroundTempSource {
    fn default() -> Self {
        GroundTempSource::Auto
    }
}

/// Operating mode of the ground-source heat pump.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub enum GshpMode {
    #[default]
    Off,
    Cooling,
    Heating,
}

/// Ground-source heat pump component.
///
/// Self-contained — no plant loop connection.  The entering water temperature
/// is computed internally from the configured ground temperature source.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GroundSourceHeatPump {
    pub name: String,
    #[serde(default = "default_submeter")]
    pub submeter: String,
    /// Rated cooling capacity [W] at rated conditions
    pub rated_cooling_capacity: f64,
    /// Rated heating capacity [W] at rated conditions
    pub rated_heating_capacity: f64,
    /// Rated COP in cooling mode
    #[serde(default = "default_gshp_cop_cooling")]
    pub cop_cooling: f64,
    /// Rated COP in heating mode
    #[serde(default = "default_gshp_cop_heating")]
    pub cop_heating: f64,
    /// Supply air temperature setpoint [°C]
    pub outlet_temp_setpoint: f64,
    /// Rated volumetric airflow [m³/s]
    pub rated_airflow: f64,
    /// Ground temperature source selection
    #[serde(default)]
    pub ground_temp_source: GroundTempSource,
    /// Ground loop burial depth [m] (used by Kusuda model)
    #[serde(default = "default_loop_depth")]
    pub loop_depth: f64,

    // ─── Performance curves (optional, resolved at build time) ───────────
    #[serde(skip)]
    pub cooling_cap_ft: Option<PerformanceCurve>,
    #[serde(skip)]
    pub cooling_eir_ft: Option<PerformanceCurve>,
    #[serde(skip)]
    pub heating_cap_ft: Option<PerformanceCurve>,
    #[serde(skip)]
    pub heating_eir_ft: Option<PerformanceCurve>,

    // ─── Ground temp model params (set at build time via configure_ground_source) ──
    #[serde(skip)]
    kusuda_t_mean: f64,
    #[serde(skip)]
    kusuda_amplitude: f64,
    #[serde(skip)]
    kusuda_phase_day: f64,
    #[serde(skip)]
    kusuda_diffusivity: f64,
    /// Monthly temps from EPW header (set by simulation driver when EpwMonthly selected)
    #[serde(skip)]
    epw_monthly_temps: Option<[f64; 12]>,

    // ─── Runtime state ───────────────────────────────────────────────────
    /// Electric power consumed this timestep [W]
    #[serde(skip)]
    pub power: f64,
    /// Thermal output to air [W] (positive = heating, negative = cooling)
    #[serde(skip)]
    pub air_thermal_output: f64,
    /// Current EWT [°C]
    #[serde(skip)]
    pub current_ewt: f64,
    /// Current mode
    #[serde(skip)]
    pub mode: GshpMode,
}

impl GroundSourceHeatPump {
    pub fn new(
        name: &str,
        rated_cooling_capacity: f64,
        rated_heating_capacity: f64,
        cop_cooling: f64,
        cop_heating: f64,
        outlet_temp_setpoint: f64,
        rated_airflow: f64,
    ) -> Self {
        Self {
            name: name.to_string(),
            submeter: "General".to_string(),
            rated_cooling_capacity,
            rated_heating_capacity,
            cop_cooling,
            cop_heating,
            outlet_temp_setpoint,
            rated_airflow,
            ground_temp_source: GroundTempSource::Auto,
            loop_depth: 1.5,
            cooling_cap_ft: None,
            cooling_eir_ft: None,
            heating_cap_ft: None,
            heating_eir_ft: None,
            kusuda_t_mean: 12.0,
            kusuda_amplitude: 10.0,
            kusuda_phase_day: 35.0,
            kusuda_diffusivity: 0.04,
            epw_monthly_temps: None,
            power: 0.0,
            air_thermal_output: 0.0,
            current_ewt: 12.0,
            mode: GshpMode::Off,
        }
    }

    /// Simulate for one timestep with an explicit entering water temperature.
    ///
    /// Returns `(supply_temp [°C], supply_mass_flow [kg/s])`.
    pub fn simulate_ground(
        &mut self,
        mode: GshpMode,
        load_fraction: f64, // PLR [0-1]
        indoor_temp: f64,   // zone air temp [°C]
        ewt: f64,           // entering water temp from ground [°C]
        _ambient_temp: f64, // outdoor air temp (reserved for defrost)
    ) -> (f64, f64) {
        self.current_ewt = ewt;
        let cp = psych::cp_air_fn_w(0.008); // typical supply air humidity

        match mode {
            GshpMode::Off => {
                self.power = 0.0;
                self.air_thermal_output = 0.0;
                self.mode = GshpMode::Off;
                (indoor_temp, 0.0)
            }
            GshpMode::Cooling => {
                let cap_mod = self
                    .cooling_cap_ft
                    .as_ref()
                    .map(|c| c.evaluate(ewt, indoor_temp))
                    .unwrap_or(1.0);
                let eir_mod = self
                    .cooling_eir_ft
                    .as_ref()
                    .map(|c| c.evaluate(ewt, indoor_temp))
                    .unwrap_or(1.0);
                let q = self.rated_cooling_capacity * load_fraction.clamp(0.0, 1.0) * cap_mod;
                let w = q / (self.cop_cooling * eir_mod).max(0.1);
                let t_supply = self.outlet_temp_setpoint;
                let mass_flow = (q / (cp * (indoor_temp - t_supply).abs())).max(0.01);
                self.power = w;
                self.air_thermal_output = -q;
                self.mode = GshpMode::Cooling;
                (t_supply, mass_flow)
            }
            GshpMode::Heating => {
                let cap_mod = self
                    .heating_cap_ft
                    .as_ref()
                    .map(|c| c.evaluate(ewt, indoor_temp))
                    .unwrap_or(1.0);
                let eir_mod = self
                    .heating_eir_ft
                    .as_ref()
                    .map(|c| c.evaluate(ewt, indoor_temp))
                    .unwrap_or(1.0);
                let q = self.rated_heating_capacity * load_fraction.clamp(0.0, 1.0) * cap_mod;
                let w = q / (self.cop_heating * eir_mod).max(0.1);
                let t_supply = self.outlet_temp_setpoint;
                let mass_flow = (q / (cp * (t_supply - indoor_temp).abs())).max(0.01);
                self.power = w;
                self.air_thermal_output = q;
                self.mode = GshpMode::Heating;
                (t_supply, mass_flow)
            }
        }
    }

    /// Ground temperature for the given day of year [°C].
    ///
    /// Selects the source based on `ground_temp_source`:
    /// - `Auto`: Kusuda-Achenbach at `loop_depth`
    /// - `EpwMonthly`: EPW header monthly temps if available, else Kusuda fallback
    /// - `Monthly(temps)`: linear interpolation of the user table
    pub fn ground_temp_for_day(&self, day_of_year: u32) -> f64 {
        match &self.ground_temp_source {
            GroundTempSource::Auto => self.kusuda(day_of_year as f64, self.loop_depth),
            GroundTempSource::EpwMonthly => {
                if let Some(ref temps) = self.epw_monthly_temps {
                    interpolate_monthly(temps, day_of_year as f64)
                } else {
                    self.kusuda(day_of_year as f64, self.loop_depth)
                }
            }
            GroundTempSource::Monthly(temps) => interpolate_monthly(temps, day_of_year as f64),
        }
    }

    /// Kusuda-Achenbach ground temperature at the given depth [°C].
    fn kusuda(&self, day: f64, depth: f64) -> f64 {
        let alpha = self.kusuda_diffusivity;
        if alpha <= 0.0 {
            return self.kusuda_t_mean;
        }
        let damping_arg = depth * (std::f64::consts::PI / (365.0 * alpha)).sqrt();
        let damping = (-damping_arg).exp();
        let phase_shift = depth / 2.0 * (365.0 / (std::f64::consts::PI * alpha)).sqrt();
        let cos_arg =
            2.0 * std::f64::consts::PI / 365.0 * (day - self.kusuda_phase_day - phase_shift);
        self.kusuda_t_mean - self.kusuda_amplitude * damping * cos_arg.cos()
    }
}

/// Linear interpolation of monthly ground temperatures (mid-month anchors).
fn interpolate_monthly(temps: &[f64; 12], day_of_year: f64) -> f64 {
    let days_in_month: [f64; 12] = [
        31.0, 28.0, 31.0, 30.0, 31.0, 30.0, 31.0, 31.0, 30.0, 31.0, 30.0, 31.0,
    ];
    let mut mid_days = [0.0f64; 12];
    let mut cum = 0.0;
    for m in 0..12 {
        mid_days[m] = cum + days_in_month[m] / 2.0;
        cum += days_in_month[m];
    }

    let doy = ((day_of_year % 365.0) + 365.0) % 365.0;

    if doy <= mid_days[0] {
        let dec_mid = mid_days[11] - 365.0;
        let span = mid_days[0] - dec_mid;
        let frac = (doy - dec_mid) / span;
        temps[11] + frac * (temps[0] - temps[11])
    } else if doy >= mid_days[11] {
        let jan_mid = mid_days[0] + 365.0;
        let span = jan_mid - mid_days[11];
        let frac = (doy - mid_days[11]) / span;
        temps[11] + frac * (temps[0] - temps[11])
    } else {
        for m in 0..11 {
            if doy >= mid_days[m] && doy < mid_days[m + 1] {
                let span = mid_days[m + 1] - mid_days[m];
                let frac = (doy - mid_days[m]) / span;
                return temps[m] + frac * (temps[m + 1] - temps[m]);
            }
        }
        temps[11]
    }
}

/// Compute day of year from month (1-based) and day.
fn day_of_year(month: u32, day: u32) -> u32 {
    let days_before: [u32; 12] = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
    let m = (month.saturating_sub(1) as usize).min(11);
    days_before[m] + day
}

impl AirComponent for GroundSourceHeatPump {
    fn name(&self) -> &str {
        &self.name
    }

    fn component_kind(&self) -> ComponentKind {
        ComponentKind::Gshp
    }

    fn simulate_air(&mut self, inlet: &AirPort, ctx: &SimulationContext) -> AirPort {
        if inlet.mass_flow <= 0.0 {
            self.power = 0.0;
            self.air_thermal_output = 0.0;
            self.mode = GshpMode::Off;
            return *inlet;
        }

        let t_in = inlet.state.t_db;
        let t_sp = self.outlet_temp_setpoint;
        let cp = psych::cp_air_fn_w(inlet.state.w);

        // Determine mode from setpoint vs inlet temp
        let mode = if t_sp < t_in - 0.1 {
            GshpMode::Cooling
        } else if t_sp > t_in + 0.1 {
            GshpMode::Heating
        } else {
            GshpMode::Off
        };

        if mode == GshpMode::Off {
            self.power = 0.0;
            self.air_thermal_output = 0.0;
            self.mode = GshpMode::Off;
            return *inlet;
        }

        // Compute EWT from stored ground temp model
        let doy = day_of_year(ctx.timestep.month, ctx.timestep.day);
        let ewt = self.ground_temp_for_day(doy);
        self.current_ewt = ewt;

        match mode {
            GshpMode::Cooling => {
                let cap_mod = self
                    .cooling_cap_ft
                    .as_ref()
                    .map(|c| c.evaluate(ewt, t_in))
                    .unwrap_or(1.0);
                let eir_mod = self
                    .cooling_eir_ft
                    .as_ref()
                    .map(|c| c.evaluate(ewt, t_in))
                    .unwrap_or(1.0);
                let cap_available = self.rated_cooling_capacity * cap_mod;
                let cap_needed = inlet.mass_flow * cp * (t_in - t_sp);
                let cap_actual = cap_needed.min(cap_available).max(0.0);
                let t_out = t_in - cap_actual / (inlet.mass_flow * cp).max(1e-6);
                self.power = cap_actual / (self.cop_cooling * eir_mod).max(0.1);
                self.air_thermal_output = -cap_actual;
                self.mode = GshpMode::Cooling;
                AirPort::new(
                    psych::MoistAirState::new(t_out, inlet.state.w, inlet.state.p_b),
                    inlet.mass_flow,
                )
            }
            GshpMode::Heating => {
                let cap_mod = self
                    .heating_cap_ft
                    .as_ref()
                    .map(|c| c.evaluate(ewt, t_in))
                    .unwrap_or(1.0);
                let eir_mod = self
                    .heating_eir_ft
                    .as_ref()
                    .map(|c| c.evaluate(ewt, t_in))
                    .unwrap_or(1.0);
                let cap_available = self.rated_heating_capacity * cap_mod;
                let cap_needed = inlet.mass_flow * cp * (t_sp - t_in);
                let cap_actual = cap_needed.min(cap_available).max(0.0);
                let t_out = t_in + cap_actual / (inlet.mass_flow * cp).max(1e-6);
                self.power = cap_actual / (self.cop_heating * eir_mod).max(0.1);
                self.air_thermal_output = cap_actual;
                self.mode = GshpMode::Heating;
                AirPort::new(
                    psych::MoistAirState::new(t_out, inlet.state.w, inlet.state.p_b),
                    inlet.mass_flow,
                )
            }
            GshpMode::Off => unreachable!(),
        }
    }

    fn set_setpoint(&mut self, setpoint: f64) {
        self.outlet_temp_setpoint = setpoint;
    }

    fn power_consumption(&self) -> f64 {
        self.power
    }

    fn thermal_output(&self) -> f64 {
        self.air_thermal_output
    }

    fn nominal_capacity(&self) -> Option<f64> {
        Some(self.rated_cooling_capacity)
    }

    fn set_nominal_capacity(&mut self, cap: f64) {
        self.rated_cooling_capacity = cap;
    }

    fn set_heating_capacity(&mut self, cap: f64) {
        self.rated_heating_capacity = cap;
    }

    fn configure_ground_source(
        &mut self,
        t_mean: f64,
        amplitude: f64,
        phase_day: f64,
        soil_diffusivity: f64,
        _loop_depth: f64,
        epw_monthly_temps: Option<[f64; 12]>,
    ) {
        self.kusuda_t_mean = t_mean;
        self.kusuda_amplitude = amplitude;
        self.kusuda_phase_day = phase_day;
        self.kusuda_diffusivity = soil_diffusivity;
        // loop_depth stays as configured in the struct (user YAML value)
        self.epw_monthly_temps = epw_monthly_temps;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_relative_eq;
    use openbse_core::ports::SizingInternalGains;
    use openbse_core::types::{DayType, TimeStep};
    use openbse_psychrometrics::MoistAirState;

    fn make_ctx() -> SimulationContext {
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
            outdoor_air: MoistAirState::from_tdb_rh(30.0, 0.40, 101325.0),
            day_type: DayType::WeatherDay,
            is_sizing: false,
            sizing_internal_gains: SizingInternalGains::Full,
        }
    }

    fn make_gshp_cooling() -> GroundSourceHeatPump {
        let mut g = GroundSourceHeatPump::new("GSHP-1", 10_000.0, 9_000.0, 4.5, 4.0, 13.0, 0.5);
        // Configure with typical Kusuda params
        g.configure_ground_source(12.0, 10.0, 35.0, 0.04, 1.5, None);
        g
    }

    fn make_gshp_heating() -> GroundSourceHeatPump {
        let mut g = GroundSourceHeatPump::new(
            "GSHP-H", 10_000.0, 9_000.0, 4.5, 4.0, 40.0, // heating setpoint
            0.5,
        );
        g.configure_ground_source(12.0, 10.0, 35.0, 0.04, 1.5, None);
        g
    }

    #[test]
    fn test_cooling_power_equals_q_over_cop() {
        let mut gshp = make_gshp_cooling();
        let ewt = 15.0;
        let (_, _) = gshp.simulate_ground(GshpMode::Cooling, 1.0, 26.0, ewt, 30.0);
        let q = -gshp.air_thermal_output;
        let expected_w = q / gshp.cop_cooling;
        assert_relative_eq!(gshp.power, expected_w, max_relative = 0.01);
        assert_eq!(gshp.mode, GshpMode::Cooling);
    }

    #[test]
    fn test_heating_power_equals_q_over_cop() {
        let mut gshp = make_gshp_heating();
        let ewt = 8.0;
        let (t_sup, _) = gshp.simulate_ground(GshpMode::Heating, 1.0, 20.0, ewt, -5.0);
        let q = gshp.air_thermal_output;
        let expected_w = q / gshp.cop_heating;
        assert_relative_eq!(gshp.power, expected_w, max_relative = 0.01);
        assert_eq!(t_sup, 40.0);
        assert_eq!(gshp.mode, GshpMode::Heating);
    }

    #[test]
    fn test_cooling_supply_temp_equals_setpoint() {
        let mut gshp = make_gshp_cooling();
        let (t_sup, _) = gshp.simulate_ground(GshpMode::Cooling, 0.8, 26.0, 15.0, 30.0);
        assert_relative_eq!(t_sup, 13.0, epsilon = 0.01);
    }

    #[test]
    fn test_ground_temp_auto_sinusoidal() {
        let mut gshp = GroundSourceHeatPump::new("G", 1000.0, 1000.0, 4.5, 4.0, 13.0, 0.5);
        gshp.ground_temp_source = GroundTempSource::Auto;
        gshp.configure_ground_source(12.0, 10.0, 35.0, 0.04, 1.5, None);

        // At depth 1.5 m, summer > winter for northern hemisphere
        let t_summer = gshp.ground_temp_for_day(196); // mid-July
        let t_winter = gshp.ground_temp_for_day(15); // mid-January
        assert!(
            t_summer > t_winter,
            "Summer EWT {:.1} must exceed winter EWT {:.1}",
            t_summer,
            t_winter
        );
    }

    #[test]
    fn test_ground_temp_monthly_interpolation() {
        let mut gshp = GroundSourceHeatPump::new("G", 1000.0, 1000.0, 4.5, 4.0, 13.0, 0.5);
        let temps = [
            -0.09, -1.03, 0.64, 3.26, 10.11, 15.39, 18.96, 20.04, 18.19, 14.09, 8.61, 3.52,
        ];
        gshp.ground_temp_source = GroundTempSource::Monthly(temps);
        gshp.configure_ground_source(12.0, 10.0, 35.0, 0.04, 1.5, None);

        // Mid-January (day 16) should be close to Jan value
        let t_jan = gshp.ground_temp_for_day(16);
        assert_relative_eq!(t_jan, -0.09, epsilon = 0.5);

        // Mid-July should be close to Jul value
        let t_jul = gshp.ground_temp_for_day(196);
        assert_relative_eq!(t_jul, 18.96, epsilon = 0.5);
    }

    #[test]
    fn test_ground_temp_epw_monthly_no_data_falls_back_to_auto() {
        let mut gshp = GroundSourceHeatPump::new("G", 1000.0, 1000.0, 4.5, 4.0, 13.0, 0.5);
        gshp.ground_temp_source = GroundTempSource::EpwMonthly;
        // No EPW data configured — should not panic, should return Kusuda value
        gshp.configure_ground_source(12.0, 10.0, 35.0, 0.04, 1.5, None);
        let t = gshp.ground_temp_for_day(180);
        assert!(t.is_finite(), "EWT must be finite even without EPW data");
    }

    #[test]
    fn test_performance_curve_reduces_capacity() {
        use crate::performance_curve::{CurveType, PerformanceCurve};
        let mut gshp = make_gshp_cooling();
        // cap_mod = 0.9 (biquadratic constant 0.9, all other coeffs 0)
        let curve = PerformanceCurve::Polynomial {
            name: "cap_ft".to_string(),
            curve_type: CurveType::Biquadratic,
            coefficients: vec![0.9, 0.0, 0.0, 0.0, 0.0, 0.0],
            min_x: -100.0,
            max_x: 100.0,
            min_y: -100.0,
            max_y: 100.0,
            min_output: None,
            max_output: None,
        };
        gshp.cooling_cap_ft = Some(curve);
        let (_, _) = gshp.simulate_ground(GshpMode::Cooling, 1.0, 26.0, 15.0, 30.0);
        let q = -gshp.air_thermal_output;
        let expected = gshp.rated_cooling_capacity * 0.9;
        assert_relative_eq!(q, expected, max_relative = 0.01);
    }

    #[test]
    fn test_simulate_air_cooling() {
        let mut gshp = make_gshp_cooling();
        let ctx = make_ctx();
        let inlet = AirPort::new(MoistAirState::from_tdb_rh(26.0, 0.5, 101325.0), 0.5);
        let outlet = gshp.simulate_air(&inlet, &ctx);
        assert!(
            outlet.state.t_db < inlet.state.t_db,
            "Cooling must reduce air temp"
        );
        assert!(gshp.power > 0.0);
        assert_eq!(gshp.mode, GshpMode::Cooling);
    }

    #[test]
    fn test_simulate_air_zero_flow() {
        let mut gshp = make_gshp_cooling();
        let ctx = make_ctx();
        let inlet = AirPort::new(MoistAirState::from_tdb_rh(26.0, 0.5, 101325.0), 0.0);
        gshp.simulate_air(&inlet, &ctx);
        assert_eq!(gshp.power, 0.0);
        assert_eq!(gshp.mode, GshpMode::Off);
    }
}
