# DOE Prototype Building Models: OpenBSE Gap Analysis

## Overview

The DOE/PNNL prototype building models consist of **16 commercial building types** across **19 climate locations**, representing ~80% of U.S. commercial building floor area. Each is fully modeled in EnergyPlus with detailed HVAC, envelope, DHW, lighting, and controls. This document maps every required capability against OpenBSE's current feature set to identify gaps.

**Legend:**
- `[HAVE]` = OpenBSE has this capability today
- `[PARTIAL]` = OpenBSE has partial support (see notes)
- `[NEED]` = Not yet implemented in OpenBSE

---

## Cross-Cutting Features (Required by All/Most Prototypes)

### Envelope

| Feature | Status | Notes |
|---------|--------|-------|
| Opaque wall constructions (layered) | `[HAVE]` | CTF conduction, all material properties |
| Simple constructions (U-factor only) | `[HAVE]` | |
| Window constructions (U + SHGC) | `[HAVE]` | SimpleGlazingSystem equivalent |
| Roof constructions (IEAD, attic, metal) | `[HAVE]` | Via layered or simple constructions |
| Ground-coupled floors (slab-on-grade) | `[HAVE]` | Kusuda ground temp model |
| Adiabatic and interzone boundaries | `[HAVE]` | |
| 3D vertex geometry | `[HAVE]` | Newell normal, auto area/azimuth/tilt |
| Overhangs and fins (shading) | `[HAVE]` | Sutherland-Hodgman polygon clipping |
| Exterior shading surfaces | `[HAVE]` | Explicit shading geometry |
| Solar on tilted surfaces (Perez model) | `[HAVE]` | Perez 1990 anisotropic sky |
| FullInteriorAndExterior solar distribution | `[NEED]` | Currently simplified; would fix remaining ASHRAE 140 gaps |
| Skylights | `[HAVE]` | Modeled as roof-mounted windows |
| Daylighting controls (continuous dimming) | `[NEED]` | Used in offices, schools, retail, hospital |
| Detailed window models (spectral glazing) | `[NEED]` | Only needed for advanced analysis, SimpleGlazing sufficient for prototypes |

### Internal Loads

| Feature | Status | Notes |
|---------|--------|-------|
| People (count, per-area, activity level) | `[HAVE]` | |
| Lights (W, W/m2, radiant/return-air fractions) | `[HAVE]` | |
| Electric equipment (W, W/m2) | `[HAVE]` | |
| Gas equipment (cooking loads) | `[NEED]` | Used in restaurants, hotels, schools, hospital |
| Elevator loads | `[HAVE]` | Modeled as zone electric equipment |
| Internal mass | `[NEED]` | EnergyPlus `InternalMass` object for furniture/contents thermal capacitance |

### Schedules

| Feature | Status | Notes |
|---------|--------|-------|
| Weekday/weekend hourly profiles | `[HAVE]` | |
| Saturday/Sunday/Holiday variants | `[HAVE]` | |
| Availability schedules (HVAC on/off) | `[HAVE]` | |
| Thermostat setpoint schedules | `[HAVE]` | Occupied/unoccupied setpoints |
| Fractional schedules for loads | `[HAVE]` | |

### Infiltration & Ventilation

| Feature | Status | Notes |
|---------|--------|-------|
| Design flow rate infiltration (DOE-2 coefficients) | `[HAVE]` | |
| ACH-based infiltration | `[HAVE]` | |
| Scheduled infiltration multiplier | `[HAVE]` | |
| HVAC-on infiltration reduction | `[PARTIAL]` | Need system-linked reduction (25% of off rate per E+) |
| Mechanical ventilation (per-person, per-area) | `[HAVE]` | |
| Minimum OA damper position | `[HAVE]` | Auto-calculated or user-specified |
| Demand controlled ventilation (CO2-based) | `[NEED]` | Used in schools (gym, cafeteria), hospitals, large assembly |
| Multi-zone ventilation optimization (62.1 Voz) | `[NEED]` | Required for VAV systems per ASHRAE 62.1 |
| Kitchen exhaust / makeup air | `[NEED]` | Restaurants, schools, hotels, hospital |
| Transfer air (zone-to-zone airflow) | `[NEED]` | Kitchen makeup from adjacent dining zones |
| Airflow network / duct leakage | `[NEED]` | Only needed for residential prototypes |

