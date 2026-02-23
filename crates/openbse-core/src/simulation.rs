//! Simulation loop and runner.
//!
//! Manages timestep iteration, air/plant loop solving, and coordinates
//! the graph-based simulation order.
//!
//! The simulation loop per timestep:
//! 1. Update SystemState from weather + previous timestep results
//! 2. Run all controllers (sense → compute → produce actions)
//! 3. Apply control actions to components (setpoints, flow rates)
//! 4. Simulate all components in topological order
//! 5. Collect results

use crate::graph::*;
use crate::ports::*;
use crate::types::*;
use openbse_psychrometrics::MoistAirState;
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
    /// Maximum air loop iterations per timestep (for convergence)
    pub max_air_loop_iterations: u32,
    /// Maximum plant loop iterations per timestep
    pub max_plant_loop_iterations: u32,
    /// Convergence tolerance for loop iteration [°C]
    pub convergence_tolerance: f64,
}

impl Default for SimulationConfig {
    fn default() -> Self {
        Self {
            timesteps_per_hour: 1,
            start_month: 1,
            start_day: 1,
            end_month: 12,
            end_day: 31,
            max_air_loop_iterations: 20,
            max_plant_loop_iterations: 10,
            convergence_tolerance: 0.01,
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
}

/// The simulation runner.
pub struct SimulationRunner {
    pub config: SimulationConfig,
    pub results: Vec<TimestepResult>,
}

impl SimulationRunner {
    pub fn new(config: SimulationConfig) -> Self {
        Self {
            config,
            results: Vec::new(),
        }
    }

    /// Run the simulation over a set of weather data (no controls).
    pub fn run(
        &mut self,
        graph: &mut SimulationGraph,
        weather_hours: &[(MoistAirState, f64)],
    ) -> Result<(), SimulationError> {
        self.run_with_controls(graph, weather_hours, &ControlSignals::default())
    }

    /// Run the simulation with control signals applied each timestep.
    ///
    /// The `signals` parameter provides setpoint overrides, flow rate overrides,
    /// and plant load demands that the controls framework has computed.
    pub fn run_with_controls(
        &mut self,
        graph: &mut SimulationGraph,
        weather_hours: &[(MoistAirState, f64)],
        signals: &ControlSignals,
    ) -> Result<(), SimulationError> {
        let days_in_months: [u32; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
        let dt = 3600.0 / self.config.timesteps_per_hour as f64;

        let start_hour = self.day_of_year(self.config.start_month, self.config.start_day, &days_in_months) * 24;
        let end_hour = self.day_of_year(self.config.end_month, self.config.end_day, &days_in_months) * 24 + 24;

        let mut sim_time = start_hour as f64 * 3600.0;

        for hour_idx in start_hour..end_hour.min(weather_hours.len() as u32) {
            let (outdoor_air, _wind_speed) = &weather_hours[hour_idx as usize];

            for sub in 1..=self.config.timesteps_per_hour {
                let (month, day) = self.month_day_from_hour(hour_idx, &days_in_months);
                let hour = (hour_idx % 24) + 1;

                let timestep = TimeStep {
                    month,
                    day,
                    hour,
                    sub_hour: sub,
                    timesteps_per_hour: self.config.timesteps_per_hour,
                    sim_time_s: sim_time,
                    dt,
                };

                let ctx = SimulationContext {
                    timestep,
                    outdoor_air: *outdoor_air,
                    day_type: DayType::WeatherDay,
                    is_sizing: false,
                };

                let result = self.simulate_timestep(graph, &ctx, signals)?;
                self.results.push(result);

                sim_time += dt;
            }
        }

        Ok(())
    }

    /// Run the simulation with envelope and control signals.
    ///
    /// Full coupled simulation:
    /// 1. Solve envelope → zone temps/loads
    /// 2. Feed zone temps to controls → HVAC setpoints
    /// 3. Simulate HVAC components
    /// 4. Feed HVAC supply conditions back to envelope
    pub fn run_with_envelope(
        &mut self,
        graph: &mut SimulationGraph,
        weather_hours: &[openbse_weather::WeatherHour],
        signals: &ControlSignals,
        envelope: &mut dyn EnvelopeSolver,
    ) -> Result<(), SimulationError> {
        let days_in_months: [u32; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
        let dt = 3600.0 / self.config.timesteps_per_hour as f64;

        envelope.initialize(dt)
            .map_err(|e| SimulationError::EnvelopeError(e))?;

        let start_hour = self.day_of_year(self.config.start_month, self.config.start_day, &days_in_months) * 24;
        let end_hour = self.day_of_year(self.config.end_month, self.config.end_day, &days_in_months) * 24 + 24;

        let mut sim_time = start_hour as f64 * 3600.0;

        for hour_idx in start_hour..end_hour.min(weather_hours.len() as u32) {
            let weather_hour = &weather_hours[hour_idx as usize];
            let outdoor_air = MoistAirState::from_tdb_rh(
                weather_hour.dry_bulb,
                weather_hour.rel_humidity / 100.0,
                weather_hour.pressure,
            );

            for sub in 1..=self.config.timesteps_per_hour {
                let (month, day) = self.month_day_from_hour(hour_idx, &days_in_months);
                let hour = (hour_idx % 24) + 1;

                let timestep = TimeStep {
                    month,
                    day,
                    hour,
                    sub_hour: sub,
                    timesteps_per_hour: self.config.timesteps_per_hour,
                    sim_time_s: sim_time,
                    dt,
                };

                let ctx = SimulationContext {
                    timestep,
                    outdoor_air,
                    day_type: DayType::WeatherDay,
                    is_sizing: false,
                };

                // Build HVAC conditions from control signals for envelope
                let mut hvac = ZoneHvacConditions::default();
                for (zone, &temp) in &signals.zone_supply_temps {
                    hvac.supply_temps.insert(zone.clone(), temp);
                }
                for (zone, &flow) in &signals.zone_air_flows {
                    hvac.supply_mass_flows.insert(zone.clone(), flow);
                }

                // Solve envelope
                let env_results = envelope.solve_timestep(&ctx, weather_hour, &hvac);

                // Simulate HVAC components
                let mut result = self.simulate_timestep(graph, &ctx, signals)?;

                // Add envelope zone outputs to the result
                for (zone_name, outputs) in &env_results.zone_outputs {
                    result.component_outputs.insert(
                        format!("Zone:{}", zone_name),
                        outputs.clone(),
                    );
                }

                self.results.push(result);
                sim_time += dt;
            }
        }

        Ok(())
    }

    /// Simulate a single timestep with control signals applied.
    pub fn simulate_timestep(
        &self,
        graph: &mut SimulationGraph,
        ctx: &SimulationContext,
        signals: &ControlSignals,
    ) -> Result<TimestepResult, SimulationError> {
        let order: Vec<_> = graph.simulation_order().to_vec();
        let mut air_states: HashMap<petgraph::graph::NodeIndex, AirPort> = HashMap::new();
        let mut water_states: HashMap<petgraph::graph::NodeIndex, WaterPort> = HashMap::new();
        let mut component_outputs: HashMap<String, HashMap<String, f64>> = HashMap::new();

        let default_air = AirPort::new(ctx.outdoor_air, 1.0);
        let default_water = WaterPort::default_water();

        for &node_idx in &order {
            let predecessors = graph.predecessors(node_idx);

            match graph.component_mut(node_idx) {
                GraphComponent::Air(component) => {
                    let comp_name = component.name().to_string();

                    // Apply control signals: coil setpoint override
                    if let Some(&sp) = signals.coil_setpoints.get(&comp_name) {
                        component.set_setpoint(sp);
                    }

                    // Determine inlet: from predecessor or outdoor air
                    let mut inlet = if let Some(&pred) = predecessors.first() {
                        air_states.get(&pred).copied().unwrap_or(default_air)
                    } else {
                        default_air
                    };

                    // Apply control signals: override mass flow if specified
                    if let Some(&flow) = signals.air_mass_flows.get(&comp_name) {
                        inlet.mass_flow = flow;
                    }

                    let outlet = component.simulate_air(&inlet, ctx);

                    let mut outputs = HashMap::new();
                    outputs.insert("outlet_temp".to_string(), outlet.state.t_db);
                    outputs.insert("outlet_w".to_string(), outlet.state.w);
                    outputs.insert("mass_flow".to_string(), outlet.mass_flow);
                    outputs.insert("outlet_enthalpy".to_string(), outlet.state.h);
                    component_outputs.insert(comp_name, outputs);

                    air_states.insert(node_idx, outlet);
                }
                GraphComponent::Plant(component) => {
                    let comp_name = component.name().to_string();

                    let inlet = if let Some(&pred) = predecessors.first() {
                        water_states.get(&pred).copied().unwrap_or(default_water)
                    } else {
                        default_water
                    };

                    // Get plant load from control signals, or default to 0
                    let load = signals.plant_loads.get(&comp_name).copied().unwrap_or(0.0);

                    let outlet = component.simulate_plant(&inlet, load, ctx);

                    let mut outputs = HashMap::new();
                    outputs.insert("outlet_temp".to_string(), outlet.state.temp);
                    outputs.insert("mass_flow".to_string(), outlet.state.mass_flow);
                    component_outputs.insert(comp_name, outputs);

                    water_states.insert(node_idx, outlet);
                }
            }
        }

        Ok(TimestepResult {
            month: ctx.timestep.month,
            day: ctx.timestep.day,
            hour: ctx.timestep.hour,
            sub_hour: ctx.timestep.sub_hour,
            component_outputs,
        })
    }

    fn day_of_year(&self, month: u32, day: u32, days_in_months: &[u32; 12]) -> u32 {
        let mut doy = 0u32;
        for m in 0..(month - 1) as usize {
            doy += days_in_months[m];
        }
        doy + day - 1
    }

    fn month_day_from_hour(&self, hour_of_year: u32, days_in_months: &[u32; 12]) -> (u32, u32) {
        let day_of_year = hour_of_year / 24;
        let mut remaining = day_of_year;
        for (m, &days) in days_in_months.iter().enumerate() {
            if remaining < days {
                return ((m + 1) as u32, remaining + 1);
            }
            remaining -= days;
        }
        (12, 31)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SimulationError {
    #[error("Simulation failed to converge at timestep {0}")]
    ConvergenceFailure(f64),
    #[error("Graph error: {0}")]
    GraphError(#[from] GraphError),
    #[error("Missing weather data for hour {0}")]
    MissingWeatherData(u32),
    #[error("Envelope error: {0}")]
    EnvelopeError(String),
}
