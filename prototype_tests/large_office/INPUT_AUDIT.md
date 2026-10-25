# Large Office: OpenBSE inputs vs the E+ reference IDF

Reference: `eplus_run/in.idf` (EnergyPlus 25.2, Denver-Aurora-Buckley TMY3).
Model: `LargeOffice_Boulder.yaml`. Every input below was checked against the
IDF object it comes from (October 2026). "Matched" means the OpenBSE input now
carries the E+ value; "gap" means OpenBSE has no input that can express it.

## Envelope (regenerated from the IDF)

`envelope_from_idf.py` rebuilds materials, constructions, window constructions,
surfaces and internal mass straight from the IDF, so names, vertices and
boundary conditions are E+'s own. Rerun it after editing the IDF:

    python3 envelope_from_idf.py eplus_run/in.idf LargeOffice_Boulder.yaml

What that fixed against the previous hand-built envelope:

| Item | Before | E+ / now |
|---|---|---|
| Mid-floor geometry | every `_f2.._f11` copy at z = 3.96-7.92 m | E+'s mid zone at z = 16.77-19.51 m (wind speed, exterior convection, infiltration) |
| Core-perimeter partitions | full 3.96 m floor-to-floor, 253 m² | 2.74 m to the drop ceiling, 173 m² |
| Perimeter floors/ceilings | rectangles (334 m²) overlapping at corners | trapezoids (313 m²) |
| Perimeter end walls | missing | 17.75 m² interzone walls between perimeter zones |
| Data centre strips | walls adiabatic, 3.6 m² | 108.6 m² walls to the core and to perimeter ZN_4, 2.5 m² to ZN_1/ZN_3 |
| Top floor | 3,563 m² outdoor roof | drop ceiling, adiabatic: E+ puts the roof on a plenum that is cut off from the building |
| Ground floor slab | adiabatic | interzone with the basement and data centre basement |
| Basement walls | 3.96 m of 200 mm block | 2.44 m C-factor walls (E+'s concrete + fictitious-resistance layers) |
| Basement internal mass | none | 7,126 m² |
| Constructions | one exterior wall, simple partition/ceiling | E+'s ground/mid/top walls, no-mass insulation and carpet, gypsum partitions, acoustic tile, doors |

Plenum zones are left out on purpose: in this IDF every surface between a
plenum and an occupied zone (and every plenum wall) is adiabatic, so they
exchange no heat with the building.

Known E+ quirk kept as is: four top-floor partitions name partners that do not
point back (e.g. `Core_top_ZN_5_Wall_South` names a ground-floor surface). With
`!zone` boundaries OpenBSE couples them to that zone's air instead of the
named surface's inside face.

## Loads and schedules

| Item | Status |
|---|---|
| People, lights, plug and IT equipment levels and fractions | matched (hourly gains equal outside DST) |
| Data centre IT schedule | matched: monthly 0.25/0.5/0.75/1.0 seasons (was a flat 0.625) |
| Infiltration | matched; the basement infiltration object (absent in E+) removed |
| Exterior lights, elevators | matched |
| Outdoor air | matched: 0.0004318 m³/s per m² for every zone, data centres included (was 62.1 office per-person + per-area, none for data centres) |
| Thermostats | matched for operating hours and design days (office WinterDesignDay 15.6-17.6-19.6-21.0 °C and SummerDesignDay 26.7-25.7-25.0-24.0 °C ramps; data centres constant); see gaps |
| Design days | matched; ASHRAE tau (0.423 / 2.02) added to the cooling day; E+'s two design days only (generated monthly cooling days off); loads averaged over 6 timesteps as `Sizing:Parameters` |

## HVAC

| Item | Status |
|---|---|
| VAV box minimum 0.3, reheat limit 40 °C, 0.5 max flow during reheat | matched (loop minimum was 0.25, the fan's power minimum) |
| VAV zone heating design supply 32.11 °C | matched |
| VAV supply fans, economizers, Warmest SAT reset 12.8-15.6 °C | already matched |
| VAV outdoor air by the 62.1 Ventilation Rate Procedure (E+ `Controller:MechanicalVentilation`) | matched: each VAV terminal carries the zone's 0.0004318 m³/s per m², which turns on OpenBSE's VRP (the loops held a fixed 9.2% of supply, so at box minimums they took about a third of E+'s outdoor air) |
| Data centre units: gas heating 0.8 / 0.7925, DX capacity and flow (E+ hard-sized, per floor), SHR 0.80066, COP 3.0 / 3.5, SmDX curves, 15.89 / 29.11 °C design supply | matched (heating was electric, DX autosized with default curves) |
| Basement single-zone VAV units: 100 kW HW coils, supply limits | matched |
| Basement single-zone VAV control (E+ PSZ-VAV: VAV no-reheat terminal, minimum 0.5 data centre / 0.3 basement, `SetpointManager:SingleZone` 15.89-29.11 / 12.89-32.11 °C, fan always on) | matched: continuous fan with a `single_zone` supply reset, which now turns the flow down before resetting the supply temperature (the units used to cycle with no reset) |
| VAV system design flow on the coincident zone sum (`Sizing:System` Type of Zone Sum to Use) | matched: `zone_sum: coincident` (mid-floor fan 14.44 m³/s per floor against E+'s 14.14) |

## Plant and DHW

| Item | Status |
|---|---|
| HW loop: 82.22 °C, 27.78 K ΔT, outdoor reset 82.2 °C at -6.7 °C → 65.6 °C at 10 °C, pump impeller 0.667, leaving-setpoint-modulated boiler | matched (ΔT was 11 K, no reset) |
| Chillers: E+ AppJ centrifugal cap/EIR-f(T) curves, 0.05608 m³/s design flow | matched (curves were different) |
| Condenser water loop | removed: E+'s chillers are air-cooled and the IDF has no condenser loop |
| Water heater 60.1 °C in the basement; mains by the E+ correlation (9.95 / 23.7) | matched |

## Engine differences found once the inputs matched

Zone gains matched E+ hour for hour, so January nights were traced term by
term through the zone air and surface heat balances:

1. **Outdoor air leaking in while the fans are off (fixed).** The envelope let
   each zone's design outdoor-air flow straight in at outdoor temperature
   whenever its air loop was off, an old LargeOffice tuning ("plenum effect").
   E+ admits no outdoor air through an idle OA system. It cost the mid core
   about 20 kW all night: 17-18 °C against E+'s 25.6 °C. Now 25.2 °C.
2. **Beam solar skipped in zones with internal mass (fixed).** With
   `full_interior_and_exterior`, a zone fell back to an area-by-type split
   whenever any surface lacked vertices, and internal mass never has them.
   The perimeter floor got ~20% of the solar instead of E+'s ~55%. Internal
   mass now takes only diffuse and reflected solar, as in E+, and the floor's
   daytime temperature and convection match.
3. **VAV supply temperature switched off by a zone vote (fixed).** The VAV
   controller turned off the cooling coil and the economizer whenever more
   zones wanted heat than cooling, and its central heating coil only did frost
   protection. In January it supplied 18-20 °C air against E+'s 14.4-16 °C,
   so the core drew ~40% more air and the perimeter boxes barely reheated.
   As in E+ (Warmest + SetpointManager:MixedAir), the mixer and both coils
   now aim at the Warmest setpoint less any fan heat still downstream, and
   the loops are in E+'s draw-through order (cooling coil, heating coil, fan).
4. **Interior long-wave exchange did not conserve energy (fixed).** Summed
   over a mid perimeter zone, E+'s net LW is ~0 W; OpenBSE's was -7.6 kW at
   night. The bounding-box view factors assumed each face held its geometric
   area, but the north face carried the partition plus 627 m² of furnishings
   (800 m² on a 200 m² face) and the 45° end walls fit no face. Interior
   long-wave now uses view factors from each zone's own polygons (internal
   mass by area, as in E+), E+'s `FixViewFactors`, and grey-body interchange
   (E+ `CalcScriptF`), so it sums to zero. Per surface it matches E+ to a few
   percent (window +4.81 vs +4.83 kW, furnishings -3.28 vs -3.33 kW), and the
   perimeter zones' January nights to 0.1-0.3 K.

With those four fixed, annual heating gas was 313 MWh against E+'s 472.5
(−34%). Following delivered heat back to sizing and the systems found:

5. **Zone sizing differed from E+'s (fixed).** OpenBSE sized on the peak
   timestep (E+ averages the loads over 6), held constant setpoints on the
   design days (E+ ramps them), added generated monthly cooling days, and
   added an outdoor-air load to each zone's airflow (E+ only floors it at the
   zone's outdoor-air flow). The VAV boxes were sized without the 1.15 sizing
   factor the controls used, and the fans summed unscaled zone flows (E+
   sums the scaled ones, coincidently here). A mid core's design heating load
   was 39.7 kW per floor against E+'s 9.5 kW; it is now 14.0 kW, and its box
   is within 1% of E+'s.
6. **No ventilation rate procedure, and the wrong air density in it (fixed).**
   E+ ventilates the VAV systems by the 62.1 VRP; the model held 9.2% outdoor
   air, so at box minimums it took about a third of E+'s, and the central
   heating coils never ran (E+ 42 MWh). The VRP also used sea-level density
   (23% too much outdoor air in Denver) and ran while ventilation was off.
7. **The basement data centre ran at 64-153 °C (fixed).** Its cycling-fan
   unit counted cold economizer air as if it flowed during the compressor's
   off-cycle, which cancelled the whole IT load, so the unit sat at its 1.3%
   PLR floor. The room heated the basement (41-48 °C in July) and the floor
   above, whose perimeter zones then needed almost no reheat. The fan was
   also sized at the 12.8 °C VAV supply temperature instead of its own
   15.89 °C. With E+'s single-zone VAV control it now holds 26.2 °C.

Annual results with all of the above, against E+ (MWh):

| End use | E+ | OpenBSE | |
|---|---|---|---|
| Heating gas | 472.5 | 419.3 | −11.3% |
| Cooling electricity | 589.5 | 560.9 | −4.8% |
| Fans | 1,042.6 | 1,037.2 | −0.5% |
| Pumps | 135.0 | 92.1 | −31.8% |
| DHW electricity | 125.1 | 112.5 | −10.1% |
| Interior lighting, equipment, exterior | | | within 1.4% |

Delivered heat is 332 vs 375 MWh: mid-floor reheat 242 vs 293 (−18%),
ground and top floors 33 vs 42 (−22%), and the central VAV heating coils 58
vs 42 (+40%). Reheat short while the central coils run long points at the
VAV supply temperature (Warmest) or the mixed air sitting warmer than E+'s;
that needs E+'s hourly supply-air node temperatures, which these runs did not
output. Cooling coil loads are within 2.5% on the VAV systems and 10% on the
data centres; fans are within 6% system by system except the basement units
(data centre −17%, CAV_bas −17%).

## Gaps: E+ inputs OpenBSE cannot express yet

1. Daylight saving time (shifts every schedule an hour, 12 Mar - 5 Nov).
2. Thermostat setpoint schedules by day type (Saturday setback at 17:00 while
   the fans run to 18:00; `Perimeter_mid_ZN_2`'s mid-day standby hours).
   Design-day schedules are now matched per thermostat.
3. People sensible fraction autocalculated from activity and zone temperature.
4. Lights visible fraction (short-wave, part of it lost through windows).
5. Two-speed DX coils (data centres).
6. Chilled water supply reset on return temperature.
7. Per-zone design supply temperatures in zone sizing. OpenBSE takes the
   building-wide minimum cooling supply temperature (12.8 °C from the VAV
   loops), so the data centres size at 0.65 m³/s per floor against E+'s
   0.90 m³/s at 15.89 °C; their design loads match (9.1 vs 8.7-10.0 kW).
8. Water-use sensible/latent gains to zones.
9. Interzone partners named per surface (the four top-floor quirks above).