### Sizing

| Feature | Status | Notes |
|---------|--------|-------|
| Design day sizing | `[HAVE]` | Heating and cooling design days |
| Zone sizing (peak loads) | `[HAVE]` | |
| System sizing (coincident peak) | `[HAVE]` | |
| Autosize fans, coils, boilers, chillers | `[HAVE]` | |
| Autosize terminal boxes | `[HAVE]` | VAV and PFP |
| Plant loop sizing | `[PARTIAL]` | Basic; needs iteration for convergence |
| Sizing safety factors | `[HAVE]` | Global sizing parameters |

### Controls

| Feature | Status | Notes |
|---------|--------|-------|
| Dual setpoint thermostat | `[HAVE]` | Heating/cooling setpoints |
| Night setback (65F/80F) | `[HAVE]` | Occupied/unoccupied setpoints |
| Night cycle availability manager | `[HAVE]` | Cycling on during unoccupied if temps drift |
| Optimum start | `[NEED]` | Pre-conditions before occupancy (offices, schools) |
| Economizer (differential dry-bulb) | `[HAVE]` | |
| Economizer (fixed dry-bulb) | `[HAVE]` | |
| Economizer (differential enthalpy) | `[HAVE]` | |
| Supply air temperature reset (warmest zone) | `[NEED]` | `SetpointManager:Warmest` for VAV systems |
| Chilled water supply temp reset | `[NEED]` | `SetpointManager:OutdoorAirReset` on CHW loop |
| Hot water supply temp reset | `[NEED]` | `SetpointManager:OutdoorAirReset` on HW loop |
| Condenser water temp reset | `[NEED]` | For cooling tower optimization |
| Single-zone supply air temp reset | `[PARTIAL]` | PSZ has fixed supply temps; need `SetpointManager:SingleZone:Reheat` equivalent |
| VAV minimum flow setpoints | `[HAVE]` | `min_flow_fraction` on VAV/PFP boxes |
| Preheat coil control (OAT < 50F) | `[NEED]` | Cold climate OA preheat for hospitals, outpatient |

### DHW / Service Hot Water

| Feature | Status | Notes |
|---------|--------|-------|
| Gas storage water heater | `[HAVE]` | Mixed-tank energy balance |
| Electric storage water heater | `[HAVE]` | |
| Heat pump water heater | `[HAVE]` | Basic COP model |
| DHW draw schedules | `[HAVE]` | Peak flow + schedule fraction |
| Mains water temperature | `[HAVE]` | Fixed value; could enhance with `Site:WaterMainsTemperature` correlation |
| DHW recirculation loop | `[NEED]` | Large buildings have recirculation losses |
| Multiple DHW end uses per system | `[HAVE]` | Vec of loads |

### Outputs & Reporting

| Feature | Status | Notes |
|---------|--------|-------|
| Timestep/hourly/daily/monthly CSV | `[HAVE]` | |
| Annual summary report | `[HAVE]` | Energy, peak loads, unmet hours |
| Custom output variables | `[HAVE]` | |
| Component-level outputs | `[HAVE]` | |

---

## Per-Prototype Building Analysis

### 1. Small Office
**5,500 ft2, 1 story, 5 zones, wood-frame, slab-on-grade**

| System | ASHRAE 90.1 Type | Status |
|--------|------------------|--------|
| HVAC | PSZ-HP (System 4) -- heat pump + gas backup | `[HAVE]` |
| Heating | Air-source heat pump + gas furnace supplemental | `[HAVE]` HP coil + gas coil |
| Cooling | Single-speed DX | `[HAVE]` |
| Fan | Constant volume, draw-through | `[HAVE]` |
| DHW | Gas storage water heater | `[HAVE]` |
| Economizer | None | `[HAVE]` (NoEconomizer) |
| Controls | Night setback, scheduled on/off | `[HAVE]` |

