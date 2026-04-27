# OpenBSE Project Status

Last updated: 2026-04-27 (v0.2.11)

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
- **VRF**: Variable refrigerant flow with `VrfOutdoorUnit` + per-zone `VrfIndoorUnit`. Heat recovery mode, performance curves f(T_outdoor, T_indoor), proportional PLR capacity limiting.
- **Dual-duct CAV**: `system_type: dual_duct` — hot and cold deck AHU with per-zone `DualDuctBox` mixing terminals. Constant total zone flow, load-proportional deck blending.
- **PTHP**: Packaged terminal heat pump with ON/OFF cycling, heat pump heating + DX cooling.
- **GSHP**: Ground-source heat pump with Kusuda-Achenbach ground temp model (auto/EPW monthly/user monthly).

### HVAC Components
- Fans: constant volume, VAV (with part-load curves), on/off
- Heating coils: electric, gas (with burner efficiency), hot water, air-source heat pump (with defrost), water-source heat pump
- Cooling coils: DX single-speed with performance curves (Cap-fT, EIR-fT, PLF-fPLR); multi-speed and variable-speed DX
- Evaporative coolers: direct (adiabatic), indirect (sensible-only), two-stage; `type: evap_cooler` in air-loop equipment
- Ducts: NTU conduction model with leakage and ambient zone coupling
- Heat recovery: enthalpy wheel and plate heat exchangers
- Boilers: hot water with PLR efficiency curves, leaving-setpoint-modulated flow
- Chillers: air-cooled with COP and capacity modeling; lead/lag sequencing (sequential/equal_split)
- Cooling towers: single/two/variable-speed, effectiveness-NTU, polynomial fan curves
- Water-to-water heat exchangers: plate-and-frame inter-loop HX (always-on and economizer modes)
- Thermal energy storage: chilled-water and ice types with full_storage/load_leveling/demand_limiting strategies
- Radiant panels: fin-tube, chilled ceiling, electric; radiant/convective split, surface MRT distribution
- Plant loop topological ordering: arbitrary inter-loop dependencies via HX and condenser connections

### Controls
- Zone thermostats with occupied/unoccupied setpoints
- Supply air temperature control with OA-based and demand-based reset (`cooling_sat_reset`, `heating_sat_reset`)
- Plant loop setpoint reset: OA-based CHW/HHW reset (`setpoint_reset` on plant loops)
- Economizer controls (differential dry bulb, fixed dry bulb, differential enthalpy, fixed enthalpy, enthalpy+high-limit)
- On/off and proportional cycling methods
- Minimum outdoor air damper position
- Availability schedules for system on/off
- Humidity-based controls: max/min RH setpoints, dehumidification and humidification modes

### Outputs
- CSV output files at timestep/hourly/daily/monthly/run-period frequency
- Aggregation modes: mean, sum, min, max
- Summary reports in three formats: text, HTML (styled tables, ASHRAE compliance), and structured CSV
- Monthly energy end-use breakdown (14 categories × 12 months), per-zone peak loads summary (TRACE-style)
- Peak loads with coincident conditions (outdoor temp, zone temp, wind speed) in text and HTML reports
- 16 energy end-use timeseries output variables (fan, cooling, heating, pump, etc. by fuel type)
- Per-component output variables via `ComponentName:variable` pattern (electric_power, thermal_output, PLR, COP, water temps, etc.)
- Zone gain breakdown: 14 individual gain categories (people, lighting, equipment, infiltration, ventilation, nat vent, solar, HVAC — each sensible/latent)
- Comfort metrics: mean radiant temperature, operative temperature
- Unmet hours time-series: per-zone per-timestep heating/cooling unmet flags
- Submeter tagging on all energy-consuming components (lights, equipment, fans, coils, boilers, chillers, pumps, DHW, exterior) with per-submeter time-series output variables and summary report breakdown
- Custom output variable selection
- CLI `-w` flag for weather file (overrides YAML `weather_files`)

