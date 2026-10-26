# Controls and systems {#sec-controls}

::: {.callout-warning}
## Pre-review content

This chapter holds text moved unchanged, apart from format, from the former single-file reference (`README.md`, mostly written in March 2026). Parts of it are stale or incomplete. It will be rewritten from the code, and checked against EnergyPlus, in this chapter's review ([#181](https://github.com/bbrannon4/OpenBSE/issues/181)); see @sec-status.
:::

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

## Schedules

**Module:** `schedule.rs`

**Purpose:** Provides named time-varying fractional multipliers (0.0-1.0) for internal gains, exhaust fans, and outdoor air. Schedules define hourly values for weekdays, weekends, and holidays, enabling realistic occupancy and equipment operation profiles.

### Inputs

| Parameter | Description |
|-----------|-------------|
| `name` | Schedule name (referenced from gains, exhaust fans, etc.) |
| `weekday` | 24-element array of hourly fractions for Monday-Friday. Index 0 = hour 1 (00:00-01:00). Default: all 1.0 (always on). |
| `weekend` | 24-element array for Saturday-Sunday. Defaults to `weekday` values if not specified. |
| `holiday` | 24-element array for holidays. Defaults to `weekend` values if not specified. |

**Built-in schedules:**
- `always_on` -- fraction = 1.0 at all times
- `always_off` -- fraction = 0.0 at all times

### Algorithm

At each timestep, the schedule manager looks up the fraction for the current hour (1-indexed) and day of week (1=Monday through 7=Sunday):

```
fraction = schedule_values[hour - 1]    (clamped to [0.0, 1.0])
```

Weekday values are used for Monday-Friday (day_of_week 1-5), weekend values for Saturday-Sunday (day_of_week 6-7). Day of week is computed assuming January 1 is a Monday (simplified calendar model consistent across simulation years).

Unknown schedule names default to fraction 1.0 (fail-safe: always on).

## ASHRAE 62.1 Outdoor Air

**Module:** `zone.rs`

**Purpose:** Calculates minimum outdoor air ventilation rates per ASHRAE Standard 62.1 using a per-person plus per-area method.

### Inputs

| Parameter | Unit | Description |
|-----------|------|-------------|
| `per_person` | m3/(s-person) | Outdoor air rate per person (e.g., 0.003539606 = 7.5 cfm/person) |
| `per_area` | m3/(s-m2) | Outdoor air rate per floor area (e.g., 0.000609599 = 0.12 cfm/ft2) |

### Algorithm

Total outdoor air requirement:
```
Q_oa = (per_person * people_count) + (per_area * floor_area)
```

This value is used by the HVAC control system to set minimum outdoor air fractions for each air loop. The per-person component accounts for occupant bioeffluent dilution, while the per-area component accounts for building material off-gassing.
