//! Chilled water cooling coil component model.
//!
//! On a plant loop the coil follows E+ `Coil:Cooling:Water` (SimpleAnalysis):
//! a counterflow coil, dry or wet by the entering dew point, sized so it
//! delivers its capacity at the design point, its valve holding the outlet
//! setpoint on the loop's water (see [`crate::water_coil`]). The leaving air
//! follows from the coil UA, so a wet coil leaves it short of saturation and
//! the coil's output follows the loop's supply temperature and water flow.
//!
//! Without a plant loop (allowed only in a model with no plant loops) it is
//! the idealised coil of the ASHRAE 140 §11 analytical cases: it cools to the
//! setpoint, limited by its capacity, and condenses down to saturation at the
//! leaving temperature when the entering air is wetter than that (the
//! quasi-analytical solution reports RHcco = 100% for every wet case).
//!
//! Either way the total cooling is the air's enthalpy drop, and on a plant
//! loop it is also the water's heat gain.

use crate::water_coil::{CoolingDesign, CoolingResult, CoolingUa};
use openbse_core::ports::*;
use openbse_psychrometrics::{self as psych, FluidState};
use serde::{Deserialize, Serialize};

fn default_submeter() -> String {
    "General".to_string()
}

/// E+ rated entering air for water cooling coils (26.67 °C dry-bulb,
/// 19.44 °C wet-bulb), used when sizing gives the coil no design air.
const RATED_AIR_DB: f64 = 26.67;
const RATED_AIR_WB: f64 = 19.44;

/// Chilled water cooling coil component.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoolingCoilCHW {
    pub name: String,
    #[serde(default = "default_submeter")]
    pub submeter: String,
    /// Nominal total cooling capacity [W] at the design point.
    /// Use AUTOSIZE for autosizing from the air loop's design load.
    pub nominal_capacity: f64,
    /// Desired outlet air temperature setpoint [°C]
    pub outlet_temp_setpoint: f64,

    // ─── Water-side design overrides (otherwise from the plant loop) ─────
    /// Design (maximum) chilled-water flow [m³/s]; 0 sizes it from the
    /// capacity and the design temperature rise.
    pub design_water_flow_rate: f64,
    /// Design chilled-water inlet (supply) temperature [°C]
    pub design_water_inlet_temp: Option<f64>,
    /// Design chilled-water outlet (return) temperature [°C]
    pub design_water_outlet_temp: Option<f64>,
    /// Design air flow [m³/s] when sizing gives none; 0 derives it.
    pub rated_air_flow: f64,
    /// Water side on a plant loop, once sized.
    #[serde(default)]
    pub water_ua: Option<CoolingUa>,

    // ─── Runtime state ───────────────────────────────────────────────────
    /// Total cooling rate delivered to air [W] (positive = cooling)
    #[serde(skip)]
    pub cooling_rate: f64,
    /// Sensible cooling rate [W]
    #[serde(skip)]
    pub sensible_cooling_rate: f64,

    #[serde(skip)]
    water_inlet: Option<WaterPort>,
    #[serde(skip)]
    water_outlet_state: Option<WaterPort>,
}

impl CoolingCoilCHW {
    /// Create a chilled water cooling coil. It runs as the ideal coil until
    /// [`AirComponent::size_water_side`] connects it to its plant loop.
    pub fn new(name: &str, capacity: f64, setpoint: f64) -> Self {
        Self {
            name: name.to_string(),
            submeter: "General".to_string(),
            nominal_capacity: capacity,
            outlet_temp_setpoint: setpoint,
            design_water_flow_rate: 0.0,
            design_water_inlet_temp: None,
            design_water_outlet_temp: None,
            rated_air_flow: 0.0,
            water_ua: None,
            cooling_rate: 0.0,
            sensible_cooling_rate: 0.0,
            water_inlet: None,
            water_outlet_state: None,
        }
    }