**Gaps for this prototype:** None significant. **Could model today.**

---

### 2. Medium Office
**53,600 ft2, 3 stories, 15 zones + plenum, steel-frame**

| System | ASHRAE 90.1 Type | Status |
|--------|------------------|--------|
| HVAC | PVAV (System 5) -- packaged DX + VAV | `[HAVE]` |
| Heating | Gas furnace at AHU + electric reheat at terminals | `[HAVE]` |
| Cooling | Two-speed DX | `[NEED]` two-speed; single-speed available |
| Fan | Variable volume | `[HAVE]` |
| Terminals | VAV boxes with electric reheat | `[HAVE]` |
| DHW | Gas storage water heater | `[HAVE]` |
| Economizer | Differential dry-bulb or enthalpy | `[HAVE]` |
| Controls | Night setback, VAV min flow, SAT reset | `[PARTIAL]` SAT reset needed |
| Daylighting | Continuous dimming (perimeter zones) | `[NEED]` |
| Elevator | Equipment load | `[HAVE]` |

**Gaps:** Two-speed DX coil, supply air temp reset (SetpointManager:Warmest), daylighting controls. **Core HVAC modelable today** (single-speed DX approximation acceptable).

---

### 3. Large Office
**498,600 ft2, 12 stories + basement, steel/mass construction**

| System | ASHRAE 90.1 Type | Status |
|--------|------------------|--------|
| HVAC | VAV with hot water reheat (System 7) | `[HAVE]` |
| Heating | Gas boiler, HW reheat coils | `[HAVE]` |
| Cooling | Water-cooled centrifugal chiller | `[PARTIAL]` Air-cooled chiller exists; water-cooled needs condenser loop |
| Cooling tower | Variable-speed, open-circuit | `[NEED]` Component exists but not integrated |
| Fan | Variable volume | `[HAVE]` |
| Terminals | VAV boxes with HW reheat | `[HAVE]` |
| Plant loops | CHW, HW, condenser water | `[PARTIAL]` CHW/HW work; condenser loop not integrated |
| Pumps | Variable-speed on CHW/HW/CW | `[NEED]` Component exists but not integrated |
| DHW | Gas storage water heater | `[HAVE]` |
| Economizer | Differential dry-bulb/enthalpy | `[HAVE]` |
| Controls | SAT reset, CHW reset, CW reset, DCV, optimal start | `[NEED]` All advanced resets |
| Daylighting | Continuous dimming | `[NEED]` |
| Data center zone | Dedicated cooling | `[NEED]` Special zone type |
| Return plenum | Plenum-based return | `[NEED]` |
| Elevator | Equipment load | `[HAVE]` |

**Gaps:** Water-cooled chiller + condenser loop + cooling tower integration, pump integration, supply/plant temp resets, DCV, daylighting, return plenum. **Core VAV+boiler modelable today; full fidelity requires plant loop completion.**

---

### 4. Stand-Alone Retail
**24,695 ft2, 1 story, 5 zones, mass/steel construction**

| System | ASHRAE 90.1 Type | Status |
|--------|------------------|--------|
| HVAC | PSZ-AC (System 3) | `[HAVE]` |
| Heating | Gas furnace in RTU | `[HAVE]` |
| Cooling | Two-speed DX | `[NEED]` two-speed |
| Fan | Constant volume | `[HAVE]` |
| DHW | Gas storage water heater | `[HAVE]` |
| Economizer | Yes (by climate zone) | `[HAVE]` |
| Controls | Night setback | `[HAVE]` |

**Gaps:** Two-speed DX coil (single-speed approximation acceptable). **Could model today.**

---

### 5. Strip Mall
**22,500 ft2, 1 story, 10 zones, steel-frame**

