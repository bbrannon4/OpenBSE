# Simulation framework {#sec-framework}

This chapter describes how a run is organized: the sequence of a run, the
timestep loop, the zone predictor–corrector, the iteration that couples HVAC
to the zones, warmup, and the component graph. The physics of each term in
the zone balance is in @sec-envelope; sizing runs are in @sec-sizing; the
control logic of each system type is in @sec-controls.

## Run sequence {#sec-run-sequence}

**Source:** `openbse-cli/src/main.rs::main`.

A run of one model proceeds as follows. A parametric model expands into
several runs first (`openbse-io/src/parametric.rs`), each following the same
sequence.

1. Parse the YAML model and validate its cross-references.
2. Read the weather file. It must start on 1 January, hour 1, and cover the
   run period; record $N$ is hour $N$ of a non-leap year (@sec-time).
3. Build the components and the component graph (@sec-graph), the air-loop
   descriptors (`LoopInfo`), DHW systems and radiant panels.
4. Initialize the envelope: geometry, constructions and CTF coefficients for
   the timestep $\Delta t$ (`BuildingEnvelope::initialize`).
5. Size zones, systems and plant from the design days, if any input is
   `autosize` (@sec-sizing). The run stops if an `autosize` value remains
   unresolved.
6. Warm up the envelope (@sec-warmup).
7. Order the plant loops (@sec-plant-order) and precompute the sunlit
   fractions of every surface for the run period
   (`BuildingEnvelope::precompute_solar`, cached in a `.solar` file).
8. Run the timestep loop (@sec-timestep-loop) over the run period.
9. Write outputs, and the run-time check report to the `.err` file
   (@sec-diagnostics).

## Simulation modes {#sec-modes}

Each timestep takes one of three paths.

- **Ideal loads or free float.** Taken when any zone has an ideal-loads
  system, or when the model has no thermostats. The envelope is solved once
  per timestep, and the ideal-loads system inside it (@sec-sizing) meets the
  zones' loads. Air loops, plant loops, VRF and radiant panels are not
  simulated.
- **Coupled envelope and HVAC.** The usual path: the HVAC iteration of @sec-hvac-iteration.
- **HVAC only.** Taken when the model has no envelope. Components are
  simulated once per timestep in graph order (`simulate_hvac`), with the
  setpoints of the model's `controls:` block.

## Timestep loop {#sec-timestep-loop}

**Source:** `openbse-cli/src/main.rs::main`, section 7.

For each hour $H$ of the run period and each timestep $k = 1,\dots,N$ in it,
the weather is interpolated between the previous and current hourly records,

