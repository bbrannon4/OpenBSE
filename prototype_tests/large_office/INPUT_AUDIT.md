# Large Office: OpenBSE inputs vs the E+ reference IDF

Reference: `eplus_run/in.idf` (EnergyPlus 25.2, Denver-Aurora-Buckley TMY3).
Model: `LargeOffice_Boulder.yaml`. Every input below was checked against the
IDF object it comes from. "Matched" means the OpenBSE input carries the E+
value; "gap" means OpenBSE has no input that can express it.

## Envelope (regenerated from the IDF)

`envelope_from_idf.py` rebuilds materials, constructions, window constructions,
surfaces and internal mass straight from the IDF, so names, vertices and
boundary conditions are E+'s own. Rerun it after editing the IDF:

    python3 envelope_from_idf.py eplus_run/in.idf LargeOffice_Boulder.yaml

What the generated envelope carries:

| Item | As in E+ |
|---|---|
| Mid-floor geometry | every `_f2.._f11` copy at E+'s mid zone height, z = 16.77-19.51 m (wind speed, exterior convection, infiltration) |
| Core-perimeter partitions | 2.74 m to the drop ceiling, 173 m² |
| Perimeter floors/ceilings | trapezoids (313 m²) |
| Perimeter end walls | 17.75 m² interzone walls between perimeter zones |
| Data centre strips | 108.6 m² walls to the core and to perimeter ZN_4, 2.5 m² to ZN_1/ZN_3 |
| Top floor | drop ceiling, adiabatic: E+ puts the roof on a plenum that is cut off from the building |
| Ground floor slab | interzone with the basement and data centre basement |
| Basement walls | 2.44 m C-factor walls (E+'s concrete + fictitious-resistance layers) |
| Basement internal mass | 7,126 m² |
| Constructions | E+'s ground/mid/top walls, no-mass insulation and carpet, gypsum partitions, acoustic tile, doors |

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
| People, lights, plug and IT equipment levels and fractions | matched |
| `RunPeriodControl:DaylightSavingTime` (2nd Sunday in March – 1st Sunday in November) | matched (`daylight_saving_time`) |
| Data centre IT schedule | matched: monthly 0.25/0.5/0.75/1.0 seasons |
| Infiltration | matched; the basement has none, as in E+ |
| Exterior lights, elevators | matched |
| Outdoor air | matched: 0.0004318 m³/s per m² for every zone, data centres included |
| Thermostats | matched for operating hours and design days (office WinterDesignDay 15.6-17.6-19.6-21.0 °C and SummerDesignDay 26.7-25.7-25.0-24.0 °C ramps; data centres constant); see gaps |
| Design days | matched, except that the cooling day's ASHRAE tau (0.423 / 2.02) is given but not used ([#50](https://github.com/bbrannon4/OpenBSE/issues/50)); E+'s two design days only (generated monthly cooling days off); loads averaged over 6 timesteps as `Sizing:Parameters` |

## HVAC

| Item | Status |
|---|---|
| VAV box minimum 0.3, reheat limit 40 °C, 0.5 max flow during reheat | matched |
| VAV zone heating design supply 32.11 °C | matched |
| VAV supply fans, economizers, Warmest SAT reset 12.8-15.6 °C | matched |
| VAV outdoor air by the 62.1 Ventilation Rate Procedure (E+ `Controller:MechanicalVentilation`) | matched: each VAV terminal carries the zone's 0.0004318 m³/s per m², which turns on OpenBSE's VRP |
| Data centre units: gas heating 0.8 / 0.7925, DX capacity and flow (E+ hard-sized, per floor), SHR 0.80066, COP 3.0 / 3.5, SmDX curves, 15.89 / 29.11 °C design supply | matched |
| Basement single-zone VAV units: 100 kW HW coils, supply limits | matched |
| Basement single-zone VAV control (E+ PSZ-VAV: VAV no-reheat terminal, minimum 0.5 data centre / 0.3 basement, `SetpointManager:SingleZone` 15.89-29.11 / 12.89-32.11 °C, fan always on) | matched: continuous fan with a `single_zone` supply reset, which turns the flow down before resetting the supply temperature |
| VAV system design flow on the coincident zone sum (`Sizing:System` Type of Zone Sum to Use) | matched: `zone_sum: coincident` (mid-floor fan 14.44 m³/s per floor against E+'s 14.14) |
| Basement and data centre unitary systems (`AirLoopHVAC:UnitarySystem`, Fan Placement `DrawThrough`): cooling coil, heating coil, then fan | matched: equipment in E+'s order, fan last (with the fan first, the single-zone reset would aim the coil one fan rise below the supply temperature, and the basement data centre coil would trim 0.7 K every timestep) |
| Basement data centre economizer: differential dry-bulb with a 23.89 °C maximum limit | matched: `max_outdoor_temp: 23.8889` |

## Plant and DHW

| Item | Status |
|---|---|
| HW loop: 82.22 °C, 27.78 K ΔT, outdoor reset 82.2 °C at -6.7 °C → 65.6 °C at 10 °C, pump impeller 0.667, leaving-setpoint-modulated boiler | matched |
| Chillers: E+ AppJ centrifugal cap/EIR-f(T) curves, 0.05608 m³/s design flow | matched |
| Chilled water primary/secondary: primary loop with two constant-speed chiller branch pumps serving an `UncontrolledOn` ideal `HeatExchanger:FluidToFluid` autosized to the primary flow; secondary loop with the variable-speed pump on its demand side | matched: one decoupled loop, the secondary pump `loop_side: demand`, so the two-pump primary bank runs at full flow whenever the loop runs, as E+ runs both branch pumps (the exchanger requests the whole primary flow) |
| Chilled water supply reset, `SetpointManager:ReturnTemperature:ChilledWater` on the primary loop: 6.7-10.22 °C, return held at 10.2 °C | matched: `setpoint_reset: return_temperature`. The secondary loop's own return-temperature manager has no effect in E+ (its exchanger is uncontrolled) |
| Chiller operation, `PlantEquipmentOperation:CoolingLoad`: chiller 1 alone to 1.57 MW, both above, loop Load Distribution Scheme `UniformLoad` | matched: `operation` with those load ranges and `distribution: uniform` |
| Condenser water loop | none: E+'s chillers are air-cooled and the IDF has no condenser loop |
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

## Gaps: E+ inputs the model does not carry

1. Thermostat setpoint schedules by day type (Saturday setback at 17:00 while
   the fans run to 18:00; `Perimeter_mid_ZN_2`'s mid-day standby hours). The
   engine takes them as `heating_setpoint_schedule` and
   `cooling_setpoint_schedule`; the model holds its occupied setpoints instead.
   Design-day schedules are matched per thermostat.
2. Lights visible fraction (short-wave, part of it lost through windows).
3. Two-speed DX coils (data centres): the model uses the single-speed coil with
   the IDF's SmDX curves.
4. Water-use sensible/latent gains to zones
   ([#288](https://github.com/bbrannon4/OpenBSE/issues/288)).
5. Non-reciprocal interzone partners (the four top-floor quirks above).
