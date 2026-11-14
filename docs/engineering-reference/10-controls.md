# Controls and systems {#sec-controls}

Air-loop controls run as a set of signal builders, one for each system type.
Each HVAC iteration, a builder reads the zones' state and sets the flows and coil
setpoints for its loop's components (@sec-hvac-iteration). A loop's type comes
from `system_type`, or is detected: a VAV fan means VAV, a `minimum_damper_position`
of 1 means DOAS, and anything else is PSZ-AC.

**Source:** `openbse-airloop/src/lib.rs` (`build_*_signals`); `main.rs`
(`simulate_all_loops`, `apply_sat_reset`, `apply_heat_recovery`,
`apply_plant_reset`).

The `openbse-controls` crate supplies only the thermostats' constant setpoints on
the coupled path. Setpoint-manager and plant-setpoint actions from a `controls:`
block take effect only in HVAC-only mode, and a warning says so.

## Thermostats and schedules {#sec-thermostats}

**Source:** `openbse-controls/src/thermostat.rs`; `openbse-envelope/src/zone.rs`
(`ThermostatScheduleEntry`); `openbse-envelope/src/schedule.rs`.

**Thermostats.** A thermostat gives a set of zones a heating and a cooling
setpoint. A `thermostat_schedule` overrides these by hour (1–24), optionally
within a date range. The first entry that applies wins. Named setpoint
schedules (`heating_setpoint_schedule`, `cooling_setpoint_schedule`) instead set
each side every timestep from a schedule of temperatures, read by hour and day
type, holidays included, as EnergyPlus `ThermostatSetpoint:DualSetpoint`; a
thermostat takes one form or the other. The scheduled setpoints drive the mode,
the load to setpoint, the coil control and unmet hours alike. Design days hold
their own setpoints (`design_day_*_setpoints`, @sec-dd-runs) and set the
schedules aside.

**Mode.** A zone's mode comes from the predictor (@sec-predictor) and is held for
the timestep. It is heating below the heating setpoint, cooling above the cooling
setpoint, and deadband between them. PSZ-AC adds a guard: heating with no heating
load but a cooling load becomes cooling, and the reverse. A zone over its maximum
relative humidity in deadband becomes cooling, with the coil aimed 0.5 K below the
zone to dehumidify.

**Schedules.** A named schedule gives a fraction for each hour, by day type (a
weekday, or each day individually, Saturday, Sunday, weekend or holiday, each
falling back to the next), from arrays or `"value until HH:MM"` strings. Optional
seasons override them by date range. Hour $h$ covers the hour ending at $h$. An
`until` boundary inside an hour gives that hour the time-weighted average of the
values it spans, which keeps the schedule's integral. A string must reach 24:00.
Values are fractions: anything outside 0 to 1 is clamped, with a validation warning,
except in a thermostat's setpoint schedules, which hold temperatures and are read
unclamped. A reference to an undefined schedule is a severe error.

The day of week starts from the weather file's January 1. A file containing
29 February has that day dropped, because the simulation calendar has 365 days,
but later dates keep their true day of week.

