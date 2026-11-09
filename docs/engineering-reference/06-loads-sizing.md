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
4. **Repetition.** The 24-hour sequence is repeated until it converges (at
   least 5 days, at most 25; see the comparison below) at the
   simulation timestep. Each timestep calls the envelope solve once and
   then advances the backward-difference history once (@sec-zone-air-hb).
   Loads are taken only from the last day.
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
window is one hour, $N$ = `timesteps_per_hour`, as EnergyPlus; $N = 1$ takes the
single-timestep peak.

**Comparison with EnergyPlus.** EnergyPlus sizes zones the same way: an
ideal sensible load holds each zone at its sizing setpoint, and the loads
are averaged with `General::MovingAvg` before the peak is taken
[@eplus-engref-25-2, "Zone Design Loads and Air Flow Rates"]. The
differences are:

- **Warmup.** OpenBSE repeats a design day until every zone's daily
  temperature extremes are within 0.4 K and its peak loads within 4% of the
  day before (loads below 100 W count as 100 W), up to 25 days, with the
  EnergyPlus test and defaults (`CheckWarmupConvergence`). It repeats at least
  5 days; the EnergyPlus default minimum is 1.
- **Gains.** EnergyPlus has no gains mode. It evaluates each schedule's
  SummerDesignDay or WinterDesignDay day schedule. OpenBSE schedules have
  no design-day day types, so the modes stand in for them.
