# Retail Standalone: OpenBSE against EnergyPlus

E+ reference: EnergyPlus 25.2 on `eplus_run/in.idf` (results in
`eplus_run/eplustbl.csv`), Denver-Aurora-Buckley TMY3. OpenBSE:
`examples/doe_retail_standalone.yaml` on the same weather file. Inputs are
audited object by object in `INPUT_AUDIT.md`.

## Annual end uses [kWh]

| End use | E+ | OpenBSE | Difference |
|---|---|---|---|
| Heating (gas) | 93,278 | 95,627 | +2.5 % |
| Cooling (electric) | 27,833 | 27,821 | 0.0 % |
| Interior lighting | 67,022 | 67,681 | +1.0 % |
| Exterior lighting | 10,733 | 10,908 | +1.6 % |
| Interior equipment | 53,972 | 53,971 | 0.0 % |
| Fans | 48,547 | 49,392 | +1.7 % |
| Water heating (gas) | 26,667 | 26,664 | 0.0 % |

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
| PSZ-AC:1 (Back_Space) gas / cooling / fan | 17,490 / 3,300 / 4,896 | 16,483 / 3,215 / 5,109 | −5.8 % / −2.6 % / +4.4 % |
| PSZ-AC:2 (Core_Retail) | 55,548 / 18,101 / 30,860 | 59,096 / 17,650 / 32,786 | +6.4 % / −2.5 % / +6.2 % |
| PSZ-AC:3 (Point_Of_Sale) | 3,968 / 3,470 / 3,454 | 4,084 / 3,590 / 3,355 | +2.9 % / +3.5 % / −2.9 % |
| PSZ-AC:4 (Front_Retail) | 5,873 / 2,963 / 6,408 | 5,827 / 2,899 / 6,220 | −0.8 % / −2.2 % / −2.9 % |
| Unit heater (Front_Entry) gas / fan | 10,398 / 2,930 | 10,392 / 2,758 | −0.1 % / −5.9 % |

Zone loads agree: each zone's infiltration within 1–3 %, its sensible cooling
from the system within 1–5 %. Core_Retail's gas is high (+6.4 %): the unit was
already about 8 % above E+'s heating without daylighting (E+ without it, about
52 MWh), which the undimmed lights hid. Core_Retail's design flow is 8.39 m³/s against E+'s 8.21 (+2.2 %).
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

E+ reports 1.5 occupied heating hours and none cooling. OpenBSE counts every
hour of every conditioned zone at 0.2 K tolerance: 580 h heating, about half of
them the entry vestibule while its heater is locked out above 7.2 °C (E+ counts
no hours in a zone without occupants) and the rest the first hour of morning
warm-up from the setback, before anyone arrives (E+ counts occupied hours only),
and 1,130 h cooling, night hours 0.2–0.5 K
above the 23.89 °C no-setback cooling setpoint, inside the night cycle's
0.5 K tolerance (E+ counts only occupied hours, at 0.56 K).
