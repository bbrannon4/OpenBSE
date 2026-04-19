//! Boiler component model.
//!
//! Physics match EnergyPlus Boilers.cc (CalcBoilerModel):
//!   PLR = Load / NominalCapacity
//!   EffCurveOutput = f(PLR) or f(PLR, Temp)
//!   FuelUsed = Load / (NominalEfficiency * EffCurveOutput)
//!   OutletTemp = InletTemp + Load / (MassFlow * Cp)
//!
//! Reference: EnergyPlus Engineering Reference, Chapter "Boilers"

use crate::performance_curve::PerformanceCurve;
use openbse_core::ports::*;
use openbse_core::types::*;
use openbse_psychrometrics::FluidState;
use serde::{Deserialize, Serialize};

fn default_submeter() -> String {
    "General".to_string()
}

/// Boiler water flow mode, matching EnergyPlus `FlowMode` field.
///
/// - `NotModulated`: fixed flow at design rate, outlet temperature varies
///   with load (current/default behaviour).
/// - `LeavingSetpointModulated`: outlet temperature is held at a leaving
///   setpoint and water flow rate is modulated to deliver the requested load.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum BoilerFlowMode {
    /// Fixed flow, variable outlet temperature (default).
    NotModulated,
    /// Modulate flow to maintain the leaving water temperature setpoint.
    LeavingSetpointModulated,
}

impl Default for BoilerFlowMode {
    fn default() -> Self {
        Self::NotModulated
    }
}

/// Boiler component matching EnergyPlus boiler model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Boiler {
    pub name: String,
    #[serde(default = "default_submeter")]
    pub submeter: String,
    /// Nominal (design) capacity [W]. Use AUTOSIZE for autosizing.
    pub nominal_capacity: f64,
    /// Nominal thermal efficiency [0-1]
    pub nominal_efficiency: f64,
    /// Design outlet temperature [°C]
    pub design_outlet_temp: f64,
    /// Design water flow rate [m³/s]. Use AUTOSIZE for autosizing.
    pub design_water_flow_rate: f64,
    /// Minimum part load ratio [0-1]
    pub min_plr: f64,
    /// Maximum part load ratio [0-1]
    pub max_plr: f64,
    /// Optimum part load ratio [0-1]
    pub opt_plr: f64,
    /// Maximum outlet temperature limit [°C]
    pub max_outlet_temp: f64,
    /// Optional efficiency curve modifier: f(PLR).
    /// If None, efficiency is constant (curve output = 1.0).
    #[serde(skip)]
    pub efficiency_curve: Option<PerformanceCurve>,
    /// Parasitic electric load (forced draft fan) [W]
    pub parasitic_electric_load: f64,
    /// Sizing factor
    pub sizing_factor: f64,
    /// Water flow mode: `NotModulated` (default) or `LeavingSetpointModulated`.
    pub flow_mode: BoilerFlowMode,
    /// Leaving water temperature setpoint [°C].
    /// Used only in `LeavingSetpointModulated` mode.  Defaults to
    /// `design_outlet_temp` when not explicitly set.
    pub leaving_setpoint: f64,

    // ─── Runtime state ──────────────────────────────────────────────────
    #[serde(skip)]
    pub fuel_used: f64,
    #[serde(skip)]
    pub boiler_load: f64,
    #[serde(skip)]
    pub operating_plr: f64,
    #[serde(skip)]
    pub parasitic_power: f64,
    #[serde(skip)]
    pub water_inlet_temp: f64,
    #[serde(skip)]
    pub water_outlet_temp: f64,
    #[serde(skip)]
    pub water_mass_flow: f64,
    #[serde(skip)]
    pub efficiency_operating: f64,
}

