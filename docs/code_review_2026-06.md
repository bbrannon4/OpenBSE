# OpenBSE Code Review — June 2026 (first pass)

Companion to `physics_review_2026-06.md`. Physics correctness was reviewed and fixed separately;
this review covers code quality, robustness, and architecture. Severity: **HIGH** (can produce
silently-wrong results or crashes from valid input) / **MED** (maintainability hazards likely to
cause future bugs) / **LOW** (cleanup).

## Overall assessment

The codebase is in much better shape than typical for its size and age: only ~25 `unwrap/expect/panic`
sites outside tests across 58k lines, exactly 1 TODO, 1 `allow(dead_code)`, hand-traced unit tests
with E+ references, clean crate layering with no circular deps, and consistent style. The risk is
concentrated in `openbse-cli/src/main.rs` (8.4k lines) — the control/dispatch layer — not in the
physics crates.

## Fix status (2026-06-12)

| Finding | Status |
|---|---|
| CR-1 name-substring coil dispatch | **FIXED** — `LoopInfo::component_kinds` map + `coil_role()` (kind-based, legacy name heuristic only as fallback for components not in the equipment list); sizing autosize loop dispatches on `comp.component_kind()` directly. All 9 dispatch blocks converted. |
| CR-2 PerformanceCurve panic | **FIXED** — `evaluate()` now interpolates TableLookup curves positionally (x→axis 0, y→axis 1); `evaluate_table()` on a Polynomial logs a warning and returns the neutral modifier instead of panicking. |
| CR-3 stringly-typed sentinels | **FIXED** — typed `ControlSignals` fields (`mixed_air_temp`, `oa_fraction`, `loop_plr`, `effective_oa_w`, `return_air_w`). The refactor confirmed the predicted failure mode twice: `__return_air_temp__` was inserted 5× and never read (dead), and `__effective_oa_w__` was read but never inserted — the ERV moisture pre-conditioning it carried was silently inoperative (now a typed field ready to be wired when HR humidity credit is implemented). |
| CR-9 NaN-unsafe sorts | **FIXED** — `total_cmp` in sizing.rs. |
| CR-12 README crate count | **FIXED** — 10 crates, table updated, AI_CONTEXT.md updated. |

A/B regression after all fixes: cases 600/900/CE100 and SingleFamily Boulder bit-identical to the
post-physics-fix baselines (pure refactor confirmed).

## Findings

### HIGH

- **CR-1: Coil control dispatch by name substring** (`main.rs` AHU coil control, ~line 6850;
  same pattern in other signal builders). `lname.contains("cool")/"heat"/"hw"` decides which
  setpoint a component receives. A coil named outside the patterns silently gets no control; a
  name matching both patterns gets the wrong one ("Precool HX", "Heat Pump Cooling"). This is a
  *silent wrong-results* class. Fix: dispatch on `ComponentKind` (already available on every
  component) and resolve ambiguity by graph position. (= physics review M-2.)

- **CR-2: `PerformanceCurve::evaluate` panics on TableLookup variant** (performance_curve.rs:240,
  :304). If YAML wires a table curve where a component calls `evaluate()`, the engine panics at
  runtime — user-input-triggerable crash. Fix: make `evaluate()` handle both variants (delegate
  to table interpolation), or validate at parse time and return a clean input error.

- **CR-3: Stringly-typed control side-channel.** `ControlSignals::coil_setpoints:
  HashMap<String, f64>` doubles as a message bus for sentinels (`__plr__`, `__oa_fraction__`,
  `__pszac_mixed_air_temp__`, `__effective_oa_w__`, `__return_air_w__`, …). A typo'd key fails
  silently to a default; a component named like a sentinel would collide. Fix: typed fields on
  `ControlSignals` (Option<f64> each); keep the map only for real per-component setpoints.

### MED

- **CR-4: `main.rs` is 8.4k lines mixing dispatcher + 8 system-type signal builders.** The
  builders (PSZ/CRAC/CRAH/DOAS/FCU/VAV/dual-duct) duplicate mixed-air, economizer, humidity, and
  PLR logic with copy-paste drift (the physics review found derate constants and capacity
  references that had drifted apart). The `openbse-controls` crate exists but is nearly empty —
  the builders belong there as testable units. Suggested order: extract shared
  mixed-air/economizer helpers first, then move one builder at a time.

