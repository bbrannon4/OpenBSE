# Air-side HVAC components {#sec-air-side}

::: {.callout-warning}
## Pre-review content

This chapter holds text moved unchanged, apart from format, from the former single-file reference (`README.md`, mostly written in March 2026). Parts of it are stale or incomplete. It will be rewritten from the code, and checked against EnergyPlus, in this chapter's review ([#181](https://github.com/bbrannon4/OpenBSE/issues/181)); see @sec-status.
:::

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
