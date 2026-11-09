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

Known E+ quirk not reproduced: four top-floor partitions name partners that do
not point back (e.g. `Core_top_ZN_5_Wall_South` names a ground-floor surface).
OpenBSE pairs interzone faces one-to-one, so these walls couple to the face of
their geometric neighbour on the same floor.

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

## Engine checks against E+

With the inputs matched, zone gains agree with E+ hour for hour, and January
nights were traced term by term through the zone air and surface heat
balances. Points where the engine and E+ must agree, checked on this model:

- No outdoor air enters through an idle outdoor-air system.
- Internal mass takes only diffuse and reflected solar; beam lands on the
  surfaces it reaches (`full_interior_and_exterior`).
- The VAV mixer and coils aim at the Warmest setpoint less the fan heat
  downstream, with the coils in E+'s draw-through order.
- Interior long-wave exchange sums to zero in each zone, with view factors from
  the zone's own polygons, `FixViewFactors` and grey-body interchange; per
  surface it matches E+ to a few percent (window +4.81 vs +4.83 kW,
  furnishings −3.28 vs −3.33 kW).
- Zone sizing averages the loads over an hour, ramps the design-day setpoints,
  and floors each zone's flow at its outdoor air; a mid core's box is within 1%
  of E+'s.
- VAV outdoor air follows the 62.1 ventilation rate procedure at the site's
  standard density.
- The basement data centre holds 26 °C under E+'s single-zone VAV control.

Annual end uses against E+ are in `COMPARISON_NOTES.md`.

## Gaps: E+ inputs OpenBSE cannot express yet

1. Daylight saving time (shifts every schedule an hour, 12 Mar - 5 Nov).
2. Thermostat setpoint schedules by day type (Saturday setback at 17:00 while
   the fans run to 18:00; `Perimeter_mid_ZN_2`'s mid-day standby hours).
   Design-day schedules are matched per thermostat.
3. People sensible fraction autocalculated from activity and zone temperature.
4. Lights visible fraction (short-wave, part of it lost through windows).
5. Two-speed DX coils (data centres). OpenBSE's multi-speed DX coil has no
   capacity or EIR curves, so the single-speed coil with the IDF's SmDX curves
   is the closer match.
6. Chilled water supply reset on return temperature.
7. Water-use sensible/latent gains to zones.
8. Non-reciprocal interzone partners (the four top-floor quirks above).
