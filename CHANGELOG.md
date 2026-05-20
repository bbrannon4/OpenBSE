# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.4.0] - 2026-05-20

### Added
- **Co-simulation interface** — new `openbse-cosim` crate lets users replace any component with an external model running in Python, Modelica, MATLAB/Simulink, or any other tool. OpenBSE acts as simulation master; external models run as child processes and exchange one JSON message per timestep over stdin/stdout. No external libraries required.
  - `ExternalAirComponent` — implements `AirComponent`; replaces a fan, coil, AHU, or any air-side component. Configured as `type: external_air` in YAML.
  - `ExternalPlantComponent` — implements `PlantComponent`; replaces a chiller, boiler, pump, or any plant-side component. Configured as `type: external_plant` in YAML.
  - Both types accept a user-defined `inputs:` list (variables OpenBSE sends) and `outputs:` list (variables the external process returns). Available inputs cover inlet conditions, outdoor conditions, simulation time, and calendar position.
  - `python/openbse_cosim.py` — thin Python helper (`run_cosim(step_fn)`) that handles the stdin/stdout loop boilerplate.
  - `examples/cosim_chiller/` — reference example: VAV office building with the native chiller replaced by a Python COP+PLR model (`chiller.py`). The script is self-contained and needs no external dependencies. Closes #44.

## [0.3.0] - 2026-05-13

### Added
- **ASHRAE Standard 205 (RS0001–RS0007) equipment performance data support** — manufacturers publish equipment performance as portable CBOR (`.a205`) or JSON files; OpenBSE now consumes them directly via a new `openbse-a205` crate and matching `*_a205` components. Existing polynomial / table-curve modes are unchanged; Standard 205 is selected by adding an `a205_file:` field to a chiller, cooling coil, or fan input. Relative paths resolve against the model YAML directory; absolute paths supported.
  - **RS0001 Chiller** — `ChillerA205` plant component. Reads a 6-axis cooling map (evap flow, evap leaving temp, condenser temp/RH/flow, compressor sequence) plus standby map; supports air-cooled, water-cooled, evaporative condenser types; continuous and discrete compressor speed control with cycling degradation. YAML: `a205_file:` on `type: chiller`. Example: `examples/vav_chw_plant_a205.yaml`.
  - **RS0004 Air-to-Air DX** — `CoolingCoilDXA205` air component. Reads gross total / sensible / power from a 6-axis map; splits delivered total cooling into sensible (lowers T_db) and latent (lowers W) using the file's sensible/total ratio. YAML: `a205_file:` on `type: cooling_coil` with DX source. Example: `examples/residential_unitary_a205.yaml`. Closes #38.
  - **RS0002 Unitary wrapper** — the `a205_file:` field on a cooling coil now also accepts an RS0002 unitary file (auto-extracts the embedded RS0004 DX system). Example: `examples/residential_unitary_rs0002.yaml`. Closes #39.
  - **RS0003 Fan Assembly** — `FanA205` air component. Reads shaft power and impeller speed from a 2-D (flow, static pressure) map. Honors the file's `heat_loss_fraction` to determine fan motor heat into the airstream. YAML: `a205_file:` on `type: fan`. Example: `examples/vav_a205_fan_and_chiller.yaml`. Closes #39.
  - **Nested RS0005 / RS0006 / RS0007** — a `FanEfficiencyChain` composes motor (RS0005) + electronic drive/VFD (RS0006) + mechanical drive/belt (RS0007) sub-components from the fan file's `assembly_components` list to compute grid-side electric power from shaft power. Falls back to a configurable motor efficiency when the file's assembly is empty. Closes #39.
  - **Standalone sub-component overrides** — `motor_a205_file:`, `drive_a205_file:`, and `vfd_a205_file:` fields on a fan input let users swap in a manufacturer-supplied motor/belt/VFD file independently of the RS0003 fan file's nested assembly. Precedence: standalone override > nested in 205 file > fallback constant. Example: `examples/vav_a205_motor_override.yaml`. Closes #40.
- New `openbse-a205` crate with generic N-d linear interpolator (edge-clamp extrapolation policy), CBOR/JSON auto-detecting loader, and per-RS schema modules.

### Changed
- `openbse-io::build_graph` is preserved for backwards compatibility; new `build_graph_with_base(model, model_dir)` accepts the model directory so relative `a205_file:` paths can be resolved. The CLI uses the new entry point.
- `ChillerInput.capacity` and `CoolingCoilInput.capacity` are now optional and ignored when `a205_file:` is set (the file is authoritative on size and efficiency).
- `ChillerInput` gains named-reference fields `capft_curve` / `eirft_curve` / `eirfplr_curve` for resolving table curves from the model's `performance_curves` section (the table-curve YAML example was previously aspirational).

