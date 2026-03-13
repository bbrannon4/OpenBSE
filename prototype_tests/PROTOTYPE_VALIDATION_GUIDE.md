# E+ Prototype Validation Guide

This document defines the process for validating OpenBSE against EnergyPlus
DOE prototype buildings. Follow it exactly. Do not skip steps. Do not
approximate. Do not guess.

## Goal

Each energy end-use (heating, cooling, fans, lighting, equipment, DHW, pumps)
must independently match EnergyPlus within **5%**. Total-energy agreement is
not sufficient — every end-use must pass on its own.

---

## Phase 1: Prepare the E+ Reference

Before writing any YAML, create a clean E+ reference run.

1. **Start from the official DOE prototype IDF** for the building type,
   climate zone, and code vintage.
2. **Simplify the IDF** to remove features OpenBSE does not support.
   Document every change. Typical simplifications:
   - Remove `AirflowNetwork` objects; replace with
     `ZoneInfiltration:DesignFlowRate` using the same design flow rates.
   - Remove `Daylighting:Controls` (OpenBSE has no daylighting model).
   - Remove any objects that reference external preprocessors
     (Basement, Slab, Kiva) if OpenBSE cannot replicate them — but note
     the ground temperatures they produce and use
     `Site:GroundTemperature:BuildingSurface` with those monthly values.
3. **Run E+ on the simplified IDF.** Confirm it completes with 0 Severe
   Errors. Save all output files (`eplustbl.csv`, `eplusout.csv`,
   `eplusout.err`, etc.) in a subfolder (e.g., `eplus_sf_run/`).
4. **Record the reference end-use values** (from `eplustbl.csv`) in
   `compare_end_uses.py`. These are the numbers we validate against.

---

## Phase 2: Build the YAML — Object-by-Object IDF Audit

Go through the simplified IDF **class by class, object by object, field by
field**. For every object, confirm its equivalent exists in the YAML with
identical numeric values. Do not round. Do not approximate. Copy values with
full decimal precision from the IDF.

### 2.1 Site and Simulation Settings

| IDF Class | YAML Location | Check |
|-----------|---------------|-------|
| `SimulationControl` | `simulation:` | Timestep, run period, terrain |
| `Timestep` | `simulation.timesteps_per_hour` | Must match exactly |
| `RunPeriod` | `simulation.start_month/day`, `end_month/day` | Full calendar year |
| `Site:Location` | Weather file (EPW) | Same EPW file, same location |
| `Site:GroundTemperature:BuildingSurface` | `simulation.ground_surface_temperatures` | 12 monthly values in °C. If E+ IDF has no object, both default to 18°C. If the IDF uses a preprocessor (Kiva, Basement, Slab), extract the monthly ground temps it produces and enter them explicitly. |
| `SizingPeriod:DesignDay` | `design_days:` | All fields: design temp, daily range, humidity type/value, pressure, wind speed, month, day, sky model |

### 2.2 Zones

| IDF Class | YAML Location | Check |
|-----------|---------------|-------|
| `Zone` | `zones:` | Every zone in the IDF must exist in the YAML. Check: name, volume (m³), floor area (m²), origin coordinates, multiplier. |
| Conditioned vs unconditioned | `zones[].conditioned` | Conditioned zones get `true`, all others `false`. |
| `InternalMass` | `zones[].internal_mass` | Construction name, area (m²). |

**Rule:** If the IDF has N zones, the YAML must have N zones. No combining.
No omitting. Unconditioned zones (attics, basements, garages, plenums) must
be modeled explicitly as `conditioned: false`.

### 2.3 Surfaces and Sub-Surfaces

For every `BuildingSurface:Detailed` in the IDF:

| IDF Field | YAML Field | Check |
|-----------|------------|-------|
| Surface Type | `type:` (wall/floor/roof) | Exact match |
| Construction Name | `construction:` | Must reference matching construction |
| Zone Name | `zone:` | Must match zone name |
| Outside Boundary Condition | `boundary:` | `Outdoors` → `outdoor`, `Ground` → `ground`, `Zone` → `!zone 'zone_name'`, `Adiabatic` → `adiabatic` |
| Vertices | `vertices:` | Copy all vertex coordinates with full decimal precision. For area-based surfaces (triangles, surfaces with sub-surfaces removed), use `area:`, `azimuth:`, `tilt:` instead. |