- **Humidity.** The outdoor humidity is a fixed relative humidity
  ([#198](https://github.com/bbrannon4/OpenBSE/issues/198)).
- **Day type.** A design day is classed as heating or cooling by a
  substring of its `day_type` ("heat" or "winter", "cool" or "summer"). A
  day type that says neither or both is a severe error.

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
month $m$ for the sun position. A wet-bulb or dew-point humidity value becomes
$h_a + 0.5(P_m - P_a)$, and any other type of humidity value is kept.

**Comparison with EnergyPlus.** EnergyPlus sizes on the given design days
only. This feature follows other tools' practice (the code cites IES-VE).
Setting `auto_cooling_design_days: false` gives EnergyPlus's behaviour. $P_m$ is
a percentile of the month's own hours, used only for the difference between months,
not an annual design condition.

## Zone sizing {#sec-zone-sizing}

**Source:** `sizing.rs` (`run_zone_sizing`, `run_sizing`).

**Design loads.** A zone's design heating load $Q_h$ is the largest hourly
heating load over all heating design days times the heating sizing factor
$f_h$ (`heating_sizing_factor`). Its design cooling load $Q_c$ is the largest
cooling load over all cooling design days times the cooling sizing factor $f_c$
(`cooling_sizing_factor`). The design day and hour of each are recorded. The
factors enter here and only here: flows and capacities sized from these loads
inherit the margin once.

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
- $T_{s,h}$ and $T_{s,c}$ are the heating and cooling supply temperatures of
  the zone's first loop that is not a DOAS, or of its DOAS if it has no other
  (35 °C and 13 °C with no air loop);
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
cooling flow is computed in the same way from that hour's load times $f_c$. This
sequence (E+ `CoolFlowSeq`) gives the coincident flow of multizone systems
(@sec-system-sizing).

**Comparison with EnergyPlus.** The method follows E+ `Sizing:Zone` with the
`DesignDay` flow method: flow from the peak load and the supply-to-zone
temperature difference, with the minimum outdoor air as a floor
[@eplus-engref-25-2, "Zone Design Loads and Air Flow Rates"]. The
differences are:

- **Sizing factors.** Applied as in EnergyPlus, to the zone's design load,
  flow and flow sequence before the outdoor-air floor
  (`updateZoneSizingEndZoneSizingCalc7`). The defaults differ on purpose:
  OpenBSE uses $f_h = 1.25$ and $f_c = 1.15$, the ASHRAE 90.1 Appendix G
  oversizing ratios, because an autosized model without stated margins should
  have the margin designers normally select. EnergyPlus's `Sizing:Parameters`
  defaults are 1.0; set both factors to 1 to match them.
- **Density.** EnergyPlus uses the standard density at the site's elevation
  pressure; OpenBSE uses the first design day's pressure. These agree when
  the design day gives the site pressure.

## System sizing {#sec-system-sizing}

**Source:** `sizing.rs` (`coincident_peaks`); `main.rs`
(`multizone_design_flow`).

**Coincident peaks.** From the zone-sizing runs (@sec-dd-runs), the
coincident peak heating and cooling loads are the largest sums over all
zone instances at one hour, before any sizing factor. They are written to
`_system_sizing.csv` as a report.

**Multizone flow.** A VAV or dual-duct loop's design supply flow follows the
loop's `zone_sum`:

$$
\dot m_\text{sys} =
\begin{cases}
\sum_z \dot m_{\text{des},z} & \text{non-coincident},\\
\max\left(\max_{d,h}\sum_z \dot m_{c,z}(d,h),\ \sum_z \dot m_{h,z}\right) & \text{coincident},
\end{cases}
$$ {#eq-multizone-flow}

where $\dot m_{c,z}(d,h)$ is the cooling flow sequence. All of these flows
carry the sizing factors already.

**Comparison with EnergyPlus.** @eq-multizone-flow matches E+ `Sizing:System`
with non-coincident or coincident zone sums
[@eplus-engref-25-2, "System Design Loads and Air Flow Rates"]. The coil loads at the system's peak are in @sec-air-sizing.

## Air-side equipment {#sec-air-sizing}

**Source:** `main.rs` (`size_air_loop`, `sizing_outdoor_air`,
`terminal_reheat_capacity`, the dual-duct and VRF blocks);
`openbse-io/src/system_sizing.rs`.

**Fan flow.** Each autosized fan in an air loop's equipment list takes the
loop's flow, converted to volume with $\rho_\text{std}$; a hard-sized fan's flow
is used for its coils. The flow is @eq-multizone-flow for VAV and dual-duct
loops, $\sum\dot m_\text{des}$ for PSZ-AC, FCU, PTAC, PTHP, CRAC and CRAH, and
the served zones' summed outdoor-air requirement $\sum\dot m_{oa}$ for a DOAS
(E+ `Sizing:System` with a `VentilationRequirement` load).

**Outdoor air.** The coil's outdoor-air fraction $x$ is the loop's stated damper
position, or else the zones' outdoor-air requirement over the system flow; a loop
whose zones state no outdoor air takes none. FCU, CRAC and CRAH recirculate ($x = 0$), a DOAS
takes only outdoor air ($x = 1$), and a loop whose zones take their outdoor air
directly has $x = 0$. Economizers and heat recovery get no credit.

A VAV loop draws its zones' requirement through the ventilation rate procedure
(@eq-vrp), so it is sized on the corrected flow: with every People object at its
full count,

$$
V_{ot,d} = \max\left(V_{ot}(\dot m_{\text{des},z}),\ V_{ot}(f_\text{min}\,\dot m_{\text{des},z})\right),
$$ {#eq-vrp-sizing}

$V_{ot} = V_{ou}/E_v$ at the zones' design flows (cooling) and at their minimum
flows, the loop's `min_vav_fraction` of design (heating), and never less than the
uncorrected requirement. The coils are then sized for the outdoor air the controls
take at turndown, when VRP raises the outdoor-air fraction, so a preheat coil can
hold the supply temperature on design nights.

**Mixed air.** The inlet state mixes return air, at the flow-weighted zone
temperature and humidity, with outdoor air: $T_m = (1 - x)T_r + xT_o$, and the same
for $w$. Zone humidity comes from a quasi-steady balance with supply air at $w_s$:

$$
w_z = \frac{\dot m\,w_s + \dot m_i\,w_o + q_l/h_{fg}}{\dot m + \dot m_i},
$$ {#eq-sizing-zone-w}

where $\dot m_i$ is the outdoor air entering the zone directly and $q_l$ the latent
gains, because the ideal loads of the sizing run remove no moisture.

**Cooling coils.** With `zone_sum: coincident` on a multizone loop, the state is
that of the hour with the largest $\dot m_h c_p(T_m - T_{s,c})$ at that hour's flow
$\dot m_h$. Otherwise it is the design-day hour with the largest total coil load at the
design flow. The total load is

$$
Q_{cc} = \dot m\left[h(T_m, w_m) - h(\min(T_m, T_{s,c}),\ \min(w_m, w_s))\right]
+ \dot m c_p\left[\Delta T_\text{sf} + (1 - x)\Delta T_\text{rf}\right],
$$ {#eq-cooling-coil-load}

where $w_s$ is `cooling_supply_humidity_ratio` (default 0.008) and the last term is
the heat of the supply and return fans. A DX coil's rated capacity is
$Q_{cc}$ over its capacity-temperature modifier at the inlet wet-bulb (with the rise
of a blow-through fan) and the outdoor temperature, then held between $\dot V/6.041\times10^{-5}$
and $\dot V/4.027\times10^{-5}$ W.

**Heating coils.** The load is the largest over the heating design-day hours of

$$
Q_{hc} = \dot m_c c_p (T_\text{out} - T_m),
$$ {#eq-heating-coil-load}

with no fan heat. For most loops $\dot m_c$ is the fan flow and $T_\text{out}$ the
heating supply temperature. A VAV loop's central coil tempers the zones' summed
heating flow to the temperature its controls give it on a heating design day, its
boxes reheating from there: `preheat_setpoint`, else the fixed supply temperature,
else the Warmest reset's 15.6 °C maximum (no zone cools), less the rise of a supply
fan downstream of the coil. E+ sizes it to the Central Heating Design Supply Air
Temperature of `Sizing:System`, an input; here the controls set it. A dual-duct hot deck carries the summed heating flow to the
heating supply temperature, at the system's outdoor-air fraction. CRAC and CRAH have
none. A DOAS heats and cools its outdoor air to the loop's supply temperatures, less
the rise of a supply fan downstream of the coils, the targets its controls run at.

**Terminals.** A VAV or fan-powered box's maximum flow is its zone's design
flow. Its reheat capacity covers the zone heating load plus reheating its
primary air at the reheat flow $\dot m_r$ from the cooling supply
temperature to the zone:

$$
Q_\text{reheat} = Q_h + c_p\,\dot m_r\,(T_{z,h} - T_{s,c}),
$$ {#eq-reheat-size}

with $c_p = 1006$ J/(kg·K). The reheat flow $\dot m_r$ is
$\max(f_\text{reheat}, f_\text{min})$ times the maximum flow for a VAV box,
and the minimum primary flow for a fan-powered box. A dual-duct box's
design flow is $\dot m_\text{des}$.

**VRF.** An autosized indoor unit's capacities are $Q_c$ and $Q_h$ of
its zone, at least 500 W. A unit with no sizing result, such as one in a
model without design days, is a severe error. An autosized airflow is
the zone's design flow (@sec-zone-sizing) at the site's standard density, with
the zone's design flows taken at the indoor unit's supply temperatures unless
an air loop other than a DOAS also serves the zone. An autosized outdoor unit
takes the sum of its indoor units' capacities.

**Comparison with EnergyPlus.** @eq-cooling-coil-load, the DX modifier and range,
and @eq-heating-coil-load follow `CoolingCapacitySizer` and `HeatingCapacitySizer`,
with `SimAirServingZones` peak states. Differences:

- **States are hourly.** E+ reads the state at the peak timestep.
- **Non-coincident peak.** E+ blends each zone's return and outdoor state at that
  zone's own peak time. With a flat zone load that peak can fall at night, and a coil
  taking mostly outdoor air is then sized on mild night air: a Hospital ER coil came
  to 7.9 kW against an 18 kW zone load. OpenBSE takes the coil's own worst hour, at
  or above the E+ size.
- **Ventilation rate procedure.** @eq-vrp-sizing follows E+ `Sizing:System` with
  VentilationRateProcedure (`ManageSystemVentilationAdjustments`), which takes the
  larger of the cooling and heating Vot; E+ uses each zone's minimum heating flow,
  OpenBSE the loop's minimum fraction of each zone's design flow.
- **Dual duct.** OpenBSE's hot deck mixes at the system's fraction, because one fan
  draws both decks.
- **VRF airflow.** EnergyPlus sizes a VRF terminal unit's cooling and heating
  supply flows separately from the zone's design cooling and heating flows;
  OpenBSE's indoor unit has one rated airflow, the larger of the two.
- **Reheat.** EnergyPlus sizes a reheat coil from the zone's design heating
  flow, between the coil inlet temperature and the zone's design heating
  supply temperature. @eq-reheat-size is the same balance written with the
  zone load.
- **Return-air lighting heat.** As in E+ `CalcZoneLeavingConditions` and
  `ZoneRetTempAtCoolPeak`, on a loop whose return carries the heat, the
  `return_air_fraction` share of lights is no zone load during sizing. It raises the
  coil's return temperature by $q/(\dot m c_p)$, up to 60 °C.
- **Hot-aisle bypass.** A data-centre zone's captured server exhaust
  (@sec-hot-aisle) is treated the same way during sizing. E+'s ITE object moves
  heat to the return through its return approach temperature instead.

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

- a boiler's flow, a pump's flow, a cooling tower's flow and a heat
  exchanger's flow use $Q_\text{loop}$;
- a pump on a loop with no sized demand uses the sum of the capacities of the
  loop's boilers and chillers.

A loop that
nothing draws on sizes its equipment to zero. A loop that has demand but
nothing to size from is a severe error.

**Comparison with EnergyPlus.** @eq-plant-load is the E+ `Sizing:Plant`
non-coincident load, with condenser loads at $Q(1 + 1/\text{COP})$
[@eplus-engref-25-2, "Plant and Condenser Loop Sizing"]. Pumps take the loop's
design flow, the sum of the demand components' design flows, as in `Pumps::SizePump`.

## Outdoor-air fraction after sizing {#sec-oa-after-sizing}

**Source:** `main.rs` (`build_loop_infos`, "Outdoor-air fractions after sizing").

Each served zone's requirement is its `outdoor_air` (@eq-zone-oa), summed or
maximized as the zone specifies, or else its connection's `per_person_oa` and
`per_area_oa`. Unless `minimum_damper_position` is given, a loop's minimum
outdoor-air fraction is

$$
x_\text{oa} = \min\left(1,\ \frac{\sum_z \dot V_{\text{oa},z}}{\dot V_\text{fan}}\right),
$$ {#eq-oa-fraction-sized}

at build time for a hard-sized fan and after sizing for an autosized one, and 0 when
no served zone states a requirement, as an EnergyPlus air loop without an
`AirLoopHVAC:OutdoorAirSystem` takes none. Sizing reads the requirement directly
(@sec-air-sizing), so it needs no fraction. Here $\dot V_{\text{oa},z}$ is at design occupancy, summing all of the
zone's People objects. This is the same function the zone's outdoor-air floor
and the ideal-loads outdoor air use, like EnergyPlus's single
`DesignSpecification:OutdoorAir` calculation. The fan flow and the outdoor-air
flows are both per zone instance, so no multiplier is needed in the ratio.

## Known limitations {#sec-sizing-limitations}

- [#198](https://github.com/bbrannon4/OpenBSE/issues/198),
  [#50](https://github.com/bbrannon4/OpenBSE/issues/50): design-day
  humidity, temperature profile and sky model.
- [#280](https://github.com/bbrannon4/OpenBSE/issues/280): a loop can't serve
  zones with different multipliers.