**Comparison with EnergyPlus.** EnergyPlus's `ZoneControl:Thermostat` takes
setpoint schedules, as OpenBSE's named setpoint schedules do. Schedules have no
design-day types, so a design day's setpoints are given on the thermostat; nor
daylight saving or start day of week
([#199](https://github.com/bbrannon4/OpenBSE/issues/199)).

## Availability {#sec-availability}

A loop whose `availability_schedule` is below 0.5 for the hour is off. Its
components report zero, and its zones float, unless a night-cycle manager runs it.
`unoccupied_heating_setpoint` and `unoccupied_cooling_setpoint` are read and never
used, and the code for them is dead
([#255](https://github.com/bbrannon4/OpenBSE/issues/255)); a thermostat's setpoint
schedules carry a setback (@sec-thermostats).

**Night cycle.** A loop's `night_cycle` (EnergyPlus `AvailabilityManager:NightCycle`
with FixedRunTime, `CalcNCycSysAvailMgr`) runs it while its schedule has it off.
At each timestep, with the zone air temperatures at the start of the timestep
$T_z$ and the setpoints now, the loop cycles on when a watched zone (any zone it
serves, or its control zone) has

$$
T_z < T_h - \tfrac12\,\Delta T_\text{tol} \quad\text{or}\quad T_z > T_c + \tfrac12\,\Delta T_\text{tol},
$$ {#eq-night-cycle}

with $\Delta T_\text{tol}$ the thermostat tolerance (half of it under FixedRunTime,
as E+). It then runs for the cycling run time rounded to whole timesteps,
$n = \operatorname{round}(t_\text{run}\, N_h / 3600)$, whatever the zones do, is
held off for the timestep after, and is checked again on the next. While it runs
the fan cycles with the coils, as E+ runs a unit whose fan operating-mode schedule
is its availability schedule (how the DOE prototypes set it), and outdoor air
follows `minimum_oa_schedule`. The cycle restarts when the schedule turns the loop
on.

**Comparison with EnergyPlus.** The control types CycleOnAny and
CycleOnControlZone and the FixedRunTime run time are E+'s; the Thermostat and
ThermostatWithMinimumRunTime run times, the zone-fan-only types and an
applicability schedule are not modelled. E+ restarts the run-time count at the
start of each warmup day; OpenBSE clears it when warmup ends.

## Part-load cycling {#sec-plr}

**Source:** `main.rs` (`simulate_all_loops`).

PSZ-AC, PTAC and PTHP loops simulate their components at full flow, then find the
loop part-load ratio from the zone load, and scale the outputs by it. From the
supply state, with $T_\text{ref} = \min(T_h, T_z)$ in heating and
$\max(T_c, T_z)$ in cooling, the net capacity is
$\dot Q_\text{cap} = \dot mc_p|T_\text{sup} - T_\text{ref}|$, with $\dot m$ the control
zone's share of the supply: all of it for a single-zone unit, $1/n$ of it for a PSZ-AC
serving $n$ zones, as E+ `UnitarySystem` divides the control zone's load by its
`ControlZoneMassFlowFrac`. Then

$$
x = \begin{cases}
\dfrac{\dot Q_h - \dot Q_f - \dot Q_\text{oa}}{\dot Q_\text{cap} - \dot Q_f - \dot Q_\text{oa}} & \text{heating},\\[2ex]
\dfrac{\dot Q_c + \dot Q_f + \dot Q_\text{oa}}{\dot Q_\text{cap} + \dot Q_f + \dot Q_\text{oa}} & \text{cooling},
\end{cases}
$$ {#eq-plr}

clamped to $[0, 1]$. The terms are:

- $\dot Q_h$ and $\dot Q_c$, the zone's loads to setpoint;
- $\dot Q_f$, the heat of a continuous fan, which arrives in the off-cycle too;
- $\dot Q_\text{oa} = x_\text{oa}\dot mc_p(T_o - T_z)$, the outdoor-air load in
  the off-cycle of a continuous fan, zero for a cycling fan.

A cycling fan runs only for $x$ of the timestep, so with no load it is off and its
outdoor air with it; a continuous fan moves its outdoor air all the time.

If the load is under 10 W, a proportional fallback applies:
$x = |T_\text{sp} - T_z|/|T_\text{sup} - T_\text{sp}|$. If the zone started the
timestep more than half the deadband beyond the setpoint on the far side, or the
unit has no equipment that heats or cools as the mode needs (a heating-only unit
when the zone wants cooling), the output is 0.

**Scaling.** Coil and fan outputs and flows scale by $x$. DX compressors and
condenser fans take the cycling penalty of @eq-dx-rtf at their combined part load,
and the cycling fan of a loop with DX runs $x/\text{PLF}(x)$ of the time while a
compressor runs (@sec-dx-cooling). Continuous fans stay at full power.
`ContinuousNoLoadOff` fans stop in deadband.

**No-load output.** With the coils off, a continuous fan still supplies its heat
and its minimum outdoor air: $\dot Q_\text{off} = \dot Q_f + \dot Q_\text{oa}$,
positive when it heats the zone. In @eq-plr it is delivered whether the coils run
or not, so the coils make up only the rest, for heating as for cooling. A zone in
its dead band, with signed loads to its setpoints $Q_{\text{to},h} < 0$ and
$Q_{\text{to},c} > 0$, is pushed out of it by that output, as in E+
`UnitarySystem::initLoadBasedControl` (DualHeatCool):

- cold outdoor air with $\dot Q_\text{off} < 0$ and
  $\dot Q_\text{off} - Q_{\text{to},h} < -1$ W ("constant fan pushes zone below
  heating set point") turns on heating, for the load $Q_{\text{to},h}$;
- fan heat with $\dot Q_\text{off} > 0$ and $\dot Q_\text{off} - Q_{\text{to},c} > 1$ W
  ("above cooling set point") turns on cooling, for the load $Q_{\text{to},c}$.

The mode is decided once per timestep from the design flow, the minimum
outdoor-air fraction and the zone at the start of the timestep.

**Comparison with EnergyPlus.** EnergyPlus's `AirLoopHVAC:UnitarySystem` solves
its part-load ratio iteratively against the unit's output; @eq-plr is its closed
form. As in EnergyPlus, a cycling fan has zero flow with no load
(`Fan:OnOff` with a cycling UnitarySystem), and a small load is met with a small
part load. PSZ-AC serves several
zones evenly, controlled by the first; E+ also controls from one zone but splits by
design flow ([#389](https://github.com/bbrannon4/OpenBSE/issues/389)).

## Multi-speed packaged units {#sec-unitary-speeds}

**Source:** `openbse-cli/src/unitary.rs` (`simulate_unitary`).

A PSZ-AC loop with a multi-speed DX coil (@sec-dx-multispeed), or a
`no_load_flow_fraction` or `heating_flow_fraction` below 1, runs as E+
`AirLoopHVAC:UnitarySystem` with `UnitarySystemPerformance:Multispeed` and a
`Fan:SystemModel`. It has a few operating points $j$, each its components run
at one supply flow with the coils in one state:

- **off**: the coils off, at the no-load flow if the fan keeps running, no flow
  if it cycles;
- **heating**: the heating coil at the heating supply temperature, at the
  heating flow;
- **cooling speed $k$**: the DX coil at full output at speed $k$, at that
  speed's flow.

At each point the minimum outdoor air is a fixed flow (E+ `Controller:OutdoorAir`
FixedMinimum), a larger share of a smaller supply; an open economizer keeps its
share. The point's sensible output to the control zone is
$\dot Q_j = s\,\dot m_jc_p(T_{\text{sup},j} - T_z)$, with $T_z$ the zone at the start of
the timestep and $s$ the control zone's share of the supply. The timestep is
split between two points so their time-weighted output meets the zone load $L$
(E+ `UnitarySys::controlUnitarySystemtoLoad`):

$$
r = \frac{L - \dot Q_a}{\dot Q_b - \dot Q_a}, \qquad
\bar X = (1 - r)\,X_a + r\,X_b,
$$ {#eq-unitary-mix}

with $r$ clamped to $[0, 1]$:

- heating: $a$ off, $b$ heating;
- cooling up to speed 1: $a$ off, $b$ speed 1, the compressor's electricity
  taking $r/\text{PLF}(r)$ instead of $r$ (@eq-dx-rtf);
- cooling above speed 1: $a$ and $b$ two adjacent speeds, no cycling penalty;
- dead band: off alone.

Rates and flows $X$ (power, fuel, thermal output, mass flow) mix by
@eq-unitary-mix; each component's inlet and outlet states, and the supply air,
mix by mass (enthalpy and humidity ratio), so $\dot m\Delta h$ of the mix is
the mixed thermal output. A dead-band zone that the off point's output pushes
past a setpoint is heated or cooled as in @sec-plr, with the off point's actual
output. A fan's power follows its speeds (@eq-fan-speeds) at each point's flow.

**Comparison with EnergyPlus.** EnergyPlus iterates the cycling and speed
ratios against the unit's output; @eq-unitary-mix is the same split in closed
form, from outputs measured at the start-of-timestep zone temperature. The
heating coil is single-stage (one heating flow). E+'s `SingleMode` operation
and multi-speed heating coils are not modelled.

## PSZ-AC {#sec-psz}

**Source:** `build_psz_signals`.

**Coils.** In heating, the coil aims at the heating supply temperature. With
`cycling: proportional`, it aims at
$T_h + (T_{s,h} - T_h)\min(e/5, 1)$ from the temperature error $e$ at the start of
the timestep, which leaves a steady offset. In cooling, the coil runs at full
capacity. Part load comes from @sec-plr.

**Single-zone reset.** With a `SingleZone` supply-temperature reset (E+
`SetpointManager:SingleZone:Heating` and `:Cooling`), the loop does not cycle. The
coil setpoints are

$$
T_{s,h} = \operatorname{clamp}\left(T_h + \frac{\dot Q_h}{\dot mc_p},\ T_\text{min},\ T_\text{max}\right) - \Delta T_f, \qquad
T_{s,c} = \operatorname{clamp}\left(T_c - \frac{\dot Q_c}{\dot mc_p},\ T_\text{min},\ T_\text{max}\right) - \Delta T_f,
$$ {#eq-single-zone-sat}

with the fan rise $\Delta T_f$ removed, as the fan is downstream of the coils. A
variable-speed fan first turns the flow down, between its minimum and design, with
cooling at the coldest supply temperature (E+ PSZ-VAV).

The economizer is in @sec-economizer.

## Multi-zone VAV {#sec-vav-control}

**Source:** `build_vav_signals`. The boxes are in @sec-vav-box.

**Supply temperature.** The supply setpoint is the warmest one that meets every
cooling zone at its design flow (E+ `SetpointManager:Warmest`):

$$
T_s = \operatorname{clamp}\left(\min_z\left(T_z - \frac{\dot Q_{c,z}}{\dot m_{d,z}c_p}\right),\ T_{s,\text{min}},\ T_{s,\text{max}}\right),
$$ {#eq-warmest}

over zones in cooling with over 100 W of load, and starting from $T_{s,\text{max}}$.
$T_{s,\text{min}}$ is the loop's `cooling_supply_temp` and
$T_{s,\text{max}} = \max(15.6\ °\text{C},\ T_{s,\text{min}})$. With
`fixed_cooling_sat`, it is the fixed supply temperature. Each coil aims at $T_s$,
less the fan's rise if the fan is downstream, which scales with the square of the
flow fraction. The design rise is $\Delta T_f = \Delta p\,f_m/(\eta\,\rho_\text{std}\,c_p)$,
with $f_m$ the share of motor power that reaches the air and $\rho_\text{std}$ the site's
standard density, as E+ fan power uses `StdRhoAir`. A preheat coil can hold a fixed setpoint.

**Zone flows.** In cooling, each zone takes
$\dot Q_c/[c_p\max(T_z - T_s, 1)]$, between its box's minimum and design flow. In
deadband it takes its minimum. In heating it takes the flow its box runs at
(@eq-vav-dual-max): the minimum while reheat to $T_{r,\text{max}}$ covers the load,
then just enough flow to carry it at that temperature, up to the box's heating
maximum. A zone without a sized box takes a fraction rising to 0.5 over a 5 K
error. The fan's flow is the total, and each box passes the flow allocated to its
zone, so the fan and coils carry exactly what the boxes deliver.

**Outdoor air.** The minimum follows the ASHRAE 62.1 ventilation rate procedure
[@ashrae621]:

$$
\begin{gathered}
V_{ou} = \sum_z V_{oz}, \qquad Z_{d,z} = \frac{V_{oz}}{V_{dz}}, \qquad X_s = \frac{V_{ou}}{V_{ps}},\\
E_v = \operatorname{clamp}\left(1 + X_s - \max_zZ_{d,z},\ 0.15,\ 1\right), \qquad
Y_s = \frac{V_{ou}/E_v}{V_{ps}},
\end{gathered}
$$ {#eq-vrp}

with $V_{oz}$ the zone requirement at the occupants $P_z(t)$ of this hour, no less than the minimum fraction. A fixed
`minimum_oa_flow` overrides it with a constant volume flow, converted to mass at the site's
standard density, as every outdoor-air flow is (E+ `StdRhoAir`).

**Comparison with EnergyPlus.** @eq-warmest and @eq-vrp follow
`SetpointManager:Warmest` and `Controller:MechanicalVentilation` with
`VentilationRateProcedure` [@eplus-engref-25-2, "Outdoor Air System"]. OpenBSE's
maximum temperature (15.6 °C) and maximum heating fraction (0.5) are not inputs;
E+ takes both from the setpoint manager and terminal
([#388](https://github.com/bbrannon4/OpenBSE/issues/388)). @eq-vrp reads the zone's
`outdoor_air`, the same requirement sizing uses, with its absolute and ACH terms and
combining method and every People object's schedule, as E+ reads
`DesignSpecification:OutdoorAir`. A zone without `outdoor_air` uses the `per_person_oa`
and `per_area_oa` of its connection. OpenBSE applies @eq-vrp to every multi-zone VAV
loop with an outdoor-air requirement; E+ only with a `Controller:MechanicalVentilation`.

## Economizers {#sec-economizer}

**Source:** the PSZ and VAV builders. Other system types have no economizer.

**Availability.** The test uses outdoor air before heat recovery:

: Economizer types. {#tbl-economizer}

| Type | Available when |
|---|---|
| `fixed_dry_bulb` | $T_o <$ limit (default 23.889 °C) |
| `differential_dry_bulb` (default) | $T_o < T_\text{ret}$ |
| `fixed_enthalpy` | $h_o <$ limit (default 65,200 J/kg) |
| `differential_enthalpy` | $h_o < h_\text{ret}$ |
| `enthalpy_with_high_limit` | $h_o < h_\text{ret}$ and $T_o <$ limit |

Optional fixed limits are AND-ed with these. The test does not run in heating.

**Damper.** When available, and with target $T_t$ (the cooling supply temperature
for PSZ, $T_s - \Delta T_f$ for VAV),

$$
x_\text{oa} = \begin{cases}
\operatorname{clamp}\left(\dfrac{T_\text{ret} - T_t}{T_\text{ret} - T_o},\ x_\text{min},\ 1\right) & T_o \le T_t,\\[2ex]
1 & T_o > T_t.
\end{cases}
$$ {#eq-economizer}

VAV holds its minimum if the resulting mixed air would still be below target
(`LockoutWithHeating`). With a `load_reset`, PSZ targets the supply temperature
that just meets its zone. `compressor_lockout` closes to the minimum when outdoor
air alone cannot meet the load (non-integrated). Mixed air then is
$(1 - x_\text{oa})(T_\text{ret} + \Delta T_{f,r}) + x_\text{oa}T_o$, with a return
fan's rise.

**Heat recovery.** Heat recovery (@sec-heat-recovery) treats the outdoor air before
it mixes, and its exhaust is the return air. It stops while the economizer is open.
With a supply setpoint, its outlet aims to land the mixed air on the economizer's
mixed-air target, the supply setpoint less the fan's rise at the current flow, which
is also what the coils aim at (E+ `OutdoorAirPretreat` with `SetpointManager:MixedAir`).

**Comparison with EnergyPlus.** The limits and @eq-economizer follow
`Controller:OutdoorAir` [@eplus-engref-25-2, "Outdoor Air System"]. The return
humidity uses the same flow weights as the return temperature, as E+'s return-air
mixer does.

## DOAS {#sec-doas}

**Source:** `build_doas_signals`.

A DOAS loop always runs, supplying 100% outdoor air at its fan's design flow. An
autosized fan takes the sum of the served zones' `outdoor_air` requirements, and a
DOAS loop with neither is a severe error. The zones share the air in proportion to
their requirements. The heating coil aims at the loop's `heating_supply_temp`, and
the cooling coil at its `cooling_supply_temp` (after any supply-temperature reset),
each less the rise of a supply fan downstream of the coils and each only if the
outdoor air needs it. Coil sizing uses the same flow and targets.

**Comparison with EnergyPlus.** An EnergyPlus DOAS takes its flow from the zones'
`DesignSpecification:OutdoorAir` and its temperatures from setpoint managers, as
here. The loop's supply-temperature controls have no DOAS-specific defaults, so a
DOAS should state both; otherwise it heats to the general 35 °C default.

## Other systems {#sec-other-systems}

- **Fan coils, PTAC, PTHP:** @sec-fcu-ptac.
- **CRAC and CRAH:** @sec-crac-crah.
- **Dual duct:** @sec-dual-duct-box. Each deck coil holds its deck temperature
  while its boxes draw air from it, and the boxes meet the predictor's zone loads.
  The loop has no economizer.

## Supply-temperature and plant resets {#sec-resets}

**Source:** `apply_sat_reset`, `apply_sat_reset_heating`, `apply_plant_reset`.

**Outdoor reset.** Between $T_{o,\text{low}}$ and $T_{o,\text{high}}$, with
$f = \operatorname{clamp}((T_o - T_{o,\text{low}})/(T_{o,\text{high}} - T_{o,\text{low}}),\ 0,\ 1)$,

$$
T_\text{sp} = T_\text{min} + (1 - f)(T_\text{max} - T_\text{min}),
$$ {#eq-oa-reset}

for cooling supply air, heating supply air and plant loops alike.

**Demand reset.** Once per timestep, a cooling supply temperature moves down by
`step` if the most loaded zone's part load is at least 0.95, and otherwise up, within
$[T_\text{min}, T_\text{max}]$; a heating supply temperature goes the opposite way.
The part load is how far the zone's box would open to meet the predictor's load at
the supply temperature the timestep started with,

$$
\text{PLR}_z = \frac{\dot Q_z}{\dot m_{d,z}c_p\max(|T_s - T_{\text{sp},z}|,\ 1)},
$$ {#eq-demand-reset-plr}

with the cooling load and setpoint for a cooling reset and the heating ones for a
heating reset.

**Comparison with EnergyPlus.** E+ has no stepping demand reset;
`SetpointManager:WarmestTemperatureFlow` reads the same box positions but sets the
supply temperature directly each timestep. The stepped reset is a common building
automation sequence (ASHRAE Guideline 36 trim and respond), here with one request
level and no ignore count.

**Plant.** A plant loop's reset reaches only the loop's return estimate
(@sec-plant-loop, [#251](https://github.com/bbrannon4/OpenBSE/issues/251)).

## Known limitations {#sec-controls-limitations}

- [#255](https://github.com/bbrannon4/OpenBSE/issues/255): no night cycle;
  unoccupied setpoints unused.
- [#388](https://github.com/bbrannon4/OpenBSE/issues/388): VAV Warmest maximum
  and no-box heating fraction are not inputs.
- [#389](https://github.com/bbrannon4/OpenBSE/issues/389): a multi-zone PSZ splits
  its flow evenly.
- [#199](https://github.com/bbrannon4/OpenBSE/issues/199): no daylight saving or
  start day of week.
