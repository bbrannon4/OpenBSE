# Hospital (DOE 90.1-2022, Denver) — input audit

`Hospital_STD2022_Denver.yaml` is generated from PNNL's prototype IDF
`eplus_run/ASHRAE901_Hospital_STD2022_Denver.idf` (E+ 22.1) by
`hospital_from_idf.py`. This file lists every IDF object class, how the model
represents it, and what OpenBSE cannot express. The E+ reference results are
`eplus_run/eplustbl.csv` and `eplus_run/eplusout.eio`, reproduced exactly with
E+ 22.1 on `weather/USA_CO_Denver-Aurora-Buckley.AFB.724695_TMY3.epw`, the
weather file the model uses.

Legend: **exact** = same input and same method; **equivalent** = expressed
differently with the same result; **approx.** = the closest OpenBSE input,
with the expected effect; **missing** = not represented.

## Site, simulation and sizing

| IDF object | Model | Status |
|---|---|---|
| Building (north axis −90°, City terrain, FullInteriorAndExterior) | vertices rotated by −90° as E+ does (`x' = −y`, `y' = x`); `terrain: city`; `solar_distribution: full_interior_and_exterior` | exact |
| Timestep 6 | `timesteps_per_hour: 6` | exact |
| Site:Location, SizingPeriod:DesignDay ×2 | `design_days` with the IDF's values (ASHRAE tau on the cooling day) | exact |
| Sizing:Parameters 1.0 / 1.0, averaging window 6 | sizing factors 1, `sizing_averaging_window: 6` | exact |
| RunPeriod (Sunday start, 2017 calendar), RunPeriodControl:SpecialDays | `holidays` on the same dates | exact |
| RunPeriodControl:DaylightSavingTime (2nd Sunday in March to 1st Sunday in November) | `daylight_saving_time` | exact |
| OutputControl:ReportingTolerances 0.556 K | `unmet_*_tolerance: 0.556` | exact |
| ShadowCalculation PolygonClipping, periodic 30 days | OpenBSE's detailed shading | equivalent |
| SurfaceConvectionAlgorithm:Inside/Outside TARP | OpenBSE defaults (TARP) | exact |
| HeatBalanceAlgorithm ConductionTransferFunction | CTF | exact |
| Site:GroundTemperature:FCfactorMethod | `ground_surface_temperatures` (the only ground surfaces are the C-factor walls, which use these) | exact |
| Site:WaterMainsTemperature (Correlation 9.95 / 23.7) | DHW `mains_temperature.correlation` | exact |
| Schedule day types SummerDesignDay / WinterDesignDay | not written: OpenBSE design days use `internal_gains` modes (full for cooling, off for heating) instead of day-type profiles | approx.: most of the IDF's design-day profiles are 1.0 (summer) and 0.0 (winter), as those modes; occupancy differs |

## Envelope

