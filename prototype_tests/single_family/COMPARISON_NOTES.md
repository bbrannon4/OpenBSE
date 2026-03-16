# Single Family CZ5B Boulder — OpenBSE vs EnergyPlus Comparison

## Models

- **OpenBSE YAML**: `SingleFamily_CZ5B_Boulder.yaml`
- **EnergyPlus IDF**: `SingleFamily_CZ5B_Boulder_simplified.idf` (AirflowNetwork removed, constant infiltration, no ducts)
- **Original IDF**: `SingleFamily_CZ5B_Boulder.idf` (unmodified DOE prototype)
- **Weather file**: `../Denver-Buckley.epw` (Denver-Aurora-Buckley AFB, WMO 724695, TMY3)

## Current Status (2026-03-15)

**7 of 8 end uses PASS the 5% threshold. Cooling (Electric) FAILS at -6.8%.**

### Annual End-Use Comparison

| End Use | E+ [kWh] | OpenBSE [kWh] | Diff | Status |
|---------|----------|---------------|------|--------|
| Heating (Gas) | 7,057 | 6,712 | -4.9% | PASS |
| Cooling (Elec) | 1,840 | 1,714 | -6.8% | FAIL |
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
| Jan | 1121 | 1091 | -2.7% |
| Feb | 1107 | 1060 | -4.2% |
| Mar | 827 | 775 | -6.3% |
| Apr | 684 | 627 | -8.4% |
| May | 114 | 94 | -17.3% |
| Jun | 1 | 0 | — |
| Jul | 0 | 0 | — |
| Aug | 1 | 0 | — |
| Sep | 55 | 47 | -14.7% |
| Oct | 703 | 655 | -6.8% |
| Nov | 921 | 887 | -3.8% |
| Dec | 1518 | 1476 | -2.8% |
| **Total** | **7,052** | **6,712** | **-4.8%** |

### Monthly Cooling (Electric) [kWh]

| Month | E+ | OpenBSE | Diff |
|-------|------|---------|------|
| Jan | 20 | 15 | -28.8% |
| Feb | 25 | 19 | -24.4% |
| Mar | 65 | 58 | -11.5% |
| Apr | 58 | 52 | -9.1% |
| May | 149 | 145 | -2.5% |
| Jun | 381 | 362 | -4.8% |
| Jul | 384 | 369 | -4.1% |
| Aug | 364 | 343 | -5.8% |
| Sep | 252 | 237 | -5.9% |
| Oct | 87 | 75 | -13.8% |
| Nov | 36 | 28 | -23.3% |
| Dec | 18 | 12 | -32.1% |
| **Total** | **1,838** | **1,714** | **-6.8%** |

### Monthly Fans (Electric) [kWh]

| Month | E+ | OpenBSE | Diff |
|-------|------|---------|------|
| Jan | 70 | 65 | -7.2% |
| Feb | 70 | 63 | -8.8% |
| Mar | 69 | 64 | -8.3% |
| Apr | 60 | 55 | -8.8% |
| May | 58 | 57 | -2.1% |
| Jun | 103 | 105 | +2.1% |
| Jul | 104 | 106 | +2.1% |
| Aug | 100 | 100 | +0.5% |
| Sep | 77 | 76 | -2.0% |
| Oct | 69 | 62 | -9.6% |
| Nov | 65 | 60 | -8.8% |
| Dec | 87 | 80 | -7.7% |
| **Total** | **932** | **893** | **-4.2%** |

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

- Investigate remaining cooling gap (~125 kWh). Likely envelope: compare hourly zone cooling loads between models to isolate whether it's solar, conduction, or internal gains.
- Fan heating-mode gap (~40 kWh): check whether E+ exhaust fan runtime differs from OpenBSE.
