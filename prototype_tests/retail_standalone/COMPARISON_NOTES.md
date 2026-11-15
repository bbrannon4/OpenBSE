# Retail Standalone: OpenBSE against EnergyPlus

E+ reference: EnergyPlus 25.2 on `eplus_run/in.idf` (results in
`eplus_run/eplustbl.csv`), Denver-Aurora-Buckley TMY3. OpenBSE:
`examples/doe_retail_standalone.yaml` on the same weather file. Inputs are
audited object by object in `INPUT_AUDIT.md`.

## Annual end uses [kWh]

| End use | E+ | OpenBSE | Difference |
|---|---|---|---|
| Heating (gas) | 93,278 | 90,705 | −2.8 % |
| Cooling (electric) | 27,833 | 27,758 | −0.3 % |
| Interior lighting | 67,022 | 67,681 | +1.0 % |
| Exterior lighting | 10,733 | 10,908 | +1.6 % |
| Interior equipment | 53,972 | 53,971 | 0.0 % |
| Fans | 48,547 | 49,185 | +1.3 % |
| Water heating (gas) | 26,667 | 26,666 | 0.0 % |

Interior lighting: Core_Retail's daylighting controls dim its lights to
51.3 MWh (E+ 50.6, +1.3 %). Its daylight factors agree with E+'s
`eplusout.dfs` to 0.1 % for the clear and overcast skies; E+ computes them
with the sun averaged over each 30-day shading period, OpenBSE with each
day's sun, so the hours a sun patch lands on a reference point differ.
Daylighting adds 3.2 MWh of heating and saves 1.3 MWh of cooling (E+: 3.5 and
1.4). Exterior lighting: daylight saving time
(E+ without it: 11,017 kWh) and the astronomical clock evaluated by the hour.

## By unit [kWh]

| | E+ | OpenBSE | Difference |
|---|---|---|---|
| PSZ-AC:1 (Back_Space) gas / cooling / fan | 17,490 / 3,300 / 4,896 | 16,816 / 3,284 / 5,003 | −3.8 % / −0.5 % / +2.2 % |
| PSZ-AC:2 (Core_Retail) | 55,548 / 18,101 / 30,860 | 53,979 / 17,956 / 31,998 | −2.8 % / −0.8 % / +3.7 % |
| PSZ-AC:3 (Point_Of_Sale) | 3,968 / 3,470 / 3,454 | 3,772 / 3,616 / 3,303 | −4.9 % / +4.2 % / −4.4 % |
| PSZ-AC:4 (Front_Retail) | 5,873 / 2,963 / 6,408 | 5,685 / 2,901 / 6,126 | −3.2 % / −2.1 % / −4.4 % |
| Unit heater (Front_Entry) gas / fan | 10,398 / 2,930 | 10,453 / 2,756 | +0.5 % / −5.9 % |

Zone loads agree: each zone's infiltration within 1–3 %, its sensible cooling
from the system within 1–5 %. During occupied winter hours the units hold
their zones at the heating setpoint as E+'s do: with a continuous fan they
meet the signed load to the setpoint, adding only the heat that keeps the
cold outdoor air from taking the zone below it (Engineering Reference,
multi-speed packaged units). The units' gas is 3–5 % below E+'s.
Core_Retail's design flow is 8.39 m³/s against E+'s 8.21 (+2.2 %).
Both convert each design-day timestep's load to a flow with the zone
temperature and `cp` at the IDF's design supply humidity (0.007 kg/kg cooling,
0.008 heating) and size on the averaged flows; E+ takes the zone temperature
at the start of each step, OpenBSE at its end, and E+'s recorded load runs
higher for a step or two after the morning recovery (Engineering Reference,
zone sizing).

## Design loads (before the 1.2 sizing factor)

| Zone | E+ cooling | OpenBSE | E+ heating | OpenBSE |
|---|---|---|---|---|
| Back_Space | 19,007 W | 17,956 W (−5.5 %) | 23,687 W | 22,791 W (−3.8 %) |
| Core_Retail | 80,034 W | 77,068 W (−3.7 %) | 57,987 W | 55,403 W (−4.5 %) |
| Point_Of_Sale | 12,430 W | 11,986 W (−3.6 %) | 11,568 W | 10,988 W (−5.0 %) |
| Front_Retail | 9,791 W | 9,374 W (−4.3 %) | 11,568 W | 10,982 W (−5.1 %) |
| Front_Entry | — | — | 36,254 W | 36,212 W (−0.1 %) |

The heating design day runs under a clear sky, as E+'s does (sky
temperature −36.8 °C at −17.9 °C), and outside faces use the IDF's TARP
convection, whose forced term falls with the size of the face: the 1,600 m²
roof's outside coefficient is 2.83 W/(m²·K), as in E+. Core_Retail's surface
temperatures then match E+'s within 0.05 K on the heating design day, and its
steady daytime design-day load is 2.4 % below E+'s (interior long-wave
exchange: OpenBSE's polygon view factors leave the floor and furnishings
0.03 K warmer). The design heating loads are 4–5 % below E+'s; the other
2 points are E+'s sizing lag after the morning recovery, which records 1–2
steps of extra load in the averaging window (Engineering Reference, zone
sizing). The cooling design day's solar is not yet E+'s ASHRAE tau model
([#50](https://github.com/bbrannon4/OpenBSE/issues/50)).

## Unmet hours

At the IDF's 0.556 K tolerance, counting every hour (E+ "Time Setpoint Not
Met", not only occupied hours) [h]:

| Zone | E+ heating | OpenBSE | E+ cooling | OpenBSE |
|---|---|---|---|---|
| Back_Space | 63.25 | 64.5 | 90.5 | 83.0 |
| Core_Retail | 0.25 | 0 | 0 | 0 |
| Point_Of_Sale | 34.25 | 35.2 | 259.75 | 256.5 |
| Front_Retail | 50.25 | 51.2 | 160.5 | 132.8 |
| Front_Entry | 382.5 | 356.5 | 0 | 0 |
| Facility (any zone) | 459.5 | 434.0 | 425.25 | 403.2 |

The entry vestibule's hours are those its heater is locked out above 7.2 °C.
E+ reports 1.5 occupied heating hours and none cooling.
