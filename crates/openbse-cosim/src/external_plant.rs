//! Plant-side co-simulation proxy.
//!
//! `ExternalPlantComponent` implements `PlantComponent` by forwarding each
//! call to an external process over stdin/stdout newline-delimited JSON.
//! The engine calls a plant component once per HVAC iteration, so a
//! timestep can hold several exchanges; see the crate docs for the protocol.
//!
//! # Variable names
//!
//! **Inputs** (what OpenBSE sends to the external process), [`PLANT_INPUTS`]:
//! - `return_temp_c` — entering water temperature [°C]
//! - `return_flow_kg_s` — water mass flow rate [kg/s]
//! - `load_request_w` — load asked of this unit [W]; positive = heating,
//!   negative = cooling (a condenser loop's heat rejection is negative)
//! - `outdoor_temp_c` — outdoor dry-bulb temperature [°C]
//! - `outdoor_humidity_ratio` — outdoor humidity ratio [kg/kg]
//! - `outdoor_pressure_pa` — barometric pressure [Pa]
//! - `sim_time_s` — simulation time elapsed [s]
//! - `dt_s` — timestep duration [s]
//! - `month`, `day`, `hour`, `sub_hour` — calendar position
//!
//! **Outputs** (what the external process returns), [`PLANT_OUTPUTS`]:
//! - `supply_temp_c` — leaving water temperature [°C] *(required)*
//! - `supply_flow_kg_s` — leaving water flow [kg/s] *(optional, passes through inlet)*
//! - `power_w` — electric power consumption [W] *(optional, default 0)*
//! - `fuel_w` — fuel energy rate [W equivalent] *(optional, default 0)*
//! - `thermal_output_w` — net heat delivered to the water [W] *(optional)*
//!
//! Every name in `outputs` must be in each response. The heat the unit
//! delivers is `ṁ·cp·(T_supply − T_return)` at the return flow; a
//! reported `thermal_output_w` must agree with it within 2 % (or 10 W).

use crate::subprocess::CosimLink;
use openbse_core::ports::{ComponentKind, PlantComponent, SimulationContext, WaterPort};
use openbse_psychrometrics::FluidState;
use std::collections::HashMap;

/// Input names an external plant component can request.
pub const PLANT_INPUTS: &[&str] = &[
    "return_temp_c",
    "return_flow_kg_s",
    "load_request_w",
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

/// Output names an external plant component can return.
pub const PLANT_OUTPUTS: &[&str] = &[
    "supply_temp_c",
    "supply_flow_kg_s",
    "power_w",
    "fuel_w",
    "thermal_output_w",
];

/// Outputs every external plant component must declare.
pub const PLANT_REQUIRED_OUTPUTS: &[&str] = &["supply_temp_c"];

pub struct ExternalPlantComponent {
    pub name: String,
    /// Shell command to launch the co-simulation process, e.g. `["python", "chiller.py"]`.
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

impl std::fmt::Debug for ExternalPlantComponent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExternalPlantComponent")
            .field("name", &self.name)
            .field("command", &self.command)
            .finish()
    }
}

impl ExternalPlantComponent {
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

    fn build_inputs(
        &self,
        inlet: &WaterPort,
        load: f64,
        ctx: &SimulationContext,
    ) -> HashMap<String, f64> {
        // Names are checked against PLANT_INPUTS when the model is read.
        self.input_vars
            .iter()
            .filter_map(|var| {
                let val = match var.as_str() {
                    "return_temp_c" => inlet.state.temp,
                    "return_flow_kg_s" => inlet.state.mass_flow,
                    "load_request_w" => load,
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

impl PlantComponent for ExternalPlantComponent {
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

    fn simulate_plant(
        &mut self,
        inlet: &WaterPort,
        load: f64,
        ctx: &SimulationContext,
    ) -> WaterPort {
        self.idle();
        if inlet.state.mass_flow <= 0.0 || load == 0.0 {
            return *inlet;
        }

        let inputs = self.build_inputs(inlet, load, ctx);
        let Some(outputs) = self.link.exchange(ctx, &inputs) else {
            return *inlet;
        };

        let supply_temp = outputs["supply_temp_c"];
        let supply_flow = outputs
            .get("supply_flow_kg_s")
            .copied()
            .unwrap_or(inlet.state.mass_flow);
        let outlet = FluidState::water(supply_temp, supply_flow);
        let heat_w = inlet.state.mass_flow * inlet.state.cp * (supply_temp - inlet.state.temp);
        self.link.check_thermal(&outputs, heat_w);

        self.last_power_w = outputs.get("power_w").copied().unwrap_or(0.0);
        self.last_fuel_w = outputs.get("fuel_w").copied().unwrap_or(0.0);
        self.last_thermal_w = heat_w;

        WaterPort::new(outlet)
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
