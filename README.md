# OpenBSE — Open Building Simulation Engine

**Modern, open-source building energy simulation in Rust.**

OpenBSE is a ground-up reimplementation of building energy simulation using validated physics (heat balance methods, psychrometrics, HVAC component performance curves) expressed in a clean, modern, composable architecture. It replaces the complexity of legacy BEM tools with:

- **Clean YAML input format** — no nodes, branches, branch lists, or connector lists
- **Graph-based simulation engine** — automatically determines execution order via topological sort
- **Decoupled controls framework** — sensor/actuator model (sense → compute → act)
- **Full building envelope physics** — CTF, convection, solar, longwave radiation, infiltration, internal gains
- **Generic component contracts** — any component can go on any loop if fluid types match
- **Multi-format weather support** — EPW and TMY3 CSV (ASHRAE Standard 140 prescribed format)
- **Built for AI and parametric workflows**

## Why This Exists

EnergyPlus is the DOE's flagship building energy simulation tool. Its physics are well-validated, but its software architecture — inherited from 1970s-era BLAST and DOE-2 Fortran — creates serious usability problems: redundant HVAC topology specification, rigid system templates, simulation order baked into user input, and controls tightly coupled to specific system types.

OpenBSE preserves the validated physics while fundamentally rethinking the system topology, HVAC modeling framework, and user input paradigm.

## Quick Start

### Build

```bash
cargo build --release
```

### Run Tests

```bash
cargo test --workspace
```

**180 tests, 0 warnings** across 8 crates.

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
| `openbse-components` | HVAC component models (fan, coils, boiler, chiller) |
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
- **Materials & Constructions** — multi-layer opaque walls, U-factor windows, simple constructions
- **Vertex Geometry** — 3D polygon vertices with automatic area, azimuth, tilt, volume calculation
- **CTF** — conduction transfer functions (lumped RC model)
- **Exterior Convection** — DOE-2 (MoWiTT) algorithm with roughness correction
- **Interior Convection** — ASHRAE/Walton natural convection correlations
- **Solar** — position (Spencer 1971), Hay-Davies anisotropic sky model (circumsolar + isotropic), Fresnel angular SHGC transmission
- **External Shading** — overhangs and fins with geometric beam shadow calculation (Sutherland-Hodgman polygon clipping), diffuse sky view factor reduction, 8x8 grid sampling for multi-caster union
- **Sky Longwave Radiation** — Berdahl-Martin sky emissivity model with cloud cover correction
- **Interior Longwave Radiation** — MRT-based surface radiation exchange
- **Infiltration** — EnergyPlus design flow rate model with wind dependence
- **Internal Gains** — people, lights, equipment with radiant/convective split
- **Zone Air Balance** — predictor-corrector with thermal capacitance
- **Ideal Loads** — nonproportional thermostat for ASHRAE 140 validation
- **Ground Temperature** — Kusuda-Achenbach model computed from weather data
- **Thermostat Schedules** — time-of-day heating/cooling setpoint variation
- **Conditional Night Ventilation** — schedule-driven with temperature conditions
- **Free-Float Mode** — no-HVAC simulation for temperature drift analysis

### HVAC Components
- **Fan** — constant volume, VAV with power curve
- **Heating Coil** — electric resistance, hot water
- **Gas Heating Coil** — burner efficiency modeling
- **Cooling Coil** — DX single-speed with COP, sensible heat ratio (SHR)
- **Boiler** — efficiency curves, part-load ratio limits
- **Chiller** — electric chiller with COP and condenser modeling
- **Cooling Tower** — single-speed with approach temperature (source written, integration pending)
- **Heat Recovery** — sensible effectiveness heat exchanger (source written, integration pending)
- **Pump** — constant/variable speed with power curve (source written, integration pending)

### Controls
- **PSZ-AC** — ASHRAE 90.1 modulating economizer, fixed heating DAT (35C), proportional cooling DAT
- **DOAS** — 100% outdoor air, fixed supply setpoints, always-on pre-conditioning
- **FCU** — per-zone recirculating fan coil, proportional fan speed modulation
- **VAV** — ASHRAE Guideline 36 dual-maximum, SAT reset (13-18C), modulating economizer, preheat frost protection
- **Design Day Autosizing** — two-stage ASHRAE-compliant sizing (zone peak + system coincident), auto-generated monthly cooling DDs

### Simulation
- Graph-based execution order (topological sort)
- Sub-hourly timesteps (1, 2, 4, 6, 10, 12, 15, 20, 30, 60 per hour)
- Multi-weather-file support (EPW and TMY3 CSV)
- Configurable CSV output with flexible variable selection
- Summary reports with monthly energy, peak loads, unmet hours
- Multi-loop coupled envelope + HVAC simulation (DOAS + FCU additive mixing)

## Validation

ASHRAE Standard 140-2023 validation cases are maintained in a separate repository (`OpenBSE-140_Tests`). Cases 600, 610, 620, 630, 900, 910, 920, and 930 have been tested against the standard's prescribed TMY3 weather data (Denver, WMO 725650). **14 of 16 metrics pass** (annual heating + cooling for all 8 cases). The two remaining failures are Case 900 cooling (12 kWh over max, <1%) and Case 910 cooling (168 kWh over max, attributed to simplified interior solar distribution).

## What's Not Yet Implemented

### Envelope
- Geometry import (gbXML, IDF vertices)
- Separate beam/diffuse interior solar distribution (beam geometric to floor, diffuse uniform)
- Neighboring-building shading
- Full state-space CTF (currently using lumped RC)

### HVAC
- Cooling tower integration into condenser water plant loop (component model written, loop wiring pending)
- Variable-speed pump integration into plant loops (component model written, loop wiring pending)
- Heat recovery integration into plant loops (component model written, loop wiring pending)
- OA damper scheduling (close OA outside occupied hours)
- Economizer lockout with heating (disable economizer during heating mode)
- Part-load boiler efficiency curves (EMS-style cubic PLR curves with skin loss)
- Chiller lead/lag sequencing with pump staging
- Optimum start/stop controls
- Moisture balance (latent loads, zone humidity tracking for humidifier control)

### General
- Python bindings (PyO3)
- Parametric run execution
- EMS-style programmable controls

## Phased Development Plan

1. **Phase 1: Foundation** — Core graph framework, psychrometrics, weather reader, simulation loop, output *(complete)*
2. **Phase 2: Envelope** — Surface heat balance, zone air balance, solar, infiltration, internal gains *(complete)*
3. **Phase 3: Envelope Validation** — Vertex geometry, external shading (overhangs/fins), ASHRAE 140 Cases 600/610/620/630/900/910/920/930 — 14/16 metrics passing *(complete)*
4. **Phase 4: Air-Side HVAC** — DX cooling coils, PSZ-AC, DOAS, FCU, VAV, design day autosizing *(complete)*
5. **Phase 5: Plant-Side HVAC** — Chillers, cooling towers, heat pumps, generic plant mixing *(partial: component models written, loop integration in progress)*
6. **Phase 6: Controls & Polish** — Advanced controls, Python API, documentation

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
- ASHRAE Standard 140-2023
- California Simulation Engine (CSE): https://github.com/cse-sim/cse
