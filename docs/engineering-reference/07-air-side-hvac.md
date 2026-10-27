# Air-side HVAC components {#sec-air-side}

Air-side components sit in an air loop's equipment list or a zone unit, and
are simulated in order along the air path (@sec-graph). Each takes its
inlet air state and mass flow and returns its outlet. Controls
(@sec-controls) set their outlet setpoints and flows before each call.
Enthalpy, $c_p$ and the conversion between temperature and enthalpy follow @eq-enthalpy, with $c_p = c_p(W)$ at the inlet humidity ratio unless stated
otherwise.

## Fans {#sec-fans}

**Source:** `openbse-components/src/fan.rs` (`Fan::calculate`,
`simulate_air`); construction in `openbse-io/src/input.rs`.

**Inputs.** A fan has:

- a design flow $\dot V_d$ [m³/s];
- a pressure rise $\Delta p$ [Pa] (default 600);
- a motor efficiency $\eta_m$ (default 0.9) and an impeller efficiency
  $\eta_i$ (default 0.71), which give the total efficiency
  $\eta_t = \eta_m\eta_i$;
- the motor-in-airstream fraction $f_\text{mot}$ (default 1);
- an optional fixed density $\rho_\text{fix}$.

The power-equation density is $\rho = \rho_\text{fix}$ if given, and
otherwise the inlet air density (@eq-density).

**Power.** For a constant-volume fan,