impl Boiler {
    /// Create a new boiler with typical defaults.
    pub fn new(
        name: &str,
        nominal_capacity: f64,
        nominal_efficiency: f64,
        design_outlet_temp: f64,
        design_water_flow_rate: f64,
    ) -> Self {
        Self {
            name: name.to_string(),
            submeter: "General".to_string(),
            nominal_capacity,
            nominal_efficiency,
            design_outlet_temp,
            design_water_flow_rate,
            min_plr: 0.0,
            max_plr: 1.0,
            opt_plr: 1.0,
            max_outlet_temp: 99.9,
            efficiency_curve: None,
            parasitic_electric_load: 0.0,
            sizing_factor: 1.0,
            flow_mode: BoilerFlowMode::NotModulated,
            leaving_setpoint: design_outlet_temp,
            fuel_used: 0.0,
            boiler_load: 0.0,
            operating_plr: 0.0,
            parasitic_power: 0.0,
            water_inlet_temp: 0.0,
            water_outlet_temp: 0.0,
            water_mass_flow: 0.0,
            efficiency_operating: 0.0,
        }
    }

    /// Set an optional efficiency curve modifier: f(PLR).
    pub fn with_efficiency_curve(mut self, curve: Option<PerformanceCurve>) -> Self {
        self.efficiency_curve = curve;
        self
    }

    /// Set the boiler flow mode and (optionally) the leaving setpoint.
    ///
    /// If `setpoint` is `None`, the existing `leaving_setpoint` (which
    /// defaults to `design_outlet_temp`) is kept.
    pub fn with_flow_mode(mut self, mode: BoilerFlowMode, setpoint: Option<f64>) -> Self {
        self.flow_mode = mode;
        if let Some(sp) = setpoint {
            self.leaving_setpoint = sp;
        }
        self
    }
}

impl PlantComponent for Boiler {
    fn name(&self) -> &str {
        &self.name
    }

    fn component_kind(&self) -> ComponentKind {
        ComponentKind::Boiler
    }

    fn rated_capacity(&self) -> f64 {
        self.nominal_capacity
    }

