# OpenBSE — Open Building Simulation Engine

**Modern, open-source building energy simulation in Rust.**

OpenBSE is a ground-up reimplementation of building energy simulation using validated physics (heat balance methods, psychrometrics, HVAC component performance curves) expressed in a clean, modern, composable architecture. It replaces the complexity of legacy BEM tools with:

- **Clean YAML input format** — no nodes, branches, branch lists, or connector lists
- **Graph-based simulation engine** — automatically determines execution order via topological sort
- **Decoupled controls framework** — sensor/actuator model (sense → compute → act)
- **Full building envelope physics** — CTF, convection, solar, longwave radiation, infiltration, internal gains
- **Generic component contracts** — any component can go on any loop if fluid types match
- **Multi-format weather support** — EPW and TMY3 CSV (ASHRAE Standard 140 prescribed format)

## Why This Exists

EnergyPlus is the DOE's flagship building energy simulation tool. Its physics are well-validated, but its software architecture — inherited from 1970s-era BLAST and DOE-2 Fortran — creates serious usability problems: redundant HVAC topology specification, rigid system templates, simulation order baked into user input, and controls tightly coupled to specific system types.

OpenBSE preserves the validated physics while fundamentally rethinking the system topology, HVAC modeling framework, and user input paradigm. EnergyPlus math is the reference standard — every algorithm is implemented to match E+ behavior, and deviations are treated as bugs.

> **For AI assistants**: See [`docs/AI_CONTEXT.md`](docs/AI_CONTEXT.md) for a comprehensive project overview designed as initial context.

## Quick Start

### Build

```bash
git clone https://github.com/bbrannon4/OpenBSE.git
cd OpenBSE
cargo build --release
```

### Run Tests

```bash
cargo test --workspace
```

**82 tests** across 8 crates (81 passing, 1 pre-existing VAV box test failure being investigated).

### Run a Simulation

```bash
./target/release/openbse examples/simple_heating.yaml -o results.csv
```

## Architecture

OpenBSE is a Rust workspace with 8 crates:

| Crate | Purpose |
|-------|---------|
| `openbse-psychrometrics` | Moist air property calculations (Hyland & Wexler) |
| `openbse-weather` | EPW and TMY3 CSV weather file parsing |
| `openbse-core` | Simulation graph, timestep loop, component traits |
| `openbse-components` | HVAC component models (fan, coils, boiler, chiller, duct) |
| `openbse-controls` | Decoupled control framework (thermostats, setpoints) |
| `openbse-envelope` | Building envelope heat balance physics |
| `openbse-io` | YAML parsing, CSV output, design day sizing, summary reports |
| `openbse-cli` | Command-line interface and multi-loop control dispatcher |

No circular dependencies. Components implement traits (`AirComponent`, `PlantComponent`, `EnvelopeSolver`) defined in `openbse-core`. Rust's type system enforces physical constraints at compile time — `AirPort` and `WaterPort` are distinct types, so connecting a water pipe to an air duct won't compile.

### Core Design Principles

1. **Graph-Based Topology** — Users describe components and connections. The engine builds the directed graph, creates internal nodes, and determines simulation order automatically.

2. **Generic Component Contracts** — Every component implements a simple trait. A boiler takes water in and sends it out hotter. A chiller takes water in and sends it out cooler. Neither knows what loop it's on.

3. **Decoupled Controls** — Controllers read from any sensor point in the graph and write to any actuator on any component, independent of system topology.

## What's Implemented

### Building Envelope
- **Materials & Constructions** — multi-layer opaque walls, U-factor windows with E+ SimpleGlazingSystem angular model (LBNL-2804E), simple constructions, F-factor ground floors
- **Vertex Geometry** — 3D polygon vertices with automatic area, azimuth, tilt, volume calculation
- **CTF** — full Seem (1987) state-space conduction transfer functions matching EnergyPlus, with NoMass layer support and lumped RC fallback
- **Exterior Convection** — DOE-2 (MoWiTT) algorithm with roughness correction
- **Interior Convection** — ASHRAE/Walton natural convection correlations
- **Solar** — position (Spencer 1971), Hay-Davies anisotropic sky model (circumsolar + isotropic), angular SHGC transmission (28-bin mapping per LBNL-2804E), FullExterior and FullInteriorAndExterior distribution with beam/diffuse split and VMULT redistribution
- **External Shading** — overhangs and fins with geometric beam shadow calculation (Sutherland-Hodgman polygon clipping), diffuse sky view factor reduction, 8x8 grid sampling for multi-caster union
- **Sky Longwave Radiation** — Berdahl-Martin sky emissivity model with cloud cover correction
- **Interior Longwave Radiation** — MRT-based surface radiation exchange
- **Infiltration** — EnergyPlus design flow rate model with wind dependence, ASHRAE combined infiltration model
- **Internal Gains** — people, lights, equipment with radiant/convective/lost fraction split
- **Zone Air Balance** — 3rd-order backward difference predictor-corrector (matching E+ ZoneTempPredictorCorrector)
- **Ideal Loads** — nonproportional thermostat for ASHRAE 140 validation
- **Ground Temperature** — monthly table or Kusuda-Achenbach model from weather data
- **Thermostat Schedules** — time-of-day heating/cooling setpoint variation
- **Conditional Night Ventilation** — schedule-driven with temperature conditions
- **Free-Float Mode** — no-HVAC simulation for temperature drift analysis

