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
within a date range. The first entry that applies wins. Night setback through the
air-loop heating path is limited
([#117](https://github.com/bbrannon4/OpenBSE/issues/117)).

**Mode.** A zone's mode comes from the predictor (@sec-predictor) and is held for
the timestep. It is heating below the heating setpoint, cooling above the cooling
setpoint, and deadband between them. PSZ-AC adds a guard: heating with no heating
load but a cooling load becomes cooling, and the reverse. A zone over its maximum
relative humidity in deadband becomes cooling, with the coil aimed 0.5 K below the
zone to dehumidify.

**Schedules.** A named schedule gives a fraction for each hour, by day type (a
weekday, or each day individually, Saturday, Sunday, weekend or holiday, each
falling back to the next), from arrays or `"value until HH:MM"` strings. Optional
seasons override them by date range. Hour $h$ covers the hour ending at $h$.

**Comparison with EnergyPlus.** EnergyPlus's `ZoneControl:Thermostat` takes
setpoint schedules, which are its only mechanism; OpenBSE's thermostat schedules
are separate from its named schedules. Schedules have no design-day types
([#232](https://github.com/bbrannon4/OpenBSE/issues/232)), daylight saving or
start day of week ([#199](https://github.com/bbrannon4/OpenBSE/issues/199)).

## Availability {#sec-availability}

A loop whose `availability_schedule` is below 0.5 for the hour is off. Its
components report zero, and its zones float. There is no night cycle:
`unoccupied_heating_setpoint` and `unoccupied_cooling_setpoint` are read and never
used, and the code for them is dead
([#255](https://github.com/bbrannon4/OpenBSE/issues/255)). EnergyPlus's
`AvailabilityManager:NightCycle`, which the DOE prototypes use, restarts the
system when a zone leaves its setback range.

## Part-load cycling {#sec-plr}

**Source:** `main.rs` (`simulate_all_loops`).

PSZ-AC, PTAC and PTHP loops simulate their components at full flow, then find the
loop part-load ratio from the zone load, and scale the outputs by it. From the
supply state, with $T_\text{ref} = \min(T_h, T_z)$ in heating and
$\max(T_c, T_z)$ in cooling, the net capacity is
$\dot Q_\text{cap} = \dot mc_p|T_\text{sup} - T_\text{ref}|$. Then

$$
x = \begin{cases}
\dfrac{\dot Q_h - \dot Q_f}{\dot Q_\text{cap} - \dot Q_f} & \text{heating},\\[2ex]
\dfrac{\dot Q_c + \dot Q_f + \dot Q_\text{oa}}{\dot Q_\text{cap} + \dot Q_f + \dot Q_\text{oa}} & \text{cooling},
\end{cases}
$$ {#eq-plr}

clamped to $[x_\text{min}, 1]$. The terms are:

- $\dot Q_h$ and $\dot Q_c$, the zone's loads to setpoint, multiplied by its zone
  multiplier;
- $\dot Q_f$, the heat of a continuous fan, which arrives in the off-cycle too;
- $\dot Q_\text{oa} = x_\text{oa}\dot mc_p(T_o - T_z)$, the outdoor-air load in
  the off-cycle of a continuous fan, zero for a cycling fan;
- $x_\text{min}$, the minimum outdoor-air fraction for a cycling fan and 0 for a
  continuous one.

If the load is under 10 W, a proportional fallback applies:
$x = |T_\text{sp} - T_z|/|T_\text{sup} - T_\text{sp}|$. If the zone started the
timestep more than half the deadband beyond the setpoint on the far side, the
output is $x_\text{min}$.

**Scaling.** Coil and fan outputs and flows scale by $x$. DX compressors and
condenser fans, and the fan of a loop with DX, scale by
$\text{RTF} = x/\max(1 - C_d(1 - x),\ 0.5)$ (@sec-dx-cooling). Continuous fans stay
at full power. `ContinuousNoLoadOff` fans stop in deadband.

**Comparison with EnergyPlus.** EnergyPlus's `AirLoopHVAC:UnitarySystem` solves
its part-load ratio iteratively against the unit's output; @eq-plr is its closed
form. EnergyPlus has zero flow with a cycling fan and no load, where
$x_\text{min}$ keeps the fan on, and a small load is met without a floor
([#256](https://github.com/bbrannon4/OpenBSE/issues/256)). PSZ-AC serves several
zones evenly, controlled by the first
([#256](https://github.com/bbrannon4/OpenBSE/issues/256)).

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
T_s = \operatorname{clamp}\left(\min_z\left(T_z - \frac{\dot Q_{c,z}}{\dot m_{d,z}c_p}\right),\ T_{s,\text{min}},\ 15.6\right),
$$ {#eq-warmest}

over zones in cooling with over 100 W of load, and starting from 15.6 °C. With
`fixed_cooling_sat`, it is the fixed supply temperature. Each coil aims at $T_s$,
less the fan's rise if the fan is downstream, which scales with the square of the
flow fraction. A preheat coil can hold a fixed setpoint.

**Zone flows.** In cooling, each zone takes
$\dot Q_c/[c_p\max(T_z - T_s, 1)]$, between its minimum and design flow. Otherwise
it takes its minimum, or in heating a fraction rising to 0.5 over a 5 K error. The
fan's flow is the total
([#246](https://github.com/bbrannon4/OpenBSE/issues/246)).

**Outdoor air.** The minimum follows the ASHRAE 62.1 ventilation rate procedure
[@ashrae621]:

$$
\begin{gathered}
V_{ou} = \sum_z V_{oz}, \qquad Z_{d,z} = \frac{V_{oz}}{V_{dz}}, \qquad X_s = \frac{V_{ou}}{V_{ps}},\\
E_v = \operatorname{clamp}\left(1 + X_s - \max_zZ_{d,z},\ 0.15,\ 1\right), \qquad
Y_s = \frac{V_{ou}/E_v}{V_{ps}},
\end{gathered}
$$ {#eq-vrp}

with $V_{oz} = R_pP_z(t) + R_aA_z$, no less than the minimum fraction. A fixed
`min_oa_flow` overrides it with a constant flow.

**Comparison with EnergyPlus.** @eq-warmest and @eq-vrp follow
`SetpointManager:Warmest` and `Controller:MechanicalVentilation` with
`VentilationRateProcedure` [@eplus-engref-25-2, "Outdoor Air System"]. OpenBSE's
maximum temperature and maximum heating fraction are hard-coded, and a higher
minimum makes the clamp panic
([#256](https://github.com/bbrannon4/OpenBSE/issues/256)). @eq-vrp omits the
absolute and ACH terms ([#231](https://github.com/bbrannon4/OpenBSE/issues/231)).

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
With a supply setpoint, its outlet aims to land the mixed air on the target (E+
`OutdoorAirPretreat`).

**Comparison with EnergyPlus.** The limits and @eq-economizer follow
`Controller:OutdoorAir` [@eplus-engref-25-2, "Outdoor Air System"]. The return
humidity is a plain mean ([#184](https://github.com/bbrannon4/OpenBSE/issues/184)).

## DOAS {#sec-doas}

**Source:** `build_doas_signals`.

A DOAS loop supplies 100% outdoor air at 30% of the served zones' design flow, with
a 0.05 kg/s floor. Its heating coil aims at the highest heating setpoint plus 2 K,
and its cooling coil at the lowest cooling setpoint minus 2 K (at least 14 °C), each
only if the outdoor air needs it.

**Comparison with EnergyPlus.** An EnergyPlus DOAS takes its flow from the zones'
`DesignSpecification:OutdoorAir` and its temperatures from setpoint managers. In
OpenBSE, neither the zones' outdoor air nor the loop's supply temperatures are read
([#256](https://github.com/bbrannon4/OpenBSE/issues/256)).

## Other systems {#sec-other-systems}

- **Fan coils, PTAC, PTHP:** @sec-fcu-ptac.
- **CRAC and CRAH:** @sec-crac-crah.
- **Dual duct:** @sec-dual-duct-box. Its coils aim at the deck temperatures when
  any zone needs them. It has no economizer, and its mode comes from the in-iteration
  temperature rather than the predictor
  ([#245](https://github.com/bbrannon4/OpenBSE/issues/245)).

## Supply-temperature and plant resets {#sec-resets}

**Source:** `apply_sat_reset`, `apply_sat_reset_heating`, `apply_plant_reset`.

**Outdoor reset.** Between $T_{o,\text{low}}$ and $T_{o,\text{high}}$, with
$f = \operatorname{clamp}((T_o - T_{o,\text{low}})/(T_{o,\text{high}} - T_{o,\text{low}}),\ 0,\ 1)$,

$$
T_\text{sp} = T_\text{min} + (1 - f)(T_\text{max} - T_\text{min}),
$$ {#eq-oa-reset}

for cooling supply air, heating supply air and plant loops alike.

**Demand reset.** A cooling supply temperature moves down by `step` if the highest
zone "PLR" is at least 0.95, and otherwise up. Heating goes the opposite way. It
runs each HVAC iteration, and its PLR is the cooling load over five times the zone
air's capacitance rate
([#256](https://github.com/bbrannon4/OpenBSE/issues/256)).

**Plant.** A plant loop's reset reaches only the loop's return estimate
(@sec-plant-loop, [#251](https://github.com/bbrannon4/OpenBSE/issues/251)).

## Known limitations {#sec-controls-limitations}

- [#255](https://github.com/bbrannon4/OpenBSE/issues/255): no night cycle;
  unoccupied setpoints unused.
- [#256](https://github.com/bbrannon4/OpenBSE/issues/256): DOAS flow and
  temperatures; VAV hard-coded limits and clamp panic; demand reset per iteration;
  cycling runtime floor; multi-zone PSZ.
- [#117](https://github.com/bbrannon4/OpenBSE/issues/117): setback through the
  air-loop path.
- [#199](https://github.com/bbrannon4/OpenBSE/issues/199): no daylight saving or
  start day of week.
- [#184](https://github.com/bbrannon4/OpenBSE/issues/184): return humidity mean.
- [#245](https://github.com/bbrannon4/OpenBSE/issues/245),
  [#246](https://github.com/bbrannon4/OpenBSE/issues/246): terminal control.