For every `FenestrationSurface:Detailed`, `Window`, `Door`, `Door:Interzone`:

| IDF Field | YAML Field | Check |
|-----------|------------|-------|
| Surface Type | `type: window` or `type: wall` (for doors) | Windows → `window`, Doors → `wall` with door construction |
| Construction | `construction:` | Window construction for windows, door construction for doors |
| Area / Vertices | `area:` or `vertices:` | Compute area from IDF vertices or width×height |
| Parent Surface | `parent_surface:` (windows only) | Engine subtracts window area from parent wall |
| Boundary | `boundary:` | Interzone doors: `!zone 'adjacent_zone'` |

**Interzone surfaces:** Both sides must be defined explicitly. The paired
surface uses the reversed construction (layers in opposite order). Create
a reversed construction in the YAML for each interzone pair.

### 2.4 Constructions and Materials

| IDF Class | YAML Location | Check |
|-----------|---------------|-------|
| `Material` | `materials:` | Every field: conductivity, density, specific heat, roughness, solar absorptance, thermal absorptance. Full precision. |
| `Material:NoMass` | `simple_constructions:` | Convert R-value to U-factor. Estimate thermal capacity. |
| `Material:AirGap` | Include in `simple_constructions:` | Use R-value from IDF. |
| `Construction` | `constructions:` | Layer order (outside to inside), material names, thicknesses. Must match IDF exactly. |
| `WindowMaterial:SimpleGlazingSystem` | `window_constructions:` | U-factor, SHGC, visible transmittance. |

### 2.5 Internal Gains

| IDF Class | YAML Location | Check |
|-----------|---------------|-------|
| `People` | `people:` | Zone, count, activity level (W/person), radiant fraction, schedule name |
| `Lights` | `lights:` | Zone, power (W), radiant fraction (E+ radiant + visible), schedule name |
| `ElectricEquipment` | `equipment:` | Zone, power (W), radiant fraction, lost fraction (= E+ lost + latent), schedule name |
| `GasEquipment` | `equipment:` | Same as electric but use full gas input power. Set lost_fraction = E+ lost + latent. |

**Fraction conversions:**
- `lost_fraction` = E+ Fraction Lost + E+ Fraction Latent (OpenBSE has no
  latent model, so latent heat is treated as lost)
- `radiant_fraction` = E+ Fraction Radiant / (1 − lost_fraction)
- Verify: `(1 − lost_fraction) × radiant_fraction` should equal E+ Fraction
  Radiant

### 2.6 Schedules

| IDF Class | YAML Location | Check |
|-----------|---------------|-------|
| `Schedule:Compact` / `Schedule:Day:Hourly` | `schedules:` | 24 hourly values for weekday. Add `weekend:` if different. Copy all values with full decimal precision from the IDF. Do not round. |
| `ScheduleTypeLimits` | (not modeled) | Note if schedule values are clamped or normalized. |

### 2.7 Infiltration

| IDF Class | YAML Location | Check |
|-----------|---------------|-------|
| `ZoneInfiltration:DesignFlowRate` | `infiltration:` | Zone, design_flow_rate (m³/s), constant/temperature/wind coefficients. |

**Every zone with infiltration in the IDF must have a matching entry.**

### 2.8 HVAC Systems

| IDF Class | YAML Location | Check |
|-----------|---------------|-------|
| `AirLoopHVAC` | `air_loops:` | System name, controls |
| `Fan:OnOff` / `Fan:ConstantVolume` / `Fan:VariableVolume` | `equipment: [type: fan]` | source (on_off/constant/vav), design_flow_rate, pressure_rise, motor_efficiency, total_efficiency or impeller_efficiency |
| `Coil:Heating:Fuel` | `equipment: [type: heating_coil]` | source (gas/electric), capacity, efficiency, setpoint |
| `Coil:Cooling:DX:SingleSpeed` | `equipment: [type: cooling_coil]` | source (dx), capacity, COP, SHR, rated airflow, setpoint, curve names |
| `Coil:Heating:Electric` | `equipment: [type: heating_coil]` | source: electric, capacity, efficiency: 1.0 |
| Performance curves | `performance_curves:` | Curve type (biquadratic/quadratic), all coefficients with full precision, min/max bounds |
| `Sizing:System` | `simulation.heating/cooling_sizing_factor` | Fraction of autosized capacity oversize |
| `SetpointManager:*` | `air_loops[].controls` | Cooling/heating supply air temps |
| `Controller:OutdoorAir` | `air_loops[].controls.minimum_damper_position` | Minimum OA fraction |

