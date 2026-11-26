# Large Office Prototype — E+ Comparison Notes

## Models

- **E+ reference:** `eplus_run/in.idf`, the DOE 90.1-2019 Large Office, run
  annually with EnergyPlus 25.2. Its end uses are in `eplus_run/eplustbl.csv`.
- **OpenBSE:** `LargeOffice_Boulder.yaml` recreates that IDF as a person would
  by hand: every floor explicit (no zone multipliers), envelope, internal mass
  and interzone surfaces generated from the IDF by `envelope_from_idf.py`, and
  every other input checked against its IDF object in `INPUT_AUDIT.md`, which
  also lists what OpenBSE can't express. The plenum zones are left out
  because every plenum surface in this IDF is adiabatic.
- **Envelope check:** `LargeOffice_Boulder_ideal.yaml`, with ideal loads in place
  of the HVAC (`IDEAL_LOADS_COMPARISON.md`).

## Weather

Both run on Denver-Aurora-Buckley AFB TMY3 (WMO 724695): E+ on
`USA_CO_Denver-Aurora-Buckley.AFB.724695_TMY3.epw`, OpenBSE on the same file
in `weather/`. Despite the model's name, it does not use
Boulder weather.

## Annual end uses

| End use | E+ [kWh] | OpenBSE [kWh] | Difference |
|---|---:|---:|---:|
| Heating (gas) | 472,503 | 454,881 | −3.7% |
| Cooling | 589,511 | 531,018 | −9.9% |
| Fans | 1,042,617 | 1,041,864 | −0.1% |
| Pumps | 135,044 | 74,008 | −45.2% |
| Interior lighting | 1,558,539 | 1,558,539 | 0.0% |
| Exterior lighting | 279,583 | 279,273 | −0.1% |
| Interior equipment | 4,076,778 | 4,076,750 | 0.0% |
| Exterior equipment | 713,075 | 713,075 | 0.0% |
| Service hot water | 125,133 | 125,142 | 0.0% |

OpenBSE values are the committed baselines in `tests/model_baselines.json`
(2026-11-25). E+ values are `eplustbl.csv` (GJ × 277.78). Every gap below is
traced at component level, with an E+ 25.2 rerun of `eplus_run/in.idf` that
writes the plant and coil node data every timestep. The component figures in the
sections below come from tracing runs without daylight saving time. Daylight
saving moves cooling 1.5% and pumps 0.1%, so the comparisons there hold.

## Pumps

E+'s pumps: two constant-speed chiller branch pumps, 60,507 kWh each, a
variable-speed secondary pump, 13,330 kWh, the hot-water pump, 331 kWh, and the
service hot water pump, 369 kWh, which both engines meter under pumps (OpenBSE's
hot-water and service hot water pumps match E+'s to 4 and 0 kWh).

**Primary pumps run together.** `CoolSys1_Supply` serves an `UncontrolledOn`,
`Ideal` `HeatExchanger:FluidToFluid` autosized to the whole primary flow
(0.1108 m³/s). The exchanger requests that flow, so E+ turns the unloaded
chiller's branch pump back on: both pumps run 6,908 h at full flow (10.54 kW
each) while chiller 2 carries load for only 713 h. The model reproduces it with
a decoupled loop (the secondary pump `loop_side: demand`), whose two-pump
constant-speed primary bank runs whenever the loop does. Running, OpenBSE's bank
draws 21.3 kW against E+'s 21.1 kW.

**Chilled water is reset to 9-10 °C.** A
`SetpointManager:ReturnTemperature:ChilledWater` holds the primary return at
10.2 °C between 6.7 and 10.22 °C. With 111 kg/s of primary flow and about 14 kg/s
of coil flow the primary ΔT is about 1 K, so E+'s supply is mostly at 10 °C (mean
8.9 °C while the plant runs). The model has the same reset (mean 8.7 °C).
The coils draw more water at that temperature: OpenBSE's secondary pump moves
16.8 kg/s while it runs (E+ 13.5) and draws 1.93 kW (E+ 1.74 kW).

