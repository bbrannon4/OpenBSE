# OpenBSE Project Status

Last updated: 2026-03-23

## What Works (Functional)

### Simulation Engine
- Single-zone and multi-zone heat balance with 3rd-order backward difference predictor-corrector
- ASHRAE Standard 140-2023: 28 cases implemented, 63/63 metrics pass (100%)
- Weather file processing (EPW/CSV format)
- Design-day autosizing for fans, coils, boilers, chillers with configurable oversize factors
- Annual, multi-month, and custom period simulations

### Building Envelope
- Opaque constructions (layered materials with conduction transfer functions)
- Simple constructions (U-factor + lumped thermal capacity)
- Window constructions (U-factor + SHGC, angular model matching E+ SimpleGlazingSystem/LBNL-2804E)
- Solar heat gains through windows (beam + diffuse, Hay-Davies anisotropic sky, angular SHGC with 28-bin mapping)
- FullExterior and FullInteriorAndExterior solar distribution with beam/diffuse split and VMULT redistribution
- External shading (overhangs and fins with geometric beam shadow calculation, diffuse sky view factor reduction)
- Ground-coupled floors (monthly table or Kusuda-Achenbach model, F-factor construction support)
- Adiabatic and interzone boundary conditions
- Surface area auto-calculation from 3D vertex coordinates

### HVAC Systems
- **PSZ-AC**: Packaged single-zone rooftop units with DX cooling + gas/electric heating, on/off and proportional cycling
- **VAV**: Variable air volume with central AHU, per-zone reheat boxes
- **DOAS**: Dedicated outdoor air systems (100% OA) with downstream fan coil units
- **FCU**: Fan coil units for zone-level heating/cooling

### HVAC Components
- Fans: constant volume, VAV (with part-load curves), on/off
- Heating coils: electric, gas (with burner efficiency), hot water
- Cooling coils: DX single-speed with performance curves (Cap-fT, EIR-fT, PLF-fPLR)
- Ducts: NTU conduction model with leakage and ambient zone coupling
- Heat recovery: enthalpy wheel and plate heat exchangers
- Boilers: hot water with efficiency and capacity control
- Chillers: air-cooled with COP and capacity modeling
- Cooling towers: single/two/variable-speed, effectiveness-NTU, polynomial fan curves
- Water-to-water heat exchangers: plate-and-frame inter-loop HX (always-on and economizer modes)
- Plant loop topological ordering: arbitrary inter-loop dependencies via HX and condenser connections

### Controls
- Zone thermostats with occupied/unoccupied setpoints
- Supply air temperature control
- Economizer controls (differential dry bulb, fixed dry bulb, differential enthalpy)
- On/off and proportional cycling methods
- Minimum outdoor air damper position
- Availability schedules for system on/off

### Outputs
- CSV output files at timestep/hourly/daily/monthly/run-period frequency
- Aggregation modes: mean, sum, min, max
- Summary reports in three formats: text, HTML (styled tables, ASHRAE compliance), and structured CSV
- Monthly energy end-use breakdown (14 categories × 12 months), per-zone peak loads summary (TRACE-style)
- 16 energy end-use timeseries output variables (fan, cooling, heating, pump, etc. by fuel type)
- Custom output variable selection
- CLI `-w` flag for weather file (overrides YAML `weather_files`)

### Performance
- Solar precompute with disk persistence (`.solar` cache file next to YAML input, geometry-fingerprinted)
- Rayon-parallelized solar precompute (embarrassingly parallel across timesteps)

### Tests
- 306+ unit tests across all crates (all component tests pass; 2 pre-existing envelope solar test failures)
- 8 example YAML files covering all system types
- 27 ASHRAE 140 validation cases in 140_tests/
- DOE prototype comparisons in prototype_tests/

---

## What Needs Work (TODO)

### High Priority
- [ ] Add basement and garage zones to SingleFamily model (primary cause of 48% heating gap vs E+)
- [ ] PLF (part-load fraction) curve support in cooling coil engine (currently hardcoded Cd=0.15)
- [ ] Chilled water cooling coil model (`source: chilled_water`)
- [ ] Connect heat recovery exhaust conditions to actual zone return air

### Medium Priority
- [x] Zone moisture (humidity ratio) balance with 3rd-order BDF integration (implemented)
- [ ] Dehumidification control (humidity-based setpoints, dedicated dehumidifier equipment)
- [ ] Multi-speed and variable-speed DX coils
- [x] Air-source heat pump heating coil (implemented with defrost and performance curves)
- [ ] Water-source heat pump models
- [x] Condenser water loops and cooling towers (fully wired: YAML parsing, topological loop ordering, autosize)
- [x] Pumps — constant/variable speed, headered staging, power curves (fully implemented)
- [x] Full state-space CTF — Seem (1987) method matching EnergyPlus (implemented)

### Lower Priority
- [ ] Geometry import (gbXML, IDF vertices)
- [x] Beam/diffuse interior solar distribution — FullExterior (beam→floor) and FullInteriorAndExterior (geometric projection with polygon clipping), VMULT diffuse redistribution. Reflected beam enters diffuse pool (deliberate improvement over E+'s single-bounce localization; approaches infinite-bounce radiosity solution). No direct solar-to-air fraction (E+ shortcut for unmodeled interior mass).
- [ ] Python bindings (PyO3)
- [ ] Parametric run execution
- [ ] VRF systems, ground-source heat pumps, radiant systems
- [x] Airflow network — multizone pressure-driven infiltration with Newton-Raphson solver, auto-generated cracks/openings from geometry, Swami & Chandra Cp correlations, stack effect, exhaust fan depressurization. Opt-in via `airflow_network.enabled: true`.
- [ ] Moisture transport through envelope

---

## Architecture

```
openbse-cli          # Binary: CLI entry point, simulation driver, system orchestration
openbse-io           # Input parsing (YAML), output writing (CSV/reports)
openbse-envelope     # Building envelope: zones, surfaces, materials, heat balance
openbse-components   # HVAC components: fans, coils, boilers, chillers, ducts, heat recovery
openbse-controls     # Controls: thermostats, setpoints, controllers
openbse-core         # Core types: simulation graph, air/water ports, time stepping
openbse-psychrometrics # Moist air property calculations
openbse-weather      # Weather file reading and processing
```

## File Counts
- Rust source files: ~42
- Example YAML files: 11
- ASHRAE 140 test cases: 27 (+4 test variants)
- Unit tests: 82
