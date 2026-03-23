# Large Office Prototype Validation Notes

## Model Information
- **Building**: DOE Prototype Large Office (12-story + basement), 46,320 m2
- **Location**: Boulder, CO (Climate Zone 5B)
- **Weather**: USA_CO_Boulder.Muni.AP.720533_TMYx.2009-2023.epw
- **Code vintage**: ASHRAE 90.1-2019 Appendix G
- **E+ version**: 25.2.0
- **E+ simplified IDF**: `LargeOffice_Denver_simplified.idf`

## Current Status: BLOCKED — Engine Changes Required

The YAML has been aligned with the IDF as closely as possible. However,
the OpenBSE engine currently lacks proper zone multiplier support for
energy reporting, which prevents meaningful end-use comparison for any
building using zone multipliers.

### YAML Fixes Applied (this pass)

1. **`zone_multiplier: 10`** — changed from `multiplier:` to match serde field name
2. **Equipment schedule (BLDG_EQUIP_SCH)** — Saturday/Sunday values corrected to match IDF full precision (weekday was already close; Saturday was completely wrong)
3. **Lighting schedule Saturday** — fixed hours 17-18 (0.15 → 0.05 to match IDF)
4. **Equipment radiant fractions** — office zones: 0.3 → 0.5; data center zones: 0.3 → 0.1 (matching IDF)
5. **DataCenter thermostat** — 15.6/24.0 → 18.0/27.0 (matching IDF HTGSETP_DC_SCH / CLGSETP_DC_SCH)
6. **Infiltration schedule** — added INFIL_SCH_PNNL (0.25 during HVAC operation, 1.0 when off) and assigned to all infiltration objects
7. **Design day values** — corrected cooling wetbulb (15.7 → 15.0), heating wind (2.3 → 2.5), cooling wind (4.0 → 3.6)

### Known YAML Issues NOT Yet Fixed

1. **Occupancy schedule** — IDF `BLDG_OCC_SCH_wo_SB` has slight "setback modulation" perturbations at hours 10, 12, 14 (0.993/0.523/0.993 vs YAML 0.95/0.5/0.95). Also Sunday differs (IDF has 0 for hours 0-5 and 18-23, YAML has 0.05 constant). These are small (~1-2% effect on occupancy-related internal gains).

2. **DataCenter equipment schedule** — IDF has monthly ramp pattern (Jan=0.25, Feb=0.50, Mar=0.75, Apr=1.0, repeating quarterly). YAML uses flat 0.625 (annual average). The annual total is identical but monthly distribution differs. OpenBSE doesn't support monthly-varying schedules, so this would need either:
   - Engine support for monthly schedule profiles, OR
   - IDF simplification to use the same flat schedule

3. **DHW use temperature** — IDF has target_temp=43.3°C and hot_supply_temp=43.3°C from a 60°C tank. The YAML uses use_temp=60.0 which draws 100% from tank. The correct value depends on matching E+'s WaterUse:Equipment mixing logic. With use_temp=60 → +5.4% vs E+. With use_temp=43.3 → -29% vs E+. The gap is partially due to E+ using varying mains temperature (avg 9.95°C) vs OpenBSE fixed 13.3°C.

4. **VAV SAT reset** — IDF uses SetpointManager:Warmest (12.8-15.6°C). YAML uses fixed cooling SAT=12.8°C. OpenBSE engine doesn't support SAT reset yet.

5. **HW loop OA reset** — IDF uses SetpointManager:OutdoorAirReset (82.2°C at OA<=-6.667, 65.6°C at OA>=10). YAML uses fixed 82°C.

6. **Plenum zones** — IDF has 3 plenum zones (GroundFloor, MidFloor, TopFloor) with infiltration. YAML has none. These plenums provide a buffer between conditioned zones and the exterior/roof, and carry return air infiltration. Missing plenums affects top-floor and bottom-floor heat balance.

7. **Exterior lighting schedule** — IDF uses a separate schedule, YAML uses `astronomical_clock: true`. Results are close (+6%) but not exact.

## E+ End-Use Reference Values

| End Use | GJ | kWh |
|---------|-----|-----|
| Heating (Gas) | 1,457.05 | 404,736 |
| Cooling (Elec) | 1,990.56 | 552,933 |
| Interior Lighting | 5,610.74 | 1,558,539 |
| Exterior Lighting | 1,006.07 | 279,464 |
| Interior Equipment | 14,676.40 | 4,076,778 |
| Exterior Equipment | 2,567.07 | 712,964 |
| Fans (Elec) | 3,637.15 | 1,010,319 |
| Pumps (Elec) | 467.17 | 129,769 |
| Water Systems (Elec) | 450.48 | 125,133 |
| **Total Site** | **31,862.69** | **8,850,747** |

