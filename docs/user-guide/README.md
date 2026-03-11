# OpenBSE User Guide

OpenBSE (**Open Building Simulation Engine**) is a modern, open-source building energy simulation engine written in Rust. It replaces the complexity of legacy simulation tools with a clean YAML-based input format — no nodes, branches, branch lists, or connector lists. You describe your building and HVAC system; the engine builds the simulation graph automatically.

## Table of Contents

- [Installation](#installation)
- [Quick Start](#quick-start)
- [Input File Format](#input-file-format)
  - [Simulation Settings](#simulation-settings)
  - [Weather Files](#weather-files)
  - [Design Days](#design-days)
  - [Schedules](#schedules)
  - [Air Loops](#air-loops)
  - [Plant Loops](#plant-loops)
  - [Zone Groups](#zone-groups)
  - [Controls](#controls)
  - [Materials](#materials)
  - [Constructions](#constructions)
  - [Simple Constructions](#simple-constructions)
  - [Window Constructions](#window-constructions)
  - [Zones](#zones)
  - [Surfaces](#surfaces)
  - [Parametric Runs](#parametric-runs)
- [Output](#output)
- [Autosizing](#autosizing)
- [Units Reference](#units-reference)
- [Example Models](#example-models)
- [Future Improvements](#future-improvements)

---

## Installation

### Prerequisites

- **Rust toolchain** (1.70 or later): install from [rustup.rs](https://rustup.rs/)
- **Git**

### Build from Source

```bash
git clone https://github.com/NatLabRockies/OpenBSE.git
cd OpenBSE
cargo build --release
```

The compiled binary will be at `target/release/openbse` (once a CLI binary is added).

### Run Tests

```bash
cargo test --workspace
```

All 180 tests should pass with zero warnings.

### Use as a Library

Add OpenBSE crates to your `Cargo.toml`:

```toml
[dependencies]
openbse-io = { path = "crates/openbse-io" }
openbse-core = { path = "crates/openbse-core" }
openbse-envelope = { path = "crates/openbse-envelope" }
```

### Minimal Rust Example

```rust
use openbse_io::input::{parse_model_yaml, build_graph, build_controllers, build_envelope};
use openbse_core::simulation::SimulationRunner;
use openbse_weather::read_epw_file;
use std::path::Path;

fn main() {
    // 1. Parse the model
    let yaml = std::fs::read_to_string("examples/simple_heating.yaml").unwrap();
    let model = parse_model_yaml(&yaml).unwrap();

    // 2. Build simulation components
    let mut graph = build_graph(&model).unwrap();
    let controllers = build_controllers(&model);
    let config = model.simulation.to_config();

    // 3. Load weather
    let weather = read_epw_file(Path::new(&model.weather_files[0])).unwrap();
    let weather_hours = weather.hours;

    // 4. Build envelope (returns None for HVAC-only models)
    let mut envelope = build_envelope(&model, weather.location.latitude,
                                       weather.location.longitude,
                                       weather.location.time_zone);

    // 5. Run simulation
    let mut runner = SimulationRunner::new(config);
    // ... run with envelope, controls, and graph
}
```

---

## Quick Start

Create a file called `my_building.yaml`:

```yaml
simulation:
  timesteps_per_hour: 1
  start_month: 1
  start_day: 1
  end_month: 1
  end_day: 31

weather_files:
  - weather/Denver.epw

air_loops:
  - name: Main AHU
    controls:
      cooling_supply_temp: 13.0
      heating_supply_temp: 35.0
      cycling: proportional
      design_zone_flow: 0.5
    equipment:
      - type: heating_coil
        name: Main Coil
        source: electric
        capacity: 50000.0
      - type: fan
        name: Supply Fan
        source: constant_volume
        design_flow_rate: 2.0
    zone_terminals:
      - zone: Office

zone_groups:
  - name: All Zones
    zones: [Office]

thermostats:
  - name: Office Thermostat
    zones: [All Zones]
    heating_setpoint: 21.1
    cooling_setpoint: 23.9
```

This defines a single air handling unit with a heating coil and fan, serving one zone with thermostat control. The engine automatically builds the simulation graph, connects components in sequence, and runs the simulation loop.

---

## Input File Format

OpenBSE models are written in YAML. Every section except `simulation` and `weather_files` is optional — you only define what your model needs.

### Simulation Settings

```yaml
simulation:
  timesteps_per_hour: 1    # 1, 2, 4, 6, 10, 12, 15, 20, 30, or 60
  start_month: 1           # 1–12
  start_day: 1             # 1–31
  end_month: 12            # 1–12
  end_day: 31              # 1–31
```

| Field | Default | Description |
|-------|---------|-------------|
| `timesteps_per_hour` | 1 | Number of simulation timesteps per hour |
| `start_month` | 1 | Simulation start month |
| `start_day` | 1 | Simulation start day |
| `end_month` | 12 | Simulation end month |
| `end_day` | 31 | Simulation end day |

### Weather Files

```yaml
weather_files:
  - "weather/USA_CO_Denver.Intl.AP.725650_TMY3.epw"
  - "weather/Denver_2020_AMY.epw"   # multi-year support
```

OpenBSE reads standard EnergyPlus Weather (EPW) files. Multiple weather files can be listed for multi-year simulation runs.

### Design Days

```yaml
design_days:
  - name: Denver Heating 99.6%
    design_temp: -17.8       # Max or min outdoor dry-bulb [°C]
    daily_range: 0.0         # Daily dry-bulb range [°C]
    humidity_type: wetbulb   # wetbulb, dewpoint, humidity_ratio, or enthalpy
    humidity_value: -17.8    # Value corresponding to humidity_type
    pressure: 83411.0        # Barometric pressure [Pa]
    wind_speed: 2.3          # [m/s]
    month: 1
    day: 21
    day_type: winter         # winter or summer
```

### Schedules

Schedules define time-varying fractional multipliers (0.0-1.0) that control when internal gains, exhaust fans, and other loads are active. Schedules are referenced by name from other objects.

```yaml
schedules:
  - name: Retail Occupancy
    weekday:  [0,0,0,0,0,0,0,0.1,0.5,0.9,1.0,1.0,0.8,1.0,1.0,1.0,1.0,1.0,0.8,0.5,0.2,0,0,0]
    weekend:  [0,0,0,0,0,0,0,0,0,0.3,0.5,0.7,0.7,0.7,0.7,0.5,0.3,0.1,0,0,0,0,0,0]
    holiday:  [0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0]
```

Each schedule has 24 hourly values (index 0 = midnight to 1am, index 23 = 11pm to midnight). Values are clamped to [0.0, 1.0].

| Field | Default | Description |
|-------|---------|-------------|
| `weekday` | all 1.0 | Hourly fractions for Monday-Friday |
| `weekend` | same as weekday | Hourly fractions for Saturday-Sunday |
| `saturday` | same as weekend | Override for Saturday only |
| `sunday` | same as weekend | Override for Sunday only |
| `monday`..`friday` | same as weekday | Override for a specific weekday |
| `holiday` | same as weekend | Hourly fractions for holidays |

#### Compact Schedule Format

As an alternative to listing 24 values, schedules support a compact string notation inspired by EnergyPlus `Schedule:Compact`. Use the `compact` field instead of explicit arrays:

```yaml
schedules:
  - name: Office Occupancy
    compact:
      weekday: "0 until 8:00, 1.0 until 18:00, 0.5 until 22:00, 0"
      weekend: "0 until 10:00, 0.5 until 14:00, 0"
      holiday: "0"
      monday: "0 until 7:00, 1.0 until 19:00, 0"
      friday: "0 until 8:00, 0.8 until 16:00, 0"
```

**Compact syntax rules:**

- Comma-separated list of `<value> [until HH:MM]` pairs
- The first value starts at hour 0 (midnight)
- `until HH:MM` means the preceding value applies from the previous boundary up to that hour
- A bare value without `until` sets the value for remaining hours (must be last)
- A single value (e.g., `"0"` or `"1"`) means constant all day

**Day type resolution order:**

1. Individual day overrides (`monday`, `tuesday`, ..., `friday`, `saturday`, `sunday`) take highest priority
2. Group defaults (`weekday` for Mon–Fri, `weekend` for Sat–Sun) apply when no individual override exists
3. `holiday` applies on holidays

Both formats can coexist in the same model — some schedules can use compact notation while others use explicit arrays. The `compact` field and explicit arrays cannot both be specified on the same schedule.

Internal gains can reference schedules by name using the `schedule` field:

```yaml
internal_gains:
  - type: people
    count: 10.0
    activity_level: 120.0
    schedule: Retail Occupancy
  - type: lights
    power: 1600.0
    schedule: Retail Occupancy
  - type: equipment
    power: 1100.0
    schedule: Retail Occupancy
```

Two built-in schedules are always available: `always_on` (1.0 at all times) and `always_off` (0.0 at all times). If no schedule is specified, the gain runs at 100% at all times.

### Air Loops

An air loop defines a series of supply-side equipment and the zone terminals it serves. Equipment is connected in the order listed — air flows through them sequentially. System behavior (PSZ-AC, DOAS, FCU, VAV, PTAC) is auto-detected from the equipment and controls configuration. Zone connections are listed under `zone_terminals:` (the legacy key `zones:` is accepted as an alias).

```yaml
air_loops:
  - name: Main AHU
    controls:
      cooling_supply_temp: 13.0       # Target supply air temp for cooling [°C]
      heating_supply_temp: 35.0       # Target supply air temp for heating [°C]
      cycling: proportional           # proportional or on_off
      deadband: 1.0                   # Deadband width [°C]
      design_zone_flow: 0.5           # Design air mass flow per zone [kg/s]
      minimum_damper_position: 0.20   # Minimum outdoor air fraction [0-1]
      economizer:
        economizer_type: differential_dry_bulb
    min_vav_fraction: 0.30            # Minimum VAV box flow fraction [0-1] (VAV only)
    equipment:
      - type: heating_coil
        name: Main Heating Coil
        source: electric              # electric, hot_water, or gas
        capacity: 50000.0             # [W] or autosize
        setpoint: 35.0                # Outlet air temperature [°C]
        efficiency: 1.0               # Coil efficiency
      - type: cooling_coil
        name: DX Cooling
        source: dx                    # dx or chilled_water
        capacity: 10500.0             # [W] or autosize
        cop: 3.5                      # Coefficient of Performance
        shr: 0.8                      # Sensible Heat Ratio [0-1]
        rated_airflow: 0.5            # [m³/s] or autosize
        setpoint: 13.0                # Outlet air temperature [°C]
      - type: fan
        name: Supply Fan
        source: constant_volume       # constant_volume, vav, or on_off
        design_flow_rate: 2.0         # [m³/s] or autosize
        pressure_rise: 600.0          # [Pa]
        motor_efficiency: 0.9
        impeller_efficiency: 0.78     # total_eff = motor_eff × impeller_eff
        motor_in_airstream_fraction: 1.0
      - type: heat_recovery
        name: Energy Recovery
        source: wheel                 # wheel or plate
        sensible_effectiveness: 0.76
        latent_effectiveness: 0.68
    zone_terminals:
      - zone: East Office
      - zone: West Office
```

#### Equipment Order (Supply Air Path)

The `equipment:` list defines the supply-side airflow path. Components are connected in series — air flows through them in the order listed. The ordering convention follows the physical path air takes from outdoor air intake to supply duct:

```yaml
equipment:
  # 1. Heat recovery (if present) — pre-conditions outdoor air using exhaust
  - type: heat_recovery
    name: Energy Recovery Wheel
    source: wheel
    sensible_effectiveness: 0.76
    latent_effectiveness: 0.68

  # 2. Outdoor air mixer — blends outdoor air with return air
  #    (automatic — controlled by minimum_damper_position and economizer settings)

  # 3. Heating coil — first-stage heating (preheat in VAV systems)
  - type: heating_coil
    name: Preheat Coil
    source: hot_water
    capacity: autosize

  # 4. Cooling coil — cools mixed air to supply temperature
  - type: cooling_coil
    name: DX Cooling Coil
    source: dx
    capacity: autosize
    cop: 3.5

  # 5. Supplemental heating coil (optional) — reheat after cooling for humidity control
  - type: heating_coil
    name: Reheat Coil
    source: electric
    capacity: autosize

  # 6. Humidifier (if present) — adds moisture to supply air
  - type: humidifier
    name: Steam Humidifier
    capacity: 5.0

  # 7. Fan — moves air through the system (last before supply duct)
  - type: fan
    name: Supply Fan
    source: vav
    design_flow_rate: autosize
```

This ordering matters because each component modifies the air state for downstream components. For example, placing the cooling coil before the heating coil ensures the cooling coil sees the mixed (not preheated) air temperature, matching typical AHU configurations.

#### System type auto-detection

The engine determines the air loop's control behavior from its equipment and controls. You can use either the short codes or descriptive aliases for `system_type`:

| Detected Behavior | Short Code | Descriptive Alias | Condition |
|-------|------------|-------------------|-------------|
| **PSZ-AC** | `psz_ac` | `packaged_single_zone` | Default — single-zone packaged unit with return-air mixing |
| **DOAS** | `doas` | `dedicated_outdoor_air` | `minimum_damper_position: 1.0` (100% outdoor air) |
| **FCU** | `fcu` | `fan_coil_unit` | Must be specified with `system_type: fcu` (no outdoor air mixing) |
| **VAV** | `vav` | `variable_air_volume` | Equipment includes a fan with `source: vav` |
| **PTAC** | `ptac` | `packaged_terminal` | Packaged terminal air conditioner / heat pump |

#### Air loop controls defaults

| Field | Default | Description |
|-------|---------|-------------|
| `cooling_supply_temp` | 13.0 °C | Target cooling supply air temperature |
| `heating_supply_temp` | 35.0 °C | Target heating supply air temperature |
| `cycling` | `proportional` | Capacity control method (`proportional` or `on_off`) |
| `deadband` | 1.0 °C | Thermostat deadband width |
| `design_zone_flow` | 0.5 kg/s | Design air mass flow per zone |
| `minimum_damper_position` | auto-calculated | Minimum outdoor air fraction [0-1] |
| `min_vav_fraction` | 0.30 | Minimum VAV box flow fraction [0-1] |

#### Fan defaults

| Field | Default |
|-------|---------|
| `source` | `constant_volume` |
| `design_flow_rate` | (required, or `autosize`) |
| `pressure_rise` | 600.0 Pa |
| `motor_efficiency` | 0.9 |
| `impeller_efficiency` | 0.78 |
| `motor_in_airstream_fraction` | 1.0 |

Fan `source` values: `constant_volume`, `vav`, `on_off`.

#### Heating coil defaults

| Field | Default |
|-------|---------|
| `source` | `electric` |
| `capacity` | (required, or `autosize`) |
| `setpoint` | 35.0 °C |
| `efficiency` | 1.0 |

Heating coil `source` values: `electric`, `hot_water`, `gas`. For gas coils, `efficiency` represents burner efficiency (e.g., 0.80 for 80%). For `hot_water` coils, `efficiency` is ignored because the boiler on the plant loop handles combustion efficiency:

```yaml
- type: heating_coil
  name: Furnace
  source: gas
  capacity: 15000.0       # [W] or autosize
  setpoint: 35.0          # [°C]
  efficiency: 0.80        # Burner efficiency for gas

- type: heating_coil
  name: HW Coil
  source: hot_water
  capacity: autosize       # Boiler handles efficiency
  setpoint: 35.0
```

#### Cooling coil defaults

| Field | Default |
|-------|---------|
| `source` | `dx` |
| `capacity` | (required, or `autosize`) |
| `cop` | 3.5 |
| `shr` | 0.8 |
| `rated_airflow` | `autosize` |
| `setpoint` | 13.0 °C |
| `cap_ft_curve` | none (linear fallback) |
| `eir_ft_curve` | none (linear fallback) |

Cooling coil `source` values: `dx`, `chilled_water`.

#### Heat recovery defaults

| Field | Default |
|-------|---------|
| `source` | `wheel` |
| `sensible_effectiveness` | 0.76 |
| `latent_effectiveness` | 0.0 |

Heat recovery `source` values: `wheel` (sensible + latent), `plate` (sensible only).

### Plant Loops

A plant loop defines supply-side equipment for hot water or chilled water distribution. Both boilers (heating) and chillers (cooling) are supported as supply equipment.

```yaml
plant_loops:
  - name: Hot Water Loop
    design_supply_temp: 82.0     # [°C]
    design_delta_t: 11.0         # [°C]
    supply_equipment:
      - type: boiler
        name: Main Boiler
        capacity: 100000.0       # [W] or autosize
        efficiency: 0.80
        design_outlet_temp: 82.0 # [°C]
        design_water_flow_rate: 0.001  # [m³/s] or autosize

  - name: Chilled Water Loop
    design_supply_temp: 7.0      # [°C]
    design_delta_t: 5.0          # [°C]
    supply_equipment:
      - type: chiller
        name: Air Cooled Chiller
        capacity: 50000.0        # [W] or autosize
        cop: 3.5                 # Rated COP at ARI conditions
        chw_setpoint: 7.0        # Chilled water supply temp [°C]
        design_chw_flow: 0.005   # [m³/s] (auto-calculated from capacity if 0)
```

#### Plant loop defaults

| Field | Default |
|-------|---------|
| `design_supply_temp` | 82.0 °C |
| `design_delta_t` | 11.0 °C |

#### Boiler defaults

| Field | Default |
|-------|---------|
| `capacity` | (required, or `autosize`) |
| `efficiency` | 0.80 |
| `design_outlet_temp` | 82.0 °C |
| `design_water_flow_rate` | `autosize` |

#### Chiller defaults

| Field | Default |
|-------|---------|
| `capacity` | (required, or `autosize`) |
| `cop` | 3.5 |
| `chw_setpoint` | 7.0 °C |
| `design_chw_flow` | Auto-calculated from capacity if 0 or not specified |

#### Pumps

Pumps circulate water through plant loops. They can be added alongside boilers and chillers in the `supply_equipment` list.

```yaml
supply_equipment:
  - type: pump
    name: HW Pump
    pump_type: variable_speed         # constant_speed or variable_speed
    design_flow_rate: autosize        # [m³/s]
    design_head: 179352.0             # [Pa] (~60 ft H2O)
    motor_efficiency: 0.9
    impeller_efficiency: 0.667
    role: single                      # single, primary, secondary, or headered
    control_strategy: demand          # demand, continuous, or staged
```

| Field | Default | Description |
|-------|---------|-------------|
| `pump_type` | `variable_speed` | `constant_speed` or `variable_speed` |
| `design_flow_rate` | (required, or `autosize`) | Design water flow rate [m³/s] |
| `design_head` | 179352.0 Pa | Design pump head (~60 ft H2O) |
| `motor_efficiency` | 0.9 | Motor efficiency [0-1] |
| `impeller_efficiency` | 0.667 | Impeller efficiency [0-1]; total = motor * impeller |
| `role` | `single` | `single`, `primary`, `secondary`, or `headered` |
| `control_strategy` | `demand` | `demand` (follows load), `continuous` (constant speed), `staged` (plant-controlled) |
| `num_pumps` | 1 | Number of pumps in headered configuration |
| `power_curve` | [0,0,0,1] | Part-load curve [c1,c2,c3,c4]: power_frac = c1 + c2*PLR + c3*PLR^2 + c4*PLR^3 |

### Zone Groups

Zone groups are named collections of zones that can be referenced by name from thermostats, people, lights, equipment, infiltration, ventilation, outdoor air, and exhaust fan definitions. They reduce repetition when multiple zones share the same loads or setpoints. Define zone groups before people/lights/equipment in the YAML for clarity.

```yaml
zone_groups:
  - name: Office Zones
    zones:
      - East Office
      - West Office

# Zone groups can be referenced anywhere a zone list is expected:
people:
  - name: Office Occupants
    zones: [Office Zones]             # Applies to all zones in the group
    people_per_area: 0.0538

lights:
  - name: Office Lighting
    zones: [Office Zones]
    watts_per_area: 10.76

thermostats:
  - name: Office Thermostat
    zones: [Office Zones]             # References zone group name
    heating_setpoint: 21.1            # [°C] (70°F)
    cooling_setpoint: 23.9            # [°C] (75°F)
    unoccupied_heating_setpoint: 15.6 # [°C] (optional)
    unoccupied_cooling_setpoint: 29.4 # [°C] (optional)
```

| Field | Default | Description |
|-------|---------|-------------|
| `heating_setpoint` | 21.1 °C | Occupied heating setpoint |
| `cooling_setpoint` | 23.9 °C | Occupied cooling setpoint |
| `unoccupied_heating_setpoint` | 15.6 °C | Unoccupied heating setpoint |
| `unoccupied_cooling_setpoint` | 29.4 °C | Unoccupied cooling setpoint |

The thermostat automatically determines the mode (heating, cooling, or deadband) and modulates supply air temperature and flow rate proportionally to the deviation from setpoint.

### Controls

Explicit setpoint overrides for components and plant loops.

```yaml
controls:
  # Fixed setpoint on an air-side component
  - type: setpoint
    name: Main Coil Setpoint
    component: Main Heating Coil    # Must match equipment name
    value: 35.0                     # [°C]

  # Fixed setpoint on a plant loop
  - type: plant_loop_setpoint
    name: HW Loop Setpoint
    loop_name: Hot Water Loop       # Must match plant loop name
    supply_temp: 82.0               # [°C]
```

### Materials

Define opaque material layers for wall, roof, and floor constructions.

```yaml
materials:
  - name: Concrete
    conductivity: 1.311          # Thermal conductivity [W/(m·K)]
    density: 2240.0              # [kg/m³]
    specific_heat: 836.8         # [J/(kg·K)]
    thickness: 0.2               # [m]
    solar_absorptance: 0.7       # [0–1]
    thermal_absorptance: 0.9     # [0–1]
    visible_absorptance: 0.7     # [0–1]
    roughness: medium_rough      # See roughness values below
```

**Roughness values:** `very_rough`, `rough`, `medium_rough`, `medium_smooth`, `smooth`, `very_smooth`

| Field | Default |
|-------|---------|
| `thickness` | 0.1 m |
| `solar_absorptance` | 0.7 |
| `thermal_absorptance` | 0.9 |
| `visible_absorptance` | 0.7 |
| `roughness` | `medium_rough` |

### Constructions

Define multi-layer opaque constructions. Layers are listed from outside to inside.

```yaml
constructions:
  - name: Exterior Wall
    layers:
      - Concrete       # outermost layer
      - Insulation
      - Gypsum         # innermost layer
```

### Simple Constructions

Simple constructions define opaque assemblies using overall thermal properties instead of individual material layers. Ideal for early design, ASHRAE 140 validation tests, and quick parametric studies.

```yaml
simple_constructions:
  - name: Adiabatic Assembly
    u_factor: 0.001              # Overall U-factor [W/(m²·K)]
    thickness: 0.2               # Total wall thickness [m]
    thermal_capacity: 20000.0    # Per unit area [J/(m²·K)]
    solar_absorptance: 0.5       # Outside solar absorptance [0-1]
    thermal_absorptance: 0.9     # Thermal (LW) absorptance [0-1]
```

| Field | Default |
|-------|---------|
| `thickness` | 0.2 m |
| `thermal_capacity` | 50000.0 J/(m²·K) |
| `solar_absorptance` | 0.7 |
| `thermal_absorptance` | 0.9 |
| `roughness` | `medium_rough` |

Simple constructions are referenced by name in surface definitions, just like regular constructions.

### Window Constructions

Windows use a simplified U-factor/SHGC model rather than individual glass layers.

```yaml
window_constructions:
  - name: Double Pane
    u_factor: 2.7               # [W/(m²·K)] including film coefficients
    shgc: 0.39                  # Solar Heat Gain Coefficient [0–1]
    visible_transmittance: 0.42 # [0–1]
```

| Field | Default |
|-------|---------|
| `visible_transmittance` | 0.6 |

### Zones

Define thermal zones with their volume, infiltration, internal gains, and advanced features. Volume and floor area default to 0, which triggers auto-calculation from surface vertices: volume is computed from the zone's enclosed floor polygon extruded to ceiling height, and floor area is the sum of floor-type surface areas. You only need to specify explicit values when the geometry is not modeled or when overriding the auto-calculation.

```yaml
zones:
  - name: East Office
    volume: 300.0                # [m³] (0 = auto-calculate from surfaces)
    floor_area: 100.0            # [m²] (0 = auto-calculate from floor surfaces)
    conditioned: true            # false = unconditioned (temperature floats freely)
    infiltration:
      design_flow_rate: 0.05     # [m³/s]
      # Alternative: air_changes_per_hour: 0.5
      constant_coefficient: 1.0            # A coefficient
      temperature_coefficient: 0.0         # B coefficient [1/°C]
      wind_coefficient: 0.0                # C coefficient [s/m]
      wind_squared_coefficient: 0.0        # D coefficient [s²/m²]
    outdoor_air:                 # ASHRAE 62.1 outdoor air specification
      per_person: 0.003539606    # [m³/s-person] (7.5 cfm/person)
      per_area: 0.000609599      # [m³/s-m²] (0.12 cfm/ft²)
    exhaust_fan:                 # Zone exhaust fan
      flow_rate: 0.10            # [m³/s]
      schedule: Exhaust Schedule # Optional schedule name (default: always on)
    solar_distribution:          # Interior solar distribution to surfaces
      floor_fraction: 0.642      # Fraction to floor [0-1]
      wall_fraction: 0.191       # Fraction to walls [0-1]
      ceiling_fraction: 0.167    # Fraction to ceiling [0-1]
    internal_gains:
      - type: people
        count: 10.0
        activity_level: 120.0       # [W/person]
        # Alternative: specify sensible/latent gains directly instead of activity_level
        # sensible_gain_per_person: 73.0   # [W/person]
        # latent_gain_per_person: 47.0     # [W/person]
        radiant_fraction: 0.3       # [0-1]
        schedule: Occupancy Schedule  # Optional schedule reference
      - type: lights
        power: 1600.0               # Total installed [W]
        radiant_fraction: 0.7
        return_air_fraction: 0.0
        schedule: Lighting Schedule
      - type: equipment
        power: 1100.0               # Total installed [W]
        radiant_fraction: 0.3
        schedule: Equipment Schedule
```

The infiltration model uses the EnergyPlus design flow rate equation:

```
Infiltration = Q_design * (A + B*|DT| + C*V_wind + D*V_wind^2)
```

You can specify infiltration as either `design_flow_rate` (m³/s) or `air_changes_per_hour`. If both are given, design_flow_rate takes precedence; if only ACH is given, it is converted using zone volume.

#### Zone defaults

| Field | Default | Description |
|-------|---------|-------------|
| `volume` | 0.0 (auto-calculate) | Zone air volume [m³] |
| `floor_area` | 0.0 (auto-calculate) | Zone floor area [m²] |
| `conditioned` | `true` | Whether zone has HVAC; `false` = free-floating |
| `multiplier` | 1 | Zone multiplier for identical zones |

#### Infiltration defaults

| Field | Default |
|-------|---------|
| `constant_coefficient` | 1.0 |
| `temperature_coefficient` | 0.0 |
| `wind_coefficient` | 0.0 |
| `wind_squared_coefficient` | 0.0 |

#### Outdoor air (ASHRAE 62.1)

Calculates minimum outdoor air based on occupancy and floor area. The `oa_method` field controls how the per-person and per-area contributions are combined:

- **`sum`** (default): `Total OA = (per_person * people_count) + (per_area * floor_area)`
- **`maximum`**: `Total OA = max(per_person * people_count, per_area * floor_area)`

```yaml
outdoor_air:
  per_person: 0.003539606    # [m³/s-person] (7.5 cfm/person)
  per_area: 0.000609599      # [m³/s-m²] (0.12 cfm/ft²)
  oa_method: sum             # sum (default) or maximum
```

| Field | Default | Description |
|-------|---------|-------------|
| `per_person` | 0.0 | Outdoor air per person [m³/s-person] |
| `per_area` | 0.0 | Outdoor air per floor area [m³/s-m²] |
| `oa_method` | `sum` | Combining method: `sum` or `maximum` |

#### Exhaust fan

Models air being removed from the zone (e.g., restroom exhaust, kitchen hood). The exhausted air is replaced by infiltration or transfer air.

| Field | Default | Description |
|-------|---------|-------------|
| `flow_rate` | (required) | Exhaust flow rate [m³/s] |
| `schedule` | always on | Schedule name for time-varying operation |

#### Solar distribution

Controls how transmitted solar radiation through windows is distributed to interior surfaces. If not specified, all transmitted solar goes to zone air (simplified model).

| Field | Default | Description |
|-------|---------|-------------|
| `floor_fraction` | 0.642 | Fraction of transmitted solar to floor [0-1] |
| `wall_fraction` | 0.191 | Fraction to walls [0-1] |
| `ceiling_fraction` | 0.167 | Fraction to ceiling/roof [0-1] |

### Surfaces

Define building surfaces — walls, roofs, floors, ceilings, and windows.

```yaml
surfaces:
  - name: East Office South Wall
    zone: East Office              # Zone this surface belongs to
    type: wall                     # wall, floor, roof, ceiling, or window
    construction: Exterior Wall    # Construction or window construction name
    area: 30.0                     # Gross area [m²]
    azimuth: 180.0                 # Degrees from north, clockwise (180 = south)
    tilt: 90.0                     # Degrees from horizontal (90 = vertical)
    boundary: outdoor              # outdoor, ground, adiabatic, or zone
    parent_surface: null           # For windows: name of parent wall
```

| Field | Default | Description |
|-------|---------|-------------|
| `azimuth` | 0.0 | Degrees from north, clockwise |
| `tilt` | 90.0 | 0=face-up (floor), 90=vertical (wall), 180=face-down (ceiling) |
| `boundary` | `outdoor` | Boundary condition |
| `parent_surface` | null | Required for windows; references the parent wall |

**Boundary conditions:**
- `outdoor` — exposed to weather (solar, wind, outdoor temperature)
- `ground` — in contact with ground (simplified ground temperature model)
- `adiabatic` — no heat transfer (perfectly insulated boundary)
- `zone: Other Zone Name` — interzone partition (heat transfer to adjacent zone)

**Windows:** Windows must reference a `parent_surface`. The engine automatically subtracts the window area from the parent wall's net area.

### Parametric Runs

Run the same model with different parameters automatically.

```yaml
parametrics:
  runs:
    - name: baseline
    - name: high_efficiency_fan
      overrides:
        "Supply Fan.impeller_efficiency": 0.85
    - name: larger_coil
      overrides:
        "Main Heating Coil.capacity": 75000.0
    - name: denver_2020_actual
      weather_file: "weather/Denver_2020_AMY.epw"
```

Each run can optionally override:
- **Component parameters** using `"Component Name.parameter_name": value`
- **Weather file** using `weather_file: "path/to/file.epw"`

### Performance Curves

Top-level reusable performance curves that HVAC equipment can reference by name. Curves modify rated equipment performance as a function of operating conditions.

```yaml
performance_curves:
  - name: DX Cool Cap fT
    curve_type: biquadratic
    coefficients: [0.942587793, 0.009543347, 0.000683770, -0.011042676, 0.000005249, -0.000009720]
    min_x: 12.78
    max_x: 23.89
    min_y: 18.33
    max_y: 46.11

  - name: DX Cool EIR fT
    curve_type: biquadratic
    coefficients: [0.342414409, 0.034885008, -0.000623700, 0.004977216, 0.000437951, -0.000728028]
    min_x: 12.78
    max_x: 23.89
    min_y: 18.33
    max_y: 46.11
```

Cooling coils reference curves by name:

```yaml
- type: cooling_coil
  name: DX Cooling
  source: dx
  capacity: autosize
  cop: 3.5
  cap_ft_curve: DX Cool Cap fT     # Optional: capacity modifier curve
  eir_ft_curve: DX Cool EIR fT     # Optional: EIR modifier curve
```

| Curve Type | Form | Coefficients |
|------------|------|-------------|
| `biquadratic` | f(x,y) = c1 + c2·x + c3·x² + c4·y + c5·y² + c6·x·y | 6 |
| `quadratic` | f(x) = c1 + c2·x + c3·x² | 3 |
| `cubic` | f(x) = c1 + c2·x + c3·x² + c4·x³ | 4 |
| `linear` | f(x) = c1 + c2·x | 2 |

When curves are absent, the engine falls back to simplified linear derating.

---

## Output

### CSV Format

Simulation results are written to CSV with one row per timestep:

```
Month,Day,Hour,SubHour,Main Heating Coil:outlet_temp,Main Heating Coil:mass_flow,Supply Fan:outlet_temp,...
1,1,1,1,35.0000,1.2000,35.2100,...
1,1,2,1,35.0000,1.2000,35.2100,...
```

**Standard air component output variables:**
| Variable | Unit | Description |
|----------|------|-------------|
| `outlet_temp` | °C | Outlet dry-bulb temperature |
| `outlet_w` | kg/kg | Outlet humidity ratio |
| `mass_flow` | kg/s | Air mass flow rate |
| `outlet_enthalpy` | J/kg | Outlet moist air enthalpy |

**Standard plant component output variables:**
| Variable | Unit | Description |
|----------|------|-------------|
| `outlet_temp` | °C | Outlet water temperature |
| `mass_flow` | kg/s | Water mass flow rate |

**Zone output variables** (when envelope is active):
| Variable | Unit | Description |
|----------|------|-------------|
| `zone_temp` | °C | Zone air temperature |
| `heating_load` | W | Zone heating load (positive = needs heating) |
| `cooling_load` | W | Zone cooling load (positive = needs cooling) |
| `infiltration_mass_flow` | kg/s | Infiltration air mass flow |

Parametric runs produce one CSV per run, written to a specified output directory.

### Summary Report

OpenBSE can generate a standard text summary report with monthly energy breakdown, peak loads, energy end-use summary, and unmet hours analysis (similar to EnergyPlus HTML output). Enable it in the model:

```yaml
summary_report: true     # default: true
```

When enabled, the engine produces a text report with:
- Annual and monthly heating/cooling energy [kWh]
- Peak heating and cooling loads with timestamps
- Energy end-use breakdown (fans, cooling, heating, lighting, equipment)
- Unmet heating/cooling hours with ASHRAE 90.1 compliance check

Set `summary_report: false` to suppress report generation.

---

## Autosizing

Fans and coils support automatic sizing with the `autosize` keyword. When a capacity or flow rate is set to `autosize`, the engine calculates the value from design day conditions.

Supported autosize fields:

| Component | Field | Description |
|-----------|-------|-------------|
| Fan | `design_flow_rate: autosize` | Size from design day peak airflow |
| Heating Coil | `capacity: autosize` | Size from design day heating load |
| Cooling Coil | `capacity: autosize` | Size from design day cooling load |
| Cooling Coil | `rated_airflow: autosize` | Size from design day peak airflow |
| Boiler | `capacity: autosize` | Size from design day heating load |
| Boiler | `design_water_flow_rate: autosize` | Size from capacity and delta-T |
| Chiller | `capacity: autosize` | Size from design day cooling load |

Example:

```yaml
equipment:
  - type: fan
    name: Supply Fan
    source: constant_volume
    design_flow_rate: autosize
  - type: cooling_coil
    name: DX Coil
    source: dx
    capacity: autosize
    rated_airflow: autosize
  - type: heating_coil
    name: Gas Furnace
    source: gas
    capacity: autosize
    efficiency: 0.80
```

---

## Units Reference

All inputs and outputs use SI units:

| Quantity | Unit |
|----------|------|
| Temperature | °C |
| Pressure | Pa |
| Volume flow rate | m³/s |
| Mass flow rate | kg/s |
| Power / Energy rate | W |
| Thermal conductivity | W/(m·K) |
| Specific heat | J/(kg·K) |
| Density | kg/m³ |
| Thickness / Length | m |
| Area | m² |
| Volume | m³ |
| R-value | m²·K/W |
| U-factor | W/(m²·K) |
| Solar radiation | W/m² |
| Humidity ratio | kg_water/kg_dry_air |
| Relative humidity | 0.0–1.0 (fraction) |
| Wind speed | m/s |
| Angles | degrees |
| Time | seconds (internal), hours (weather) |

---

## Example Models

Eight example models are provided in the `examples/` directory, demonstrating different system types and complexity levels:

| File | Description |
|------|-------------|
| [`simple_heating.yaml`](../../examples/simple_heating.yaml) | Basic air loop with heating coil and constant-volume fan serving 3 zones. Includes full building envelope, materials, constructions, windows, infiltration, and internal gains. |
| [`retail_rtu.yaml`](../../examples/retail_rtu.yaml) | Retail rooftop unit (PSZ-AC) with both heating and cooling coils. Demonstrates a typical packaged single-zone system with economizer controls. |
| [`doas_fancoil.yaml`](../../examples/doas_fancoil.yaml) | Dedicated outdoor air system paired with fan coil units. DOAS auto-detected via `minimum_damper_position: 1.0`. FCU loops use `system_type: fcu`. |
| [`vav_reheat.yaml`](../../examples/vav_reheat.yaml) | Variable air volume system with central cooling and zone-level reheat. Auto-detected from VAV fan (`source: vav`) with `min_vav_fraction` control. |
| [`residential_unitary.yaml`](../../examples/residential_unitary.yaml) | Residential unitary system (furnace + DX cooling). Demonstrates gas heating coils and residential-scale equipment sizing. |
| [`doe_retail_standalone.yaml`](../../examples/doe_retail_standalone.yaml) | DOE reference building with schedules, multiple zones, performance curves for DX coils, and economizer controls. |
| [`multi_year_parametric.yaml`](../../examples/multi_year_parametric.yaml) | Multi-year parametric runs with weather file and parameter overrides. |
| [`1zone_uncontrolled.yaml`](../../examples/1zone_uncontrolled.yaml) | Single-zone free-floating model — no HVAC, just building envelope. |

---

## Future Improvements

The following features are planned for future releases:

### Design Day Climate Database

Auto-populate design day conditions from ASHRAE Climatic Design Conditions by location name or weather station ID, rather than requiring manual entry of design temperatures, humidity, and pressure. Example of the envisioned syntax:

```yaml
design_days:
  - location: Boulder, CO
    heating: 99.6%     # Auto-lookup heating design conditions
    cooling: 0.4%      # Auto-lookup cooling design conditions
```

### YAML Includes

An `include:` directive for reusable material, schedule, and construction libraries. This would allow standard libraries to be maintained independently and shared across projects:

```yaml
includes:
  - standards/ashrae_90.1_2019_constructions.yaml
  - standards/doe_prototype_schedules.yaml
  - project/common_materials.yaml
```

Included files would be merged into the main model before parsing, with local definitions taking precedence over included ones for name conflicts.
