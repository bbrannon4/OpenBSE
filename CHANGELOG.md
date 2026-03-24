# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
- **Output reporting** — Configurable CSV time-series output and end-of-run summary reports
- **ASHRAE Standard 140-2023 validation** — 27 test cases implemented covering envelope and cooling equipment
- **DOE prototype models** — Single-family, mid-rise apartment, large office, and hospital building models for validation
- **CLI tool (`openbse`)** — Run simulations, view summaries, and export results from the command line
- **Desktop editor (`openbse-editor`)** — Tauri + React GUI with schema-driven object editing and validation
- **Psychrometric library** — Moist-air property calculations based on Hyland & Wexler correlations
- **8-crate workspace** — Modular architecture: core, components, controls, envelope, weather, psychrometrics, io, cli
- **12 example models** — From simple single-zone to full VAV + chilled-water plant configurations
