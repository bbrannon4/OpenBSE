# Plant {#sec-plant}

::: {.callout-warning}
## Pre-review content

This chapter holds text moved unchanged, apart from format, from the former single-file reference (`README.md`, mostly written in March 2026). Parts of it are stale or incomplete. It will be rewritten from the code, and checked against EnergyPlus, in this chapter's review ([#181](https://github.com/bbrannon4/OpenBSE/issues/181)); see @sec-status.
:::

## Boiler

**Module:** `boiler.rs`

**Purpose:** Models a hot water boiler that heats water in response to a heating load from the plant loop.

### Inputs

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

### Outputs

| Variable | Unit | Description |
|----------|------|-------------|
| `outlet_temp` | °C | Outlet water temperature |
| `mass_flow` | kg/s | Water mass flow rate |

Internal state: `fuel_used` [W], `boiler_load` [W], `operating_plr`, `parasitic_power` [W].

### Calculations

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

## Chiller (Air-Cooled)

**Module:** `chiller.rs`

**Purpose:** Models an air-cooled electric chiller that produces chilled water for cooling coils on a plant loop. Performance degrades with increasing condenser temperature. Part-load efficiency follows the EIR-f(PLR) curve; without one, COP stays constant across part load.

### Inputs

| Parameter | Unit | Default | Description |
|-----------|------|---------|-------------|
| `name` | — | required | Component name |
| `rated_capacity` | W | required | Rated cooling capacity at ARI conditions (29.4 C outdoor) |
| `rated_cop` | — | required | Rated COP at ARI conditions (typically 2.5-4.0) |
| `chw_setpoint` | C | required | Chilled water supply setpoint |
| `design_chw_flow` | m3/s | required | Design chilled water flow rate |
| `min_plr` | — | 0.25 | Minimum part-load ratio; below it the chiller cycles |

### Outputs

| Variable | Unit | Description |
|----------|------|-------------|
| `outlet_temp` | C | Chilled water outlet temperature |
| `mass_flow` | kg/s | Water mass flow rate |

Internal state: `actual_capacity` [W], `actual_cop`, `electric_power` [W], `plr`.

### Calculations

**Reference:** EnergyPlus Engineering Reference, "Chiller:Electric"

**Step 1: Capacity correction**

Capacity derates linearly as outdoor temperature increases above ARI rated conditions (T_rated = 29.4 C):

```
Cap_factor = clamp(1.0 - 0.015 * (T_outdoor - 29.4), 0.5, 1.1)
Available_capacity = Rated_capacity * Cap_factor
```

**Step 2: Part-load ratio, cycling and false loading**

Below `min_plr` the chiller cycles on and off (E+ `ChillerCyclingRatio`). Below `min_unloading_ratio`, which defaults to `min_plr` and is at least `min_plr`, the compressor runs at that ratio and false-loads (hot-gas bypass, E+ `ChillerFalseLoadRate`):

```
PLR = min(Load / Available_capacity, 1.0)
PLR_op = max(PLR, min_unloading_ratio)
Cycling_ratio = min(PLR / min_plr, 1.0)
Actual_capacity = PLR * Available_capacity
False_load = PLR_op * Available_capacity * Cycling_ratio - Actual_capacity
```

The false load adds to the condenser heat.

**Step 3: Electric power (E+ Chiller:Electric:EIR)**

```
Electric_power = Available_capacity * (1 / Rated_COP) * EIRFT * EIRFPLR(PLR_op) * Cycling_ratio
```

With an `eirft` curve, EIRFT is evaluated at (CHW setpoint, condenser temperature). Without one, it falls back to `1 / clamp(1.0 - 0.02 * (T_condenser - 29.4), 0.4, 1.1)`.

With an `eirfplr` curve, EIRFPLR is evaluated at `PLR_op`. E+ requires this curve and has no default. Without one, OpenBSE uses `EIRFPLR = PLR_op`. Because EIRFPLR scales the *available* capacity, this keeps the COP at `Rated_COP / EIRFT` at every part load, with no part-load advantage or penalty. Below `min_plr` the cycling ratio keeps the time-averaged COP the same. 99 of the 111 air-cooled chillers in the E+ `AirCooledChiller.idf` dataset use exactly this curve.

**Step 4: Outlet temperature**
```
dT = Actual_capacity / (m_water * Cp_water)
T_outlet = max(T_inlet - dT, T_chw_setpoint - 2.0)
```

### Reformulated EIR (`model: reformulated_eir`)

**Reference:** EnergyPlus Engineering Reference, "Chiller:Electric:ReformulatedEIR" (Hydeman et al. 2002), with the `LeavingCondenserWaterTemperature` part-load curve type. Water-cooled only.

Steps 1–3 are the same, with two differences:
- CAPFT and EIRFT are evaluated at (CHW setpoint, **leaving** condenser water temperature).
- EIRFPLR is a two-variable curve, evaluated at (leaving condenser water temperature, `PLR_op`). E+ typically uses a bicubic.

All three curves are required, because the EIR model's fallbacks refer to the entering condenser temperature.

The leaving condenser temperature depends on the chiller's own power, so it is solved each timestep. All compressor heat goes to the condenser (the E+ default):

