//! Heating coil component models.
//!
//! Electric, gas and hot-water coils. Electric and gas coils match
//! EnergyPlus `HeatingCoils.cc`: Q = min(load, capacity), with the input
//! efficiency.
//!
//! A hot-water coil on a plant loop follows E+ `Coil:Heating:Water`
//! (`WaterCoils.cc`): a cross-flow ε-NTU coil sized so it delivers its
//! capacity at the design point, its valve holding the outlet setpoint on the
//! loop's water (see [`crate::water_coil`]). Without a plant loop (allowed only
//! in a model with no plant loops) it is an ideal coil limited by capacity.

use crate::water_coil::{HeatingDesign, HeatingUa};
use openbse_core::ports::*;
use openbse_psychrometrics::{self as psych, FluidState};
use serde::{Deserialize, Serialize};

fn default_submeter() -> String {
    "General".to_string()
}

/// Heating coil type.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum HeatingCoilType {
    /// Simple electric resistance coil
    Electric,
    /// Hot water coil connected to a plant loop
    HotWater,
    /// Gas furnace (natural gas burner)
    Gas,
}

/// Rated conditions of a hot-water coil (E+ `Coil:Heating:Water` "Nominal
/// Capacity" input method). Any value left `None` comes from the coil's
/// design point: the loop's design supply temperature and temperature
/// difference, and the air its sizing found.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct HotWaterRating {
    /// Rated water inlet temperature [°C]
    pub water_inlet_temp: Option<f64>,
    /// Rated water outlet temperature [°C]
    pub water_outlet_temp: Option<f64>,
    /// Rated air inlet temperature [°C]
    pub air_inlet_temp: Option<f64>,
    /// Rated air outlet temperature [°C]
    pub air_outlet_temp: Option<f64>,
}

/// E+ `Coil:Heating:Water` rated air temperatures, used when neither the
/// rating nor sizing gives the coil's air.
const RATED_AIR_INLET: f64 = 16.6;
const RATED_AIR_OUTLET: f64 = 32.2;

/// Heating coil component.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeatingCoil {
    pub name: String,
    pub coil_type: HeatingCoilType,
    #[serde(default = "default_submeter")]
    pub submeter: String,
    /// Nominal heating capacity [W]. Use AUTOSIZE for autosizing.
    pub nominal_capacity: f64,
    /// Efficiency [0-1]: burner efficiency of a gas coil, conversion
    /// efficiency of an electric coil; 1.0 for hot water.
    pub efficiency: f64,
    /// Desired outlet air temperature setpoint [°C]
    pub outlet_temp_setpoint: f64,

    /// Rated conditions of a hot-water coil.
    #[serde(default)]
    pub rating: HotWaterRating,
    /// Water side of a hot-water coil on a plant loop, once sized.
    #[serde(default)]
    pub water_ua: Option<HeatingUa>,

    /// On-cycle parasitic electric load [W] of a gas coil (draft / inducer
    /// fan). Drawn in proportion to burner firing; its heat leaves with the
    /// flue gas, so it never reaches the supply air. Default 0.0.
    #[serde(default)]
    pub parasitic_electric_load: f64,

    // ─── Runtime state ──────────────────────────────────────────────────
    #[serde(skip)]
    pub heating_rate: f64,
    #[serde(skip)]
    pub energy_consumption: f64,
    #[serde(skip)]
    water_inlet: Option<WaterPort>,
    #[serde(skip)]
    water_outlet: Option<WaterPort>,
}

impl HeatingCoil {
    fn new(name: &str, coil_type: HeatingCoilType, capacity: f64, setpoint: f64) -> Self {
        Self {
            name: name.to_string(),
            coil_type,
            submeter: "General".to_string(),
            nominal_capacity: capacity,
            efficiency: 1.0,
            outlet_temp_setpoint: setpoint,
            rating: HotWaterRating::default(),
            water_ua: None,
            parasitic_electric_load: 0.0,
            heating_rate: 0.0,
            energy_consumption: 0.0,
            water_inlet: None,
            water_outlet: None,
        }
    }