## [0.2.11] - 2026-04-27

### Added
- **Energy End-Use Dashboard** (`tools/editor`) — New ⚡ Energy tab shows a monthly stacked bar chart and annual summary table for electricity, natural gas, district heat/cool end uses. Zone-level unmet heating/cooling hours displayed with per-zone breakdown when any zone exceeds zero. Reads site-level `*_kwh` columns from the loaded results CSV automatically. SI (kWh) and IP (kBtu) unit systems supported. Closes #24.
- **HVAC network expanded** (`tools/editor`) — `NetworkView` now renders `evap_cooler` (sublabel from mode), `vrf_outdoor_unit`, `vrf_indoor_unit`, `radiant_panel` (sublabel from source) on the air-side graph and `thermal_storage` (sublabel shows type + strategy), `gshp` on the plant-side graph. `dual_duct_box` labeled "Dual Duct Box" in terminal nodes. All new types have distinct Tokyo Night theme colors and 20×20 SVG icons. Closes #24.
- **View Report button** (`tools/editor`) — Results toolbar "View Report" button locates the `*_summary.html` file next to the active CSV and opens it in the default browser via a new `open_in_browser` Tauri command. Shows an inline message if no report is found. Closes #24.

## [0.2.10] - 2026-04-19

### Added
- **Setpoint reset controls** — OA-based and demand-based SAT reset on `AirLoopControls` (`cooling_sat_reset`, `heating_sat_reset`); OA-based plant loop setpoint reset on `PlantLoopInput` (`setpoint_reset`). Closes #32.
- **Evaporative cooling** — `EvapCooler` component with `direct`, `indirect`, and `two_stage` modes. Integrated as air-loop equipment via `type: evap_cooler`. Closes #33.
- **Thermal energy storage** — `ThermalStorage` plant component with `chilled_water` and `ice` types. `full_storage`, `load_leveling`, and `demand_limiting` control strategies. Standby loss via `loss_ua`. Closes #37.

## [0.2.9] - 2026-04-19

### Added
- **Dual-duct CAV system** — `system_type: dual_duct` with `DualDuctBox` mixing box terminals. Hot and cold deck supply temperatures blend per zone proportionally to load. Autosizing, parametric override support. See `examples/office_dual_duct.yaml`. Closes #13.

## [0.2.8] - 2026-04-19

### Added
- **Ground-source heat pump (GSHP)** — New `GroundSourceHeatPump` component (`gshp.rs`). Uses Kusuda-Achenbach ground temperature equation as the entering water temperature (EWT) instead of a condenser water loop. Three ground temp source modes: `auto` (derives mean/amplitude/phase from hourly EPW data), `epw_monthly` (uses EPW header ground temperature table when available, falls back to `auto`), `monthly` (user-supplied 12-value table [°C]). Separate `rated_cooling_capacity` and `rated_heating_capacity` with `autosize` support. Optional capacity/EIR performance curves (`cap_ft`, `eir_ft`). Compressor energy routed to `cooling_electric` end-use. See `examples/residential_gshp.yaml`. Closes #12.

### Added
- **Table-lookup performance curves** — `PerformanceCurve::TableLookup` variant accepts manufacturer tabular data directly (no polynomial fitting). N-linear interpolation, per-axis `hold_edge`/`linear` extrapolation, optional output clamping. Axis variables are named (`outdoor_dry_bulb`, `part_load_ratio`, etc.) — order-independent and validated against slot type contracts at parse time. Closes #30.

## [0.2.6] - 2026-04-18

### Added
- **VRF systems** — `VrfOutdoorUnit` coordinates multiple `VrfIndoorUnit` fan-coil units per zone. Performance curves f(outdoor T, indoor T). Heat recovery mode routes cooling rejection to heating zones, reducing compressor work. Capacity limiting with proportional PLR scaling. See `examples/office_vrf.yaml`. Closes #10.
- **Radiant panels** — `RadiantPanel` component supporting fin-tube radiators, panel radiators, and chilled ceiling panels. Hot-water, chilled-water, and electric sources. Configurable `radiant_fraction` (default 0.50 for heating, 0.70 for cooling). Radiant portion distributed to zone surfaces via area×absorptance, affecting MRT. New `radiant_gains` field on `ZoneHvacConditions`. Closes #11.
- **HAMT moisture transport** — 1D finite-difference coupled heat-and-moisture solver. Activated per-construction when all material layers have `vapor_resistance_factor` and `sorption_isotherm` defined; falls back to CTF otherwise. New material fields: `vapor_resistance_factor`, `sorption_isotherm`, `liquid_transport_coeff`. Closes #5.

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
