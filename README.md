# OpenBSE — Open Building Simulation Engine


**This is both an experiment in a potential new building energy modeling engine, but also to test the limits of AI in this type of physics based software development.  ~This line was written by a human, nothing else in this file or repo is.**

**Modern, open-source building energy simulation in Rust.**

OpenBSE is a ground-up reimplementation of building energy simulation using validated physics (heat balance methods, psychrometrics, HVAC component performance curves) expressed in a clean, modern, composable architecture. It replaces the complexity of legacy BEM tools with:

- **Clean YAML input format** — no nodes, branches, branch lists, or connector lists
- **Graph-based simulation engine** — automatically determines execution order via topological sort
- **Decoupled controls framework** — sensor/actuator model (sense → compute → act)
- **Full building envelope physics** — CTF, convection, solar, longwave radiation, infiltration, internal gains
- **Generic component contracts** — any component can go on any loop if fluid types match
- **Plant, zone equipment and data-center models** — boilers, chillers, towers, storage, VRF, radiant panels, CRAC/CRAH
- **Multi-format weather support** — EPW and TMY3 CSV (ASHRAE Standard 140 prescribed format)

## Why This Exists

EnergyPlus is the DOE's flagship building energy simulation tool. Its physics are well-validated, but its software architecture — inherited from 1970s-era BLAST and DOE-2 Fortran — creates serious usability problems: redundant HVAC topology specification, rigid system templates, simulation order baked into user input, and controls tightly coupled to specific system types.

OpenBSE preserves the validated physics while fundamentally rethinking the system topology, HVAC modeling framework, and user input paradigm. EnergyPlus is the minimum reference standard: each model either matches the EnergyPlus method or deviates from it deliberately, for more accurate physics, for energy conservation or for a project requirement. The [Engineering Reference](docs/engineering-reference/) records which, and why, for every model.

> **For AI assistants**: See [`docs/AI_CONTEXT.md`](docs/AI_CONTEXT.md) for a comprehensive project overview designed as initial context.

## Quick Start

### Download