| System | ASHRAE 90.1 Type | Status |
|--------|------------------|--------|
| HVAC | PSZ-AC (System 3) per tenant | `[HAVE]` |
| Heating | Gas furnace | `[HAVE]` |
| Cooling | Two-speed DX | `[NEED]` two-speed |
| Fan | Constant volume | `[HAVE]` |
| DHW | Gas storage water heater | `[HAVE]` |
| Economizer | Yes (by climate zone) | `[HAVE]` |

**Gaps:** Two-speed DX. **Could model today** with single-speed approximation.

---

### 6. Primary School
**73,960 ft2, 1 story, many zones, steel-frame**

| System | ASHRAE 90.1 Type | Status |
|--------|------------------|--------|
| HVAC (classrooms) | PVAV (System 5) -- DX + gas + VAV w/ electric reheat | `[HAVE]` |
| HVAC (gym/kitchen/cafe) | PSZ-AC (System 3) | `[HAVE]` |
| Heating | Gas furnace (PSZ) + boiler for HW reheat (VAV) | `[HAVE]` |
| Cooling | Two-speed DX (PVAV) + DX (PSZ) | `[NEED]` two-speed |
| Chiller + cooling tower | Water-cooled for VAV zones | `[PARTIAL]` |
| Fan | VAV + constant volume | `[HAVE]` |
| Terminals | VAV boxes with HW reheat | `[HAVE]` |
| DHW | Gas storage (large capacity for cafeteria) | `[HAVE]` |
| Economizer | Yes | `[HAVE]` |
| Kitchen exhaust | Dedicated exhaust + makeup air | `[NEED]` |
| Transfer air | Dining to kitchen | `[NEED]` |
| DCV | Gym, cafeteria | `[NEED]` |
| Daylighting | Classroom perimeters | `[NEED]` |

**Gaps:** Kitchen exhaust/makeup, transfer air, DCV, two-speed DX, daylighting. **HVAC skeleton modelable today; kitchen ventilation needs work.**

---

### 7. Secondary School
**210,900 ft2, 2 stories, many zones, steel-frame**

Same as Primary School plus:

| Additional Feature | Status |
|-------------------|--------|
| Auditorium (PSZ-AC) | `[HAVE]` |
| Two gymnasiums | `[HAVE]` |
| DCV on auditorium + gyms | `[NEED]` |
| Multiple VAV AHUs | `[HAVE]` (multiple air loops) |

**Gaps:** Same as Primary School. **HVAC skeleton modelable.**

---

### 8. Outpatient Healthcare
**40,950 ft2, 3 stories, steel-frame**

| System | ASHRAE 90.1 Type | Status |
|--------|------------------|--------|
| HVAC | PVAV (System 5/6) -- DX + electric reheat | `[HAVE]` |
| Heating | Electric reheat + HW baseboard | `[PARTIAL]` No baseboard component |
| Cooling | Packaged DX or air-cooled chiller | `[HAVE]` |
| Fan | VAV + constant volume | `[HAVE]` |
| Terminals | VAV boxes with electric reheat | `[HAVE]` |
| DHW | Gas storage water heater | `[HAVE]` |
| Economizer | Yes | `[HAVE]` |
| Preheat coil | HW coil on OA when OAT < 50F (CZ 6+) | `[NEED]` |
| Baseboard heaters | HW radiant/convective baseboard | `[NEED]` |
| Healthcare ventilation | ASHRAE 170 elevated OA rates | `[HAVE]` (via per-zone OA specs) |

**Gaps:** Baseboard heaters, OA preheat coil, healthcare-specific ventilation modes. **Core HVAC modelable.**

---

### 9. Hospital
**241,410 ft2, 5 stories + basement, mass construction**

