//! Type-safe port system for connecting HVAC and plant components.
//!
//! Uses Rust's type system to enforce physical constraints at compile time:
//! - AirPort and WaterPort are distinct types — you cannot connect a water pipe to an air duct.
//! - Components declare their ports via traits; the graph builder validates connections.

use openbse_psychrometrics::{FluidState, MoistAirState};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ─── Sizing Internal Gains Mode ─────────────────────────────────────────────

/// Controls how internal gains (people, lights, equipment) are handled
/// during design day sizing simulations.
///
/// Each design day can specify its own mode. The choice affects which loads
/// the sizing calculation sees, and therefore how large the HVAC equipment
/// is sized:
///
/// - **Heating design days** typically use `Off` (0% gains) so that heating
///   equipment is sized for worst-case heating demand without internal gains
///   offsetting the load.
/// - **Cooling design days** typically use `Full` (100% gains) so that cooling
///   equipment captures the worst-case cooling demand with all heat sources
///   active.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SizingInternalGains {
    /// No internal gains (0%). Use for heating design days to maximize
    /// heating load (most conservative).
    Off,

    /// Full design-level gains (100%) at all hours, ignoring schedules.
    /// Use for cooling design days to maximize cooling load (most conservative).
    /// This is the EnergyPlus default for SummerDesignDay.
    Full,

    /// Follow the normal occupancy/lighting/equipment schedules.
    /// Gains vary hour-by-hour according to the assigned schedule profiles.
    Scheduled,

    /// Full design-level gains during occupied hours (schedule fraction > 0),
    /// zero gains during unoccupied hours (schedule fraction = 0).
    /// A middle ground: captures peak occupied loads without inflating
    /// unoccupied periods.
    FullWhenOccupied,
}

// ─── Port Types ──────────────────────────────────────────────────────────────

/// An air-side port (inlet or outlet of an air-handling component).
#[derive(Debug, Clone, Copy)]
pub struct AirPort {
    pub state: MoistAirState,
    /// Mass flow rate [kg/s]
    pub mass_flow: f64,
}

impl AirPort {
    pub fn new(state: MoistAirState, mass_flow: f64) -> Self {
        Self { state, mass_flow }
    }

    /// Create a default/zeroed air port (used for initialization).
    pub fn default_at_pressure(p_b: f64) -> Self {
        Self {
            state: MoistAirState::new(20.0, 0.008, p_b),
            mass_flow: 0.0,
        }
    }
}

/// A water-side port (inlet or outlet of a plant component).
#[derive(Debug, Clone, Copy)]
pub struct WaterPort {
    pub state: FluidState,
}

impl WaterPort {
    pub fn new(state: FluidState) -> Self {
        Self { state }
    }

    pub fn default_water() -> Self {
        Self {
            state: FluidState::water(20.0, 0.0),
        }
    }
}

// ─── Component Traits ────────────────────────────────────────────────────────

/// Trait for air-side components (fans, coils, mixing boxes, etc.).
///
/// Every air-side component takes air in and produces air out.
/// The component does NOT know what loop it's on — it just transforms fluid state.
pub trait AirComponent: std::fmt::Debug {
    /// Component name.
    fn name(&self) -> &str;

    /// Simulate this component for one timestep.
    /// Takes inlet air conditions and returns outlet air conditions.
    fn simulate_air(
        &mut self,
        inlet: &AirPort,
        ctx: &SimulationContext,
    ) -> AirPort;

    /// Whether this component has a water-side connection (e.g., hot water coil).
    fn has_water_side(&self) -> bool {
        false
    }

    /// Set the water-side inlet conditions (for coils connected to plant loops).
    fn set_water_inlet(&mut self, _inlet: &WaterPort) {}

    /// Get the water-side outlet conditions after simulation.
    fn water_outlet(&self) -> Option<WaterPort> {
        None
    }

    /// Design air flow rate for autosizing [m³/s]. Returns None if not applicable.
    fn design_air_flow_rate(&self) -> Option<f64> {
        None
    }

