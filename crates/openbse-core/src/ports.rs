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

// ─── Component Kind ─────────────────────────────────────────────────────────

/// Classifies a component for energy accounting.
///
/// The simulation driver uses this to route power and fuel consumption
/// to the correct building end-use category (fan electric, heating gas,
/// cooling electric, etc.) without relying on component names.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ComponentKind {
    Fan,
    HeatingCoil,
    CoolingCoil,
    HeatRecovery,
    Humidifier,
    Duct,
    Pump,
    Boiler,
    Chiller,
    CoolingTower,
    HeatExchanger,
    VrfIndoor,
    VrfOutdoor,
    RadiantPanel,
    Gshp,
    DualDuctBox,
    EvapCooler,
    ThermalStorage,
    /// Computer-room air conditioner (self-contained DX)
    Crac,
    /// Computer-room air handler (chilled-water)
    Crah,
    /// IT server loads (separate from general equipment for PUE accounting)
    ItEquipment,
    /// UPS and transformer electrical distribution losses
    ElecDistribution,
    Other,
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

    /// What kind of component this is, for energy accounting.
    /// Defaults to `Other` — override to get automatic end-use routing.
    fn component_kind(&self) -> ComponentKind {
        ComponentKind::Other
    }

    /// Simulate this component for one timestep.
    ///
    /// Takes inlet air conditions and returns outlet air conditions.
    /// The component transforms the air state (temperature, humidity, enthalpy)
    /// and must leave `mass_flow` unchanged unless it intentionally adds or
    /// removes air (e.g., a mixing box).
    ///
    /// **Zero flow**: when `inlet.mass_flow <= 0.0`, return `*inlet` unchanged
    /// and set all internal power/thermal outputs to zero.
    ///
    /// **Sign convention**: heating raises `outlet.state.t_db` above inlet;
    /// cooling lowers it. The component reports the energy it consumed via
    /// `power_consumption()` and `fuel_consumption()` after this call.
    ///
    /// `ctx` provides the current timestep, outdoor conditions, and whether
    /// this is a sizing run — use it for schedule lookups and outdoor-air
    /// dependent calculations.
    fn simulate_air(&mut self, inlet: &AirPort, ctx: &SimulationContext) -> AirPort;

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

    /// Instantaneous electric power consumption [W].
    ///
    /// Always positive. Report ALL electric power here — fan motors, compressors,
    /// electric resistance elements, etc. This is the sole input to the
    /// electric energy accounting; `fuel_consumption()` is separate.
    /// Default 0.0.
    fn power_consumption(&self) -> f64 {
        0.0
    }

    /// Instantaneous fuel energy rate [W equivalent] (gas, oil, propane, etc.).
    ///
    /// Always positive. Only non-electric fuel goes here — for a gas furnace
    /// this is `heating_rate / thermal_efficiency`. Electric components should
    /// leave this at the default 0.0 and report via `power_consumption()`.
    fn fuel_consumption(&self) -> f64 {
        0.0
    }

    /// Net thermal output delivered to the air stream [W].
    ///
    /// Positive = heat added to air, negative = heat removed from air.
    /// For a heating coil this is positive; for a cooling coil, negative.
    /// For a fan, this is the motor waste heat entering the airstream.
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

    /// Set heating nominal capacity [W] for heat pump components with separate
    /// heating and cooling capacities.  Default no-op.
    fn set_heating_capacity(&mut self, _cap: f64) {}

    /// Inject ground temperature model parameters for ground-source heat pump
    /// components.  Called at build time by the simulation driver after reading
    /// weather data.  Default no-op — only `GroundSourceHeatPump` overrides this.
    ///
    /// Parameters mirror the Kusuda-Achenbach equation:
    ///   `t_mean` — annual mean ground surface temperature [°C]
    ///   `amplitude` — half of annual peak-to-peak surface amplitude [°C]
    ///   `phase_day` — day of year of minimum surface temperature
    ///   `soil_diffusivity` — soil thermal diffusivity [m²/day]
    ///   `loop_depth` — burial depth override [m] (ignored; GSHP uses its own)
    ///   `epw_monthly_temps` — monthly temps from EPW header, if available
    fn configure_ground_source(
        &mut self,
        _t_mean: f64,
        _amplitude: f64,
        _phase_day: f64,
        _soil_diffusivity: f64,
        _loop_depth: f64,
        _epw_monthly_temps: Option<[f64; 12]>,
    ) {
    }

    /// Name of the ambient zone for this component, if applicable.
    /// Returns `Some("outdoor")`, `Some("ground")`, or `Some(zone_name)`
    /// for duct components. Returns `None` for all other components.
    fn ambient_zone(&self) -> Option<&str> {
        None
    }

    /// Additional component-specific output variables beyond the standard set.
    ///
    /// Reports `snake_case_variable_name` → value pairs by calling `out(name,
    /// value)` for each. Values should use natural SI units (W, kg/s, °C, Pa,
    /// etc.). This visitor form lets the caller write directly into its own
    /// output map without each component allocating a fresh `HashMap` per
    /// timestep (the simulation hot path). Use [`Self::detailed_outputs`] when
    /// a materialized map is convenient (e.g. tests).
    ///
    /// ```text
    /// // Example from a cooling coil:
    /// out("sensible_cooling_rate", 12000.0);  // W
    /// out("latent_cooling_rate",   3000.0);   // W
    /// out("apparatus_dewpoint",    10.5);     // °C
    /// ```
    fn report_outputs(&self, out: &mut dyn FnMut(&str, f64)) {
        let _ = out;
    }

    /// Convenience wrapper that materializes [`Self::report_outputs`] into a
    /// `HashMap`. Allocates; prefer `report_outputs` on the hot path.
    fn detailed_outputs(&self) -> std::collections::HashMap<String, f64> {
        let mut m = std::collections::HashMap::new();
        self.report_outputs(&mut |k, v| {
            m.insert(k.to_string(), v);
        });
        m
    }
}

