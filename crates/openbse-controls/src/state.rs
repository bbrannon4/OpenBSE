//! System state — the shared data structure that connects sensors and actuators.
//!
//! Controllers read from SystemState (sensor side) and produce ControlActions
//! (actuator side). The simulation loop populates SystemState each timestep
//! from component outputs and weather, then applies ControlActions to components
//! before simulating them.

use openbse_psychrometrics::MoistAirState;

use std::collections::HashMap;

// ─── System State (Sensor Side) ──────────────────────────────────────────────

/// Current state of the entire system, populated by the simulation loop.
/// Controllers read from this — it's the "sensor" side of the framework.
#[derive(Debug, Clone)]
pub struct SystemState {
    /// Current outdoor air conditions
    pub outdoor_air: MoistAirState,

    /// Zone air temperatures [°C], keyed by zone name
    pub zone_temps: HashMap<String, f64>,

    /// Zone humidity ratios [kg/kg], keyed by zone name
    pub zone_humidity: HashMap<String, f64>,

    /// Zone heating loads [W], keyed by zone name (positive = needs heating)
    pub zone_heating_loads: HashMap<String, f64>,

    /// Zone cooling loads [W], keyed by zone name (positive = needs cooling)
    pub zone_cooling_loads: HashMap<String, f64>,

    /// Component outlet air temperatures [°C], keyed by component name
    pub component_outlet_temps: HashMap<String, f64>,

    /// Component outlet water temperatures [°C], keyed by component name
    pub component_outlet_water_temps: HashMap<String, f64>,

    /// Plant loop supply temperatures [°C], keyed by loop name
    pub plant_loop_temps: HashMap<String, f64>,

    /// Plant loop total loads [W], keyed by loop name
    pub plant_loop_loads: HashMap<String, f64>,

    /// Air loop supply air temperatures [°C], keyed by loop name
    pub air_loop_supply_temps: HashMap<String, f64>,
}

impl SystemState {
    pub fn new(outdoor_air: MoistAirState) -> Self {
        Self {
            outdoor_air,
            zone_temps: HashMap::new(),
            zone_humidity: HashMap::new(),
            zone_heating_loads: HashMap::new(),
            zone_cooling_loads: HashMap::new(),
            component_outlet_temps: HashMap::new(),
            component_outlet_water_temps: HashMap::new(),
            plant_loop_temps: HashMap::new(),
            plant_loop_loads: HashMap::new(),
            air_loop_supply_temps: HashMap::new(),
        }
    }

    /// Get a zone temperature, or return a default if the zone hasn't been simulated yet.
    pub fn zone_temp(&self, zone: &str) -> f64 {
        self.zone_temps.get(zone).copied().unwrap_or(21.0)
    }

    /// Get a component's outlet air temperature.
    pub fn component_outlet_temp(&self, component: &str) -> Option<f64> {
        self.component_outlet_temps.get(component).copied()
    }

    /// Get a plant loop supply temperature.
    pub fn plant_loop_temp(&self, loop_name: &str) -> Option<f64> {
        self.plant_loop_temps.get(loop_name).copied()
    }
}

// ─── Control Actions (Actuator Side) ─────────────────────────────────────────

/// A control action — the output of a controller that gets applied to a component.
///
/// Actions target components by name and set specific parameters.
/// The simulation loop matches these to graph components and applies them.
#[derive(Debug, Clone)]
pub enum ControlAction {
    /// Set a coil's outlet air temperature setpoint [°C]
    SetCoilSetpoint {
        component: String,
        setpoint: f64,
    },

    /// Set a component's air mass flow rate [kg/s]
    SetAirMassFlow {
        component: String,
        mass_flow: f64,
    },

    /// Set a plant loop supply temperature setpoint [°C]
    SetPlantLoopSetpoint {
        loop_name: String,
        setpoint: f64,
    },

    /// Set a plant component's load demand [W]
    SetPlantLoad {
        component: String,
        load: f64,
    },

    /// Set a zone's supply air flow rate [kg/s]
    SetZoneAirFlow {
        zone: String,
        mass_flow: f64,
    },

    /// Set a zone's target supply air temperature [°C]
    SetZoneSupplyTemp {
        zone: String,
        supply_temp: f64,
    },
}

impl ControlAction {
    /// Get the target component/loop name for this action.
    pub fn target(&self) -> &str {
        match self {
            ControlAction::SetCoilSetpoint { component, .. } => component,
            ControlAction::SetAirMassFlow { component, .. } => component,
            ControlAction::SetPlantLoopSetpoint { loop_name, .. } => loop_name,
            ControlAction::SetPlantLoad { component, .. } => component,
            ControlAction::SetZoneAirFlow { zone, .. } => zone,
            ControlAction::SetZoneSupplyTemp { zone, .. } => zone,
        }
    }
}