    /// Set the design air flow rate (called during autosizing).
    fn set_design_air_flow_rate(&mut self, _flow: f64) {}

    /// Set the outlet temperature setpoint [°C].
    /// Called by the controls framework to override coil/component setpoints.
    /// Default implementation is a no-op (component doesn't use setpoints).
    fn set_setpoint(&mut self, _setpoint: f64) {}

    /// Get the current setpoint, if any.
    fn setpoint(&self) -> Option<f64> {
        None
    }

    /// Nominal capacity [W] for autosizing. Returns None if not applicable.
    fn nominal_capacity(&self) -> Option<f64> {
        None
    }

    /// Set the nominal capacity (called during autosizing).
    fn set_nominal_capacity(&mut self, _cap: f64) {}

    /// Electric or fuel power consumption this timestep [W].
    /// Default 0.0. Override in Fan, DX coil, etc.
    fn power_consumption(&self) -> f64 {
        0.0
    }

    /// Fuel energy consumption this timestep [W equivalent].
    /// For gas coils: fuel consumed = heating_rate / efficiency.
    /// Default 0.0.
    fn fuel_consumption(&self) -> f64 {
        0.0
    }

    /// Thermal output (heating or cooling) this timestep [W].
    /// Positive = heating added to air, negative = cooling removed.
    fn thermal_output(&self) -> f64 {
        0.0
    }

    /// Set exhaust (return) air conditions for heat recovery components.
    /// Called each timestep by the simulation driver before `simulate_air()`.
    /// Default implementation is a no-op (most components don't need exhaust air).
    fn set_exhaust_conditions(&mut self, _temp: f64, _w: f64) {}

    /// Set the ambient temperature surrounding this component [°C].
    /// Used by duct components to model conduction losses to the surrounding space.
    fn set_ambient_temp(&mut self, _temp: f64) {}

    /// Name of the ambient zone for this component, if applicable.
    /// Returns `Some("outdoor")`, `Some("ground")`, or `Some(zone_name)`
    /// for duct components. Returns `None` for all other components.
    fn ambient_zone(&self) -> Option<&str> { None }
}

/// Trait for plant-side components (boilers, chillers, pumps, etc.).
///
/// Every plant component takes water in and produces water out.
/// A boiler doesn't "know" it's on a hot water loop — it just adds heat to fluid.
pub trait PlantComponent: std::fmt::Debug {
    /// Component name.
    fn name(&self) -> &str;

    /// Simulate this component for one timestep.
    /// Takes inlet fluid conditions and returns outlet fluid conditions.
    fn simulate_plant(
        &mut self,
        inlet: &WaterPort,
        load: f64,
        ctx: &SimulationContext,
    ) -> WaterPort;

    /// Design water flow rate for autosizing [m³/s]. Returns None if not applicable.
    fn design_water_flow_rate(&self) -> Option<f64> {
        None
    }

    /// Set the design water flow rate (called during autosizing).
    fn set_design_water_flow_rate(&mut self, _flow: f64) {}

    /// Power consumption of this plant component [W].
    fn power_consumption(&self) -> f64 {
        0.0
    }

    /// Fuel consumption [W equivalent].
    fn fuel_consumption(&self) -> f64 {
        0.0
    }

    /// Thermal output (heating or cooling delivered) [W].
    /// Positive = heating, negative = cooling.
    fn thermal_output(&self) -> f64 {
        0.0
    }

    /// Nominal capacity [W]. Returns None if not applicable.
    fn nominal_capacity(&self) -> Option<f64> {
        None
    }

    /// Set the nominal capacity (called during autosizing).
    fn set_nominal_capacity(&mut self, _cap: f64) {}

    /// Set source-side conditions for inter-loop heat exchangers.
    /// Called by the simulation driver to inject source loop state before
    /// `simulate_plant()`. Default no-op — only `WaterToWaterHX` overrides.
    fn set_source_conditions(&mut self, _temp: f64, _mass_flow: f64) {}
}

