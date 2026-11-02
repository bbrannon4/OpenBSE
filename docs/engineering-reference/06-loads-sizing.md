# Loads and sizing {#sec-sizing}

Loads are what a zone needs to hold its setpoint. Sizing turns the
design-day loads into the capacities and flows of every input marked
`autosize`. Sizing runs once, before the annual simulation, and only when
the model has design days and an envelope. Its steps, in order, are:

1. design-day runs with ideal loads (@sec-dd-runs);
2. zone sizing: peak loads and design flows per zone (@sec-zone-sizing);
3. system sizing: coincident peaks (@sec-system-sizing);
4. air-side equipment: fans, coils, terminals and VRF units
   (@sec-air-sizing);
5. plant sizing: boilers, chillers, pumps, towers and heat exchangers
   (@sec-plant-sizing);
6. the outdoor-air fraction of each loop, recomputed from its sized fan
   (@sec-oa-after-sizing).

If any `autosize` value is still unresolved after these steps, the run
stops with a severe error (`autosize_check::unsized_autosize_errors`).

## Ideal loads air system {#sec-ideal-loads}

**Source:** `openbse-envelope/src/zone.rs` (`IdealLoadsAirSystem`).

An ideal-loads system is a heat source and sink with four inputs: heating
and cooling capacities [W], and heating and cooling setpoints [°C]. It
meets the zone's sensible load exactly, within its capacity. Its solve,
with the frozen mode and the backward-difference load, is described in @sec-zone-air-hb. A model can give zones ideal loads directly, as the
ASHRAE 140 cases do. Sizing also installs one temporarily in every
conditioned zone (@sec-dd-runs).

Ideal loads are zone equipment, as in EnergyPlus: a model can serve some zones
with them and others with air loops, VRF or radiant panels, and all of them run.
A zone can't take ideal loads and another system together; that is a severe
validation error.

## Design-day runs {#sec-dd-runs}

**Source:** `openbse-io/src/sizing.rs` (`run_single_design_day`,
`hourly_peaks_of_averaged_loads`).

Each design day is run on its own copy of the envelope, all in parallel.
The weather is described in @sec-design-days. A run has these steps:

1. **Ideal loads.** Every conditioned zone without an ideal-loads system of
   its own gets one with $10^6$ W of capacity. Both of its setpoints are
   set to the zone thermostat's heating setpoint on a heating design day,
   or its cooling setpoint on a cooling design day. A zone without a
   thermostat uses 21 °C or 24 °C.
2. **Initial state.** Zone temperatures are set to that setpoint, humidity
   ratios to 0.008 kg/kg, and the backward-difference history is reset to
   first order.
3. **Setpoint schedules.** Two hourly overrides can replace the constant
   setpoint. A design day's `cooling_setpoint_schedule` sets every zone's
   cooling setpoint. A thermostat's `design_day_heating_setpoints` or
   `design_day_cooling_setpoints` set its own zones' setpoints. These
   capture setback recovery, which E+ prototypes size on through their
   SummerDesignDay and WinterDesignDay setpoint schedules.
4. **Repetition.** The 24-hour sequence is repeated 5 times at the
   simulation timestep. Each timestep calls the envelope solve once and
   then advances the backward-difference history once (@sec-zone-air-hb).
   Loads are recorded only on the last day.
5. **Outdoor air.** Zones whose air loop carries outdoor air leave it out
   of their balance, so their loads exclude it (@sec-infiltration).

**Internal gains.** The design day's `internal_gains` mode decides the
gains:

: Internal-gain modes during sizing (`SizingInternalGains`). {#tbl-sizing-gains}

| Mode | Gains | Default for |
|---|---|---|
| `off` | none | heating design days |
| `full` | design level at every hour, schedules ignored | cooling design days |
| `scheduled` | the normal schedules | |
| `full_when_occupied` | design level in hours a gain's schedule is on, else none | |

**Averaging and peaks.** Let $Q_k$ be a zone's heating or cooling load at
timestep $k$ of the last day, with $n$ timesteps per day. Its moving
average over a window of $N$ timesteps (`sizing_averaging_window`) wraps
round the day:

$$
\bar Q_k = \frac1N \sum_{j=0}^{N-1} Q_{(k - j) \bmod n}.
$$ {#eq-sizing-avg}

Each hour's load is the largest $\bar Q_k$ among its timesteps. The default
window is $N = 1$, the single-timestep peak.

**Comparison with EnergyPlus.** EnergyPlus sizes zones the same way: an
ideal sensible load holds each zone at its sizing setpoint, and the loads
are averaged with `General::MovingAvg` before the peak is taken
[@eplus-engref-25-2, "Zone Design Loads and Air Flow Rates"]. The
differences are:

- **Warmup.** EnergyPlus repeats a design day until temperatures and loads
  converge (up to 25 days by default), not a fixed 5
  ([#232](https://github.com/bbrannon4/OpenBSE/issues/232)).
- **Weather.** EnergyPlus evaluates the design-day weather at each
  timestep. OpenBSE reuses the hour's values for every sub-hourly step
  ([#232](https://github.com/bbrannon4/OpenBSE/issues/232)).
- **Averaging window.** EnergyPlus defaults to one hour of timesteps;
  OpenBSE defaults to one timestep
  ([#232](https://github.com/bbrannon4/OpenBSE/issues/232)).
- **Gains.** EnergyPlus has no gains mode. It evaluates each schedule's
  SummerDesignDay or WinterDesignDay day schedule. OpenBSE schedules have
  no design-day day types, so the modes stand in for them. The
  `SizingInternalGains::Full` comment wrongly calls `full` the EnergyPlus
  default ([#232](https://github.com/bbrannon4/OpenBSE/issues/232)).
- **Humidity.** The outdoor humidity is a fixed relative humidity
  ([#198](https://github.com/bbrannon4/OpenBSE/issues/198)).
- **Day type.** A design day is classed as heating or cooling by a
  substring of its `day_type` ("heat" or "winter", "cool" or "summer"). A
  day type that matches neither is skipped without warning
  ([#232](https://github.com/bbrannon4/OpenBSE/issues/232)).

## Generated monthly cooling design days {#sec-dd-monthly}

**Source:** `sizing.rs` (`generate_monthly_cooling_dds`,
`monthly_peak_dry_bulb`).

With `auto_cooling_design_days` (on by default), a cooling design day is
added for every month that has none. These days catch a peak in a shoulder
month, when the sun is lower and enters further through the glazing. The
anchor is the user's cooling design day with the highest design
temperature $T_{\text{dd},a}$, in month $a$. For each month $m$, let $P_m$
be the 99.6th percentile of that month's dry-bulb temperatures in the
weather file. Then

$$
T_{\text{dd},m} = T_{\text{dd},a} + (P_m - P_a),
$$ {#eq-dd-monthly}

and the day is skipped if $T_{\text{dd},m} < T_{\text{dd},a} - 20$ K. The
generated day keeps the anchor's daily range, pressure, wind, gains mode,
optical depths and setpoint schedule. It uses a representative day of
month $m$ for the sun position, and $h_a + 0.5(P_m - P_a)$ for the humidity
value.

**Comparison with EnergyPlus.** EnergyPlus sizes on the given design days
only. This feature follows other tools' practice (the code cites IES-VE).
Setting `auto_cooling_design_days: false` gives EnergyPlus's behaviour. The
humidity shift adds kelvins whatever `humidity_type` is. It has no effect
today, because the humidity value is not read
([#198](https://github.com/bbrannon4/OpenBSE/issues/198),
[#232](https://github.com/bbrannon4/OpenBSE/issues/232)).

## Zone sizing {#sec-zone-sizing}

**Source:** `sizing.rs` (`run_zone_sizing`, `run_sizing`).

**Peak loads.** A zone's design heating load $Q_h$ is the largest hourly
heating load over all heating design days, and $Q_c$ the largest cooling
load over all cooling design days. The design day and hour of each are
recorded.

**Zone multipliers.** A zone with multiplier $N_z$ stands for $N_z$ identical
zones. OpenBSE simulates one instance with one instance of its air loop. All
sizing is therefore per instance, including the loads, flows and capacities in
the sizing report, and capacities given as inputs. The multiplier enters once:
HVAC energy, plant demand and building totals are multiplied by $N_z$, so a model
with every multiplier at 2 uses exactly twice the energy. EnergyPlus instead sizes
and runs the system for all instances and divides what reaches each zone by
$N_z$. Both give the same energy for a loop whose zones share a multiplier. A loop
or VRF system serving zones with different multipliers can't be represented here
and is a severe error ([#280](https://github.com/bbrannon4/OpenBSE/issues/280)).

**Design flows.** The design flows [kg/s] are

$$
\begin{gathered}
\dot m_h = \max\left(\frac{Q_h}{c_p\max(T_{s,h} - T_{z,h},\ 5)},\ \dot m_\text{oa}\right),
\qquad
\dot m_c = \max\left(\frac{Q_c}{c_p\max(T_{z,c} - T_{s,c} - \Delta T_\text{fan},\ 3)},\ \dot m_\text{oa}\right),\\
\dot m_\text{des} = \max(\dot m_h,\ \dot m_c,\ 0.01),
\end{gathered}
$$ {#eq-zone-design-flow}

where:

- $c_p = 1005$ J/(kg·K);
- $T_{z,h}$ and $T_{z,c}$ are the zone's heating and cooling setpoints;
- $T_{s,h}$ is the highest heating supply temperature of all air loops, and
  $T_{s,c}$ the lowest cooling supply temperature (35 °C and 13 °C with no
  air loops);
- $\Delta T_\text{fan}$ is `sizing_fan_delta_t` (default 0), the supply fan's
  temperature rise;
- $\dot m_\text{oa}$ is the zone's minimum outdoor-air flow, a floor on both
  flows:

$$
\dot m_\text{oa} = \rho_\text{std}\,\dot V_{\text{oa},z},
\qquad
\rho_\text{std} = \frac{p_\text{dd}}{287.042 \times 293.15},
$$ {#eq-zone-oa-floor}

with $\dot V_{\text{oa},z}$ from @eq-zone-oa at design occupancy, per zone
instance, and $p_\text{dd}$ the first design day's pressure. The outdoor-air load itself is not part of $Q_h$ or $Q_c$;
it belongs to the central coil.

**Cooling flow sequence.** For each cooling design day and hour, the zone's
cooling flow is computed in the same way from that hour's load. This
sequence (E+ `CoolFlowSeq`) gives the coincident flow of multizone systems
(@sec-system-sizing).

**Per-loop supply temperature.** @eq-zone-design-flow uses the coldest
cooling supply temperature of all loops. For a loop whose own cooling
supply temperature differs by more than 0.5 K, each zone's flow is
recomputed as $f_cQ_c/(c_p\max(T_{z,c} - T_{s,c,\text{loop}}, 5))$, with $f_c$
the cooling sizing factor. The recomputed flow replaces the old one only if
it is larger.

**Comparison with EnergyPlus.** The method follows E+ `Sizing:Zone` with the
`DesignDay` flow method: flow from the peak load and the supply-to-zone
temperature difference, with the minimum outdoor air as a floor
[@eplus-engref-25-2, "Zone Design Loads and Air Flow Rates"]. The
differences are:

- **Sizing factors.** EnergyPlus multiplies the zone's design load and flow
  by the zone sizing factor; OpenBSE does not (see @sec-air-sizing,
  [#49](https://github.com/bbrannon4/OpenBSE/issues/49)).
- **Supply temperatures.** EnergyPlus takes them per zone from `Sizing:Zone`;
  OpenBSE uses the extremes over all loops. The per-loop correction has a
  different $\Delta T$ floor and no fan rise or outdoor-air floor
  ([#229](https://github.com/bbrannon4/OpenBSE/issues/229)).
- **Density.** EnergyPlus uses the standard density at the site's elevation
  pressure; OpenBSE uses the first design day's pressure. These agree when
  the design day gives the site pressure.

## System sizing {#sec-system-sizing}

**Source:** `sizing.rs` (`run_system_sizing`); `main.rs`
(`multizone_design_flow`).

**Coincident peaks.** Every design day is run again (@sec-dd-runs). The
coincident peak heating and cooling loads are the largest sums over all
zones at one hour. They size primary pumps (@sec-plant-sizing) and are
written to `_system_sizing.csv`. The second run uses the same inputs as the
first, so it repeats it exactly
([#232](https://github.com/bbrannon4/OpenBSE/issues/232)).

**Multizone flow.** A VAV or dual-duct loop's design supply flow follows the
loop's `zone_sum`:

$$
\dot m_\text{sys} =
\begin{cases}
\sum_z \dot m_{\text{des},z} & \text{non-coincident},\\
\max\left(f_c\max_{d,h}\sum_z \dot m_{c,z}(d,h),\ f_h\sum_z \dot m_{h,z}\right) & \text{coincident},
\end{cases}
$$ {#eq-multizone-flow}

where $\dot m_{c,z}(d,h)$ is the cooling flow sequence, and $f_h$ and $f_c$
are the heating and cooling sizing factors.

**Comparison with EnergyPlus.** @eq-multizone-flow matches E+ `Sizing:System`
with non-coincident or coincident zone sums
[@eplus-engref-25-2, "System Design Loads and Air Flow Rates"]. EnergyPlus
also computes the system's coil loads at its own peak, from the mixed-air
state. OpenBSE has no system-level coil load (@sec-air-sizing).

## Air-side equipment {#sec-air-sizing}

**Source:** `main.rs` ("Design Day Sizing Run": `loop_comp_sizing`,
`terminal_reheat_capacity`, the dual-duct and VRF blocks).

**Sizing factors.** The heating and cooling sizing factors $f_h$ and $f_c$
default to 1.25 and 1.15 (`heating_sizing_factor`,
`cooling_sizing_factor`). They multiply capacities and some flows, as the
table shows. The zone design flows of VAV and dual-duct zones are
multiplied by $f_c$ before anything else uses them.

**Fans and coils.** Each autosized fan, coil and GSHP in an air loop's
equipment list is sized from its loop. Below, $\Sigma Q$ is the sum of the
served zones' peak loads.

: Air-loop sizing by system type. $\dot m_\text{des}$ is the zone design flow (with $f_c$ for VAV and dual-duct zones). {#tbl-air-sizing}

| System | Fan flow | Heating capacity | Cooling capacity |
|---|---|---|---|
| PSZ-AC | $\sum\dot m_\text{des}$ | $f_h\Sigma Q_h$ | $f_c\Sigma Q_c$ |
| VAV, dual-duct | @eq-multizone-flow | $f_h\Sigma Q_h$ | $f_c\Sigma Q_c$ |
| CRAC, CRAH | $\sum\dot m_\text{des}$ | none | $f_c\Sigma Q_c$ |
| FCU, PTAC, PTHP | $\sum\dot m_\text{des}$ | $\max(f_h\Sigma Q_h,\ Q_{m,h})$ | $\max(f_c\Sigma Q_c,\ Q_{m,c})$ |
| DOAS | $0.3\sum\dot m_\text{des}$ | $f_h\dot m c_p(20 - T_{o,h})$ | $f_c\dot m c_p(T_{o,c} - 18)$ |

Here:

- the fan flow is converted to volume with $\rho_\text{std}$;
- for FCU, PTAC and PTHP, $\dot m$ is the fan flow and $T_{s,h}$, $T_{s,c}$
  the loop's supply temperatures. The mixed-air coil loads are
  $Q_{m,h} = \dot m c_p(T_{s,h} - T_{m,h})$ and
  $Q_{m,c} = \dot m c_p(T_{m,c} - T_{s,c})$, with the mixed-air temperature
  $T_m = (1 - x_\text{oa})T_z + x_\text{oa}T_o$ and $T_z$ fixed at 21 °C for
  heating and 24 °C for cooling;
- for DOAS, $\dot m$ is the outdoor-air flow, $0.3\sum\dot m_\text{des}$;
- $T_{o,h}$ is the coldest heating design temperature (−20 °C if there is
  none), and $T_{o,c}$ the hottest cooling design temperature (35 °C if
  there is none).

**Terminals.** A VAV or fan-powered box's maximum flow is its zone's design
flow. Its reheat capacity covers the zone heating load plus reheating its
primary air at the reheat flow $\dot m_r$ from the cooling supply
temperature to the zone:

$$
Q_\text{reheat} = f_hQ_h + c_p\,\dot m_r\,(T_{z,h} - T_{s,c}),
$$ {#eq-reheat-size}

with $c_p = 1006$ J/(kg·K). The reheat flow $\dot m_r$ is
$\max(f_\text{reheat}, f_\text{min})$ times the maximum flow for a VAV box,
and the minimum primary flow for a fan-powered box. A dual-duct box's
design flow is $f_c\dot m_\text{des}$.

**VRF.** An autosized indoor unit's capacities are $f_cQ_c$ and $f_hQ_h$ of
its zone, at least 500 W, or 3000 W if the zone has no sizing result. Its
airflow is $5\times10^{-5}$ m³/s per watt of the larger capacity. An
autosized outdoor unit takes the sum of its indoor units' capacities.

**Comparison with EnergyPlus.** EnergyPlus sizes a central coil from the
system design flow and the enthalpy difference between the mixed air and
the supply air at the system peak, plus the fan heat (`CoolingCapacitySizer`,
`HeatingCapacitySizer`). The result is a total capacity. OpenBSE sums the
zone sensible peaks. Its central coils therefore leave out the outdoor-air,
latent and fan loads, although the DX and chilled-water capacities they set
are total capacities
([#229](https://github.com/bbrannon4/OpenBSE/issues/229)). Other
differences:

- **FCU and DOAS.** The hard-coded zone and supply temperatures and the DOAS
  outdoor-air fraction of 0.3 are not in EnergyPlus
  ([#229](https://github.com/bbrannon4/OpenBSE/issues/229)).
- **Sizing factors.** Where the factors are applied differs from
  EnergyPlus, whose defaults are 1.0
  ([#49](https://github.com/bbrannon4/OpenBSE/issues/49)).
- **VRF airflow.** EnergyPlus sizes VRF terminal airflow from the zone
  design flow ([#232](https://github.com/bbrannon4/OpenBSE/issues/232)).
- **Reheat.** EnergyPlus sizes a reheat coil from the zone's design heating
  flow, between the coil inlet temperature and the zone's design heating
  supply temperature. @eq-reheat-size is the same balance written with the
  zone load. Return-air lighting heat is not yet part of coil sizing
  ([#185](https://github.com/bbrannon4/OpenBSE/issues/185)).

## Plant sizing {#sec-plant-sizing}

**Source:** `openbse-io/src/plant.rs` (`size_plant`, `LoopSizer`);
`main.rs` (`plant_sizing_inputs`, `apply_plant_sizes`).

Plant sizing runs after the air side, from the sized capacities of the
demand components. These are water coils, hot-water reheat terminals and
radiant panels, each multiplied by its zone multiplier, as the demand it
places on the shared loop. A loop's design
load is

$$
Q_\text{loop} = \sum_\text{demand}|Q_i| + \sum_{\text{HX drawing on it}} Q_{\text{loop}'} + \sum_{\text{chillers rejecting to it}} Q_\text{ch}\left(1 + \frac{1}{\text{COP}}\right),
$$ {#eq-plant-load}

solved recursively over loops linked by heat exchangers and condensers. A
cycle is a severe error.

**Equipment.** Every autosized boiler and chiller on a loop gets
$Q_\text{loop}$. Water flows come from

$$
\dot V = \frac{Q}{\rho_w c_{p,w}\,\max(\Delta T_\text{loop}, 1)},

$$ {#eq-plant-flow}

with $\rho_w$ and $c_{p,w}$ at the loop's conversion temperature (@sec-water-properties),
where $Q$ is the following basis:

- a boiler's flow, a cooling tower's flow and a heat exchanger's flow use
  $Q_\text{loop}$;
- a pump on a condenser loop uses $Q_\text{loop}$;
- a pump on a heating-only or cooling-only loop uses the coincident zone
  peak of @sec-system-sizing;
- a pump on a loop with both boilers and chillers uses the sum of their
  capacities.

A pump falls back to $Q_\text{loop}$ when its basis is zero. A loop that
nothing draws on sizes its equipment to zero. A loop that has demand but
nothing to size from is a severe error.

**Comparison with EnergyPlus.** @eq-plant-load is the E+ `Sizing:Plant`
non-coincident load, with condenser loads at $Q(1 + 1/\text{COP})$
[@eplus-engref-25-2, "Plant and Condenser Loop Sizing"]. EnergyPlus sizes pumps from the
loop's design flow. OpenBSE's coincident zone peak excludes the outdoor-air
and reheat loads and the sizing factors, so on a heating-only or
cooling-only loop the pump flow is below its coils' design flows
([#230](https://github.com/bbrannon4/OpenBSE/issues/230)).

## Outdoor-air fraction after sizing {#sec-oa-after-sizing}

**Source:** `main.rs` ("Recompute OA fractions after sizing");
`openbse-io/src/input.rs` (`compute_oa_fraction`).

When a loop is built, its minimum outdoor-air fraction is computed from the zone
outdoor-air flows (@eq-zone-oa), summed or maximized as each zone specifies. With
an autosized fan, the fraction falls back to 0.2 until sizing is done.
After sizing, unless `minimum_damper_position` is given, the fraction
becomes

$$
x_\text{oa} = \min\left(1,\ \frac{\sum_z \dot V_{\text{oa},z}}{\dot V_\text{fan}}\right),
$$ {#eq-oa-fraction-sized}

with $\dot V_{\text{oa},z}$ from @eq-zone-oa at design occupancy, summing all of the
zone's People objects. This is the same function the zone's outdoor-air floor
and the ideal-loads outdoor air use, like EnergyPlus's single
`DesignSpecification:OutdoorAir` calculation. The fan flow and the outdoor-air
flows are both per zone instance, so no multiplier is needed in the ratio.

## Known limitations {#sec-sizing-limitations}

- [#49](https://github.com/bbrannon4/OpenBSE/issues/49): sizing factors
  applied to capacities and some flows, not to zone loads; defaults
  1.25/1.15 rather than 1.0.
- [#185](https://github.com/bbrannon4/OpenBSE/issues/185): return-air
  lighting heat should size the coil, not the zone flow.
- [#198](https://github.com/bbrannon4/OpenBSE/issues/198),
  [#50](https://github.com/bbrannon4/OpenBSE/issues/50): design-day
  humidity, temperature profile and sky model.
- [#229](https://github.com/bbrannon4/OpenBSE/issues/229): central coils
  sized from summed zone sensible peaks, without outdoor-air, latent or fan
  loads; hard-coded temperatures in FCU and DOAS sizing.
- [#230](https://github.com/bbrannon4/OpenBSE/issues/230): pumps sized from
  the coincident zone peak, not the loop design flow.
- [#280](https://github.com/bbrannon4/OpenBSE/issues/280): a loop can't serve
  zones with different multipliers.
- [#232](https://github.com/bbrannon4/OpenBSE/issues/232): duplicate system
  run, fixed warmup, hourly weather, averaging default, generated design
  days, gains-mode comment, day-type matching, VRF airflow.
