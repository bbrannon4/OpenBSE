# Shipped-model checks

`run_shipped_models.py` runs every model in `examples/` and `prototype_tests/`
and fails if any of them breaks, produces nonsense, or changes its results
without an updated baseline. The checks were added after
the week of 2026-10-21, when several shipped models were found broken by hand (#159).

## What each model must pass

1. **Runs.** The release binary exits with code 0. It parses and validates the
   model first, so a severe validation error fails here.
2. **Sanity gates**, read from the outputs:
   - every `zone_temp` column stays within `zone_temp_limits_c` (default −40…60 °C);
   - no NaN or inf in any column of the timestep results CSV;
   - no `plr`, `rtf`, `part_load_ratio` or `*capacity*` column goes negative;
   - no `cop*` column is negative, or ≤ 0 while its component's `plr` > 0;
   - no sized capacity, airflow or peak in the `*_sizing.csv` files is negative
     (this catches the −99,999 W autosize sentinel);
   - annual unmet heating and cooling hours are at most `max_unmet_hours`
     (default 300, from the summary; summed over zones);
   - the `.err` file reports no severe error (`max_severe_errors`, default 0).
     The engine's run-time energy-conservation and sanity checks (#160) write
     their findings there; severe ones don't stop the run, so the exit code
     alone would let them through.
3. **Baselines.** Annual end uses and unmet hours from `<stem>_summary.csv`,
   and the mean of all zone temperatures over the run, match
   `model_baselines.json` within 0.1% (with a floor of 0.1 kWh, 1 h or 0.01 °C).
   Parametric models write no summary, so for them the baseline holds each
   run's total electric and fuel energy, integrated from its results CSV.

## Files

| File | What it holds |
|---|---|
| `run_shipped_models.py` | the runner |
| `model_checks.json` | each model's tier and any exception, with a written reason |
| `model_baselines.json` | the expected annual results, one entry per model |
| `../weather/` | the weather files the models use (see its README) |

Every YAML under `examples/` and `prototype_tests/` must have an entry in
`model_checks.json`, so a new model can't be skipped by accident. Tiers:

- `fast`: runs in CI on every push and PR (about 3 minutes in all).
- `slow`: Apartment and Hospital (6–15 min each). CI runs them on
  `workflow_dispatch` and on every push to `main`. Entries marked `heavy`
  always run one at a time.
- `local`: never runs in CI, because the model needs more memory than a GitHub
  runner has (the three LargeOffice models: 19 GB peak against the runner's 16 GB).
  Run `--tier local` on your machine, alone, before merging an engine change.
- `skip`: never runs; listed in every report with its `reason`.

Any override (`weather`, `max_unmet_heating_hours`, `max_unmet_cooling_hours`,
`zone_temp_limits_c`, `baseline_platforms`, `max_severe_errors`) needs a matching `<key>_reason`, or
the runner refuses to start. `baseline_platforms` (e.g. `["Linux"]`) marks a
result known to differ by platform: elsewhere a baseline drift is a warning.

## Running locally

```bash
cargo build --release
python3 tests/run_shipped_models.py              # fast tier
python3 tests/run_shipped_models.py --tier slow  # Apartment, Hospital, one at a time
python3 tests/run_shipped_models.py --tier local # LargeOffice x3: ~19 GB each, nothing else running
python3 tests/run_shipped_models.py --models examples/vav_reheat.yaml --keep-outputs --out /tmp/vav
```

- Outputs go to a temporary directory and are deleted afterwards, unless you
  pass `--out` and `--keep-outputs`. Keep outputs out of `~/Documents`: iCloud
  stalls builds and turns files into "dataless" placeholders there.
- The binary is `$CARGO_TARGET_DIR/release/openbse`, or `target/release/openbse`.
  Use one target directory per checkout. Cargo silently links stale crates
  when two checkouts share one.
- `--jobs N` (default 2) runs that many non-heavy models at once.
- **Memory.** The LargeOffice models peak at about 13–19 GB each, and
  Hospital and Apartment at about 5–9 GB. The slow tier runs them one at a time.
  On macOS the runner also waits before each launch while
  `kern.memorystatus_vm_pressure_level` reports warn or critical.

## When results change on purpose

A PR that changes results updates the baseline in the same commit, so every
result change is an explicit, reviewable diff:

```bash
python3 tests/run_shipped_models.py --tier all --update-baselines
```

Then put the new SHA-256 it prints for `tests/model_baselines.json` into the
pin in `.github/workflows/ci.yml` ("Verify model baselines are untampered").
The same pin covers `model_checks.json`. This follows the ASHRAE 140 job's
pattern for `acceptance_ranges_140_2023.json`. The baselines and thresholds are
the pass/fail law for this job, so they can't change without an explicit diff
to the workflow file. Say in the PR why each number moved.

`--update-baselines` only rewrites models that ran and passed their gates, so
`--models …` updates just those entries. A model that fails a sanity gate
can't be baselined; fix it, or give it a documented exception.

**Platform.** The baselines must match what CI measures on `ubuntu-latest`.
If a local macOS run differs from CI by more than the tolerance, take the
values from the `shipped-models-report` artifact of the CI run:

```bash
python3 tests/run_shipped_models.py --baselines-from shipped-models-report.json
```
