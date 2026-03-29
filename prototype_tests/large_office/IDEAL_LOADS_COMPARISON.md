# Large Office Ideal Loads Comparison: OpenBSE vs EnergyPlus

## Purpose

Isolate building thermodynamics from HVAC system modeling by replacing all
real HVAC equipment with ideal loads (perfect heating/cooling to setpoint).
This reveals pure envelope/gains/infiltration differences between the two
engines before HVAC tuning begins.

## Configuration

| Parameter | Value |
|-----------|-------|
| Weather | Denver-Buckley.epw (CZ5B) |
| Office setpoints | Htg 21 C / Clg 24 C (constant, no setback) |
| DataCenter setpoints | Htg 18 C / Clg 27 C (constant) |
| HVAC capacity | NoLimit (effectively unlimited) |
| Outdoor air | **Removed** from both models |
| Ventilation | **Removed** from both models |
| Infiltration | Kept identical |
| Internal loads | Kept identical (lights, people, equipment) |
| Schedules | Kept identical |
| Constructions | Kept identical |

### Files Created

- **OpenBSE**: `LargeOffice_Boulder_ideal.yaml`
  - Removed: `air_loops`, `plant_loops`, `thermostats`, `outdoor_air`
  - Added: `ideal_loads` with constant setpoints and 100 MW capacity
- **EnergyPlus**: `eplus_run/in_ideal.idf`
  - Removed: All AirLoopHVAC, Plant, Fan, Coil, Chiller, Boiler, Pump,
    Controller, SetpointManager, AvailabilityManager objects
  - Added: ZoneHVAC:IdealLoadsAirSystem per conditioned zone (NoLimit, no OA)
  - Thermostat schedules replaced with constant 21/24 C (office) and 18/27 C (datacenter)

## RESOLVED: CTF Instability Fixed via Implicit FD Fallback

**NaN divergence eliminated.** All 70 zones now simulate stably for the full annual run.

### Root Cause (Confirmed)

CTF divergence requires ΣΦ ≥ 0.99 AND a pinned zone temperature (ideal loads).
When zone temperature is clamped to setpoint, the CTF history amplification factor
`1/(1-ΣΦ)` ≥ 100 accumulates coherently across timesteps — leading to NaN in 1-3
hours. With real HVAC the zone varies naturally and breaks the coherent buildup.

### Fix: Implicit Backward-Euler FD Fallback

`FdSurface` in `ctf.rs` provides an unconditionally-stable tridiagonal (Thomas
algorithm) replacement for CTF on any surface where:
- ΣΦ ≥ 0.99, AND
- The zone uses ideal loads (zone temp is pinned to setpoint)

Outdoor surfaces are excluded — dynamic weather prevents buildup at any ΣΦ.

**Surfaces switched to FD in this model (17 total):**
- Ground floor slabs (5 zones): ΣΦ = 0.9947
- Basement walls (6 surfaces): ΣΦ = 0.9930
- Basement floors (2): ΣΦ = 0.9986
- Interior mass (4+ floors): ΣΦ = 0.9998

### Results After Fix

| Metric | Before | After | E+ Reference |
|--------|--------|-------|-------------|
| Unmet Hours | 134,293 | **0** | — |
| Max zone temp | NaN | 27.0 C | — |
| Heating [kWh] | 294,000,000 | **496,324** | 338,205 |
| Cooling [kWh] | 286,000,000 | **5,722,172** | 6,331,810 |

Cooling is within 9.6% of E+. Heating is 46.8% high — see remaining discrepancies below.

### Remaining Heating Discrepancy (+46.8%)

E+ shows zero heating for: Basement, Core_bottom, Core_mid, Core_top (core internal
gains overwhelm any envelope loss). OpenBSE shows non-zero heating for Core_top and
Basement. Suspected causes:

1. **Basement heating**: E+ basement = 0 heating; OBSE peak = 44,663 W.
   Ground temperature profile or basement wall conductance may be too high.
2. **Core_top heating**: E+ = 0 heating; OBSE peak = 57,180 W.
   Possible roof construction issue or interzone BC underestimating heat from below.

### Action Items

