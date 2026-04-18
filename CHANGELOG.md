# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.2.5] - 2026-04-18

### Added

- **DX coil dehumidification** — `autocalculate_shr` now defaults to `true` for all DX coils. Supply air humidity ratio is wired into `ZoneHvacConditions` so dehumidified coil outlet air correctly updates the zone moisture balance each timestep. Closes #7.
- **Humidity-based controls** — `max_relative_humidity` and `min_relative_humidity` setpoints on zones (%). When zone RH exceeds the max, dehumidification mode activates the DX coil even without sensible cooling demand. When RH drops below the min, the humidifier activates. Closes #15.
- **Multi-speed and variable-speed DX coils** — `CoolingCoilDXMultiSpeed` with per-speed `DXSpeedStage` entries (capacity, COP, SHR, performance curves). Two-speed control selects low/high stage based on sensible load; variable-speed interpolates capacity and COP between bounding stages. Use `source: dx_multispeed` in YAML. Closes #6.
- **Water-source heat pump (WSHP)** — New `WaterSourceHeatPump` component (`wshp.rs`). Performance curves as f(entering water temp, entering air temp). Heat rejection/absorption is energy-balanced for both heating (extracts from water loop) and cooling (rejects to water loop) modes. Connects to a condenser water plant loop. Use `source: wshp` in YAML. Closes #9.
- **Chiller lead/lag sequencing** — `staging_mode: sequential` (default, already working) or `equal_split` on `PlantLoopInput`. Sequential fills each unit to capacity before staging the next; equal split divides load evenly. Optional `staging_threshold` (default 0.9) prevents premature lag-chiller starts in sequential mode. Closes #14.

## [0.2.4] - 2026-04-18

### Added

- **PTHP system type** — `system_type: pthp` adds packaged terminal heat pump support: heat pump heating coil with ON/OFF PLR cycling (same as DX cooling, not PLR=1 water-coil modulation). Existing `HeatingCoilInput` with `source: heat_pump` wires directly in. Includes `examples/hotel_pthp.yaml`. Closes #8.
- **Advanced economizer modes** — Three new `EconomizerType` variants available in all system types (PSZ-AC and VAV): `fixed_enthalpy` (OA when outdoor enthalpy < configurable limit, default 65.2 kJ/kg), `enthalpy_with_high_limit` (differential enthalpy AND dry-bulb high-limit). Also fixes `differential_enthalpy` which was incorrectly comparing temperatures instead of enthalpies. Closes #16.
- **ComponentKind enum** — All HVAC components now implement `component_kind() -> ComponentKind` on the `AirComponent` and `PlantComponent` traits, enabling type-safe energy accounting without string matching. Closes #23.
- **`docs/CONTRIBUTING_COMPONENTS.md`** — Step-by-step guide for adding new physics components: trait implementation, YAML registration, sign conventions, unit reference table, and a worked example. Closes #23.
- **Zone air balance diagnostic outputs** — New per-zone output variables `q_surf_conv_total`, `q_surf_conv_walls`, `q_surf_conv_floors`, `q_surf_conv_roofs`, `q_surf_conv_windows`, `q_infiltration_sensible`, and `q_thermal_mass` expose each term of the zone air energy balance for validation and debugging.
- **Per-surface convection outputs** — `conv_to_zone` (h_conv × A × ΔT) and `h_conv_inside` are now available as surface-level output variables.
- **`compare_zone_balance.py`** — Zone air balance comparison script for Single Family prototype (OB vs E+ component-by-component).

### Fixed

