# OpenBSE — AI Assistant Context

This document provides the context an AI assistant needs to work effectively on OpenBSE. Read this before making changes.

## What OpenBSE Is

OpenBSE is a building energy simulation engine written in Rust. It models heat transfer through building envelopes, HVAC system energy consumption, and zone temperature control on sub-hourly timesteps for full-year simulations.

**The reference standard is EnergyPlus.** Every physics algorithm is implemented to match E+ behavior. When our results diverge from E+, the divergence is treated as a bug in OpenBSE, not a difference of opinion. Do not invent or approximate physics — look up how E+ does it and implement that.

## Architecture

```
openbse-psychrometrics   ← No dependencies. Moist air properties (Hyland & Wexler).
openbse-weather          ← No dependencies. EPW and TMY3 weather file parsing.
openbse-core             ← Depends on psychrometrics. Traits (AirComponent, PlantComponent),
                            simulation graph, AirPort/WaterPort types.
openbse-components       ← Depends on core + psychrometrics. HVAC models: fan, coils,
                            boiler, chiller, duct, heat recovery, pump, VAV box, etc.
openbse-controls         ← Depends on core. Thermostats, setpoint controllers.
openbse-envelope         ← Depends on core + psychrometrics + weather. Zone heat balance,
                            surfaces, CTF, solar, infiltration, convection, radiation.
openbse-io               ← Depends on all above. YAML input parsing, CSV output, sizing.
openbse-cli              ← Depends on all above. Binary entry point, simulation driver,
                            system-level control logic (PSZ-AC, VAV, DOAS, FCU dispatchers).
```

### Key Traits

- **`AirComponent`** (`openbse-core/src/ports.rs`): `simulate_air(inlet) -> outlet`, `thermal_output()`, `power_consumption()`, `fuel_consumption()`. Implemented by fans, coils, ducts, etc.
- **`PlantComponent`** (`openbse-core/src/ports.rs`): `simulate_plant(inlet) -> outlet`. Implemented by boilers, chillers, pumps.
- **`AirPort`** / **`WaterPort`**: Type-safe fluid connectors. You cannot accidentally connect water to air — the compiler prevents it.

### Simulation Flow

1. **Sizing** (`openbse-io/src/sizing.rs`): Design day simulation to autosize equipment capacities and airflows.
2. **Annual simulation** (`openbse-cli/src/main.rs`): For each timestep:
   - Compute solar position and incident radiation
   - Run envelope heat balance (surface temperatures, zone loads)
   - Predictor: estimate zone temperature without HVAC
   - Determine HVAC mode (heating/cooling/deadband) from predicted temperature vs setpoints
   - Build control signals (DAT, airflow, PLR) in `build_psz_signals()` / `build_vav_signals()` etc.
   - Simulate HVAC components in graph order
   - Corrector: recompute zone temperature with actual HVAC output
   - Write outputs

### YAML Input → Rust Structs

- YAML is parsed by `openbse-io/src/input.rs` using serde
- Equipment is an enum `EquipmentInput` with variants for each component type
- `build_graph()` in `input.rs` instantiates Rust component structs from YAML
- When adding a new component: add a YAML variant to `EquipmentInput`, add a match arm in `build_graph()`, and handle it in the `component_names` extraction in `main.rs`

## File Organization

```
crates/                      Rust source code (8 crates)
examples/                    Example YAML model files (simple_heating, vav_reheat, etc.)
140_tests/                   ASHRAE Standard 140-2023 validation test cases
  cases/                       31 YAML input files
  weather/                     Prescribed weather data (Denver EPW)
  reference_idfs/              EnergyPlus IDFs for cross-reference
  scripts/                     Python validation scripts
  results/                     Aggregated pass/fail results
prototype_tests/             DOE prototype building comparisons vs EnergyPlus
  single_family/                 Residential house model + E+ run outputs
  large_office/                  Large office model + E+ run outputs
  hospital/                      Hospital model
  apartment/                     Mid-rise apartment model
  compare_end_uses.py            End-use comparison charts
docs/                        Documentation
  AI_CONTEXT.md                This file
  STATUS.md                    Feature status and TODO tracking
  user-guide/README.md         YAML input format reference
  engineering-reference/README.md  Physics algorithm documentation
```

## Validation Status (as of March 2026)

### ASHRAE 140-2023
- 27 test cases implemented (Section 7 thermal fabric + CE100 cooling equipment)
- **48 of 63 metrics pass** (76.2%)
- Case 600 series (low-mass): mostly passing
- Case 900 series (high-mass): some failures from simplified CTF model

### DOE Prototype Comparison (EnergyPlus)
- Single-Family house (CZ5B Boulder): Lighting/equipment/DHW within 1%. Heating +48%, cooling +14%, fans +38% — under active investigation. Primary cause: missing basement/garage zones.
- Large Office, Hospital, Mid-Rise Apartment: in progress

## Known Limitations (things that are NOT modeled)

Be explicit about these — do not guess or approximate:

- **No moisture balance**: Zone humidity is not tracked. DX coils are sensible-only. No dehumidification.
- **No latent loads**: Equipment latent fractions are parsed but not applied to zone humidity.
- **Simplified CTF**: Lumped RC model, not full state-space. Multi-layer constructions with insulation outside mass can have numerical issues.
- **No airflow network**: Infiltration uses constant design flow rates, not pressure-driven multizone airflow.
- **No geometry import**: Surfaces must be specified as 3D vertices in YAML. No gbXML/IDF/BIM import.
- **No VRF, ground-source heat pumps, or radiant systems**.
- **Plant loop integration incomplete**: Cooling towers, pumps, and heat recovery components exist but aren't wired into plant loop simulation yet.

## Conventions

- **E+ is always right.** When in doubt, check the EnergyPlus Engineering Reference or source code.
- **No hallucinated physics.** If you don't know the correct equation, say so. Don't guess coefficient values or make up correlations.
- **Unit tests verify against known references**, not arbitrary values. Each test should document what it validates and where the reference value comes from.
- **`psych::CP_AIR` is private** — use `openbse_psychrometrics::cp_air_fn_w(w)` instead.
- **VAV box test `test_vav_heating_mode_with_electric_reheat`** is a pre-existing failure — do not try to "fix" it by changing expected values without understanding the physics.
- **Weather files (*.epw) are gitignored** except the ASHRAE 140 prescribed weather file in `140_tests/weather/`.