/// Context passed to every component during simulation.
#[derive(Debug, Clone)]
pub struct SimulationContext {
    /// Current timestep info
    pub timestep: crate::types::TimeStep,
    /// Outdoor air conditions
    pub outdoor_air: MoistAirState,
    /// Current day type
    pub day_type: crate::types::DayType,
    /// Is this a sizing run?
    pub is_sizing: bool,
    /// How internal gains are handled during sizing design days.
    ///
    /// Only meaningful when `is_sizing == true`. During normal simulation,
    /// schedules are always used regardless of this setting.
    pub sizing_internal_gains: SizingInternalGains,
}

// ─── Envelope Solver Interface ──────────────────────────────────────────────

/// HVAC conditions that the envelope needs each timestep.
#[derive(Debug, Clone, Default)]
pub struct ZoneHvacConditions {
    /// HVAC supply air temperature per zone [°C]
    pub supply_temps: HashMap<String, f64>,
    /// HVAC supply air mass flow per zone [kg/s]
    pub supply_mass_flows: HashMap<String, f64>,
    /// Zone cooling setpoints [°C] — used to compute ideal loads at setpoint
    pub cooling_setpoints: HashMap<String, f64>,
    /// Zone heating setpoints [°C] — used to compute ideal loads at setpoint
    pub heating_setpoints: HashMap<String, f64>,
    /// Zones where outdoor air (ventilation) is handled by the HVAC supply stream.
    /// When true, the zone's own outdoor_air specification is suppressed to avoid
    /// double-counting. When false (e.g., PTAC/FCU with separate ERV), the zone
    /// receives outdoor air directly at outdoor temperature.
    pub oa_handled_by_hvac: HashMap<String, bool>,
}

/// Results that the envelope produces each timestep.
#[derive(Debug, Clone, Default)]
pub struct EnvelopeResults {
    /// Zone air temperatures [°C]
    pub zone_temps: HashMap<String, f64>,
    /// Zone humidity ratios [kg/kg]
    pub zone_humidity: HashMap<String, f64>,
    /// Zone heating loads [W] (positive = needs heating)
    pub zone_heating_loads: HashMap<String, f64>,
    /// Zone cooling loads [W] (positive = needs cooling)
    pub zone_cooling_loads: HashMap<String, f64>,
    /// Ideal cooling load at setpoint [W] — what HVAC must deliver to hold zone at cooling setpoint
    pub ideal_cooling_loads: HashMap<String, f64>,
    /// Ideal heating load at setpoint [W] — what HVAC must deliver to hold zone at heating setpoint
    pub ideal_heating_loads: HashMap<String, f64>,
    /// E+-style predictor: free-floating zone temps WITHOUT HVAC [°C].
    /// Used for mode determination (Heating / Cooling / Deadband).
    pub predictor_temps: HashMap<String, f64>,
    /// Per-zone output variables for reporting
    pub zone_outputs: HashMap<String, HashMap<String, f64>>,
}

/// Trait for the building envelope thermal solver.
///
/// The simulation loop calls `solve_timestep` once per timestep.
/// The implementation manages its own internal state (CTF history,
/// previous surface temps, etc.) across timesteps.
pub trait EnvelopeSolver: std::fmt::Debug {
    /// Initialize the solver (compute CTF coefficients, set initial conditions).
    fn initialize(&mut self, dt: f64) -> Result<(), String>;

    /// Solve all zones for one timestep.
    fn solve_timestep(
        &mut self,
        ctx: &SimulationContext,
        weather: &openbse_weather::WeatherHour,
        hvac: &ZoneHvacConditions,
    ) -> EnvelopeResults;

    /// Update zone temperature BDF history after HVAC convergence.
    ///
    /// Must be called exactly ONCE per physical timestep, AFTER all
    /// HVAC iterations have converged.
    fn update_bdf_history(&mut self);

    /// Get all zone names managed by this solver.
    fn zone_names(&self) -> Vec<String>;
}
