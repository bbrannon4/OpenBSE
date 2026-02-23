//! Decoupled sensor/actuator control framework.
//!
//! Controls are a separate layer from physical components. A controller:
//! - Reads from any sensor point in the system (via SystemState)
//! - Computes control actions
//! - Writes actuator commands back (setpoints, flow rates, on/off)
//!
//! The simulation loop runs: sense → compute → actuate → simulate components → check convergence.
//!
//! This replaces EnergyPlus's rigid SetpointManager/Controller paradigm where
//! specific controller types only work with specific system configurations.

pub mod state;
pub mod thermostat;
pub mod setpoint;

pub use state::SystemState;
pub use thermostat::{ZoneThermostat, ZoneGroup};
pub use setpoint::{SetpointController, PlantLoopSetpoint};

use openbse_core::ports::SimulationContext;
use state::ControlAction;

/// Trait for all controllers in the system.
///
/// Every controller follows the same lifecycle per timestep:
/// 1. `sense()` — read current conditions from SystemState
/// 2. `compute()` — determine what actions to take
/// 3. `actions()` — return the list of actuator commands
///
/// Controllers do NOT directly modify components. They produce ControlActions
/// that the simulation loop applies to the graph.
pub trait Controller: std::fmt::Debug {
    /// Controller name.
    fn name(&self) -> &str;

    /// Read sensors and compute control actions for this timestep.
    fn update(&mut self, state: &SystemState, ctx: &SimulationContext);

    /// Return the control actions to apply.
    fn actions(&self) -> &[ControlAction];
}
