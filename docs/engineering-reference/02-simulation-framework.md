# Simulation framework {#sec-framework}

::: {.callout-warning}
## Pre-review content

This chapter holds text moved unchanged, apart from format, from the former single-file reference (`README.md`, mostly written in March 2026). Parts of it are stale or incomplete. It will be rewritten from the code, and checked against EnergyPlus, in this chapter's review ([#181](https://github.com/bbrannon4/OpenBSE/issues/181)); see @sec-status.
:::

## Architecture Overview

OpenBSE is organized as a Rust workspace with 7 crates. The dependency graph flows downward — no circular dependencies:

```
openbse-psychrometrics          (no internal deps)
    │
    ├── openbse-weather         (psychrometrics)
    │
    ├── openbse-core            (psychrometrics, weather)
    │       │
    │       ├── openbse-components   (core, psychrometrics)
    │       ├── openbse-controls     (core, psychrometrics)
    │       └── openbse-envelope     (core, psychrometrics, weather)
    │
    └── openbse-io              (all of the above)
```

| Crate | Purpose |
|-------|---------|
| `openbse-psychrometrics` | Moist air property calculations (Hyland & Wexler) |
| `openbse-weather` | EPW weather file parsing, design day processing |
| `openbse-core` | Simulation graph, timestep loop, component/envelope traits |
| `openbse-components` | HVAC component models (fan, heating coil, cooling coil, boiler, chiller, cooling tower, heat recovery, pump) |
| `openbse-controls` | Decoupled sensor/actuator control framework |
| `openbse-envelope` | Building envelope heat balance physics |
| `openbse-io` | YAML input parsing, CSV output writing |

### Key Design Patterns

**Trait-based components.** HVAC and plant equipment implement the `AirComponent` or `PlantComponent` trait. The envelope implements the `EnvelopeSolver` trait. All three traits are defined in `openbse-core` to avoid circular dependencies.

**Graph-based simulation order.** Components are nodes in a directed graph (using petgraph). The engine computes a topological sort to determine the correct simulation order — upstream components run before downstream ones.

**Decoupled controls.** Controllers read a `SystemState` snapshot (zone temps, component outlets, loads) and produce `ControlAction` commands (setpoints, flow rates). The simulation loop converts these to `ControlSignals` that are applied to components. Controllers never touch components directly.

## Simulation Loop

**Crate:** `openbse-core` — `simulation.rs`

### Configuration

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `timesteps_per_hour` | u32 | 1 | Sub-hourly resolution |
| `start_month` / `start_day` | u32 | 1/1 | Simulation start |
| `end_month` / `end_day` | u32 | 12/31 | Simulation end |
| `max_air_loop_iterations` | u32 | 20 | Convergence iterations per air loop |
| `max_plant_loop_iterations` | u32 | 10 | Convergence iterations per plant loop |
| `convergence_tolerance` | f64 | 0.01 °C | Loop convergence threshold |

### Per-Timestep Algorithm

For each hour in the weather data, and for each sub-timestep within that hour:

1. **Build simulation context** from weather data (outdoor air state, time indices).
2. **Solve envelope** (if present):
   - Compute solar position and incident radiation on surfaces.
   - Compute internal gains and infiltration for each zone.
   - Pass HVAC supply conditions from control signals.
   - Iterate surface ↔ zone coupling (5 iterations) to converge zone temperatures.
3. **Simulate HVAC components** in topological order:
   - For each component in the graph's simulation order:
     - Determine inlet conditions from the upstream predecessor (or outdoor air if none).
     - Apply control signal overrides (setpoint, mass flow).
     - Call `simulate_air()` or `simulate_plant()`.
     - Record outlet conditions for the next component.
4. **Collect results** — each component's output variables are stored in a `TimestepResult`.

### Timestep Outputs

Each timestep produces a `TimestepResult` containing:
- Month, day, hour, sub-hour indices
- A map of component name → output variable name → value

Air components report: `outlet_temp` [°C], `outlet_w` [kg/kg], `mass_flow` [kg/s], `outlet_enthalpy` [J/kg].

Plant components report: `outlet_temp` [°C], `mass_flow` [kg/s].

Zone outputs (from envelope): `zone_temp`, `heating_load`, `cooling_load`, `infiltration_mass_flow`, etc.

## Simulation Graph

**Crate:** `openbse-core` — `graph.rs`

**Purpose:** Manages the directed acyclic graph of HVAC and plant components. Determines the order in which components are simulated so that upstream components (producing outlet conditions) run before downstream components (consuming those conditions as inlet).

### Graph Structure

- **Nodes:** Each node holds either an air-side component (`Box<dyn AirComponent>`) or a plant-side component (`Box<dyn PlantComponent>`).
- **Edges:** Connections typed as `AirFlow`, `WaterFlow`, or `AirToPlant`.
- **Implementation:** Uses `petgraph::DiGraph` internally.

### Simulation Order

After all components and connections are added, `compute_simulation_order()` performs a topological sort. This guarantees that for any edge A → B, component A is simulated before component B.

### Component Lookup

Components can be looked up by name using `node_by_name(name)`, which returns the graph node index. This is used by the controls framework to apply setpoints and flow overrides to specific components.
