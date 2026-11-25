# Retail Standalone: OpenBSE inputs vs the E+ reference IDF

Reference: `eplus_run/in.idf`, the EnergyPlus 25.2 example file
`ASHRAE901_RetailStandalone_STD2019_Denver.idf` with its three RunPeriods
(January, April, July) replaced by one annual RunPeriod. Weather:
Denver-Aurora-Buckley TMY3 (`weather/USA_CO_Denver-Aurora-Buckley.AFB.724695_TMY3.epw`), the file the
E+ run used. Model: `examples/doe_retail_standalone.yaml`, written by
`retail_from_idf.py` from the IDF (rerun it after editing either):

    python3 retail_from_idf.py eplus_run/in.idf ../../examples/doe_retail_standalone.yaml

"Matched" means the YAML carries the IDF's value; "transcribed" means an IDF
mechanism OpenBSE lacks is written as an equivalent OpenBSE input; "gap" means
OpenBSE cannot express it (with its measured effect on the E+ results, from E+
runs of the IDF with the object removed).

## Site, simulation, sizing

| IDF object | Status |
|---|---|
| `Timestep` 4 | matched |
| `Building` (north 0, City terrain, FullInteriorAndExterior) | matched (`terrain: city`, `solar_distribution: full_interior_and_exterior`) |
| `RunPeriod` (Jan 1 Sunday, annual), `RunPeriodControl:SpecialDays` (10 holidays) | matched: the weather year starts on a Sunday; the holidays are the IDF's dates for that year |
| `RunPeriodControl:DaylightSavingTime` | **gap**: OpenBSE has no daylight saving ([#199](https://github.com/bbrannon4/OpenBSE/issues/199)); E+ without it: lighting +0.9 %, heating −0.8 %, exterior lighting +2.6 % |
| `SizingPeriod:DesignDay` × 2 | matched (dry-bulb, range, wet-bulb, pressure, wind, ASHRAE tau 0.423 / 2.020; the heating day has clearness 0) |
| `Sizing:Parameters` 1.2 / 1.2, 6 timesteps | matched (`sizing_averaging_window: 6`) |
| `Sizing:Zone` (12.8 / 40 °C supply, 0.008 / 0.008) | matched (loop supply temperatures) |
| `Sizing:System` (coincident, 12.8 / 40 °C) | matched for single-zone loops |
| `Site:GroundTemperature:FCfactorMethod` | matched on each F-factor construction |
| `Site:WaterMainsTemperature` (correlation 9.95 / 23.7) | matched |
| `ShadowCalculation`, `SurfaceConvectionAlgorithm:Inside` (TARP), `HeatBalanceAlgorithm` (CTF) | OpenBSE defaults (TARP inside, CTF); the building is a convex box with no shading surfaces |
| `SurfaceConvectionAlgorithm:Outside` (TARP) | `outside_convection_algorithm: tarp` (OpenBSE's default is DOE-2, as E+'s) |

## Envelope

| IDF object | Status |
|---|---|
| `Zone` × 5 (origins, volumes) | matched; vertices carry the zone origins; floor areas from the polygons |
| `BuildingSurface:Detailed` × 36 | matched (names, vertices, boundaries, interzone partners via `adjacent_surface`) |
| `FenestrationSurface:Detailed` windows × 3, skylights × 16 | matched |
| Doors × 13 (swinging and roll-up) | matched as wall subsurfaces of their parent walls with the door construction: an opaque `Door` is an ordinary opaque surface in E+'s heat balance, and its area comes off the parent |
| `WindowProperty:FrameAndDivider` `Skylight_Frame` | no effect: zero frame width, no divider, no reveals |
| `Material` × 18, `Material:NoMass` × 14, `Construction` (those used) | matched |
| `WindowMaterial:SimpleGlazingSystem` × 2 | matched (U, SHGC, VT) |
| `Construction:FfactorGroundFloor` × 5 | matched (F 0.90012, the IDF's exposed perimeters) |
| `InternalMass` × 5 | matched |

## Loads and schedules

| IDF object | Status |
|---|---|
| `Schedule:Compact` | matched: weekday, Saturday, Sunday and holiday profiles where the IDF differs from OpenBSE's fallback |
| `People` × 5 (6.193 m²/person, ACTIVITY_SCH 120 W, radiant 0.3, sensible autocalculated) | matched; Front_Entry has 0 people |
| `Lights` × 5 (W/m², radiant 0.7, visible 0.2) | matched; the visible fraction is counted as radiant (0.9), see gaps |
| EMS `SET_<zone>_Light_EMS_Program` × 5 (lights capped at 0.02 W/ft² while the zone has no occupants) | transcribed: people and lighting schedules are fixed, so the program's result is a fixed schedule; the derived schedules carry the cap. E+ reads the previous timestep's occupancy, so it applies the cap one 15-minute step late at each change; the hourly schedules cannot hold that |
| `ElectricEquipment` × 4 | matched |
| `ZoneInfiltration:DesignFlowRate` × 6 | matched: Flow/ExteriorArea written as the design flow E+ computes (rate × the zone's ExteriorTotalSurfArea, gross outdoor walls and roofs plus each opaque door's area again, as `SurfaceGeometry.cc` counts it); values agree with `eplusout.eio` |
| `DesignSpecification:OutdoorAir` (Flow/Area) | matched for the four packaged units; Front_Entry's (used only by E+ zone sizing) is left out, as its unit heater has no outdoor air |
| `Daylighting:Controls` (SplitFlux, ContinuousOff, 0.2 / 0.2) and `Daylighting:ReferencePoint` × 2 in Core_Retail | matched (`daylighting_controls`); the IDF's zone-relative reference points are written in absolute coordinates (zone origin added). The glare inputs only report glare and are left out |
| `Exterior:Lights` × 2 (AstronomicalClock) | matched |
| `WaterHeater:Mixed` (stand-alone, 40 gal, 11.7 kW, 0.82, 1,860 W on- and off-cycle parasitic gas, losses to Back_Space) | matched; `use_temperature` at the tank limit draws the whole use flow from the tank, as a stand-alone E+ tank does |

## HVAC

| IDF object | Status |
|---|---|
| `AirLoopHVAC:UnitarySystem` × 4 (Load control, blow-through) | matched as packaged single-zone loops, multi-speed operation |
| `Fan:SystemModel` (discrete speeds 0.66 / 0.4 and 1 / 1; PSZ-AC:4 one speed) | matched (`speeds`) |
| `UnitarySystemPerformance:Multispeed` (no-load 0.66, cooling 0.66 / 1, heating 1) | matched (`no_load_flow_fraction`, speed `flow_fraction`s) |
| `Coil:Cooling:DX:MultiSpeed` (two speeds, hard-sized, curves) | matched (`dx_multispeed`) |
| `Coil:Heating:Fuel` × 5 | matched |
| Fan operating-mode schedule `HVACOperationSchd` | matched: continuous while available; the night cycle runs the fan cycling |
| `AvailabilityManager:NightCycle` × 4 (CycleOnAny, 1 K, 1,800 s) | matched (`night_cycle`) |
| `Controller:OutdoorAir` (FixedMinimum, `MinOA_MotorizedDamper_Sched`, DifferentialDryBulb, LockoutWithHeating) | matched; the mixed-air setpoint from `SetpointManager:SingleZone:Cooling` (10–50 °C) through `SetpointManager:MixedAir` is the economizer's `load_reset` |
| Maximum outdoor-air fraction schedule 0.75 | **gap**: OpenBSE's economizer can open to 100 % |
| `ZoneControl:Thermostat` / `ThermostatSetpoint:DualSetpoint` × 5 | matched (`heating_setpoint_schedule`, `cooling_setpoint_schedule`; design-day profiles from the schedules' design-day day types) |
| EMS `OA_BASED_OPTIMUM_START_EMS_Program` (Core_Retail keeps its setback one more hour on mild mornings) | **gap**: no EMS; E+ without it: heating +0.2 %, fans −0.7 % |
| `ZoneHVAC:UnitHeater` (Front_Entry; fan on while there is a heating load) | matched (`continuous_no_load_off`) |
| EMS `FrontEntry_HeaterControl` (unit heater off above 45 °F outdoors) | transcribed as `high_temperature_turn_off` (the E+ AvailabilityManager:HighTemperatureTurnOff that does the same) |

## Gaps

1. Daylight saving time (above).
2. Lights visible fraction (0.2) counted as long-wave radiant; E+ distributes it as short-wave, some of it lost through windows.
3. Optimum-start EMS program (above).
4. Economizer maximum outdoor-air fraction 0.75 (above).
