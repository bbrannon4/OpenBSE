//! Evaporative cooler component.
//!
//! Three modes:
//!   Direct: adiabatic humidification — DBT drops, W rises, enthalpy conserved.
//!   Indirect: outdoor secondary air is cooled by its own pad, then cools the
//!     supply sensibly across a heat exchanger (no moisture added to the supply).
//!   TwoStage: indirect followed by direct.
//!
//! Reference: EnergyPlus Engineering Reference, "Evaporative Coolers"

use openbse_core::ports::*;
use openbse_psychrometrics as psych;
use serde::{Deserialize, Serialize};

fn default_submeter() -> String {
    "General".to_string()
}
fn default_evap_effectiveness() -> f64 {
    0.80
}
fn default_hx_effectiveness() -> f64 {
    0.70
}
fn default_pump_power() -> f64 {
    100.0
}
fn default_secondary_air_flow_ratio() -> f64 {
    1.0
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum EvapCoolerMode {
    /// Adiabatic direct: adds humidity, lowers DBT, approximately conserves enthalpy.
    #[default]
    Direct,
    /// Indirect HX: lowers DBT using wet-bulb as driving temperature, no W addition.
    Indirect,
    /// Indirect stage followed by direct stage.
    TwoStage,
}

/// Evaporative cooler component (direct, indirect, or two-stage).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvapCooler {
    pub name: String,
    #[serde(default = "default_submeter")]
    pub submeter: String,
    #[serde(default)]
    pub mode: EvapCoolerMode,
    /// Saturation effectiveness [0-1] for direct stage (default 0.80)
    #[serde(default = "default_evap_effectiveness")]
    pub effectiveness: f64,
    /// Heat exchanger effectiveness [0-1] for indirect stage (default 0.70)
    #[serde(default = "default_hx_effectiveness")]
    pub hx_effectiveness: f64,
    /// Recirculating water pump power while the cooler operates [W] (default 100).
    #[serde(default = "default_pump_power")]
    pub pump_power: f64,
    /// Secondary (scavenger) air fan power while an indirect stage operates [W]
    /// (default 0).
    #[serde(default)]
    pub secondary_fan_power: f64,
    /// Secondary to primary air mass flow ratio for the indirect stage (default
    /// 1.0). Sets how much water the secondary pad evaporates.
    #[serde(default = "default_secondary_air_flow_ratio")]
    pub secondary_air_flow_ratio: f64,

    // ─── Runtime state ──────────────────────────────────────────────────
    /// Electric power this timestep [W]: pump, plus the secondary fan for an
    /// operating indirect stage.
    #[serde(skip)]
    pub power: f64,
    #[serde(skip)]
    pub thermal_out: f64,
    /// Water evaporated this timestep [kg/s]: the direct pad plus the secondary
    /// pad of an indirect stage (drift and blowdown are not modelled).
    #[serde(skip)]
    pub water_consumption: f64,
}

impl EvapCooler {
    pub fn new(name: &str, mode: EvapCoolerMode) -> Self {
        Self {
            name: name.to_string(),
            submeter: "General".to_string(),
            mode,
            effectiveness: 0.80,
            hx_effectiveness: 0.70,
            pump_power: 100.0,
            secondary_fan_power: 0.0,
            secondary_air_flow_ratio: 1.0,
            power: 0.0,
            thermal_out: 0.0,
            water_consumption: 0.0,
        }
    }

    /// Compute direct-stage outlet: DBT drops, W rises, enthalpy conserved.
    fn direct_stage(t_db: f64, w: f64, p_b: f64, effectiveness: f64) -> (f64, f64) {
        let t_wb = psych::twb_fn_tdb_w_pb(t_db, w, p_b);
        let t_out = t_db - effectiveness * (t_db - t_wb);
        // Enthalpy balance: h_in = h_out → solve W_out
        let h_in = psych::h_fn_tdb_w(t_db, w);
        let w_out = psych::w_fn_tdb_h(t_out, h_in).max(w); // W_out >= W_in
        (t_out, w_out)
    }