/// Trait for plant-side components (boilers, chillers, pumps, etc.).
///
/// Every plant component takes water in and produces water out.
/// A boiler doesn't "know" it's on a hot water loop — it just adds heat to fluid.
pub trait PlantComponent: std::fmt::Debug {
    /// Component name.
    fn name(&self) -> &str;

    /// What kind of component this is, for energy accounting.
    /// Defaults to `Other` — override to get automatic end-use routing.
    fn component_kind(&self) -> ComponentKind {
        ComponentKind::Other
    }

    /// Simulate this component for one timestep.
    ///
    /// Takes inlet water conditions and a requested thermal load, returns
    /// outlet water conditions.
    ///
    /// **`load`**: requested thermal load in Watts.
    /// Positive = heating requested (raise water temperature).
    /// Negative = cooling requested (lower water temperature).
    /// The component should deliver up to its capacity and update its
    /// internal `thermal_output()` accordingly.
    ///
    /// **Zero flow**: when `inlet.state.mass_flow <= 0.0`, return `*inlet`
    /// unchanged and set all internal power/thermal outputs to zero.
    ///
    /// `ctx` provides the current timestep, outdoor conditions, and whether
    /// this is a sizing run.
    fn simulate_plant(
        &mut self,
        inlet: &WaterPort,
        load: f64,
        ctx: &SimulationContext,
    ) -> WaterPort;

    /// Rated (maximum) thermal capacity [W]. Used for PLR-based staging.
    /// Returns `f64::INFINITY` if unconstrained (default).
    fn rated_capacity(&self) -> f64 {
        f64::INFINITY
    }

    /// Design water flow rate for autosizing [m³/s]. Returns None if not applicable.
    fn design_water_flow_rate(&self) -> Option<f64> {
        None
    }

    /// Set the design water flow rate (called during autosizing).
    fn set_design_water_flow_rate(&mut self, _flow: f64) {}

    /// Instantaneous electric power consumption [W]. Always positive.
    ///
    /// Report all electric power here — pump motors, chiller compressors, etc.
    fn power_consumption(&self) -> f64 {
        0.0
    }