    fn simulate_plant(
        &mut self,
        inlet: &WaterPort,
        load: f64,
        _ctx: &SimulationContext,
    ) -> WaterPort {
        // No load: pass through
        if load <= 0.0 {
            self.fuel_used = 0.0;
            self.boiler_load = 0.0;
            self.operating_plr = 0.0;
            self.parasitic_power = 0.0;
            self.water_inlet_temp = inlet.state.temp;
            self.water_outlet_temp = inlet.state.temp;
            self.water_mass_flow = inlet.state.mass_flow;
            self.efficiency_operating = 0.0;
            return *inlet;
        }

        // In NotModulated mode we also need inlet flow; pass through if zero.
        if self.flow_mode == BoilerFlowMode::NotModulated && inlet.state.mass_flow <= 0.0 {
            self.fuel_used = 0.0;
            self.boiler_load = 0.0;
            self.operating_plr = 0.0;
            self.parasitic_power = 0.0;
            self.water_inlet_temp = inlet.state.temp;
            self.water_outlet_temp = inlet.state.temp;
            self.water_mass_flow = inlet.state.mass_flow;
            self.efficiency_operating = 0.0;
            return *inlet;
        }

        // Limit load to capacity
        let boiler_load = load.min(self.nominal_capacity);

        let cp = inlet.state.cp; // J/(kg·K)

        // ── Determine outlet temp and mass flow based on flow mode ──────
        let (outlet_temp, outlet_mass_flow, actual_load) = match self.flow_mode {
            BoilerFlowMode::NotModulated => {
                // Fixed flow: outlet temp varies with load
                let m = inlet.state.mass_flow;
                let dt = boiler_load / (m * cp);
                let mut t_out = inlet.state.temp + dt;
                let mut q = boiler_load;

                // Limit outlet temperature
                if t_out > self.max_outlet_temp {
                    t_out = self.max_outlet_temp;
                    q = m * cp * (t_out - inlet.state.temp);
                }

                (t_out, m, q)
            }
            BoilerFlowMode::LeavingSetpointModulated => {
                // Modulate flow to hold leaving setpoint
                let t_set = self.leaving_setpoint.min(self.max_outlet_temp);
                let dt_set = t_set - inlet.state.temp;

                if dt_set <= 0.0 {
                    // Inlet already at or above setpoint — no heating needed
                    self.fuel_used = 0.0;
                    self.boiler_load = 0.0;
                    self.operating_plr = 0.0;
                    self.parasitic_power = 0.0;
                    self.water_inlet_temp = inlet.state.temp;
                    self.water_outlet_temp = inlet.state.temp;
                    self.water_mass_flow = inlet.state.mass_flow;
                    self.efficiency_operating = 0.0;
                    return *inlet;
                }

                // Required mass flow [kg/s] to deliver boiler_load at the
                // target delta-T:
                let m_required = boiler_load / (cp * dt_set);

                // Design mass flow limit [kg/s] (design_water_flow_rate is
                // stored in m³/s; convert via density ≈ 998 kg/m³).
                let design_mass_flow =
                    self.design_water_flow_rate * openbse_psychrometrics::RHO_WATER;

                let m_actual = m_required.clamp(0.0, design_mass_flow);

                let (t_out, q) = if m_actual < m_required && design_mass_flow > 0.0 {
                    // Flow-limited: cannot reach setpoint, compute actual
                    // outlet temp from clamped flow.
                    let q_actual = m_actual * cp * dt_set;
                    (t_set, q_actual)
                } else {
                    (t_set, boiler_load)
                };

                (t_out, m_actual, q)
            }
        };

        self.boiler_load = actual_load;

        // Calculate PLR
        let plr = (actual_load / self.nominal_capacity).clamp(self.min_plr, self.max_plr);

        // Evaluate efficiency curve (1.0 if no curve)
        let eff_curve_output = self
            .efficiency_curve
            .as_ref()
            .map(|c| c.evaluate_1d(plr))
            .unwrap_or(1.0);

        // Calculate fuel use: FuelUsed = Load / (NomEff * CurveOutput)
        let boiler_eff = (self.nominal_efficiency * eff_curve_output).clamp(0.01, 1.1);
        self.fuel_used = self.boiler_load / boiler_eff;

        // Parasitic electric power
        self.operating_plr = plr;
        self.parasitic_power = self.parasitic_electric_load * plr;

        // Store water conditions for detailed outputs
        self.water_inlet_temp = inlet.state.temp;
        self.water_outlet_temp = outlet_temp;
        self.water_mass_flow = outlet_mass_flow;
        self.efficiency_operating = boiler_eff;

        WaterPort::new(FluidState::water(outlet_temp, outlet_mass_flow))
    }

    fn design_water_flow_rate(&self) -> Option<f64> {
        if is_autosize(self.design_water_flow_rate) {
            None
        } else {
            Some(self.design_water_flow_rate)
        }
    }

    fn set_design_water_flow_rate(&mut self, flow: f64) {
        self.design_water_flow_rate = flow;
    }

    fn power_consumption(&self) -> f64 {
        // Boiler parasitic electric (e.g. forced draft fan)
        self.parasitic_power
    }

    fn fuel_consumption(&self) -> f64 {
        // Gas/fuel consumed = heat output / efficiency
        self.fuel_used
    }

    fn thermal_output(&self) -> f64 {
        self.boiler_load
    }

    fn nominal_capacity(&self) -> Option<f64> {
        if is_autosize(self.nominal_capacity) {
            None
        } else {
            Some(self.nominal_capacity)
        }
    }

    fn set_nominal_capacity(&mut self, cap: f64) {
        self.nominal_capacity = cap;
    }

