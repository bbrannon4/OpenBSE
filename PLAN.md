# Retail Building Example — Implementation Plan

## Building Description
A simple imaginary retail store, part of a larger building (strip mall or inline retail):
- **Sales Area** (~80 m², ~12m x 6.7m): Main retail space, south-facing storefront window, conditioned by RTU
- **Office** (~12 m², ~3m x 4m): Small back office, conditioned by same RTU
- **Restroom** (~6 m², ~2m x 3m): Unconditioned, with exhaust fan
- **Storage** (~15 m², ~3m x 5m): Unconditioned
- Ceiling height: 3.66m (12 ft)
- South wall: storefront window (~6m wide x 2.4m tall)
- North wall (back): normal exterior wall
- East/West walls: adiabatic (shared with adjacent tenant spaces)
- Roof: adiabatic (part of larger building)
- Floor: slab-on-grade (ground contact)

## HVAC System
Packaged single-zone rooftop unit (PSZ-AC / RTU):
- **DX cooling coil**: Single-speed, ~10.5 kW (3 ton), COP ~3.5
- **Gas furnace heating coil**: ~15 kW, 80% efficiency
- **Supply fan**: Constant volume, ~0.5 m³/s
- Serves both Sales Area AND Office (single thermostat in Sales Area)
- ASHRAE 62.1 ventilation: 7.5 cfm/person + 0.12 cfm/ft² for retail; 5 cfm/person + 0.06 cfm/ft² for office

## Restroom Exhaust
- Exhaust fan: ~25 cfm (~0.012 m³/s), runs during occupied hours

## Gap Analysis — New Features Needed

### 1. ✅ Internal Gain Schedules (CRITICAL)
Currently internal gains are constant 24/7. Real buildings have time-varying occupancy, lighting, and equipment.
**Add:** A `schedule` field on each internal gain that references a named schedule, plus first-class `Schedule` objects.

### 2. ✅ DX Cooling Coil Component
No cooling equipment exists. Need a single-speed DX cooling coil.
**Add:** `CoolingCoilDX` component in `openbse-components` with rated capacity, COP, and SHR.

### 3. ✅ Gas Heating Coil (Furnace)
Existing `HeatingCoil` only supports `Electric` and `HotWater`. Need gas/fuel type.
**Add:** `Gas` variant to `HeatingCoil` with burner efficiency.

### 4. ✅ Exhaust Fan
No exhaust fan model exists.
**Add:** `ExhaustFan` as a zone-level feature (flow rate + schedule) that removes air from the zone.

### 5. ✅ PSZ-AC / RTU System Type
The current HVAC coupling in main.rs is rudimentary. Need a proper single-zone packaged system that:
- Has a supply fan, heating coil, and cooling coil in sequence
- Controls supply air temp to meet thermostat in a control zone
- Distributes supply air proportionally to served zones
- Includes minimum outdoor air intake (ASHRAE 62.1)

### 6. ✅ ASHRAE 62.1 Outdoor Air
Currently ventilation is only modeled via infiltration ACH or scheduled ventilation.
**Add:** `outdoor_air` specification on zones with `per_person` and `per_area` flow rates.

## Implementation Steps

### Step 1: Named Schedules + Internal Gain Schedules
- Add `schedules` section to `ModelInput` with `Schedule` type (day-type aware, hourly fractions)
- Add optional `schedule` field to `InternalGainInput` variants (People, Lights, Equipment)
- Update `resolve_gains()` to accept hour/day-type and look up schedule fraction
- Wire through heat_balance.rs

### Step 2: Gas Heating Coil
- Add `Gas` variant to `HeatingCoil` (or just add `fuel_type` field and `burner_efficiency`)
- Add `gas` option to `HeatingCoilInput` coil_type in input.rs
- Report gas energy consumption separately from electric

### Step 3: DX Cooling Coil
- Create `crates/openbse-components/src/cooling_coil.rs`
- `CoolingCoilDX` struct with rated_capacity, rated_cop, rated_shr, rated_airflow
- `simulate_air()` implementation: compute available cooling at current conditions
- Simple performance: capacity scales with outdoor temp (derating at high ambient)
- Add `cooling_coil` variant to `EquipmentInput` enum in input.rs

### Step 4: Exhaust Fan (Zone-Level)
- Add `exhaust_fan` to `ZoneInput` with flow_rate and schedule name
- In heat_balance.rs, subtract exhaust mass flow from zone air balance
- Exhaust reduces zone pressure (increases infiltration slightly — simplified model)

### Step 5: Outdoor Air / ASHRAE 62.1
- Add `outdoor_air` specification to zone or air loop level
- Fields: `per_person` (m³/s-person), `per_area` (m³/s-m²), `method` (Sum or Max)
- Calculate minimum OA flow based on zone people count and floor area
- Apply as additional ventilation in the zone air balance

### Step 6: PSZ-AC System Integration in CLI
- Update the coupled envelope+HVAC path in main.rs to support the new components
- Fan → Heating Coil → Cooling Coil sequence on air loop
- Thermostat control: if zone temp > cooling setpoint → activate DX coil; if < heating setpoint → activate heating coil
- Mix return air with outdoor air at minimum ventilation rate
- Report fan electric, cooling electric, heating gas energy

### Step 7: Create Retail Example YAML
- Build the full 4-zone retail building model
- All vertex geometry for sales, office, restroom, storage
- Interior walls between zones (BoundaryCondition::Zone)
- Adiabatic east/west walls and roof
- Ground-contact floor
- Internal loads with schedules (occupied 8am-9pm weekdays, 9am-7pm weekends)
- RTU serving sales + office
- Exhaust fan in restroom
- ASHRAE 62.1 ventilation

### Step 8: Run and Verify
- Build release binary
- Run the retail example
- Verify reasonable results (heating/cooling energy, zone temperatures)
- Save results to examples/retail_store/

---

## Status: Complete

All eight implementation steps have been completed. The retail building example is fully implemented and running with the following results:

- **4-zone model** with vertex geometry: Sales Area (80 m2), Office (12 m2), Restroom (6 m2), Storage (15 m2)
- **PSZ-AC rooftop unit** with DX cooling coil (3 ton, COP 3.5), gas furnace (15 kW, 80% efficiency), constant-volume supply fan
- **ASHRAE 62.1 ventilation** with per-person and per-area outdoor air rates for retail and office zones
- **Restroom exhaust fan** operating during occupied hours
- **Internal gain schedules** for occupancy, lighting, and equipment (weekday 8am-9pm, weekend 9am-7pm)
- **Adiabatic** east/west walls and roof (shared with adjacent tenant spaces), slab-on-grade floor
- **Design day autosizing** correctly sizes the RTU coils and fan from zone peak and system coincident loads
- **Summary report** output with monthly energy breakdowns, peak loads, and unmet hours

This example also served as the proving ground for the multi-loop control framework (PSZ-AC type), the DX cooling coil component, the gas furnace heating coil, exhaust fan modeling, and the ASHRAE 62.1 outdoor air integration.