    /// Create an electric resistance heating coil.
    pub fn electric(name: &str, capacity: f64, setpoint: f64) -> Self {
        Self::new(name, HeatingCoilType::Electric, capacity, setpoint)
    }

    /// Create a gas furnace heating coil.
    ///
    /// Gas coils have a burner efficiency (typically 0.78-0.92 for furnaces).
    /// The coil delivers capacity to the air, but consumes capacity/efficiency
    /// worth of fuel energy.
    pub fn gas(name: &str, capacity: f64, setpoint: f64, burner_efficiency: f64) -> Self {
        let mut coil = Self::new(name, HeatingCoilType::Gas, capacity, setpoint);
        coil.efficiency = burner_efficiency;
        coil
    }

    /// Create a hot-water heating coil. It runs as an ideal coil until
    /// [`AirComponent::size_water_side`] connects it to its plant loop.
    pub fn hot_water(name: &str, capacity: f64, setpoint: f64) -> Self {
        Self::new(name, HeatingCoilType::HotWater, capacity, setpoint)
    }

    /// Size the water side from a design point (see [`HeatingUa::size`]).
    pub fn with_water_design(mut self, design: &HeatingDesign) -> Self {
        self.water_ua = Some(HeatingUa::size(design, psych::cp_air_fn_w(0.008)));
        self
    }

    fn simulate_hot_water(&mut self, inlet: &AirPort, cp_air: f64, q_required: f64) -> AirPort {
        self.energy_consumption = 0.0;
        self.water_outlet = None;
        let q_actual = match (self.water_ua.as_ref(), self.water_inlet.as_ref()) {
            (Some(ua), Some(wi)) => {
                let (q, flow) = ua.control(
                    q_required,
                    inlet.mass_flow,
                    cp_air,
                    inlet.state.t_db,
                    wi.state.mass_flow,
                    wi.state.temp,
                );
                let t_out = if flow > 0.0 {
                    wi.state.temp - q / (flow * psych::water_cp(wi.state.temp))
                } else {
                    wi.state.temp
                };
                self.water_outlet = Some(WaterPort::new(FluidState::water(t_out, flow)));
                q
            }
            // No plant loop: an ideal coil.
            _ => q_required.min(self.nominal_capacity),
        };
        self.heating_rate = q_actual;
        let outlet_t = inlet.state.t_db + q_actual / (inlet.mass_flow * cp_air);
        AirPort::new(
            psych::MoistAirState::new(outlet_t, inlet.state.w, inlet.state.p_b),
            inlet.mass_flow,
        )
    }

    /// The design point of a hot-water coil of capacity `q`: the rating where
    /// given, else the loop's design water and the air sizing found.
    fn heating_design(&self, q: f64, d: &WaterCoilDesign) -> HeatingDesign {
        let cp_air = psych::cp_air_fn_w(0.008);
        let t_water_in = self.rating.water_inlet_temp.unwrap_or(d.t_water_in);
        let t_water_out = self
            .rating
            .water_outlet_temp
            .unwrap_or(t_water_in - d.delta_t);
        let t_air_in = self
            .rating
            .air_inlet_temp
            .or(d.air_in.map(|a| a.0))
            .unwrap_or(RATED_AIR_INLET);
        let air_flow = match (self.rating.air_outlet_temp, d.air_flow) {
            (Some(t_out), _) => q / (cp_air * (t_out - t_air_in).max(0.1)),
            (None, Some(m)) if m > 0.0 => m,
            _ => q / (cp_air * (RATED_AIR_OUTLET - RATED_AIR_INLET)),
        };
        HeatingDesign {
            q,
            air_flow,
            t_air_in,
            water_flow: q / (psych::water_cp(t_water_in) * (t_water_in - t_water_out).max(0.1)),
            t_water_in,
        }
    }
}

impl AirComponent for HeatingCoil {
    fn name(&self) -> &str {
        &self.name
    }

    fn component_kind(&self) -> ComponentKind {
        ComponentKind::HeatingCoil
    }

