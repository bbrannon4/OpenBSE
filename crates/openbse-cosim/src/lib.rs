//! Co-simulation interface for OpenBSE.
//!
//! Connects OpenBSE to external models (Python, Modelica, MATLAB/Simulink, etc.)
//! using a subprocess + newline-delimited JSON protocol. Each external component
//! runs as a child process, started on its first exchange and stopped at the
//! end of the run; OpenBSE drives the master clock.
//!
//! # Protocol
//!
//! Each exchange is one request line and one response line:
//!
//! ```text
//! → {"time_s": 3600.0, "dt_s": 3600.0, "iteration": 0, "warmup": false,
//!    "sizing": false, "inputs": {"return_temp_c": 12.1, ...}}
//! ← {"outputs": {"supply_temp_c": 7.0, "power_w": 2100.0}}
//! ← {"error": "message"}                       (stops the run)
//! ```
//!
//! - The engine calls a component once per HVAC iteration, so a timestep can
//!   hold several exchanges. They share `time_s`; `iteration` counts them from
//!   0, and the engine keeps the outputs of the last one. A model with state
//!   (a tank, an integral term) evaluates every iteration from the state at
//!   the start of the step and commits it when a request with a new `time_s`
//!   arrives. A component with no flow or no load is not called.
//! - `warmup` is true during warmup, which repeats the first days of the run
//!   until zone temperatures settle, so the same `time_s` values recur. A
//!   model should reset or discard its state at the first request with
//!   `warmup` false.
//! - `sizing` is true for design-day sizing calls.
//! - At the end the engine sends `{"command": "stop"}` and closes stdin.
//!
//! Any failure stops the run with a severe error: a process that can't start,
//! a closed pipe, an unparseable line, an `error` reply, a response missing a
//! declared output or holding a non-finite number, a `thermal_output_w` that
//! disagrees with the returned outlet state, or no response within the
//! component's `timeout_s` (default 60 s). Unknown input or output names, and
//! a missing required output, are severe errors when the model is read.
//!
//! See [`ExternalAirComponent`] and [`ExternalPlantComponent`] for the
//! variable names.

pub mod external_air;
pub mod external_plant;
pub mod subprocess;

pub use external_air::ExternalAirComponent;
pub use external_plant::ExternalPlantComponent;