$$
x = x_{H-1} + \frac{k}{N}\left(x_H - x_{H-1}\right),
$$ {#eq-weather-interp}

for dry-bulb temperature, humidity, pressure, wind speed, long-wave
radiation and sky cover (`WeatherHour::interpolate`). Solar irradiance and
wind direction take the current hour's value for the whole hour. At $k = N$
the interpolated values equal the hourly record. @sec-site describes the
weather data.

On the coupled path, one timestep runs as follows.

1. **Start-of-step state.** The zone air temperatures
   $T_z^\text{start}$, humidity ratios, predictor temperatures and loads are
   read from the envelope. They stay fixed through the HVAC iteration; see @sec-predictor.
2. **Shading control.** Blind and shade deployment is evaluated from the
   previous timestep's surface conditions (`evaluate_blind_control`).
3. **Radiant panels and VRF.** Panel part-load ratios and VRF indoor-unit
   modes are set from the start-of-step loads.
4. **HVAC iteration** (@sec-hvac-iteration), then the corrector solve.
5. **History update.** `BuildingEnvelope::update_bdf_history` shifts each
   zone's temperature and humidity history and each surface's CTF history,
   exactly once per timestep.
6. **Run-time checks and outputs.** The conservation checks read the
   converged state (@sec-diagnostics). Then come the data-center aisle model,
   electrical distribution losses, DHW systems, exterior equipment and
   submeters.

DHW systems run after the zone heat balance. The heat that water heaters
exchange with their ambient zones is therefore applied to those zones on the
next timestep (`dhw_zone_gains`).

## Zone air heat balance in time {#sec-zone-time}

**Source:** `openbse-envelope/src/zone.rs::backward_diff_effective`,
`solve_zone_air_temp`, `solve_zone_air_temp_with_q`; called from
`openbse-envelope/src/heat_balance.rs::solve_timestep`, step 6c.

Each zone's air is one well-mixed node, with capacitance
$C_z = \rho V c_p$ [J/K]. Its energy balance is

$$
\begin{aligned}
C_z \frac{dT_z}{dt}
={}& \sum_i h_{c,i} A_i \left(T_{s,i} - T_z\right)
+ \dot m_\text{inf} c_p \left(T_\text{o} - T_z\right) \\
&+ \dot m_\text{sup} c_p \left(T_\text{sup} - T_z\right)
+ \dot Q_\text{conv} + \dot Q_\text{HVAC},
\end{aligned}
$$ {#eq-zone-ode}

where:

- the sum runs over the zone's surfaces $i$, with inside convection
  coefficient $h_{c,i}$, area $A_i$ and inside surface temperature $T_{s,i}$;
- $\dot m_\text{inf}$ is all outdoor air entering the zone directly
  (infiltration, ventilation, natural ventilation, and outdoor air not
  handled by an air loop), at $T_\text{o}$;
- $\dot m_\text{sup}$ is the HVAC supply air at $T_\text{sup}$;
- $\dot Q_\text{conv}$ is the convective gain to the air: internal gains,
  solar and window gains, and the HVAC-related gains passed in as
  `other_sensible_gains` (duct losses, radiant-panel convection,
  water-heater losses, less lighting and hot-aisle return-air heat);
- $\dot Q_\text{HVAC}$ is the output of an ideal-loads system, and zero
  otherwise.

@sec-envelope defines each term. $\rho$ is evaluated at the weather's
barometric pressure and the zone's current temperature and humidity ratio,
and $c_p$ at its humidity ratio.

### Time discretization {#sec-bdf}

The derivative is replaced by a backward difference of order $k$ over the
zone temperatures at the ends of the last timesteps, $T_n$, $T_{n-1}$,
$T_{n-2}$:

$$
\frac{dT_z}{dt} \approx \frac{a_k}{\Delta t}\left(T_z - \tilde T_k\right),
$$ {#eq-bdf}

with the coefficients of @tbl-bdf. The code folds $a_k$ into an effective
timestep $\Delta t / a_k$, so that every zone solve keeps the first-order
form.

| Order $k$ | $a_k$ | $\tilde T_k$ |
|---|---|---|
| 1 | 1 | $T_n$ |
| 2 | $3/2$ | $(4T_n - T_{n-1})/3$ |
| 3 | $11/6$ | $(18T_n - 9T_{n-1} + 2T_{n-2})/11$ |

: Backward-difference coefficients. {#tbl-bdf}

Substituting @eq-bdf into @eq-zone-ode, with surface temperatures and flows
fixed at their current values, gives the zone temperature at the end of the
timestep:

$$
T_z = \frac{\sum_i h_{c,i} A_i T_{s,i} + \dot m_\text{inf} c_p T_\text{o}
      + \dot m_\text{sup} c_p T_\text{sup} + \dot Q_\text{conv} + \dot Q_\text{HVAC}
      + a_k \frac{C_z}{\Delta t} \tilde T_k}
     {\sum_i h_{c,i} A_i + \dot m_\text{inf} c_p + \dot m_\text{sup} c_p + a_k \frac{C_z}{\Delta t}}.
$$ {#eq-zone-solve}

The order starts at 1 and rises by one with each history update, up to 3.
After warmup the history is reset to the current temperature and the order to
1 (`reset_bdf_history_to_current`). During warmup the order is capped at 1
(@sec-warmup). The zone humidity ratio uses the same scheme with its own
history (@sec-envelope).

**Comparison with EnergyPlus.** @eq-zone-solve with $k = 3$ is EnergyPlus's
default `ThirdOrderBackwardDifference` algorithm
(`ZoneTempPredictorCorrector`, `ZoneSpaceHeatBalanceData::correctAirTemp`):
$T_z = [\,\text{TempIndCoef} + \text{AirPowerCap}(3T_n - \tfrac32 T_{n-1} + \tfrac13 T_{n-2})\,]
/ [\,\tfrac{11}{6}\text{AirPowerCap} + \text{TempDepCoef}\,]$
[@eplus-engref-25-2, "Zone Air Heat Balance Algorithm"]. The two differ in
the following ways:

- **Order ramp.** EnergyPlus applies the third-order form from the first
  timestep, with its history initialized to the starting temperature.
  OpenBSE ramps the order from 1.
- **No alternative algorithms.** OpenBSE has no `AnalyticalSolution` or
  `EulerMethod` options.
- **No capacitance multiplier.** OpenBSE has no zone air capacitance
  multiplier (`ZoneCapacitanceMultiplier:ResearchSpecial`).

## Predictor {#sec-predictor}

**Source:** `heat_balance.rs::solve_timestep` (`temp_no_hvac`, ideal-load
lines); `main.rs::simulate_all_loops` (predictor modes).

Each envelope solve also computes the temperature the zone would reach with
no supply air, which is @eq-zone-solve with $\dot m_\text{sup} = 0$ and
$\dot Q_\text{HVAC} = 0$:

$$
T_z^\text{nf} = \frac{\sum_i h_{c,i} A_i T_{s,i} + \dot m_\text{inf} c_p T_\text{o}
      + \dot Q_\text{conv} + a_k \frac{C_z}{\Delta t} \tilde T_k}
     {\sum_i h_{c,i} A_i + \dot m_\text{inf} c_p + a_k \frac{C_z}{\Delta t}}.
$$ {#eq-free-float}

$\dot Q_\text{conv}$ keeps the HVAC-related gains of the solve, so
$T_z^\text{nf}$ counts the radiant-panel convection and duct losses already
delivered. The air-loop controls take each zone's mode from it:

$$
\text{mode} =
\begin{cases}
\text{heating} & T_z^\text{nf} < T_\text{heat,sp},\\
\text{cooling} & T_z^\text{nf} > T_\text{cool,sp},\\
\text{deadband} & \text{otherwise.}
\end{cases}
$$ {#eq-predictor-mode}

When no free-float temperature is available (during warmup), the mode comes
from the loads instead: heating if the heating load exceeds 10 W and the
cooling load, cooling in the converse case, and otherwise from the zone
temperature against the setpoints.

**Timing.** The first HVAC iteration starts from the free-float temperature
stored by the previous timestep's solve. Its envelope solve then refreshes
the free-float temperature with the current timestep's gains, weather and
surface temperatures, and the mode is held from there on. The first iteration
never counts as converged, so the converged solution always uses the current
timestep's mode. VRF indoor-unit modes refresh at the same point.
`simulation.current_timestep_predictor: false` keeps the previous timestep's
value throughout, which delays the response to a new load by one timestep.

**Load to setpoint.** Each solve also stores a zone's load as a linear
function of the target temperature $T^*$ and the start temperature
$T^\text{start}$ (`IdealLoadLine`):

$$
\begin{aligned}
\dot Q(T^*, T^\text{start}) ={}&
\left(\sum_i h_{c,i} A_i + \dot m_\text{inf} c_p + a_k \tfrac{C_z}{\Delta t}\right) T^* \\
&- \sum_i h_{c,i} A_i T_{s,i} - \dot m_\text{inf} c_p T_\text{o} - \dot Q_\text{conv}
- a_k \tfrac{C_z}{\Delta t}\, T^\text{start},
\end{aligned}
$$ {#eq-load-line}

positive when heat is needed. Within the HVAC iteration the controls use
$\dot Q(T_\text{sp}, T_z^\text{start})$, the load to bring the air from its
start-of-step temperature to setpoint. The heating load is
$\max(\dot Q, 0)$ at the heating setpoint and the cooling load
$\max(-\dot Q, 0)$ at the cooling setpoint. On the first iteration the
previous solve's loads are used.

**Comparison with EnergyPlus.** EnergyPlus predicts each zone's load once per
timestep, before the system simulation, with the current timestep's weather,
gains and surface temperatures
(`ZoneSpaceHeatBalanceData::calcPredictedSystemLoad`)
[@eplus-engref-25-2, "Summary of Predictor-Corrector Procedure"]
[@taylor1990]. Holding the mode and the load fixed through the system
iteration matches EnergyPlus. E+ evaluates the predictor before any HVAC
runs, where OpenBSE needs one iteration to get it. The refresh comes from a
solve that includes that iteration's HVAC, so its surface temperatures differ
slightly from E+'s, which use the previous zone air temperature. Two other
differences are filed as issues:

- **Load history.** @eq-load-line uses $T^\text{start}$ where @eq-zone-solve
  and EnergyPlus use the history $\tilde T_k$. The two agree only when the
  zone temperature is steady
  ([#192](https://github.com/bbrannon4/OpenBSE/issues/192)).
- **Unscheduled heating setpoint.** The heating load targets the constant
  occupied setpoint, even when a setpoint schedule or the unoccupied
  setpoint is in force
  ([#117](https://github.com/bbrannon4/OpenBSE/issues/117)).

## HVAC–zone iteration {#sec-hvac-iteration}

**Source:** `main.rs::main`, "Predictor-Corrector HVAC-Envelope Iteration".

On the coupled path, HVAC and zones are iterated to a fixed point within the
timestep. Iteration $m = 0, 1, \dots$ runs four steps.

1. **HVAC.** `simulate_all_loops` builds each air loop's control signals
   and simulates its components (@sec-controls). It uses:
   - the zone temperatures of the last iteration, $T_z^{(m-1)}$ (or
     $T_z^\text{start}$ when $m = 0$), for AHU-level controls such as
     supply-air reset and the economizer;
   - $T_z^\text{start}$, the predictor modes and the start-of-step loads
     for terminal and unitary control.

   Radiant panels are simulated next, and then the plant loops, which take
   the coil, terminal and panel loads in plant order (@sec-plant-order).
2. **Supply damping.** For $m \ge 1$, each zone's supply is blended with
   the previous iteration's, by mass flow and by mass-weighted temperature
   and humidity:

   $$
   \begin{aligned}
   \dot m^\text{d} &= \tfrac12\left(\dot m^{(m)} + \dot m^{(m-1)}\right),\\
   T^\text{d} &= \frac{\dot m^{(m)} T^{(m)} + \dot m^{(m-1)} T^{(m-1)}}{2\,\dot m^\text{d}},\qquad
   W^\text{d} = \frac{\dot m^{(m)} W^{(m)} + \dot m^{(m-1)} W^{(m-1)}}{2\,\dot m^\text{d}}.
   \end{aligned}
   $$ {#eq-supply-damping}

   VRF and radiant-panel output is not damped.
3. **Envelope solve.** The envelope is solved with the damped supply, VRF
   output, radiant-panel gains, duct losses, water-heater gains and the
   return-air corrections (`BuildingEnvelope::solve_timestep`). Each solve
   reruns the inside surface heat balance, starting from the last zone
   temperatures, and then the zone air balance @eq-zone-solve. The zone
   history $\tilde T_k$ and the CTF histories are not shifted, so every
   solve in the timestep starts from the same past.
4. **Convergence test.** The iteration stops when

   $$
   \max_z \left| T_z^{(m)} - T_z^{(m-1)} \right| \le 0.05\ \text{K},
   $$ {#eq-hvac-convergence}

   or after 10 iterations. Unconverged timesteps are counted and reported
   by the run-time checks (@sec-diagnostics).

**Corrector.** The coils, plant and fuel are booked from the last
iteration. If damping changed any zone's supply in that iteration, the
envelope is solved once more with that iteration's undamped supply. The zone
then receives exactly the heat the coils produced. The HVAC-to-zone delivery
check verifies this every timestep.

**Comparison with EnergyPlus.** EnergyPlus solves the surfaces once per zone
timestep, predicts the zone loads, iterates the air and plant loops against
fixed zone conditions until their node states converge (at most 20
iterations, `HVACManager::SimHVAC`), and then corrects the zone air once
with the converged supply (`HVACManager::ManageHVAC`)
[@eplus-engref-25-2, "Integrated Solution Manager"]. OpenBSE's corrector
step matches it. The differences are deliberate:

- **Implicit coupling.** OpenBSE re-solves the surfaces and the zone air
  within the iteration, so the HVAC sees zone and return conditions that
  respond to its own output. Only control decisions are held at the start
  of the step.
- **Supply damping.** @eq-supply-damping steers the iteration of on/off
  systems toward convergence. It has no EnergyPlus counterpart and no effect
  on the energy booked.
- **Fixed system timestep.** EnergyPlus shortens the system timestep, down
  to 1 minute, when a zone's temperature changes by more than 0.3 K in a
  step (`DataConvergParams::MaxZoneTempDiff`). OpenBSE always uses the zone
  timestep $\Delta t$.

**Assumptions.** Within a timestep, the zone's own response is the only
state that changes. Time-dependent state in the envelope solve therefore
advances once per timestep, in `update_bdf_history`, whatever the number of
solves. Species transport integrates from the start-of-step concentrations on
every solve, so the last solve's flows set the result. The natural-ventilation
open/close decision is made on the first solve from the start-of-step zone
temperature, and held for the rest of the timestep.

## Warmup {#sec-warmup}

**Source:** `main.rs::main`, section 7a.

Before the run period, the envelope is driven with the first 7 days of the
weather file for up to 4 cycles (28 days). Each timestep makes a single HVAC
pass (no iteration and no predictor), with no plant loops, and solves the
envelope once. The BDF order is capped at 1. Warmup ends at the first cycle
for which

$$
\max_z \left| T_z^\text{end} - T_z^\text{begin} \right| < 0.5\ \text{K},
$$ {#eq-warmup}

where $T_z^\text{begin}$ and $T_z^\text{end}$ are the zone air temperatures
at the start and end of the 7-day cycle. Afterwards the zone history is reset
(@sec-bdf), and the night-cycle timers and the simulation clock restart.

**Comparison with EnergyPlus.** EnergyPlus repeats the first day of the run
period with the full HVAC simulation. Each day it compares every zone's
maximum and minimum temperature (tolerance 0.4 K) and peak heating and
cooling loads (4%) with the previous day's, for between 1 and 25 days
(`HeatBalanceManager::CheckWarmupConvergence`). It reports a severe error if
these do not converge. OpenBSE's warmup differs in four ways:

- it always uses 1–7 January, whatever the run period;
- it tests one temperature per zone and no loads;
- it accepts non-convergence silently;
- it leaves VRF, radiant panels and the plant out.

These differences are filed as
[#189](https://github.com/bbrannon4/OpenBSE/issues/189).

## Component graph {#sec-graph}

**Source:** `openbse-core/src/graph.rs::SimulationGraph`; built by
`openbse-io/src/input.rs::build_graph`.

Every air-side and plant component is a node of a directed graph
(`petgraph::DiGraph`). Consecutive components of an air loop, then its
terminals, are joined by `AirFlow` edges, and consecutive supply components
of a plant loop by `WaterFlow` edges. The `AirToPlant` edge type exists but
is not created. `compute_simulation_order` sorts the graph topologically. If
the graph has a cycle, a modified Kahn's algorithm breaks it at the node with
the most incoming edges.

The graph's order is used only on the HVAC-only path. On the coupled path:

- each air loop runs its components in the order they are listed in the
  model (`LoopInfo::component_names`, `simulate_loop_components`). The first
  component takes the mixed air; each later one takes the previous outlet;
- plant loops are ordered as in @sec-plant-order, and their equipment is
  dispatched by load (@sec-plant).

### Plant-loop order {#sec-plant-order}

Plant loops are simulated in an order that respects their couplings:

- a heat exchanger's source loop runs before the loop it serves;
- a chilled-water loop runs before its condenser loop;
- a heat-recovery chiller's loop runs before the loop it heats.

The order is a topological sort of these dependencies (Kahn's algorithm).
Loops in a dependency cycle are appended at the end. A loop that runs before
one it depends on reads that loop's supply conditions from the previous
timestep (`loop_supply_conditions`).

## Known limitations {#sec-framework-limitations}

- [#117](https://github.com/bbrannon4/OpenBSE/issues/117): heating loads for
  control target the constant occupied setpoint, not the scheduled or
  unoccupied one.
- [#189](https://github.com/bbrannon4/OpenBSE/issues/189): warmup days,
  convergence test and HVAC coverage differ from EnergyPlus.
- [#191](https://github.com/bbrannon4/OpenBSE/issues/191): ideal loads on
  any zone disables every air loop, plant loop, VRF and radiant panel.
- [#192](https://github.com/bbrannon4/OpenBSE/issues/192): the load to
  setpoint ignores the backward-difference history.
- [#193](https://github.com/bbrannon4/OpenBSE/issues/193): unused
  `SimulationRunner` and configuration fields, and stale comments in the
  loop.
- No adaptive system timestep (see @sec-hvac-iteration).
