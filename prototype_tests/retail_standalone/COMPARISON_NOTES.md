# Retail Standalone: OpenBSE against EnergyPlus

E+ reference: EnergyPlus 25.2 on `eplus_run/in.idf` (results in
`eplus_run/eplustbl.csv`), Denver-Aurora-Buckley TMY3. OpenBSE:
`examples/doe_retail_standalone.yaml` on the same weather file. Inputs are
audited object by object in `INPUT_AUDIT.md`.

## Annual end uses [kWh]

| End use | E+ | OpenBSE | Difference |
|---|---|---|---|
| Heating (gas) | 93,278 | 92,716 | −0.6 % |
| Cooling (electric) | 27,833 | 28,678 | +3.0 % |
| Interior lighting | 67,022 | 77,916 | +16.3 % |
| Exterior lighting | 10,733 | 10,908 | +1.6 % |
| Interior equipment | 53,972 | 53,971 | 0.0 % |
| Fans | 48,547 | 50,838 | +4.7 % |
| Water heating (gas) | 26,667 | 26,663 | 0.0 % |

Interior lighting: E+ dims Core_Retail's lights with its daylighting controls;
without them E+ uses 77,889 kWh (OpenBSE +0.03 %). OpenBSE has no daylighting
model yet (gap 1 in `INPUT_AUDIT.md`). Exterior lighting: daylight saving time
(E+ without it: 11,017 kWh) and the astronomical clock evaluated by the hour.

## By unit [kWh]

| | E+ | OpenBSE | Difference |
|---|---|---|---|
| PSZ-AC:1 (Back_Space) gas / cooling / fan | 17,490 / 3,300 / 4,896 | 16,188 / 3,271 / 5,120 | −7.4 % / −0.9 % / +4.6 % |
| PSZ-AC:2 (Core_Retail) | 55,548 / 18,101 / 30,860 | 56,401 / 18,829 / 33,339 | +1.5 % / +4.0 % / +8.0 % |
| PSZ-AC:3 (Point_Of_Sale) | 3,968 / 3,470 / 3,454 | 4,014 / 3,630 / 3,371 | +1.2 % / +4.6 % / −2.4 % |
| PSZ-AC:4 (Front_Retail) | 5,873 / 2,963 / 6,408 | 5,727 / 2,947 / 6,260 | −2.5 % / −0.5 % / −2.3 % |
| Unit heater (Front_Entry) gas / fan | 10,398 / 2,930 | 10,384 / 2,748 | −0.1 % / −6.2 % |

Zone loads agree: each zone's infiltration within 1–3 %, its sensible heating
and cooling from the system within 1–10 % (Core_Retail cooling +6 %, its
undimmed lights). Core_Retail's fan is high mainly through its design flow,
8.70 m³/s against E+'s 8.21: E+ converts each design-day timestep's load to a
flow at that timestep's zone temperature and `cp` at the design supply
humidity, so its flow is lowest during the morning pull-down from the setup
temperature; OpenBSE divides the peak load by the occupied-setpoint
temperature difference.

## Design loads (before the 1.2 sizing factor)

| Zone | E+ cooling | OpenBSE | E+ heating | OpenBSE |
|---|---|---|---|---|
| Back_Space | 19,007 W | 18,248 W (−4.0 %) | 23,687 W | 22,080 W (−6.8 %) |
| Core_Retail | 80,034 W | 78,952 W (−1.4 %) | 57,987 W | 52,341 W (−9.7 %) |
| Point_Of_Sale | 12,430 W | 12,057 W (−3.0 %) | 11,568 W | 10,716 W (−7.4 %) |
| Front_Retail | 9,791 W | 9,459 W (−3.4 %) | 11,568 W | 10,711 W (−7.4 %) |
| Front_Entry | — | — | 36,254 W | 36,190 W (−0.2 %) |

The design heating loads of the four retail zones are 7–10 % below E+'s
(the heating coils, autosized from them with the 1.2 factor, still meet the
annual heating: unmet heating hours in those zones are few). Not traced yet.

## Unmet hours

E+ reports 1.5 occupied heating hours and none cooling. OpenBSE counts every
hour of every conditioned zone at 0.2 K tolerance: 574 h heating, about half of
them the entry vestibule while its heater is locked out above 7.2 °C (E+ counts
no hours in a zone without occupants) and the rest the first hour of morning
warm-up from the setback, before anyone arrives (E+ counts occupied hours only),
and 1,151 h cooling, night hours 0.2–0.5 K
above the 23.89 °C no-setback cooling setpoint, inside the night cycle's
0.5 K tolerance (E+ counts only occupied hours, at 0.56 K).