## OpenBSE Current Results (zone multiplier not applied to reporting)

| End Use | E+ (kWh) | OpenBSE (kWh) | Diff | Notes |
|---------|----------|---------------|------|-------|
| Interior Lighting | 1,558,539 | 491,490 | -68% | Missing zone_multiplier on reporting |
| Exterior Lighting | 279,464 | 296,815 | +6% | Astronomical clock vs IDF schedule |
| Interior Equipment | 4,076,778 | 2,670,154 | -35% | Missing zone_multiplier on reporting |
| Exterior Equipment | 712,964 | 711,724 | 0% | OK |
| Fans | 1,010,319 | 396,587 | -61% | Missing zone_multiplier on HVAC reporting |
| Pumps | 129,769 | 35,723 | -72% | Missing zone_multiplier on HVAC reporting |
| Cooling | 552,933 | 408,356 | -26% | Missing zone_multiplier on HVAC reporting |
| Heating (Gas) | 404,736 | 139,021 | -66% | Missing zone_multiplier on HVAC reporting |
| DHW (Elec) | 125,133 | 131,926 | +5% | Mains temp and mixing model differences |

**These results are not meaningful** because the zone multiplier is not applied to energy reporting. The sizing correctly applies the multiplier, so HVAC equipment operates at the right capacity level, but the reported energy is only the single-zone component. Once the engine applies zone_multiplier to reporting, results should be ~3x higher for most HVAC end uses (since mid floors with mult=10 dominate the building).

## Required Engine Changes

### Critical (blocking validation)

1. **Zone multiplier on internal gains reporting** (`main.rs` ~line 1933-1936):
   Zone lighting_power and equipment_power must be multiplied by `zone.input.zone_multiplier` before writing to the snapshot. Currently:
   ```rust
   snapshot.zone_lighting_power.insert(name, zone.lighting_power);
   ```
   Should be:
   ```rust
   let zmult = zone.input.zone_multiplier as f64;
   snapshot.zone_lighting_power.insert(name, zone.lighting_power * zmult);
   ```

2. **Zone multiplier on HVAC component energy reporting** (`main.rs` ~line 1920-1932):
   Fan, coil, and pump power from HVAC components must be multiplied by the served zone's multiplier. The `comp_zone_multiplier` map was built at startup (lines 456-482) but is currently not applied to component energy. The `zmult` multiplication that was previously there was removed (see git stash "Remove zone multiplier capability"). It needs to be restored.

   **However**: this needs careful design. The simulation already uses multiplied loads for PLR (via `zmult_plr` at line 2813). If the fan/coil outputs already reflect the multiplied load (because sizing includes multiplier), then multiplying again would double-count. The correct approach depends on whether `simulate_all_loops()` returns outputs at the single-zone or building level. This needs testing.

3. **Serde alias for `zone_multiplier`** (`zone.rs` line 423):
   Add `alias = "multiplier"` to the serde attribute so YAML can use either `multiplier:` or `zone_multiplier:`. Currently only `zone_multiplier:` works. (The YAML has been updated to use `zone_multiplier:` as a workaround.)

### Important (needed for <5% accuracy)

4. **SAT reset (SetpointManager:Warmest)**: VAV cooling supply air temperature should reset between 12.8-15.6°C based on warmest zone demand. Currently fixed at 12.8°C. This affects fan energy (lower SAT = more reheat + less airflow needed).

5. **HW loop temperature OA reset**: Hot water supply temperature should reset 82.2→65.6°C based on outdoor air temperature. Currently fixed at 82°C.

6. **Monthly schedule support**: DataCenter equipment schedule varies by month (0.25/0.50/0.75/1.00 repeating quarterly). Currently modeled as flat 0.625 average.

7. **Varying mains water temperature**: E+ uses Site:WaterMainsTemperature correlation (annual avg 9.95°C, seasonal variation). OpenBSE uses fixed temperature. This affects DHW energy by ~5-30% depending on the value chosen. (Note: a `MainsTemperature::Correlation` variant already exists in input.rs but may not be fully wired.)

### Minor (nice to have)

8. **Exterior lighting schedule**: Consider supporting the E+ exterior lighting schedule directly instead of astronomical clock (6% gap).

9. **Plenum zone modeling**: The IDF has 3 unconditioned plenum zones with infiltration. Adding these would improve accuracy of top-floor and first-floor heat balance.
