# Single Family CZ5B Boulder — OpenBSE vs EnergyPlus Comparison

## Models

- **OpenBSE YAML**: `SingleFamily_CZ5B_Boulder.yaml`
- **EnergyPlus IDF**: `SingleFamily_CZ5B_Boulder_simplified.idf` (AirflowNetwork removed, constant infiltration, no ducts)
- **Original IDF**: `SingleFamily_CZ5B_Boulder.idf` (unmodified DOE prototype)
- **Weather file**: `../Denver-Buckley.epw` (Denver-Aurora-Buckley AFB, WMO 724695, TMY3)

## Current Status (2026-03-27)

**3 of 7 end uses PASS the 5% threshold. Heating (+15.2%) and Cooling (-10.2%) FAIL.**

Regression from 2026-03-15 caused by commit 90733ae: angular solar fix changed beam
solar transmission (angle-dependent Tsol), and infiltration correction raised from
prior value to 0.0370 m³/s. Higher infiltration drives the heating increase; altered
solar distribution shifts the cooling balance. Root cause of heating gap traced to
outside surface temperature BC differences (see f6e0a8f).

### Annual End-Use Comparison

| End Use | E+ [kWh] | OpenBSE [kWh] | Diff | Status |
|---------|----------|---------------|------|--------|
| Heating (Gas) | 7,057 | 8,128 | +15.2% | FAIL |
| Cooling (Elec) | 1,840 | 1,651 | -10.2% | FAIL |
| Interior Lighting | 1,038 | 1,038 | -0.1% | PASS |
| Exterior Lighting | 212 | 211 | -0.4% | PASS |
| Interior Equipment | 10,083 | 10,077 | -0.1% | PASS |
| Fans (Elec) | 933 | 893 | -4.3% | PASS |
| Pumps (Elec) | 0 | 0 | — | — |
| DHW (Gas) | 2,157 | 2,206 | +2.3% | PASS |
| **Total** | **23,320** | **22,850** | **-2.0%** | |

### Monthly Heating (Gas) [kWh]

| Month | E+ | OpenBSE | Diff |
|-------|------|---------|------|
| Jan | 1121 | 1321 | +17.8% |
| Feb | 1107 | 1273 | +15.0% |
| Mar | 827 | 945 | +14.3% |
| Apr | 684 | 773 | +13.0% |
| May | 114 | 139 | +21.9% |
| Jun | 1 | 1 | — |
| Jul | 0 | 0 | — |
| Aug | 1 | 2 | — |
| Sep | 55 | 66 | +20.0% |
| Oct | 703 | 794 | +12.9% |
| Nov | 921 | 1074 | +16.6% |
| Dec | 1518 | 1740 | +14.6% |
| **Total** | **7,052** | **8,128** | **+15.2%** |

### Monthly Cooling (Electric) [kWh]

| Month | E+ | OpenBSE | Diff |
|-------|------|---------|------|
| Jan | 20 | 11 | -42.4% |
| Feb | 25 | 14 | -42.3% |
| Mar | 65 | 51 | -21.5% |
| Apr | 58 | 45 | -22.7% |
| May | 149 | 132 | -11.5% |
| Jun | 381 | 362 | -5.1% |
| Jul | 384 | 367 | -4.4% |
| Aug | 364 | 340 | -6.5% |
| Sep | 252 | 231 | -8.5% |
| Oct | 87 | 66 | -23.6% |
| Nov | 36 | 22 | -37.9% |
| Dec | 18 | 10 | -45.0% |
| **Total** | **1,840** | **1,651** | **-10.2%** |

### Monthly Fans (Electric) [kWh]

| Month | E+ | OpenBSE | Diff |
|-------|------|---------|------|
| Jan | 70 | 83 | +18.5% |
| Feb | 70 | 80 | +13.4% |
| Mar | 69 | 75 | +9.0% |
| Apr | 60 | 64 | +7.2% |
| May | 58 | 56 | -3.1% |
| Jun | 103 | 104 | +0.6% |
| Jul | 104 | 105 | +0.9% |
| Aug | 100 | 99 | -0.6% |
| Sep | 77 | 76 | -2.0% |
| Oct | 69 | 71 | +3.0% |
| Nov | 65 | 73 | +12.2% |
| Dec | 87 | 102 | +17.2% |
| **Total** | **932** | **988** | **+5.9%** |

## Key Observations

- **Cooling gap is load-driven at low PLR**: Peak summer months (May-Jul) are within 5% after the PLF curve fix. The remaining annual gap comes from winter/shoulder months where absolute cooling loads are small (12-58 kWh/mo) but percentage errors are large (10-30%). This suggests an envelope-level difference (window SHGC, solar gain timing, or thermostat deadband) rather than equipment efficiency.
- **Heating shoulder months**: Apr (-8.4%) and Oct (-6.8%) exceed 5% individually but the annual total passes. Same root cause — mild-weather loads are slightly lower in OpenBSE.
- **Fans split by mode**: Fans are ~2% high in cooling-dominant months (Jun-Aug) and ~8% low in heating-dominant months. The mode-dependent split may relate to different fan runtime patterns between heating and cooling.
- **Gas furnace has no part-load curve**: Confirmed — E+ `Coil:Heating:Fuel` uses constant efficiency × PLR with no cycling degradation.
- **DX PLF curve**: Fixed 2026-03-15 — `main.rs` now evaluates the actual quadratic PLF curve from the YAML instead of a hardcoded Cd=0.15 linear default. Closed cooling gap from -8.5% to -6.8%.

## Changes Made (2026-03-15)

1. **Weather file**: Both models now use `Denver-Buckley.epw` (WMO 724695)
2. **Exterior lighting**: Added to YAML with astronomical clock control (210.7 vs 211.5 kWh E+)
3. **DHW water heater**: Added `Modulate` control type to engine; YAML updated to 1-gal tank, 0.97 efficiency, UA=0, matching IDF tankless
4. **DX PLF curve**: Engine now reads the coil's PLF curve from YAML instead of hardcoded default
5. **E+ IDF**: Added `Output:Meter` monthly objects for end-use comparison
6. **Water heater ambient zone**: Added `ambient_zone` support to engine (references zone temperature dynamically)

## Next Steps

- **Fix heating regression (+15%)**: root cause traced to outside surface temperature BC
  differences (commit f6e0a8f). The angular solar fix in 90733ae changed how beam Tsol
  is applied; need to verify outside BC matches E+ `SurfaceProperty:OtherSideCoefficients`
  or the standard combined radiation/convection calculation. Compare hourly outside
  surface temps between OpenBSE and E+ for opaque walls.
- **Fix cooling regression (-10%)**: correlated with heating gap — if outside BCs are
  corrected, cooling should recover toward -6% as well.
- **Fan gap (+5.9%)**: heating-mode fan runtime is higher because heating runs more;
  fixing heating regression should bring fans in line.