    fn simulate_air(&mut self, inlet: &AirPort, _ctx: &SimulationContext) -> AirPort {
        if inlet.mass_flow <= 0.0 {
            self.heating_rate = 0.0;
            self.energy_consumption = 0.0;
            return *inlet;
        }

        let cp_air = psych::cp_air_fn_w(inlet.state.w);

        // Calculate required heating to reach setpoint
        let q_required = inlet.mass_flow * cp_air * (self.outlet_temp_setpoint - inlet.state.t_db);

        // Only heat, don't cool
        let q_required = q_required.max(0.0);

        // Guard against AUTOSIZE sentinel (-99999) that was never resolved
        if self.nominal_capacity < 0.0 {
            self.heating_rate = 0.0;
            self.energy_consumption = 0.0;
            return *inlet;
        }

        match self.coil_type {
            HeatingCoilType::Electric => {
                // Limit by capacity
                let q_actual = q_required.min(self.nominal_capacity);
                let outlet_t = inlet.state.t_db + q_actual / (inlet.mass_flow * cp_air);

                self.heating_rate = q_actual;
                self.energy_consumption = q_actual / self.efficiency;

                AirPort::new(
                    psych::MoistAirState::new(outlet_t, inlet.state.w, inlet.state.p_b),
                    inlet.mass_flow,
                )
            }
            HeatingCoilType::Gas => {
                // Gas furnace: same as electric but with burner efficiency
                let q_actual = q_required.min(self.nominal_capacity);
                let outlet_t = inlet.state.t_db + q_actual / (inlet.mass_flow * cp_air);

                self.heating_rate = q_actual;
                // Gas consumption = delivered heat / burner efficiency
                self.energy_consumption = if self.efficiency > 0.0 {
                    q_actual / self.efficiency
                } else {
                    q_actual
                };

                AirPort::new(
                    psych::MoistAirState::new(outlet_t, inlet.state.w, inlet.state.p_b),
                    inlet.mass_flow,
                )
            }
            HeatingCoilType::HotWater => self.simulate_hot_water(inlet, cp_air, q_required),
        }
    }

    fn has_water_side(&self) -> bool {
        matches!(self.coil_type, HeatingCoilType::HotWater)
    }

    fn set_water_inlet(&mut self, inlet: &WaterPort) {
        self.water_inlet = Some(*inlet);
    }

    fn water_outlet(&self) -> Option<WaterPort> {
        self.water_outlet
    }