    /// The idealised coil without a plant loop.
    fn ideal(&self, inlet: &AirPort, cp_air: f64) -> CoolingResult {
        let p_b = inlet.state.p_b;
        let t_set = self.outlet_temp_setpoint;
        let w_out_set = inlet.state.w.min(psych::w_fn_tdb_rh_pb(t_set, 1.0, p_b));
        let h_in = psych::h_fn_tdb_w(inlet.state.t_db, inlet.state.w);
        let q_sens_set = inlet.mass_flow * cp_air * (inlet.state.t_db - t_set);
        let q_total_set =
            (inlet.mass_flow * (h_in - psych::h_fn_tdb_w(t_set, w_out_set))).max(q_sens_set);
        // Short of capacity, scale sensible and latent together (time-averaged
        // part load) so the air's enthalpy drop stays the total.
        let plr = (self.nominal_capacity.max(0.0) / q_total_set).min(1.0);
        let q_total = q_total_set * plr;
        let w_out = (inlet.state.w - plr * (inlet.state.w - w_out_set)).max(1.0e-5);
        let t_out = psych::tdb_fn_h_w(h_in - q_total / inlet.mass_flow, w_out);
        CoolingResult {
            t_air_out: t_out,
            w_air_out: w_out,
            q_total,
            q_sensible: inlet.mass_flow * cp_air * (inlet.state.t_db - t_out),
        }
    }

    /// The coil's design point: the loop's design water (or the overrides)
    /// and the air sizing found, else E+'s rated air.
    fn cooling_design(&self, d: &WaterCoilDesign) -> CoolingDesign {
        let q_total = self.nominal_capacity.max(0.0);
        let t_water_in = self.design_water_inlet_temp.unwrap_or(d.t_water_in);
        let rise = self
            .design_water_outlet_temp
            .map_or(d.delta_t, |t| t - t_water_in)
            .max(0.1);
        let water_flow = if self.design_water_flow_rate > 0.0 {
            self.design_water_flow_rate * psych::water_rho(t_water_in)
        } else {
            q_total / (psych::water_cp(t_water_in) * rise)
        };
        let (t_air_in, w_air_in) = d.air_in.unwrap_or((
            RATED_AIR_DB,
            psych::w_fn_tdb_twb_pb(RATED_AIR_DB, RATED_AIR_WB, d.p_b),
        ));
        let air_flow = match d.air_flow {
            Some(m) if m > 0.0 => m,
            _ if self.rated_air_flow > 0.0 => {
                self.rated_air_flow * psych::rho_air_fn_pb_tdb_w(d.p_b, t_air_in, w_air_in)
            }
            // Air that the capacity brings to the setpoint at 90% RH, as E+
            // resets an unattainable design outlet.
            _ => {
                let t_out = self.outlet_temp_setpoint.min(t_air_in - 1.0);
                let w_out = w_air_in.min(psych::w_fn_tdb_rh_pb(t_out, 0.9, d.p_b));
                let dh = psych::h_fn_tdb_w(t_air_in, w_air_in) - psych::h_fn_tdb_w(t_out, w_out);
                q_total / dh.max(1000.0)
            }
        };
        CoolingDesign {
            q_total,
            air_flow,
            t_air_in,
            w_air_in,
            water_flow,
            t_water_in,
            p_b: d.p_b,
        }
    }
}

impl AirComponent for CoolingCoilCHW {
    fn name(&self) -> &str {
        &self.name
    }

    fn component_kind(&self) -> ComponentKind {
        ComponentKind::CoolingCoil
    }

    fn simulate_air(&mut self, inlet: &AirPort, _ctx: &SimulationContext) -> AirPort {
        self.cooling_rate = 0.0;
        self.sensible_cooling_rate = 0.0;
        self.water_outlet_state = None;
        if inlet.mass_flow <= 0.0 || inlet.state.t_db <= self.outlet_temp_setpoint {
            return *inlet;
        }

        let cp_air = psych::cp_air_fn_w(inlet.state.w);
        let result = match (self.water_ua.as_ref(), self.water_inlet.as_ref()) {
            (Some(ua), Some(wi)) => {
                let (r, flow) = ua.control(
                    self.outlet_temp_setpoint,
                    inlet.mass_flow,
                    &inlet.state,
                    wi.state.mass_flow,
                    wi.state.temp,
                );
                let t_out = if flow > 0.0 {
                    wi.state.temp + r.q_total / (flow * psych::water_cp(wi.state.temp))
                } else {
                    wi.state.temp
                };
                self.water_outlet_state = Some(WaterPort::new(FluidState::water(t_out, flow)));
                r
            }
            _ => self.ideal(inlet, cp_air),
        };

        self.cooling_rate = result.q_total;
        self.sensible_cooling_rate = result.q_sensible;
        AirPort::new(
            psych::MoistAirState::new(result.t_air_out, result.w_air_out, inlet.state.p_b),
            inlet.mass_flow,
        )
    }

    fn has_water_side(&self) -> bool {
        true
    }

    fn set_water_inlet(&mut self, inlet: &WaterPort) {
        self.water_inlet = Some(*inlet);
    }

    fn water_outlet(&self) -> Option<WaterPort> {
        self.water_outlet_state
    }

