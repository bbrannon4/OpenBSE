//! Simulation configuration, per-timestep results and control signals.
//!
//! The simulation loop itself is `main` in `openbse-cli` (Engineering Reference,
//! chapter 2).

use std::collections::HashMap;

/// Configuration for a simulation run.
#[derive(Debug, Clone)]
pub struct SimulationConfig {
    /// Number of timesteps per hour (1, 2, 4, 6, 10, 12, 15, 20, 30, 60)
    pub timesteps_per_hour: u32,
    /// Start month (1-12)
    pub start_month: u32,
    /// Start day
    pub start_day: u32,
    /// End month (1-12)
    pub end_month: u32,
    /// End day
    pub end_day: u32,
}

impl Default for SimulationConfig {
    fn default() -> Self {
        Self {
            timesteps_per_hour: 1,
            start_month: 1,
            start_day: 1,
            end_month: 12,
            end_day: 31,
        }
    }
}

/// Result data collected at each timestep for each component.
#[derive(Debug, Clone)]
pub struct TimestepResult {
    pub month: u32,
    pub day: u32,
    pub hour: u32,
    pub sub_hour: u32,
    /// Component name -> output variables
    pub component_outputs: HashMap<String, HashMap<String, f64>>,
}

/// Setpoint overrides applied by the controls framework.
///
/// This is the bridge between the controls crate and the simulation loop.
/// The controls crate produces these, the simulation loop consumes them.
/// This avoids a circular dependency: core doesn't depend on controls.
#[derive(Debug, Clone, Default)]
pub struct ControlSignals {
    /// Coil setpoint overrides: component_name -> setpoint [°C]
    pub coil_setpoints: HashMap<String, f64>,
    /// Air mass flow overrides: component_name -> mass_flow [kg/s]
    pub air_mass_flows: HashMap<String, f64>,
    /// Plant loop setpoints: loop_name -> setpoint [°C]
    pub plant_loop_setpoints: HashMap<String, f64>,
    /// Plant component load demands: component_name -> load [W]
    pub plant_loads: HashMap<String, f64>,
    /// Zone supply air temps: zone_name -> supply_temp [°C]
    pub zone_supply_temps: HashMap<String, f64>,
    /// Zone air flows: zone_name -> mass_flow [kg/s]
    pub zone_air_flows: HashMap<String, f64>,
    /// SAT setpoint from the VAV signal builder [°C] (0 when unset); the
    /// mixed-air target that heat recovery is throttled to.
    pub sat_setpoint: f64,
    /// Whether the economizer is admitting more than minimum outdoor air;
    /// heat recovery is locked out then (E+ `Economizer Lockout = Yes`).
    pub economizer_active: bool,

    // ── Loop-level signals ──────────────────────────────────────────────
    // Previously smuggled through `coil_setpoints` as "__sentinel__" string
    // keys, where a typo'd key silently fell back to a default. Typed fields
    // make the producer/consumer contract checkable by the compiler.
    /// Mixed-air (or recirculation) temperature entering the component chain [°C].
    pub mixed_air_temp: Option<f64>,
    /// Outdoor-air fraction of supply flow [0-1].
    pub oa_fraction: Option<f64>,
    /// Loop part-load ratio for systems that modulate internally [0-1].
    pub loop_plr: Option<f64>,
    /// Post-heat-recovery outdoor air temperature [°C].
    pub effective_oa_temp: Option<f64>,
    /// Post-heat-recovery outdoor air humidity ratio [kg/kg].
    pub effective_oa_w: Option<f64>,
    /// Served-zone average return-air humidity ratio [kg/kg].
    pub return_air_w: Option<f64>,
    /// Return-air temperature from the served zones, before any return fan
    /// [°C]: the economizer's reference and heat recovery's exhaust inlet.
    pub return_air_temp: Option<f64>,
}