| System | ASHRAE 90.1 Type | Status |
|--------|------------------|--------|
| HVAC | VAV with HW reheat (System 7) | `[HAVE]` |
| CAV for ORs | Constant volume, 100% OA critical areas | `[PARTIAL]` Can approximate with DOAS |
| Heating | Gas boiler, HW reheat | `[HAVE]` |
| Cooling | Water-cooled centrifugal chiller | `[PARTIAL]` Air-cooled only |
| Cooling tower | Variable-speed | `[NEED]` Not integrated |
| Plant loops | CHW, HW, condenser water | `[PARTIAL]` |
| Pumps | Variable-speed | `[NEED]` Not integrated |
| DHW | Gas boiler SHW (large) | `[HAVE]` |
| Economizer | Yes | `[HAVE]` |
| Kitchen exhaust | Yes | `[NEED]` |
| Laundry loads | Process loads | `[HAVE]` (as equipment) |
| Preheat coil | HW coil on OA for cold climates | `[NEED]` |
| Pressure relationships | OR positive/negative pressure zones | `[NEED]` |
| Controls | SAT reset, CHW/HW reset, DCV, optimal start | `[NEED]` |
| Elevator | Multiple | `[HAVE]` (as equipment) |

**Gaps:** Water-cooled chiller, condenser loop/tower integration, pump integration, kitchen exhaust, preheat coil, pressure relationships, advanced resets. **Core VAV modelable; full hospital fidelity requires significant plant + ventilation work.**

---

### 10. Small Hotel
**43,200 ft2, 4 stories, wood/steel-frame**

| System | ASHRAE 90.1 Type | Status |
|--------|------------------|--------|
| Guest rooms | PTAC (System 1) -- DX + electric heat | `[NEED]` |
| Common areas | Split-system DX + gas furnace (PSZ variant) | `[HAVE]` |
| DHW | Gas storage water heater | `[HAVE]` |
| Economizer | Common areas only | `[HAVE]` |
| Laundry loads | Process equipment | `[HAVE]` (as equipment) |
| Elevator | Equipment load | `[HAVE]` |

**Gaps:** PTAC zone-level equipment is the critical missing piece. **Common areas modelable today; guest rooms need PTAC.**

---

### 11. Large Hotel
**122,132 ft2, 6 stories + basement, mass construction**

| System | ASHRAE 90.1 Type | Status |
|--------|------------------|--------|
| Guest rooms | Four-pipe fan coil units + DOAS | `[PARTIAL]` FCU exists but not four-pipe; DOAS exists |
| Public areas | VAV with HW reheat (System 7) | `[HAVE]` |
| Kitchen | PSZ-AC | `[HAVE]` |
| Heating | Gas boiler, HW | `[HAVE]` |
| Cooling | Water-cooled or air-cooled chiller | `[PARTIAL]` Air-cooled only |
| Plant loops | CHW, HW, (condenser water if water-cooled) | `[PARTIAL]` |
| DHW | Gas water heater (high capacity) | `[HAVE]` |
| DOAS | Tempered ventilation to guest rooms | `[HAVE]` |
| Kitchen exhaust | Yes | `[NEED]` |
| Laundry exhaust | Yes | `[NEED]` |
| Controls | SAT reset, CHW reset, economizer on VAV | `[PARTIAL]` Economizer yes; resets no |

**Gaps:** Four-pipe FCU (HW+CHW coils in zone unit), water-cooled chiller, kitchen/laundry exhaust, plant temp resets. **Public area VAV + DOAS modelable today.**

---

### 12. Non-Refrigerated Warehouse
**52,045 ft2, 1 story, metal building**

| System | ASHRAE 90.1 Type | Status |
|--------|------------------|--------|
| Office | PSZ-AC (System 3) | `[HAVE]` |
| Fine storage | PSZ-AC (System 3) | `[HAVE]` |
| Bulk storage | Gas unit heater (System 9 -- heating only) | `[NEED]` |
| DHW | Gas storage (minimal) | `[HAVE]` |
| Economizer | PSZ zones only | `[HAVE]` |
| Skylights | Fine storage | `[HAVE]` |
| Metal building construction | Walls and roof | `[HAVE]` (via layered constructions) |

**Gaps:** Gas unit heater (heating-only zone equipment, no cooling). Simple component to add. **Office/storage zones modelable; bulk storage needs unit heater.**

---

### 13. Quick-Service Restaurant
**2,500 ft2, 1 story, 2 zones, wood-frame**