**Loop hours.** With the running draw matched, the gap is how long the chilled
water runs: OpenBSE's primary pumps run 3,160 h, E+'s the equivalent of 5,740 h
at full flow (60,507 kWh ÷ 10.54 kW; some running only part of a timestep, in
6,908 h). Almost all of the difference is the basement data centre:

- The data centre and basement units are E+ `AirLoopHVAC:UnitarySystem`s with
  draw-through fans, and the model lists their coils before their fans, as E+
  does. With the fan first (blow-through), the single-zone reset would aim the
  coil one fan rise below the supply temperature, so the coil would trim 0.7 K
  every timestep, the chilled water would never stop and the zone would sit
  0.8 K under its 27 °C setpoint.
- E+'s single-zone VAV unit then runs the basement data centre at full fan flow
  in the high-IT months and overcools it to 23.5-24.5 °C, against its 27 °C
  setpoint: 4,285 h below 26.5 °C (mean 24.8 °C), with 314 of the coil's
  668 MWh in those hours, and 3,262 h in which that coil alone keeps the plant
  on. The cause is in `SZVAVModel.cc`: without an air-terminal mixer, the
  region-1 boundary load is `minAirMassFlow·(h(T_min supply) − h(T_inlet))`, from
  the unit's inlet (mixed-air) temperature rather than the zone's. With the
  economizer bringing the mixed air to within a fan rise of the minimum supply
  temperature, both boundary loads collapse to a few kilowatts, the 360 kW load
  falls in region 3, and the unit runs at maximum flow with its coil
  over-delivering. OpenBSE's single-zone VAV control meets the load, as the
  model intends: the zone holds 26.7 °C on average, never below 26.5 °C, and
  the coil runs 2,516 h (401 MWh).

The pump gap is therefore E+ overcooling its data centre, not OpenBSE physics:
over E+'s 5,740 full-flow hours OpenBSE's primary bank would draw 122 MWh (E+
121 MWh). The same overcooling is most of the cooling gap: the
data centre coil cools 401 MWh against E+'s 668 (the other coils: VAV 1,783 vs
1,815, basement 94 vs 107 MWh).

## Chillers

The chillers follow the IDF's operation: chiller 1 alone up to 1.57 MW, both above
that at uniform load (`PlantEquipmentOperation:CoolingLoad`, `UniformLoad`). With
both running, each sits at a part load of 0.62. Sequential staging would run
chiller 1 at 1.0 and hold chiller 2 at its 0.25 minimum, false-loading, at a
cost of 11 MWh of chiller electricity a year. Chiller 2 carries 552 MWh in
570 h, 23.5% of the cooling (E+ 651 MWh in 713 h, 23.9%). The remaining cooling
gap is the data centre overcooling above: the chillers deliver 2,346 MWh, against
E+'s 2,720.

## Heating

Both engines run the schedules on daylight saving time from 12 March to
5 November (`RunPeriodControl:DaylightSavingTime`, `daylight_saving_time`).
Without it, OpenBSE's gas is 5.5% under E+'s. E+ starts the building an hour
earlier against the clock, at 05:00 standard time, and its mid-floor reheat in
that hour alone is 26.5 MWh. With it, gas rises 8.4 MWh to 3.7% under E+.

Without daylight saving, the boiler-load gap is the perimeter reheat (mid
floors −22.5, bottom −3.1, top −2.1 MWh against E+), less the central coils
(+6.4 MWh). Outside the daylight saving period the boiler load is within
2.4% of E+'s, with the reheat low and the central coils high, a split between
the VAV supply-air temperature and the terminal reheat that is not traced
further here.

## Open gaps

- **E+ data centre overcooling** (above,
  [#497](https://github.com/bbrannon4/OpenBSE/issues/497)): a deliberate
  difference; OpenBSE meets the zone load.