### HVAC Components
- **Fan** — constant volume, VAV with power curve, on/off cycling
- **Heating Coil** — electric resistance, hot water, gas burner (with efficiency)
- **Cooling Coil** — DX single-speed with COP, performance curves (Cap-fT, EIR-fT, PLF-fPLR), outdoor temp derating
- **Heat Pump Coil** — air-source heat pump heating with defrost and performance curves
- **Duct** — NTU conduction model with leakage fraction and ambient zone coupling
- **Boiler** — efficiency curves, part-load ratio limits
- **Chiller** — electric chiller with COP and condenser modeling
- **Cooling Tower** — single-speed with approach temperature (component written, plant loop wiring pending)
- **Heat Recovery** — sensible effectiveness heat exchanger (air-side integrated, plant loop wiring pending)
- **Pump** — constant/variable speed with power curves, headered staging, heat-to-fluid modeling

### Controls
- **PSZ-AC** — ASHRAE 90.1 modulating economizer, on/off and proportional cycling, gas furnace with fixed heating DAT
- **DOAS** — 100% outdoor air, fixed supply setpoints, always-on pre-conditioning
- **FCU** — per-zone recirculating fan coil, proportional fan speed modulation
- **VAV** — ASHRAE Guideline 36 dual-maximum, SAT reset (13-18°C), modulating economizer, preheat frost protection
- **Design Day Autosizing** — two-stage ASHRAE-compliant sizing (zone peak + system coincident), configurable oversize factors

### Simulation
- Graph-based execution order (topological sort)
- Sub-hourly timesteps (1, 2, 4, 6, 10, 12, 15, 20, 30, 60 per hour)
- Multi-weather-file support (EPW and TMY3 CSV)
- Configurable CSV output with flexible variable selection
- Summary reports with monthly energy, peak loads, unmet hours
- Multi-loop coupled envelope + HVAC simulation (DOAS + FCU additive mixing)

## Validation

### ASHRAE Standard 140-2023

Test cases are in [`140_tests/`](140_tests/). 27 cases from Section 7 (Building Thermal Envelope) plus Case CE100 (Cooling Equipment) have been implemented. Current status: **48 of 63 metrics pass** (76.2%) against the standard's prescribed acceptance ranges.

Cases 600, 610, 620, 630 (low-mass) pass all metrics. Case 900 series (high-mass) has some failures attributed to the simplified wall construction model.

### DOE Prototype Comparison (EnergyPlus)

DOE prototype building models are being validated against EnergyPlus using the simplified IDFs in [`prototype_tests/`](prototype_tests/). Current status for the Single-Family house (CZ5B Boulder):

| End Use | EnergyPlus | OpenBSE | Diff |
|---------|-----------|---------|------|
| Heating (Gas) | 6,689 kWh | 9,907 kWh | +48% |
| Cooling (Elec) | 1,600 kWh | 1,817 kWh | +14% |
| Interior Lighting | 1,038 kWh | 1,038 kWh | 0% |
| Interior Equipment | 10,084 kWh | 10,077 kWh | 0% |
| Fans | 852 kWh | 1,179 kWh | +38% |
| DHW (Gas) | 2,158 kWh | 2,173 kWh | +1% |

Lighting, equipment, and DHW match within 1%. Heating and fan gaps are under active investigation — primary cause is the missing basement/garage zone modeling (E+ models 4 zones; our model has 2). Large Office, Hospital, and Mid-Rise Apartment prototypes are also in progress.

## What's Not Yet Implemented

### Envelope
- Geometry import (gbXML, IDF vertices)
- Moisture transport through envelope

### HVAC
- Plant loop wiring for cooling towers and heat recovery (component models written, loop integration pending)
- Multi-speed and variable-speed DX coils
- Dehumidification modeling in DX coils (currently sensible-only)
- Water-source heat pump models (air-source implemented)
- Chiller lead/lag sequencing
- VRF systems

### General
- Python bindings (PyO3)
- Parametric run execution (data structure defined, execution not wired)
- EMS-style programmable controls

## Test Organization

OpenBSE has three categories of tests:

1. **Unit tests** — Inline `#[cfg(test)]` modules in Rust source files. Run with `cargo test --workspace`.
2. **ASHRAE 140 validation** — Standard test cases in [`140_tests/`](140_tests/). Run individual cases with `./target/release/openbse 140_tests/cases/ashrae140_case600.yaml`.
3. **E+ prototype comparison** — DOE prototype models in [`prototype_tests/`](prototype_tests/). Each model has a YAML input file and Python comparison scripts.

## License

MIT OR Apache-2.0

## References

- EnergyPlus Engineering Reference and source code: https://github.com/NREL/EnergyPlus
- Psychrometrics: Hyland & Wexler (1983)
- CTF: Seem (1987)
- Exterior Convection: DOE-2 MoWiTT model
- Interior Convection: Walton (1983)
- Solar Position: Spencer (1971)
- Sky Emissivity: Berdahl & Martin (1984)
- Window Angular Properties: LBNL-2804E (Curcija et al.)
- ASHRAE Standard 140-2023
- California Simulation Engine (CSE): https://github.com/cse-sim/cse