| System | ASHRAE 90.1 Type | Status |
|--------|------------------|--------|
| HVAC | PSZ-AC (System 3), one per zone | `[HAVE]` |
| Heating | Gas furnace | `[HAVE]` |
| Cooling | Two-speed DX | `[NEED]` two-speed |
| DHW | Gas storage (high usage) | `[HAVE]` |
| Kitchen exhaust | 100% OA kitchen, large exhaust | `[NEED]` |
| Gas cooking equipment | Process loads | `[NEED]` Gas equipment type |
| Economizer | Yes (by climate zone) | `[HAVE]` |

**Gaps:** Kitchen exhaust/makeup air system, gas cooking equipment loads, two-speed DX. **HVAC modelable; kitchen ventilation is the key gap.**

---

### 14. Full-Service Restaurant
**5,502 ft2, 1 story, 2 zones, wood-frame**

Same gaps as Quick-Service Restaurant. **HVAC modelable; kitchen ventilation needed.**

---

### 15. Mid-Rise Apartment
**33,740 ft2, 4 stories, wood/steel-frame**

| System | ASHRAE 90.1 Type | Status |
|--------|------------------|--------|
| HVAC | Split-system DX + gas furnace per unit | `[HAVE]` (PSZ-AC equivalent) |
| Corridors | Electric unit heaters | `[NEED]` Unit heater component |
| DHW | Gas storage water heater | `[HAVE]` |
| Economizer | None | `[HAVE]` (NoEconomizer) |
| Controls | 24/7 operation, per-unit thermostat | `[HAVE]` |
| Elevator | Equipment load | `[HAVE]` |

**Gaps:** Electric unit heater for corridors (simple). **Apartments modelable today** (corridor heaters can be approximated as ideal loads or electric baseboard).

---

### 16. High-Rise Apartment
**84,360 ft2, 10 stories, mass/steel-frame**

| System | ASHRAE 90.1 Type | Status |
|--------|------------------|--------|
| HVAC | Water-source heat pumps (WSHP) per unit | `[NEED]` |
| Plant | Central condenser water loop | `[NEED]` |
| Heat rejection | Fluid cooler on condenser loop | `[NEED]` |
| Supplemental heat | Gas boiler on condenser loop | `[PARTIAL]` Boiler exists; loop coupling needed |
| DHW | Gas storage water heater | `[HAVE]` |
| Economizer | None | `[HAVE]` |
| Elevator | Equipment load | `[HAVE]` |

**Gaps:** Water-source heat pump component, condenser water loop integration, fluid cooler. **This is the most unique prototype -- requires a new HVAC paradigm (water-loop HP).**

---

## Summary: Required New Components & Features

### Tier 1 -- Enables Multiple Prototypes (High Impact)

| Feature | Unlocks Prototypes | Effort |
|---------|-------------------|--------|
| **PTAC / PTHP zone equipment** | Small Hotel, Large Hotel (guest rooms), Apartments (alt config) | Medium |
| **Water-cooled chiller (EIR model)** | Large Office, Hospital, Large Hotel, Secondary School | Medium |
| **Condenser water loop + cooling tower integration** | Large Office, Hospital, Large Hotel, Schools | Medium |
| **Pump integration into plant loops** | All chiller/boiler plants (6+ prototypes) | Medium |
| **Kitchen exhaust + makeup air system** | Restaurants (2), Schools (2), Hotels (2), Hospital | Medium |
| **Two-speed / multi-speed DX coils** | Medium Office, Retail, Strip Mall, Schools, Restaurants | Medium |
| **Supply air temp reset (warmest zone)** | All VAV systems (5+ prototypes) | Small |
| **Plant loop temp resets (OAR)** | All central plant systems (5+ prototypes) | Small |
| **Gas equipment internal loads** | Restaurants (2), Hotels (2), Schools (2), Hospital | Small |
| **Demand controlled ventilation** | Schools (2), Hospital, Large Office | Medium |
| **Daylighting controls** | Offices (2), Schools (2), Retail, Hospital | Large |