1. ~~Fix NaN divergence~~ ✓ Done via FD fallback
2. **Investigate Basement heating**: Compare ground BC temperature profile and
   CfactorUndergroundWall construction U-value against E+ inputs
3. **Investigate Core_top heating**: Check roof construction conductance and whether
   interzone heat transfer from Core_mid_f11 is being properly accounted for
4. **DataCenter boundary conditions**: DataCenter zones are adiabatic in OpenBSE
   but have interzone boundaries to plenums in E+ — explains seasonal differences

## EnergyPlus Results (Reference)

### Annual Zone Ideal Loads (kWh)

| Zone | Heating | Cooling |
|------|--------:|--------:|
| Basement | 0 | 234,454 |
| Core_bottom | 0 | 243,047 |
| Core_mid (x10) | 0 | 1,970,210 |
| Core_top | 0 | 196,705 |
| DataCenter_basement (x1) | 0 | 1,988,787 |
| DataCenter_bot | 0 | 39,198 |
| DataCenter_mid (x10) | 0 | 357,570 |
| DataCenter_top | 0 | 35,700 |
| Perimeter_bot (4 zones) | 9,524 | 123,807 |
| Perimeter_mid (4 zones x10) | 296,248 | 1,039,432 |
| Perimeter_top (4 zones) | 32,433 | 102,901 |
| **TOTAL** | **338,205** | **6,331,810** |

Key observations from E+ results:
- Core zones are cooling-only (high internal gains overwhelm envelope loss)
- Perimeter_mid zones have significant heating AND cooling (perimeter effects)
- North-facing perimeter (ZN_3) has highest heating, south-facing (ZN_1) has highest cooling
- DataCenter_basement dominates datacenter cooling (783 m2 floor area vs 36 m2 for others)

## DataCenter Zone Comparison (Only Valid Zones)

These are the only zones where OpenBSE produced valid (non-NaN) results.

### Annual Totals (kWh)

| Zone | E+ Cooling | OBSE Cooling | Diff |
|------|----------:|------------:|-----:|
| DataCenter_bot | 39,198 | 45,306 | +15.6% |
| DataCenter_mid (sum f2-f11) | 357,570 | 453,062 | +26.7% |
| DataCenter_top | 35,700 | 45,306 | +26.9% |

### Monthly Pattern Discrepancy

E+ shows strong seasonal variation (e.g., DataCenter_bot: 1,020 kWh in Jan
vs 5,588 kWh in Aug). OpenBSE shows nearly constant monthly cooling (~3,840
kWh/month for bot/top).

**Root cause**: In E+, DataCenter zones transfer heat through floor/ceiling
to adjacent plenum zones, which have temperature-dependent heat balance.
In OpenBSE, DataCenter zone boundaries are modeled as **adiabatic**, meaning
no heat transfer to adjacent spaces. The OpenBSE DataCenter zones are
thermally isolated islands that only reject internal gains.

This means the DataCenter comparison is not apples-to-apples. The DataCenter
zones in E+ gain/lose heat through their floor/ceiling (seasonal effect),
while in OpenBSE they are perfectly insulated.

## Next Steps

1. **Fix NaN divergence** (blocking): This is the highest priority. Without
   stable ideal loads for multi-zone buildings, envelope validation is
   impossible.
2. **DataCenter boundary conditions**: Verify whether DataCenter zones should
   have adiabatic or interzone boundaries in the OpenBSE YAML (match E+ model).
3. **Re-run comparison** after NaN fix to get valid office zone loads.
4. **Monthly and hourly comparisons** for all zones once NaN is resolved.
5. **Hourly temperature profiles** for Core_mid and Perimeter_mid_ZN_1 on
   Jan 10 (winter) and Jul 19 (summer) representative days.

## Files

| File | Description |
|------|-------------|
| `LargeOffice_Boulder_ideal.yaml` | OpenBSE ideal loads input |
| `eplus_run/in_ideal.idf` | EnergyPlus ideal loads input |
| `eplus_run_ideal/` | EnergyPlus ideal loads output directory |
| `LargeOffice_Boulder_ideal_zone_results.csv` | OpenBSE hourly zone results (mostly NaN) |
| `LargeOffice_Boulder_ideal_summary.csv` | OpenBSE annual summary |