    fn size_water_side(&mut self, design: &WaterCoilDesign) -> Option<String> {
        if !matches!(self.coil_type, HeatingCoilType::HotWater) {
            return None;
        }
        let d = self.heating_design(self.nominal_capacity.max(0.0), design);
        let ua = HeatingUa::size(&d, psych::cp_air_fn_w(0.008));
        self.water_ua = Some(ua);
        (!ua.reaches_design && d.q > 0.0).then(|| {
            format!(
                "Hot-water coil '{}': {:.0} W cannot be reached heating {:.3} kg/s of air \
                 from {:.1} °C on {:.1} °C water; the coil delivers less than its capacity",
                self.name, d.q, d.air_flow, d.t_air_in, d.t_water_in
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
        match self.coil_type {
            HeatingCoilType::Electric => self.energy_consumption,
            // E+ Coil:Heating:Fuel on-cycle parasitic electric load: scales
            // with the burner firing fraction (load / nominal capacity).
            HeatingCoilType::Gas
                if self.parasitic_electric_load > 0.0 && self.nominal_capacity > 0.0 =>
            {
                self.parasitic_electric_load
                    * (self.heating_rate / self.nominal_capacity).clamp(0.0, 1.0)
            }
            _ => 0.0, // gas/HW coils don't consume electricity
        }
    }

    fn fuel_consumption(&self) -> f64 {
        match self.coil_type {
            HeatingCoilType::Gas => self.energy_consumption,
            _ => 0.0,
        }
    }

    fn thermal_output(&self) -> f64 {
        self.heating_rate
    }

    fn report_outputs(&self, out: &mut dyn FnMut(&str, f64)) {
        out("sensible_load", self.heating_rate);
        if self.water_ua.is_some() {
            let flow = self.water_outlet.map_or(0.0, |w| w.state.mass_flow);
            out("water_mass_flow", flow);
            if let Some(w) = self.water_outlet {
                out("water_outlet_temperature", w.state.temp);
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

    fn make_ctx() -> SimulationContext {
        SimulationContext {
            timestep: TimeStep {
                month: 1,
                day: 1,
                hour: 12,
                sub_hour: 1,
                timesteps_per_hour: 1,
                sim_time_s: 0.0,
                dt: 3600.0,
            },
            outdoor_air: MoistAirState::from_tdb_rh(0.0, 0.5, 101325.0),
            day_type: DayType::WeatherDay,
            is_sizing: false,
            sizing_internal_gains: SizingInternalGains::Full,
        }
    }

    #[test]
    fn test_electric_coil_heats_to_setpoint() {
        let mut coil = HeatingCoil::electric("Test Coil", 50000.0, 35.0);
        let inlet_state = MoistAirState::from_tdb_rh(10.0, 0.5, 101325.0);
        let inlet = AirPort::new(inlet_state, 1.0);
        let ctx = make_ctx();

        let outlet = coil.simulate_air(&inlet, &ctx);

        // Should reach setpoint (coil has enough capacity)
        assert_relative_eq!(outlet.state.t_db, 35.0, max_relative = 0.01);
        assert!(coil.heating_rate > 0.0);
    }

    #[test]
    fn test_electric_coil_capacity_limited() {
        // Very small coil capacity
        let mut coil = HeatingCoil::electric("Small Coil", 1000.0, 35.0);
        let inlet_state = MoistAirState::from_tdb_rh(10.0, 0.5, 101325.0);
        let inlet = AirPort::new(inlet_state, 1.0);
        let ctx = make_ctx();

        let outlet = coil.simulate_air(&inlet, &ctx);

        // Should NOT reach setpoint — capacity limited
        assert!(outlet.state.t_db < 35.0);
        assert!(outlet.state.t_db > 10.0);
        assert_relative_eq!(coil.heating_rate, 1000.0, max_relative = 0.001);
    }

    #[test]
    fn test_coil_no_cooling() {
        // If inlet is already above setpoint, coil should not cool
        let mut coil = HeatingCoil::electric("Test Coil", 50000.0, 20.0);
        let inlet_state = MoistAirState::from_tdb_rh(25.0, 0.5, 101325.0);
        let inlet = AirPort::new(inlet_state, 1.0);
        let ctx = make_ctx();

        let outlet = coil.simulate_air(&inlet, &ctx);

        assert_relative_eq!(outlet.state.t_db, 25.0, max_relative = 0.001);
        assert_eq!(coil.heating_rate, 0.0);
    }

    #[test]
    fn test_gas_coil_parasitic_electric_follows_firing_fraction() {
        let ctx = make_ctx();
        let inlet = AirPort::new(MoistAirState::from_tdb_rh(10.0, 0.5, 101325.0), 1.0);

        // Default: a gas coil draws no electricity.
        let mut plain = HeatingCoil::gas("Furnace", 10000.0, 50.0, 0.8);
        plain.simulate_air(&inlet, &ctx);
        assert_eq!(plain.power_consumption(), 0.0);

        // Capacity-limited (full fire): the whole parasitic load is drawn,
        // and fuel use and delivered heat are unchanged by it.
        let mut full = HeatingCoil::gas("Furnace", 10000.0, 50.0, 0.8);
        full.parasitic_electric_load = 50.0;
        let outlet = full.simulate_air(&inlet, &ctx);
        assert_relative_eq!(full.heating_rate, 10000.0, max_relative = 1e-9);
        assert_relative_eq!(full.power_consumption(), 50.0, max_relative = 1e-9);
        assert_eq!(full.fuel_consumption(), plain.fuel_consumption());
        assert_eq!(outlet.state.t_db, {
            let mut p = HeatingCoil::gas("Furnace", 10000.0, 50.0, 0.8);
            p.simulate_air(&inlet, &ctx).state.t_db
        });

        // Setpoint-limited (part fire): scales with load / capacity.
        let mut part = HeatingCoil::gas("Furnace", 10000.0, 12.0, 0.8);
        part.parasitic_electric_load = 50.0;
        part.simulate_air(&inlet, &ctx);
        let fraction = part.heating_rate / 10000.0;
        assert!(fraction > 0.0 && fraction < 1.0);
        assert_relative_eq!(
            part.power_consumption(),
            50.0 * fraction,
            max_relative = 1e-9
        );

        // Idle: nothing drawn.
        let warm = AirPort::new(MoistAirState::from_tdb_rh(30.0, 0.5, 101325.0), 1.0);
        part.simulate_air(&warm, &ctx);
        assert_eq!(part.power_consumption(), 0.0);
    }

    // ─── Hot-water coil on a plant loop ──────────────────────────────

    /// A 10 kW coil sized at the E+ rated point (82.2/71.1 °C water,
    /// 16.6/32.2 °C air) through the same path a plant loop uses.
    fn rated_coil(setpoint: f64) -> (HeatingCoil, f64, f64) {
        let design = WaterCoilDesign {
            t_water_in: 82.2,
            delta_t: 82.2 - 71.1,
            air_flow: None,
            air_in: Some((16.6, 0.008)),
            p_b: 101325.0,
        };
        let mut coil = HeatingCoil::hot_water("HW", 10_000.0, setpoint);
        assert!(coil.size_water_side(&design).is_none());
        let cp_air = psych::cp_air_fn_w(0.008);
        let m_air = 10_000.0 / (cp_air * (32.2 - 16.6));
        let m_water = coil.design_water_flow().unwrap();
        (coil, m_air, m_water)
    }

    #[test]
    fn test_hot_water_coil_delivers_capacity_at_its_design_point() {
        let (mut coil, m_air, m_water) = rated_coil(60.0);
        coil.set_water_inlet(&WaterPort::new(FluidState::water(82.2, m_water)));
        let inlet = AirPort::new(MoistAirState::new(16.6, 0.008, 101325.0), m_air);
        let outlet = coil.simulate_air(&inlet, &make_ctx());
        // The UA reproduces the rated point.
        assert_relative_eq!(coil.heating_rate, 10_000.0, max_relative = 1e-3);
        assert_relative_eq!(outlet.state.t_db, 32.2, epsilon = 0.02);
    }

    #[test]
    fn test_hot_water_coil_follows_loop_temperature() {
        // A 60 °C loop does not heat as well as an 82 °C one.
        let (mut coil, m_air, m_water) = rated_coil(60.0);
        let inlet = AirPort::new(MoistAirState::new(16.6, 0.008, 101325.0), m_air);
        let mut q_at = |t| {
            coil.set_water_inlet(&WaterPort::new(FluidState::water(t, m_water)));
            coil.simulate_air(&inlet, &make_ctx());
            coil.heating_rate
        };
        let (q82, q60, q50) = (q_at(82.2), q_at(60.0), q_at(50.0));
        assert!(
            q60 < 0.75 * q82 && q50 < q60 && q50 > 0.0,
            "{q82} {q60} {q50}"
        );
    }

    #[test]
    fn test_hot_water_coil_holds_setpoint_on_less_water_and_balances() {
        let (mut coil, m_air, m_water) = rated_coil(25.0);
        coil.set_water_inlet(&WaterPort::new(FluidState::water(82.2, m_water)));
        let inlet = AirPort::new(MoistAirState::new(16.6, 0.008, 101325.0), m_air);
        let outlet = coil.simulate_air(&inlet, &make_ctx());
        assert_relative_eq!(outlet.state.t_db, 25.0, epsilon = 1e-6);
        let wo = coil.water_outlet().expect("water outlet");
        assert!(wo.state.mass_flow < 0.5 * m_water);
        // Air gain = water loss = thermal_output: the plant sees what the air got.
        let air = m_air * psych::cp_air_fn_w(0.008) * (outlet.state.t_db - 16.6);
        let water = wo.state.mass_flow * psych::water_cp(82.2) * (82.2 - wo.state.temp);
        assert_relative_eq!(air, coil.thermal_output(), max_relative = 1e-9);
        assert_relative_eq!(water, coil.thermal_output(), max_relative = 1e-9);
        let mut flow = None;
        coil.report_outputs(&mut |k, v| {
            if k == "water_mass_flow" {
                flow = Some(v)
            }
        });
        assert_eq!(flow, Some(wo.state.mass_flow));
    }

    #[test]
    fn test_hot_water_coil_starved_of_water_heats_less() {
        let (mut coil, m_air, m_water) = rated_coil(60.0);
        let inlet = AirPort::new(MoistAirState::new(16.6, 0.008, 101325.0), m_air);
        coil.set_water_inlet(&WaterPort::new(FluidState::water(82.2, 0.0)));
        coil.simulate_air(&inlet, &make_ctx());
        assert_eq!(coil.heating_rate, 0.0);
        coil.set_water_inlet(&WaterPort::new(FluidState::water(82.2, 0.3 * m_water)));
        coil.simulate_air(&inlet, &make_ctx());
        assert!(coil.heating_rate > 0.3 * 10_000.0 && coil.heating_rate < 10_000.0);
        // More water than its design flow is not drawn.
        coil.set_water_inlet(&WaterPort::new(FluidState::water(82.2, 3.0 * m_water)));
        coil.simulate_air(&inlet, &make_ctx());
        assert_relative_eq!(coil.water_outlet().unwrap().state.mass_flow, m_water);
    }

    #[test]
    fn test_hot_water_coil_rating_sets_design_water() {
        // A rated condensing-boiler coil (60/40 °C) on a loop designed at 82 °C:
        // the rating wins, and the design flow follows its 20 K drop.
        let mut coil = HeatingCoil::hot_water("HW", 10_000.0, 40.0);
        coil.rating = HotWaterRating {
            water_inlet_temp: Some(60.0),
            water_outlet_temp: Some(40.0),
            ..Default::default()
        };
        let design = WaterCoilDesign {
            t_water_in: 82.0,
            delta_t: 11.0,
            air_flow: Some(0.5),
            air_in: Some((10.0, 0.005)),
            p_b: 101325.0,
        };
        assert!(coil.size_water_side(&design).is_none());
        assert_relative_eq!(
            coil.design_water_flow().unwrap(),
            10_000.0 / (psych::water_cp(60.0) * 20.0),
            max_relative = 1e-9
        );
    }

    #[test]
    fn test_hot_water_coil_without_plant_is_ideal() {
        let mut coil = HeatingCoil::hot_water("HW", 5_000.0, 50.0);
        let inlet = AirPort::new(MoistAirState::new(10.0, 0.005, 101325.0), 1.0);
        coil.simulate_air(&inlet, &make_ctx());
        assert_relative_eq!(coil.heating_rate, 5_000.0);
        assert!(coil.water_outlet().is_none());
    }

    #[test]
    fn test_unreachable_design_warns() {
        let mut coil = HeatingCoil::hot_water("HW", 100_000.0, 60.0);
        let design = WaterCoilDesign {
            t_water_in: 45.0,
            delta_t: 10.0,
            air_flow: Some(1.0),
            air_in: Some((10.0, 0.005)),
            p_b: 101325.0,
        };
        assert!(coil.size_water_side(&design).is_some());
    }

    #[test]
    fn test_electric_coil_reads_efficiency() {
        let mut coil = HeatingCoil::electric("E", 10_000.0, 30.0);
        coil.efficiency = 0.95;
        let inlet = AirPort::new(MoistAirState::new(20.0, 0.005, 101325.0), 0.2);
        coil.simulate_air(&inlet, &make_ctx());
        assert_relative_eq!(coil.power_consumption(), coil.heating_rate / 0.95);
    }
}
