//! Radiant panel component model.
//!
//! Models fin-tube radiators, panel radiators, and chilled ceiling panels that
//! transfer heat from a water loop (or electric resistance) to the zone via a
//! configurable radiant/convective split. Radiant heat affects MRT and surface
//! temperatures rather than only the zone air temperature.
//!
//! Heat delivery:
//!   Q_convective = Q_total × (1 - radiant_fraction)  → zone air directly
//!   Q_radiant    = Q_total × radiant_fraction          → surfaces (via ZoneHvacConditions.radiant_gains)
//!
//! Water-source panels implement `PlantComponent` to participate in plant loops.
//! Electric panels compute output from thermostat mode directly.
//!
//! Default radiant fractions (matching ASHRAE HOF):
//!   fin-tube / panel radiator: 0.50
//!   chilled ceiling:           0.70

use openbse_core::ports::*;
use openbse_psychrometrics::FluidState;
use serde::{Deserialize, Serialize};

fn default_submeter() -> String {
    "General".to_string()
}

fn default_hot_water_radiant_fraction() -> f64 {
    0.50
}

fn default_chilled_ceiling_radiant_fraction() -> f64 {
    0.70
}

/// Heat source type for a radiant panel.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RadiantPanelSource {
    /// Fin-tube or panel radiator connected to a hot water plant loop.
    HotWater,
    /// Chilled ceiling panel connected to a chilled water plant loop.
    ChilledWater,
    /// Electric resistance radiant panel (no plant loop connection).
    Electric,
}

/// A radiant panel (fin-tube radiator, chilled ceiling, or electric radiant heater).
///
/// Water-source panels implement `PlantComponent` and participate in hot or
/// chilled water plant loops. Electric panels run standalone.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RadiantPanel {
    pub name: String,
    #[serde(default = "default_submeter")]
    pub submeter: String,
    /// Zone this panel serves.
    pub zone: String,
    /// Heat source type.
    pub source: RadiantPanelSource,
    /// Rated capacity [W] (positive = heating for hot-water/electric, positive = cooling for CHW).
    pub rated_capacity: f64,
    /// Fraction of total output that is radiant [0-1].
    /// Remainder is convective.
    pub radiant_fraction: f64,
    /// Optional UA model [W/K].
    /// When `Some(ua)`: Q_total = ua × (T_water - T_zone), capped at rated_capacity.
    /// When `None`: Q_total = rated_capacity × PLR.
    #[serde(default)]
    pub ua: Option<f64>,

    // ─── Runtime state ──────────────────────────────────────────────────
    /// Entering water temperature for this timestep [°C] (water-source only).
    #[serde(skip)]
    pub entering_water_temp: f64,
    /// Part-load ratio [0-1] (electric/PLR-mode panels).
    #[serde(skip)]
    pub plr: f64,
    /// Electric power consumed this timestep [W].
    #[serde(skip)]
    pub power: f64,
    /// Total thermal output delivered to the zone this timestep [W].
    /// Positive = heat gain to zone (heating for HW/electric, negative for CHW).
    #[serde(skip)]
    pub thermal_output_to_zone: f64,
    /// Convective fraction of output [W].
    #[serde(skip)]
    pub convective_output: f64,
    /// Radiant fraction of output [W].
    #[serde(skip)]
    pub radiant_output: f64,
}

impl RadiantPanel {
    /// Create a hot-water fin-tube radiator.
    pub fn new_hot_water(name: &str, zone: &str, rated_capacity: f64, ua: Option<f64>) -> Self {
        Self {
            name: name.to_string(),
            submeter: "General".to_string(),
            zone: zone.to_string(),
            source: RadiantPanelSource::HotWater,
            rated_capacity,
            radiant_fraction: default_hot_water_radiant_fraction(),
            ua,
            entering_water_temp: 60.0,
            plr: 0.0,
            power: 0.0,
            thermal_output_to_zone: 0.0,
            convective_output: 0.0,
            radiant_output: 0.0,
        }
    }

