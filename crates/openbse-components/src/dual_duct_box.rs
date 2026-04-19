//! Dual-duct mixing box terminal unit.
//!
//! Models a constant air volume (CAV) dual-duct mixing box that blends supply air
//! from a hot deck and cold deck to meet the zone's heating or cooling load.
//!
//! Control sequence (CAV — total flow is always constant at design_flow):
//!   1. Heating: hot damper opens from minimum toward full; cold damper takes remainder.
//!      blend is mostly hot deck air → zone receives warm supply.
//!   2. Deadband: each damper at 50% → supply temperature is midpoint blend.
//!   3. Cooling: cold damper opens from minimum toward full; hot damper takes remainder.
//!      blend is mostly cold deck air → zone receives cool supply.
//!
//! Total zone supply flow = hot_flow + cold_flow = design_flow (constant).
//!
//! Reference: EnergyPlus Engineering Reference,
//!   "AirTerminal:DualDuct:ConstantVolume"

use openbse_core::ports::ComponentKind;
use serde::{Deserialize, Serialize};

fn default_submeter() -> String {
    "General".to_string()
}

fn default_min_flow_fraction() -> f64 {
    0.20
}

/// CAV dual-duct mixing box terminal unit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DualDuctBox {
    pub name: String,
    #[serde(default = "default_submeter")]
    pub submeter: String,
    /// Design total supply flow [kg/s]
    pub design_flow: f64,
    /// Minimum flow fraction for each damper [0-1], default 0.20
    #[serde(default = "default_min_flow_fraction")]
    pub min_flow_fraction: f64,

    // ─── Runtime state ───────────────────────────────────────────────────
    /// Current hot deck flow contribution [kg/s]
    #[serde(skip)]
    pub hot_flow: f64,
    /// Current cold deck flow contribution [kg/s]
    #[serde(skip)]
    pub cold_flow: f64,
    /// Current blended supply temperature [°C]
    #[serde(skip)]
    pub supply_temp: f64,
    /// Current total supply mass flow [kg/s]
    #[serde(skip)]
    pub supply_mass_flow: f64,
}

impl DualDuctBox {
    /// Create a new dual-duct mixing box.
    ///
    /// # Arguments
    /// * `name` - Component name
    /// * `design_flow` - Design total supply flow rate [kg/s]
    /// * `min_flow_fraction` - Minimum fraction of design_flow per damper [0-1]
    pub fn new(name: &str, design_flow: f64, min_flow_fraction: f64) -> Self {
        Self {
            name: name.to_string(),
            submeter: "General".to_string(),
            design_flow,
            min_flow_fraction: min_flow_fraction.clamp(0.0, 0.5),
            hot_flow: 0.0,
            cold_flow: 0.0,
            supply_temp: 21.0,
            supply_mass_flow: 0.0,
        }
    }

    /// Simulate the mixing box for one timestep.
    ///
    /// Determines damper positions from mode and PLR, then blends the two
    /// deck temperatures proportionally to compute the mixed supply temperature.
    ///
    /// # Arguments
    /// * `heating` - true when the zone needs heating
    /// * `cooling` - true when the zone needs cooling
    /// * `plr` - part-load ratio [0-1] (modulates the active damper)
    /// * `hot_deck_temp` - temperature of the hot deck supply air [°C]
    /// * `cold_deck_temp` - temperature of the cold deck supply air [°C]
    ///
    /// # Returns
    /// `(supply_temp, total_mass_flow)` — blended supply temperature [°C] and
    /// total supply mass flow rate [kg/s] (always equals design_flow).
    pub fn simulate(
        &mut self,
        heating: bool,
        cooling: bool,
        plr: f64,
        hot_deck_temp: f64,
        cold_deck_temp: f64,
    ) -> (f64, f64) {
        let plr = plr.clamp(0.0, 1.0);
        let min_flow = self.design_flow * self.min_flow_fraction;

        if heating && !cooling {
            // Heating: hot damper modulates open; cold damper takes remainder
            self.hot_flow = min_flow + plr * (self.design_flow - min_flow);
            self.cold_flow = self.design_flow - self.hot_flow;
        } else if cooling && !heating {
            // Cooling: cold damper modulates open; hot damper takes remainder
            self.cold_flow = min_flow + plr * (self.design_flow - min_flow);
            self.hot_flow = self.design_flow - self.cold_flow;
        } else {
            // Deadband (or simultaneous — not physically valid): 50/50 split
            self.hot_flow = self.design_flow / 2.0;
            self.cold_flow = self.design_flow / 2.0;
        }

        // Blended supply temperature: energy-weighted mix
        let total = self.design_flow.max(1e-6);
        self.supply_temp =
            (self.hot_flow * hot_deck_temp + self.cold_flow * cold_deck_temp) / total;
        self.supply_mass_flow = self.design_flow;

        (self.supply_temp, self.design_flow)
    }