    /// Instantaneous fuel energy rate [W equivalent]. Always positive.
    ///
    /// Only non-electric fuel (gas, oil). For a gas boiler this is
    /// `heating_rate / thermal_efficiency`.
    fn fuel_consumption(&self) -> f64 {
        0.0
    }

    /// Net thermal output delivered to the fluid [W].
    ///
    /// Positive = heat added to fluid (boiler heating water).
    /// Negative = heat removed from fluid (chiller cooling water).
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

    /// Additional component-specific output variables beyond the standard set.
    ///
    /// Reports `snake_case_variable_name` → value pairs by calling `out(name,
    /// value)` for each. Values should use natural SI units (W, kg/s, °C, Pa,
    /// etc.). This visitor form lets the caller write directly into its own
    /// output map without each component allocating a fresh `HashMap` per
    /// timestep (the simulation hot path). Use [`Self::detailed_outputs`] when
    /// a materialized map is convenient (e.g. tests).
    ///
    /// ```text
    /// // Example from a boiler:
    /// out("plr", 0.75);         // part-load ratio [0-1]
    /// out("efficiency", 0.82);  // current thermal efficiency [0-1]
    /// out("outlet_temp", 82.0); // °C
    /// ```
    fn report_outputs(&self, out: &mut dyn FnMut(&str, f64)) {
        let _ = out;
    }

    /// Convenience wrapper that materializes [`Self::report_outputs`] into a
    /// `HashMap`. Allocates; prefer `report_outputs` on the hot path.
    fn detailed_outputs(&self) -> std::collections::HashMap<String, f64> {
        let mut m = std::collections::HashMap::new();
        self.report_outputs(&mut |k, v| {
            m.insert(k.to_string(), v);
        });
        m
    }
}

/// Context passed to every component during simulation.
///
/// Provides time, weather, and simulation mode info that components need
/// to compute their physics. Components should not store references to
/// this — it changes every timestep.
#[derive(Debug, Clone)]
pub struct SimulationContext {
    /// Current timestep info (month, day, hour, sub-hour, dt).
    /// Use `timestep.dt` for the integration interval in seconds.
    pub timestep: crate::types::TimeStep,
    /// Current outdoor air conditions (temperature, humidity, pressure).
    /// Use for equipment that depends on ambient conditions (e.g., air-cooled
    /// condensers, economizers, cooling towers).
    pub outdoor_air: MoistAirState,
    /// Current day type (weekday, weekend, holiday, design day, etc.).
    /// Use for schedule lookups when component behavior varies by day type.
    pub day_type: crate::types::DayType,
    /// When `true`, this is a design-day sizing run. Components should use
    /// design conditions (rated capacities, design flows) rather than
    /// schedule-modulated values. Autosized fields may not yet be resolved
    /// on the first sizing pass.
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
    /// HVAC supply air humidity ratio per zone [kg/kg]
    pub supply_humidity_ratios: HashMap<String, f64>,
    /// Zones where outdoor air (ventilation) is handled by the HVAC supply stream.
    /// When true, the zone's own outdoor_air specification is suppressed to avoid
    /// double-counting. When false (e.g., PTAC/FCU with separate ERV), the zone
    /// receives outdoor air directly at outdoor temperature.
    pub oa_handled_by_hvac: HashMap<String, bool>,
    /// Radiant heat injected to zone surfaces from radiant HVAC panels [W].
    /// Distributed across surfaces by area × absorptance, same as internal gains radiative split.
    /// Positive = heat gain to zone surfaces.
    pub radiant_gains: HashMap<String, f64>,
    /// Convective sensible heat deposited into a zone by HVAC distribution losses
    /// (e.g. supply-duct conduction and supply-air leakage when the duct runs
    /// through this zone) [W]. Positive = heat gain to the zone air.
    pub other_sensible_gains: HashMap<String, f64>,
    /// Latent heat deposited into a zone by HVAC distribution losses (moisture
    /// carried by leaked supply air) [W]. Positive = moisture gain to the zone.
    pub other_latent_gains: HashMap<String, f64>,
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
