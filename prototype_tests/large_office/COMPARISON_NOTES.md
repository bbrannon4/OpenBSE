# Large Office Prototype Validation Notes

## Status: In Progress (HVAC end uses not within 5%)

4 of 10 end uses pass the 5% threshold. The remaining 6 (fans, pumps, cooling,
heating, exterior lighting, DHW) are blocked by engine-level differences in zone
thermal mass handling and HVAC sizing.

## Model Information
- **Building**: DOE Prototype Large Office (12-story + basement), 46,320 m2
- **Location**: Boulder, CO (Climate Zone 5B)
- **Weather**: `USA_CO_Boulder.Muni.AP.720533_TMYx.2009-2023.epw`
- **E+ version**: 25.2.0
- **E+ IDF**: `LargeOffice_Denver_simplified.idf` (in `eplus_run/`)
- **OpenBSE YAML**: `LargeOffice_Boulder.yaml` (expanded via `expand_zones.py`)

## Key Model Changes

### Zone Multiplier Expansion
OpenBSE does not support zone multipliers. The 6 mid-floor zones (5 office + 1 DC,
each with `zone_multiplier: 10`) were expanded into 60 explicit zones via `expand_zones.py`:
- 10 copies of each office mid zone (floors 2-11)
- 10 copies of DataCenter_mid_ZN_6
- 10 per-floor VAV systems and 10 per-floor DC PSZ-AC systems
- Zone groups, infiltration, equipment, DHW loads expanded accordingly
- Total: 74 zones, 360 surfaces, 74 HVAC terminals

### DC Fan Continuous Operation
E+ DC fans use `Fan:SystemModel` with Discrete speed control (always full power).
Added `fan_operating_mode: continuous` to DC bot/mid/top air loops.

### VAV Settings
- Box min flow fraction: 0.30 (matches E+)
- System fan power min flow fraction: 0.25 (matches E+)

---

## Annual End-Use Comparison

| End Use                 |   E+ [kWh] | OpenBSE [kWh] |  Diff % | Status |
|-------------------------|------------|---------------|---------|--------|
| Interior Lighting       |  1,558,539 |     1,599,908 |   +2.7% | PASS   |
| Exterior Lighting       |    279,464 |       296,815 |   +6.2% | FAIL   |
| Interior Equipment      |  4,076,778 |     4,091,485 |   +0.4% | PASS   |
| Exterior Equipment      |    713,075 |       711,724 |   -0.2% | PASS   |
| Fans (Electric)         |  1,010,319 |       565,338 |  -44.0% | FAIL   |
| Pumps (Electric)        |    129,769 |        67,287 |  -48.1% | FAIL   |
| Cooling (Electric)      |    552,933 |       625,756 |  +13.2% | FAIL   |
| Heating (Gas)           |    404,736 |       497,225 |  +22.9% | FAIL   |
| Heating (Electric)      |          0 |             0 |    0.0% | PASS   |
| DHW (Electric)          |    125,133 |       131,926 |   +5.4% | FAIL   |
|-------------------------|------------|---------------|---------|--------|
| **Total**               |  8,850,747 |     8,587,464 |   -3.0% |        |

---

## Monthly Comparison — Fans [kWh]

| Month | E+       | OpenBSE  | Diff %  |
|-------|----------|----------|---------|
| Jan   |   65,675 |   38,592 |  -41.2% |
| Feb   |   62,266 |   34,385 |  -44.8% |
| Mar   |   85,382 |   40,399 |  -52.7% |
| Apr   |   90,151 |   38,767 |  -57.0% |
| May   |   74,739 |   44,282 |  -40.8% |
| Jun   |   90,888 |   57,940 |  -36.2% |
| Jul   |  102,158 |   66,092 |  -35.3% |
| Aug   |  114,584 |   65,666 |  -42.7% |
| Sep   |   80,976 |   55,816 |  -31.1% |
| Oct   |   77,990 |   44,811 |  -42.5% |
| Nov   |   78,373 |   39,563 |  -49.5% |
| Dec   |   87,138 |   39,023 |  -55.2% |

## Monthly Comparison — Cooling (Electric) [kWh]

| Month | E+       | OpenBSE  | Diff %  |
|-------|----------|----------|---------|
| Jan   |    5,352 |   29,598 | +453.1% |
| Feb   |   11,854 |   26,037 | +119.7% |
| Mar   |   27,982 |   31,264 |  +11.7% |
| Apr   |   36,715 |   34,910 |   -4.9% |
| May   |   24,734 |   44,023 |  +78.0% |
| Jun   |   78,950 |   80,056 |   +1.4% |
| Jul   |  106,848 |   97,834 |   -8.4% |
| Aug   |  119,900 |   99,609 |  -16.9% |
| Sep   |   54,316 |   75,079 |  +38.2% |
| Oct   |   34,820 |   45,380 |  +30.3% |
| Nov   |   24,718 |   31,470 |  +27.3% |
| Dec   |   26,744 |   30,497 |  +14.0% |

## Monthly Comparison — Heating (Gas) [kWh]