    /// Create a chilled ceiling panel.
    pub fn new_chilled_water(name: &str, zone: &str, rated_capacity: f64, ua: Option<f64>) -> Self {
        Self {
            name: name.to_string(),
            submeter: "General".to_string(),
            zone: zone.to_string(),
            source: RadiantPanelSource::ChilledWater,
            rated_capacity,
            radiant_fraction: default_chilled_ceiling_radiant_fraction(),
            ua,
            entering_water_temp: 10.0,
            plr: 0.0,
            power: 0.0,
            thermal_output_to_zone: 0.0,
            convective_output: 0.0,
            radiant_output: 0.0,
        }
    }

    /// Create an electric radiant panel.
    pub fn new_electric(name: &str, zone: &str, rated_capacity: f64) -> Self {
        Self {
            name: name.to_string(),
            submeter: "General".to_string(),
            zone: zone.to_string(),
            source: RadiantPanelSource::Electric,
            rated_capacity,
            radiant_fraction: default_hot_water_radiant_fraction(),
            ua: None,
            entering_water_temp: 0.0,
            plr: 0.0,
            power: 0.0,
            thermal_output_to_zone: 0.0,
            convective_output: 0.0,
            radiant_output: 0.0,
        }
    }

    /// Compute output from zone temperature and PLR (electric/PLR-mode panels).
    ///
    /// `plr` should be in [0, 1].  For electric panels, PLR = 1.0 when zone
    /// needs heating (below heating setpoint), 0.0 in deadband, etc.
    pub fn simulate_electric(&mut self, plr: f64) {
        self.plr = plr.clamp(0.0, 1.0);
        let q = self.rated_capacity * self.plr;
        self.power = q;
        self.thermal_output_to_zone = q;
        self.convective_output = q * (1.0 - self.radiant_fraction);
        self.radiant_output = q * self.radiant_fraction;
    }

    /// Compute output using UA model (water-source panels).
    ///
    /// `t_water` = entering water temperature [°C]
    /// `t_zone`  = current zone air temperature [°C]
    /// `ua`      = effective UA [W/K]
    ///
    /// Returns Q [W] (positive = heat into zone for HW; negative for CHW).
    pub fn simulate_water_ua(&mut self, t_water: f64, t_zone: f64) {
        self.entering_water_temp = t_water;
        let ua = self.ua.unwrap_or(self.rated_capacity / 40.0_f64.max(1.0));
        let q_raw = ua * (t_water - t_zone);
        // Cap magnitude at rated capacity; preserve sign for cooling.
        let q = if q_raw.abs() > self.rated_capacity {
            q_raw.signum() * self.rated_capacity
        } else {
            q_raw
        };
        self.thermal_output_to_zone = q;
        self.convective_output = q * (1.0 - self.radiant_fraction);
        self.radiant_output = q * self.radiant_fraction;
        self.power = 0.0;
        self.plr = (q.abs() / self.rated_capacity.max(1.0)).clamp(0.0, 1.0);
    }

    /// Compute output using PLR model (water-source without UA).
    ///
    /// `plr` = requested part-load ratio [0, 1].
    /// Sign convention: positive = heating, negative = cooling.
    pub fn simulate_water_plr(&mut self, t_water: f64, plr: f64) {
        self.entering_water_temp = t_water;
        self.plr = plr.clamp(0.0, 1.0);
        let q = match self.source {
            RadiantPanelSource::ChilledWater => -(self.rated_capacity * self.plr),
            _ => self.rated_capacity * self.plr,
        };
        self.thermal_output_to_zone = q;
        self.convective_output = q * (1.0 - self.radiant_fraction);
        self.radiant_output = q * self.radiant_fraction;
        self.power = 0.0;
    }
}

// ─── PlantComponent for water-source panels ──────────────────────────────────

impl PlantComponent for RadiantPanel {
    fn name(&self) -> &str {
        &self.name
    }

    fn component_kind(&self) -> ComponentKind {
        ComponentKind::RadiantPanel
    }

    fn rated_capacity(&self) -> f64 {
        self.rated_capacity
    }

