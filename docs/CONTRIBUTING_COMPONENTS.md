# Contributing a New Component to OpenBSE

This guide walks you through adding a new physics component to the simulation engine. It assumes you understand building physics and can write basic Rust (or Python — Rust is similar enough to pick up).

## Overview

OpenBSE models HVAC equipment as a chain of **components** connected by **ports**. Each component is a black box: air (or water) flows in, the component does something to it — heats it, cools it, moves it — and air (or water) flows out. The component reports how much energy it consumed. It does not know what loop it sits on, what zone it serves, or what other components exist.

There are two kinds of components. **Air-side components** (fans, coils, heat recovery wheels) implement the `AirComponent` trait — they receive an `AirPort` (temperature, humidity, pressure, mass flow) and return a modified `AirPort`. **Plant-side components** (boilers, chillers, pumps) implement the `PlantComponent` trait — they receive a `WaterPort` (temperature, mass flow) plus a requested thermal load, and return a modified `WaterPort`. Everything else — the simulation graph, timestep loop, controls, envelope solver — is handled for you.

## Step 1: Write the Physics

Create a new file in `crates/openbse-components/src/`. For example, `electric_baseboard.rs`.

### Air-side component template

```rust
use openbse_core::ports::*;
use openbse_core::types::*;

/// Electric baseboard heater.
///
/// Adds heat to the zone air proportional to the requested load,
/// up to its rated capacity.
#[derive(Debug)]
pub struct ElectricBaseboard {
    name: String,
    /// Rated heating capacity [W]
    capacity: f64,
    /// Current electric power draw [W]
    power: f64,
    /// Current heating output [W]
    heating_rate: f64,
    /// Outlet temperature setpoint [°C]
    setpoint: f64,
}

impl ElectricBaseboard {
    pub fn new(name: &str, capacity: f64) -> Self {
        Self {
            name: name.to_string(),
            capacity,
            power: 0.0,
            heating_rate: 0.0,
            setpoint: 20.0,
        }
    }
}

impl AirComponent for ElectricBaseboard {
    fn name(&self) -> &str {
        &self.name
    }

    fn component_kind(&self) -> ComponentKind {
        ComponentKind::HeatingCoil
    }

    fn simulate_air(&mut self, inlet: &AirPort, _ctx: &SimulationContext) -> AirPort {
        // Zero flow — heater off
        if inlet.mass_flow <= 0.0 {
            self.power = 0.0;
            self.heating_rate = 0.0;
            return *inlet;
        }

        // How much heating is needed to reach the setpoint?
        let cp = openbse_psychrometrics::cp_air_fn_w(inlet.state.w);
        let q_needed = inlet.mass_flow * cp * (self.setpoint - inlet.state.t_db);

        // Only heat (don't cool), and cap at rated capacity
        let q_delivered = q_needed.clamp(0.0, self.capacity);

        // Electric baseboard: COP = 1.0, so power = heat delivered
        self.power = q_delivered;
        self.heating_rate = q_delivered;

        // Compute outlet temperature
        let dt = q_delivered / (inlet.mass_flow * cp);
        let outlet_temp = inlet.state.t_db + dt;

        AirPort::new(
            openbse_psychrometrics::MoistAirState::new(
                outlet_temp,
                inlet.state.w,
                inlet.state.p_b,
            ),
            inlet.mass_flow,
        )
    }

    fn power_consumption(&self) -> f64 {
        self.power
    }

    fn thermal_output(&self) -> f64 {
        self.heating_rate
    }

    fn set_setpoint(&mut self, setpoint: f64) {
        self.setpoint = setpoint;
    }

    fn setpoint(&self) -> Option<f64> {
        Some(self.setpoint)
    }

    fn nominal_capacity(&self) -> Option<f64> {
        Some(self.capacity)
    }

    fn set_nominal_capacity(&mut self, cap: f64) {
        self.capacity = cap;
    }
}
```

### Plant-side component template

```rust
use openbse_core::ports::*;
use openbse_core::types::*;

#[derive(Debug)]
pub struct MyPlantComponent {
    name: String,
    // your fields here
}

impl PlantComponent for MyPlantComponent {
    fn name(&self) -> &str {
        &self.name
    }

    fn component_kind(&self) -> ComponentKind {
        ComponentKind::Boiler // pick the right variant
    }

    fn simulate_plant(
        &mut self,
        inlet: &WaterPort,
        load: f64,     // requested thermal load [W], positive = heating
        _ctx: &SimulationContext,
    ) -> WaterPort {
        // your physics here
        // return outlet water conditions
        *inlet
    }
}
```

### Register it in the crate

Add `pub mod electric_baseboard;` to `crates/openbse-components/src/lib.rs`.

## Step 2: Register It in the YAML Parser

Open `crates/openbse-io/src/input.rs`. Find the `EquipmentInput` enum (for air-side) or `PlantEquipmentInput` enum (for plant-side) and add your variant:

```rust
// Before:
pub enum EquipmentInput {
    Fan(FanInput),
    HeatingCoil(HeatingCoilInput),
    CoolingCoil(CoolingCoilInput),
    // ...
}

// After:
pub enum EquipmentInput {
    Fan(FanInput),
    HeatingCoil(HeatingCoilInput),
    CoolingCoil(CoolingCoilInput),
    // ...
    #[serde(rename = "electric_baseboard")]
    ElectricBaseboard(ElectricBaseboardInput),
}
```

Then find the `build_graph()` function and add a match arm that constructs your component:

```rust
EquipmentInput::ElectricBaseboard(eb) => {
    let component = ElectricBaseboard::new(&eb.name, eb.capacity);
    graph.add_air_component(Box::new(component))
}
```

## Step 3: Write a Unit Test

Add a `#[cfg(test)]` module at the bottom of your component file:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_relative_eq;
    use openbse_psychrometrics::MoistAirState;

    fn make_ctx() -> SimulationContext {
        SimulationContext {
            timestep: TimeStep {
                month: 1, day: 1, hour: 12, sub_hour: 1,
                timesteps_per_hour: 1, sim_time_s: 0.0, dt: 3600.0,
            },
            outdoor_air: MoistAirState::from_tdb_rh(20.0, 0.5, 101325.0),
            day_type: DayType::WeatherDay,
            is_sizing: false,
            sizing_internal_gains: SizingInternalGains::Full,
        }
    }

    #[test]
    fn test_baseboard_heats_air() {
        let mut bb = ElectricBaseboard::new("Test BB", 5000.0);
        bb.set_setpoint(25.0);

        let inlet = AirPort::new(
            MoistAirState::from_tdb_rh(20.0, 0.5, 101325.0),
            0.5, // 0.5 kg/s
        );
        let outlet = bb.simulate_air(&inlet, &make_ctx());

        // Outlet should be warmer
        assert!(outlet.state.t_db > inlet.state.t_db);
        // Power should be positive
        assert!(bb.power_consumption() > 0.0);
        // Humidity unchanged
        assert_relative_eq!(outlet.state.w, inlet.state.w, max_relative = 1e-6);
    }

    #[test]
    fn test_baseboard_zero_flow() {
        let mut bb = ElectricBaseboard::new("Test BB", 5000.0);
        let inlet = AirPort::new(
            MoistAirState::from_tdb_rh(20.0, 0.5, 101325.0),
            0.0,
        );
        let outlet = bb.simulate_air(&inlet, &make_ctx());

        assert_eq!(bb.power_consumption(), 0.0);
        assert_relative_eq!(outlet.state.t_db, inlet.state.t_db);
    }
}
```

Run: `cargo test -p openbse-components`

## Sign Conventions and Units

| Method | Units | Sign | Notes |
|---|---|---|---|
| `power_consumption()` | W | always positive | electric power only |
| `fuel_consumption()` | W | always positive | fuel energy rate (gas, oil) |
| `thermal_output()` | W | + = heating, - = cooling | heat added to fluid |
| `AirPort.mass_flow` | kg/s | always positive | |
| `AirPort.state.t_db` | °C | | dry-bulb temperature |
| `AirPort.state.w` | kg/kg | | humidity ratio |
| `AirPort.state.p_b` | Pa | | barometric pressure |
| `WaterPort.state.temp` | °C | | |
| `WaterPort.state.mass_flow` | kg/s | always positive | |

## What NOT to Touch

- **You do not need to edit `openbse-cli/src/main.rs`.** Energy accounting is handled automatically via `component_kind()`.
- **You do not need to understand the simulation graph.** The graph builder wires components together from the YAML.
- **You do not need to know how the timestep loop works.** Your `simulate_air()` or `simulate_plant()` is called once per timestep with the right inlet conditions.

## Worked Example: Electric Baseboard Heater

Here is every step from scratch:

### 1. Create the file

Create `crates/openbse-components/src/electric_baseboard.rs` with the air-side template shown in Step 1 above.

### 2. Register the module

Add to `crates/openbse-components/src/lib.rs`:

```rust
pub mod electric_baseboard;
```

### 3. Write and run the test

Add the test module from Step 3 to the bottom of `electric_baseboard.rs`.

```bash
cargo test -p openbse-components -- electric_baseboard
```

### 4. (Optional) Wire into the YAML parser

Follow Step 2 to add an `ElectricBaseboard` variant to `EquipmentInput` and a match arm in `build_graph()`. This lets users specify the component in YAML:

```yaml
air_loops:
  - name: Baseboard Loop
    equipment:
      - type: electric_baseboard
        name: Living Room Baseboard
        capacity: 5000
```

### 5. Check everything

```bash
cargo fmt --all
cargo clippy --workspace
cargo test --workspace
```

All three must pass before submitting.