```
T_cond_out = T_cond_in + (Electric_power(T_cond_out) + Actual_capacity(T_cond_out)) / (m_cond * Cp_water)
```

- `T_cond_in` is the condenser entering temperature from the EIR model: `condenser_entering_temp`, held no lower than wet-bulb + `tower_approach`.
- `m_cond` is constant while the chiller runs (E+ `ConstantFlow`). Unless `design_condenser_flow` is given, it is sized as in E+: `Rated_capacity * (1 + 1/Rated_COP) / (Cp_water * dT_cond)`, where `dT_cond` is the condenser plant loop's `design_delta_t` (5.56 K without one).
- E+ solves this with regula falsi. OpenBSE uses fixed-point iteration, which converges in a few steps: power changes by a few kW per kelvin, against a condenser `m_cond * Cp_water` of order 100 kW/K.
- Inputs outside a curve's limits are held at the limits, as in E+.

The model reproduces EnergyPlus 25.2 timestep outputs of the DOE prototype hospital's chiller to within 0.01% in power and 0.001 K in leaving condenser temperature, over PLR 0.02 to 0.97 (unit test `test_reformulated_eir_matches_eplus_timesteps`). Replaying that chiller's annual E+ load profile through these equations gives E+'s annual electricity to within 0.1%.

One difference from E+: CAPFT and EIRFT both use the CHW setpoint here. E+ uses the setpoint for CAPFT and the actual leaving CHW temperature for EIRFT.

### Condenser heat recovery (`heat_recovery`)

**Reference:** EnergyPlus `Chiller:Electric:EIR` heat recovery with a leaving temperature setpoint (`calcHeatRecovery`). Model `eir`, water-cooled only.

Part of the condenser heat goes to a heat-recovery water stream on a heating plant loop instead of the tower:

```
Q_rejected   = Electric_power + Actual_capacity + False_load
Q_recoverable = min(m_hr * Cp_water * max(T_hr_setpoint - T_hr_in, 0),
                    capacity_fraction * Rated_capacity * (1 + 1/Rated_COP),
                    remaining demand of the receiving loop)
Q_recovered  = min(Q_rejected, Q_recoverable)      (0 while T_hr_in > inlet_high_limit)
Q_condenser  = Q_rejected - Q_recovered
T_curve      = (Q_recovered * T_hr_in + Q_condenser * T_cond_in) / Q_rejected
```

- CAPFT and EIRFT are evaluated at `T_curve`, so recovering heat into warm water costs compressor power.
- `T_curve` and the split depend on each other through the power, so they are solved together by fixed-point iteration. E+ blends with the split from its previous iteration, which converges to the same point.
- `T_hr_in` is the receiving loop's return temperature: its supply setpoint (with any reset) minus its `design_delta_t`, as the loop solver assumes.
- `m_hr` defaults to the design condenser flow × `capacity_fraction`, as E+ autosizes it.

In the plant solver:
- The chiller's loop runs before the receiving loop.
- The recovered heat covers the receiving loop's load before its boilers; its pumps still carry the full loop flow.
- A condenser loop receives only `Q_condenser`.

Capping recovery at the receiving loop's demand stands in for E+'s plant: there, heat the loop cannot absorb warms the heat-recovery water toward its setpoint until recovery stops.

Unit tests reproduce EnergyPlus 22.1 timesteps of the DOE prototype hospital's heat-recovery chiller (`test_heat_recovery_matches_eplus_timesteps`):
- power and recovered heat within 0.001% when all heat is recovered and when the chiller false-loads below its 0.2 min unloading ratio;
- within 0.16% when recovery is capped by the setpoint, where E+'s one-iteration lag in the blend shows.

---

## Cooling Tower

**Module:** `cooling_tower.rs`

**Purpose:** Models a cooling tower for condenser water heat rejection. The tower cools warm condenser water by evaporating a portion of it into the outdoor air stream. Performance is fundamentally limited by the outdoor wet-bulb temperature.

**Reference:** EnergyPlus Engineering Reference, "Cooling Towers"

### Types

- **SingleSpeed** -- fan is either fully on or fully off
- **TwoSpeed** -- fan operates at high speed, low speed (~50%), or off
- **VariableSpeed** -- fan speed modulates continuously to match load (VFD)

### Inputs

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

### Outputs

| Variable | Unit | Description |
|----------|------|-------------|
| `outlet_temp` | C | Cooled condenser water outlet temperature |
| `mass_flow` | kg/s | Water mass flow rate |

Internal state: `fan_power` [W], `heat_rejected` [W].

### Calculations

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

## Pump

**Module:** `pump.rs`

**Purpose:** Models constant-speed and variable-speed centrifugal pumps for plant (water) loops. Variable-speed pumps follow the affinity laws for power reduction at partial flow.

**Reference:** EnergyPlus Engineering Reference, "Pumps"

### Types

- **ConstantSpeed** -- always runs at design power when on
- **VariableSpeed** -- power follows affinity laws with flow fraction

### Inputs

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

### Outputs

| Variable | Unit | Description |
|----------|------|-------------|
| `outlet_temp` | C | Water outlet temperature (slightly higher than inlet due to pump heat) |
| `mass_flow` | kg/s | Water mass flow rate |

Internal state: `power` [W], `heat_to_fluid` [W].

### Calculations

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
