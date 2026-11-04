//! Dual-duct constant-volume mixing box (E+ `AirTerminal:DualDuct:ConstantVolume`,
//! `DualDuct.cc` `SimDualDuctConstVol`).
//!
//! The box always passes its design flow. It splits that flow between the hot
//! and cold decks so the mixed air meets the zone's load. With the load `Q`
//! (positive heating) measured against the zone temperature `T_ref`:
//!
//!   T_target = T_ref + Q / (ṁ·cp)
//!   ṁ_cold   = ṁ · (T_hot − T_target) / (T_hot − T_cold),  limited to [0, ṁ]
//!   ṁ_hot    = ṁ − ṁ_cold
//!
//! which is E+'s `ṁ_cold = (QZnReq − ṁ·cp·T_hot) / (cp·T_cold − cp·T_hot)` with
//! `QZnReq = Q + ṁ·cp·T_ref`. A zero load supplies air at the zone temperature.
//! The outlet mixes the two decks' enthalpy and humidity ratio at those flows.

use openbse_core::ports::ComponentKind;
use openbse_psychrometrics::{self as psych, MoistAirState};
use serde::{Deserialize, Serialize};

fn default_submeter() -> String {
    "General".to_string()
}

/// Deck temperatures closer than this give no usable mix (E+ `SmallTempDiff`).
const SMALL_TEMP_DIFF: f64 = 1.0e-5;

/// CAV dual-duct mixing box terminal unit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DualDuctBox {
    pub name: String,
    #[serde(default = "default_submeter")]
    pub submeter: String,
    /// Design total supply flow [kg/s]
    pub design_flow: f64,

    // ─── Runtime state ───────────────────────────────────────────────────
    /// Current hot deck flow [kg/s]
    #[serde(skip)]
    pub hot_flow: f64,
    /// Current cold deck flow [kg/s]
    #[serde(skip)]
    pub cold_flow: f64,
    /// Current mixed supply temperature [°C]
    #[serde(skip)]
    pub supply_temp: f64,
    /// Current mixed supply humidity ratio [kg/kg]
    #[serde(skip)]
    pub supply_w: f64,
}

impl DualDuctBox {
    /// Create a new dual-duct mixing box with its design total flow [kg/s].
    pub fn new(name: &str, design_flow: f64) -> Self {
        Self {
            name: name.to_string(),
            submeter: "General".to_string(),
            design_flow,
            hot_flow: 0.0,
            cold_flow: 0.0,
            supply_temp: 21.0,
            supply_w: 0.0,
        }
    }

    /// Split `flow` [kg/s] between the decks so the mixed air delivers `load`
    /// [W, positive heating] to a zone at `t_ref` [°C], given the decks'
    /// temperatures [°C]. Returns `(hot_flow, cold_flow)`.
    pub fn set_flows(
        &mut self,
        flow: f64,
        load: f64,
        t_ref: f64,
        t_hot: f64,
        t_cold: f64,
        cp: f64,
    ) -> (f64, f64) {
        let flow = flow.max(0.0);
        let cold = if flow <= 0.0 {
            0.0
        } else if (t_hot - t_cold).abs() > SMALL_TEMP_DIFF {
            let t_target = t_ref + load / (flow * cp);
            flow * (t_hot - t_target) / (t_hot - t_cold)
        } else if load > 0.0 {
            0.0
        } else {
            flow
        };
        self.cold_flow = cold.clamp(0.0, flow);
        self.hot_flow = flow - self.cold_flow;
        (self.hot_flow, self.cold_flow)
    }

