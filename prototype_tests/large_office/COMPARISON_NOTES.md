# Large Office Prototype — Validation Notes

## Status: 7 of 10 end uses within 5%

## Weather File
- Boulder, CO TMYx 2009-2023 (EPW)
- Start day: Sunday (Jan 1)
- Elevation: 1612 m, Pressure: 82,237 Pa

## Annual Energy End-Use Comparison

| End Use | E+ [kWh] | OpenBSE [kWh] | Diff | Status |
|---|---|---|---|---|
| Interior Lighting | 1,558,539 | 1,558,539 | -0.0% | ✅ PASS |
| Exterior Lighting | 279,464 | 292,265 | +4.6% | ✅ PASS |
| Interior Equipment | 4,076,778 | 4,072,466 | -0.1% | ✅ PASS |
| Exterior Equipment | 713,075 | 696,046 | -2.4% | ✅ PASS |
| Fans | 1,010,319 | 996,162 | -1.4% | ✅ PASS |
| Pumps | 129,769 | 98,574 | -24.0% | ❌ FAIL |
| Cooling (Electric) | 552,933 | 619,200 | +12.0% | ❌ FAIL |
| Heating (Gas) | 404,736 | 443,810 | +9.7% | ❌ FAIL |
| Heating (Electric) | 0 | 0 | 0% | ✅ PASS |
| DHW (Electric) | 125,133 | 128,981 | +3.1% | ✅ PASS |

## Key Modeling Differences

### Zone Multipliers → Explicit Zones
OpenBSE doesn't support zone multipliers. The E+ model has 5 mid-floor zones with
`zone_multiplier = 10`. In OpenBSE, these are expanded to 50 explicit zones (5 zones × 10
floors: f2–f11), each with its own surfaces, internal loads, infiltration, and HVAC terminal.
Each floor has its own separate VAV air loop (VAV_mid_f2 through VAV_mid_f11).

### Internal Thermal Mass
Added `InteriorFurnishings` (6-inch wood, 540 kg/m³) internal mass to all conditioned zones
at 2.03× floor area, matching E+ `InternalMass` objects. This is critical for preventing
overnight zone temperature drift — without it, zones drop 10-15°C overnight and require
massive morning recovery heating.

### VAV Control Architecture
- **build_vav_signals**: Computes per-zone VAV flows using load-based approach with
  SetpointManager:Warmest SAT logic
- **Terminal box control**: Derives control signal from build_vav_signals zone flows to
  maintain mass balance between fan and terminal flows
- **Economizer**: Fixed dry-bulb with LockoutWithHeating (locks to min OA when mixed air
  would be below SAT) plus cooling-dominant gate to prevent excessive economizer activity
  when heating zones outnumber cooling zones

## Remaining Discrepancies

### Cooling (+12.0%)
Root cause: Perimeter zone cooling loads are higher than E+ due to differences in envelope
thermal mass modeling (OpenBSE uses simplified conduction vs E+'s CTF, no plenum zones).
The perimeter zones receive more solar gain and respond faster to outdoor temperature changes,
resulting in higher cooling loads during afternoon hours. On a typical March weekday, OpenBSE
delivers ~19.8 kW cooling to Perimeter_ZN_1 while E+ delivers only 1.3 kW at the same hour.

### Heating Gas (+9.7%)
Two contributing factors:
1. Zones drift ~5°C below E+ temperatures overnight (18.6°C vs 24.2°C) due to less envelope
   thermal mass (no CTF wall model, no plenum zones). This causes morning heating recovery.
2. The economizer cooling-dominant gate prevents free cooling when heating zones outnumber
   cooling zones, reducing economizer hours and increasing mechanical cooling + reheat.

### Pumps (-24.0%)
Downstream of the cooling/heating issue. CHW and HHW pumps run whenever their loop has coil
demand. With different coil loading patterns, pump runtimes differ. The HHW pump in particular
runs very few hours (~2 MWh/yr vs expected 10+).

## Engine Changes Made

1. **Internal thermal mass**: Added `InteriorFurnishings` construction and internal_mass
   entries to all conditioned zones (2.03× floor area of 6-inch wood).

2. **OA load in zone sizing**: Added outdoor air ventilation load to zone peak cooling
   calculations during sizing.

3. **VAV load-based control**: Replaced proportional-error (5°C band) with load-based
   approach: `m = Q / (Cp × (T_zone - SAT))`.

4. **SetpointManager:Warmest SAT**: For each cooling zone, compute SAT needed at max flow,
   take minimum → system SAT. Clamps between 12.8°C and 15.6°C.

5. **Terminal-fan mass balance**: Terminal control signal derived from build_vav_signals
   zone flows so terminal + fan flows are consistent.

6. **Economizer LockoutWithHeating**: Mixed-air-based lockout prevents cold OA when
   preheat would fire. Cooling-dominant gate prevents economizer when heating zones
   outnumber cooling zones.
