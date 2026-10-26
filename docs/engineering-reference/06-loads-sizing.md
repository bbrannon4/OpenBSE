# Loads and sizing {#sec-sizing}

::: {.callout-warning}
## Pre-review content

This chapter holds text moved unchanged, apart from format, from the former single-file reference (`README.md`, mostly written in March 2026). Parts of it are stale or incomplete. It will be rewritten from the code, and checked against EnergyPlus, in this chapter's review ([#181](https://github.com/bbrannon4/OpenBSE/issues/181)); see @sec-status.
:::

## Design Day Sizing

**Crate:** `openbse-io` -- `sizing.rs`

**Purpose:** Automatic equipment sizing (autosizing) using design day simulations. Implements a two-stage ASHRAE-compliant approach that first sizes zone-level equipment, then sizes central system equipment.

**Reference:** ASHRAE Handbook -- Fundamentals, Chapter 18 (Nonresidential Cooling and Heating Load Calculations)

### Stage 1: Zone Sizing

For each design day (all provided, not just the first):

1. Generate 24 hours of synthetic weather for the design day conditions.
   - Heating design days use constant temperature, no solar radiation, and specified wind speed.
   - Cooling design days use a sinusoidal daily temperature profile with solar radiation.
2. Run the envelope simulation with warmup days to reach quasi-steady-state.
3. Record peak heating and cooling loads per zone per timestep.
4. Take the maximum across all design days of the same type.

**Results per zone:**
- Peak heating load [W] and the design day / hour where it occurred
- Peak cooling load [W] and the design day / hour where it occurred
- Zone design airflows [kg/s] (max of heating and cooling airflows)

These are used to size zone-level equipment: VAV boxes, fan coil units, etc.

### Stage 2: System Sizing

With zone equipment hard-sized from Stage 1:

1. Re-run each design day with zone airflows set to their design values.
2. At each timestep, sum all zone loads (coincident peak).
3. System capacity = maximum coincident sum across all hours and all design days.

**Results:**
- Coincident peak heating capacity [W] with sizing factor
- Coincident peak cooling capacity [W] with sizing factor
- System airflow [kg/s] and volume flow [m3/s]

These are used to size AHU coils, fans, and central plant equipment.

**Per-system-type sizing overrides:**

- **PSZ-AC / VAV:** Use system-wide coincident peak from Stage 2.
- **DOAS:** Coils are sized to pre-condition 100% OA from design outdoor conditions to fixed supply setpoints: `Q_heat = m_oa * Cp * (T_supply_heat - T_outdoor_heat_design)` and `Q_cool = m_oa * Cp * (T_outdoor_cool_design - T_supply_cool)`.
- **FCU:** Sized to its served zone(s) only -- uses zone peak loads from Stage 1.