    /// Component classification for energy accounting.
    pub fn component_kind(&self) -> ComponentKind {
        ComponentKind::DualDuctBox
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOT: f64 = 38.0;
    const COLD: f64 = 13.0;
    const FLOW: f64 = 0.5; // kg/s
    const MIN_FRAC: f64 = 0.20;
    const TOL: f64 = 1e-6;

    fn make_box() -> DualDuctBox {
        DualDuctBox::new("test-box", FLOW, MIN_FRAC)
    }

    #[test]
    fn test_full_heating_plr1() {
        let mut b = make_box();
        let (supply_temp, total_flow) = b.simulate(true, false, 1.0, HOT, COLD);

        // PLR=1 heating: hot damper fully open, cold at minimum
        assert!(
            (total_flow - FLOW).abs() < TOL,
            "total flow must equal design_flow"
        );
        // hot_flow = min + 1.0*(design - min) = design_flow
        // cold_flow = design_flow - design_flow = 0
        let expected_hot = FLOW; // min + 1.0*(FLOW - FLOW*MIN_FRAC) = FLOW
        let expected_cold = 0.0;
        assert!(
            (b.hot_flow - expected_hot).abs() < TOL,
            "hot_flow = {}, expected {}",
            b.hot_flow,
            expected_hot
        );
        assert!(
            (b.cold_flow - expected_cold).abs() < TOL,
            "cold_flow = {}, expected {}",
            b.cold_flow,
            expected_cold
        );
        // Supply temp must equal hot deck (all hot air)
        assert!(
            (supply_temp - HOT).abs() < TOL,
            "supply_temp = {}, expected {}",
            supply_temp,
            HOT
        );
    }

    #[test]
    fn test_full_cooling_plr1() {
        let mut b = make_box();
        let (supply_temp, total_flow) = b.simulate(false, true, 1.0, HOT, COLD);

        assert!(
            (total_flow - FLOW).abs() < TOL,
            "total flow must equal design_flow"
        );
        // cold_flow = design_flow; hot_flow = 0
        assert!(
            (b.cold_flow - FLOW).abs() < TOL,
            "cold_flow = {}, expected {}",
            b.cold_flow,
            FLOW
        );
        assert!((b.hot_flow).abs() < TOL, "hot_flow = {}", b.hot_flow);
        assert!(
            (supply_temp - COLD).abs() < TOL,
            "supply_temp = {}, expected {}",
            supply_temp,
            COLD
        );
    }

    #[test]
    fn test_deadband_midpoint_blend() {
        let mut b = make_box();
        // Deadband: both dampers at 50/50
        let (supply_temp, total_flow) = b.simulate(false, false, 0.0, HOT, COLD);

        assert!(
            (total_flow - FLOW).abs() < TOL,
            "total flow must equal design_flow"
        );
        // 50/50 split → supply = midpoint
        let expected_temp = (HOT + COLD) / 2.0;
        assert!(
            (supply_temp - expected_temp).abs() < TOL,
            "supply_temp = {}, expected midpoint {}",
            supply_temp,
            expected_temp
        );
    }

    #[test]
    fn test_partial_heating_plr() {
        let mut b = make_box();
        let plr = 0.5;
        let (supply_temp, total_flow) = b.simulate(true, false, plr, HOT, COLD);

        assert!(
            (total_flow - FLOW).abs() < TOL,
            "total flow must equal design_flow"
        );
        // hot_flow = min + 0.5*(design - min)
        let min_flow = FLOW * MIN_FRAC;
        let expected_hot = min_flow + plr * (FLOW - min_flow);
        let expected_cold = FLOW - expected_hot;
        let expected_temp = (expected_hot * HOT + expected_cold * COLD) / FLOW;
        assert!(
            (b.hot_flow - expected_hot).abs() < TOL,
            "hot_flow mismatch: {} vs {}",
            b.hot_flow,
            expected_hot
        );
        assert!(
            (supply_temp - expected_temp).abs() < TOL,
            "supply_temp mismatch: {} vs {}",
            supply_temp,
            expected_temp
        );
    }

    #[test]
    fn test_component_kind() {
        let b = make_box();
        assert_eq!(b.component_kind(), ComponentKind::DualDuctBox);
    }
}