    fn size_water_side(&mut self, design: &WaterCoilDesign) -> Option<String> {
        let d = self.cooling_design(design);
        let ua = CoolingUa::size(&d);
        self.water_ua = Some(ua);
        (!ua.reaches_design && d.q_total > 0.0).then(|| {
            format!(
                "Chilled-water coil '{}': {:.0} W cannot be reached cooling {:.3} kg/s of air \
                 from {:.1} °C on {:.1} °C water; the coil delivers less than its capacity",
                self.name, d.q_total, d.air_flow, d.t_air_in, d.t_water_in
            )
        })
    }

    fn design_water_flow(&self) -> Option<f64> {
        self.water_ua.map(|ua| ua.water_flow_design)
    }

    fn design_air_flow_rate(&self) -> Option<f64> {
        None // Coils don't set air flow rate
    }

    fn set_setpoint(&mut self, setpoint: f64) {
        self.outlet_temp_setpoint = setpoint;
    }

    fn setpoint(&self) -> Option<f64> {
        Some(self.outlet_temp_setpoint)
    }

    fn nominal_capacity(&self) -> Option<f64> {
        Some(self.nominal_capacity)
    }

    fn set_nominal_capacity(&mut self, cap: f64) {
        self.nominal_capacity = cap;
    }

    fn unsized_fields(&self) -> Vec<&'static str> {
        openbse_core::types::autosize_fields([("capacity", self.nominal_capacity)])
    }

    fn power_consumption(&self) -> f64 {
        0.0 // CHW coils don't consume electricity (pumps and chillers do)
    }

    fn thermal_output(&self) -> f64 {
        // Negative = cooling demand (convention: positive = heating, negative = cooling)
        -self.cooling_rate
    }

    fn report_outputs(&self, out: &mut dyn FnMut(&str, f64)) {
        out("sensible_load", self.sensible_cooling_rate);
        out("total_load", self.cooling_rate);
        out(
            "latent_load",
            (self.cooling_rate - self.sensible_cooling_rate).max(0.0),
        );
        if self.water_ua.is_some() {
            if let Some(ref wi) = self.water_inlet {
                out("water_inlet_temperature", wi.state.temp);
            }
            let flow = self.water_outlet_state.map_or(0.0, |w| w.state.mass_flow);
            out("water_mass_flow", flow);
            if let Some(ref wo) = self.water_outlet_state {
                out("water_outlet_temperature", wo.state.temp);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_relative_eq;
    use openbse_core::types::{DayType, TimeStep};
    use openbse_psychrometrics::MoistAirState;

    const P: f64 = 101325.0;

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
            outdoor_air: MoistAirState::from_tdb_rh(35.0, 0.4, P),
            day_type: DayType::WeatherDay,
            is_sizing: false,
            sizing_internal_gains: SizingInternalGains::Full,
        }
    }

    /// A coil sized for 1 kg/s from 26 °C / 50% RH to 13 °C on a 6.7 °C,
    /// 5.6 K loop, through the path the plant loop uses.
    fn plant_coil(setpoint: f64) -> (CoolingCoilCHW, AirPort, f64) {
        let inlet = AirPort::new(MoistAirState::from_tdb_rh(26.0, 0.5, P), 1.0);
        let q = psych::h_fn_tdb_w(26.0, inlet.state.w) - psych::h_fn_tdb_w(13.0, 0.0085);
        let mut coil = CoolingCoilCHW::new("CHW", q, setpoint);
        let design = WaterCoilDesign {
            t_water_in: 6.7,
            delta_t: 5.6,
            air_flow: Some(1.0),
            air_in: Some((26.0, inlet.state.w)),
            p_b: P,
        };
        assert!(coil.size_water_side(&design).is_none());
        let m_w = coil.design_water_flow().unwrap();
        coil.set_water_inlet(&WaterPort::new(FluidState::water(6.7, m_w)));
        (coil, inlet, m_w)
    }

    fn enthalpy_drop(inlet: &AirPort, outlet: &AirPort) -> f64 {
        inlet.mass_flow
            * (psych::h_fn_tdb_w(inlet.state.t_db, inlet.state.w)
                - psych::h_fn_tdb_w(outlet.state.t_db, outlet.state.w))
    }

    #[test]
    fn test_plant_coil_holds_setpoint_short_of_saturation_and_balances() {
        let (mut coil, inlet, m_w) = plant_coil(14.0);
        let outlet = coil.simulate_air(&inlet, &make_ctx());
        assert_relative_eq!(outlet.state.t_db, 14.0, epsilon = 0.01);
        // A real coil leaves the air short of saturation.
        let rh = psych::rh_fn_tdb_w_pb(outlet.state.t_db, outlet.state.w, P);
        assert!(rh < 0.99 && outlet.state.w < inlet.state.w, "RH {rh}");
        // Air enthalpy drop = water gain = -thermal_output.
        let wo = coil.water_outlet().unwrap();
        assert!(wo.state.mass_flow < m_w);
        let water = wo.state.mass_flow * psych::water_cp(6.7) * (wo.state.temp - 6.7);
        assert_relative_eq!(enthalpy_drop(&inlet, &outlet), -coil.thermal_output());
        assert_relative_eq!(water, coil.cooling_rate, max_relative = 1e-9);
    }

    #[test]
    fn test_plant_coil_follows_supply_temperature_and_flow() {
        // A warmer supply or a starved branch reaches the air side.
        let (mut coil, inlet, m_w) = plant_coil(10.0);
        let mut q_at = |t, m| {
            coil.set_water_inlet(&WaterPort::new(FluidState::water(t, m)));
            coil.simulate_air(&inlet, &make_ctx());
            coil.cooling_rate
        };
        let design = q_at(6.7, m_w);
        assert!(q_at(10.0, m_w) < 0.8 * design);
        assert!(q_at(6.7, 0.3 * m_w) < design);
        assert_eq!(q_at(6.7, 0.0), 0.0);
    }

    #[test]
    fn test_ideal_coil_without_plant_condenses_to_saturation() {
        // ASHRAE 140 §11 behaviour, kept for coils with no plant loop.
        let mut coil = CoolingCoilCHW::new("CHW", 50_000.0, 13.0);
        let inlet = AirPort::new(MoistAirState::from_tdb_rh(27.0, 0.6, P), 1.0);
        let outlet = coil.simulate_air(&inlet, &make_ctx());
        assert_relative_eq!(outlet.state.t_db, 13.0, epsilon = 1e-9);
        assert_relative_eq!(outlet.state.w, psych::w_fn_tdb_rh_pb(13.0, 1.0, P));
        assert_relative_eq!(enthalpy_drop(&inlet, &outlet), coil.cooling_rate);
        assert!(coil.water_outlet().is_none());
    }

    #[test]
    fn test_ideal_coil_capacity_limited_and_no_heating() {
        let mut coil = CoolingCoilCHW::new("CHW", 1000.0, 13.0);
        let inlet = AirPort::new(MoistAirState::from_tdb_rh(35.0, 0.4, P), 1.0);
        let outlet = coil.simulate_air(&inlet, &make_ctx());
        assert!(outlet.state.t_db > 13.0 && outlet.state.t_db < 35.0);
        assert_relative_eq!(coil.cooling_rate, 1000.0, max_relative = 1e-3);
        let cold = AirPort::new(MoistAirState::from_tdb_rh(10.0, 0.5, P), 1.0);
        let outlet = coil.simulate_air(&cold, &make_ctx());
        assert_eq!(outlet.state.t_db, 10.0);
        assert_eq!(coil.cooling_rate, 0.0);
    }

    #[test]
    fn test_design_overrides_set_the_water_flow() {
        let mut coil = CoolingCoilCHW::new("CHW", 20_000.0, 13.0);
        coil.design_water_inlet_temp = Some(5.0);
        coil.design_water_outlet_temp = Some(12.0);
        let design = WaterCoilDesign {
            t_water_in: 7.0,
            delta_t: 5.0,
            air_flow: None,
            air_in: None,
            p_b: P,
        };
        coil.size_water_side(&design);
        assert_relative_eq!(
            coil.design_water_flow().unwrap(),
            20_000.0 / (psych::water_cp(5.0) * 7.0),
            max_relative = 1e-9
        );
        coil.design_water_flow_rate = 0.001;
        coil.size_water_side(&design);
        assert_relative_eq!(
            coil.design_water_flow().unwrap(),
            0.001 * psych::water_rho(5.0),
            max_relative = 1e-9
        );
    }

    #[test]
    fn test_chw_coil_zero_flow_and_power() {
        let mut coil = CoolingCoilCHW::new("CHW", 50_000.0, 13.0);
        let inlet = AirPort::new(MoistAirState::from_tdb_rh(25.0, 0.5, P), 0.0);
        coil.simulate_air(&inlet, &make_ctx());
        assert_eq!(coil.cooling_rate, 0.0);
        assert_eq!(coil.power_consumption(), 0.0);
    }
}
