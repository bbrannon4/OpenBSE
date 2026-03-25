# Large Office Prototype — Validation Notes

## Status: 7 of 10 end uses within 5%

## Weather File
- OpenBSE and EnergyPlus: `USA_CO_Denver-Aurora-Buckley.AFB.724695_TMY3.epw`

## Annual Energy End-Use Comparison

| End Use | E+ [kWh] | OpenBSE [kWh] | Diff % | Status |
|---|---|---|---|---|
| Interior Lighting | 1,558,539 | 1,558,539 | -0.0% | PASS |
| Exterior Lighting | 279,464 | 292,265 | +4.6% | PASS |
| Interior Equipment | 4,076,778 | 4,072,466 | -0.1% | PASS |
| Exterior Equipment | 713,075 | 696,046 | -2.4% | PASS |
| Cooling (Electric) | 552,933 | 578,785 | +4.7% | PASS |
| DHW (Electric) | 125,133 | 128,981 | +3.1% | PASS |
| Heating (Electric) | 0 | 0 | 0% | PASS |
| Fans (Electric) | 1,010,319 | 875,861 | -13.3% | FAIL |
| Pumps (Electric) | 129,769 | 92,495 | -28.7% | FAIL |
| Heating (Gas) | 404,736 | 452,962 | +11.9% | FAIL |

## Remaining Gaps — Root Cause Analysis

### Fans (-13.3%)
OpenBSE design-day zone cooling loads are 18% lower than E+'s for the core zone
(58.9 kW steady-state vs E+'s 85.3 kW transient peak at 8 AM). The gap is from
E+'s thermal mass effects during sizing: with constant 24/7 internal gains, E+'s
CTF model produces a daily load oscillation (25-85 kW) as surface thermal mass
absorbs and releases heat. OpenBSE's ideal-loads sizing reaches perfect steady
state (58.9 kW flat) because the zone temp is clamped at setpoint, preventing
surface temperature oscillation. The 18% lower design flow leads to lower flow
fractions at runtime, amplified by the cubic fan power curve to -13% energy.

### Heating Gas (+11.9%)
Economizer lockout during heating periods prevents free cooling for core zones.
E+ data shows 36-100% OA on winter weekdays with zero AHU heating — the
economizer provides free cooling while VAV reheat handles perimeters. Removing
OpenBSE's lockout allows free cooling but increases reheat energy to +53%
because the cold SAT (15.6C) supply air requires massive perimeter reheat.
The lockout trades higher cooling for lower heating — current balance gives
the best overall result. A more sophisticated approach (per-zone SAT
optimization or variable SAT based on heating/cooling balance) would help.

### Pumps (-28.7%)
CHW primary pump runs ~3200 hrs/year in OpenBSE vs E+'s longer runtime.
Plant loop pump cycling logic differs: E+ runs pumps whenever any demand
exists on the loop; OpenBSE may shut down pumps more aggressively during
low-load periods. HHW pump energy is also low (2.0 MWh vs expected 10+ MWh),
suggesting the hot water loop isn't cycling the pump enough for reheat demand.

## Zone Thermal Load Comparison
- Datacenter cooling: OpenBSE 2,450 MWh vs E+ 2,432 MWh (within 1%)
- Office cooling: OpenBSE 2,920 MWh vs E+ 3,435 MWh (15% lower)
- Total zone cooling: OpenBSE 5,370 MWh vs E+ 5,867 MWh (8.5% lower)

## Key Engine Changes Made
1. SetpointManager:Warmest SAT reset (highest SAT satisfying all cooling zones)
2. Corrected chiller CAPFT/EIRFT curves from E+ IDF (WC_PD_2004)
3. Load-based VAV zone flow with SAT-consistent terminal control signals
4. Economizer activation from ideal cooling loads + heating-zone lockout
5. Frost-only AHU preheat (2C threshold, not heating to SAT)
6. OA ventilation load in zone design-day sizing
