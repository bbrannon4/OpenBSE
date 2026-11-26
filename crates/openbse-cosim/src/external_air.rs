//! Air-side co-simulation proxy.
//!
//! `ExternalAirComponent` implements `AirComponent` by forwarding each call
//! to an external process over stdin/stdout newline-delimited JSON. The
//! engine calls an air component once per HVAC iteration, so a timestep can
//! hold several exchanges; see the crate docs for the protocol.
//!
//! # Variable names
//!
//! **Inputs** (what OpenBSE sends to the external process), [`AIR_INPUTS`]:
//! - `inlet_temp_c` — inlet dry-bulb temperature [°C]
//! - `inlet_humidity_ratio` — inlet humidity ratio [kg/kg]
//! - `inlet_mass_flow_kg_s` — inlet mass flow rate [kg/s]
//! - `inlet_enthalpy_j_kg` — inlet specific enthalpy [J/kg]
//! - `outdoor_temp_c` — outdoor dry-bulb temperature [°C]
//! - `outdoor_humidity_ratio` — outdoor humidity ratio [kg/kg]
//! - `outdoor_pressure_pa` — barometric pressure [Pa]
//! - `sim_time_s` — simulation time elapsed [s]
//! - `dt_s` — timestep duration [s]
//! - `month`, `day`, `hour`, `sub_hour` — calendar position
//!
//! **Outputs** (what the external process returns), [`AIR_OUTPUTS`]:
//! - `outlet_temp_c` — outlet dry-bulb temperature [°C] *(required)*
//! - `outlet_humidity_ratio` — outlet humidity ratio [kg/kg] *(required)*
//! - `power_w` — electric power consumption [W] *(optional, default 0)*
//! - `fuel_w` — fuel energy rate [W equivalent] *(optional, default 0)*
//! - `thermal_output_w` — net heat added to the airstream [W] *(optional)*
//! - `outlet_mass_flow_kg_s` — outlet mass flow [kg/s] *(optional, passes through inlet)*
//!
//! Every name in `outputs` must be in each response. The heat the unit adds
//! is `ṁ·(h_out − h_in)` at the inlet flow, from the returned state; a
//! reported `thermal_output_w` must agree with it within 2 % (or 10 W).

use crate::subprocess::CosimLink;
use openbse_core::ports::{AirComponent, AirPort, ComponentKind, SimulationContext};
use openbse_psychrometrics::MoistAirState;
use std::collections::HashMap;

/// Input names an external air component can request.
pub const AIR_INPUTS: &[&str] = &[
    "inlet_temp_c",
    "inlet_humidity_ratio",
    "inlet_mass_flow_kg_s",
    "inlet_enthalpy_j_kg",
    "outdoor_temp_c",
    "outdoor_humidity_ratio",
    "outdoor_pressure_pa",
    "sim_time_s",
    "dt_s",
    "month",
    "day",
    "hour",
    "sub_hour",
];

/// Output names an external air component can return.
pub const AIR_OUTPUTS: &[&str] = &[
    "outlet_temp_c",
    "outlet_humidity_ratio",
    "power_w",
    "fuel_w",
    "thermal_output_w",
    "outlet_mass_flow_kg_s",
];

/// Outputs every external air component must declare.
pub const AIR_REQUIRED_OUTPUTS: &[&str] = &["outlet_temp_c", "outlet_humidity_ratio"];

pub struct ExternalAirComponent {
    pub name: String,
    /// Shell command to launch the co-simulation process, e.g. `["python", "ahu.py"]`.
    pub command: Vec<String>,
    /// Input variable names sent each exchange.
    pub input_vars: Vec<String>,
    /// Output variable names expected in each response.
    pub output_vars: Vec<String>,

    link: CosimLink,
    last_power_w: f64,
    last_fuel_w: f64,
    last_thermal_w: f64,
}

impl std::fmt::Debug for ExternalAirComponent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExternalAirComponent")
            .field("name", &self.name)
            .field("command", &self.command)
            .finish()
    }
}

