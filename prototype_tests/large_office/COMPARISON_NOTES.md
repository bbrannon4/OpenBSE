# Large Office Prototype — Validation Notes

## Status: 7 of 9 end uses within 5% (excluding Heating Electric = 0)

## Weather File
- OpenBSE and EnergyPlus: `USA_CO_Denver-Aurora-Buckley.AFB.724695_TMY3.epw`

## Annual Energy End-Use Comparison

| End Use | E+ [kWh] | OpenBSE [kWh] | Diff % | Status |
|---|---|---|---|---|
| Interior Lighting | 1,558,539 | 1,558,539 | -0.0% | PASS |
| Exterior Lighting | 279,464 | 292,265 | +4.6% | PASS |
| Interior Equipment | 4,076,778 | 4,072,466 | -0.1% | PASS |
| Exterior Equipment | 713,075 | 696,046 | -2.4% | PASS |
| Fans (Electric) | 1,010,319 | 1,004,841 | -0.5% | PASS |
| Heating (Gas) | 404,736 | 422,120 | +4.3% | PASS |
| DHW (Electric) | 125,133 | 128,981 | +3.1% | PASS |
| Cooling (Electric) | 552,933 | 620,021 | +12.1% | FAIL |
| Pumps (Electric) | 129,769 | 95,661 | -26.3% | FAIL |

## Remaining Gaps — Root Cause Analysis

### Cooling (+12.1%)
The chiller electricity is 16.8% above E+ (484.6 vs 414.8 MWh). The excess comes
from the economizer lockout being more aggressive than E+'s LockoutWithHeating.
OpenBSE locks the economizer when more zones need heating than cooling (zone count).
E+'s lockout only engages when the AHU preheat coil fires (mixed air < SAT). In
shoulder seasons, OpenBSE locks out the economizer earlier, requiring more mechanical
cooling for core zones that could otherwise use free cooling.

The DC DX cooling is actually -9.9% below E+ (124.4 vs 138.1 MWh).

### Pumps (-26.3%)
Three pump systems contribute:
- CHW Primary (constant speed): 70.6 MWh (reasonable)
- CHW Secondary (variable speed): 22.7 MWh (E+ estimated ~36.5 MWh, -38%)
- HHW Pump (variable speed): 1.9 MWh (E+ estimated ~9.3 MWh, -79%)

The HHW pump runs at minimum most of the time because reheat demand is concentrated
in the morning startup hours. The CHW secondary pump gap correlates with the overall
chiller/cooling coil load pattern.

## Key Engine Changes Made During Validation

1. **Load-based VAV zone flows**: `build_vav_signals` computes zone airflows from
   ideal cooling loads and SAT (two-pass: first pass estimates SAT, second pass
   recomputes flows with actual SAT). Terminal box control signals derived from
   these flows for fan/terminal mass balance.

2. **Economizer lockout**: Cooling-dominant check (n_cool > n_heat) approximates
   E+'s LockoutWithHeating. When most zones need heating, economizer locks to
   minimum OA to prevent excessive reheat.

3. **VAV zone sizing factor**: 1.15x cooling sizing factor applied to VAV zone
   design airflows to compensate for OpenBSE's steady-state design-day peak vs
   E+'s transient CTF peak (85.3 kW vs 58.9 kW at hour 8).

4. **OA load in zone sizing**: Outdoor air ventilation load added to zone design
   cooling load, matching E+'s sizing approach.

## Model Differences vs E+

The simplified IDF and OpenBSE YAML both represent the same building, with these
known differences:

1. **No plenum zones**: OpenBSE doesn't model the 3 return-air plenums (GroundFloor,
   MidFloor, TopFloor) present in E+. This causes core zones to overheat at night
   (28°C vs E+'s 25.5°C) because heat has no escape path through the adiabatic ceiling.
   The phantom OA from zone outdoor_air specs partially compensates.

2. **Zone multiplier expansion**: E+ uses zone_multiplier=10 for mid-floor zones.
   OpenBSE expands these to 10 explicit floors (60 mid-floor zones total), each with
   its own air loop. This matches E+'s thermal behavior but increases simulation time.

3. **Holidays**: E+ uses US federal holidays. OpenBSE uses the same schedule.