Pre-built binaries are available on the [Releases](https://github.com/bbrannon4/OpenBSE/releases) page. Each download includes the CLI engine, the desktop editor, and example models.

**macOS users:** macOS blocks unsigned applications by default. After extracting, run these commands to allow them:

```bash
xattr -d com.apple.quarantine openbse
xattr -cr openbse-editor*.dmg
```

Or for the editor after installing: `xattr -cr /Applications/openbse-editor.app`

### Build from Source

```bash
git clone https://github.com/bbrannon4/OpenBSE.git
cd OpenBSE
cargo build --release
```

### Run Tests

```bash
cargo test --workspace
```

About 900 unit and integration tests across the 11 crates.

### Run a Simulation

```bash
./target/release/openbse examples/simple_heating.yaml -w weather.epw -o results.csv
```

The `-w` flag specifies the weather file (EPW). If omitted, the engine falls back to `weather_files` in the YAML. The CLI flag always takes precedence. The `-o` flag sets the results CSV; every other output (`.err`, summaries, sizing reports) goes in the same folder, which is created if it doesn't exist. Without `-o`, outputs go next to the model file.

## Architecture

OpenBSE is a Rust workspace with 11 crates:

| Crate | Purpose |
|-------|---------|
| `openbse-psychrometrics` | Moist air property calculations (Hyland & Wexler) |
| `openbse-weather` | EPW and TMY3 CSV weather file parsing |
| `openbse-core` | Simulation graph, timestep loop, component traits |
| `openbse-components` | HVAC component models (fans, coils, heat pumps, terminals, VRF, boilers, chillers, towers, pumps, storage, water heaters) |
| `openbse-controls` | Decoupled control framework (thermostats, setpoints, resets) |
| `openbse-envelope` | Building envelope heat balance, airflow network and comfort |
| `openbse-io` | YAML parsing, CSV output, design day sizing, summary reports |
| `openbse-airloop` | Per-system-type air-loop control signal builders (PSZ/CRAC/CRAH/DOAS/FCU/VAV/dual-duct) |
| `openbse-cli` | Command-line interface and multi-loop control dispatcher |
| `openbse-a205` | ASHRAE Standard 205 equipment performance data |
| `openbse-cosim` | Co-simulation proxies (external air/plant components via subprocess) |

No circular dependencies. Components implement traits (`AirComponent`, `PlantComponent`, `EnvelopeSolver`) defined in `openbse-core`. Rust's type system enforces physical constraints at compile time — `AirPort` and `WaterPort` are distinct types, so connecting a water pipe to an air duct won't compile.

The **desktop editor** (`openbse-editor`) is a Tauri + React app in `tools/editor/` that provides a GUI for editing YAML model files with schema validation, selecting weather files, and running simulations with real-time output streaming — all without touching the command line.

### Core Design Principles

1. **Graph-Based Topology** — Users describe components and connections. The engine builds the directed graph, creates internal nodes, and determines simulation order automatically.

2. **Generic Component Contracts** — Every component implements a simple trait. A boiler takes water in and sends it out hotter. A chiller takes water in and sends it out cooler. Neither knows what loop it's on.

3. **Decoupled Controls** — Controls are a layer separate from the components: they read the system state and write setpoints, flows and on/off commands. Each air-loop system type has its own control sequence in `openbse-airloop`.

## What's Implemented

Each model is documented, with its equations, its EnergyPlus comparison and its known limitations, in the [Engineering Reference](docs/engineering-reference/). Open limitations are tracked as [issues](https://github.com/bbrannon4/OpenBSE/issues).

### Site and weather
- **Weather files** — EPW and TMY3 CSV, linearly interpolated within the hour; malformed or short files stop the run with an error that names the record
- **Design days** — ASHRAE daily temperature profile with clear-sky or tau solar, and optional generated monthly cooling design days
- **Solar position** — Spencer (1971) with the equation of time; Perez (1990) anisotropic sky diffuse (isotropic, circumsolar and horizon brightening), matching E+ `AnisoSkyViewFactors`
- **Sky temperature** — from the weather file's horizontal infrared, as in EnergyPlus; Clark & Allen sky emissivity with an opaque-cloud correction when the file has none (design days, TMY3)
- **Ground temperatures** — monthly table or the Kusuda-Achenbach model from weather data
- **Psychrometrics** — Hyland & Wexler (1983) moist-air properties at site pressure

### Building envelope
- **Materials and constructions** — multi-layer opaque constructions, massless layers, simple U-factor constructions, F-factor ground floors
- **Vertex geometry** — 3D polygon surfaces with automatic area, azimuth, tilt and zone volume; interzone surfaces; internal mass; zone multipliers
- **Conduction** — Seem (1987) state-space conduction transfer functions matching EnergyPlus, with a lumped RC element for single-node constructions
- **Convection** — TARP natural convection inside; DOE-2 (MoWiTT) with roughness correction outside
- **Interior long-wave exchange** — grey-body ScriptF exchange between a zone's surfaces from their view factors
- **Windows** — U-factor/SHGC glazing with the E+ SimpleGlazingSystem angular model (LBNL-2804E), a glass-node heat balance, and interior shades and blinds triggered by irradiance or zone temperature with hysteresis
- **Interior solar distribution** — FullExterior or FullInteriorAndExterior (beam projected onto the inside surfaces it reaches), with diffuse and reflected solar redistributed by area and absorptance
- **External shading** — overhangs, fins and shading surfaces with polygon-clipped beam shadows and sky view factor reduction; solar precomputed and cached to disk
- **Infiltration and ventilation** — E+ design-flow-rate infiltration, the ASHRAE combined model, scheduled and natural ventilation
- **Airflow network** — multizone pressure network generated from the geometry (cracks, openings, stack and wind pressures with Swami & Chandra Cp), duct leakage, and passive species such as CO₂
- **Internal gains** — people, lights and equipment with radiant, convective, latent and lost fractions
- **Zone air balance** — third-order backward-difference predictor-corrector for temperature and humidity (E+ `ZoneTempPredictorCorrector`), with optional linear room-air stratification
- **Thermal comfort** — Fanger PMV/PPD (ISO 7730 / ASHRAE 55), operative and mean radiant temperature, direct-beam solar MRT correction (ASHRAE 55 SolarCal), and a spatial comfort grid within a zone

### Air-side components
- **Fans** — constant volume, variable volume with a part-load power curve, and on/off cycling; ASHRAE Standard 205 fan data
- **Heating coils** — electric, gas, hot water (UA sized on the loop's water) and air-source heat pump with defrost and supplemental heat
- **Cooling coils** — single-speed and multi-speed DX with E+ curves, bypass-factor wet-coil model and part-load cycling penalty; chilled water; ASHRAE Standard 205 DX data
- **Ground-source heat pump** — water-to-air heat pump on the ground temperature, with EIR curves and cycling penalty
- **Heat recovery** — enthalpy wheel (sensible and latent) and sensible plate exchanger, with exhaust-flow ratio and economizer bypass
- **Humidifier** — steam, including its sensible heat, standby power and water use
- **Evaporative coolers** — direct, indirect and two-stage, with pump and fan power and water use
- **Ducts** — conduction and leakage to an ambient zone, outdoors or ground

### Zone equipment and terminals
- **VAV boxes** — dual-maximum reheat (electric or hot water) passing the flow the loop allocates; parallel fan-powered boxes; dual-duct boxes with parallel hot and cold decks
- **Fan coils** — load-based, with E+ `ASHRAE90VariableFan` (variable fan speed) or `CyclingFan` control; they meet the load left by the other loops serving the zone
- **PTAC and PTHP** — packaged terminal units cycling to the zone load
- **Variable refrigerant flow** — outdoor unit with capacity and EIR curves, minimum part-load cycling and heat recovery; indoor units with a wet-coil latent split
- **Radiant panels** — hot-water, chilled-water and electric panels with a radiant fraction
- **CRAC and CRAH** — data-center units with contained hot-aisle return (IT heat counted once), variable-speed airflow and dehumidification
- **Ideal loads** — zone equipment, alone or beside other systems in other zones

A water-source heat pump (`wshp`) is rejected as a severe input error: it isn't yet connected to a water loop ([#361](https://github.com/bbrannon4/OpenBSE/issues/361)).

### Plant
- **Plant loops** — ordered by their dependencies (inter-loop heat exchangers, condensers), so primary/secondary/tertiary arrangements and condenser loops solve in one timestep; sequential or equal-split equipment staging
- **Pumps** — constant and variable speed, E+ default curves, headered staging, pump heat into the loop
- **Boilers** — efficiency curves and part-load limits
- **Chillers** — E+ EIR and reformulated EIR, air- and water-cooled, condenser heat recovery and minimum unloading; ASHRAE Standard 205 chillers from their performance map
- **Cooling towers** — single, two and variable speed with E+ fan cycling logic; capacity limited by a minimum approach to wet-bulb, with no Merkel or approach model ([#252](https://github.com/bbrannon4/OpenBSE/issues/252))
- **Water-to-water heat exchangers** — fixed effectiveness, always on or as a waterside economizer
- **Thermal storage** — chilled-water or ice storage with full-storage, load-leveling and demand-limiting strategies
- **Service water heating** — gas and electric storage tanks with burner cycling and losses to their zone; heat-pump water heaters with E+ curves, evaporator cooling and drying of their zone, and a backup element

### Controls
- **Thermostats and schedules** — hourly setpoint schedules, day types including holidays, seasons, and `"value until HH:MM"` schedules
- **PSZ-AC** — single-zone packaged unit: cycling to the zone load (`cycling: on_off`, E+ `UnitarySystem` part load) or proportional heating with a steady offset (`cycling: proportional`); cycling, continuous or no-load-off fans; single-zone supply-temperature reset (E+ PSZ-VAV); a multi-zone PSZ controls from its first zone
- **VAV** — warmest-zone supply-temperature reset (E+ `SetpointManager:Warmest`), dual-maximum boxes, ASHRAE 62.1 ventilation rate procedure, preheat
- **DOAS** — 100% outdoor air, always on, its flow from the served zones' `outdoor_air` and its supply temperatures from the loop's controls
- **Economizers** — fixed and differential dry-bulb and enthalpy, with high limits and compressor lockout
- **Resets** — outdoor-air resets for supply air and plant loops; demand (trim and respond) supply-temperature reset
- **Data centers** — IT load with hot-aisle return, UPS and transformer losses, ASHRAE 90.4 PUE and mechanical load component

### Sizing
- **Design-day autosizing** — zone peaks from ideal-loads design-day runs with sizing factors; coincident or non-coincident multizone air flows; coils sized at the mixed-air state; plant from loop design loads; a severe error for any `autosize` value left unresolved
- **Zone multipliers** — one instance simulated and sized; energy and plant demand multiplied once

### Simulation and outputs
- Graph-based execution order (topological sort); multi-loop coupled envelope and HVAC iteration
- Sub-hourly timesteps (up to 60 per hour)
- Co-simulation of external air and plant components through a subprocess protocol
- Configurable CSV output with flexible variable selection and energy end uses
- Summary reports in text, HTML and CSV: annual and monthly end uses, per-zone peak loads, sizing results, and unmet hours counted for the building (an hour is unmet if any conditioned zone is more than 0.2 K outside the setpoint the HVAC controlled to) beside each zone's own hours
- Built-in energy-conservation and sanity checks every run: zone air heat balance, plant- and air-loop closure, surface temperature limits, HVAC convergence and unphysical component outputs, reported in the `.err` file
- Strict input checking: unknown keys, undefined references, unphysical values, `autosize` fields nothing sizes and malformed weather files stop the run with an error that names the problem

## Validation

### ASHRAE Standard 140-2023

Test cases are in [`140_tests/`](140_tests/). The building-envelope and HVAC-equipment test suites are implemented — 96 cases across four sections. Current status: **all 550 gate checks pass**.

| Section | Cases | Gate checks passing |
|---|---|---|
| 7 — Building thermal envelope and fabric loads (600 and 900 series) | 27 | 63 / 63 |
| 9A — Cooling equipment, analytical verification (CE100–CE200) | 14 | 112 / 112 |
| 9B — Cooling equipment, dynamic comparative (CE300–CE545) | 20 | 231 / 231 |
| 10 — Heating equipment, fuel-fired furnace (HE100–HE230) | 11 | 48 / 48 |
| 11 — Air-side HVAC equipment (AE101–AE445) | 24 | 96 / 96 |

The gate runs in CI on every commit. Its ranges are taken from the standard's analytical solutions and example results; how each was derived is recorded in [`acceptance_ranges_140_2023.json`](140_tests/acceptance_ranges_140_2023.json).

Against the standard's software acceptance criteria (Normative Annex A3), the v0.7.0 submission's results are within range on all 114 range cases: thermal fabric low mass 21 of 21, thermal fabric high mass 19 of 19, cooling equipment 14 of 14, heating equipment 6 of 6, and air-side equipment 54 of 54. See [`140_tests/README.md`](140_tests/README.md) for the breakdown.

Not yet implemented: weather drivers (Section 6), the Section 7 in-depth diagnostic cases (195–470, 800, 810), ground-coupled slab-on-grade (Section 8), and the Class II procedures (Section 12).

### DOE Prototype Comparison (EnergyPlus)

DOE prototype building models are compared with EnergyPlus, end use by end use, using the IDFs in [`prototype_tests/`](prototype_tests/). The goal is every end use within 5%, with the model's inputs matching the IDF rather than tuned to close a gap.

**Large Office** ([`LargeOffice_Boulder.yaml`](prototype_tests/large_office/LargeOffice_Boulder.yaml)) recreates the DOE 90.1-2019 IDF floor by floor, without zone multipliers, and runs on the same Denver-Aurora-Buckley TMY3 weather. Annual end uses against EnergyPlus 25.2 on [`eplus_run/in.idf`](prototype_tests/large_office/eplus_run/in.idf):

| End use | EnergyPlus [kWh] | OpenBSE [kWh] | Difference |
|---|---:|---:|---:|
| Heating (gas) | 472,503 | 455,946 | −3.5% |
| Cooling | 589,511 | 589,937 | +0.1% |
| Fans | 1,042,617 | 1,038,790 | −0.4% |
| Pumps | 135,044 | 99,990 | −26.0% |
| Interior lighting | 1,558,539 | 1,558,539 | 0.0% |
| Exterior lighting | 279,583 | 279,273 | −0.1% |
| Interior equipment | 4,076,778 | 4,076,725 | 0.0% |
| Exterior equipment | 713,075 | 713,075 | 0.0% |
| Service hot water | 125,133 | 125,146 | 0.0% |

The pump gap is open. The inputs are checked object by object against the IDF in [`INPUT_AUDIT.md`](prototype_tests/large_office/INPUT_AUDIT.md), which also lists what OpenBSE can't yet express.

**Hospital, Apartment and Single Family:** comparisons are in progress. Hospital heating is well below EnergyPlus.

Open gaps are tracked as [issues](https://github.com/bbrannon4/OpenBSE/issues).

## Roadmap

Planned features, known bugs, and validation tasks are tracked as [GitHub Issues](https://github.com/bbrannon4/OpenBSE/issues). Key areas:

- **Envelope**: geometry import, moisture transport (HAMT), multi-layer glazing, C-factor walls, ground heat transfer models
- **HVAC**: water-source heat pumps on a water loop, cooling tower heat-transfer model, absorption chillers, district energy, unit ventilators, chilled beams
- **Controls**: night cycle and unoccupied setbacks, humidity control, EMS-style programmable logic
- **Validation**: close the remaining prototype end-use gaps against EnergyPlus; the bugs the built-in energy checks have found
- **Infrastructure**: expanded output variables, parametric engine, results viewer, Python bindings

Issues labeled [`good first issue`](https://github.com/bbrannon4/OpenBSE/issues?q=label%3A%22good+first+issue%22) are good starting points.

**Contributing:** OpenBSE is dual-licensed (see below), so pull requests from outside contributors can't be merged until a contributor licence agreement is in place. Bug reports and ideas are very welcome as [issues](https://github.com/bbrannon4/OpenBSE/issues).

## Key Files for Contributors

If you're contributing (or pointing an AI tool at this repo), start here:

| What you need | Where to look |
|---|---|
| **Full project context for AI** | [`docs/AI_CONTEXT.md`](docs/AI_CONTEXT.md) — comprehensive overview, architecture, and conventions |
| **YAML input schema** | [`docs/openbse_schema.json`](docs/openbse_schema.json) — JSON Schema for all input fields |
| **Component traits (ports)** | [`crates/openbse-core/src/ports.rs`](crates/openbse-core/src/ports.rs) — `AirComponent`, `PlantComponent`, `EnvelopeSolver` traits, port types, sign conventions |
| **All HVAC components** | [`crates/openbse-components/src/`](crates/openbse-components/src/) — fans, coils, terminals, VRF, boilers, chillers, pumps, etc. |
| **YAML parsing + model structure** | [`crates/openbse-io/src/input.rs`](crates/openbse-io/src/input.rs) — `ModelInput` and all input structs |
| **Output variables + CSV writing** | [`crates/openbse-io/src/output.rs`](crates/openbse-io/src/output.rs) — `OutputSnapshot`, available variables |
| **Simulation driver** | [`crates/openbse-cli/src/main.rs`](crates/openbse-cli/src/main.rs) — timestep loop, HVAC iteration, plant dispatch, energy accounting |
| **Air-loop controls** | [`crates/openbse-airloop/src/lib.rs`](crates/openbse-airloop/src/lib.rs) — control signals for each system type |
| **Physics and equations** | [`docs/engineering-reference/`](docs/engineering-reference/) — every model's equations, EnergyPlus comparison and known limitations |
| **Envelope heat balance** | [`crates/openbse-envelope/src/heat_balance.rs`](crates/openbse-envelope/src/heat_balance.rs) — surface and zone heat balances, solar, convection, radiation |
| **CTF conduction** | [`crates/openbse-envelope/src/ctf.rs`](crates/openbse-envelope/src/ctf.rs) — Seem state-space method |
| **Prototype validation process** | [`prototype_tests/PROTOTYPE_VALIDATION_GUIDE.md`](prototype_tests/PROTOTYPE_VALIDATION_GUIDE.md) — step-by-step E+ comparison methodology |
| **Example models** | [`examples/`](examples/) — simple YAML models for various system types |

### Typical workflow for implementing a new feature

1. Read the relevant Issue on GitHub for context and scope
2. Read [`docs/AI_CONTEXT.md`](docs/AI_CONTEXT.md) for project architecture
3. Check existing components in `crates/openbse-components/src/` for patterns to follow
4. Implement, add tests, run `cargo fmt --all && cargo clippy --workspace && cargo test --workspace`

## Test Organization

OpenBSE has four categories of tests:

1. **Unit tests** — Inline `#[cfg(test)]` modules in Rust source files. Run with `cargo test --workspace`.
2. **ASHRAE 140 validation** — Standard test cases in [`140_tests/`](140_tests/). Run individual cases with `./target/release/openbse 140_tests/cases/ashrae140_case600.yaml`.
3. **E+ prototype comparison** — DOE prototype models in [`prototype_tests/`](prototype_tests/). Each model has a YAML input file and Python comparison scripts.
4. **Shipped-model checks** — every model in `examples/` and `prototype_tests/` runs in CI with sanity gates (zone temperatures, NaN/inf, negative capacities or part-load ratios, unmet hours) and committed end-use baselines. Run locally with `python3 tests/run_shipped_models.py`; see [`tests/README.md`](tests/README.md), including how to update baselines when results change on purpose.

## Development

Before committing any Rust changes, always run:

```bash
cargo fmt --all            # auto-format code
cargo clippy --workspace   # check for common mistakes
cargo test --workspace     # run all tests
```

`cargo fmt` enforces a consistent style automatically — just run it and commit the result. CI will reject pushes where formatting is off.

`cargo clippy` is a linter that catches common issues (unnecessary clones, unused variables, loops that could be simpler, etc.). CI runs clippy on every push; warnings are reported but don't fail the build. When writing new code, aim for zero clippy warnings.

For the editor frontend (`tools/editor/`):

```bash
cd tools/editor
npm ci                     # install dependencies
npx tsc -b                 # type-check TypeScript
npm run build              # full build (tsc + vite)
```

## License

Copyright (C) 2026 bbrannon4 and contributors.

OpenBSE is licensed under the [GNU Affero General Public License v3.0](LICENSE) (`AGPL-3.0-only`). You may use it for any purpose, including paid work. If you distribute OpenBSE or a modified version, or let others interact with a modified version over a network, you must make the corresponding source available under the same license.

Versions up to and including v0.7.0 were released under `MIT OR Apache-2.0` and remain available under those terms.

## Commercial Licensing

This project is licensed under AGPL-3.0. If you wish to use this software in a commercial or proprietary environment without being bound by copyleft requirements, contact [@bbrannon4 on GitHub](https://github.com/bbrannon4) for commercial licensing options.

## References

- EnergyPlus Engineering Reference and source code: https://github.com/NREL/EnergyPlus
- Psychrometrics: Hyland & Wexler (1983)
- CTF: Seem (1987)
- Exterior Convection: DOE-2 MoWiTT model
- Interior Convection: Walton (1983)
- Solar Position: Spencer (1971)
- Sky Emissivity (without weather-file infrared): Clark & Allen (1978)
- Anisotropic Sky Diffuse: Perez et al. (1990)
- Window Angular Properties: LBNL-2804E (Curcija et al.)
- ASHRAE Standard 140-2023
- California Simulation Engine (CSE): https://github.com/cse-sim/cse