### Tier 2 -- Enables Specific Prototypes

| Feature | Unlocks Prototypes | Effort |
|---------|-------------------|--------|
| **Gas unit heater (heating-only zone equip)** | Warehouse (bulk storage) | Small |
| **Electric baseboard heater** | Outpatient HC, Apartments (corridors), Hotels (corridors) | Small |
| **Water-source heat pump** | High-Rise Apartment (only user) | Large |
| **Fluid cooler** | High-Rise Apartment | Medium |
| **Four-pipe fan coil unit** | Large Hotel (guest rooms) | Medium |
| **OA preheat coil** | Hospital, Outpatient HC (cold climates) | Small |
| **Return air plenum** | Large Office, Hospital | Medium |
| **Internal mass** | All prototypes (thermal capacitance of contents) | Small |
| **Optimum start controller** | Offices, Schools | Small |
| **Transfer air (zone-to-zone)** | Restaurants, Schools (kitchen makeup) | Medium |

### Tier 3 -- Advanced / Lower Priority

| Feature | Unlocks Prototypes | Effort |
|---------|-------------------|--------|
| **Dehumidification (latent DX)** | All DX systems (accuracy improvement) | Large |
| **Pressure relationships (OR zones)** | Hospital | Large |
| **Zone air distribution effectiveness** | All prototypes (accuracy improvement) | Small |
| **DHW recirculation loop** | Large buildings | Medium |
| **Airflow network / duct leakage** | Residential prototypes only | Large |
| **Multi-zone 62.1 Voz procedure** | All VAV systems | Medium |

---

## Prototype Readiness Summary

| # | Prototype | Ready Today? | Critical Gaps |
|---|-----------|-------------|---------------|
| 1 | **Small Office** | **YES** | None |
| 2 | **Medium Office** | **MOSTLY** | Two-speed DX (minor), SAT reset, daylighting |
| 3 | **Large Office** | **PARTIAL** | Water-cooled chiller, condenser loop, pumps, resets, daylighting |
| 4 | **Stand-Alone Retail** | **YES** | Two-speed DX (minor) |
| 5 | **Strip Mall** | **YES** | Two-speed DX (minor) |
| 6 | **Primary School** | **MOSTLY** | Kitchen exhaust, DCV, daylighting |
| 7 | **Secondary School** | **MOSTLY** | Kitchen exhaust, DCV, daylighting |
| 8 | **Outpatient Healthcare** | **MOSTLY** | Baseboard heaters, preheat coil |
| 9 | **Hospital** | **PARTIAL** | Water-cooled chiller, condenser loop, pumps, kitchen exhaust, preheat |
| 10 | **Small Hotel** | **PARTIAL** | PTAC zone equipment |
| 11 | **Large Hotel** | **PARTIAL** | Four-pipe FCU, water-cooled chiller, kitchen/laundry exhaust |
| 12 | **Warehouse** | **MOSTLY** | Gas unit heater (simple to add) |
| 13 | **Quick-Service Restaurant** | **MOSTLY** | Kitchen exhaust/makeup air |
| 14 | **Full-Service Restaurant** | **MOSTLY** | Kitchen exhaust/makeup air |
| 15 | **Mid-Rise Apartment** | **YES** | Corridor unit heater (minor) |
| 16 | **High-Rise Apartment** | **NO** | Water-source heat pump, condenser loop, fluid cooler |

### Score: 4-5 prototypes modelable today, 7-8 mostly modelable, 3-4 need significant work

---

## Recommended Implementation Roadmap

### Phase 1: Quick Wins (enables 3-4 more prototypes)
1. Gas unit heater component (Warehouse)
2. Gas equipment internal loads (Restaurants, Hotels, Schools)
3. Electric baseboard heater (Outpatient, Apartments, Hotels)
4. Internal mass (all prototypes -- thermal accuracy)
5. Supply air temp reset -- SetpointManager:Warmest (all VAV)
6. Plant loop temp resets -- outdoor air reset (all plants)

