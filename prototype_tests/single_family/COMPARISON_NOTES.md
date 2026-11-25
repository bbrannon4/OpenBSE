# Single Family CZ5B Boulder — OpenBSE vs EnergyPlus Comparison

## Models

- **OpenBSE YAML**: `SingleFamily_CZ5B_Boulder.yaml`
- **EnergyPlus IDF**: `SingleFamily_CZ5B_Boulder_simplified.idf` (AirflowNetwork removed, constant infiltration, no ducts), run with EnergyPlus 25.2 (`eplus_run/`)
- **Original IDF**: `SingleFamily_CZ5B_Boulder.idf` (unmodified DOE prototype)
- **Weather file**: `weather/USA_CO_Denver-Aurora-Buckley.AFB.724695_TMY3.epw` (Denver-Aurora-Buckley AFB, WMO 724695, TMY3), for both

## Annual End Uses

| End Use | E+ [kWh] | OpenBSE [kWh] | Diff |
|---------|---------:|--------------:|-----:|
| Heating (Gas) | 7,057 | 7,257 | +2.8% |
| Cooling (Elec) | 1,840 | 1,933 | +5.0% |
| Interior Lighting | 1,038 | 1,038 | −0.1% |
| Exterior Lighting | 212 | 210 | −0.5% |
| Interior Equipment (electric and gas) | 10,083 | 10,077 | −0.1% |
| Fans (Elec) | 933 | 819 | −12.2% |
| Pumps (Elec) | 0 | 0 | — |
| DHW (Gas) | 2,157 | 2,203 | +2.1% |
| **Total** | **23,320** | **23,536** | **+0.9%** |

OpenBSE has no unmet hours. The E+ figures are from `eplus_run/eplustbl.csv`
and the monthly ones below from `eplus_run/eplusmtr.csv` (whose heating and
cooling totals differ from the annual table by under 0.1%).

### Monthly Heating (Gas) [kWh]

| Month | E+ | OpenBSE | Diff |
|---|---:|---:|---:|
| Jan | 1,121 | 1,150 | +2.5% |
| Feb | 1,107 | 1,132 | +2.2% |
| Mar | 827 | 853 | +3.1% |
| Apr | 684 | 704 | +3.0% |
| May | 114 | 124 | +8.5% |
| Jun | 1 | 1 | — |
| Jul | 0 | 0 | — |
| Aug | 1 | 1 | — |
| Sep | 55 | 60 | +9.1% |
| Oct | 703 | 732 | +4.1% |
| Nov | 921 | 950 | +3.1% |
| Dec | 1,518 | 1,550 | +2.1% |
| **Total** | **7,052** | **7,257** | **+2.9%** |

### Monthly Cooling (Electric) [kWh]

| Month | E+ | OpenBSE | Diff |
|---|---:|---:|---:|
| Jan | 20 | 22 | +7.8% |
| Feb | 25 | 26 | +4.3% |
| Mar | 65 | 67 | +2.7% |
| Apr | 58 | 59 | +1.9% |
| May | 149 | 158 | +6.2% |
| Jun | 380 | 399 | +4.9% |
| Jul | 384 | 408 | +6.0% |
| Aug | 364 | 386 | +5.9% |
| Sep | 252 | 262 | +3.9% |
| Oct | 87 | 90 | +3.4% |
| Nov | 36 | 38 | +5.5% |
| Dec | 18 | 19 | +4.6% |
| **Total** | **1,838** | **1,932** | **+5.1%** |

### Monthly Fans (Electric) [kWh]

| Month | E+ | OpenBSE | Diff |
|---|---:|---:|---:|
| Jan | 70 | 52 | −26.7% |
| Feb | 70 | 51 | −27.1% |
| Mar | 69 | 55 | −20.2% |
| Apr | 60 | 49 | −19.5% |
| May | 58 | 57 | −1.3% |
| Jun | 103 | 104 | +1.3% |
| Jul | 104 | 106 | +2.0% |
| Aug | 100 | 102 | +2.0% |
| Sep | 77 | 76 | −1.1% |
| Oct | 69 | 57 | −17.1% |
| Nov | 65 | 50 | −23.6% |
| Dec | 87 | 61 | −30.1% |
| **Total** | **932** | **819** | **−12.1%** |

## Observations

- **Gains and DHW** match within 0.5% and 2.1%.
- **Heating** is 2–4% above E+ in every heating month, a little more in the
  shoulder months May and September, where the loads are small.
- **Cooling** is 2–8% above E+ month by month, 5.0% over the year, at the
  edge of the 5% target.
- **Fans** match E+ within 2% in the cooling months (May–September) and run
  17–30% below it in the heating months. This gap is open.