### Editor (Desktop Workbench)
- **Energy End-Use Dashboard** — ⚡ Energy tab with monthly stacked bar chart and annual summary table of electricity/gas/district end uses; zone-level unmet hours breakdown. Auto-detects site-level `*_kwh` columns from loaded CSV.
- **HVAC Network expanded** — Network view renders `evap_cooler`, `vrf_outdoor_unit`, `vrf_indoor_unit`, `radiant_panel` (air-side) and `thermal_storage`, `gshp` (plant-side); `dual_duct_box` labeled correctly. All new types have distinct colors and SVG icons.
- **View Report** — Results toolbar button opens `*_summary.html` in the default browser; shows inline message if no report is found.

### Performance
- Solar precompute with disk persistence (`.solar` cache file next to YAML input, geometry-fingerprinted)
- Rayon-parallelized solar precompute (embarrassingly parallel across timesteps)

### Tests
- 400+ unit tests across all crates (all component tests pass; 2 pre-existing envelope solar test failures)
- 8 example YAML files covering all system types
- 27 ASHRAE 140 validation cases in 140_tests/
- DOE prototype comparisons in prototype_tests/

---

## Roadmap

Planned features, bugs, and validation tasks are tracked as [GitHub Issues](https://github.com/bbrannon4/OpenBSE/issues).

### Recently completed
- Setpoint reset controls — OA-based and demand-based SAT reset; OA-based plant loop CHW/HHW reset
- Evaporative cooling — direct (adiabatic), indirect (sensible), two-stage modes
- Thermal energy storage — chilled_water and ice types, full_storage/load_leveling/demand_limiting strategies
- Dual-duct CAV system type with DualDuctBox mixing terminals, autosizing, parametric support
- Ground-source heat pump (GSHP) with Kusuda-Achenbach, EPW monthly, and user monthly ground temp options
- Table-lookup performance curves (N-linear interpolation, named axes, slot validation)
- VRF systems (VrfOutdoorUnit + VrfIndoorUnit per zone, heat recovery mode, performance curves)
- Radiant panels (fin-tube, chilled ceiling, electric) with radiant/convective split and surface distribution
- HAMT coupled heat-and-moisture transport (activated per-construction when moisture material data provided)
- DX coil dehumidification (autocalculate_shr default true; supply humidity wired to zone moisture balance)
- Humidity-based controls (max/min_relative_humidity setpoints, dehumidification and humidification modes)
- Multi-speed and variable-speed DX coils (CoolingCoilDXMultiSpeed, per-speed performance curves)
- Water-source heat pump (WaterSourceHeatPump, plant loop condenser connection, energy-balanced)
- Chiller lead/lag sequencing (StagingMode: sequential/equal_split, staging_threshold)
- PTHP system type (heat pump heating + DX cooling, ON/OFF PLR cycling)
- Advanced economizer modes: fixed enthalpy, enthalpy + high-limit; fixed differential enthalpy bug
- ComponentKind enum for type-safe energy accounting on all components
- Inter-floor slab solar interaction fix (Single Family: heating error +18% → +4.8%)
- Zone air balance diagnostic output variables (q_surf_conv_*, q_infiltration_sensible, q_thermal_mass)
- Per-surface convection outputs (conv_to_zone, h_conv_inside)
- docs/CONTRIBUTING_COMPONENTS.md — guide for adding new physics components
- Zone moisture (humidity ratio) balance with 3rd-order BDF integration
- Air-source heat pump heating coil with defrost and performance curves
- Condenser water loops and cooling towers (YAML parsing, topological loop ordering, autosize)
- Pumps — constant/variable speed, headered staging, power curves
- Full state-space CTF — Seem (1987) method matching EnergyPlus
- Parametric run execution (scalar overrides, weather swaps, sweep expansion)
- Airflow network — multizone pressure-driven infiltration (Newton-Raphson, auto-generated cracks)
- Boiler PLR efficiency curves, leaving-setpoint-modulated flow mode
- Hot water coil UA-LMTD (NTU-effectiveness) model
- Mains water temperature sinusoidal correlation

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
- Rust source files: ~46
- Example YAML files: 11
- ASHRAE 140 test cases: 28 (+4 test variants)
- Unit tests: 400+
