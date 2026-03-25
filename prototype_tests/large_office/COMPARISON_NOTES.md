# Large Office Prototype — Validation Notes

## Status: 7 of 10 end uses within 5%

## Weather File
- OpenBSE: `USA_CO_Denver-Aurora-Buckley.AFB.724695_TMY3.epw`
- EnergyPlus: Same EPW

## Annual Energy End-Use Comparison

| End Use | E+ [kWh] | OpenBSE [kWh] | Diff % | Status |
|---|---|---|---|---|
| Interior Lighting | 1,558,539 | 1,558,539 | -0.0% | PASS |
| Exterior Lighting | 279,464 | 292,265 | +4.6% | PASS |
| Interior Equipment | 4,076,778 | 4,072,466 | -0.1% | PASS |
| Exterior Equipment | 713,075 | 696,046 | -2.4% | PASS |
| Cooling (Electric) | 552,933 | 575,788 | +4.1% | PASS |
| DHW (Electric) | 125,133 | 128,981 | +3.1% | PASS |
| Heating (Electric) | 0 | 0 | 0% | PASS |
| Fans (Electric) | 1,010,319 | 913,172 | -9.6% | FAIL |
| Pumps (Electric) | 129,769 | 92,100 | -29.0% | FAIL |
| Heating (Gas) | 404,736 | 458,607 | +13.3% | FAIL |

## Key Fixes Applied

1. **Zone Multiplier Expansion**: OpenBSE lacks zone multipliers, so all 12 floors +
   basement are modeled as explicit zones (74 total). Mid-floor zones (5 office +
   1 datacenter per floor x 10 floors) were duplicated with identical geometry,
   loads, and HVAC terminals.

2. **Chiller Performance Curves**: Corrected CAPFT and EIRFT biquadratic curves to
   match E+ IDF values (WC_PD_2004 curves). Previous curves had incorrect coefficients
   that gave wrong condenser-temperature sensitivity, inflating winter chiller energy.

3. **SetpointManager:Warmest SAT Logic**: Implemented E+-style Warmest SAT reset.
   For each cooling zone, compute the supply temp that satisfies it at max flow:
   `SAT_zone = T_zone - Q / (m_max x Cp)`. System SAT = min(SAT_max, min(SAT_zone)).
   Keeps SAT warm (~15.6C) most of the year, dropping to 12.8C only at peak.

4. **Economizer Detection**: Fixed `any_cooling` flag to include zones with ideal
   cooling loads. Added heating-mode lockout to prevent cold OA intake when
   perimeter zones need heating.

5. **Preheat Coil**: Changed from heating-to-SAT to frost-protection-only (2C
   threshold), matching E+ behavior where the AHU heating coil rarely fires.

6. **Load-Based VAV Control**: Replaced proportional-error (5C band) zone flow
   computation with load-based approach: `m = Q / (Cp x (T_zone - SAT))`.
   Terminal control signals derived from the same zone flows for mass balance.

7. **VAV Sizing Factor**: Applied cooling sizing factor (1.15x) to VAV terminal
   max flows and fan design flow only (not PSZ/PTAC systems). Partially closes
   the 18% gap between OpenBSE and E+ zone design cooling loads.

8. **OA Load in Zone Sizing**: Added outdoor air ventilation load to zone cooling
   loads during design-day sizing, matching E+'s zone sizing algorithm.

## Remaining Gaps

### Fans (-9.6%)
OpenBSE zone peak cooling loads during design-day sizing are ~18% lower than E+'s
(58.9 kW vs ~73 kW per Core_mid zone). The 1.15x sizing factor closes part of
this gap. The remaining difference comes from different solar/transient effects
in design-day zone loads and different OA mixing in the ideal loads calculation.

### Pumps (-29%)
Pump energy is low because the chilled water and hot water plant loops see less
demand. The pump model itself may also differ from E+'s HeaderedPumps:VariableSpeed.

### Heating Gas (+13.3%)
The economizer lockout (disabled when any perimeter zone needs heating) prevents
free cooling during shoulder seasons when core zones need cooling but perimeters
need heating. E+ allows the economizer during mixed heating/cooling. Without the
lockout, heating drops but cooling rises significantly.

## Zone Thermal Load Comparison
- Datacenter cooling: OpenBSE 2,450 MWh vs E+ 2,432 MWh (within 1%)
- Office cooling: OpenBSE 2,920 MWh vs E+ 3,435 MWh (15% lower)
- Total zone cooling: OpenBSE 5,370 MWh vs E+ 5,867 MWh (8.5% lower)