### 2.9 Thermostats

| IDF Class | YAML Location | Check |
|-----------|---------------|-------|
| `ZoneControl:Thermostat` | `thermostats:` | Zone assignment |
| `ThermostatSetpoint:DualSetpoint` | `thermostats[].heating/cooling_setpoint` | °C values with full precision |
| Setpoint schedules | Check if constant or scheduled | If scheduled, use occupied/unoccupied setpoints |

### 2.10 DHW

| IDF Class | YAML Location | Check |
|-----------|---------------|-------|
| `WaterHeater:Mixed` | `dhw_systems[].water_heater` | Fuel type, tank volume, capacity, efficiency, setpoint, UA standby, deadband |
| `WaterUse:Equipment` | `dhw_systems[].loads[]` | Peak flow rate (convert m³/s to L/s), schedule, use temperature |
| `WaterUse:Connections` | `dhw_systems:` | Mains temperature |

### 2.11 Ventilation and Exhaust

| IDF Class | YAML Location | Check |
|-----------|---------------|-------|
| `Fan:ZoneExhaust` | `exhaust_fans:` | Zone, flow rate (m³/s) |
| `DesignSpecification:OutdoorAir` | `air_loops[].controls.minimum_damper_position` or dedicated OA flow | OA flow rate per person or per area |

### 2.12 Features Not Yet in OpenBSE

If the IDF contains any of these, **stop and report to the user** before
proceeding. Do not silently skip them:

- `AirflowNetwork:*` (pressure-driven infiltration)
- `Daylighting:Controls`
- `ZoneHVAC:*` that is not FCU (e.g., PTAC, PTHP, VRF, radiant)
- `Coil:Cooling:DX:TwoSpeed` / `MultiSpeed` / `VariableSpeed`
- `HeatPump:*`
- `ZoneHVAC:Dehumidifier:*`
- `SurfaceProperty:ExposedFoundationPerimeter` / `Foundation:Kiva`
  (note the ground temps and use monthly table instead)
- `WindowShadingControl` / interior blinds
- Multiple air loops serving the same zone
- Plant loop equipment not yet wired (cooling towers, ground HX)

---

## Phase 3: Validate — Loads Before Systems

After the YAML is built, validate in this specific order. **Do not skip
ahead.** Fix each category before moving to the next.

### Step 1: Internal Gains (Lighting, Equipment, People)

These should match E+ within **0.5%** — they are direct schedule×power
calculations with no physics involved.

- Compare annual lighting energy (kWh)
- Compare annual electric equipment energy (kWh)
- Compare annual gas equipment energy (kWh)
- Compare annual DHW energy (kWh)

If any of these are off by more than 1%, there is a transcription error in
the YAML. Find it and fix it before proceeding.

### Step 2: Envelope Loads (Zone Heat Balance)

Compare the zone-level heat balance components:

- **Transmitted solar** through windows (kWh) — should match within 1%
- **Opaque surface conduction** (kWh) — should match within 5%
- **Infiltration heat loss** (kWh) — should match within 2%
- **Window conduction** (kWh) — should match within 5%

If transmitted solar is off, check window areas, SHGC, and angular
transmittance model. If opaque conduction is off, check construction layers,
material properties, and surface areas. If infiltration is off, check design
flow rates and coefficients.

### Step 3: Zone Temperatures (Unconditioned Zones)

For unconditioned zones (attic, basement, garage), compare hourly or monthly
mean zone temperatures against E+ output. They should track within a few
degrees. If a zone is too hot or too cold, check:

- Surface areas and constructions
- Infiltration rate
- Interzone surface coupling (both sides defined? correct constructions?)
- Ground temperature (for basement)

### Step 4: HVAC System Response

Only after Steps 1–3 pass, compare HVAC energy:

- **Heating energy** (gas or electric, kWh)
- **Cooling energy** (electric, kWh)
- **Fan energy** (electric, kWh)

If heating/cooling are off but envelope loads match, the issue is in the
system model:

- Check supply air temperatures (heating DAT, cooling DAT)
- Check fan efficiency (total efficiency = motor_eff × impeller_eff)
- Check DX coil performance curves (biquadratic coefficients, bounds)
- Check cycling method (on/off vs proportional) and PLF curve
- Check sizing: are autosized capacities and airflows reasonable?
- Check economizer settings
- Check minimum OA fraction

