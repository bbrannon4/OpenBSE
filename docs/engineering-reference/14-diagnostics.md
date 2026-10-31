# Diagnostics {#sec-diagnostics}

OpenBSE reports problems in a `.err` file beside its outputs, in EnergyPlus's style:
warnings and severe errors, with a summary line. It is written whether or not there are any.

## Input validation {#sec-validation}

**Source:** `openbse-io/src/input.rs` (`validate_model`, `ValidationResult`);
`openbse-io/src/plant.rs` (`validate_plant_connections`, `validate_chiller_models`);
`openbse-cli/src/autosize_check.rs`.

Before running, the model is checked for the following:

- **References:** zones in each object, zone groups, constructions and the kind each must be,
  and schedules.
- **Plant:** every water coil, terminal and panel is connected to a loop that exists, and
  every loop has demand. Chiller inputs meet each model's needs.
- **Values:** for example, a heating supply temperature at or below the setpoint, and
  thermostat schedules of the wrong length.

A severe error stops the run. A missing schedule is a warning, and the object falls back to
always on. After sizing, any `autosize` value still unresolved is a severe error
(@sec-sizing), because otherwise the sentinel would run as a real value.

## Run-time checks {#sec-run-checks}

**Source:** `openbse-cli/src/run_checks.rs` (`RunChecks`);
`openbse-envelope/src/diagnostics.rs`.

Energy passes between OpenBSE's models by name, so a load can vanish at a hand-off while
each component is right on its own. These checks watch the hand-offs through the run period,
and report at its end. They read the solution and never change it, so severe findings do not
cancel the run; the `.err` file says the run completed with them.

**Balances.** For each balance, with residual $r$ and scale $s$ at each timestep, a timestep
counts as off when

$$
|r| > 0.01\,s + 1\ \text{W},
$$ {#eq-check-step}

and the whole run fails when

$$
\frac{\left|\sum r\Delta t\right|}{\sum s\Delta t} > 0.01.
$$ {#eq-check-run}

: Run-time checks. {#tbl-run-checks}

| Check | Balance | Severity |
|---|---|---|
| Zone air | surface convection + advection + gains + HVAC = storage (@sec-zone-air-hb) | severe |
| HVAC to zone | heat that air loops, VRF and panels send = heat the zone receives | severe |
| Plant excess | supply equipment delivers no more than the loop's demand | severe |
| Plant reporting | each demand component reports an output at least once | severe |
| Unphysical | NaN or infinite outputs, negative PLR or capacity, COP at or below 0 while running | severe |
| Envelope holds | inside surface temperature beyond 200 °C, NaN states replaced | severe |
| Air component | $\dot m\Delta h$ across a component = its `thermal_output` | warning |
| Plant unmet | capacity-limited loops | warning |
| HVAC convergence | timesteps whose iteration reached its limit (@sec-hvac-iteration) | warning |

The air-component check stays a warning because of a known imbalance
([#168](https://github.com/bbrannon4/OpenBSE/issues/168)).

**Holds.** The envelope solver keeps running when a state goes bad: it holds a surface
temperature beyond the limit at its previous iteration's value, and replaces NaN states from
the previous timestep. Each hold is recorded, so a model can't run all year on stuck surfaces
and report no errors. Beyond 2.5 times the limit, the run stops.

**Unmet hours.** A zone counts as unmet in a timestep when its temperature is more than 0.2 K
outside its active setpoints. These are reported per zone, and in the summary
([#165](https://github.com/bbrannon4/OpenBSE/issues/165)).

**Comparison with EnergyPlus.** The zone heat balance and surface temperature limit have
counterparts in EnergyPlus: its zone check uses 20% of the root sum of squares of the terms, and its surface limit is the `Building` input's default of
200 °C. Plant unmet demand matches its "Plant Supply Side Unmet Demand Rate". The tighter
tolerances above are a deliberate choice, as these checks exist to catch modelling errors.

**What they can't see.** The checks compare the two sides of a hand-off. They miss a hand-off
that was never written: thermal storage charging, heat-exchanger heat, pump heat, and any
moisture error. There is no whole-building energy balance
([#264](https://github.com/bbrannon4/OpenBSE/issues/264)).

## Continuous integration {#sec-ci-checks}

CI runs every shipped model with sanity gates and result baselines, and the ASHRAE 140 suite
against its ranges. A change that shifts results has to change a baseline on purpose.

## Known limitations {#sec-diagnostics-limitations}

- [#264](https://github.com/bbrannon4/OpenBSE/issues/264): no building energy or moisture
  balance; some hand-offs unchecked; missing schedules are warnings; curve coefficient counts
  unchecked.
- [#165](https://github.com/bbrannon4/OpenBSE/issues/165): unmet-hour counts and limits.
- [#168](https://github.com/bbrannon4/OpenBSE/issues/168): Standard 205 DX coil imbalance.