    /// Mix the decks' actual states at the flows `set_flows` chose.
    pub fn mix(&mut self, hot: &MoistAirState, cold: &MoistAirState) -> MoistAirState {
        let total = self.hot_flow + self.cold_flow;
        let (h, w) = if total > 0.0 {
            (
                (self.hot_flow * hot.h + self.cold_flow * cold.h) / total,
                (self.hot_flow * hot.w + self.cold_flow * cold.w) / total,
            )
        } else {
            ((hot.h + cold.h) / 2.0, (hot.w + cold.w) / 2.0)
        };
        let state = MoistAirState::new(psych::tdb_fn_h_w(h, w), w, cold.p_b);
        self.supply_temp = state.t_db;
        self.supply_w = state.w;
        state
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
    const CP: f64 = 1006.0;

    fn deck(t: f64, w: f64) -> MoistAirState {
        MoistAirState::new(t, w, 101_325.0)
    }

    #[test]
    fn zero_load_supplies_air_at_the_zone_temperature() {
        let mut b = DualDuctBox::new("b", FLOW);
        let (hot, cold) = b.set_flows(FLOW, 0.0, 22.0, HOT, COLD, CP);
        assert!((hot + cold - FLOW).abs() < 1e-12);
        let out = b.mix(&deck(HOT, 0.008), &deck(COLD, 0.008));
        assert!((out.t_db - 22.0).abs() < 0.05, "supply {}", out.t_db);
    }

    #[test]
    fn small_loads_take_the_matching_deck() {
        // A small cooling load cools and a small heating load heats: the
        // supply sits just either side of the zone temperature.
        let mut b = DualDuctBox::new("b", FLOW);
        b.set_flows(FLOW, -200.0, 24.0, HOT, COLD, CP);
        let cooling = b.mix(&deck(HOT, 0.008), &deck(COLD, 0.008)).t_db;
        b.set_flows(FLOW, 200.0, 21.0, HOT, COLD, CP);
        let heating = b.mix(&deck(HOT, 0.008), &deck(COLD, 0.008)).t_db;
        assert!(cooling < 24.0 && cooling > 23.0, "cooling supply {cooling}");
        assert!(heating > 21.0 && heating < 22.0, "heating supply {heating}");
    }

    #[test]
    fn the_mix_meets_the_load_within_the_decks() {
        let mut b = DualDuctBox::new("b", FLOW);
        for &(load, t_ref) in &[(3000.0, 21.0), (-4000.0, 24.0), (-1000.0, 24.0)] {
            b.set_flows(FLOW, load, t_ref, HOT, COLD, CP);
            let out = b.mix(&deck(HOT, 0.008), &deck(COLD, 0.008));
            let delivered = FLOW * CP * (out.t_db - t_ref);
            assert!(
                (delivered - load).abs() < 0.01 * load.abs(),
                "load {load}: delivered {delivered}"
            );
        }
    }

    #[test]
    fn loads_beyond_a_deck_take_all_of_that_deck() {
        let mut b = DualDuctBox::new("b", FLOW);
        assert_eq!(b.set_flows(FLOW, 1.0e6, 21.0, HOT, COLD, CP), (FLOW, 0.0));
        assert_eq!(b.set_flows(FLOW, -1.0e6, 24.0, HOT, COLD, CP), (0.0, FLOW));
    }

    #[test]
    fn equal_decks_pick_one_by_the_load_sign() {
        let mut b = DualDuctBox::new("b", FLOW);
        assert_eq!(b.set_flows(FLOW, 100.0, 21.0, 20.0, 20.0, CP), (FLOW, 0.0));
        assert_eq!(b.set_flows(FLOW, -100.0, 21.0, 20.0, 20.0, CP), (0.0, FLOW));
    }

    #[test]
    fn mixing_conserves_mass_moisture_and_enthalpy() {
        let mut b = DualDuctBox::new("b", FLOW);
        let (hot, cold) = b.set_flows(FLOW, 1500.0, 21.0, HOT, COLD, CP);
        let (h_deck, c_deck) = (deck(HOT, 0.0080), deck(COLD, 0.0075));
        let out = b.mix(&h_deck, &c_deck);
        let enthalpy_in = hot * h_deck.h + cold * c_deck.h;
        let water_in = hot * h_deck.w + cold * c_deck.w;
        assert!((FLOW * out.h - enthalpy_in).abs() < 1e-6 * enthalpy_in.abs());
        assert!((FLOW * out.w - water_in).abs() < 1e-12);
    }

    #[test]
    fn test_component_kind() {
        let b = DualDuctBox::new("b", FLOW);
        assert_eq!(b.component_kind(), ComponentKind::DualDuctBox);
    }
}