| Month | E+       | OpenBSE  | Diff %  |
|-------|----------|----------|---------|
| Jan   |   76,731 |   96,539 |  +25.8% |
| Feb   |   66,690 |   87,073 |  +30.6% |
| Mar   |   50,682 |   56,674 |  +11.8% |
| Apr   |   27,490 |   29,877 |   +8.7% |
| May   |   18,395 |   15,839 |  -13.9% |
| Jun   |    1,217 |    3,915 | +221.7% |
| Jul   |      402 |    1,585 | +294.3% |
| Aug   |      452 |      668 |  +47.8% |
| Sep   |    5,503 |    3,939 |  -28.4% |
| Oct   |   24,893 |   24,972 |   +0.3% |
| Nov   |   54,096 |   74,633 |  +38.0% |
| Dec   |   78,185 |  101,510 |  +29.8% |

## Monthly Comparison — Pumps [kWh]

| Month | E+       | OpenBSE  | Diff %  |
|-------|----------|----------|---------|
| Jan   |    1,780 |    4,621 | +159.6% |
| Feb   |    2,250 |    4,177 |  +85.6% |
| Mar   |   15,376 |    4,515 |  -70.6% |
| Apr   |   14,405 |    4,478 |  -68.9% |
| May   |    5,070 |    5,001 |   -1.4% |
| Jun   |   11,273 |    7,045 |  -37.5% |
| Jul   |   17,101 |    8,032 |  -53.0% |
| Aug   |   17,519 |    8,228 |  -53.0% |
| Sep   |    8,914 |    6,647 |  -25.4% |
| Oct   |    6,661 |    5,014 |  -24.7% |
| Nov   |   15,257 |    4,441 |  -70.9% |
| Dec   |   14,164 |    4,657 |  -67.1% |

---

## Root Cause Analysis

### Fan Energy (-44%): Zone Design Load Mismatch

The dominant issue is that OpenBSE computes significantly lower zone design cooling
loads than E+. This directly drives smaller HVAC sizing and less runtime airflow.

**Evidence — E+ vs OpenBSE system design flows:**
- VAV_mid: E+ = 139.33 m3/s, OpenBSE (10 floors combined) = ~89 m3/s (36% less)
- Core_mid zone peak cooling: E+ = 85,286 W, OpenBSE = 42,476 W (50% less)

The Core_mid zone has NO exterior surfaces (all adiabatic). Its cooling load comes
entirely from internal gains. The 2x E+ load difference is caused by:

1. **Thermal mass sizing transients**: E+ runs a full transient design day where
   radiant heat (90% of lighting, 50% of equipment) is absorbed by surfaces and
   released with time delay. The accumulated effect creates peak cooling that exceeds
   instantaneous internal gains. OpenBSE's CTF may not fully capture this.

2. **Fan heat iteration**: E+ includes fan heat in the sizing loop (fan heats supply
   air, requiring more airflow, which increases fan heat). This positive feedback is
   iterated to convergence.

### Heating (+23%): Less Fan Heat Compensation

Higher heating gas directly follows from lower fan energy. In E+, VAV fans add ~1,010 MWh
of heat to the airstream annually. In OpenBSE, only ~565 MWh. The missing ~445 MWh of
fan heat must be replaced by the boiler, explaining most of the +93 MWh heating increase
(the rest is offset by system efficiency differences).

### Cooling (+13%): DC Continuous Fans + Load Differences

With continuous DC fans, more fan heat enters DC zones, increasing DX cooling load.
Additionally, the zone cooling load profile differs from E+ due to different thermal
mass behavior.

### Exterior Lighting (+6.2%): Astronomical Clock

OpenBSE computes 271 more nighttime hours than E+ (4,632 vs 4,361). The solar position
algorithm may differ in sunrise/sunset threshold (E+ uses -0.833 degree refraction
correction) or equation of time calculations.

### DHW (+5.4%): Water Heater Model Differences

E+ uses Hendron/Burch mains temperature correlation (monthly varying). OpenBSE uses
fixed 13.3C. Minor standby loss and efficiency differences contribute.

---

## Engine Changes Needed

| Priority | Change | Impact |
|----------|--------|--------|
| HIGH | Fix radiant heat distribution to surfaces in sizing design days | Fixes fans, pumps, heating, cooling |
| HIGH | Add fan heat feedback iteration in system sizing | Fixes fans, pumps |
| MEDIUM | Verify CTF transient behavior matches E+ for interior/adiabatic surfaces | Improves all HVAC end uses |
| LOW | Astronomical clock refraction correction (-0.833 deg threshold) | Fixes exterior lighting |
| LOW | Verify DHW water heater standby loss calculation | Fixes DHW |

## Files

| File | Description |
|------|-------------|
| `LargeOffice_Boulder.yaml.bak` | Source YAML with zone_multiplier fields |
| `expand_zones.py` | Script to expand multipliers into explicit zones |
| `LargeOffice_Boulder.yaml` | Expanded YAML (regenerate, do not edit directly) |
| `eplus_run/` | E+ run with simplified IDF and table output |
| `eplus_detailed_run/` | E+ run with hourly fan/pump meter outputs |