    /// Compute indirect-stage outlet, as E+ `EvaporativeCooler:Indirect:CelDekPad`
    /// does: the secondary (scavenger) stream is first cooled by its own pad
    /// toward its wet-bulb, then cools the primary sensibly across the HX:
    ///
    ///   T_sec_out  = T_sec − ε_sat·(T_sec − T_wb_sec)
    ///   T_prim_out = T_prim − ε_hx·(T_prim − T_sec_out)
    ///
    /// The older product form ε_sat·ε_hx·(T_prim − T_wb_sec) only agrees when
    /// the primary and secondary inlets are the same air (EVAP-2 / #59).
    /// Returns the primary outlet (t, w) and the secondary pad outlet humidity
    /// ratio (for water use), or `None` when the secondary air cannot cool the
    /// primary.
    fn indirect_stage(
        t_db: f64,
        w: f64,
        secondary: &psych::MoistAirState,
        sat_eff: f64,
        hx_eff: f64,
    ) -> Option<(f64, f64, f64)> {
        let (t_sec_out, w_sec_out) =
            Self::direct_stage(secondary.t_db, secondary.w, secondary.p_b, sat_eff);
        if t_db <= t_sec_out {
            return None;
        }
        Some((t_db - hx_eff * (t_db - t_sec_out), w, w_sec_out))
    }
}

impl AirComponent for EvapCooler {
    fn name(&self) -> &str {
        &self.name
    }

    fn component_kind(&self) -> ComponentKind {
        ComponentKind::EvapCooler
    }

    fn simulate_air(&mut self, inlet: &AirPort, ctx: &SimulationContext) -> AirPort {
        if inlet.mass_flow <= 0.0 {
            self.power = 0.0;
            self.thermal_out = 0.0;
            self.water_consumption = 0.0;
            return *inlet;
        }

        let t_db = inlet.state.t_db;
        let w = inlet.state.w;
        let p_b = inlet.state.p_b;

        // Secondary (scavenger) stream for the indirect HX = outdoor air.
        let oa = &ctx.outdoor_air;
        let m_secondary = inlet.mass_flow * self.secondary_air_flow_ratio.max(0.0);

        // Water evaporated by the secondary pad, and whether an indirect stage
        // ran (its secondary fan only runs then).
        let mut secondary_water = 0.0;
        let mut indirect_ran = false;
        let mut run_indirect = |t: f64, w_: f64| -> (f64, f64) {
            match Self::indirect_stage(t, w_, oa, self.effectiveness, self.hx_effectiveness) {
                Some((t_out, w_out, w_sec_out)) => {
                    indirect_ran = true;
                    secondary_water = m_secondary * (w_sec_out - oa.w).max(0.0);
                    (t_out, w_out)
                }
                None => (t, w_),
            }
        };

        let (t_out, w_out) = match self.mode {
            EvapCoolerMode::Direct => Self::direct_stage(t_db, w, p_b, self.effectiveness),
            EvapCoolerMode::Indirect => run_indirect(t_db, w),
            EvapCoolerMode::TwoStage => {
                // Stage 1: indirect; stage 2: direct saturation of the primary.
                let (t_int, w_int) = run_indirect(t_db, w);
                Self::direct_stage(t_int, w_int, p_b, self.effectiveness)
            }
        };

        let h_in = psych::h_fn_tdb_w(t_db, w);
        let h_out = psych::h_fn_tdb_w(t_out, w_out);
        // Thermal output: negative = cooling (removes enthalpy from air)
        self.thermal_out = inlet.mass_flow * (h_out - h_in);

        let direct_water = inlet.mass_flow * (w_out - w).max(0.0);
        self.water_consumption = direct_water + secondary_water;
        // The pump runs only while some stage is actually cooling.
        let operating = t_out < t_db - 1.0e-9;
        self.power = if operating {
            self.pump_power
                + if indirect_ran {
                    self.secondary_fan_power
                } else {
                    0.0
                }
        } else {
            0.0
        };

        AirPort::new(
            openbse_psychrometrics::MoistAirState::new(t_out, w_out, p_b),
            inlet.mass_flow,
        )
    }

    fn power_consumption(&self) -> f64 {
        self.power
    }

    fn thermal_output(&self) -> f64 {
        self.thermal_out
    }