impl ExternalAirComponent {
    pub fn new(
        name: impl Into<String>,
        command: Vec<String>,
        input_vars: Vec<String>,
        output_vars: Vec<String>,
    ) -> Self {
        let name = name.into();
        let link = CosimLink::new(&name, command.clone(), output_vars.clone());
        Self {
            name,
            command,
            input_vars,
            output_vars,
            link,
            last_power_w: 0.0,
            last_fuel_w: 0.0,
            last_thermal_w: 0.0,
        }
    }

    /// Run the subprocess in `dir`, normally the model's directory.
    pub fn with_working_dir(mut self, dir: Option<&std::path::Path>) -> Self {
        self.link.set_working_dir(dir);
        self
    }

    /// Longest wait for one response before the run stops [s].
    pub fn with_timeout(mut self, timeout_s: f64) -> Self {
        self.link.set_timeout(timeout_s);
        self
    }

    fn build_inputs(&self, inlet: &AirPort, ctx: &SimulationContext) -> HashMap<String, f64> {
        // Names are checked against AIR_INPUTS when the model is read.
        self.input_vars
            .iter()
            .filter_map(|var| {
                let val = match var.as_str() {
                    "inlet_temp_c" => inlet.state.t_db,
                    "inlet_humidity_ratio" => inlet.state.w,
                    "inlet_mass_flow_kg_s" => inlet.mass_flow,
                    "inlet_enthalpy_j_kg" => inlet.state.h,
                    "outdoor_temp_c" => ctx.outdoor_air.t_db,
                    "outdoor_humidity_ratio" => ctx.outdoor_air.w,
                    "outdoor_pressure_pa" => ctx.outdoor_air.p_b,
                    "sim_time_s" => ctx.timestep.sim_time_s,
                    "dt_s" => ctx.timestep.dt,
                    "month" => ctx.timestep.month as f64,
                    "day" => ctx.timestep.day as f64,
                    "hour" => ctx.timestep.hour as f64,
                    "sub_hour" => ctx.timestep.sub_hour as f64,
                    _ => return None,
                };
                Some((var.clone(), val))
            })
            .collect()
    }

    fn idle(&mut self) {
        self.last_power_w = 0.0;
        self.last_fuel_w = 0.0;
        self.last_thermal_w = 0.0;
    }
}

impl AirComponent for ExternalAirComponent {
    fn name(&self) -> &str {
        &self.name
    }

    fn fatal_error(&self) -> Option<String> {
        self.link.failure()
    }

    fn set_warmup(&mut self, warmup: bool) {
        self.link.set_warmup(warmup);
    }

    fn component_kind(&self) -> ComponentKind {
        ComponentKind::Other
    }

    fn simulate_air(&mut self, inlet: &AirPort, ctx: &SimulationContext) -> AirPort {
        self.idle();
        if inlet.mass_flow <= 0.0 {
            return *inlet;
        }

        let inputs = self.build_inputs(inlet, ctx);
        let Some(outputs) = self.link.exchange(ctx, &inputs) else {
            return *inlet;
        };

        let outlet_flow = outputs
            .get("outlet_mass_flow_kg_s")
            .copied()
            .unwrap_or(inlet.mass_flow);
        let outlet_state = MoistAirState::new(
            outputs["outlet_temp_c"],
            outputs["outlet_humidity_ratio"],
            inlet.state.p_b,
        );
        let heat_w = inlet.mass_flow * (outlet_state.h - inlet.state.h);
        self.link.check_thermal(&outputs, heat_w);

        self.last_power_w = outputs.get("power_w").copied().unwrap_or(0.0);
        self.last_fuel_w = outputs.get("fuel_w").copied().unwrap_or(0.0);
        self.last_thermal_w = heat_w;

        AirPort::new(outlet_state, outlet_flow)
    }

    fn power_consumption(&self) -> f64 {
        self.last_power_w
    }

    fn fuel_consumption(&self) -> f64 {
        self.last_fuel_w
    }

    fn thermal_output(&self) -> f64 {
        self.last_thermal_w
    }

    fn report_outputs(&self, out: &mut dyn FnMut(&str, f64)) {
        out("power_w", self.last_power_w);
        out("fuel_w", self.last_fuel_w);
        out("thermal_output_w", self.last_thermal_w);
    }
}