### Phase 2: Plant Loop Completion (enables Large Office, Hospital, Large Hotel)
1. Water-cooled chiller (Chiller:Electric:EIR with condenser water inlet/outlet)
2. Integrate cooling tower into condenser water loop
3. Integrate pumps into CHW/HW/CW loops
4. Plant loop iteration for convergence (supply-demand matching)
5. Condenser water loop as a full loop type

### Phase 3: Zone Equipment (enables Hotels, Apartments)
1. PTAC component (DX cooling + electric/HW heating, through-wall)
2. PTHP component (heat pump variant of PTAC)
3. Four-pipe fan coil unit (CHW cooling coil + HW heating coil + fan)
4. OA preheat coil on air loop outdoor air path

### Phase 4: Ventilation & Exhaust (enables Restaurants, Schools, Hospital)
1. Kitchen exhaust fan with makeup air requirements
2. Transfer air (zone-to-zone airflow connections)
3. Demand controlled ventilation (CO2-based or occupancy-based)
4. Multi-zone 62.1 Voz ventilation optimization

### Phase 5: DX Coil Enhancements (accuracy for all DX-based systems)
1. Two-speed DX coils (staged capacity)
2. Multi-speed DX coils
3. Variable-speed DX coils (inverter-driven)
4. Latent/dehumidification modeling in DX coils

### Phase 6: Advanced Controls & Features
1. Daylighting controls (continuous dimming, stepped)
2. Optimum start controller
3. Return air plenum modeling
4. Water-source heat pump + condenser water loop (High-Rise Apartment)
5. Fluid cooler component

---

## EnergyPlus Object Coverage Matrix

Total unique EnergyPlus object types used across prototypes: ~130

| Category | E+ Objects Used | OpenBSE Has | Coverage |
|----------|----------------|-------------|----------|
| Air Loops | 12 | 8 | 67% |
| Air Terminals | 6 | 4 | 67% |
| Zone HVAC Equipment | 8 | 1 | 13% |
| Cooling Coils | 5 | 3 | 60% |
| Heating Coils | 6 | 5 | 83% |
| Fans | 5 | 3 | 60% |
| Plant Loops & Operations | 6 | 3 | 50% |
| Chillers | 2 | 1 | 50% |
| Boilers | 1 | 1 | 100% |
| Cooling Towers | 3 | 1 (not integrated) | 33% |
| Pumps | 4 | 2 (not integrated) | 50% |
| Water Heaters / DHW | 7 | 3 | 43% |
| Outdoor Air & Ventilation | 5 | 3 | 60% |
| Heat Recovery | 3 | 2 | 67% |
| Setpoint Managers | 8 | 2 | 25% |
| Availability Managers | 4 | 2 | 50% |
| Daylighting | 3 | 0 | 0% |
| Infiltration | 1 | 1 | 100% |
| Sizing | 5 | 4 | 80% |
| Controllers | 2 | 2 | 100% |
| Zone Controls | 3 | 1 | 33% |
| Performance Curves | 5 | 4 | 80% |
| Schedules & Loads | 7 | 5 | 71% |
| Envelope & Construction | 8 | 6 | 75% |
| **Overall** | **~130** | **~68** | **~52%** |

---

## Sources

- [DOE Prototype Building Models](https://www.energycodes.gov/prototype-building-models)
- [DOE Commercial Reference Buildings](https://www.energy.gov/eere/buildings/commercial-reference-buildings)
- PNNL-20405: Achieving 30% Goal (90.1-2010 Prototype HVAC System List)
- PNNL-23269: Enhancements to ASHRAE 90.1 Prototype Models
- PNNL-32815: Prototypes Based on 90.1-2019 Appendix G
- NIST TN 2072: Airflow and IAQ Models of DOE Prototype Buildings
- NREL/TP-5500-46861: U.S. DOE Commercial Reference Building Models
- DOE Prototype Building Scorecards (energycodes.gov)
- ASHRAE 90.1 Appendix G, Tables G3.1.1-3 and G3.1.1-4