    /// Simulate water-source panel for one timestep.
    ///
    /// `load` = requested thermal load [W] (positive = zone needs heating, negative = cooling).
    /// Returns outlet water conditions after heat exchange.
    fn simulate_plant(
        &mut self,
        inlet: &WaterPort,
        load: f64,
        _ctx: &SimulationContext,
    ) -> WaterPort {
        if load.abs() < 1.0 || inlet.state.mass_flow <= 0.0 {
            self.thermal_output_to_zone = 0.0;
            self.convective_output = 0.0;
            self.radiant_output = 0.0;
            self.power = 0.0;
            self.plr = 0.0;
            return *inlet;
        }

        self.entering_water_temp = inlet.state.temp;

        // UA model or PLR model depending on configuration.
        let q = if let Some(ua) = self.ua {
            // UA model: estimate zone temp from supply water temp.
            // Zone temp not available in plant interface; use UA directly with
            // inlet water and cap at rated capacity.
            let q_ua = ua * (inlet.state.temp - 20.0); // 20°C nominal zone temp
            let q_cap = q_ua.abs().min(self.rated_capacity) * q_ua.signum();
            // Respect requested load direction and cap
            if load.signum() == q_cap.signum() {
                if q_cap.abs() < load.abs() {
                    q_cap
                } else {
                    load.signum() * load.abs().min(q_cap.abs())
                }
            } else {
                0.0
            }
        } else {
            // PLR model: deliver requested load up to rated capacity.
            let q_max = match self.source {
                RadiantPanelSource::ChilledWater => -self.rated_capacity,
                _ => self.rated_capacity,
            };
            if load.signum() == q_max.signum() || q_max == 0.0 {
                let magnitude = load.abs().min(self.rated_capacity);
                load.signum() * magnitude
            } else {
                0.0
            }
        };

        self.thermal_output_to_zone = q;
        self.convective_output = q * (1.0 - self.radiant_fraction);
        self.radiant_output = q * self.radiant_fraction;
        self.plr = (q.abs() / self.rated_capacity.max(1.0)).clamp(0.0, 1.0);
        self.power = 0.0;

        // Compute outlet water temperature from energy balance.
        let cp = inlet.state.cp;
        let m = inlet.state.mass_flow;
        // Heat removed from water = Q delivered to zone (water cools for HW, warms for CHW).
        let dt = -q / (m * cp).max(1e-6);
        let t_out = inlet.state.temp + dt;

        WaterPort::new(FluidState::water(t_out, m))
    }

    fn power_consumption(&self) -> f64 {
        self.power
    }

    fn thermal_output(&self) -> f64 {
        self.thermal_output_to_zone
    }

    fn nominal_capacity(&self) -> Option<f64> {
        Some(self.rated_capacity)
    }

    fn set_nominal_capacity(&mut self, cap: f64) {
        self.rated_capacity = cap;
    }