$$
P = \frac{\dot m\,\Delta p}{\eta_t\,\rho}.
$$ {#eq-fan-cv}

For a VAV fan, with the flow fraction
$x = \min(\dot m/(\rho\dot V_d), 1)$,

$$
P = \left(c_1 + c_2x + c_3x^2 + c_4x^3 + c_5x^4\right)\frac{\rho\dot V_d\,\Delta p}{\eta_t\,\rho}.
$$ {#eq-fan-vav}

The default coefficients are $(0.0407599,\ 0.0880450,\ -0.0729636,\
0.9437398,\ 0)$. An `on_off` fan uses @eq-fan-cv. It cycles through the flow
the loop controls give it (@sec-controls).

**Heat to the air.** The shaft power is $\eta_mP$. All of it, plus the
fraction $f_\text{mot}$ of the motor loss, heats the air:

$$
\dot Q_\text{air} = \eta_mP + (1 - \eta_m)P\,f_\text{mot},
\qquad
h_\text{out} = h_\text{in} + \frac{\dot Q_\text{air}}{\dot m}, \qquad W_\text{out} = W_\text{in}.
$$ {#eq-fan-heat}

**Comparison with EnergyPlus.** @eq-fan-cv–@eq-fan-heat match
`Fan:ConstantVolume` and `Fan:VariableVolume` (`Fans.cc`, `simulateConstant`,
`simulateVAV`) [@eplus-engref-25-2, "Fans"], with these differences
([#71](https://github.com/bbrannon4/OpenBSE/issues/71)):

- **Density.** EnergyPlus always uses the standard density; OpenBSE uses
  the inlet density unless `air_density` is set.
- **Minimum flow fraction.** EnergyPlus evaluates the VAV curve at no less
  than its minimum flow fraction (default 0.25). OpenBSE evaluates it down
  to zero flow.
- **Coefficients.** The EnergyPlus examples use $c_3 = -0.07292612$.
- **On/off fans.** `Fan:OnOff` has power-ratio curves and takes the coil's
  runtime fraction. OpenBSE's on/off fan is the constant-volume fan.

## ASHRAE 205 fans {#sec-fan-a205}

**Source:** `openbse-components/src/fan_a205.rs` (`FanA205`);
`openbse-a205/src/rs0003.rs` (`FanInterpolator`, `FanEfficiencyChain`).

A fan with an `a205_file` uses a Standard 205 RS0003 performance map
[@ashrae205]. Each timestep, the map is interpolated at the standard-air
volume flow $\dot V = \dot m/1.204$ and the design static pressure, giving
the shaft power $P_s$ and the impeller speed $n$. The grid power follows
the drive chain:

- a belt drive (RS0007) with efficiency $\eta_d(P_s)$;
- a motor (RS0005) with efficiency $\eta_m(P, n)$, or the fan's
  `motor_efficiency` if the file has no motor;
- a VFD (RS0006) with efficiency $\eta_v(P, f)$ at the frequency
  $f = n_\text{motor}\,p/2$, for $p$ poles.

Each stage divides by its efficiency:

$$
P = \frac{P_s}{\eta_d\,\eta_m\,\eta_v}, \qquad
\dot Q_\text{air} = P_s + (1 - f_\text{loss})(P - P_s),
$$ {#eq-fan-a205}

with $f_\text{loss}$ the file's `heat_loss_fraction`, the share of the
losses that leaves the airstream. Standalone motor, drive and VFD files
replace the ones embedded in the fan file.

**Comparison with EnergyPlus.** EnergyPlus has no Standard 205 fan. Its
nearest model is `Fan:SystemModel`, which follows a duct static-pressure
curve. OpenBSE always queries the map at the design pressure, which
amounts to constant duct-static-pressure control
([#72](https://github.com/bbrannon4/OpenBSE/issues/72)). After a failed map
query, the previous timestep's power is reported again
([#72](https://github.com/bbrannon4/OpenBSE/issues/72)).

## Heating coils {#sec-heating-coils}

**Source:** `openbse-components/src/heating_coil.rs` (`HeatingCoil`);
construction in `openbse-io/src/input.rs`. Heat-pump heating coils are in @sec-air-side-remaining.

All three types heat the air toward the outlet setpoint $T_\text{sp}$ and
never cool it. The load is

$$
\dot Q_\text{req} = \max\left(0,\ \dot m c_p(T_\text{sp} - T_\text{in})\right), \qquad
\dot Q = \min(\dot Q_\text{req},\ \dot Q_\text{cap}), \qquad
T_\text{out} = T_\text{in} + \frac{\dot Q}{\dot m c_p},
$$ {#eq-heating-coil}

with $W$ unchanged. The coil types differ as follows:

- **Electric.** The coil draws $\dot Q$ of electricity. The `efficiency`
  input is not read: it is fixed at 1
  ([#234](https://github.com/bbrannon4/OpenBSE/issues/234)).
- **Gas.** The coil burns $\dot Q/\eta_b$ of fuel, with $\eta_b$ the burner
  efficiency. An on-cycle parasitic electric load $P_\text{par}$ is drawn in
  proportion to firing, $P_\text{par}\dot Q/\dot Q_\text{cap}$. Its heat is
  lost with the flue gas.
- **Hot water.** The coil uses no energy itself. Its output is a load on its
  plant loop (@sec-plant).

**Hot-water capacity.** The hot-water coil has two optional limits, but in
the current code neither one ever applies
([#234](https://github.com/bbrannon4/OpenBSE/issues/234)):

- A **water-side limit**, $\dot m_wc_{p,w}(T_{w,\text{in}} - 71)$. The coil
  is never given its water inlet, so this limit is replaced by the nominal
  capacity. As a result, hot-water coil output does not depend on the
  loop's supply temperature or flow.
- A **UA model** (`ua_model: true`), meant to apply a cross-flow
  effectiveness. With no water inlet, the coil falls back to the plain
  model above. The UA model also has two errors: its effectiveness has the
  exponents of the both-unmixed correlation swapped, and its UA is taken
  from the counterflow LMTD at the rated point. Together they would deliver
  86.5% of rated capacity at the rated point.

**Comparison with EnergyPlus.** The electric and gas coils match
`Coil:Heating:Electric` and `Coil:Heating:Fuel` with no part-load fuel curve
(`HeatingCoils.cc`). OpenBSE has no part-load curve and no off-cycle
parasitic fuel ([#71](https://github.com/bbrannon4/OpenBSE/issues/71)).
`Coil:Heating:Water` (`WaterCoils.cc`, `CalcSimpleHeatingCoil`) instead
computes the output from the cross-flow both-unmixed effectiveness

$$
\varepsilon = 1 - \exp\left[\frac{\text{NTU}^{0.22}}{C_r}\left(e^{-C_r\text{NTU}^{0.78}} - 1\right)\right],
$$ {#eq-crossflow-unmixed}

the water inlet temperature and both flows. Its UA is solved so that the
model meets the rated capacity [@eplus-engref-25-2, "Heating Coils"]. In
EnergyPlus, the loop temperature and flow therefore limit the coil
([#234](https://github.com/bbrannon4/OpenBSE/issues/234)).

## Ducts {#sec-ducts}

**Source:** `openbse-components/src/duct.rs` (`Duct`); `main.rs`
(`simulate_loop_components`, and "Duct distribution losses → ambient
zone").

A duct of length $L$, diameter $D$ and wall U-value $U$ exchanges heat with
its ambient temperature $T_a$. Its effectiveness is that of a stream
against a constant-temperature reservoir:

$$
\varepsilon = 1 - \exp\left(-\frac{U\pi DL}{\dot m c_p}\right), \qquad
T_\text{out} = T_\text{in} - \varepsilon(T_\text{in} - T_a), \qquad
\dot Q_\text{cond} = \dot m c_p(T_\text{in} - T_\text{out}).
$$ {#eq-duct}

A fraction $f_\text{leak}$ of the inlet mass flow leaks out, so
$\dot m_\text{out} = (1 - f_\text{leak})\dot m$, with the outlet state
unchanged by the leak.

The ambient is set by `ambient_zone`:

- `outdoor`: the outdoor dry-bulb;
- `ground`: a fixed 18 °C, not the site ground temperature
  ([#235](https://github.com/bbrannon4/OpenBSE/issues/235));
- a zone name: that zone's air temperature. An unknown name silently uses
  the outdoor temperature
  ([#235](https://github.com/bbrannon4/OpenBSE/issues/235)).

**Losses to the ambient zone.** When the ambient is a zone, the losses
return to it as gains, multiplied by the duct's zone multiplier:

- sensible: $\dot Q_\text{cond} + \dot m_\text{leak}c_p(T_\text{in} - T_z)$;
- latent: $\dot m_\text{leak}(W_\text{in} - W_z)h_{fg}$, with
  $h_{fg} = 2.501\times10^6$ J/kg.

The duct reads that zone's temperature within the HVAC iteration
(@sec-hvac-iteration), so the coupling converges with it.

**Comparison with EnergyPlus.** EnergyPlus's `Duct` object is lossless. Duct
losses need AirflowNetwork distribution, whose duct conduction has the form
of @eq-duct [@eplus-engref-25-2, "Airflow Network Model"]. OpenBSE adds
this as a component. It has no inward (return-side) leakage
([#235](https://github.com/bbrannon4/OpenBSE/issues/235)).

## Known limitations: fans, heating coils and ducts {#sec-air-side-limitations-a}

- [#71](https://github.com/bbrannon4/OpenBSE/issues/71): VAV fan density
  and minimum flow fraction, on/off fan curves, gas-coil part-load curve and
  off-cycle parasitics.
- [#72](https://github.com/bbrannon4/OpenBSE/issues/72): Standard 205 fan
  at fixed static pressure; stale power after a failed map query.
- [#234](https://github.com/bbrannon4/OpenBSE/issues/234): hot-water coils
  never see their water inlet; the UA model is inert and has swapped
  exponents; electric-coil efficiency is not read.
- [#235](https://github.com/bbrannon4/OpenBSE/issues/235): duct ground
  ambient fixed at 18 °C, unknown ambient zones, no inward leakage.

## Remaining air-side components {#sec-air-side-remaining}

::: {.callout-warning}
## Pre-review content

The rest of this chapter holds text moved unchanged, apart from format, from the former single-file reference (`README.md`, mostly written in March 2026). Parts of it are stale or incomplete. It will be rewritten from the code, and checked against EnergyPlus, in this chapter's review ([#181](https://github.com/bbrannon4/OpenBSE/issues/181)); see @sec-status.
:::

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
