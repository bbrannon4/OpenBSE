# Diagnostics {#sec-diagnostics}

OpenBSE reports problems in a `.err` file beside its outputs, in EnergyPlus's style:
warnings and severe errors, with a summary line. It is written whether or not there are any.

## Input validation {#sec-validation}

**Source:** `openbse-io/src/input.rs` (`validate_model`, `ValidationResult`);
`openbse-io/src/plant.rs` (`validate_plant_connections`, `validate_chiller_models`);
`openbse-cli/src/autosize_check.rs`; `openbse-cli/src/sizing_checks.rs`.

Before running, the model is checked for the following:

- **References:** zones in each object, zone groups, constructions and the kind each must be,
  schedules, and every named performance curve (coils, heat pumps, boilers, chillers, VRF,
  water heaters, UPS units).
- **Plant:** every water coil, terminal and panel is connected to a loop that exists, and
  every loop has demand. Chiller inputs meet each model's needs.
- **Values:** for example, a heating supply temperature at or below the setpoint, and
  thermostat schedules of the wrong length.

A severe error stops the run. A missing schedule is a warning, and the object falls back to
always on. After sizing, any `autosize` value still unresolved is a severe error
(@sec-sizing), because otherwise the sentinel would run as a real value. A DX coil whose
rated airflow per watt of rated capacity lies outside 300–450 cfm/ton is a warning, as in
EnergyPlus.

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
| Air component | $\dot m\Delta h$ across a component = its `thermal_output` | severe |
| Air-loop mass | an air loop's terminal boxes pass the flow it supplies; a dual-duct loop's decks carry what its boxes pass | severe |
| Plant unmet | capacity-limited loops | warning |
| Surface–air convergence | envelope solves whose outer passes reached their limit (@sec-surface-hb) | warning |
| HVAC convergence | timesteps whose iteration reached its limit (@sec-hvac-iteration) | warning |

Every air-side component books what its air stream receives, so a component whose
$\dot m\Delta h$ and `thermal_output` disagree is a wiring or model error.

**Holds.** The envelope solver keeps running when a state goes bad: it holds a surface
temperature beyond the limit at its previous iteration's value, and replaces NaN states from
the previous timestep. Each hold is recorded, so a model can't run all year on stuck surfaces
and report no errors. Beyond 2.5 times the limit, the run stops.

**Unmet hours.** A conditioned zone is unmet in a timestep when its temperature is more than
0.2 K below the heating setpoint or above the cooling setpoint the HVAC controlled to that
timestep. Unconditioned zones are never counted. The summary counts the building: a timestep
is an unmet heating (cooling) hour if any zone is unmet heating (cooling), and an unmet load
hour if any zone is unmet either way, however many zones are. Each zone's own hours are
listed beside them, and its flags are the `zone:unmet_heating` and `zone:unmet_cooling`
outputs. The ASHRAE 90.1 line compares the unmet load hours with Appendix G's limit of 300
[@ashrae901].

**Comparison with EnergyPlus.** The building counts match EnergyPlus's facility "Time Setpoint
Not Met" (any zone) with its default 0.2 K reporting tolerance, and the per-zone hours its
zone values. EnergyPlus also reports both during occupied hours only; OpenBSE counts every
hour. The zone heat balance and surface temperature limit have counterparts in EnergyPlus: its zone check uses 20% of the root sum of squares of the terms, and its surface limit is the `Building` input's default of
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
- Unmet hours are counted over every hour; there is no occupied-hours count.