    fn detailed_outputs(&self) -> std::collections::HashMap<String, f64> {
        let mut m = std::collections::HashMap::new();
        m.insert("plr".to_string(), self.operating_plr);
        m.insert(
            "efficiency_operating".to_string(),
            self.efficiency_operating,
        );
        m.insert("water_inlet_temperature".to_string(), self.water_inlet_temp);
        m.insert(
            "water_outlet_temperature".to_string(),
            self.water_outlet_temp,
        );
        m.insert("water_mass_flow".to_string(), self.water_mass_flow);
        m
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_relative_eq;
    use openbse_core::types::{DayType, TimeStep};
    use openbse_psychrometrics::{MoistAirState, CP_WATER};

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
    fn test_boiler_basic_operation() {
        let mut boiler = Boiler::new("Test Boiler", 100_000.0, 0.80, 82.0, 0.001);

        let inlet = WaterPort::new(FluidState::water(60.0, 2.0)); // 60°C, 2 kg/s
        let load = 50_000.0; // 50 kW
        let ctx = make_ctx();

        let outlet = boiler.simulate_plant(&inlet, load, &ctx);

        // Outlet should be warmer
        assert!(outlet.state.temp > inlet.state.temp);

        // Delta-T = Q / (m * cp) = 50000 / (2 * 4180) = 5.98°C
        let expected_dt = 50_000.0 / (2.0 * CP_WATER);
        assert_relative_eq!(
            outlet.state.temp - inlet.state.temp,
            expected_dt,
            max_relative = 0.001
        );

        // Fuel used = Load / Efficiency = 50000 / 0.80 = 62500 W
        assert_relative_eq!(boiler.fuel_used, 50_000.0 / 0.80, max_relative = 0.001);
    }

    #[test]
    fn test_boiler_capacity_limited() {
        let mut boiler = Boiler::new("Small Boiler", 30_000.0, 0.80, 82.0, 0.001);

        let inlet = WaterPort::new(FluidState::water(60.0, 2.0));
        let load = 50_000.0; // Exceeds capacity
        let ctx = make_ctx();

        let _outlet = boiler.simulate_plant(&inlet, load, &ctx);

        // Should be limited to 30 kW
        assert_relative_eq!(boiler.boiler_load, 30_000.0, max_relative = 0.001);
    }

    #[test]
    fn test_boiler_no_load() {
        let mut boiler = Boiler::new("Test Boiler", 100_000.0, 0.80, 82.0, 0.001);

        let inlet = WaterPort::new(FluidState::water(60.0, 2.0));
        let ctx = make_ctx();

        let outlet = boiler.simulate_plant(&inlet, 0.0, &ctx);

        assert_eq!(boiler.fuel_used, 0.0);
        assert_relative_eq!(outlet.state.temp, inlet.state.temp, max_relative = 0.001);
    }

    #[test]
    fn test_boiler_plr_curve() {
        use crate::performance_curve::{CurveType, PerformanceCurve};

        // Simple PLR curve: 0.8 + 0.2*PLR (efficiency improves at higher load)
        let curve = PerformanceCurve::Polynomial {
            name: "BoilerPLR".to_string(),
            curve_type: CurveType::Linear,
            coefficients: vec![0.8, 0.2],
            min_x: 0.0,
            max_x: 1.0,
            min_y: -100.0,
            max_y: 100.0,
            min_output: None,
            max_output: None,
        };

        let mut boiler = Boiler::new("Curved Boiler", 100_000.0, 0.80, 82.0, 0.001)
            .with_efficiency_curve(Some(curve));

        let inlet = WaterPort::new(FluidState::water(60.0, 2.0));
        let ctx = make_ctx();

        // At 50% load: curve output = 0.8 + 0.2*0.5 = 0.9
        let _outlet = boiler.simulate_plant(&inlet, 50_000.0, &ctx);
        let expected_fuel = 50_000.0 / (0.80 * 0.9);
        assert_relative_eq!(boiler.fuel_used, expected_fuel, max_relative = 0.001);
    }

    #[test]
    fn test_boiler_leaving_setpoint_modulated() {
        use openbse_psychrometrics::RHO_WATER;

        // 100 kW boiler, design flow 0.002 m³/s, setpoint 82 °C
        let design_flow_m3s = 0.002;
        let setpoint = 82.0;
        let mut boiler = Boiler::new("SP Boiler", 100_000.0, 0.80, setpoint, design_flow_m3s)
            .with_flow_mode(BoilerFlowMode::LeavingSetpointModulated, Some(setpoint));

        let t_inlet = 60.0;
        let dt_set = setpoint - t_inlet; // 22 K
        let ctx = make_ctx();

        // ── Case 1: moderate load, flow within design ──────────────────
        let load = 50_000.0; // 50 kW
        let inlet = WaterPort::new(FluidState::water(t_inlet, 0.0)); // inlet flow ignored in SP mode
        let outlet = boiler.simulate_plant(&inlet, load, &ctx);

        // Expected mass flow = Q / (cp * dT)
        let m_expected = load / (CP_WATER * dt_set);
        assert_relative_eq!(outlet.state.mass_flow, m_expected, max_relative = 0.01);
        // Outlet temp should equal setpoint
        assert_relative_eq!(outlet.state.temp, setpoint, max_relative = 0.001);
        // Load fully met
        assert_relative_eq!(boiler.boiler_load, load, max_relative = 0.001);

        // ── Case 2: large load that exceeds design flow ────────────────
        // Design mass flow = 0.002 * 998 = 1.996 kg/s
        // Max deliverable Q at design flow = 1.996 * 4180 * 22 = 183,592 W
        // Request more than that:
        // 100 kW capacity with dt=22 K requires m = 100000/(4180*22) = 1.087 kg/s
        // Design mass flow = 1.996 kg/s — so 100 kW is NOT flow-limited.
        // Actually 100 kW with dt=22 K requires m = 100000/(4180*22) = 1.087 kg/s
        // Design mass flow = 1.996 kg/s — so 100 kW is NOT flow-limited.
        // Use a smaller design flow to force flow-limiting:
        let mut boiler2 = Boiler::new("SP Boiler2", 100_000.0, 0.80, setpoint, 0.0005)
            .with_flow_mode(BoilerFlowMode::LeavingSetpointModulated, Some(setpoint));

        let inlet2 = WaterPort::new(FluidState::water(t_inlet, 0.0));
        let outlet2 = boiler2.simulate_plant(&inlet2, 80_000.0, &ctx);

        let design_mass_flow2 = 0.0005 * RHO_WATER; // ~0.499 kg/s
                                                    // At design flow, max Q = 0.499 * 4180 * 22 = 45,892 W
        let q_max = design_mass_flow2 * CP_WATER * dt_set;
        assert_relative_eq!(
            outlet2.state.mass_flow,
            design_mass_flow2,
            max_relative = 0.01
        );
        assert_relative_eq!(outlet2.state.temp, setpoint, max_relative = 0.001);
        assert_relative_eq!(boiler2.boiler_load, q_max, max_relative = 0.01);
        // Fuel = Q / eff
        assert_relative_eq!(boiler2.fuel_used, q_max / 0.80, max_relative = 0.01);
    }

    #[test]
    fn test_boiler_not_modulated_unchanged() {
        // Verify that NotModulated mode (default) still behaves identically
        // to the original implementation.
        let mut boiler = Boiler::new("NM Boiler", 100_000.0, 0.80, 82.0, 0.001);
        assert_eq!(boiler.flow_mode, BoilerFlowMode::NotModulated);

        let inlet = WaterPort::new(FluidState::water(60.0, 2.0));
        let load = 50_000.0;
        let ctx = make_ctx();
        let outlet = boiler.simulate_plant(&inlet, load, &ctx);

        let expected_dt = load / (2.0 * CP_WATER);
        assert_relative_eq!(outlet.state.temp - 60.0, expected_dt, max_relative = 0.001);
        // Mass flow unchanged (not modulated)
        assert_relative_eq!(outlet.state.mass_flow, 2.0, max_relative = 0.001);
    }
}
