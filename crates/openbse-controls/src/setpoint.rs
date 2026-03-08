//! Setpoint controllers — fixed setpoints for coils, plant loops, etc.
//!
//! These are the simplest controllers: they read nothing (or just outdoor air)
//! and write a fixed setpoint to a target component or loop.
//!
//! More advanced reset logic (OA reset, demand-based reset) will build on
//! these same structures — the sensor/actuator pattern stays the same.

use crate::state::{ControlAction, SystemState};
use crate::Controller;
use openbse_core::ports::SimulationContext;
use serde::{Deserialize, Serialize};

// ─── Fixed Setpoint Controller ───────────────────────────────────────────────

/// Sets a fixed temperature setpoint on a component (coil, etc.).
///
/// This is the simplest controller — "keep the coil outlet at 35°C."
/// Used for furnace discharge temp, preheat coil setpoint, etc.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetpointController {
    name: String,
    /// Target component name
    pub component: String,
    /// Fixed setpoint value [°C]
    pub setpoint: f64,
    /// Whether this is an air-side setpoint (coil) or water-side
    pub is_air_side: bool,

    #[serde(skip)]
    current_actions: Vec<ControlAction>,
}

impl SetpointController {
    /// Create a fixed air-side setpoint (e.g., coil outlet temp).
    pub fn air_setpoint(name: &str, component: &str, setpoint: f64) -> Self {
        Self {
            name: name.to_string(),
            component: component.to_string(),
            setpoint,
            is_air_side: true,
            current_actions: Vec::new(),
        }
    }

    /// Create a fixed plant-side setpoint (e.g., boiler outlet temp).
    pub fn plant_setpoint(name: &str, component: &str, setpoint: f64) -> Self {
        Self {
            name: name.to_string(),
            component: component.to_string(),
            setpoint,
            is_air_side: false,
            current_actions: Vec::new(),
        }
    }
}

impl Controller for SetpointController {
    fn name(&self) -> &str {
        &self.name
    }

    fn update(&mut self, _state: &SystemState, _ctx: &SimulationContext) {
        self.current_actions.clear();

        if self.is_air_side {
            self.current_actions.push(ControlAction::SetCoilSetpoint {
                component: self.component.clone(),
                setpoint: self.setpoint,
            });
        } else {
            self.current_actions.push(ControlAction::SetPlantLoad {
                component: self.component.clone(),
                load: 0.0, // Plant setpoint — actual load determined by demand
            });
        }
    }

    fn actions(&self) -> &[ControlAction] {
        &self.current_actions
    }
}

// ─── Plant Loop Setpoint Controller ──────────────────────────────────────────

/// Sets a supply temperature setpoint for a plant loop.
///
/// "Keep the hot water loop at 82°C" or "Keep the chilled water loop at 6.7°C."
/// The plant loop solver uses this setpoint to determine how much load to
/// place on supply equipment (boilers, chillers, etc.).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlantLoopSetpoint {
    name: String,
    /// Target plant loop name
    pub loop_name: String,
    /// Supply temperature setpoint [°C]
    pub supply_temp_setpoint: f64,

    #[serde(skip)]
    current_actions: Vec<ControlAction>,
}

impl PlantLoopSetpoint {
    pub fn new(name: &str, loop_name: &str, supply_temp: f64) -> Self {
        Self {
            name: name.to_string(),
            loop_name: loop_name.to_string(),
            supply_temp_setpoint: supply_temp,
            current_actions: Vec::new(),
        }
    }
}

impl Controller for PlantLoopSetpoint {
    fn name(&self) -> &str {
        &self.name
    }

    fn update(&mut self, _state: &SystemState, _ctx: &SimulationContext) {
        self.current_actions.clear();
        self.current_actions.push(ControlAction::SetPlantLoopSetpoint {
            loop_name: self.loop_name.clone(),
            setpoint: self.supply_temp_setpoint,
        });
    }

    fn actions(&self) -> &[ControlAction] {
        &self.current_actions
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openbse_core::ports::SizingInternalGains;
    use openbse_core::types::{DayType, TimeStep};
    use openbse_psychrometrics::MoistAirState;

    fn make_ctx() -> SimulationContext {
        SimulationContext {
            timestep: TimeStep {
                month: 1, day: 15, hour: 12, sub_hour: 1,
                timesteps_per_hour: 1, sim_time_s: 0.0, dt: 3600.0,
            },
            outdoor_air: MoistAirState::from_tdb_rh(0.0, 0.5, 101325.0),
            day_type: DayType::WeatherDay,
            is_sizing: false,
            sizing_internal_gains: SizingInternalGains::Full,
        }
    }

    #[test]
    fn test_fixed_coil_setpoint() {
        let mut ctrl = SetpointController::air_setpoint(
            "Furnace Discharge",
            "Furnace Coil",
            48.9, // 120°F
        );

        let state = SystemState::new(MoistAirState::from_tdb_rh(0.0, 0.5, 101325.0));
        let ctx = make_ctx();
        ctrl.update(&state, &ctx);

        assert_eq!(ctrl.actions().len(), 1);
        match &ctrl.actions()[0] {
            ControlAction::SetCoilSetpoint { component, setpoint } => {
                assert_eq!(component, "Furnace Coil");
                assert!((setpoint - 48.9).abs() < 0.001);
            }
            _ => panic!("Expected SetCoilSetpoint"),
        }
    }

    #[test]
    fn test_plant_loop_setpoint() {
        let mut ctrl = PlantLoopSetpoint::new(
            "HW Loop Control",
            "Hot Water Loop",
            82.0,
        );

        let state = SystemState::new(MoistAirState::from_tdb_rh(0.0, 0.5, 101325.0));
        let ctx = make_ctx();
        ctrl.update(&state, &ctx);

        assert_eq!(ctrl.actions().len(), 1);
        match &ctrl.actions()[0] {
            ControlAction::SetPlantLoopSetpoint { loop_name, setpoint } => {
                assert_eq!(loop_name, "Hot Water Loop");
                assert!((setpoint - 82.0).abs() < 0.001);
            }
            _ => panic!("Expected SetPlantLoopSetpoint"),
        }
    }
}