### Step 5: Hourly Comparison (if needed)

If annual totals are within 5% but you want to verify dynamic behavior:

- Export hourly zone temperatures and compare against E+ hourly output
- Export hourly heating/cooling coil power and compare
- Look for systematic biases (always high in winter? always low at night?)

---

## Phase 4: Document Results

Update `compare_end_uses.py` with the final OpenBSE values. The script
should print a table showing each end-use with E+ value, OpenBSE value, and
percent difference.

Update `docs/STATUS.md` and `docs/AI_CONTEXT.md` with current comparison
results.

---

## Common Pitfalls

1. **Stale E+ reference values.** Always re-run E+ on the simplified IDF
   and use those results. Do not use values from the original (unsimplified)
   IDF.

2. **Ground temperatures.** If the IDF uses Kiva/Basement/Slab preprocessors,
   the simplified IDF must include explicit `Site:GroundTemperature:BuildingSurface`
   with the monthly values those preprocessors computed. Otherwise E+
   defaults to 18°C constant, which may not match the original model.

3. **Fraction conversions.** E+ `Fraction Radiant`, `Fraction Latent`,
   `Fraction Lost` do not map 1:1 to OpenBSE fields. See Section 2.5.

4. **Missing zones.** Every zone in the IDF must exist in the YAML. Missing
   unconditioned zones (basements, garages, plenums) change the thermal
   boundary conditions for conditioned zones.

5. **Missing interzone surfaces.** When a surface has `Zone` boundary in the
   IDF, both sides must be defined in the YAML with reversed constructions.

6. **Schedule precision.** Copy all 24 hourly values with full decimal
   precision. Rounding 0.88310 to 0.88 changes annual energy by ~0.4%.

7. **Construction layer order.** The first layer in a construction is the
   outside face. Reversed constructions for interzone surfaces must have
   layers in the opposite order.

8. **OtherSideCoefficients.** If an IDF surface references
   `SurfaceProperty:OtherSideCoefficients`, check whether that object exists
   in the simplified IDF. If not, the surface may be using default ground
   temperatures. Match accordingly.

9. **DX curve coefficients.** Copy biquadratic/quadratic coefficients with
   full precision (6+ decimal places). Wrong coefficients are a common source
   of cooling energy errors.

10. **Sizing oversize factors.** Check `Sizing:System` for
    `Fraction of Autosized Heating/Cooling Capacity`. These multiply the
    autosized values. Missing them changes peak capacity and part-load
    behavior.

---

## File Organization

```
prototype_tests/
  PROTOTYPE_VALIDATION_GUIDE.md       ← This file
  compare_end_uses.py                 ← Comparison charts and tables
  Boulder.epw                         ← Shared weather file
  compare_zone_loads.py               ← Zone-level load comparison tools
  compare_zone_heatbal.py
  compare_surfaces_and_loads.py
  extract_openbse_zone_loads.py

  single_family/                      ← Each prototype gets its own subfolder
    SingleFamily_CZ5B_Boulder.yaml      OpenBSE model
    SingleFamily_CZ5B_Boulder.idf       Original E+ IDF
    SingleFamily_CZ5B_Boulder_simplified.idf  Simplified IDF (features removed)
    SingleFamily_CZ5B_comparison.png    Comparison chart
    simplify_sf_idf.py                  Script that simplifies the IDF
    eplus_run/                          E+ run outputs (from simplified IDF)
      in.idf                              IDF that was actually run
      eplustbl.csv                        Tabular results (end-use values)
      eplusout.err                        Error/warning log

  large_office/
    LargeOffice_Boulder.yaml
    LargeOffice_Denver.idf
    LargeOffice_Denver_simplified.idf
    LargeOffice_Boulder_comparison.png
    eplus_run/

  hospital/
    Hospital_STD2022_Boulder.yaml
    Hospital_STD2022_comparison.png

  apartment/
    ApartmentMidRise_Boulder.yaml
    ApartmentMidRise_Denver.idf
    ApartmentMidRise_Denver_heatbal.idf
    gen_apartment_yaml.py
    compare_bottom_floor.py
    eplus_run/
```

Each prototype follows the same pattern: original IDF, simplified IDF,
OpenBSE YAML, comparison chart, and an `eplus_run/` subfolder with E+
outputs from the simplified IDF.