| IDF object | Model | Status |
|---|---|---|
| Material, Material:NoMass, Construction | `materials`, `constructions` | exact |
| WindowMaterial:SimpleGlazingSystem | `window_constructions` (U, SHGC, VT) | exact |
| Construction:CfactorUndergroundWall ×4 | explicit two-layer construction as E+ builds it: no-mass insulation `R = 1/C + R_soil(H) − 0.15/1.95` outside 0.15 m of E+'s FC concrete | exact |
| BuildingSurface:Detailed (402), FenestrationSurface:Detailed (40 windows, 17 doors) | `surfaces` with vertices; doors as wall subsurfaces | exact |
| Self-referencing interzone surfaces (every interior surface of a multiplied zone) | surfaces facing their own zone (E+ semantics) | exact |
| InternalMass ×55 | `internal_mass` | exact |
| Zone multipliers (17 zones, ×4 to ×10) | one zone N times as large: every surface keeps its orientation with N× area (area/azimuth/tilt, no vertices), N× volume, floor area, internal mass and absolute loads | equivalent for the heat balance (all of these zones' surfaces are outdoors or face their own zone, so nothing couples them to other zones); geometric self-shading and interior beam projection are not computed for these zones (no vertices), which then use OpenBSE's fixed solar distribution. #286 (mixed multipliers on one loop) would remove the need |
| Daylighting:Controls ×6 (SplitFlux, ContinuousOff) | `daylighting_controls` for the 4 controls in unmultiplied zones | approx.: Office2_Mult5_Flr_5 and Office4_Mult6_Flr_5 have no vertices here, so their daylighting is left out; their lights run at the full schedule |

## Internal gains

| IDF object | Model | Status |
|---|---|---|
| People ×55 (number of people, ACTIVITY_SCH 120 W, radiant 0.3, autocalculated sensible fraction) | `people` with `count` (×N), 120 W | exact |
| Lights ×55 (Watts/Area, radiant 0.7 + visible 0.2) | `lights` | exact |
| ElectricEquipment ×50, GasEquipment ×1 (EquipmentLevel) | `equipment` (×N), fuel electricity / natural gas | exact |
| ZoneInfiltration:DesignFlowRate (Flow/ExteriorWallArea, velocity term 0.224) | `infiltration` with the design flow E+ computes (rate × gross outdoor wall area, ×N); agrees with `eplusout.eio` | exact |
| WaterUse:Equipment zone gains (sensible fraction 0.2, latent 0.05) | `equipment` with fuel `none`: sensible `0.2·ṁ·c_p·(60 − 22.5 °C)`, latent `0.05·ṁ·h_fg`, on the water-use schedule | approx.: E+ uses the zone's actual temperature; the zones sit between the 21 °C and 24 °C setpoints, ±4 % on these gains |
| Exterior:Lights (AstronomicalClock), Exterior:FuelEquipment ×2 | `exterior_equipment` | exact |
| ElectricLoadCenter:Transformer (500 kVA, nominal efficiency 0.991, serving an EMS meter of 2.2 % of interior lighting) | — | missing: OpenBSE's transformer model is a constant efficiency on IT load. E+ reports 19.6 MWh of transformer losses under Exterior Equipment |
| Generator:PVWatts, ElectricLoadCenter:Inverter:PVWatts / Distribution | — | missing: no PV in OpenBSE. PV production is not part of the end uses compared |
| EMS Transformer_Load_Prog | — | belongs to the transformer above |

## Air side

| IDF object | Model | Status |
|---|---|---|
| AirLoopHVAC ×8 (7 VAV, 1 CAV) with Sizing:System | `air_loops`, `zone_sum` from Sizing:System | exact |
| Fan:VariableVolume ×7, Fan:ConstantVolume | `fan` with the IDF's pressure rise, efficiencies and curve coefficients | exact |
| Coil:Cooling:Water ×8 | `cooling_coil` chilled water, autosized | exact |
| Coil:Heating:Water (central) ×8 | left out | equivalent: Sizing:System central heating supply temperature −15 °C (VAV) / 12.8 °C (kitchen) sizes them to 0 W (VAV_1: 3.1 kW; E+ total 0.2 MWh/yr) |
| Sizing:System Central Heating Design Supply Air Temperature (−15 °C) | `central_heating_supply_temp`: terminal reheat is sized for −15 °C primary air, as E+ does with a central heating coil | exact |
| SetpointManager:Warmest (VAV_1, VAV_2: 12.8–15.6 °C) | OpenBSE's VAV Warmest reset (12.8–15.6 °C) | exact |
| SetpointManager:Scheduled VAV_SAT_SCH 12.8 °C (ER, OR, ICU, PATRMS, LABS) | `fixed_cooling_sat: true` | exact |
| SetpointManager:SingleZone:Cooling / Heating (kitchen) | PSZ `cycling: modulating` | equivalent |
| SetpointManager:MixedAir | OpenBSE's mixed-air target (supply setpoint less fan rise) | exact |
| Controller:OutdoorAir: VAV_1/2 minimum fraction BLDG_OA_FRAC_SCH (0.25 occupied, else 0), DifferentialDryBulb, LockoutWithHeating | `minimum_damper_position: 0.25` × the schedule normalized, economizer differential dry bulb | exact |
| Controller:MechanicalVentilation (VAV_1, VAV_2: VRP, DCV) | `dcv: true` with the zones' `outdoor_air`; VRP above the minimum | exact |
| Controller:OutdoorAir ER/OR/ICU/PATRMS (minimum fraction 0.33 and the design OA), LABS (1.0) | `minimum_damper_position` from the schedule; the zones' OA raises it through VRP | approx.: E+ takes max(Σ zone OA, fraction × supply); OpenBSE's VRP uses Σ zone OA ÷ Ev, slightly higher when Ev < 1 |
| Kitchen Controller:OutdoorAir (design OA × Kitchen_Exhaust_SCH, economizer) | the zone's OA × `minimum_oa_schedule`, economizer | exact |
| EMS vav_1/2_MAX_OA_FRAC | — | missing: caps the economizer at the design OA (4.33 / 4.48 m³/s) when OA is at the VRP minimum; inactive at those hours |
| HeatExchanger:AirToAir:SensibleAndLatent ×3 (rotary, 0.70/0.75 sensible, 0.60 latent, ExhaustOnly frost control at −23.3 °C, supply outlet temperature control) | `heat_recovery` wheel, effectivenesses, parasitic power | approx.: frost control and the 75 %-flow effectivenesses are not modelled; OpenBSE throttles recovery to the mixed-air target, like the outlet temperature control |
| SetpointManager:OutdoorAirPretreat | OpenBSE's wheel throttling | equivalent |
| Humidifier:Steam:Electric ×5 (100 kW, 3.72e-5 m³/s) | `humidifier` `rated_power` and `rated_capacity` | exact |
| ZoneControl:Humidistat ×5, SetpointManager:MultiZone:Humidity:Minimum (default limits 0.005 / 0.012) | zone `min_relative_humidity`; the humidifier's supply setpoint follows E+'s setpoint manager with its default limits | exact (E+ adds the zones' moisture load to their current humidity ratio, OpenBSE to the setpoint's) |
| SetpointManager:MultiZone:Humidity:Maximum (cooling-coil dehumidification) | zone `max_relative_humidity` | approx.: OpenBSE dehumidifies a zone above it in deadband; in Denver the 60 % limit is not reached |
| AvailabilityManager:NightCycle (HVACOperationSchd always on) | `night_cycle` | exact (inactive) |
| AirTerminal:SingleDuct:VAV:Reheat ×54 (constant minimum fractions as the IDF, ReverseWithLimits, 0.5 maximum reheat fraction, 40 °C) | `vav_box` with the IDF's `min_flow_fraction`, `max_reheat_fraction`, `max_reheat_temp` | exact |
| OR boxes: Scheduled minimum OR_MinSA_Sched | `min_flow_fraction_schedule`; sizing on the schedule's mean (0.6), as E+ | exact |
| AirTerminal:SingleDuct:ConstantVolume:NoReheat (kitchen) | direct zone connection | exact |
| Sizing:Zone DesignDayWithLimit (minimum cooling flow per area for ER, OR, ICU, patient rooms; absolute for the kitchen) | zone `minimum_cooling_flow_per_area` / `minimum_cooling_flow` | exact |
| DesignSpecification:OutdoorAir (Flow/Area) | `outdoor_air` per_area | exact |
| DesignSpecification:ZoneAirDistribution (Ez 1) | default | exact |
| Fan:ZoneExhaust ×8 | `exhaust_fans` (×N); the kitchen unit's and VAV_2's outdoor air covers their zones' exhaust, as E+ Controller:OutdoorAir | exact |

## Plant

| IDF object | Model | Status |
|---|---|---|
| PlantLoop HeatSys1, Sizing:Plant 48.89 / 13.89, SetpointManager:OutdoorAirReset 60 → 48.89 °C | `plant_loops` HeatSys1 with `setpoint_reset` | exact |
| Boiler:HotWater 1,549 kW, 0.945, LeavingSetpointModulated | `boiler` | exact |
| EMS SetBoilerPar / SetBoilerCurve (efficiency curve replaced each iteration with a function of PLR) | `efficiency_curve` as a PLR table of the same function | exact (tabulated, linear between points) |
| Pump:VariableSpeed HeatSys1 | `pump` with the IDF's head, motor efficiency, curve and shaft power per flow per head (1.4999 → impeller 0.6667) | exact |
| CoolSys1_Supply (primary: two chillers with constant-speed pumps) + CoolSys1_Demand (secondary: VS pump) + ideal HeatExchanger:FluidToFluid | one decoupled CHW loop: the chiller pumps (headered, 2) on the supply side, the secondary pump `loop_side: demand` | equivalent hydraulics; the heat-recovery chiller on this loop keeps the primary side running all year (pumps, COMPARISON_NOTES.md), and E+'s EMS stages chiller 2's pump with it |
| Chiller:Electric:ReformulatedEIR ×2 (670 kW, COP 5.60, LeavingCondenserWaterTemperature curves) | `chiller` model reformulated_eir | exact |
| PlantEquipmentOperation:CoolingLoad (chiller 1 to 682 kW, both above) + UniformLoad + EMS PlantPump_EMS_Program (chiller 2 off below 670 kW) | `staging_mode: sequential`, threshold 1.0, the heat-recovery chiller first | approx.: OpenBSE's `operation.load_ranges` runs only the listed units, and the heat-recovery chiller shares CoolSys1 here (E+ has it on its own loop), so the ranges cannot be written without it taking a uniform share; sequential staging loads chiller 1 to capacity before chiller 2 where E+ splits evenly once both run |
| Chiller:Electric:EIR heat-recovery chiller (94 kW, COP 2.84) on its own bypass loop pre-cooling the CHW return through an ideal HX, heat recovered to a loop whose dummy WaterHeater:Mixed preheats the HW return; EMS BypassHXStatus_Prg resets its setpoint for the heating demand | first chiller on CoolSys1 with `heat_recovery` to HeatSys1, `heating_led: true` | equivalent: E+'s EMS asks for HW demand ÷ (1 + 1/COP_rated) of cooling; OpenBSE solves the cooling whose heat meets the demand |
| CondenserLoop TowerWaterSys, 21.1 °C setpoint | `TowerWaterSys` plant loop | approx.: OpenBSE's chillers take their condenser temperature from the wet-bulb plus approach, not the tower loop (09-plant comparison) |
| CoolingTower:VariableSpeed ×2 (CoolTools), EMS ChangTowerFlow | `cooling_tower` variable speed | approx.: OpenBSE's tower is a load-ratio model, no CoolTools approach correlation |
| HeaderedPumps:VariableSpeed (4 pumps) | `pump` num_pumps 4 | exact inputs |
| Pump:VariableSpeed heat recovery loops | — | belong to the heat-recovery plumbing above (0.3 MWh/yr in E+) |

## Service water heating and refrigeration

| IDF object | Model | Status |
|---|---|---|
| WaterHeater:Mixed SWHSys1 (2,271 L, 176 kW gas, 0.95, 14.5 kW gas parasitic on and off cycle) on the SWH plant loop with Pump:ConstantSpeed | `dhw_systems` SWHSys1 with pump | exact |
| WaterUse:Equipment ×30 (×N), target 60 °C, plant-connected | `loads` with use 60 °C | exact |
| Booster (electric, 22.7 L, inlet 60 °C) and laundry (gas, 1,136 L) water heaters, stand-alone | their own `dhw_systems` | exact |
| Refrigeration:Case ×2, Refrigeration:CompressorRack ×2 | — | missing: OpenBSE has no refrigeration model. E+: 29.6 MWh (rack compressors 12.6) |
| EMS (8 programs) | as listed in their rows | — |
