# ASHRAE Standard 140-2023 Gap Analysis for OpenBSE

## Section 7: Building Thermal Envelope and Fabric Load Tests

### 600 Series (Low-Mass) — What We Need

| Case | Test Feature | Status | Notes |
|------|-------------|--------|-------|
| 600 | Base case: low-mass, south windows, 20/27 deadband | DONE | Ideal loads (1MW heating + 1MW cooling), nonproportional thermostat, layered constructions, interior solar distribution (floor 64.2%), raised floor boundary |
| 610 | South shading (overhang) | DONE | Geometric beam shadow (Sutherland-Hodgman polygon clipping), diffuse sky view factor reduction, explicit shading surface vertices |
| 620 | East/west window orientation | DONE | Supported with vertex geometry |
| 630 | East/west shading (overhang + fins) | DONE | Auto-generated overhang/fin geometry, multi-caster beam shadow union (8x8 grid), diffuse sky+horizon shading ratios |
| 640 | Thermostat setback schedule | DONE | ThermostatScheduleEntry with time-of-day setpoints, wrap-past-midnight support |
| 650 | Night ventilation | DONE | VentilationScheduleEntry with ACH rate, conditional min_indoor_temp and outdoor_temp_must_be_lower |
| 660 | Low-e argon windows | PARTIAL | Need angular-dependent transmittance, argon gas fill properties |
| 670 | Single-pane windows | DONE | Supported (different U/SHGC values) |
| 680 | Increased insulation | DONE | Just different material thicknesses |
| 685 | 20/20 thermostat (no deadband) | DONE | Ideal loads with heating_setpoint = cooling_setpoint = 20.0 |
| 695 | Increased insulation + 20/20 thermostat | DONE | Combination of 680 and 685 |
| 600FF | Free-float (no HVAC) | DONE | `conditioned: false` or omit `ideal_loads`; zone temperature floats freely |
| 650FF | Free-float night ventilation | DONE | Free-float mode with ventilation_schedule entries |
| 680FF | Free-float increased insulation | DONE | Free-float mode with thicker insulation |

### Required Features (Priority Order for 600 Series)

1. ~~**Ideal mechanical cooling**~~ DONE — IdealLoadsAirSystem with 1MW heating + 1MW cooling, 100% convective to zone air
2. ~~**Nonproportional thermostat**~~ DONE — On/off control: heat if T < setpoint, cool if T > setpoint, deadband between
3. ~~**Thermostat schedules**~~ DONE — ThermostatScheduleEntry with start_hour, end_hour, wrap-past-midnight
4. ~~**External shading**~~ DONE — Cases 610, 630: geometric beam shadow calculations with Sutherland-Hodgman polygon clipping, diffuse sky view factor shading, auto-generated overhang/fin geometry
5. ~~**Scheduled ventilation**~~ DONE — VentilationScheduleEntry with conditional temperature logic (Case 650)
6. ~~**Free-float mode**~~ DONE — Zones can be unconditioned (conditioned: false) or simply have no ideal_loads
7. ~~**Interior solar distribution**~~ DONE — InteriorSolarDistribution with floor/wall/ceiling fractions (default: ASHRAE 140 values)
8. ~~**Raised floor boundary**~~ DONE — Exterior boundary condition on floor surfaces
9. ~~**Layered constructions**~~ DONE — Multi-layer material constructions with thermal properties

### Remaining Gaps

Only one feature from the 600 series remains unimplemented:
- **Angular-dependent window transmittance** (Case 660): currently uses a Fresnel double-pane model which works well for clear glass but may need spectral extension for low-e coated glass

### Validation Status (14/16 ASHRAE 140 Metrics Pass)

All 8 primary cases (600, 610, 620, 630, 900, 910, 920, 930) have been tested. 14 of 16 annual energy metrics (heating + cooling per case) pass the ASHRAE 140-2023 acceptance ranges. The two failures are:
- **Case 900 Cooling:** 2726 kWh (max 2714, over by 12 kWh / <1%)
- **Case 910 Cooling:** 1658 kWh (max 1490, over by 168 kWh)

Root cause: The 900-to-910 overhang cooling reduction delta is too small because OpenBSE distributes all transmitted solar using fixed interior fractions (64.2% to floor), while E+ distributes beam solar geometrically (~90% to floor). Fix requires implementing separate beam/diffuse interior solar distribution.

### Case 600 Base Spec Details (from ASHRAE 140-2023 Section 7.2.1)
- Geometry: 8m x 6m x 2.7m, 12m2 south windows (2 x 3m x 2m)
- Walls: Plasterboard/Fiberglass/Wood siding (U~0.514 W/m2K)
- Roof: Plasterboard/Fiberglass/Roofdeck (U~0.318 W/m2K)
- Floor: Timber/Insulation raised floor (U~0.039 W/m2K), exposed to outdoor air, no solar
- Windows: Clear double-pane, U=2.10, SHGC=0.769, angular-dependent transmittance
- Infiltration: 0.5 ACH constant (altitude-corrected to 0.414 ACH at 1650m)
- Internal gains: 200W continuous, 60% radiative, 40% convective
- HVAC: Ideal 1000kW heating + 1000kW cooling, 100% efficient, 100% convective
- Thermostat: Heat ON if T<20, Cool ON if T>27 (nonproportional/on-off)
- Interior solar distribution: Floor 64.2%, Ceiling 16.7%, etc.
- Ground reflectance: 0.2
- Site altitude: 1650m

### 900 Series (High-Mass) — Additional Requirements
- Heavy construction materials (concrete block walls, concrete slab floor)
- Case 960 (Sunspace) needs multi-zone thermal coupling
- Otherwise same feature requirements as 600 series

### In-Depth Tests (200-400 Series) — Additional Requirements
- High-conductance wall elements (opaque panels with window-like U-value)
- Variable interior/exterior IR emittance
- Constant surface coefficients (override calculated values)
- Interior shortwave absorptance variations
