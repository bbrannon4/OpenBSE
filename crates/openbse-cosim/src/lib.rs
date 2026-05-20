//! Co-simulation interface for OpenBSE.
//!
//! Connects OpenBSE to external models (Python, Modelica, MATLAB/Simulink, etc.)
//! using a subprocess + newline-delimited JSON protocol. Each external component
//! runs as a child process; OpenBSE drives the master clock and exchanges one
//! JSON message per timestep.
//!
//! See [`ExternalAirComponent`] and [`ExternalPlantComponent`] for details on
//! the available input/output variable names and the wire protocol.

pub mod external_air;
pub mod external_plant;
pub mod subprocess;

pub use external_air::ExternalAirComponent;
pub use external_plant::ExternalPlantComponent;