    fn report_outputs(&self, out: &mut dyn FnMut(&str, f64)) {
        out("plr", self.plr);
        out("radiant_output", self.radiant_output.max(0.0));
        out("convective_output", self.convective_output.max(0.0));
        out("entering_water_temp", self.entering_water_temp);
        out(
            "radiant_panel_heating_rate",
            if self.thermal_output_to_zone > 0.0 {
                self.thermal_output_to_zone
            } else {
                0.0
            },
        );
        out(
            "radiant_panel_cooling_rate",
            if self.thermal_output_to_zone < 0.0 {
                -self.thermal_output_to_zone
            } else {
                0.0
            },
        );
        out("radiant_panel_electric_power", self.power);
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
                month: 1,
                day: 15,
                hour: 8,
                sub_hour: 1,
                timesteps_per_hour: 1,
                sim_time_s: 0.0,
                dt: 3600.0,
            },
            outdoor_air: MoistAirState::from_tdb_rh(-5.0, 0.80, 101325.0),
            day_type: DayType::WeatherDay,
            is_sizing: false,
            sizing_internal_gains: SizingInternalGains::Full,
        }
    }

    #[test]
    fn test_hot_water_radiant_convective_split() {
        // Verify heating output splits correctly: 50% radiant, 50% convective (default).
        let mut panel = RadiantPanel::new_hot_water("HW-Panel-1", "Room", 2000.0, None);
        let inlet = WaterPort::new(FluidState::water(70.0, 0.05)); // 70°C hot water
        let ctx = make_ctx();

        // Request 1000W heating from plant
        let _outlet = panel.simulate_plant(&inlet, 1000.0, &ctx);

        assert!(
            panel.thermal_output_to_zone > 0.0,
            "Heating must deliver positive heat"
        );
        assert_relative_eq!(
            panel.radiant_output,
            panel.thermal_output_to_zone * 0.5,
            max_relative = 0.01
        );
        assert_relative_eq!(
            panel.convective_output,
            panel.thermal_output_to_zone * 0.5,
            max_relative = 0.01
        );
        assert_relative_eq!(
            panel.radiant_output + panel.convective_output,
            panel.thermal_output_to_zone,
            max_relative = 0.001
        );
    }

    #[test]
    fn test_chilled_water_cooling_mode() {
        // Chilled ceiling panel should deliver cooling (negative Q to zone).
        let mut panel = RadiantPanel::new_chilled_water("CHW-Panel-1", "Room", 3000.0, None);
        let inlet = WaterPort::new(FluidState::water(8.0, 0.1)); // 8°C chilled water
        let ctx = make_ctx();

        // Request 2000W cooling (negative load for cooling)
        let _outlet = panel.simulate_plant(&inlet, -2000.0, &ctx);

        assert!(
            panel.thermal_output_to_zone < 0.0,
            "Chilled panel must deliver negative Q (cooling)"
        );
        assert_relative_eq!(panel.radiant_fraction, 0.70, max_relative = 0.001);
        assert_relative_eq!(
            panel.radiant_output.abs(),
            panel.thermal_output_to_zone.abs() * 0.70,
            max_relative = 0.01
        );
    }

    #[test]
    fn test_electric_radiant_panel_heating_mode() {
        // Electric panel with PLR=1.0 delivers full rated capacity.
        let mut panel = RadiantPanel::new_electric("Elec-Panel-1", "Room", 1500.0);
        panel.simulate_electric(1.0);

        assert_relative_eq!(panel.thermal_output_to_zone, 1500.0, max_relative = 0.001);
        assert_relative_eq!(panel.power, 1500.0, max_relative = 0.001);
        // Default radiant fraction for electric is 0.50
        assert_relative_eq!(panel.radiant_output, 750.0, max_relative = 0.001);
        assert_relative_eq!(panel.convective_output, 750.0, max_relative = 0.001);
    }

    #[test]
    fn test_hot_water_ua_model() {
        // UA model: Q = ua × (T_water - T_zone), capped at rated_capacity.
        let mut panel = RadiantPanel::new_hot_water("HW-UA", "Room", 5000.0, Some(100.0));
        // With T_water=60°C, T_zone=20°C: Q = 100 × 40 = 4000W < 5000W cap
        panel.simulate_water_ua(60.0, 20.0);

        assert_relative_eq!(panel.thermal_output_to_zone, 4000.0, max_relative = 0.001);
        assert_relative_eq!(panel.entering_water_temp, 60.0, max_relative = 0.001);
    }

    #[test]
    fn test_water_outlet_temp_energy_balance() {
        // Verify outlet water temperature is computed correctly from energy balance.
        let mut panel = RadiantPanel::new_hot_water("HW-Balance", "Room", 3000.0, None);
        let m_dot = 0.1; // kg/s
        let cp = 4186.0; // J/(kg·K)
        let inlet = WaterPort::new(FluidState::water(70.0, m_dot));
        let ctx = make_ctx();

        let outlet = panel.simulate_plant(&inlet, 2000.0, &ctx);

        // Expected outlet temp: T_in - Q/(m*cp) = 70 - 2000/(0.1*4186)
        let expected_dt = 2000.0 / (m_dot * cp);
        let expected_t_out = 70.0 - expected_dt;
        assert_relative_eq!(outlet.state.temp, expected_t_out, max_relative = 0.01);
    }

    #[test]
    fn test_zero_flow_no_output() {
        // Zero water flow → no thermal output.
        let mut panel = RadiantPanel::new_hot_water("HW-Zero", "Room", 3000.0, None);
        let inlet = WaterPort::new(FluidState::water(70.0, 0.0));
        let ctx = make_ctx();

        let _outlet = panel.simulate_plant(&inlet, 2000.0, &ctx);

        assert_eq!(panel.thermal_output_to_zone, 0.0);
        assert_eq!(panel.plr, 0.0);
    }
}