- **CR-5: Component output reporting is stale/inconsistent under PLR scaling.** Observed during
  CE100 debugging: after the system-level PLR block zeroes `electric_power`/`mass_flow`/
  `thermal_output`, `detailed_outputs()` values (`sensible_load`, `total_load`, `plr`, inlet/
  outlet states) remain at the component's unscaled internal state. CSV columns for one
  component can disagree with each other (mass_flow=0 with total_load=7455 W). Fix: scale or
  re-derive detailed outputs in the same pass, or document which columns are pre-PLR.

- **CR-6: `AUTOSIZE = -99999.0` sentinel f64** with `is_autosize(val) = |val − AUTOSIZE| < 1`.
  Sentinel values flow through arithmetic until something checks; the physics review nearly
  attributed a divergence to this. `AutosizeValue` enum already exists in core — push it to all
  capacity/flow fields and resolve to plain f64 immediately after sizing.

- **CR-7: `comp_kind_map` in main.rs duplicates `EquipmentInput → ComponentKind` mapping** that
  also exists implicitly via each component's `component_kind()`. CLAUDE.md memory even warns
  the enum must be updated in two places. Fix: build the graph first, then ask components for
  their kind; delete the match.

- **CR-8: `detailed_outputs() -> HashMap<String, f64>` allocates a fresh map with String keys
  per component per timestep** (35k timesteps × N components). Fine today, but it's the hot
  path; an enum-keyed small-vec or a visitor (`fn report(&self, out: &mut dyn FnMut(&str, f64))`)
  removes the churn. Bundle with CR-5 since both touch the same surface.

- **CR-9: `sizing.rs` `partial_cmp().unwrap()`** (lines 762, 798, 951) panics on NaN design
  temps (corrupt weather file). Use `f64::total_cmp`.

- **CR-10: Inconsistent diagnostics: `eprintln!` in ctf.rs warnings vs `log::warn!` elsewhere.**
  CTF U-value mismatch warnings bypass the log filter and can't be silenced/captured. Unify on
  `log`.

### LOW

- **CR-11: Clippy backlog (~157 warnings).** Top categories: derivable impls (17), doc list
  indentation (14), no-effect operations (8), ref-to-ref patterns (6), too-many-arguments (14
  functions, up to 12 args — mostly the signal builders; CR-4 fixes these structurally),
  `get(0)` → `first()` (4). One `cargo clippy --fix` pass plus manual triage would get close to
  zero; CI already runs clippy non-blocking — consider making it blocking once clean.
- **CR-12: README says "8 crates"; there are 10** (`openbse-a205`, `openbse-cosim` missing from
  the table and architecture diagram in AI_CONTEXT.md).
- **CR-13: HP-4 curve argument-order convention** (heating coils evaluate `f(T_out, T_in)`,
  cooling `f(T_wb_in, T_out)`): document in the schema field descriptions so E+ curve
  coefficients aren't pasted with swapped axes.
- **CR-14: Float equality comparisons** (7 sites) are all sentinel/zero checks and benign; no
  action needed beyond awareness.

## Not yet reviewed (second pass candidates)

- `tools/editor/` (Tauri + React) — entire frontend.
- `openbse-a205` interpolation internals, `openbse-cosim` subprocess protocol (error handling
  around child process death mid-simulation is worth a look).
- `output.rs` (3.9k lines) variable-resolution logic.
- Concurrency: none observed (single-threaded sim), so no review needed yet.

## Suggested fix order

1. CR-2 (panic from valid input — small fix)
2. CR-1 + CR-3 (silent-wrong-results class; do together since both touch ControlSignals)
3. CR-5 (reporting consistency — users compare these CSVs to E+)
4. CR-9, CR-10, CR-12 (small)
5. CR-4 extraction (largest; do incrementally, one system type at a time, A/B regression after each)
6. CR-6, CR-7, CR-8, CR-11 opportunistically during CR-4
