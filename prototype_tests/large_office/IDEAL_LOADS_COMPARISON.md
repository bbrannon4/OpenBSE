# Large Office Ideal Loads Comparison: OpenBSE vs EnergyPlus

## Purpose

Isolate building thermodynamics from HVAC system modeling by replacing all
real HVAC equipment with ideal loads (perfect heating/cooling to setpoint).
This shows envelope, gains and infiltration differences between the two
engines apart from the HVAC.

## Configuration

| Parameter | Value |
|-----------|-------|
| Weather | Denver-Aurora-Buckley AFB TMY3 (CZ5B), as the full model |
| Office setpoints | Htg 21 C / Clg 24 C (constant, no setback) |
| DataCenter setpoints | Htg 18 C / Clg 27 C (constant) |
| HVAC capacity | NoLimit (effectively unlimited) |
| Outdoor air | None in either model |
| Ventilation | None in either model |
| Infiltration | Identical |
| Internal loads | Identical (lights, people, equipment) |
| Schedules | Identical |
| Constructions | Identical |

### Models

- **OpenBSE**: `LargeOffice_Boulder_ideal.yaml`, the envelope, gains and
  schedules of `LargeOffice_Boulder.yaml` with `ideal_loads` (constant
  setpoints, 100 MW capacity) in place of the air loops, plant loops,
  thermostats and outdoor air. Its end uses are baselined in CI with the other
  shipped models.
- **EnergyPlus**: the IDF with every AirLoopHVAC, Plant, Fan, Coil, Chiller,
  Boiler, Pump, Controller, SetpointManager and AvailabilityManager object
  replaced by a `ZoneHVAC:IdealLoadsAirSystem` per conditioned zone (NoLimit,
  no outdoor air) and the thermostat schedules by constant 21/24 C (office) and
  18/27 C (data centre). That IDF and its outputs are not kept in the
  repository.

## EnergyPlus Results (Reference)

The reference IDF used zone multipliers (×10 on the mid floor).

### Annual Zone Ideal Loads (kWh)

| Zone | Heating | Cooling |
|------|--------:|--------:|
| Basement | 0 | 234,454 |
| Core_bottom | 0 | 243,047 |
| Core_mid (x10) | 0 | 1,970,210 |
| Core_top | 0 | 196,705 |
| DataCenter_basement (x1) | 0 | 1,988,787 |
| DataCenter_bot | 0 | 39,198 |
| DataCenter_mid (x10) | 0 | 357,570 |
| DataCenter_top | 0 | 35,700 |
| Perimeter_bot (4 zones) | 9,524 | 123,807 |
| Perimeter_mid (4 zones x10) | 296,248 | 1,039,432 |
| Perimeter_top (4 zones) | 32,433 | 102,901 |
| **TOTAL** | **338,205** | **6,331,810** |

Key observations from E+ results:
- Core zones are cooling-only (high internal gains overwhelm envelope loss)
- Perimeter_mid zones have significant heating AND cooling (perimeter effects)
- North-facing perimeter (ZN_3) has highest heating, south-facing (ZN_1) has highest cooling
- DataCenter_basement dominates datacenter cooling (783 m2 floor area vs 36 m2 for others)
- The data centre zones exchange heat through their floors and ceilings with
  the adjacent plenum zones, which gives their cooling a seasonal swing
  (DataCenter_bot: 1,020 kWh in January, 5,588 kWh in August)

A zone-by-zone OpenBSE comparison against this reference is open.