    fn report_outputs(&self, out: &mut dyn FnMut(&str, f64)) {
        out("water_consumption", self.water_consumption);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_relative_eq;
    use openbse_core::types::{DayType, TimeStep};
    use openbse_psychrometrics::{MoistAirState, STD_PRESSURE};

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
            outdoor_air: MoistAirState::from_tdb_rh(35.0, 0.20, STD_PRESSURE),
            day_type: DayType::WeatherDay,
            is_sizing: false,
            sizing_internal_gains: SizingInternalGains::Full,
        }
    }

    fn make_inlet(t_db: f64, rh: f64) -> AirPort {
        AirPort::new(MoistAirState::from_tdb_rh(t_db, rh, STD_PRESSURE), 1.0)
    }

    #[test]
    fn test_direct_cools_and_humidifies() {
        let mut ec = EvapCooler::new("EC1", EvapCoolerMode::Direct);
        let inlet = make_inlet(35.0, 0.15);
        let ctx = make_ctx();
        let outlet = ec.simulate_air(&inlet, &ctx);

        // DBT drops
        assert!(
            outlet.state.t_db < inlet.state.t_db,
            "direct: DBT should drop"
        );
        // Humidity rises
        assert!(outlet.state.w > inlet.state.w, "direct: W should rise");
        // Enthalpy approximately conserved (within 1%)
        let h_in = psych::h_fn_tdb_w(inlet.state.t_db, inlet.state.w);
        let h_out = psych::h_fn_tdb_w(outlet.state.t_db, outlet.state.w);
        assert_relative_eq!(h_in, h_out, max_relative = 0.01);
    }

    #[test]
    fn test_indirect_cools_no_humidity_change() {
        let mut ec = EvapCooler::new("EC2", EvapCoolerMode::Indirect);
        let inlet = make_inlet(35.0, 0.15);
        let ctx = make_ctx();
        let outlet = ec.simulate_air(&inlet, &ctx);

        assert!(
            outlet.state.t_db < inlet.state.t_db,
            "indirect: DBT should drop"
        );
        assert_relative_eq!(outlet.state.w, inlet.state.w, max_relative = 0.001);
    }

    #[test]
    fn test_two_stage_cools_more_than_either_alone() {
        let inlet = make_inlet(35.0, 0.15);
        let ctx = make_ctx();

        let mut direct = EvapCooler::new("D", EvapCoolerMode::Direct);
        let outlet_d = direct.simulate_air(&inlet, &ctx);

        let mut indirect = EvapCooler::new("I", EvapCoolerMode::Indirect);
        let outlet_i = indirect.simulate_air(&inlet, &ctx);

        let mut two = EvapCooler::new("2S", EvapCoolerMode::TwoStage);
        let outlet_2 = two.simulate_air(&inlet, &ctx);

        assert!(
            outlet_2.state.t_db < outlet_d.state.t_db,
            "two-stage should be cooler than direct alone"
        );
        assert!(
            outlet_2.state.t_db < outlet_i.state.t_db,
            "two-stage should be cooler than indirect alone"
        );
    }

    #[test]
    fn test_indirect_driven_by_secondary_not_primary() {
        // EVAP-1 (#59): indirect cooling must track the SECONDARY (outdoor) wet
        // bulb, not the primary inlet's. With a humid primary inlet but dry
        // outdoor (secondary) air, the achievable cooling is set by the dry
        // outdoor wet-bulb — much colder than the primary's own wet-bulb.
        let inlet = make_inlet(35.0, 0.60); // humid primary (high WB)
        let mut ctx = make_ctx();
        ctx.outdoor_air = MoistAirState::from_tdb_rh(35.0, 0.10, STD_PRESSURE); // dry secondary

        let mut ec = EvapCooler::new("EC", EvapCoolerMode::Indirect);
        let outlet = ec.simulate_air(&inlet, &ctx);

        // Floor = dry outdoor wet-bulb (~17.6 °C), far below the humid primary
        // wet-bulb (~28 °C). The old model would barely cool this humid inlet.
        let t_wb_secondary = psych::twb_fn_tdb_w_pb(35.0, ctx.outdoor_air.w, STD_PRESSURE);
        let t_wb_primary = psych::twb_fn_tdb_w_pb(35.0, inlet.state.w, STD_PRESSURE);
        assert!(
            t_wb_secondary < t_wb_primary - 5.0,
            "secondary should be much drier"
        );
        // Outlet must reflect the colder secondary-driven floor.
        let expected = 35.0 - 0.80 * 0.70 * (35.0 - t_wb_secondary);
        assert_relative_eq!(outlet.state.t_db, expected, max_relative = 0.01);
        // Humidity unchanged (indirect adds no moisture to the primary).
        assert_relative_eq!(outlet.state.w, inlet.state.w, max_relative = 1e-6);
    }

    #[test]
    fn test_indirect_no_cooling_when_secondary_warmer_than_primary() {
        // If the secondary wet-bulb is at/above the primary dry-bulb, no
        // sensible cooling is available.
        let inlet = make_inlet(18.0, 0.30); // cool primary
        let mut ctx = make_ctx();
        ctx.outdoor_air = MoistAirState::from_tdb_rh(35.0, 0.80, STD_PRESSURE); // hot humid secondary

        let mut ec = EvapCooler::new("EC", EvapCoolerMode::Indirect);
        let outlet = ec.simulate_air(&inlet, &ctx);
        assert_relative_eq!(outlet.state.t_db, 18.0, max_relative = 1e-6);
    }

    #[test]
    fn test_indirect_stages_in_series_when_primary_differs_from_secondary() {
        // EVAP-2 (#59): with return-like primary air at 30 °C and hot, dry
        // outdoor secondary air, the secondary pad acts first, then the HX.
        let mut ctx = make_ctx();
        ctx.outdoor_air = MoistAirState::from_tdb_rh(40.0, 0.10, STD_PRESSURE);
        let inlet = make_inlet(30.0, 0.30);
        let mut ec = EvapCooler::new("EC", EvapCoolerMode::Indirect);
        let outlet = ec.simulate_air(&inlet, &ctx);

        let oa = &ctx.outdoor_air;
        let t_wb_sec = psych::twb_fn_tdb_w_pb(oa.t_db, oa.w, oa.p_b);
        let t_sec_out = 40.0 - 0.80 * (40.0 - t_wb_sec);
        let expected = 30.0 - 0.70 * (30.0 - t_sec_out);
        assert_relative_eq!(outlet.state.t_db, expected, max_relative = 1e-3);
        // The product form would have used the primary's 30 °C against the
        // secondary wet-bulb and given a different answer.
        let product = 30.0 - 0.80 * 0.70 * (30.0 - t_wb_sec);
        assert!((expected - product).abs() > 0.1);
    }

    #[test]
    fn test_pump_and_secondary_fan_only_while_cooling() {
        let ctx = make_ctx(); // 35 °C, 20 % RH outdoors
        let mut ec = EvapCooler::new("EC", EvapCoolerMode::Indirect);
        ec.pump_power = 250.0;
        ec.secondary_fan_power = 400.0;
        ec.simulate_air(&make_inlet(35.0, 0.15), &ctx);
        assert_eq!(ec.power, 650.0);

        // Primary already colder than the cooled secondary air: nothing runs.
        ec.simulate_air(&make_inlet(15.0, 0.30), &ctx);
        assert_eq!(ec.power, 0.0);

        // A direct cooler never runs the secondary fan.
        let mut direct = EvapCooler::new("D", EvapCoolerMode::Direct);
        direct.pump_power = 250.0;
        direct.secondary_fan_power = 400.0;
        direct.simulate_air(&make_inlet(35.0, 0.15), &ctx);
        assert_eq!(direct.power, 250.0);
    }

    #[test]
    fn test_water_consumption() {
        let ctx = make_ctx();
        let inlet = make_inlet(35.0, 0.15);

        let mut direct = EvapCooler::new("D", EvapCoolerMode::Direct);
        let out = direct.simulate_air(&inlet, &ctx);
        assert_relative_eq!(
            direct.water_consumption,
            inlet.mass_flow * (out.state.w - inlet.state.w),
            max_relative = 1e-9
        );

        // Indirect: the supply gains no moisture; the secondary pad evaporates
        // water in proportion to the secondary flow.
        let mut ind = EvapCooler::new("I", EvapCoolerMode::Indirect);
        ind.secondary_air_flow_ratio = 2.0;
        ind.simulate_air(&inlet, &ctx);
        let oa = &ctx.outdoor_air;
        let (_, w_sec) = EvapCooler::direct_stage(oa.t_db, oa.w, oa.p_b, 0.80);
        assert_relative_eq!(
            ind.water_consumption,
            2.0 * inlet.mass_flow * (w_sec - oa.w),
            max_relative = 1e-9
        );
    }

    #[test]
    fn test_zero_flow_passthrough() {
        let mut ec = EvapCooler::new("EC3", EvapCoolerMode::Direct);
        let inlet = AirPort::new(MoistAirState::from_tdb_rh(30.0, 0.50, STD_PRESSURE), 0.0);
        let ctx = make_ctx();
        let outlet = ec.simulate_air(&inlet, &ctx);
        assert_relative_eq!(outlet.state.t_db, inlet.state.t_db, max_relative = 0.001);
        assert_eq!(ec.power, 0.0);
    }
}
