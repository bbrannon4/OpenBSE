//! Air-side co-simulation proxy.
//!
//! `ExternalAirComponent` implements `AirComponent` by forwarding each timestep
//! to an external process over stdin/stdout newline-delimited JSON.
//!
//! # Variable names
//!
//! **Inputs** (what OpenBSE sends to the external process):
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
//! **Outputs** (what the external process must return):
//! - `outlet_temp_c` — outlet dry-bulb temperature [°C] *(required)*
//! - `outlet_humidity_ratio` — outlet humidity ratio [kg/kg] *(required)*
//! - `power_w` — electric power consumption [W] *(optional, default 0)*
//! - `fuel_w` — fuel energy rate [W equivalent] *(optional, default 0)*
//! - `thermal_output_w` — net heat added to airstream [W] *(optional, default 0)*
//! - `outlet_mass_flow_kg_s` — outlet mass flow [kg/s] *(optional, passes through inlet)*

use crate::subprocess::SubprocessTransport;
use openbse_core::ports::{AirComponent, AirPort, ComponentKind, SimulationContext};
use openbse_psychrometrics::MoistAirState;
use std::collections::HashMap;

pub struct ExternalAirComponent {
    pub name: String,
    /// Shell command to launch the co-simulation process, e.g. `["python", "ahu.py"]`.
    pub command: Vec<String>,
    /// Input variable names sent each timestep.
    pub input_vars: Vec<String>,
    /// Output variable names expected each timestep.
    pub output_vars: Vec<String>,

    transport: Option<SubprocessTransport>,
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
        Self {
            name: name.into(),
            command,
            input_vars,
            output_vars,
            transport: None,
            last_power_w: 0.0,
            last_fuel_w: 0.0,
            last_thermal_w: 0.0,
        }
    }

    fn ensure_started(&mut self) -> Result<(), String> {
        if self.transport.is_none() {
            self.transport = Some(SubprocessTransport::spawn(&self.command)?);
        }
        Ok(())
    }

    fn build_inputs(&self, inlet: &AirPort, ctx: &SimulationContext) -> HashMap<String, f64> {
        self.input_vars
            .iter()
            .map(|var| {
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
                    _ => {
                        log::warn!(
                            "cosim '{}': unknown input variable '{}', sending 0",
                            self.name,
                            var
                        );
                        0.0
                    }
                };
                (var.clone(), val)
            })
            .collect()
    }
}

impl AirComponent for ExternalAirComponent {
    fn name(&self) -> &str {
        &self.name
    }

    fn component_kind(&self) -> ComponentKind {
        ComponentKind::Other
    }

    fn simulate_air(&mut self, inlet: &AirPort, ctx: &SimulationContext) -> AirPort {
        if inlet.mass_flow <= 0.0 {
            self.last_power_w = 0.0;
            self.last_fuel_w = 0.0;
            self.last_thermal_w = 0.0;
            return *inlet;
        }

        if let Err(e) = self.ensure_started() {
            log::error!("cosim '{}': failed to start subprocess: {}", self.name, e);
            return *inlet;
        }

        let inputs = self.build_inputs(inlet, ctx);
        let time_s = ctx.timestep.sim_time_s;
        let dt_s = ctx.timestep.dt;

        let outputs = match self
            .transport
            .as_mut()
            .unwrap()
            .exchange(time_s, dt_s, &inputs)
        {
            Ok(o) => o,
            Err(e) => {
                log::error!("cosim '{}': exchange failed: {}", self.name, e);
                self.last_power_w = 0.0;
                self.last_fuel_w = 0.0;
                self.last_thermal_w = 0.0;
                return *inlet;
            }
        };

        self.last_power_w = outputs.get("power_w").copied().unwrap_or(0.0);
        self.last_fuel_w = outputs.get("fuel_w").copied().unwrap_or(0.0);
        self.last_thermal_w = outputs.get("thermal_output_w").copied().unwrap_or(0.0);

        let outlet_temp = outputs
            .get("outlet_temp_c")
            .copied()
            .unwrap_or(inlet.state.t_db);
        let outlet_w = outputs
            .get("outlet_humidity_ratio")
            .copied()
            .unwrap_or(inlet.state.w);
        let outlet_flow = outputs
            .get("outlet_mass_flow_kg_s")
            .copied()
            .unwrap_or(inlet.mass_flow);

        AirPort::new(
            MoistAirState::new(outlet_temp, outlet_w, inlet.state.p_b),
            outlet_flow,
        )
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

    fn detailed_outputs(&self) -> HashMap<String, f64> {
        HashMap::from([
            ("power_w".to_string(), self.last_power_w),
            ("fuel_w".to_string(), self.last_fuel_w),
            ("thermal_output_w".to_string(), self.last_thermal_w),
        ])
    }
}
