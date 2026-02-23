# OpenBSE Project Status

Last updated: 2026-02-23

## What Works (Functional)

### Simulation Engine
- Single-zone and multi-zone heat balance (RC network) with timestep marching
- ASHRAE Standard 140-2023 Cases 600/610/620/630/900/910/920/930 validated (14/16 metrics pass)
- Weather file processing (EPW/CSV format)
- Design-day autosizing for fans, coils, boilers, chillers
- Annual, multi-month, and custom period simulations
- Multi-year parametric runs with weather file and parameter overrides

### Building Envelope
- Opaque constructions (layered materials with conduction transfer functions)
- Simple constructions (U-factor + lumped thermal capacity)
- Window constructions (U-factor + SHGC based)
- Solar heat gains through windows (beam + diffuse, Hay-Davies anisotropic sky model, Fresnel angular SHGC, interior distribution)
- External shading (overhangs and fins with geometric beam shadow calculation, diffuse sky view factor reduction)
- Ground-coupled floors
- Adiabatic and interzone boundary conditions
- Surface area auto-calculation from 3D vertex coordinates

### HVAC Systems
- **PSZ-AC**: Packaged single-zone rooftop units with DX cooling + gas/electric heating
- **VAV**: Variable air volume with central AHU, per-zone reheat boxes (FCU terminals)
- **DOAS**: Dedicated outdoor air systems (100% OA) with downstream fan coil units
- **FCU**: Fan coil units for zone-level heating/cooling
- System type auto-detection from equipment and controls configuration

### HVAC Components
- Fans: constant volume, VAV (with part-load curves), on/off
- Heating coils: electric, gas (with burner efficiency), hot water
- Cooling coils: DX single-speed with outdoor temperature derating
- Heat recovery: enthalpy wheel and plate heat exchangers
- Boilers: hot water with efficiency and capacity control
- Chillers: air-cooled with COP and capacity modeling
- Performance curves: biquadratic, quadratic, cubic, linear (reusable, top-level)

### Controls
- Zone thermostats with occupied/unoccupied setpoints
- Fixed setpoint controllers on coils
- Plant loop setpoint controllers
- Supply air temperature control from air loop controls section
- Economizer controls (differential dry bulb, fixed dry bulb, differential enthalpy)
- Cycling methods (proportional, on/off)
- Minimum outdoor air damper position
- Availability schedules for system on/off

### Zone Loads
- People (absolute count, per-area, per-person, with activity level)
- Lights (absolute power or W/m2, with radiant/return-air fractions)
- Equipment (absolute power or W/m2, with radiant fraction)
- Infiltration (design flow, ACH, per-floor-area, with wind/temperature coefficients)
- Ventilation (scheduled, per-person, per-area, with combining methods)
- Exhaust fans (scheduled)
- Outdoor air requirements (per-person, per-area)
- Ideal loads air systems (for unconditioned-zone testing)

### Schedules
- Weekday/weekend/saturday/sunday/holiday hourly profiles
- Availability schedules for HVAC on/off control

### Outputs
- CSV output files at timestep/hourly/daily/monthly/run-period frequency
- Aggregation modes: mean, sum, min, max
- Standard summary report (annual energy, peak loads, hours unmet)
- Custom output variable selection

### Tests
- 180 unit tests across all crates (100% passing)
- 8 example YAML files covering all system types
- 29 ASHRAE 140 validation case files (8 primary cases: 600, 610, 620, 630, 900, 910, 920, 930)

---

## What Needs Work (TODO)

### High Priority
- [ ] Wire `heating_supply_temp` / `cooling_supply_temp` from AirLoopControls into active supply temp control logic (fields populated on LoopInfo but not yet read by simulation)
- [ ] Wire `cycling` method (on/off vs proportional) into PLR/capacity control logic
- [ ] Implement DX coil capacity curve evaluation using attached `cap_ft_curve` and `eir_ft_curve` in real simulations (infrastructure built, curves attached, but only one example coil uses them so far)
- [ ] Part-load performance curves for fans and coils (PLF curves)
- [ ] Chilled water cooling coil model (`source: chilled_water` on CoolingCoilInput)
- [ ] Connect heat recovery exhaust conditions to actual zone return air (currently uses fixed 22-24C proxy)

### Medium Priority
- [ ] Dehumidification modeling in DX coils (currently sensible-only, humidity passes through)
- [ ] Latent load handling in zone heat balance
- [ ] Multi-speed and variable-speed DX coils
- [ ] Heat pump models (air-source, water-source)
- [ ] Demand-controlled ventilation (CO2-based)
- [ ] Condenser water loops and cooling towers (components exist but not integrated)
- [ ] Pumps for hot water and chilled water loops (component exists)
- [ ] Zone air distribution effectiveness
- [ ] Return air path modeling (currently simplified)

### Lower Priority
- [ ] Full parametric run execution in CLI (schema defined, output writer exists, execution engine not complete)
- [ ] JSON schema validation against input files
- [ ] IDF/gbXML import converters
- [ ] Radiant heating/cooling systems
- [ ] VRF (variable refrigerant flow) systems
- [ ] Ground source heat pumps
- [ ] Thermal energy storage
- [ ] Solar thermal collectors
- [ ] Daylighting controls
- [ ] Airflow network / natural ventilation
- [ ] Moisture transport through envelope
- [ ] Detailed window models (angular-dependent SHGC, frame/divider effects)
- [ ] Sub-hourly occupancy and load schedules
- [ ] Web-based IDF-editor-like UI (enabled by openbse_schema.json)

---

## Architecture

```
openbse-cli          # Binary: CLI entry point, simulation driver, system orchestration
openbse-io           # Input parsing (YAML), output writing (CSV/reports)
openbse-envelope     # Building envelope: zones, surfaces, materials, heat balance
openbse-components   # HVAC components: fans, coils, boilers, chillers, heat recovery
openbse-controls     # Controls: thermostats, setpoints, controllers
openbse-core         # Core types: simulation graph, air/water ports, time stepping
openbse-psychrometrics # Moist air property calculations
openbse-weather      # Weather file reading and processing
```

## File Counts
- Rust source files: ~41
- Example YAML files: 8
- ASHRAE 140 test files: 29
- Unit tests: 180
