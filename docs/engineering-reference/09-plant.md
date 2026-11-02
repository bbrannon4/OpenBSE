# Plant {#sec-plant}

## Loop solution {#sec-plant-loop}

**Source:** `openbse-cli/src/main.rs` (plant step of the HVAC iteration);
`openbse-io/src/plant.rs` (`coil_plant_load`). The order of the loops is in @sec-plant-order.

Each HVAC iteration, each plant loop is solved from its load, in order.

**Load.** A loop's load $\dot Q$ is the sum of the thermal outputs of the water
coils and hot-water reheat terminals on it, each multiplied by its zone
multiplier, plus its water-source radiant panels. Heating is positive and
cooling negative. A condenser loop instead takes the heat rejected by the
chillers on it, $\sum(\dot Q_\text{ch} + P_\text{ch} - \dot Q_\text{rec})$.

**State.** With the design temperature difference $\Delta T_d$ and the supply
setpoint $T_\text{sp}$ (from the loop's outdoor reset if it has one), the loop's
flow and return temperature are

$$
\dot m = \frac{|\dot Q|}{c_{p,w}\,\max(\Delta T_d, 1)}, \qquad
T_\text{ret} = T_\text{sp} \mp \Delta T_d,
$$ {#eq-plant-state}

with the minus sign for a heating loop and $c_{p,w}$ at the loop's conversion
temperature (@sec-water-properties). A loop with no load is not simulated.

**Dispatch.** The supply equipment is simulated in series, in input order. Each
takes the outlet of the one before:

- **Pumps** see the whole load and flow.
- **Sequential staging** (`staging_mode`): each unit takes the remaining load.
  The next starts only if the last unit's output over its rated capacity reached
  `staging_threshold` (default 0.9), or it was unable to meet the load asked of it.
- **Equal split:** each boiler, chiller, thermal store or external plant gets an
  equal share of the load.

Heat recovered from a chiller on another loop (@sec-chiller) is taken off a
heating loop's load before its boilers run.

**Comparison with EnergyPlus.** EnergyPlus solves a loop's flow from its demand
side (coil control valves, limited by design flows and the pump) and tracks the
loop's temperature with its fluid volume and common-pipe options
[@eplus-engref-25-2, "Plant Flow Resolver"]. OpenBSE's loop is an algebraic
balance. It has no flow limit, no water temperature history, and its supply
state reaches neither coils nor chillers. A setpoint reset changes only @eq-plant-state, so it brings no chiller efficiency gain. Pump heat is not added
to the load ([#251](https://github.com/bbrannon4/OpenBSE/issues/251)).

## Pumps {#sec-pumps}

**Source:** `openbse-components/src/pump.rs` (`Pump`).

A pump's design power, from the design flow $\dot V_d$ and head $H$, is

$$
P_d = \frac{\dot V_dH}{\eta_m\eta_i},
$$ {#eq-pump-design}

with motor efficiency $\eta_m$ and impeller efficiency $\eta_i$ (default 0.667).
With the flow fraction $x = \min(\dot m/(\rho_w\dot V_d), 1)$, $\rho_w$ at the entering water:

- **Constant speed:** $P = P_d$ whenever the loop has a load.
- **Variable speed:** $P = P_df(\max(x, x_\text{min}))$, with $x_\text{min}$
  default 0.1 and $f$ the optional cubic $c_1 + c_2x + c_3x^2 + c_4x^3$, or
  $x^n$ (default $n = 3$).
- **Headered ($N$ pumps):** $\lceil xN\rceil$ pumps run, each at $P_d/N$ (constant)
  or at the variable-speed power for its share of the flow.

Heat to the water is $P f_\text{fluid}$ (default $f_\text{fluid} = 1$). It raises
the outlet temperature but not the load.

**Comparison with EnergyPlus.** @eq-pump-design is `Pump:VariableSpeed` with a
hydraulic efficiency of $1/1.282 = 0.78$ by default, where OpenBSE's is 0.667.
EnergyPlus's default part-load curve is linear ($f = x$), with no default
minimum flow. Its heat to the fluid is $\eta_mP + (1 - \eta_m)Pf_\text{fluid}$,
with a default $f_\text{fluid}$ of 0
([#251](https://github.com/bbrannon4/OpenBSE/issues/251)). Pump sizing is in
@sec-plant-sizing ([#230](https://github.com/bbrannon4/OpenBSE/issues/230)).

## Boilers {#sec-boilers}

**Source:** `openbse-components/src/boiler.rs` (`Boiler`).

The boiler meets $\dot Q = \min(\dot Q_\text{req}, \dot Q_\text{cap})$, with
$\text{PLR} = \operatorname{clamp}(\dot Q/\dot Q_\text{cap},\ \text{PLR}_\text{min},\ \text{PLR}_\text{max})$:

$$
\dot F = \frac{\dot Q}{\operatorname{clamp}(\eta_n f(\text{PLR}),\ 0.01,\ 1.1)}, \qquad
P_\text{par} = P_{\text{par},d}\text{PLR}, \qquad
T_\text{out} = T_\text{in} + \frac{\dot Q}{\dot m c_{p,w}},
$$ {#eq-boiler}

with $\eta_n$ the nominal efficiency and $f$ the optional efficiency curve. The
outlet is limited to 99.9 °C. In `LeavingSetpointModulated` mode, the outlet is
the leaving setpoint and the flow is the one that delivers the load, up to the
design flow.

**Comparison with EnergyPlus.** @eq-boiler matches `Boiler:HotWater`
(`Boilers.cc`, `CalcBoilerModel`) [@eplus-engref-25-2, "Boilers"]. EnergyPlus's
curve can also depend on water temperature, which a condensing boiler needs, and
it has an off-cycle parasitic fuel load
([#71](https://github.com/bbrannon4/OpenBSE/issues/71)).

## Chillers {#sec-chiller}

**Source:** `openbse-components/src/chiller.rs` (`AirCooledChiller`, which also
models water-cooled chillers: `operate`, `operate_reformulated`,
`operate_with_heat_recovery`, `condenser_temp`).

**EIR model.** At the condenser temperature $T_c$ and the chilled-water setpoint
$T_e$, with load $\dot Q$,

$$
\begin{gathered}
\dot Q_\text{av} = \dot Q_r\text{CAPFT}(T_e, T_c), \qquad
\text{PLR} = \min(\dot Q/\dot Q_\text{av}, 1), \qquad
c = \min(\text{PLR}/\text{PLR}_\text{min}, 1),\\
P = \frac{\dot Q_\text{av}}{\text{COP}_r}\text{EIRFT}(T_e, T_c)\,\text{EIRFPLR}\left(\max(\text{PLR}, u)\right)c, \qquad
\dot Q_\text{false} = \max\left(0,\ \max(\text{PLR}, u)\dot Q_\text{av}c - \text{PLR}\dot Q_\text{av}\right),
\end{gathered}
$$ {#eq-chiller}

where $c$ is the cycling ratio and $u$ the minimum unloading ratio (default
$\text{PLR}_\text{min}$, itself default 0.25). Without curves, three fallbacks
apply: $\text{CAPFT} = \operatorname{clamp}(1 - 0.015(T_c - 29.4),\ 0.5,\ 1.1)$,
$\text{EIRFT} = 1/\operatorname{clamp}(1 - 0.02(T_c - 29.4),\ 0.4,\ 1.1)$ and
$\text{EIRFPLR} = \text{PLR}$.

**Condenser temperature.** An air-cooled chiller uses the outdoor dry-bulb. A
water-cooled chiller uses $\max(T_{c,\text{sp}},\ T_{wb} + a)$ with the input
setpoint and approach $a$ (default 5.56 K), or $T_{wb} + a$. It does not read its
condenser loop ([#251](https://github.com/bbrannon4/OpenBSE/issues/251)).

**Reformulated EIR.** `model: reformulated_eir` evaluates the curves at the
*leaving* condenser temperature, with EIRFPLR a function of that temperature and
PLR. The temperature is found by fixed-point iteration on the condenser balance:

$$
T_{c,\text{out}} = T_{c,\text{in}} + \frac{P + \dot Q + \dot Q_\text{false}}{\dot m_cc_{p,w}},
$$ {#eq-chiller-reformulated}

with $\dot m_c$ the constant condenser flow, or sized from the capacity,
COP and $\Delta T_d$.

**Heat recovery.** An EIR chiller can reject part of its heat to another loop.
It recovers

$$
\dot Q_\text{rec} = \min\left(\dot Q_\text{rej},\ \dot m_\text{hr}c_{p,w}(T_\text{hr,sp} - T_\text{hr,in}),\ f_\text{hr}\dot Q_{\text{rej},r},\ \dot Q_\text{demand}\right),
$$ {#eq-chiller-hr}

and the curves read the heat-weighted blend of the two entering temperatures.
The blend and split are solved together. Recovery stops above an inlet high
limit.

**Water side.** The outlet is $T_\text{in} - \dot Q/(\dot mc_{p,w})$, limited to
2 K below the setpoint.

**Comparison with EnergyPlus.** @eq-chiller and @eq-chiller-reformulated match
`Chiller:Electric:EIR` and `Chiller:Electric:ReformulatedEIR` (`ChillerElectricEIR.cc`,
`ChillerReformulatedEIR.cc`) [@eplus-engref-25-2, "Electric Chiller Model Based
on Condenser Entering Temperature"]. The differences are
([#251](https://github.com/bbrannon4/OpenBSE/issues/251)):

- EnergyPlus reads the actual leaving chilled-water and condenser temperatures.
- EnergyPlus requires the curves.
- The condenser loop's load leaves out the false load, so energy doesn't close
  while false-loading.
- OpenBSE has no air-cooled condenser fan power ratio.

## ASHRAE 205 chillers {#sec-chiller-a205}

**Source:** `openbse-components/src/chiller_a205.rs` (`ChillerA205`).

A chiller with an `a205_file` reads a Standard 205 RS0001 map [@ashrae205] of
net capacity and power against the evaporator and condenser states and the
compressor sequence. The PLR is the load over the full-sequence capacity, mapped
to a sequence, with cycling below the minimum. The condenser temperature follows @sec-chiller. The map is queried at the file's evaporator and condenser flows.

**Comparison with EnergyPlus.** EnergyPlus's `Chiller:Electric:ASHRAE205` reads
the same data. OpenBSE's differences, including the fixed flows, are in
[#72](https://github.com/bbrannon4/OpenBSE/issues/72).

## Cooling towers {#sec-cooling-tower}

**Source:** `openbse-components/src/cooling_tower.rs` (`CoolingTower`).

A tower rejects heat from the condenser water, limited by a minimum approach of
2 K above the outdoor wet-bulb:

$$
\dot Q = \min\left(\dot Q_\text{req},\ \dot m c_{p,w}\max(T_\text{in} - T_{wb} - 2,\ 0)\right), \qquad
T_\text{out} = T_\text{in} - \frac{\dot Q}{\dot m c_{p,w}}.
$$ {#eq-tower}

With $x = \min(\dot Q/\dot Q_d, 1)$ and
$\dot Q_d = \rho\dot V_dc_{p,w}\Delta T_{\text{range},d}$, fan power is:

- **Single speed:** $P_d$ whenever it rejects heat.
- **Two speed:** $P_d$ for $x > 0.5$, otherwise $P_df(0.5)$.
- **Variable speed:** $P_df(x)$, with
  $f(x) = 0.0058 + 0.0740x - 0.2180x^2 + 0.8517x^3 + 0.2866x^4$ by default.

**Comparison with EnergyPlus.** `CoolingTower:SingleSpeed` and `:TwoSpeed` use a
UA from design conditions in the Merkel effectiveness model, with fan cycling.
`CoolingTower:VariableSpeed` solves the air-flow ratio from an approach model
[@eplus-engref-25-2, "Cooling Towers and Evaporative Fluid Coolers"]. @eq-tower has no such model, so a tower's
size does not limit its rejection, and fan power does not cycle. There is no
water use ([#252](https://github.com/bbrannon4/OpenBSE/issues/252)).

## Water-to-water heat exchangers {#sec-plant-hx}

**Source:** `openbse-components/src/heat_exchanger.rs` (`WaterToWaterHX`).

A heat exchanger on a demand loop draws on a source loop at fixed effectiveness
$\varepsilon$:

$$
\dot Q = \min\left(|\dot Q_\text{req}|,\ \varepsilon\min(C_d, C_s)|T_s - T_\text{in}|\right),
$$ {#eq-plant-hx}

in the load's direction, if the temperatures allow it. In `Economizer` mode, it
runs only when $T_s < T_\text{in} - \Delta T_\text{th}$ (default 2 K).

**Comparison with EnergyPlus.** `HeatExchanger:FluidToFluid` derives its
effectiveness from UA and the flows
([#71](https://github.com/bbrannon4/OpenBSE/issues/71)). In OpenBSE, the heat moved
never reaches the source loop as a load, and the source state is that of the last
time the source loop ran
([#253](https://github.com/bbrannon4/OpenBSE/issues/253)).

## Thermal storage {#sec-thermal-storage}

**Source:** `openbse-components/src/thermal_storage.rs` (`ThermalStorage`).

A chilled-water or ice store holds energy $E$ [Wh], with three strategies:

- **Full storage:** charge at the maximum rate off-peak, discharge to the load on
  peak.
- **Load leveling:** discharge the load above `max_charge_rate`; charge with the
  slack below it.
- **Demand limiting:** discharge the load above a threshold.

Discharge is also limited by the maximum rate and the stored energy. The state
evolves with a standby loss at 20 °C ambient and a fixed store temperature (6 °C or
−5 °C):

$$
E^{t+\Delta t} = \operatorname{clamp}\left(E^t + \left[\dot Q_\text{ch} - \dot Q_\text{dis} - UA(20 - T_\text{store})\right]\frac{\Delta t}{3600},\ 0,\ E_\text{cap}\right).
$$ {#eq-storage}

The store draws 500 W while charging or discharging.

**Comparison with EnergyPlus.** EnergyPlus's `ThermalStorage:Ice:Simple` and
`ThermalStorage:ChilledWater:Mixed` are charged by a chiller. In OpenBSE charging
costs nothing: $\dot Q_\text{ch}$ puts no load on a chiller, so storage's savings
are overstated. It also runs only while the loop has a load
([#253](https://github.com/bbrannon4/OpenBSE/issues/253)).

## Known limitations {#sec-plant-limitations}

- [#251](https://github.com/bbrannon4/OpenBSE/issues/251): loops solved from the
  load, no flow limit or temperature feedback, reset inert, condenser temperature
  not from the tower, pump defaults and heat.
- [#230](https://github.com/bbrannon4/OpenBSE/issues/230): pump sizing basis.
- [#252](https://github.com/bbrannon4/OpenBSE/issues/252): tower has no
  effectiveness model, fan cycling or water use.
- [#253](https://github.com/bbrannon4/OpenBSE/issues/253): storage charges free;
  heat-exchanger heat never reaches its source loop.
- [#234](https://github.com/bbrannon4/OpenBSE/issues/234),
  [#239](https://github.com/bbrannon4/OpenBSE/issues/239): coils don't see their
  loop's water.
- [#71](https://github.com/bbrannon4/OpenBSE/issues/71),
  [#72](https://github.com/bbrannon4/OpenBSE/issues/72): boiler curve,
  heat-exchanger effectiveness, Standard 205 chiller flows.
- [#36](https://github.com/bbrannon4/OpenBSE/issues/36),
  [#35](https://github.com/bbrannon4/OpenBSE/issues/35): no absorption chillers or
  district connections.
