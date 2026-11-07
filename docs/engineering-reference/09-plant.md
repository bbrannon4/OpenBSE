# Plant {#sec-plant}

## Loop solution {#sec-plant-loop}

**Source:** `openbse-cli/src/main.rs` (plant step of the HVAC iteration);
`openbse-io/src/plant.rs` (`coil_plant_load`). The order of the loops is in @sec-plant-order.

Each HVAC iteration, each plant loop is solved from its load, in order.

**Load.** A loop's load $\dot Q$ is the sum of the thermal outputs of the water
coils and hot-water reheat terminals on it, each multiplied by its zone
multiplier, plus its water-source radiant panels. Heating is positive and
cooling negative. A condenser loop instead takes the heat rejected by the
chillers on it, $\sum(\dot Q_\text{ch} + P_\text{ch} + \dot Q_\text{false} - \dot Q_\text{rec})$,
with the false load of @eq-chiller.

**Coils.** Before the air side runs, each water coil and hot-water reheat
terminal on the loop gets the loop's supply water: up to its design water flow
(@sec-heating-coils, @sec-chw-coil) at the supply temperature

$$
T_\text{sup} = T_\text{sp} + \delta,
$$ {#eq-plant-supply}

with $T_\text{sp}$ the supply setpoint (from the loop's outdoor reset if it has
one) and $\delta$ the shortfall of the last pass, below. Each coil's valve takes
the water it needs and reports its flow.

**State.** The loop's flow is the sum of its coils' water flows, each times its
zone multiplier, plus each water radiant panel's load at the design difference
$\Delta T_d$. A condenser loop, or a loop whose coils drew no water, takes its
load at $\Delta T_d$. The water returns with what the load did to it:

$$
\dot m = \sum_k \dot m_{w,k} + \frac{|\dot Q_\text{panels}|}{c_{p,w}\Delta T_d}, \qquad
T_\text{ret} = T_\text{sp} \mp \frac{|\dot Q|}{\dot m c_{p,w}},
$$ {#eq-plant-state}

with the minus sign for a heating loop, $\Delta T_d$ at least 1 K and
$c_{p,w}$ at the loop's conversion temperature (@sec-water-properties). A
condenser loop returns $\Delta T_d$ warmer. A loop with no load is not
simulated.

**Shortfall.** If the supply equipment delivers less than the load, the water
leaves short of setpoint by
$\delta = \mp(|\dot Q| - \dot Q_\text{del})/(\dot m c_{p,w})$, and the coils see
that water on the next pass. Since a coil's output rises more slowly with its
water temperature than the water's own capacity rate, the passes converge to the
output the equipment can supply.

**Flow limit.** If the coils draw more water than the loop's smallest pump
moves at its design flow, every coil's available flow is scaled down by that
ratio for the rest of the timestep.

**Dispatch.** The supply equipment is simulated in series: the pumps first, as
at the inlet of the supply side, then the rest in input order. Each takes the
outlet of the one before:

- **Pumps** see the whole load and flow. Their heat (@eq-pump-heat) is in the
  water the other units receive: it comes off a heating loop's load and is added
  to a cooling or condenser loop's, as $\dot Q - \dot Q_\text{pump}$ in the
  load's sign (a condenser loop's rejection counts positive).
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
balance on the same demand-side flows: its coils' valves set the flow, limited
by their design flows and by the pump, and the supply temperature they see falls
when the equipment falls short. It has no water temperature history (loop
volume, common pipe), and a constant-speed pump does not bypass its excess flow
to the return ([#251](https://github.com/bbrannon4/OpenBSE/issues/251)). The
supply setpoint, its reset included, reaches the coils, the chillers' curves and
leaving-setpoint boilers, as an EnergyPlus setpoint manager sets the supply
outlet node's setpoint that they read. Pump heat enters
the load as EnergyPlus adds a loop side's `TotalPumpHeat` to its water. Because
the loop has no water temperature history, the coils see the setpoint, and pump
heat beyond a heating loop's load is not carried to the next timestep.

## Pumps {#sec-pumps}

**Source:** `openbse-components/src/pump.rs` (`Pump`).

A pump's design power, from the design flow $\dot V_d$ and head $H$, is

$$
P_d = \frac{\dot V_dH}{\eta_m\eta_i},
$$ {#eq-pump-design}

with motor efficiency $\eta_m$ and impeller efficiency $\eta_i$ (default 0.78).
With the flow fraction $x = \min(\dot m/(\rho_w\dot V_d), 1)$, $\rho_w$ at the entering water:

- **Constant speed:** $P = P_d$ whenever the loop has a load.
- **Variable speed:** $P = P_df(x)$, with $f$ the cubic
  $c_1 + c_2x + c_3x^2 + c_4x^3$, by default $f = x$.
- **Headered ($N$ pumps):** $\lceil xN\rceil$ pumps run, each at $P_d/N$ (constant)
  or at the variable-speed power for an equal share of the flow.

All the shaft work ends up in the water, and the fraction $f_\text{fluid}$
(default 0) of the motor losses:

$$
\dot Q_\text{fluid} = \eta_mP + (1 - \eta_m)Pf_\text{fluid}.
$$ {#eq-pump-heat}

It raises the outlet temperature and enters the loop's load
(@sec-plant-loop).

**Comparison with EnergyPlus.** @eq-pump-design and @eq-pump-heat match
`Pump:VariableSpeed` and `Pump:ConstantSpeed` (`Pumps.cc`, `CalcPumps`), whose
default design shaft power per unit flow per unit head, 1.282051282 W/((m³/s)·Pa),
is $\eta_i = 0.78$, and whose default part-load coefficients are (0, 1, 0, 0).
EnergyPlus's headered pumps run all but one at full flow and the last at part
load; OpenBSE runs the pumps that are on at an equal share, as parallel pumps on
one speed signal do. The two agree for a linear curve. OpenBSE has no minimum
flow and no continuous pump control: a pump runs only while its loop has a load,
as an EnergyPlus `Intermittent` pump does. Pump sizing is in
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
the loop's supply setpoint $T_\text{sp}$ and the flow is the one that delivers
the load, up to the design flow.

**Comparison with EnergyPlus.** @eq-boiler matches `Boiler:HotWater`
(`Boilers.cc`, `CalcBoilerModel`) [@eplus-engref-25-2, "Boilers"]. EnergyPlus's
curve can also depend on water temperature, which a condensing boiler needs, and
it has an off-cycle parasitic fuel load
([#71](https://github.com/bbrannon4/OpenBSE/issues/71)).

## Chillers {#sec-chiller}

**Source:** `openbse-components/src/chiller.rs` (`AirCooledChiller`, which also
models water-cooled chillers: `operate`, `operate_reformulated`,
`operate_with_heat_recovery`, `condenser_temp`).

**EIR model.** At the condenser temperature $T_c$ and the loop's supply
setpoint $T_e = T_\text{sp}$ (@eq-plant-supply, its reset included), with load
$\dot Q$,

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

- EnergyPlus reads the condenser loop's actual temperature, where OpenBSE uses
  the condenser temperature above. Both evaluate the curves at the leaving
  chilled-water setpoint.
- EnergyPlus requires the curves.
- OpenBSE has no air-cooled condenser fan power ratio.

## ASHRAE 205 chillers {#sec-chiller-a205}

**Source:** `openbse-components/src/chiller_a205.rs` (`ChillerA205`).

A chiller with an `a205_file` reads a Standard 205 RS0001 map [@ashrae205] of
net capacity and power against the evaporator and condenser states and the
compressor sequence, at the loop's supply setpoint and the condenser temperature
of @sec-chiller. The chiller meets the load exactly:

- below the first stage's capacity $\dot Q_1$ it cycles that stage, at power
  $P_1c/(1 - C_d(1 - c))$ with $c = \dot Q/\dot Q_1$ and the file's cycling
  degradation coefficient $C_d$;
- with continuous control, it runs at the sequence whose capacity is the load;
- with discrete stages, it cycles between the two stages that bracket the load,
  at power $fP_{k+1} + (1 - f)P_k$ with $f$ the time at the upper stage;
- above full capacity, it runs the full stage.

The map is queried at the file's evaporator and condenser flows. Its condenser
heat is the cooling plus the power.

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

The load over the design capacity, $x = \min(\dot Q/\dot Q_d, 1)$ with
$\dot Q_d = \rho\dot V_dc_{p,w}\Delta T_{\text{range},d}$, stands for the air-flow
ratio the fan needs. Fan power is:

- **Single speed:** $P_dx$: the fan runs that fraction of the step.
- **Two speed:** low speed moves half the air at $0.16P_d$ and cycles up to
  $x = 0.5$, giving $0.16P_d\,x/0.5$; above that the fan cycles between low and
  high speed, $hP_d + (1 - h)0.16P_d$ with $h = (x - 0.5)/0.5$.
- **Variable speed:** $P_df(x)$, with $f$ the quartic `fan_power_curve`, by default
  $f(x) = x^3$.

The design air flow, inlet temperature and approach are not read; a warning says
so if they are given.

**Comparison with EnergyPlus.** The fan logic follows EnergyPlus: a single-speed
fan cycles to the fraction of the step needed, a two-speed fan cycles at low
speed and then between speeds (`FanModeFrac`), with low-speed defaults of half
the air flow and 0.16 of the power, and a variable-speed tower without a curve
uses $x^3$. The difference is $x$: EnergyPlus finds the fraction or air-flow
ratio from the tower's capacity at the current wet-bulb and water state, with a
UA from the design conditions in the Merkel model (single and two speed) or an
approach model (variable speed) [@eplus-engref-25-2, "Cooling Towers and
Evaporative Fluid Coolers"]. @eq-tower has no such model, so a tower's size does
not limit its rejection and its part load reads against the design capacity.
There is no water use, basin heater, free convection or cell staging
([#252](https://github.com/bbrannon4/OpenBSE/issues/252)).

## Water-to-water heat exchangers {#sec-plant-hx}

**Source:** `openbse-components/src/heat_exchanger.rs` (`WaterToWaterHX`);
`openbse-cli/src/main.rs` (plant step).

A heat exchanger on a demand loop draws its design flow $\dot V_d$ from a source
loop and moves heat at fixed effectiveness $\varepsilon$:

$$
\dot Q = \min\left(|\dot Q_\text{req}|,\ \varepsilon\min(C_d, C_s)|T_s - T_\text{in}|\right),
$$ {#eq-plant-hx}

in the load's direction (it is the one supply unit that gets the load's sign),
if the temperatures allow it, with $C_s = \rho_w\dot V_dc_{p,w}$ at $T_s$. In
`economizer` mode, it runs only when $T_s < T_\text{in} - \Delta T_\text{th}$
(default 2 K).

The source water arrives at the source loop's supply setpoint less any shortfall
(@eq-plant-supply), or, on a loop with a cooling tower, at
$\max(T_\text{sp},\ T_{wb} + 2\ \text{K})$, the coldest the tower can make it
(@sec-cooling-tower). The demand loop runs first (@sec-plant-order), and the
heat moved is load on the source loop: heat for a tower loop to reject, or
heating or cooling for any other.

**Comparison with EnergyPlus.** `HeatExchanger:FluidToFluid` derives its
effectiveness from UA and the flows
([#71](https://github.com/bbrannon4/OpenBSE/issues/71)), and its source side is a
demand component of the source loop, as here. EnergyPlus's source water is at
that loop's actual supply temperature; OpenBSE's follows the setpoint and the
tower's wet-bulb limit, since its loops have no temperature history
([#251](https://github.com/bbrannon4/OpenBSE/issues/251)). Both sides draw the
same design flow, as EnergyPlus autosizes the side on the source loop to the
flow of the side it serves (`SizeFluidHeatExchanger`).

## Thermal storage {#sec-thermal-storage}

**Source:** `openbse-components/src/thermal_storage.rs` (`ThermalStorage`);
`openbse-cli/src/main.rs` (plant step).

A chilled-water or ice store on a chilled-water loop holds energy $E$ [Wh]. Each
timestep, before the loop is solved and whether or not it has a load, the store
plans from the coils' cooling load $\dot Q_c$ with one of three strategies:

- **Full storage:** charge at `max_charge_rate` off-peak; discharge on peak.
- **Load leveling:** the chillers run at `max_charge_rate`: the store charges with
  the slack below it and discharges the load above it.
- **Demand limiting:** discharge the load above `threshold_w`.

A store never charges and discharges in the same step. Its charge $\dot Q_\text{ch}$
is added to the loop's cooling load, so the loop's chillers make it; discharge
meets the load the dispatch passes to the store, up to its plan. Charging comes
last: if the chillers fall short, the charge is cut first, and the store keeps
only what was made. Ice charging costs the chillers $1/f$ times their power for
the charge, with $f$ = `ice_charge_cop_factor` (default 0.85); the difference is
booked to the store as cooling electricity.

The state evolves once per timestep, from its value at the start of the step
however many HVAC iterations run, with a standby loss at 20 °C ambient and a
fixed store temperature (6 °C or −5 °C):

$$
E^{t+\Delta t} = E^t - \dot Q_\text{loss}\frac{\Delta t}{3600} + \left(\dot Q_\text{ch} - \dot Q_\text{dis}\right)\frac{\Delta t}{3600}, \qquad
\dot Q_\text{loss} = \min\left(UA(20 - T_\text{store}),\ \frac{3600E^t}{\Delta t}\right).
$$ {#eq-storage}

Charge is limited by the maximum rate and the room left after the loss, and
discharge by the maximum rate and the energy left after it, so $E$ stays within
$[0, E_\text{cap}]$ and closes exactly. A store must sit on a loop with a
chiller.

**Comparison with EnergyPlus.** EnergyPlus's `ThermalStorage:Ice:Simple` and
`ThermalStorage:ChilledWater:Mixed` are plant components on the chiller's branch:
the chiller charges them at the leaving temperature the loop asks for, and their
loss follows the tank temperature and the ambient. OpenBSE charges at the loop's
setpoint with the $1/f$ penalty for ice, and its loss uses a fixed store
temperature and a 20 °C ambient
([#253](https://github.com/bbrannon4/OpenBSE/issues/253)).

## Known limitations {#sec-plant-limitations}

- [#251](https://github.com/bbrannon4/OpenBSE/issues/251): no loop thermal
  capacity, condenser temperature not from the tower.
- [#230](https://github.com/bbrannon4/OpenBSE/issues/230): pump sizing basis.
- [#252](https://github.com/bbrannon4/OpenBSE/issues/252): tower has no
  effectiveness model (size doesn't limit rejection) or water use.
- [#253](https://github.com/bbrannon4/OpenBSE/issues/253): storage is a
  simplified component, not a chiller-storage plant branch.
- [#71](https://github.com/bbrannon4/OpenBSE/issues/71),
  [#72](https://github.com/bbrannon4/OpenBSE/issues/72): boiler curve,
  heat-exchanger effectiveness, Standard 205 chiller flows.
- [#36](https://github.com/bbrannon4/OpenBSE/issues/36),
  [#35](https://github.com/bbrannon4/OpenBSE/issues/35): no absorption chillers or
  district connections.