- **Inter-floor slab solar interaction (Single Family prototype)** — The 1F/2F structural deck was modeled as an adiabatic surface, causing `FullExterior` solar distribution to send ~50% of beam solar to it. Since both faces are in the same zone the surface is adiabatic, so all absorbed energy recycled back to zone air with no loss path — an artificial gain loop not present in E+. Converted to `internal_mass` (matching E+'s approach): thermal mass is preserved, solar distribution is excluded. Heating error vs E+ reduced from +18% to +4.8%; cooling from unconstrained to +3.5%. Closes #18.
- **`differential_enthalpy` economizer** — Was comparing outdoor dry-bulb temperature against return air temperature instead of computing and comparing air enthalpies. Now correctly uses h = cp·T + hfg·w.

## [0.2.3] - 2026-04-06

### Added

- **ASHRAE 140 CI regression gate** — All 63 ASHRAE 140-2023 cases run on every push/PR; CI fails if any currently-passing case regresses.

### Fixed

- `build_140_csv.py` column matching for free-float and sun zone temperatures broken by output format changes.

## [0.2.2] - 2026-03-29

### Added

- **Output variable customization** — Outputs can now be specified for 'all' relevant parameters (zones, surfaces, components, etc) within the Workbench editor.  Note that the drop down menu functionality is not yet integrated and so requires manual text entry.


## [0.2.0] - 2026-03-25

### Added

- **Parametric run engine** — Execute multiple simulation runs with parameter overrides from a single YAML file. Supports scalar overrides on any named component field (`"Boiler-1.efficiency": 0.95`), per-run weather file swaps, and automatic sweep expansion with zip or Cartesian product modes. Each run produces a separate results CSV.
- **Sweep syntax** — Auto-generate parametric runs from `values: [0.80, 0.85, 0.90]` lists or `range: { min: 3.0, max: 5.0, step: 0.5 }` specifications. Combine multiple sweeps with `cross_product: true` for full factorial analysis.
- **Section override scaffolding** — Data structures and YAML parsing for future template-based section replacement (`include:` with `replaces:` lists). Execution not yet implemented.

## [0.1.0] - 2026-03-23

### Added

- **Simulation engine** — Graph-based execution with automatic topological sort and sub-hourly timesteps
- **Clean YAML input format** — Declarative building models replacing legacy IDD/IDF with composable, human-readable definitions
- **Building envelope physics** — Materials, constructions, windows, CTF conduction, interior/exterior convection, solar distribution (28-bin SHGC mapping), external beam shadow geometry, longwave radiation exchange, and zone air heat balance (3rd-order BDF)
- **Infiltration modeling** — ASHRAE enhanced combined model with multizone pressure-network airflow solver
- **Internal gains** — People, lights, and equipment with radiant/convective/latent splits and schedule control
- **HVAC components** — Fans, heating coils (electric, hot-water, gas), DX cooling coils with performance curves, heat-pump coils, ducts, boilers, chillers, cooling towers, heat recovery, water-to-water heat exchangers, and pumps
- **System templates** — PSZ-AC, DOAS + fan-coil, VAV with per-zone reheat, and residential unitary configurations
- **Plant loops** — Multi-loop simulation with topological ordering, supply/demand side wiring, and condenser loops
- **Controls framework** — Decoupled sensor/actuator control model with thermostats, setpoint managers, and night ventilation
- **Design-day autosizing** — Automatic equipment sizing from design-day peak loads
- **Weather support** — EPW and TMY3 CSV weather file parsing
- **Solar cache** — Disk-persistent solar pre-computation cache for faster repeated runs
- **Output reporting** — Configurable CSV time-series output and end-of-run summary reports (HTML and CSV)
- **Holiday schedules** — Named holiday dates with schedule day-type overrides
- **ASHRAE Standard 140-2023 validation** — 28 test cases implemented, 63/63 metrics passing
- **DOE prototype models** — Single-family, mid-rise apartment, large office, and hospital building models for validation
- **CLI tool (`openbse`)** — Run simulations, view summaries, and export results from the command line
- **Desktop editor (`openbse-editor`)** — Tauri + React GUI with schema-driven object editing, validation, and integrated simulation runner
- **Psychrometric library** — Moist-air property calculations based on Hyland & Wexler correlations
- **8-crate workspace** — Modular architecture: core, components, controls, envelope, weather, psychrometrics, io, cli
- **11 example models** — From simple single-zone to full VAV + chilled-water plant configurations
