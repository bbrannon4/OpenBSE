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
//! Every panel meets its zone's load up to what it can deliver. An electric
//! panel can deliver its rated capacity. A water panel can deliver
//! UA·(T_water − T_zone) (heating) or UA·(T_zone − T_water) (cooling), up to its
//! rated capacity, as E+'s water baseboard and cooling panel do. Without a
//! stated UA, UA is the rated capacity over the rating temperature difference:
//! the loop's design supply temperature against E+'s rating air temperatures
//! (18 °C entering air for a baseboard, 24 °C space for a cooling panel).
//!
//! Default radiant fractions (matching ASHRAE HOF):
//!   fin-tube / panel radiator: 0.50
//!   chilled ceiling:           0.70

use openbse_core::ports::ComponentKind;
use serde::{Deserialize, Serialize};

/// Air temperature a hot-water panel's rated capacity is quoted at [°C]
/// (E+ `ZoneHVAC:Baseboard:RadiantConvective:Water` sizing).
pub const RATED_AIR_TEMP_HEATING: f64 = 18.0;
/// Space temperature a chilled panel's rated capacity is quoted at [°C]
/// (E+ `ZoneHVAC:CoolingPanel:RadiantConvective:Water` default).
pub const RATED_AIR_TEMP_COOLING: f64 = 24.0;

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
/// Water-source panels draw on a hot or chilled water plant loop; electric
/// panels run standalone.
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
    /// Water-to-zone conductance [W/K]; `None` derives it from the rated
    /// capacity at the rating temperatures.
    #[serde(default)]
    pub ua: Option<f64>,
    /// Water temperature the rated capacity is quoted at [°C]: the plant
    /// loop's design supply temperature.
    #[serde(default)]
    pub rated_water_temp: f64,

    // ─── Runtime state ──────────────────────────────────────────────────
    /// Entering water temperature for this timestep [°C] (water-source only).
    #[serde(skip)]
    pub entering_water_temp: f64,
    /// Output over rated capacity [0-1].
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
            rated_water_temp: 60.0,
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
            rated_water_temp: 10.0,
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
            rated_water_temp: 0.0,
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

    /// Water-to-zone conductance [W/K]: the stated UA, else the rated capacity
    /// over the rating temperature difference (0 when the loop's design
    /// temperature can't deliver heat at the rating point; validation
    /// rejects that).
    pub fn effective_ua(&self) -> f64 {
        if let Some(ua) = self.ua {
            return ua;
        }
        let dt = match self.source {
            RadiantPanelSource::ChilledWater => RATED_AIR_TEMP_COOLING - self.rated_water_temp,
            _ => self.rated_water_temp - RATED_AIR_TEMP_HEATING,
        };
        if dt > 0.0 {
            self.rated_capacity / dt
        } else {
            0.0
        }
    }

    /// Heat [W, magnitude] the panel can deliver into a zone at `t_zone`
    /// from water entering at `t_water`: UA times the driving difference, up
    /// to the rated capacity. An electric panel can deliver its rated
    /// capacity.
    pub fn capacity(&self, t_water: f64, t_zone: f64) -> f64 {
        let dt = match self.source {
            RadiantPanelSource::Electric => return self.rated_capacity.max(0.0),
            RadiantPanelSource::ChilledWater => t_zone - t_water,
            RadiantPanelSource::HotWater => t_water - t_zone,
        };
        (self.effective_ua() * dt).clamp(0.0, self.rated_capacity.max(0.0))
    }

    /// Water panel: deliver `load` [W, magnitude, heating for hot water and
    /// cooling for chilled water] up to what the water can deliver.
    pub fn simulate_water(&mut self, t_water: f64, t_zone: f64, load: f64) {
        self.entering_water_temp = t_water;
        let magnitude = load.max(0.0).min(self.capacity(t_water, t_zone));
        let q = match self.source {
            RadiantPanelSource::ChilledWater => -magnitude,
            _ => magnitude,
        };
        self.thermal_output_to_zone = q;
        self.convective_output = q * (1.0 - self.radiant_fraction);
        self.radiant_output = q * self.radiant_fraction;
        self.power = 0.0;
        self.plr = (magnitude / self.rated_capacity.max(1.0)).clamp(0.0, 1.0);
    }

    /// Component classification for energy accounting.
    pub fn component_kind(&self) -> ComponentKind {
        ComponentKind::RadiantPanel
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_relative_eq;

    #[test]
    fn test_hot_water_radiant_convective_split() {
        // Verify heating output splits correctly: 50% radiant, 50% convective (default).
        let mut panel = RadiantPanel::new_hot_water("HW-Panel-1", "Room", 2000.0, Some(100.0));
        panel.simulate_water(70.0, 20.0, 1000.0);

        assert_relative_eq!(panel.thermal_output_to_zone, 1000.0, max_relative = 1e-9);
        assert_relative_eq!(panel.radiant_output, 500.0, max_relative = 1e-9);
        assert_relative_eq!(panel.convective_output, 500.0, max_relative = 1e-9);
    }

    #[test]
    fn test_chilled_water_cooling_mode() {
        // Chilled ceiling panel delivers cooling (negative Q to zone).
        let mut panel = RadiantPanel::new_chilled_water("CHW-Panel-1", "Room", 3000.0, None);
        panel.rated_water_temp = 14.0;
        panel.simulate_water(14.0, 25.0, 2000.0);

        assert_relative_eq!(panel.thermal_output_to_zone, -2000.0, max_relative = 1e-9);
        assert_relative_eq!(panel.radiant_fraction, 0.70, max_relative = 0.001);
        assert_relative_eq!(panel.radiant_output, -1400.0, max_relative = 1e-9);
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

    /// A UA panel is thermostatic: it meets the load, and only the load,
    /// up to UA·ΔT.
    #[test]
    fn ua_panel_meets_the_load_up_to_ua_times_dt() {
        let mut panel = RadiantPanel::new_hot_water("HW-UA", "Room", 5000.0, Some(100.0));
        panel.simulate_water(60.0, 20.0, 1500.0);
        assert_relative_eq!(panel.thermal_output_to_zone, 1500.0, max_relative = 1e-9);
        panel.simulate_water(60.0, 20.0, 9000.0);
        assert_relative_eq!(panel.thermal_output_to_zone, 4000.0, max_relative = 1e-9);
        panel.simulate_water(60.0, 20.0, 0.0);
        assert_eq!(panel.thermal_output_to_zone, 0.0);
    }

    /// Without a stated UA the output still collapses as the water cools:
    /// rated at the loop's design temperature and E+'s 18 °C rating air.
    #[test]
    fn output_follows_the_water_temperature_without_a_stated_ua() {
        let mut panel = RadiantPanel::new_hot_water("HW", "Room", 2500.0, None);
        panel.rated_water_temp = 82.0;
        assert_relative_eq!(panel.capacity(82.0, 18.0), 2500.0, max_relative = 1e-9);
        assert_relative_eq!(
            panel.capacity(50.0, 18.0),
            2500.0 * 32.0 / 64.0,
            max_relative = 1e-9
        );
        assert_eq!(
            panel.capacity(30.0, 31.0),
            0.0,
            "water colder than the zone heats nothing"
        );
        panel.simulate_water(30.0, 20.0, 2500.0);
        assert_relative_eq!(
            panel.thermal_output_to_zone,
            2500.0 * 10.0 / 64.0,
            max_relative = 1e-9
        );
    }
}
