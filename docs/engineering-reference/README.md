# OpenBSE Engineering Reference

This document describes the algorithms, equations, and physics models implemented in each OpenBSE module. It covers what each component does, its inputs and outputs, the calculations it performs, and the references for the underlying science.

## Table of Contents

- [Architecture Overview](#architecture-overview)
- [Simulation Loop](#simulation-loop)
- [Psychrometrics](#psychrometrics)
- [Weather Data](#weather-data)
- [Components](#components)
  - [Fan](#fan)
  - [Heating Coil](#heating-coil)
  - [Cooling Coil (DX)](#cooling-coil-dx)
  - [Boiler](#boiler)
  - [Chiller (Air-Cooled)](#chiller-air-cooled)
  - [Cooling Tower](#cooling-tower)
  - [Heat Recovery](#heat-recovery)
  - [Pump](#pump)
- [Controls](#controls)
  - [Zone Thermostat](#zone-thermostat)
  - [Setpoint Controller](#setpoint-controller)
  - [Plant Loop Setpoint](#plant-loop-setpoint)
- [Multi-Loop HVAC Controls](#multi-loop-hvac-controls)
  - [Control Dispatcher Architecture](#control-dispatcher-architecture)
  - [PSZ-AC (Packaged Single-Zone AC)](#psz-ac-packaged-single-zone-ac)
  - [DOAS (Dedicated Outdoor Air System)](#doas-dedicated-outdoor-air-system)
  - [FCU (Fan Coil Unit)](#fcu-fan-coil-unit)
  - [VAV (Variable Air Volume)](#vav-variable-air-volume)
  - [OA Fraction and Humidity Blending](#oa-fraction-and-humidity-blending)
- [Design Day Sizing](#design-day-sizing)
  - [Stage 1: Zone Sizing](#stage-1-zone-sizing)
  - [Stage 2: System Sizing](#stage-2-system-sizing)
- [Building Envelope](#building-envelope)
  - [Materials and Constructions](#materials-and-constructions)
  - [Simple Constructions](#simple-constructions)
  - [Conduction Transfer Functions (CTF)](#conduction-transfer-functions-ctf)
  - [Interior Convection](#interior-convection)
  - [Exterior Convection](#exterior-convection)
  - [Solar Position and Incident Radiation](#solar-position-and-incident-radiation)
  - [Interior Solar Distribution](#interior-solar-distribution)
  - [Infiltration](#infiltration)
  - [Internal Gains](#internal-gains)
  - [Schedules](#schedules)
  - [Exhaust Fans](#exhaust-fans)
  - [ASHRAE 62.1 Outdoor Air](#ashrae-621-outdoor-air)
  - [Vertex Geometry](#vertex-geometry)
  - [Ground Temperature Model](#ground-temperature-model)
  - [Zone Air Heat Balance](#zone-air-heat-balance)
  - [Heat Balance Solver](#heat-balance-solver)
- [Simulation Graph](#simulation-graph)

---

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

---

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

---

## Psychrometrics

**Crate:** `openbse-psychrometrics` — `lib.rs`

Implements moist air property calculations using the Hyland & Wexler (1983) formulation, consistent with EnergyPlus and ASHRAE Handbook of Fundamentals.

### MoistAirState

The primary data structure representing the thermodynamic state of moist air:

| Field | Type | Unit | Description |
|-------|------|------|-------------|
| `t_db` | f64 | °C | Dry-bulb temperature |
| `w` | f64 | kg/kg | Humidity ratio (kg water per kg dry air) |
| `h` | f64 | J/kg | Specific enthalpy of moist air |
| `p_b` | f64 | Pa | Barometric pressure |

Derived properties (computed on demand):
- `rh()` — relative humidity [0.0–1.0]
- `t_wb()` — wet-bulb temperature [°C]
- `t_dp()` — dew-point temperature [°C]
- `rho()` — moist air density [kg/m³]
- `v()` — specific volume [m³/kg]
- `cp()` — specific heat [J/(kg·K)]

### FluidState

Represents a liquid fluid (water):

| Field | Type | Unit | Description |
|-------|------|------|-------------|
| `temp` | f64 | °C | Fluid temperature |
| `mass_flow` | f64 | kg/s | Mass flow rate |
| `cp` | f64 | J/(kg·K) | Specific heat (4180 for water) |

### Key Functions

**Saturation pressure** (`psat_fn_temp`):

Uses the Hyland & Wexler (1983) equations for water vapor saturation pressure over ice (T < 0 °C) and over liquid water (T ≥ 0 °C). Valid for -100 °C to 200 °C.

**Enthalpy** (`h_fn_tdb_w`):

```
h = Cp_air · T_db + w · (h_fg + Cp_vapor · T_db)
```

where:
- Cp_air = 1004.84 J/(kg·K) — dry air specific heat
- Cp_vapor = 1858.95 J/(kg·K) — water vapor specific heat
- h_fg = 2,500,940 J/kg — heat of vaporization at 0 °C

**Density** (`rho_air_fn_pb_tdb_w`):

Uses the ideal gas law for moist air:

```
ρ = p_b / (R_da · T_abs · (1 + 1.6078 · w))
```

where R_da = 287.042 J/(kg·K) and T_abs = T_db + 273.15.

**Wet-bulb temperature** (`twb_fn_tdb_w_pb`):

Uses bisection search to find T_wb such that `w_fn_tdb_twb_pb(T_db, T_wb, p_b) = w`.

### Constants

| Constant | Value | Description |
|----------|-------|-------------|
| `CP_AIR` | 1004.84 J/(kg·K) | Dry air specific heat |
| `CP_VAPOR` | 1858.95 J/(kg·K) | Water vapor specific heat |
| `HFG_WATER` | 2,500,940 J/kg | Heat of vaporization at 0°C |
| `CP_WATER` | 4180 J/(kg·K) | Liquid water specific heat |
| `RHO_WATER` | 998.2 kg/m³ | Water density at ~20°C |
| `STD_PRESSURE` | 101,325 Pa | Standard atmospheric pressure |
| `MOL_MASS_RATIO` | 0.621945 | Molar mass ratio (water/air) |
| `AUTOSIZE` | -99999.0 | Sentinel value for autosizing |

---

## Weather Data

**Crate:** `openbse-weather` — `lib.rs`

### EPW File Parsing

Reads standard EnergyPlus Weather (EPW) files containing 8,760 hourly records (one year, non-leap). The parser extracts:

**Location data:**

| Field | Unit | Description |
|-------|------|-------------|
| `city` | — | City name |
| `state_province` | — | State/province |
| `country` | — | Country code |
| `latitude` | degrees | Site latitude (positive north) |
| `longitude` | degrees | Site longitude (positive east) |
| `time_zone` | hours | UTC offset |
| `elevation` | m | Site elevation above sea level |

**Hourly data:**

| Field | Unit | Description |
|-------|------|-------------|
| `dry_bulb` | °C | Dry-bulb temperature |
| `dew_point` | °C | Dew-point temperature |
| `rel_humidity` | % | Relative humidity (0–100) |
| `pressure` | Pa | Atmospheric pressure |
| `global_horiz_rad` | Wh/m² | Global horizontal radiation |
| `direct_normal_rad` | Wh/m² | Direct normal radiation |
| `diffuse_horiz_rad` | Wh/m² | Diffuse horizontal radiation |
| `wind_speed` | m/s | Wind speed |
| `wind_direction` | degrees | Wind direction (from north, clockwise) |
| `horiz_ir_rad` | Wh/m² | Horizontal infrared radiation |
| `opaque_sky_cover` | tenths | Opaque sky cover (0–10) |

### Design Days

Design days provide extreme-condition sizing data. Each includes a design temperature, humidity condition, barometric pressure, and wind speed for a specific month and day.

Humidity can be specified as wet-bulb, dew-point, humidity ratio, or enthalpy. Day types are `SummerDesign` or `WinterDesign`.

---

## Components

**Crate:** `openbse-components`

### Fan

**Module:** `fan.rs`

**Purpose:** Models air-side fans that move air through the HVAC system and add heat from motor and shaft losses.

**Types:**
- **Constant Volume** — fixed-speed fan, power proportional to flow
- **VAV** (Variable Air Volume) — variable-speed fan with power curve
- **On/Off** — cycles on or off (treated as constant volume when on)

#### Inputs

| Parameter | Unit | Default | Description |
|-----------|------|---------|-------------|
| `name` | — | required | Component name |
| `source` | — | constant_volume | Fan type: `constant_volume`, `vav`, or `on_off` |
| `design_flow_rate` | m³/s | required | Design volumetric flow rate |
| `design_pressure_rise` | Pa | 600.0 | Total static pressure rise |
| `motor_efficiency` | — | 0.9 | Motor efficiency |
| `impeller_efficiency` | — | 0.78 | Impeller/belt efficiency (total = motor × impeller) |
| `motor_in_airstream_fraction` | — | 1.0 | Fraction of motor heat entering airstream |
| `vav_coefficients` | — | see below | Power curve coefficients (VAV only) |

**Default VAV curve coefficients:** `[0.0408, 0.0880, -0.0730, 0.9437, 0.0]`

This is a 4th-order polynomial: `PLF = C₁ + C₂·PLR + C₃·PLR² + C₄·PLR³ + C₅·PLR⁴`

**Default constant volume coefficients:** `[1.0, 0.0, 0.0, 0.0, 0.0]` (PLF = 1 always)

#### Outputs

| Variable | Unit | Description |
|----------|------|-------------|
| `outlet_temp` | °C | Outlet dry-bulb (higher than inlet due to fan heat) |
| `outlet_w` | kg/kg | Humidity ratio (unchanged through fan) |
| `mass_flow` | kg/s | Air mass flow rate |
| `outlet_enthalpy` | J/kg | Outlet enthalpy |

Internal state: `power` [W], `heat_to_air` [W].

#### Calculations

**Reference:** EnergyPlus `Fans.cc`

**Step 1: Fan power**

For constant volume:
```
Power = ṁ · ΔP / (η_total · ρ_air)
```

For VAV:
```
flow_fraction = ṁ_actual / ṁ_design
PLF = C₁ + C₂·ff + C₃·ff² + C₄·ff³ + C₅·ff⁴
Power = Power_design · PLF
```

where `Power_design = ṁ_design · ΔP / (η_total · ρ_air)`.

**Step 2: Heat to air**
```
shaft_power = η_motor · Power
heat_to_air = shaft_power + (Power − shaft_power) · f_motor_in_airstream
```

The first term is shaft friction heat (always enters the airstream). The second term is motor waste heat, only the fraction physically in the airstream.

**Step 3: Outlet conditions**
```
h_outlet = h_inlet + heat_to_air / ṁ
T_outlet = f(h_outlet, w_inlet)    [inverse psychrometric]
w_outlet = w_inlet                 [no moisture change]
```

---

### Heating Coil

**Module:** `heating_coil.rs`

**Purpose:** Models heating coils that raise air temperature toward a setpoint. Three types: electric resistance, hot water, and gas furnace.

#### Inputs

| Parameter | Unit | Default | Description |
|-----------|------|---------|-------------|
| `name` | — | required | Component name |
| `source` | — | electric | `electric`, `hot_water`, or `gas` |
| `nominal_capacity` | W | required | Maximum heating capacity |
| `efficiency` | — | 1.0 | Heating efficiency (electric = 1.0, gas = burner efficiency, typically 0.78-0.92) |
| `outlet_temp_setpoint` | °C | 35.0 | Target outlet air temperature |
| `design_water_flow_rate` | m³/s | 0.0 | Design water flow (hot water only) |
| `design_water_inlet_temp` | °C | 82.0 | Design inlet water temp (hot water only) |
| `design_water_outlet_temp` | °C | 71.0 | Design outlet water temp (hot water only) |

#### Outputs

| Variable | Unit | Description |
|----------|------|-------------|
| `outlet_temp` | °C | Outlet air temperature (at or below setpoint) |
| `outlet_w` | kg/kg | Humidity ratio (unchanged) |
| `mass_flow` | kg/s | Air mass flow rate (unchanged) |
| `outlet_enthalpy` | J/kg | Outlet enthalpy |

Internal state: `heating_rate` [W], `energy_consumption` [W].

#### Calculations

**Reference:** EnergyPlus `HeatingCoils.cc`

**Step 1: Required heating**
```
Q_required = max(0, ṁ_air · Cp_air · (T_setpoint − T_inlet))
```

The coil only heats — if the inlet is already above setpoint, `Q_required = 0`.

**Step 2: Available capacity**

*Electric coil:*
```
Q_actual = min(Q_required, Q_capacity)
Energy_consumption = Q_actual / η
```

*Gas furnace coil:*
```
Q_actual = min(Q_required, Q_capacity)
Fuel_consumption = Q_actual / η_burner
```

The gas coil delivers `Q_actual` watts of heating to the airstream, but consumes `Q_actual / η_burner` watts of fuel energy. Typical burner efficiencies range from 0.78 (standard furnace) to 0.92 (condensing furnace). The fuel consumption is tracked separately from electric consumption for energy reporting.

*Hot water coil:*
```
Q_water_side = ṁ_water · Cp_water · max(0, T_water_in − T_water_out_design)
Q_actual = min(Q_required, Q_capacity, Q_water_side)
T_water_out = T_water_in − Q_actual / (ṁ_water · Cp_water)
```

**Step 3: Outlet conditions**
```
T_outlet = T_inlet + Q_actual / (ṁ_air · Cp_air)
h_outlet = h(T_outlet, w_inlet)
```

---

### Cooling Coil (DX)

**Module:** `cooling_coil.rs`

**Purpose:** Models a single-speed direct expansion (DX) cooling coil as found in packaged rooftop units and split systems. Uses a simplified steady-state model with capacity and COP derating based on outdoor temperature.

**Reference:** EnergyPlus Engineering Reference, "Coil:Cooling:DX:SingleSpeed"

#### Inputs

| Parameter | Unit | Default | Description |
|-----------|------|---------|-------------|
| `name` | — | required | Component name |
| `rated_capacity` | W | required | Total cooling capacity at ARI conditions (35 C outdoor, 26.7 C DB / 19.4 C WB indoor) |
| `rated_cop` | — | required | Coefficient of performance at ARI conditions (typically 3.0-5.0) |
| `rated_shr` | 0-1 | required | Sensible heat ratio at ARI conditions (typically 0.7-0.85) |
| `rated_airflow` | m3/s | required | Rated air volume flow rate |
| `outlet_temp_setpoint` | C | required | Desired outlet air temperature setpoint |

#### Outputs

| Variable | Unit | Description |
|----------|------|-------------|
| `outlet_temp` | C | Outlet air temperature (at or above setpoint if capacity-limited) |
| `outlet_w` | kg/kg | Humidity ratio (unchanged in current model) |
| `mass_flow` | kg/s | Air mass flow rate (unchanged) |
| `outlet_enthalpy` | J/kg | Outlet enthalpy |

Internal state: `cooling_rate` [W] (total), `sensible_cooling_rate` [W], `power_consumption` [W].

#### Calculations

**Step 1: Required sensible cooling**
```
Q_sensible_required = m_air * Cp_air * (T_inlet - T_setpoint)
```

The coil only cools -- if the inlet is already below the setpoint, the coil is off.

**Step 2: Capacity and COP at current conditions**

When performance curves are attached (`cap_ft_curve`, `eir_ft_curve`), the engine evaluates biquadratic functions of entering wet-bulb temperature and outdoor dry-bulb temperature:

```
Cap_modifier = cap_ft_curve(T_wb_entering, T_db_outdoor)
Available_capacity = Rated_capacity * Cap_modifier

EIR_modifier = eir_ft_curve(T_wb_entering, T_db_outdoor)
Available_COP = Rated_COP / EIR_modifier
```

When curves are absent, capacity and COP are derated linearly from ARI rated conditions (T_rated = 35 C outdoor):

```
Capacity_correction = clamp(1.0 - 0.008 * (T_outdoor - 35.0), 0.5, 1.05)
COP_correction      = clamp(1.0 - 0.012 * (T_outdoor - 35.0), 0.4, 1.10)
```

```
Available_capacity = Rated_capacity * Capacity_correction
Available_COP      = Rated_COP * COP_correction
```

**Step 3: Sensible/latent split and part-load ratio**

The sensible heat ratio determines what fraction of total cooling capacity is available as sensible cooling:

```
Available_sensible = Available_capacity * SHR
PLR = clamp(Q_sensible_required / Available_sensible, 0.0, 1.0)
```

Actual cooling delivered:
```
Q_sensible = Available_sensible * PLR
Q_total    = Available_capacity * PLR
```

**Step 4: Outlet temperature**
```
dT = Q_sensible / (m_air * Cp_air)
T_outlet = T_inlet - dT
```

**Step 5: Electric power consumption**

At part load, the compressor cycles on and off. A part-load fraction (PLF) curve accounts for cycling losses:

```
PLF = 1.0 - C_d * (1.0 - PLR)
```

where C_d = 0.15 is a typical cycling degradation coefficient. The runtime fraction is:

```
RTF = PLR / PLF
Power = Available_capacity * RTF / Available_COP
```

---

### Boiler

**Module:** `boiler.rs`

**Purpose:** Models a hot water boiler that heats water in response to a heating load from the plant loop.

#### Inputs

| Parameter | Unit | Default | Description |
|-----------|------|---------|-------------|
| `name` | — | required | Component name |
| `nominal_capacity` | W | required | Maximum heating capacity |
| `nominal_efficiency` | — | 0.80 | Efficiency at full load |
| `design_outlet_temp` | °C | 82.0 | Design hot water outlet temperature |
| `design_water_flow_rate` | m³/s | 0.0 | Design water flow rate |
| `min_plr` | — | 0.0 | Minimum part-load ratio |
| `max_plr` | — | 1.0 | Maximum part-load ratio |
| `opt_plr` | — | 1.0 | Optimum part-load ratio |
| `max_outlet_temp` | °C | 99.9 | Maximum allowed outlet temperature |
| `efficiency_curve` | — | Constant | PLR-dependent efficiency curve |
| `parasitic_electric_load` | W | 0.0 | Parasitic electric consumption |
| `sizing_factor` | — | 1.0 | Capacity multiplier for sizing |

**Efficiency curve types:**
- `Constant` — efficiency does not vary with load (curve output = 1.0)
- `PartLoadRatio([C₀, C₁, C₂, C₃])` — polynomial: `f(PLR) = C₀ + C₁·PLR + C₂·PLR² + C₃·PLR³`

#### Outputs

| Variable | Unit | Description |
|----------|------|-------------|
| `outlet_temp` | °C | Outlet water temperature |
| `mass_flow` | kg/s | Water mass flow rate |

Internal state: `fuel_used` [W], `boiler_load` [W], `operating_plr`, `parasitic_power` [W].

#### Calculations

**Reference:** EnergyPlus `Boilers.cc`

**Step 1: Determine boiler load**
```
Q_boiler = min(Load_requested, Q_capacity)
```

**Step 2: Part-load ratio**
```
PLR = clamp(Q_boiler / Q_capacity, PLR_min, PLR_max)
```

**Step 3: Efficiency correction**
```
curve_output = clamp(f_curve(PLR), 0.01, 1.1)
```

**Step 4: Outlet temperature**
```
T_outlet = T_inlet + Q_boiler / (ṁ_water · Cp_water)
```

If `T_outlet > T_max`, recalculate:
```
Q_actual = ṁ_water · Cp_water · (T_max − T_inlet)
T_outlet = T_max
```

**Step 5: Fuel consumption**
```
Fuel = Q_boiler / (η_nominal · curve_output)
Parasitic = Parasitic_design · PLR
```

---

### Chiller (Air-Cooled)

**Module:** `chiller.rs`

**Purpose:** Models an air-cooled electric chiller that produces chilled water for cooling coils on a plant loop. Performance degrades with increasing outdoor temperature and part-load operation.

#### Inputs

| Parameter | Unit | Default | Description |
|-----------|------|---------|-------------|
| `name` | — | required | Component name |
| `rated_capacity` | W | required | Rated cooling capacity at ARI conditions (29.4 C outdoor) |
| `rated_cop` | — | required | Rated COP at ARI conditions (typically 2.5-4.0) |
| `chw_setpoint` | C | required | Chilled water supply setpoint |
| `design_chw_flow` | m3/s | required | Design chilled water flow rate |
| `min_plr` | — | 0.1 | Minimum part-load ratio |

#### Outputs

| Variable | Unit | Description |
|----------|------|-------------|
| `outlet_temp` | C | Chilled water outlet temperature |
| `mass_flow` | kg/s | Water mass flow rate |

Internal state: `actual_capacity` [W], `actual_cop`, `electric_power` [W], `plr`.

#### Calculations

**Reference:** EnergyPlus Engineering Reference, "Chiller:Electric"

**Step 1: Capacity correction**

Capacity derates linearly as outdoor temperature increases above ARI rated conditions (T_rated = 29.4 C):

```
Cap_factor = clamp(1.0 - 0.015 * (T_outdoor - 29.4), 0.5, 1.1)
Available_capacity = Rated_capacity * Cap_factor
```

**Step 2: Part-load ratio**
```
PLR = clamp(Load / Available_capacity, PLR_min, 1.0)
Actual_capacity = PLR * Available_capacity
```

**Step 3: COP correction**

COP degrades with both outdoor temperature and part-load operation:

```
COP_factor = clamp(1.0 - 0.02 * (T_outdoor - 29.4), 0.4, 1.1)
PLR_factor = clamp(0.5 + 0.75 * PLR - 0.25 * PLR^2, 0.3, 1.0)
Actual_COP = max(Rated_COP * COP_factor * PLR_factor, 0.5)
```

The PLR efficiency curve `0.5 + 0.75*PLR - 0.25*PLR^2` models the characteristic efficiency degradation at low part-loads where compressor cycling losses dominate.

**Step 4: Electric power and outlet temperature**
```
Electric_power = Actual_capacity / Actual_COP
dT = Actual_capacity / (m_water * Cp_water)
T_outlet = max(T_inlet - dT, T_chw_setpoint - 2.0)
```

---

### Cooling Tower

**Module:** `cooling_tower.rs`

**Purpose:** Models a cooling tower for condenser water heat rejection. The tower cools warm condenser water by evaporating a portion of it into the outdoor air stream. Performance is fundamentally limited by the outdoor wet-bulb temperature.

**Reference:** EnergyPlus Engineering Reference, "Cooling Towers"

#### Types

- **SingleSpeed** -- fan is either fully on or fully off
- **TwoSpeed** -- fan operates at high speed, low speed (~50%), or off
- **VariableSpeed** -- fan speed modulates continuously to match load (VFD)

#### Inputs

| Parameter | Unit | Default | Description |
|-----------|------|---------|-------------|
| `name` | — | required | Component name |
| `tower_type` | — | required | SingleSpeed, TwoSpeed, or VariableSpeed |
| `design_water_flow` | m3/s | required | Design water flow rate |
| `design_air_flow` | m3/s | required | Design air flow rate |
| `design_fan_power` | W | required | Design fan power at full speed |
| `design_inlet_water_temp` | C | required | Design inlet water temperature (typically 35 C) |
| `design_approach` | C | required | Design approach temperature (T_water_out - T_wb, typically 3-5 C) |
| `design_range` | C | required | Design range (T_water_in - T_water_out, typically 5-6 C) |
| `min_approach` | C | 2.0 | Minimum approach temperature (physical limit) |

#### Outputs

| Variable | Unit | Description |
|----------|------|-------------|
| `outlet_temp` | C | Cooled condenser water outlet temperature |
| `mass_flow` | kg/s | Water mass flow rate |

Internal state: `fan_power` [W], `heat_rejected` [W].

#### Calculations

**Design capacity:**
```
Q_design = m_water_design * Cp_water * Range_design
```

where `m_water_design = design_water_flow * rho_water`.

**Minimum achievable outlet temperature:**
```
T_min_outlet = T_wb_outdoor + Approach_min
```

The outlet water temperature can never fall below the outdoor wet-bulb temperature plus the minimum approach. This is the fundamental thermodynamic limit of evaporative cooling.

**Actual heat rejection:**
```
Q_max = m_water * Cp_water * max(0, T_water_in - T_min_outlet)
Q_actual = min(Load, Q_max)
T_water_out = max(T_water_in - Q_actual / (m_water * Cp_water), T_min_outlet)
```

**Fan power:**

| Tower Type | Fan Power |
|------------|-----------|
| SingleSpeed | `P_design` (full on whenever load > 0) |
| TwoSpeed | `P_design` if PLR > 0.5, else `P_design * 0.125` (cubic law at 50% speed) |
| VariableSpeed | `P_design * PLR^3` (cubic fan law per affinity laws) |

where `PLR = Q_actual / Q_design`.

---

### Heat Recovery

**Module:** `heat_recovery.rs`

**Purpose:** Models air-to-air heat recovery devices (rotary enthalpy wheels and plate heat exchangers) that transfer energy between exhaust air and incoming outdoor air in DOAS systems.

**Reference:** ASHRAE Handbook of Fundamentals, Chapter 26

#### Types

- **Wheel** — rotary enthalpy wheel that recovers both sensible heat and moisture (latent)
- **Plate** — plate heat exchanger that recovers sensible heat only (no moisture transfer)

#### Inputs

| Parameter | Unit | Default | Description |
|-----------|------|---------|-------------|
| `name` | — | required | Component name |
| `source` | — | required | `wheel` or `plate` |
| `sensible_effectiveness` | 0-1 | required | Sensible effectiveness at 100% airflow (typical 0.70-0.85) |
| `latent_effectiveness` | 0-1 | required | Latent effectiveness (typical 0.60-0.75; always 0 for PlateHX) |
| `exhaust_air_temp` | C | 22.0 | Exhaust air dry-bulb temperature |
| `exhaust_air_w` | kg/kg | 0.008 | Exhaust air humidity ratio |
| `parasitic_power` | W | required | Parasitic electric power (wheel motor, etc.) |

#### Outputs

| Variable | Unit | Description |
|----------|------|-------------|
| `outlet_temp` | C | Supply air temperature after recovery |
| `outlet_w` | kg/kg | Supply air humidity ratio after recovery |
| `mass_flow` | kg/s | Air mass flow rate (unchanged) |

Internal state: `sensible_recovery` [W], `latent_recovery` [W], `electric_power` [W].

#### Calculations

**Bypass check:**

When the outdoor air temperature is within 1 C of the exhaust air temperature, the device bypasses -- no energy transfer occurs and parasitic power is zero. This prevents marginal or wrong-direction energy transfer.

**Sensible recovery (effectiveness-NTU approach):**
```
C_supply = m_air * Cp_air
Q_sensible = epsilon_s * C_supply * (T_exhaust - T_outdoor)
T_outlet = T_outdoor + Q_sensible / C_supply
```

Positive `Q_sensible` means heating the supply air (winter preheating); negative means cooling (summer precooling).

**Latent recovery (enthalpy wheel only):**
```
delta_W = epsilon_l * (W_exhaust - W_outdoor)
W_outlet = W_outdoor + delta_W
Q_latent = m_air * h_fg * delta_W
```

where `h_fg = 2.454 x 10^6 J/kg` (heat of vaporization at ~20 C). Plate heat exchangers have zero latent recovery.

**Total recovery:**
```
Q_total = Q_sensible + Q_latent
```

---

### Pump

**Module:** `pump.rs`

**Purpose:** Models constant-speed and variable-speed centrifugal pumps for plant (water) loops. Variable-speed pumps follow the affinity laws for power reduction at partial flow.

**Reference:** EnergyPlus Engineering Reference, "Pumps"

#### Types

- **ConstantSpeed** -- always runs at design power when on
- **VariableSpeed** -- power follows affinity laws with flow fraction

#### Inputs

| Parameter | Unit | Default | Description |
|-----------|------|---------|-------------|
| `name` | — | required | Component name |
| `pump_type` | — | required | ConstantSpeed or VariableSpeed |
| `design_flow_rate` | m3/s | required | Design maximum volumetric flow rate |
| `design_head` | Pa | required | Design pump head (typically 150,000-300,000 Pa) |
| `motor_efficiency` | 0-1 | required | Motor efficiency |
| `curve_exponent` | — | 3.0 | Affinity law exponent for variable-speed pump |
| `min_flow_fraction` | 0-1 | 0.1 | Minimum flow fraction (VFD low limit) |
| `motor_heat_to_fluid_fraction` | 0-1 | 1.0 | Fraction of motor heat going into the fluid |

#### Outputs

| Variable | Unit | Description |
|----------|------|-------------|
| `outlet_temp` | C | Water outlet temperature (slightly higher than inlet due to pump heat) |
| `mass_flow` | kg/s | Water mass flow rate |

Internal state: `power` [W], `heat_to_fluid` [W].

#### Calculations

**Design power:**
```
P_design = Q_design * H_design / eta_motor
```

**Operating power:**

*Constant speed:*
```
P = P_design    (whenever load > 0)
```

*Variable speed (affinity laws):*
```
PLR = clamp(m_actual / m_design, PLR_min, 1.0)
P = P_design * PLR^n
```

where `n` is the curve exponent (default 3.0, consistent with the cubic fan/pump affinity law).

**Heat addition to fluid:**
```
Q_heat = P * f_motor_heat_to_fluid
dT = Q_heat / (m_water * Cp_water)
T_outlet = T_inlet + dT
```

The pump adds a small temperature rise to the water from motor and friction losses that are dissipated into the fluid stream.

---

## Controls

**Crate:** `openbse-controls`

### Control Framework

The controls framework uses a **sense → compute → act** pattern, fully decoupled from components:

1. **Sense:** Controllers read the `SystemState` — a snapshot of zone temperatures, component outlet temperatures, loop temperatures, and loads.
2. **Compute:** Each controller determines what actions to take.
3. **Act:** Controllers produce `ControlAction` commands that the simulation loop converts to `ControlSignals` and applies to components.

#### SystemState (Sensor Side)

| Field | Type | Description |
|-------|------|-------------|
| `outdoor_air` | MoistAirState | Current outdoor conditions |
| `zone_temps` | HashMap | Zone name → air temperature [°C] |
| `zone_humidity` | HashMap | Zone name → humidity ratio [kg/kg] |
| `zone_heating_loads` | HashMap | Zone name → heating load [W] |
| `zone_cooling_loads` | HashMap | Zone name → cooling load [W] |
| `component_outlet_temps` | HashMap | Component name → outlet temp [°C] |
| `component_outlet_water_temps` | HashMap | Component name → water outlet [°C] |
| `plant_loop_temps` | HashMap | Loop name → supply temp [°C] |
| `plant_loop_loads` | HashMap | Loop name → total load [W] |

#### ControlAction (Actuator Side)

| Action | Fields | Description |
|--------|--------|-------------|
| `SetCoilSetpoint` | component, setpoint [°C] | Override a coil's outlet setpoint |
| `SetAirMassFlow` | component, mass_flow [kg/s] | Override a component's air flow |
| `SetPlantLoopSetpoint` | loop_name, setpoint [°C] | Set plant loop supply setpoint |
| `SetPlantLoad` | component, load [W] | Set plant component load demand |
| `SetZoneAirFlow` | zone, mass_flow [kg/s] | Set zone supply air flow |
| `SetZoneSupplyTemp` | zone, supply_temp [°C] | Set zone supply air temperature |

#### ControlSignals

The simulation loop aggregates all `ControlAction` commands into a single `ControlSignals` struct:

| Field | Type | Description |
|-------|------|-------------|
| `coil_setpoints` | HashMap | Component name → setpoint [°C] |
| `air_mass_flows` | HashMap | Component name → mass flow [kg/s] |
| `plant_loop_setpoints` | HashMap | Loop name → setpoint [°C] |
| `plant_loads` | HashMap | Component name → load [W] |
| `zone_supply_temps` | HashMap | Zone name → supply temp [°C] |
| `zone_air_flows` | HashMap | Zone name → mass flow [kg/s] |

---

### Zone Thermostat

**Module:** `thermostat.rs`

**Purpose:** Determines heating/cooling mode for each zone and modulates HVAC supply air temperature and flow rate to maintain setpoints.

#### Inputs

The thermostat defines temperature goals only. Supply temperatures and design flows come from the air loop's `controls` section.

| Parameter | Unit | Default | Description |
|-----------|------|---------|-------------|
| `zones` | — | required | List of zone group names |
| `heating_setpoint` | °C | 21.1 | Occupied heating setpoint |
| `cooling_setpoint` | °C | 23.9 | Occupied cooling setpoint |
| `unoccupied_heating_setpoint` | °C | 15.6 | Unoccupied heating setpoint |
| `unoccupied_cooling_setpoint` | °C | 29.4 | Unoccupied cooling setpoint |

Air loop controls provide:
| Parameter | Unit | Default | Source |
|-----------|------|---------|--------|
| `heating_supply_temp` | °C | 35.0 | Air loop controls |
| `cooling_supply_temp` | °C | 13.0 | Air loop controls |
| `design_zone_flow` | kg/s | 0.5 | Air loop controls |

#### Algorithm

**Mode determination:**
```
if T_zone < T_heating_setpoint:  mode = Heating
if T_zone > T_cooling_setpoint:  mode = Cooling
otherwise:                       mode = Deadband
```

**Supply temperature modulation (proportional):**

In heating mode:
```
error = T_heating_setpoint − T_zone
fraction = clamp(error / 5.0, 0.0, 1.0)
T_supply = T_zone + fraction · (T_heating_supply − T_zone)
```

In cooling mode:
```
error = T_zone − T_cooling_setpoint
fraction = clamp(error / 5.0, 0.0, 1.0)
T_supply = T_zone − fraction · (T_zone − T_cooling_supply)
```

The 5°C denominator means full supply temperature is reached when the zone is 5°C away from setpoint.

**Flow modulation:**
```
flow = design_zone_flow · fraction
```

Minimum flow of 10% of design is maintained during deadband for ventilation.

#### Outputs

Produces `SetZoneSupplyTemp` and `SetZoneAirFlow` actions for each zone.

---

### Setpoint Controller

**Module:** `setpoint.rs`

**Purpose:** Applies a fixed setpoint to a component (coil outlet temperature, boiler outlet temperature).

#### Inputs

| Parameter | Description |
|-----------|-------------|
| `name` | Controller name |
| `component` | Target component name |
| `setpoint` | Fixed setpoint value [°C] |

#### Algorithm

Every timestep, produces a `SetCoilSetpoint` action targeting the named component with the configured setpoint value. No sensing or modulation — purely a constant setpoint.

---

### Plant Loop Setpoint

**Module:** `setpoint.rs`

**Purpose:** Sets the supply temperature setpoint for a plant loop.

#### Inputs

| Parameter | Description |
|-----------|-------------|
| `name` | Controller name |
| `loop_name` | Target plant loop name |
| `supply_temp_setpoint` | Fixed supply temperature setpoint [°C] |

#### Algorithm

Every timestep, produces a `SetPlantLoopSetpoint` action targeting the named plant loop.

---

## Multi-Loop HVAC Controls

**Crate:** `openbse-cli` -- `main.rs` (signal builders)

The multi-loop control framework dispatches to the appropriate control strategy for each air loop based on its auto-detected system behavior. System type is inferred from the equipment and controls configuration (e.g., VAV fan → VAV behavior, `minimum_damper_position: 1.0` → DOAS behavior, FCU requires explicit `system_type: fcu`). Each system type has its own signal builder function that produces `ControlSignals` (coil setpoints, mass flows, OA fractions) for that loop's components. Multiple loops can serve the same zone (e.g., DOAS + FCU), and their supply air contributions are mixed using enthalpy-weighted averaging.

### Control Dispatcher Architecture

At each timestep, `simulate_all_loops` iterates over all air loops and:

1. Calls the system-type-specific signal builder for each loop.
2. Runs that loop's components in order with the generated signals.
3. Distributes supply air to served zones (flow allocation depends on system type).
4. Mixes supply air from multiple loops per zone using mass-flow-weighted temperature averaging:

```
T_mixed = Sum(T_i * m_i) / Sum(m_i)
m_total = Sum(m_i)
```

This allows DOAS ventilation air and FCU recirculated air to additively condition the same zone.

### HVAC Mode Determination

All system types share the same three-state mode logic per zone:

```
if T_zone < T_heating_setpoint:  mode = Heating
if T_zone > T_cooling_setpoint:  mode = Cooling
otherwise:                       mode = Deadband
```

---

### PSZ-AC (Packaged Single-Zone AC)

**Purpose:** Single-zone thermostat-controlled packaged unit with return-air mixing. Suitable for residential unitary systems and rooftop units.

#### Control Logic

The control zone is the first served zone. A single thermostat in that zone drives the heating/cooling mode for the entire loop.

**Airflow:**
- Heating/Cooling: 100% of total design flow (sum of all served zone design flows)
- Deadband: 30% of design flow (minimum circulation)

**Heating mode** -- fixed discharge air temperature of 35 C. The furnace/heating coil modulates its burner rate to achieve this DAT.

**Cooling mode** -- proportional discharge air temperature. The cooling DAT ramps from the cooling setpoint toward 12 C as the cooling error increases:

```
cooling_error = max(0, T_zone - T_cooling_setpoint)
cooling_DAT = clamp(T_cooling_setpoint - min(cooling_error, 10), 12, T_cooling_setpoint)
```

**Economizer (ASHRAE 90.1 section 6.5.1):**

Modulating differential dry-bulb economizer. In cooling mode, when outdoor air is cooler than return air, the outdoor air fraction is modulated to achieve the cooling DAT as a mixed-air target:

```
if mode == Cooling AND T_outdoor < T_return:
    OA_fraction = clamp((T_return - cooling_DAT) / (T_return - T_outdoor), OA_min, 1.0)
else:
    OA_fraction = OA_min
```

When the economizer can fully satisfy the cooling DAT through mixing alone, the DX coil stays off (free cooling). The default minimum outdoor air fraction is 0.20.

**Mixed air temperature:**
```
T_mixed = T_return * (1 - OA_frac) + T_outdoor * OA_frac
```

---

### DOAS (Dedicated Outdoor Air System)

**Purpose:** Pre-conditions 100% outdoor air to a fixed supply temperature. Does not modulate based on zone temperature -- always runs. Intended to be paired with FCU loops for zone-level temperature control.

#### Control Logic

**Airflow:** 30% of the total zone design flows (representing typical outdoor air fraction for ventilation).

**Supply temperature setpoints** (derived from zone setpoints to avoid delivering air that adds load):

```
T_supply_heat = max(zone_heating_setpoints) + 2 C   [warm neutral air in winter]
T_supply_cool = max(min(zone_cooling_setpoints) - 2 C, 14 C)   [cool dehumidified air in summer]
```

The 14 C minimum cooling setpoint ensures adequate dehumidification.

**Coil activation:**
- Heating coil fires only when `T_outdoor < T_supply_heat`
- Cooling coil fires only when `T_outdoor > T_supply_cool`

**Outdoor air fraction:** Always 1.0 (100% outdoor air by definition).

---

### FCU (Fan Coil Unit)

**Purpose:** Recirculating fan coil unit with per-zone thermostat control. Each FCU loop serves exactly one zone. Uses 100% recirculated zone air (no OA mixing); ventilation is handled by a separate DOAS loop.

#### Control Logic

**Fan speed modulation** -- proportional to heating/cooling error:

```
Deadband: flow = design_flow * 0.20 (fan at minimum)
Heating:  error = clamp(T_heat_sp - T_zone, 0, 5)
          frac  = 0.30 + 0.70 * (error / 5.0)    [30-100% of design]
          flow  = design_flow * frac
Cooling:  error = clamp(T_zone - T_cool_sp, 0, 5)
          frac  = 0.30 + 0.70 * (error / 5.0)    [30-100% of design]
          flow  = design_flow * frac
```

Full fan speed is reached when the zone is 5 C away from setpoint.

**Heating coil setpoint:**
```
error = T_heat_sp - T_zone
target = clamp(T_heat_sp + min(error, 14), T_heat_sp, 45 C)
```

**Cooling coil setpoint:**
```
error = T_zone - T_cool_sp
target = clamp(T_cool_sp - min(error, 10), 12 C, T_cool_sp)
```

**Outdoor air fraction:** Always 0.0 (100% recirculated zone air). The FCU inlet temperature is overridden with the current zone air temperature.

---

### VAV (Variable Air Volume)

**Purpose:** Central AHU with per-zone VAV boxes. Implements ASHRAE Guideline 36 dual-maximum control and supply air temperature reset.

#### Zone-Level VAV Box Control (ASHRAE G36 section 5.2)

The dual-maximum concept uses separate maximum airflow setpoints for heating and cooling modes, preventing the inefficiency of delivering high volumes of cold air during heating:

```
V_heat_max = 50% of design flow  (dual-maximum heating cap)
V_cool_max = 100% of design flow

Cooling: frac = V_min + (1.0 - V_min) * clamp(error / 5.0, 0, 1)
         zone_flow = design_flow * frac     [ramps V_min to V_cool_max]

Heating: frac = V_min + (V_heat_max - V_min) * clamp(error / 5.0, 0, 1)
         zone_flow = design_flow * frac     [ramps V_min to V_heat_max]

Deadband: zone_flow = design_flow * V_min   [minimum ventilation]
```

where `V_min` is the configurable minimum VAV fraction (default 0.30).

#### AHU-Level Supply Air Temperature Reset (ASHRAE G36 section 5.16)

The supply air temperature (SAT) resets between 13 C and 18 C based on the worst-case cooling demand across all zones:

```
max_cooling_demand = max across all zones of: clamp((T_zone - T_cool_sp) / 5.0, 0, 1)
SAT = 18 - (18 - 13) * max_cooling_demand
```

- `max_cooling_demand = 1.0` (peak cooling): SAT = 13 C
- `max_cooling_demand = 0.0` (no cooling): SAT = 18 C (saves energy in mild weather)

#### AHU Economizer

Same modulating differential dry-bulb logic as PSZ-AC, but targeting the SAT setpoint as the mixed-air temperature:

```
if any_cooling AND T_outdoor < T_return_avg:
    OA_fraction = clamp((T_return_avg - SAT) / (T_return_avg - T_outdoor), OA_min, 1.0)
else:
    OA_fraction = OA_min
```

#### AHU Preheat (Frost Protection)

When mixed air temperature falls below 4 C, the preheat coil activates to bring it to 4.5 C:

```
if T_mixed_air < 4.0:
    preheat_setpoint = 4.5
else:
    preheat_setpoint = off
```

#### AHU Warm Deck Heating

When a majority of zones need heating and no zones need cooling, the AHU heating coil heats supply air to the SAT reset maximum (18 C) to assist zone-level reheat:

```
if mostly_heating AND NOT any_cooling AND T_mixed < 18.0:
    heating_setpoint = 18.0
else:
    heating_setpoint = off
```

---

### OA Fraction and Humidity Blending

All system types propagate an OA fraction signal (`__oa_fraction__`) that determines how outdoor and return air humidity ratios are blended at the loop inlet:

```
W_mixed = OA_frac * W_outdoor + (1 - OA_frac) * W_indoor
```

| System Type | OA Fraction |
|-------------|-------------|
| PSZ-AC | Modulated by economizer (OA_min to 1.0) |
| DOAS | Always 1.0 (100% outdoor air) |
| FCU | Always 0.0 (100% recirculated zone air) |
| VAV | Modulated by economizer (OA_min to 1.0) |

---

## Design Day Sizing

**Crate:** `openbse-io` -- `sizing.rs`

**Purpose:** Automatic equipment sizing (autosizing) using design day simulations. Implements a two-stage ASHRAE-compliant approach that first sizes zone-level equipment, then sizes central system equipment.

**Reference:** ASHRAE Handbook -- Fundamentals, Chapter 18 (Nonresidential Cooling and Heating Load Calculations)

### Stage 1: Zone Sizing

For each design day (all provided, not just the first):

1. Generate 24 hours of synthetic weather for the design day conditions.
   - Heating design days use constant temperature, no solar radiation, and specified wind speed.
   - Cooling design days use a sinusoidal daily temperature profile with solar radiation.
2. Run the envelope simulation with warmup days to reach quasi-steady-state.
3. Record peak heating and cooling loads per zone per timestep.
4. Take the maximum across all design days of the same type.

**Results per zone:**
- Peak heating load [W] and the design day / hour where it occurred
- Peak cooling load [W] and the design day / hour where it occurred
- Zone design airflows [kg/s] (max of heating and cooling airflows)

These are used to size zone-level equipment: VAV boxes, fan coil units, etc.

### Stage 2: System Sizing

With zone equipment hard-sized from Stage 1:

1. Re-run each design day with zone airflows set to their design values.
2. At each timestep, sum all zone loads (coincident peak).
3. System capacity = maximum coincident sum across all hours and all design days.

**Results:**
- Coincident peak heating capacity [W] with sizing factor
- Coincident peak cooling capacity [W] with sizing factor
- System airflow [kg/s] and volume flow [m3/s]

These are used to size AHU coils, fans, and central plant equipment.

**Per-system-type sizing overrides:**

- **PSZ-AC / VAV:** Use system-wide coincident peak from Stage 2.
- **DOAS:** Coils are sized to pre-condition 100% OA from design outdoor conditions to fixed supply setpoints: `Q_heat = m_oa * Cp * (T_supply_heat - T_outdoor_heat_design)` and `Q_cool = m_oa * Cp * (T_outdoor_cool_design - T_supply_cool)`.
- **FCU:** Sized to its served zone(s) only -- uses zone peak loads from Stage 1.

---

## Building Envelope

**Crate:** `openbse-envelope`

The building envelope module solves the thermal physics of the building shell: heat conduction through opaque walls (via CTF), convection at interior and exterior surfaces, solar radiation processing, infiltration of outdoor air, and internal heat gains from occupants, lights, and equipment.

### Materials and Constructions

**Module:** `material.rs`

#### Material

An opaque material layer with thermal and surface properties.

| Field | Unit | Default | Description |
|-------|------|---------|-------------|
| `name` | — | required | Material name |
| `conductivity` | W/(m·K) | required | Thermal conductivity |
| `density` | kg/m³ | required | Density |
| `specific_heat` | J/(kg·K) | required | Specific heat capacity |
| `thickness` | m | 0.1 | Layer thickness |
| `solar_absorptance` | 0–1 | 0.7 | Solar radiation absorptance |
| `thermal_absorptance` | 0–1 | 0.9 | Longwave/thermal radiation absorptance |
| `visible_absorptance` | 0–1 | 0.7 | Visible light absorptance |
| `roughness` | enum | MediumRough | Surface roughness classification |

**Derived properties:**
- Thermal resistance: `R = thickness / conductivity` [m²·K/W]
- Thermal diffusivity: `α = conductivity / (density · specific_heat)` [m²/s]

#### Roughness

Affects exterior forced convection coefficient. Values and their forced-convection multipliers:

| Roughness | Multiplier |
|-----------|------------|
| VeryRough | 2.17 |
| Rough | 1.67 |
| MediumRough | 1.52 |
| MediumSmooth | 1.13 |
| Smooth | 1.11 |
| VerySmooth | 1.00 |

**Reference:** EnergyPlus `ConvectionCoefficients.cc`

#### Construction

A multi-layer opaque construction. Layers are ordered outside to inside.

| Field | Description |
|-------|-------------|
| `name` | Construction name |
| `layers` | Layer names, outside → inside |

**Derived properties:**
- Total resistance: `R_total = Σ R_layer` [m²·K/W]
- U-factor: `U = 1 / R_total` [W/(m²·K)] (no film coefficients included)

#### Window Construction

Windows use a simplified performance-based model.

| Field | Unit | Default | Description |
|-------|------|---------|-------------|
| `name` | — | required | Name |
| `u_factor` | W/(m²·K) | required | Overall U-factor (includes film coefficients) |
| `shgc` | 0–1 | required | Solar Heat Gain Coefficient at normal incidence |
| `visible_transmittance` | 0–1 | 0.6 | Visible light transmittance |

---

### Simple Constructions

**Module:** `material.rs`

**Purpose:** Provides a simplified opaque construction model defined by overall thermal properties rather than individual material layers. Ideal for early design, ASHRAE 140 test cases, and quick parametric studies where full layer-by-layer definition is unnecessary.

#### Inputs

| Parameter | Unit | Default | Description |
|-----------|------|---------|-------------|
| `name` | — | required | Construction name |
| `u_factor` | W/(m2-K) | required | Overall U-factor (conductance, no film coefficients) |
| `thickness` | m | 0.2 | Total wall thickness |
| `thermal_capacity` | J/(m2-K) | 50,000 | Thermal capacity per unit area (rho * cp * thickness sum) |
| `solar_absorptance` | 0-1 | 0.7 | Outside solar absorptance |
| `thermal_absorptance` | 0-1 | 0.9 | Thermal (longwave) absorptance |
| `roughness` | enum | MediumRough | Surface roughness classification |

The `thermal_capacity` default of 50,000 J/(m2-K) corresponds to a light construction. A heavyweight concrete wall might have 200,000-400,000 J/(m2-K).

The simple construction is converted internally to an equivalent single-layer material for CTF calculation, preserving the same steady-state and first-order transient behavior as the specified U-factor and thermal capacity.

---

### Conduction Transfer Functions (CTF)

**Module:** `ctf.rs`

**Purpose:** Computes heat conduction through multi-layer opaque wall constructions. CTFs encode the transient thermal response of a wall into a set of coefficients that relate surface temperatures and heat fluxes across timesteps.

**Reference:** Seem (1987), EnergyPlus Engineering Reference Chapter on CTF.

#### CTF Equations

The per-timestep conduction equations are:

```
q_inside  = Σⱼ Y[j]·T_out(t-j) − Σⱼ Z[j]·T_in(t-j) + Σⱼ Φ[j]·q_in(t-j)
q_outside = Σⱼ X[j]·T_out(t-j) − Σⱼ Y[j]·T_in(t-j) + Σⱼ Φ[j]·q_out(t-j)
```

where:
- `X` — outside surface CTF coefficients
- `Y` — cross CTF coefficients (linking outside and inside)
- `Z` — inside surface CTF coefficients
- `Φ` — flux history coefficients
- `j=0` is the current timestep, `j≥1` are past timesteps

Sign convention: `q_inside` positive = heat flowing into the zone.

#### Coefficients

| Symbol | Name | Description |
|--------|------|-------------|
| X[j] | Outside | Relate outside surface temperature to outside heat flux |
| Y[j] | Cross | Relate outside temperature to inside flux (and vice versa) |
| Z[j] | Inside | Relate inside surface temperature to inside heat flux |
| Φ[j] | Flux history | Weight previous flux values for transient response |

#### Implementation: Full State-Space Method (Seem 1987)

OpenBSE implements the full Seem (1987) state-space CTF method, matching EnergyPlus. This is implemented in `ctf.rs`.

**Key steps:**

1. **Layer discretization**: Each material layer is divided into nodes with spacing `dx = sqrt(2·α·dt)`, minimum 6 nodes per layer.

2. **State-space matrix construction**: The A (node coupling), B (boundary input), C (output), and D (feedthrough) matrices are assembled from layer thermal properties. NoMass layers are folded into boundary conductance terms.

3. **Matrix exponential**: `exp(A·dt)` computed via Taylor series with scaling and squaring for numerical stability.

4. **CTF coefficient extraction**: The R-matrix recurrence iteratively computes X, Y, Z, and Φ coefficient vectors (up to 18 terms) until convergence.

5. **Reciprocity enforcement**: Cross-term symmetry `average(|s(0,1)|, |s(1,0)|)` ensures energy conservation.

**Fallback modes:**
- **Low thermal mass** (`C_total < 1 kJ/m²·K`): Pure steady-state CTF (`X=Y=Z=U`, no flux history)
- **Degenerate cases**: First-order lumped RC as fallback if the state-space method produces NaN
- **Synthetic constructions**: Heuristic layer synthesis for `SimpleConstruction` inputs that lack explicit material layers

#### CTF History

Each surface maintains a history of past surface temperatures and heat fluxes. At each timestep, the history is shifted: current values are pushed in, the oldest values fall off.

---

### Interior Convection

**Module:** `convection.rs`

**Purpose:** Computes the convective heat transfer coefficient between interior surfaces and zone air.

**Model:** ASHRAE simple algorithm (Walton, 1983)

**Reference:** Walton (1983), EnergyPlus `ConvectionCoefficients.cc`

#### Function

```
h_conv = interior_convection(T_surface, T_zone, tilt_deg)
```

**Inputs:**
| Parameter | Unit | Description |
|-----------|------|-------------|
| `t_surface` | °C | Interior surface temperature |
| `t_zone` | °C | Zone air temperature |
| `tilt_deg` | degrees | Surface tilt from horizontal |

**Output:** Interior convection coefficient [W/(m²·K)]

#### Algorithm

**Near-vertical surfaces** (tilt 60°–120°, i.e. |cos(tilt)| < 0.5):
```
h = 1.31 · |ΔT|^(1/3)
```

**Near-horizontal surfaces** (|cos(tilt)| ≥ 0.5):

Stability is determined by the direction of buoyancy:
- **Stable** (warm surface facing down, or cool surface facing up): stratification resists convection
- **Unstable** (warm surface facing up, or cool surface facing down): buoyancy drives convection

```
Stable:    h = 1.810 · |ΔT|^(1/3) / (1.382 + |cos(tilt)|)
Unstable:  h = 9.482 · |ΔT|^(1/3) / (7.238 − |cos(tilt)|)
```

**Minimum:** 0.1 W/(m²·K) to prevent numerical issues with very small temperature differences.

---

### Exterior Convection

**Module:** `convection.rs`

**Purpose:** Computes the convective heat transfer coefficient between exterior surfaces and the outdoor environment.

**Model:** TARP (Thermal Analysis Research Program) — combined natural + forced convection.

**Reference:** Walton (1983), EnergyPlus Engineering Reference

#### Function

```
h_conv = exterior_convection(T_surface, T_outdoor, wind_speed, tilt_deg, roughness)
```

#### Algorithm

**Natural convection component** — uses the same Walton correlations as interior convection:
```
H_natural = interior_convection(T_surface, T_outdoor, tilt_deg)
```

**Forced convection component** — wind-driven, with a roughness multiplier:
```
H_forced = R_f · 2.537 · V_wind^0.5
```

where R_f is the roughness multiplier (see [Roughness](#roughness) table).

**Combined (TARP model):**
```
H = sqrt(H_natural² + H_forced²)
```

**Minimum:** 0.1 W/(m²·K)

---

### Solar Position and Incident Radiation

**Module:** `solar.rs`

**Purpose:** Computes solar position (altitude, azimuth), incident solar radiation on tilted surfaces using the Hay-Davies anisotropic sky model, and angular-dependent window transmittance using Fresnel optics.

#### Solar Position

**Function:** `solar_position(day_of_year, solar_hour, latitude_deg)`

**Reference:** Spencer (1971)

**Algorithm:**

Day angle:
```
Γ = 2π · (day_of_year − 1) / 365
```

Declination (Spencer):
```
δ = 0.006918
  − 0.399912·cos(Γ) + 0.070257·sin(Γ)
  − 0.006758·cos(2Γ) + 0.000907·sin(2Γ)
  − 0.002697·cos(3Γ) + 0.00148·sin(3Γ)
```

Hour angle:
```
ω = (solar_hour − 12) · 15°
```

Solar altitude:
```
sin(α) = sin(φ)·sin(δ) + cos(φ)·cos(δ)·cos(ω)
```

Solar azimuth:
```
sin(γ) = −cos(δ)·sin(ω) / cos(α)
```

where φ is site latitude.

#### Equation of Time

**Function:** `equation_of_time(day_of_year)` — returns correction in hours.

Converts clock time to solar time:
```
solar_hour = clock_hour + (longitude/15 − time_zone) + equation_of_time
```

#### Incident Solar on Tilted Surface

**Function:** `incident_solar_components(beam_normal, diffuse_horiz, global_horiz, solar_pos, azimuth, tilt, ground_reflectance, day_of_year, elevation_m)`

**Model:** Hay-Davies (1980) anisotropic sky model

**Reference:** Hay & Davies (1980) "Calculation of the Solar Irradiance Incident on an Inclined Surface"

**Inputs:**
| Parameter | Unit | Description |
|-----------|------|-------------|
| `beam_normal` | W/m² | Direct normal irradiance |
| `diffuse_horiz` | W/m² | Diffuse horizontal irradiance |
| `global_horiz` | W/m² | Global horizontal irradiance |
| `solar_pos` | -- | Solar position struct |
| `azimuth` | degrees | Surface azimuth (from north, clockwise) |
| `tilt` | degrees | Surface tilt (from horizontal) |
| `ground_reflectance` | 0-1 | Ground albedo (default 0.2) |
| `day_of_year` | -- | Day of year (1-365) |
| `elevation_m` | m | Site elevation (reserved for future Perez model) |

**Algorithm:**

Angle of incidence:
```
cos(θ) = cos(α_solar)·cos(γ_solar − γ_surface)·sin(β_surface) + sin(α_solar)·cos(β_surface)
```

Hay-Davies anisotropy index:
```
a_i = I_beam_normal / I_extraterrestrial
```

where `I_ext = 1353 · (1 + 0.033·cos(2π·DOY/365))` W/m².

Sky view factor:
```
VF_sky = (1 + cos(tilt)) / 2
```

The diffuse is decomposed into two components:

**Circumsolar** (directional, co-located with sun direction):
```
I_circumsolar = I_diffuse_horiz · a_i · cos(θ) / cos(θ_z)
```

**Isotropic** (uniform sky dome):
```
I_isotropic = I_diffuse_horiz · (1 - a_i) · VF_sky
```

**Ground-reflected:**
```
I_ground = I_global_horiz · ρ_ground · (1 − cos(tilt)) / 2
```

**Total:**
```
I_total = I_beam + I_circumsolar + I_isotropic + I_ground
```

The circumsolar component is critical for shading: it receives the same directional shading (sunlit fraction) as the beam component, since it represents forward-scattered light from the sun's direction.

#### Window Transmitted Solar (Angular SHGC)

**Function:** `window_transmitted_solar_angular(shgc, area, beam_incident, diffuse_incident, cos_aoi)`

Uses separate angular modifiers for beam and diffuse solar, based on Fresnel optics for double-pane glass.

**Beam modifier** (angular dependence):
```
beam_mod = fresnel_double_pane_modifier(cos_aoi, shgc)
```

Uses the Fresnel equations for unpolarized light at a glass-air interface (n = 1.526) to compute the transmittance ratio `T(θ)/T(0)` for a double-pane assembly, accounting for inter-pane reflections.

**Diffuse modifier** (hemispherical integration):
```
diff_mod = diffuse_shgc_modifier(shgc)
```

Computed as the hemispherical average of the beam modifier over 0-90 degrees, weighted by `cos(θ)·sin(θ)`. For SHGC = 0.789 (ASHRAE 140 clear double-pane), diff_mod = 0.864.

**Transmitted solar:**
```
Q_beam = SHGC · beam_mod · Area · I_beam
Q_diffuse = SHGC · diff_mod · Area · I_diffuse
Q_total = Q_beam + Q_diffuse
```

---

### Interior Solar Distribution

**Module:** `envelope.rs` (solar distribution logic within the zone heat balance)

**Purpose:** Distributes transmitted window solar gains onto interior zone surfaces. Two methods are implemented, selected via the `solar_distribution` YAML field.

#### FullExterior (Default)

All transmitted beam solar is distributed to floor surfaces, weighted by floor area:

```
Q_beam_floor_i = Q_beam_total × (A_floor_i / A_floor_total)
```

Diffuse transmitted solar is distributed to all interior surfaces using a VMULT factor:

```
VMULT = 1 / Σ(A_i × α_i)    for all interior surfaces
Q_diffuse_surface_i = Q_diffuse_total × A_i × α_i × VMULT
```

where `α_i` is the interior solar absorptance of surface `i`. This matches the EnergyPlus FullExterior algorithm.

#### FullInteriorAndExterior

Beam solar is geometrically projected through each window onto interior surfaces using the actual sun direction vector. The algorithm:

1. For each window, project a beam rectangle onto the plane of each interior surface using the solar direction vector.
2. Clip the projected rectangle against the receiving surface polygon using Sutherland-Hodgman polygon clipping.
3. Assign beam solar proportional to the clipped area on each surface.
4. Any beam solar that doesn't hit a surface (e.g., exits through another window) falls back to the FullExterior floor distribution.

Diffuse distribution uses the same VMULT method as FullExterior.

#### Reflected Beam Handling (Deliberate Deviation from EnergyPlus)

After beam solar is absorbed by its target surface (absorptance `α`), the reflected fraction `(1 - α)` enters the diffuse pool for redistribution via VMULT. This differs from EnergyPlus, which keeps reflected beam localized to the surface that received it.

**Rationale:** In reality, reflected beam bounces multiple times off interior surfaces. The infinite-bounce solution converges to a distribution governed by the radiosity equation `(I - ρF)⁻¹`, where `F` is the view factor matrix. Placing reflected beam into the diffuse pool (area-weighted) is a closer approximation to this converged state than E+'s single-bounce localization. The approximation omits view-factor weighting between surfaces, which is acceptable for typical rectangular rooms but could diverge for unusual geometries.

OpenBSE also omits E+'s direct solar-to-zone-air fraction, which E+ uses to account for solar absorbed by unmodeled interior objects (furniture, carpets) that release heat convectively with minimal time lag. This is a known simplification — see STATUS.md for details.

---

### External Shading

**Module:** `shading.rs`

**Purpose:** Computes the shading effect of overhangs, fins, and other external shading surfaces on building surfaces. Two types of shading are calculated: beam (directional) and diffuse (sky dome).

#### Beam Shadow Calculation

**Function:** `calculate_sunlit_fraction(surface_vertices, caster_polygons, sun_direction)`

**Algorithm:** For each timestep, the sun direction is used to project shading surface (caster) polygons onto the receiving surface plane. The projected shadow polygon is clipped against the receiving surface using the Sutherland-Hodgman polygon clipping algorithm. The sunlit fraction is `1 - (shadow_area / surface_area)`.

**Multi-caster handling:** When multiple casters shade the same surface (e.g., overhang + two fins), an 8x8 grid sampling method is used with UNION logic (a point is shaded if ANY caster shadows it). For single casters, exact polygon area is used for efficiency.

#### Diffuse Sky Shading Ratios

**Function:** `compute_diffuse_sky_shading_ratio(surface_vertices, caster_polygons)`

**Algorithm:** Samples the sky hemisphere using 144 patches (6 altitude x 24 azimuth directions). For each patch, checks if any caster blocks the line of sight from 5x5 sample points on the receiving surface. The ratio of unblocked patches (weighted by `cos(altitude)`) to the total gives the diffuse sky shading ratio.

Two ratios are computed:
- **DifShdgRatioIsoSky:** Fraction of isotropic sky dome visible (patches above 10 deg altitude)
- **DifShdgRatioHoriz:** Fraction of horizon band visible (patches below 10 deg altitude)

These ratios are precomputed once at startup and stored per surface.

#### Overhang and Fin Geometry Generation

Overhangs and fins can be specified as either:
1. **Explicit vertices** in the `shading_surfaces` section
2. **Auto-generated** from `shading: overhang/left_fin/right_fin` parameters on surfaces

Auto-generation creates overhang/fin polygons based on:
- `depth` -- projection distance from wall surface
- `offset_above` -- vertical offset above window (overhangs, default 0)
- `left_extension/right_extension` -- lateral extensions (overhangs, default 0)
- `extend_above/extend_below` -- vertical extensions (fins, default 0)

---

### Infiltration

**Module:** `infiltration.rs`

**Purpose:** Computes outdoor air infiltration into zones based on temperature difference and wind speed.

**Model:** EnergyPlus Design Flow Rate model

**Reference:** EnergyPlus `HeatBalanceAirManager.cc`

#### Inputs

| Parameter | Unit | Default | Description |
|-----------|------|---------|-------------|
| `design_flow_rate` | m³/s | 0.0 | Design infiltration volume flow |
| `air_changes_per_hour` | 1/hr | 0.0 | Alternative: ACH-based specification |
| `coeff_a` | — | 1.0 | Constant coefficient |
| `coeff_b` | 1/°C | 0.0 | Temperature difference coefficient |
| `coeff_c` | s/m | 0.0 | Wind speed coefficient |
| `coeff_d` | s²/m² | 0.0 | Wind speed squared coefficient |

If `air_changes_per_hour` is specified and `design_flow_rate` is zero, the flow rate is computed as:
```
Q_design = ACH · Volume / 3600
```

#### Algorithm

**Volume flow rate:**
```
Q = Q_design · (A + B·|T_zone − T_outdoor| + C·V_wind + D·V_wind²)
```

**Mass flow rate:**
```
ṁ = Q · ρ_outdoor
```

where `ρ_outdoor` is the outdoor air density at current conditions.

---

### Internal Gains

**Module:** `internal_gains.rs`

**Purpose:** Computes convective and radiative heat gains from occupants, lighting, and equipment.

#### Gain Types

**People:**
```
Q_total = count · activity_level        [W]
Q_radiative = Q_total · radiant_fraction
Q_convective = Q_total · (1 − radiant_fraction)
```

| Parameter | Unit | Default | Description |
|-----------|------|---------|-------------|
| `count` | — | required | Number of occupants |
| `activity_level` | W/person | 120.0 | Metabolic rate per person |
| `radiant_fraction` | 0–1 | 0.3 | Fraction of heat that is radiative |

**Lights:**
```
Q_input = power                                            [W]
Q_return_air = power · return_air_fraction
Q_zone = power · (1 − return_air_fraction)
Q_radiative = Q_zone · radiant_fraction
Q_convective = Q_zone · (1 − radiant_fraction)
```

| Parameter | Unit | Default | Description |
|-----------|------|---------|-------------|
| `power` | W | required | Total installed lighting power |
| `radiant_fraction` | 0–1 | 0.7 | Fraction of zone heat that is radiative |
| `return_air_fraction` | 0–1 | 0.0 | Fraction of heat to return air plenum |

**Equipment:**
```
Q_total = power                          [W]
Q_radiative = power · radiant_fraction
Q_convective = power · (1 − radiant_fraction)
```

| Parameter | Unit | Default | Description |
|-----------|------|---------|-------------|
| `power` | W | required | Total equipment power |
| `radiant_fraction` | 0–1 | 0.3 | Fraction of heat that is radiative |

The resolved gain output provides `convective` [W], `radiative` [W], and `total` [W].

Internal gains support time-varying operation through the schedule system (see [Schedules](#schedules)). Each gain type can reference a named schedule; the schedule fraction multiplies the design power at each timestep.

---

### Schedules

**Module:** `schedule.rs`

**Purpose:** Provides named time-varying fractional multipliers (0.0-1.0) for internal gains, exhaust fans, and outdoor air. Schedules define hourly values for weekdays, weekends, and holidays, enabling realistic occupancy and equipment operation profiles.

#### Inputs

| Parameter | Description |
|-----------|-------------|
| `name` | Schedule name (referenced from gains, exhaust fans, etc.) |
| `weekday` | 24-element array of hourly fractions for Monday-Friday. Index 0 = hour 1 (00:00-01:00). Default: all 1.0 (always on). |
| `weekend` | 24-element array for Saturday-Sunday. Defaults to `weekday` values if not specified. |
| `holiday` | 24-element array for holidays. Defaults to `weekend` values if not specified. |

**Built-in schedules:**
- `always_on` -- fraction = 1.0 at all times
- `always_off` -- fraction = 0.0 at all times

#### Algorithm

At each timestep, the schedule manager looks up the fraction for the current hour (1-indexed) and day of week (1=Monday through 7=Sunday):

```
fraction = schedule_values[hour - 1]    (clamped to [0.0, 1.0])
```

Weekday values are used for Monday-Friday (day_of_week 1-5), weekend values for Saturday-Sunday (day_of_week 6-7). Day of week is computed assuming January 1 is a Monday (simplified calendar model consistent across simulation years).

Unknown schedule names default to fraction 1.0 (fail-safe: always on).

---

### Exhaust Fans

**Module:** `zone.rs`

**Purpose:** Models zone-level exhaust fans (restroom exhaust, kitchen hoods, etc.) that remove air from the zone. The exhausted air is replaced by infiltration or transfer air from adjacent spaces, creating an additional air exchange load on the zone.

#### Inputs

| Parameter | Unit | Description |
|-----------|------|-------------|
| `flow_rate` | m3/s | Exhaust volume flow rate |
| `schedule` | — | Optional schedule name for time-varying operation (default: always on) |

The exhaust fan is specified as part of a zone definition. When the schedule is active, the fan removes air at the specified flow rate, which is converted to mass flow using outdoor air density and added to the zone's air exchange calculation.

---

### ASHRAE 62.1 Outdoor Air

**Module:** `zone.rs`

**Purpose:** Calculates minimum outdoor air ventilation rates per ASHRAE Standard 62.1 using a per-person plus per-area method.

#### Inputs

| Parameter | Unit | Description |
|-----------|------|-------------|
| `per_person` | m3/(s-person) | Outdoor air rate per person (e.g., 0.003539606 = 7.5 cfm/person) |
| `per_area` | m3/(s-m2) | Outdoor air rate per floor area (e.g., 0.000609599 = 0.12 cfm/ft2) |

#### Algorithm

Total outdoor air requirement:
```
Q_oa = (per_person * people_count) + (per_area * floor_area)
```

This value is used by the HVAC control system to set minimum outdoor air fractions for each air loop. The per-person component accounts for occupant bioeffluent dilution, while the per-area component accounts for building material off-gassing.

---

### Vertex Geometry

**Module:** `geometry.rs`

**Purpose:** Provides 3D polygon-based geometry for building surfaces, enabling automatic calculation of surface areas, orientations, and zone volumes from vertex coordinates.

**Reference:** Newell (1972), EnergyPlus `SurfaceGeometry.cc`

#### Coordinate System

Right-hand coordinate system: X = East, Y = North, Z = Up. Vertices are ordered counter-clockwise when viewed from outside the building (Newell convention).

#### Point3D

A vertex in building coordinates:

| Field | Unit | Description |
|-------|------|-------------|
| `x` | m | East coordinate |
| `y` | m | North coordinate |
| `z` | m | Up coordinate |

#### Newell's Method

Computes the outward normal vector and area of a polygon from its vertices:

```
For each vertex pair (i, j = i+1 mod n):
    Nx += (Yi - Yj) * (Zi + Zj)
    Ny += (Zi - Zj) * (Xi + Xj)
    Nz += (Xi - Xj) * (Yi + Yj)

Area = |N| / 2
```

The magnitude of the Newell normal vector equals twice the polygon area.

#### Derived Properties

**Azimuth** (from surface normal):
```
azimuth = atan2(Nx, Ny)    [mapped to 0-360 degrees]
```
0 = North, 90 = East, 180 = South, 270 = West.

**Tilt** (from surface normal):
```
tilt = acos(Nz / |N|)      [degrees]
```
0 = face-up (horizontal roof), 90 = vertical wall, 180 = face-down (floor).

**Zone volume** (divergence theorem):
```
V = (1/3) * |Sum over faces of (N_hat . P_centroid) * A_face|
```

Requires surfaces to form a closed volume with outward-pointing normals.

**Zone floor area:**

Sum of polygon areas for all surfaces with tilt > 150 degrees (downward-pointing normals, identifying floor surfaces).

#### Auto-Calculation

When a zone's volume or floor area is specified as 0.0 in the input, the engine automatically computes these values from the zone's surface vertices using the above algorithms. This avoids manual calculation for complex geometries.

---

### Ground Temperature Model

**Module:** `ground_temp.rs`

**Purpose:** Computes ground temperature as a function of depth and time of year using the Kusuda-Achenbach sinusoidal model. Used for floor and below-grade wall boundary conditions.

**Reference:** Kusuda & Achenbach (1965), ASHRAE Handbook of Fundamentals

#### Equation

```
T(z,t) = T_mean - A * exp(-z * sqrt(pi / (365 * alpha)))
         * cos(2*pi/365 * (t - t_shift - z/2 * sqrt(365 / (pi * alpha))))
```

| Symbol | Unit | Description |
|--------|------|-------------|
| `T_mean` | C | Annual mean ground surface temperature |
| `A` | C | Annual surface temperature amplitude (half of peak-to-peak) |
| `z` | m | Depth below surface |
| `t` | days | Day of year |
| `t_shift` | days | Day of minimum surface temperature (typically ~35 for northern hemisphere) |
| `alpha` | m2/day | Soil thermal diffusivity (default 0.04 m2/day) |

#### Inputs

| Parameter | Unit | Default | Description |
|-----------|------|---------|-------------|
| `t_mean` | C | 10.0 | Annual mean ground temperature |
| `amplitude` | C | 10.0 | Surface temperature amplitude |
| `phase_day` | days | 35.0 | Day of minimum surface temperature |
| `soil_diffusivity` | m2/day | 0.04 | Soil thermal diffusivity |
| `depth` | m | 0.0 | Depth below surface |

#### Auto-Calibration from Weather Data

When weather data is available, the model parameters are automatically derived:

1. Compute monthly average dry-bulb temperatures from hourly weather data.
2. `T_mean` = annual average of monthly means.
3. `Amplitude` = (max monthly average - min monthly average) / 2.
4. `Phase_day` = mid-month day of year for the coldest month.
5. `Soil_diffusivity` = 0.04 m2/day (typical soil, not derived from weather).

#### Physical Behavior

- At the surface (z = 0), temperature follows the full annual swing.
- With increasing depth, the exponential damping factor `exp(-z * sqrt(pi/(365*alpha)))` reduces the amplitude.
- At great depth (z >> 0), temperature converges to `T_mean` (the damping factor approaches zero).
- There is also a phase lag: deeper temperatures peak later in the year due to the `z/2 * sqrt(365/(pi*alpha))` term.

---

### Zone Air Heat Balance

**Module:** `zone.rs`

**Purpose:** Solves for the zone air temperature using an energy balance that accounts for surface convection, infiltration, HVAC supply, internal gains, and zone air thermal capacitance.

**Reference:** EnergyPlus predictor-corrector method (HeatBalanceAirManager)

#### Equation

```
         SumHAT + MCPI·T_outdoor + MCPSYS·T_supply + Q_conv + Cap·T_prev
T_zone = ─────────────────────────────────────────────────────────────────
                    SumHA + MCPI + MCPSYS + Cap
```

| Term | Definition | Unit |
|------|-----------|------|
| SumHA | Σ(h_conv,i · A_i) for all zone surfaces | W/K |
| SumHAT | Σ(h_conv,i · A_i · T_surface,i) for all zone surfaces | W |
| MCPI | ṁ_infiltration · Cp_air | W/K |
| MCPSYS | ṁ_HVAC_supply · Cp_air | W/K |
| Q_conv | Total convective gains (internal + solar transmitted + window conduction) | W |
| Cap | ρ_air · V_zone · Cp_air / dt | W/K |
| T_prev | Zone temperature from previous timestep | °C |

The `Cap · T_prev` term in the numerator and `Cap` in the denominator provide the thermal capacitance of the zone air mass — this is what gives the solution its transient behavior. Without it (dt → ∞), the equation reduces to a pure steady-state balance.

#### Zone Loads

Heating and cooling loads are the residual of the energy balance without the HVAC term:

```
Load = SumHAT − SumHA·T_zone + MCPI·(T_outdoor − T_zone) + Q_conv + Cap·(T_prev − T_zone)/dt
```

- If Load > 0: heating load (zone losing more heat than it gains)
- If Load < 0: cooling load (zone gaining more heat than it loses)

---

### Heat Balance Solver

**Module:** `heat_balance.rs`

**Purpose:** Orchestrates the full envelope heat balance: links solar, conduction (CTF), convection, infiltration, internal gains, and zone air balance into a coupled solution.

This is the central solver of the envelope module. It implements the `EnvelopeSolver` trait defined in `openbse-core`.

#### BuildingEnvelope

| Field | Description |
|-------|-------------|
| `zones` | Runtime zone states (temperature, humidity, loads, surface links) |
| `surfaces` | Runtime surface states (temps, fluxes, convection coefficients) |
| `ctf_coefficients` | CTF coefficients for each opaque surface |
| `ctf_histories` | Past surface temps and fluxes for each surface |
| `materials` | Material property lookup table |
| `constructions` | Construction layer lookup table |
| `window_constructions` | Window property lookup table |
| `latitude`, `longitude`, `time_zone` | Site location for solar calculations |
| `ground_reflectance` | Ground albedo (default 0.2) |

#### Initialization

When `initialize(dt)` is called:
1. Resolve material properties onto each surface (absorptances, roughness, U-factor, SHGC).
2. Identify windows vs. opaque surfaces.
3. Subtract window areas from their parent walls' net areas.
4. Assign surfaces to their zones.
5. Compute CTF coefficients for all opaque surfaces.
6. Initialize CTF histories at 21°C.

#### Per-Timestep Algorithm

```
solve_timestep(ctx, weather, hvac) → EnvelopeResults
```

**Step 1: Solar position**
- Compute day of year, equation of time, solar hour.
- Calculate solar altitude and azimuth.

**Step 2: Incident solar on each surface**
- For outdoor-facing surfaces, compute beam + diffuse + ground-reflected solar.
- For opaque surfaces: absorbed solar = solar_absorptance × incident.
- For windows: transmitted solar = SHGC × area × incident.

**Step 3: Internal gains**
- For each zone, resolve all internal gain types (people, lights, equipment) into convective and radiative components.

**Step 4: Infiltration**
- For each zone, compute infiltration mass flow from design flow rate, temperature difference, and wind speed.

**Step 5: Apply HVAC conditions**
- Read supply air temperatures and mass flows from the HVAC control signals.

**Step 6: Surface ↔ Zone coupling iteration (5 iterations)**

This inner loop iterates to converge the coupled surface temperatures and zone air temperatures:

**6a. Outside surface temperatures:**
- Outdoor surfaces: `T_surface_outside = T_outdoor + Q_solar_absorbed / h_conv_outside`
- Ground contact: `T_surface_outside = T_ground(depth, day_of_year)` via the [Kusuda-Achenbach model](#ground-temperature-model)
- Adiabatic: `T_surface_outside = T_surface_inside`
- Interzone: `T_surface_outside = T_adjacent_zone`

**6b. CTF conduction (opaque surfaces):**
Apply CTF equations to compute inside heat flux from outside/inside temperatures and history.

**6c. Inside surface temperature:**
```
T_surface_inside = T_zone + (q_ctf_in + q_radiative_flux) / h_conv_inside
```

where `q_radiative_flux` is the zone's radiative internal gains distributed proportionally to surface area.

**6d. Window heat transfer:**
```
Q_window_conduction = U_window · (T_outdoor − T_zone)    [W/m²]
Q_window_solar = SHGC · Area · I_incident                [W]
```

**6e. Zone air heat balance:**
Solve the predictor-corrector equation (see [Zone Air Heat Balance](#zone-air-heat-balance)) using:
- Surface convective fluxes (SumHA, SumHAT)
- Infiltration (MCPI)
- HVAC supply (MCPSYS)
- Convective gains (internal + solar transmitted through windows + window conduction)
- Zone air capacitance

**Step 7: Update histories**
- Shift CTF histories with current surface temperatures and fluxes.
- Update zone previous temperatures for next timestep.

**Step 8: Build results**
- Return zone temperatures, humidity ratios, heating/cooling loads, and detailed output variables for each zone.

#### Envelope Results

| Field | Type | Description |
|-------|------|-------------|
| `zone_temps` | HashMap | Zone → air temperature [°C] |
| `zone_humidity` | HashMap | Zone → humidity ratio [kg/kg] |
| `zone_heating_loads` | HashMap | Zone → heating load [W] (positive = needs heating) |
| `zone_cooling_loads` | HashMap | Zone → cooling load [W] (positive = needs cooling) |
| `zone_outputs` | HashMap | Zone → detailed output variables map |

---

## Performance Curves

**Crate:** `openbse-components` — `performance_curve.rs`

**Purpose:** Reusable polynomial curves that modify rated equipment performance as a function of operating conditions (temperatures, flow ratios, etc.). Curves are defined at the top level of the model and referenced by name from equipment.

### Curve Types

| Type | Form | Coefficients |
|------|------|-------------|
| Biquadratic | f(x,y) = c1 + c2·x + c3·x² + c4·y + c5·y² + c6·x·y | 6 |
| Quadratic | f(x) = c1 + c2·x + c3·x² | 3 |
| Cubic | f(x) = c1 + c2·x + c3·x² + c4·x³ | 4 |
| Linear | f(x) = c1 + c2·x | 2 |

### Input/Output Clamping

Input values are clamped to `[min_x, max_x]` and `[min_y, max_y]` before evaluation. Output is clamped to `[min_output, max_output]` if those limits are set. This prevents extrapolation beyond the curve's valid range.

### Usage

**DX Cooling Coils:** Two biquadratic curves as functions of entering wet-bulb temperature (x) and outdoor dry-bulb temperature (y):
- `cap_ft_curve` — capacity modifier: `available_capacity = rated_capacity × cap_ft(T_wb, T_odb)`
- `eir_ft_curve` — EIR modifier: `available_COP = rated_COP / eir_ft(T_wb, T_odb)`

When curves are absent, the engine falls back to simplified linear derating with outdoor temperature.

---

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
