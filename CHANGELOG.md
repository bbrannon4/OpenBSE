# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- **Energy-conservation and sanity checks during the run** (#160). Every run now checks the hand-offs where a load can go missing while each component is correct on its own, and reports to the `.err` file. All checks are on by default, read the solution without changing it, and cost well under 1% of run time.

  | Check | Severity |
  |---|---|
  | **Zone air heat balance**: surface convection + outdoor and interzone air + convective gains (incl. `other_sensible_gains`) + HVAC = air storage, per zone and timestep (1% of the terms' RSS, E+ uses 20%) and over the run (1% of the largest term) | severe |
  | **Plant loops**: supply equipment never delivers more than the loop demand, and every coil, terminal and panel a loop draws on reports a load (the #151 failure) | severe |
  | **Surface temperatures** beyond ±200 °C, which used to be held silently, are counted per surface; past 2.5× the limit the run stops, as in E+. NaN zone and surface states replaced in the history are counted too | severe |
  | **Unphysical outputs**: NaN or inf anywhere, a negative PLR or capacity, a COP ≤ 0 while running | severe |
  | **HVAC → zone delivery**: heat the air loops, VRF and radiant panels send equals heat the zone receives | warning (#166, #167) |
  | **Air components**: ṁ·Δh across each component equals its `thermal_output` | warning (#168) |
  | **Plant unmet demand** (E+ "Plant Supply Side Unmet Demand Rate") | warning |
  | **HVAC non-convergence**: timesteps that hit the iteration limit, with the worst zones and hours | warning (#170) |

  Each message names the zone, loop, component or surface, the size of the imbalance and the worst hour. Severe findings during the run don't cancel it, as in EnergyPlus. The `.err` footer now reads "Simulation completed: N warning(s), M severe error(s)". The closure checks that the shipped models still fail stay warnings until those issues are fixed. What they found:
  - zones receive a blend of the last two HVAC iterations' supply air while coils book the last one, up to 14% of a zone's HVAC heat over a year (#166);
  - radiant panels' convective share never reaches the zone (#167);
  - the A205 DX coil books 16% more cooling than its air gets (#168);
  - the cosim chiller example never runs its chiller (#169);
  - office_vrf fails to converge in 89% of timesteps (#170).

- **Interior shades as a layer** (#144). A `shade:` block on `shading_control` models a roller shade or similar with its own optical and thermal properties (solar transmittance and reflectance, openness, emissivity, distance to glass), instead of an SHGC swap. While deployed:
  - glass, shade and gap air are solved together, so the shade warms in the sun;
  - the room and the comfort grid see the shade's temperature;
  - only the openness lets beam through;
  - the window's share of radiant internal gains lands on the shade.
  
  Validated against EnergyPlus 25.2 on ASHRAE 140 case 600 with a roller shade: heating −0.7%, cooling +2.1%, hourly shade temperature within 0.49 K RMSE. New output `surface:shade_temperature`. The comfort JSON records when each shade is down, and the Workbench draws lowered shades. Example: `examples/roller_shade_office.yaml`.
- **Spatial comfort grid** (#136). A new opt-in `comfort_grid:` block lays occupant points across each zone's floor (default spacing 0.5 m) and writes `<stem>_comfort.json`. Each point gets its own mean radiant temperature, from view factors to the real surface polygons, using an ISO 7726 seated or standing person (averaged over facing direction) or a sphere. It also gets direct sun through the windows, using the beam each window actually transmits. Per-point annual statistics cover occupied hours: comfortable, cold, hot and sunlit hours, and mean and maximum operative temperature. The file also keeps hourly surface temperatures, window beam and sun direction, so a viewer can recompute any hour. Zones need vertices on every surface; others are skipped and listed. Zone air is well mixed, so the grid shows radiant and solar variation only, not a CFD air field. The grid adds well under 1% to run time (the DOE large office: 178,000 points, about 45 MB per year). Example: `examples/comfort_perimeter_office.yaml`.
- **Workbench Comfort tab** (#136): a 3D view of the comfort grid. It shows semi-transparent walls, tinted windows and a coloured tile per occupant point. Pick a metric (operative temperature, MRT, sun on the occupant, PMV or PPD), then step through any day and hour or play the day. You can isolate a zone, toggle roofs, and hover for point values. Colour scales are fixed per metric. A banner states that zone air is well mixed, so the colours show radiant and solar variation only.
- **Comfort tab, phase 4** (#136):
  - an Annual mode with per-point comfortable, cold, warm and in-sun shares of occupied hours, mean and maximum operative temperature, and spatial comfort autonomy;
  - a north-up 2D Plan view, and a north arrow that follows the 3D camera;
  - a zone picker to show any subset of zones or a whole floor (level) at once;
  - PNG export with title, legend and north arrow.
- **Reformulated EIR chillers** (`model: reformulated_eir`), following EnergyPlus `Chiller:Electric:ReformulatedEIR` with its `LeavingCondenserWaterTemperature` part-load curve.
  - CAPFT and EIRFT take the leaving condenser water temperature, and EIR-f(PLR) is a function of that temperature and PLR.
  - The leaving temperature depends on the chiller's own power, so it is solved each timestep from the condenser heat balance.
  - Condenser water flow is constant while the chiller runs. Unless `design_condenser_flow` is given, it is sized as in E+: capacity × (1 + 1/COP) at the condenser loop's design ΔT, or 5.56 K without a loop.
  - Water-cooled only, and all three curves are required; a missing one is a severe error.
  - New `bicubic` curve type (E+ `Curve:Bicubic`, 10 coefficients), for named and inline curves.
  - Checked against EnergyPlus 25.2 timestep outputs of the DOE prototype hospital's chiller: power within 0.01% and leaving condenser temperature within 0.001 K, from cycling at PLR 0.02 to full load. Replaying that chiller's annual E+ loads through the model reproduces its annual electricity to 0.1%.
  - Chillers using the default `model: eir` are unchanged.
- **`prototype_tests/hospital/Hospital_STD2022_Boulder.yaml` uses its EnergyPlus chillers.** As in the 2022 prototype (`ASHRAE901_Hospital_STD2022_Denver.idf` from energycodes.gov), both are now reformulated EIR chillers with:
  - `WC_PD_GT150_2010_PA` CAPFT and EIRFT curves and a `WC_PD_150to300_2010_PA` EIRFPLR curve;
  - min PLR and min unloading ratio 0.15;
  - the condenser loop's 21.1 °C setpoint.

  Results:
  - Cooling electricity goes from 297 to 289 MWh, and Chiller 1's COP from 6.15 to 6.33.
  - EnergyPlus 22.1 running that IDF on the same Boulder weather gives 402 MWh (the repo's chart shows 412). Of that, 205 MWh comes from the two main chillers (COP 7.3) and 198 MWh from a 94 kW heat-recovery chiller that OpenBSE does not model.
  - In E+, that chiller carries the year-round CHW base load: about 40 MWh a month in winter, against about 10 MWh for the main chillers. Here, Chiller 1 carries the whole winter load (about 60 MWh a month) at PLR 0.1–0.2 for 6,205 hours, where the curve's part-load penalty is steepest.
  - Summer loads match: July is 351 MWh here against 349 MWh for E+'s main chillers.
  - Replaying E+'s timestep loads through OpenBSE's equations reproduces E+'s main-chiller electricity to 0.1%. The YAML's fixed 6.67 °C CHW setpoint (E+ resets 6.7–8.9 °C on outdoor temperature) moves it by +0.6%, and the condenser approximation by −0.6%.
- **Design-day sizing can follow EnergyPlus's setpoint schedules and load averaging.**
  - Thermostats take `design_day_heating_setpoints` and `design_day_cooling_setpoints`: 24 hourly values held on heating and cooling design days, as E+'s WinterDesignDay and SummerDesignDay setpoint schedules are. A night setback then sizes for the morning warm-up.
  - `simulation.sizing_averaging_window` averages design-day zone loads over that many timesteps before their peak is taken (E+ `Sizing:Parameters` Timesteps in Averaging Window). The default of 1 keeps the peak timestep. Without averaging, a setpoint step sizes on one timestep's spike: a LargeOffice core's design heating load was 39.7 kW per floor against E+'s 9.5 kW, and is 14.0 kW with E+'s window of 6.
  - `simulation.auto_cooling_design_days: false` sizes on the listed design days only, as E+ does. The default keeps the monthly cooling design days generated from the weather file.
  - **Single-zone VAV.** A `psz_ac` loop with a `vav` fan and `cooling_sat_reset: single_zone` now works like E+'s PSZ-VAV (a VAV no-reheat terminal under `SetpointManager:SingleZone`). In cooling it holds its coldest supply temperature and turns the flow down toward `min_vav_fraction`; the supply temperature resets only once the flow reaches a limit. Heating and deadband run at minimum flow. Such a loop used to hold design flow. Only the LargeOffice's two basement loops combine the two.
  - `controls.zone_sum: coincident` sizes a VAV or dual-duct loop's supply flow on the largest sum of its zones' cooling flows at one hour of a cooling design day, as E+ `Sizing:System` with Coincident does. The default, `non_coincident`, sums each zone's own peak flow.

- **Chiller condenser heat recovery** (`heat_recovery:` on a chiller), following EnergyPlus `Chiller:Electric:EIR` heat recovery with a leaving temperature setpoint. Model `eir`, water-cooled only.
  - **The split.** Of the heat the chiller rejects, the heat-recovery water takes what heats it from the receiving loop's return temperature to `leaving_setpoint`, capped at `capacity_fraction` × full-load rejection and at the loop's remaining demand. The condenser takes the rest.
  - **The power penalty.** CAPFT and EIRFT see the heat-weighted blend of the two entering water temperatures, so recovering heat into warm water costs compressor power. The blend and the split are solved together each timestep; E+ uses the split from its previous iteration.
  - **The plant.** The recovered heat covers the receiving loop's load before its boilers, and a condenser loop gets only the rest.
- **`min_unloading_ratio` on chillers** (E+ Minimum Unloading Ratio, default `min_plr`). Between `min_plr` and this ratio, the chiller runs at the ratio and false-loads: the extra capacity is rejected with the condenser heat, as in E+.
- **Both are checked against EnergyPlus 22.1**, using timesteps of the DOE prototype hospital's heat-recovery chiller (`ASHRAE901_Hospital_STD2022_Denver.idf`, Boulder weather):
  - Power and recovered heat match to 0.001% when all heat is recovered and when the chiller false-loads below its 0.2 min unloading ratio.
  - They match to 0.16% when recovery is capped by the 48.89 °C setpoint, where E+'s one-iteration lag in the blend shows.
  - Replaying all of that chiller's annual E+ timesteps reproduces its electricity to −0.06% and its recovered heat to +0.08%.
- **Hospital heat-recovery chiller.** `prototype_tests/hospital/Hospital_STD2022_Boulder.yaml` now has the 2022 prototype's 94 kW heat-recovery chiller: COP 2.84, `Lp2LpChlr` curves, min PLR 0.1, min unloading ratio 0.2. It leads the CHW loop and recovers into the HHW loop at 48.89 °C. In E+ it pre-cools the CHW return through an ideal heat exchanger, and its heat reaches the hot-water return through a tank with no heater.
  - **Cooling electricity** goes from 289 to 328 MWh (E+ 402): 188 MWh from the heat-recovery chiller (E+ 198) and 140 MWh from the main chillers (E+ 205).
  - **Heating gas** goes from 70.7 to 4.8 MWh (E+ 647). The chiller recovers 62 MWh, against 653 MWh in E+, because the Hospital's hot-water load here is about 67 MWh a year against 1,194 MWh in E+. That is the known reheat gap.
  - **Effect of the small hot-water demand:** most of the chiller's condenser heat goes to 21 °C tower water, so it runs at COP 4.5 against E+'s 3.1.
  - **It also carries more CHW load** (838 against 620 MWh), since nothing here limits it the way E+'s bypass heat exchanger does.
### Changed
- **An `autosize` input that nothing sizes now stops the run.** Air-side sizing runs only from `design_days`, so without them every air-side `autosize` kept the internal sentinel (−99,999): fans fell back to 0.5 kg/s and coils to 0 W, with no warning. That is how `examples/datacenter_crah.yaml` ran its data hall at thousands of degrees. After air-side and plant sizing, every field still unsized is a severe error in the `.err` file, naming the component and field and saying what would size it: design days, or an explicit value where OpenBSE does not autosize that field. Covered: component capacities and design air and water flows, `controls.design_zone_flow`, dual-duct boxes, VRF indoor units (which fell back to 3 kW) and radiant panels. Fields resolved some other way are unaffected, such as the DHW pump's design flow, which comes from the draw rates.
- **A surface or internal mass that names an undefined construction is now an error.** It used to run with a placeholder: U = 5.0 W/m²K for opaque surfaces, U = 3.0 and SHGC = 0.7 for windows. The construction must also be the right kind. A window must name a `window_constructions` entry; other surfaces must name a `constructions`, `simple_constructions` or `f_factor_constructions` entry. No shipped model used an undefined name. A new test requires every shipped model to pass validation.
- **Unknown keys in a model are now an error** (#127). A misspelt or unsupported key (`efficency:`, `flow_per_person:`) used to be dropped silently and the field's default used, so a typo quietly changed the model. The error names the key, its location and the accepted keys. Models that relied on stray keys being ignored need those keys removed. Five examples that had never parsed were brought up to the current schema and now run: `chiller_table_curves`, `chw_plant_condenser`, `hotel_pthp`, `hotel_radiant_panels` and `office_dual_duct`. A new test parses every shipped model with the strict parser.
- **License changed to AGPL-3.0-only** (previously `MIT OR Apache-2.0`). Releases up to and including v0.7.0 remain available under their original terms. Also resolves the mismatch in #122, where the manifests declared a dual license but only the MIT text was included.
- Comfort grid: an hour counts as "in direct sun" only when the sun raises the occupant's MRT by at least 1 K. Faint beam, such as through a shade's openness, no longer marks a seat as sunlit.
- **Output aggregation is now per variable**. Energy meters (`building:*`, `submeter:*`, `zone:heating_energy`/`cooling_energy`) are summed, everything else is time-averaged, and any variable can be overridden with an `@mean|@sum|@min|@max` suffix (e.g. `zone:temperature@max`). The file-level `aggregation:` key is deprecated: it still applies to non-energy variables without a suffix and logs a warning.
- `building:*` and `submeter:*` output columns are now integrated energy in **Wh** (Σ power·dt) instead of summed watts labelled `[W]`. Values are unchanged at one timestep per hour and no longer N× too large at sub-hourly timesteps. Summing other rates integrates them too (kg/s → kg, m³/s → m³, unmet/active flags → hours).
- VRF electricity is split between Heating (Electric) and Cooling (Electric) by indoor-unit mode, instead of all being reported as cooling.
- Fan `source` values other than `constant_volume`, `vav` and `on_off` are now an input error. Previously, values such as `variable` or `variable_volume` silently ran as constant volume.
- Fixed examples that used ignored keys: `examples/datacenter_crah.yaml` (fan `source`, pump efficiency, unoccupied setpoint names) and `prototype_tests/large_office/LargeOffice_Boulder*.yaml` (data-center fans now actually run as VAV, which changes their results). `schema_sync` now fails on any unknown key in a repo YAML.

### Fixed
- **Cooling towers report their heat rejection as `thermal_output`** (#160). It was left at the default 0, so a condenser loop's remaining load was never reduced by its tower. Annual results of the shipped models are unchanged; the tower's `thermal_output` column in the results CSV now shows the heat removed from the condenser water (negative).
- **Plant equipment now receives its coil loads, and autosized boilers and chillers are sized.**
  - **`autosize` is resolved.** A boiler or chiller with `capacity: autosize` used to run with the −99,999 W sentinel as its capacity. A chiller then reported PLR −4 to −6 at about a quarter of its COP, and a boiler reported negative fuel, which the summary showed as zero gas. Autosized plant capacity is now the loop's design load, following EnergyPlus `Sizing:Plant` with its default non-coincident option: the sum of the design capacities of the coils, reheat terminals and radiant panels on the loop, after design-day sizing. A loop drawn on through a heat exchanger adds that loop's design load. A condenser loop's design load is its chillers' capacity × (1 + 1/COP). Autosized boiler water flow, cooling tower flow and heat exchanger flow carry that load at the loop's `design_delta_t`. Primary pumps keep their existing basis (the design-day coincident peak), and fall back to the loop design load when it is zero. Condenser pumps and towers are now sized from the sized chillers, where they used to read zero from an autosized one.
  - **An autosize that cannot be resolved stops the run.** Examples: a loop whose coils were never sized because there are no design days, or a loop with demand but zero design load. The failure is a severe error in the `.err` file. Equipment on a loop that nothing draws on never runs; as in EnergyPlus, its autosize resolves to zero.
  - **Unconnected water coils are an error.** In a model with plant loops, a `hot_water` or `chilled_water` coil, hot-water VAV reheat or water radiant panel with no `plant_loop` used to heat or cool for free while the loop's equipment sat idle; it is now a severe error that lists the loops to choose from. Without plant loops it is still accepted as an ideal water source (ASHRAE 140 AE cases). Other new checks:
    - `plant_loop` on a DX, gas or electric coil is a severe error.
    - A `plant_loop`, `condenser_plant_loop` or `source_loop` naming no loop is a severe error.
    - A plant loop nothing draws on is a warning.
  - **Examples wired to their plants.** `vav_reheat` and `doas_fancoil` had no `plant_loop` on any coil, and their "CHW" cooling coils defaulted to DX. Their headers describe chilled-water and hot-water coils, so the coils now use those sources and loops. The AHU preheat coil is now connected in `vav_chw_plant`, `vav_chw_plant_a205`, `vav_a205_fan_and_chiller`, `vav_a205_motor_override` and `cosim_chiller`. Annual results that change:
    - `office_dual_duct`: gas 0 → 10.3 MWh.
    - `vav_chw_plant`: gas 0 → 27.9 MWh; cooling electricity 10.2 → 6.3 MWh.
    - `doas_fancoil`: gas 0 → 105 MWh.
    - `datacenter_crah` (Chicago): chiller 591 kW, PLR 0.70–0.97; PUE 2.21 → 1.46.
  - New tests check that every shipped model connects its water coils, and that coil loads reach an autosized boiler and chiller end to end.
- **A chiller without an EIR-f(PLR) curve now keeps its rated COP at part load.** Chiller power is available capacity × (1/COP) × EIRFT × EIRFPLR × cycling ratio. Without an `eirfplr` curve, EIRFPLR was a flat 1.0, so the chiller drew full-load power at any load. Its COP fell to max(PLR, `min_plr`) × rated COP: 1.75 at PLR 0.5 for a 3.5-COP chiller.
  - EnergyPlus requires this curve. The fallback is now EIRFPLR = PLR, clamped to `min_plr`..1. COP stays at its rated value adjusted only by EIRFT, including while the chiller cycles below `min_plr`. 99 of the 111 air-cooled chillers in EnergyPlus's `AirCooledChiller.idf` dataset use exactly this curve.
  - Chillers with an `eirfplr` curve are unchanged, and no ASHRAE 140 case has a chiller.
  - Annual chiller results that change (seasonal COP, cooling electricity). Coil loads are unchanged. Seasonal COP can exceed rated because, without an `eirft` curve, the linear EIRFT fallback raises COP when the condenser is cooler than 29.4 °C.
    - `datacenter_crah` (Chicago TMY3): COP 2.77 → 3.49, rated 3.2; PUE 1.455 → 1.384.
    - `vav_reheat`: COP 1.44 → 3.93, rated 3.8; 12.5 → 4.6 MWh.
    - `vav_chw_plant`: COP 1.44 → 3.62, rated 3.5; 6.6 → 2.6 MWh.
    - `doas_fancoil`: COP 1.16 → 3.56, rated 3.5; 10.3 → 3.3 MWh.
    - `chw_plant_condenser`: COP 1.65 → 6.59, rated 6.0; 4.1 → 1.0 MWh.
    - `prototype_tests/hospital/Hospital_STD2022_Boulder.yaml`: COP 2.65 and 1.62 → 6.15 and 6.13, rated 5.6; 710 → 297 MWh against EnergyPlus's 412 MWh, so +72% becomes −28%. A gap remains because the YAML carries no chiller curves. The EnergyPlus prototype uses `Chiller:Electric:ReformulatedEIR` (as in E+'s `ASHRAE901_Hospital_STD2019_Denver.idf`), whose part-load curve also depends on condenser temperature.
- **Dual-duct boxes with `design_flow: autosize` are now sized.** A 0.5 kg/s placeholder kept sizing from replacing the value, so every such box ran at 0.5 kg/s whatever its zone needed. In `examples/office_dual_duct.yaml` the three boxes now get 0.36, 0.30 and 0.23 kg/s from design-day sizing; before, they summed to 1.5 kg/s against a fan sized for about 0.9 kg/s. Annual results change: fans 15.6 → 9.1 MWh, cooling electricity 16.3 → 10.4 MWh, unmet hours 3,520 → 2. Gas rises from 10.3 to 78.5 MWh. The old run delivered 68 MWh of zone heating on 10.3 MWh of gas, so its energy balance did not close. Now the 38 °C hot deck's minimum flow is heated and mixed with cold-deck air all year, as in any dual-duct system.
- **Two shipped models that failed validation run again.**
  - `prototype_tests/single_family/SingleFamily_CZ5B_Boulder.yaml`: since #150 rejects an unknown `ambient_zone`, the model has failed validation, because its water heater named the EnergyPlus zone `unheatedbsmt_unit1` instead of `unheatedbsmt`. Results are unchanged, because the tank's standby UA is 0.
  - `examples/multi_year_parametric.yaml`: its air loop served a zone it never defined, so it failed validation. It now defines the zone, its surfaces and a thermostat. Its Denver AMY weather files are still not shipped; run it with `-w`.
- **A heating supply temperature too low to heat a zone is now a warning, not a crash.** A PSZ, PTAC/PTHP or dual-duct loop heats its zones only with supply air at `heating_supply_temp`. At or below a served zone's heating setpoint, that air cannot heat the zone, and nothing said so. With `cycling: proportional`, a PSZ loop also crashed: its heating ramp clamps between the setpoint and `heating_supply_temp`, and that clamp panics when the setpoint is the higher of the two.
  - Validation now warns, naming the loop and each zone with its setpoint. VAV, DOAS and fan-coil loops are not checked, because their zones are reheated or the unit sets its own target. Loops with a supply-air reset are not checked either.
  - When there is nothing to ramp, the loop now holds `heating_supply_temp` instead of panicking. The single-zone reset can also push the temperature below the setpoint during a run.
  - `prototype_tests/hospital/Hospital_STD2022_Boulder.yaml` crashed this way in warmup and runs again. Its `CAV_KITCHEN` loop had copied only the 12.8 °C minimum of its EnergyPlus single-zone setpoint managers (12.8–40 °C), so it now heats at up to 40 °C.
- **Heat recovery runs in the air stream, as in EnergyPlus.** An energy recovery wheel or plate used to be left out of the air path. The air handler saw raw outdoor air, and the recovered heat was booked afterwards as a negative-fuel "HR Heat Savings" credit, sized from an estimated coil load. In the Hospital those credits (about −1,600 MWh) outweighed the 688 MWh the boiler burned. The summary clamped the negative total to zero, so it reported no heating gas at all.
  - The unit now tempers the outdoor air ahead of the mixing box, with EnergyPlus's controls. It is off while the economizer admits more than minimum outdoor air (`Economizer Lockout`). It is throttled so the mixed air does not pass its setpoint, the supply temperature less the fan rise (`SetpointManager:OutdoorAirPretreat` with supply-air temperature control).
  - New `cooling_sensible_effectiveness` input: EnergyPlus's separate cooling-season value. The Hospital wheels now use 0.70 in heating and 0.75 in cooling, as in its E+ model.
  - The credits and the clamp at zero are gone, so a negative gas total would now show.
- **VAV reheat is sized and controlled as in EnergyPlus.**
  - **Sizing.** Autosized reheat is now the zone's design heating load plus warming the box's reheat airflow from the air handler's supply temperature to the zone. It was the zone load alone, so a zone with no design heating load (large internal gains) got 0 W. Nine Hospital boxes got 0 W (labs, operating rooms, ER and ICU rooms, corridors), and those zones dropped as low as 1.6 °C.
  - **Control.** The box now reheats by the load to the heating setpoint plus the warming of its primary air, in heating and in deadband (E+ `SimVAV`). Before, it reheated by the zone load alone: heating zones fell short by the primary-air term, and deadband zones were dragged below setpoint by cold minimum flow.
  - **Damper.** The damper opens past minimum only when the reheat would exceed `max_reheat_temp` (`ReverseWithLimits`). It used to open in proportion to the load over the coil capacity, so a bigger coil opened it less.
  - Annual results that change (heating energy; unmet heating hours):
    - `Hospital_STD2022_Boulder`: gas 70.7 MWh (reported as 0 before); 71,148 → 6.
    - `LargeOffice_Boulder`: gas 574 → 688 MWh (+20%); −5%.
    - `LargeOffice_Boulder_expanded`: gas +18%; −3%.
    - `vav_chw_plant`, `vav_chw_plant_a205` and `cosim_chiller`: gas +5.5%; 147 → 0.
    - `vav_a205_fan_and_chiller` and `vav_a205_motor_override`: gas +24%; 41 → 0.
    - `chiller_table_curves`: electric −1%; 1,969 → 1,611.

    ASHRAE 140 is unchanged: its terminal-reheat cases fix their reheat capacity and hold minimum flow while reheating.
  - **Still off from EnergyPlus.** Hospital heating gas is about a tenth of the E+ value (666 MWh). Its VAV minimum flows are below E+'s: 0.5 against 1.0 for the labs and ICU rooms, 0.3 against 0.5 for the patient rooms. LargeOffice heating gas was already 42% above E+ (405 MWh) and is now 70% above. Both gaps need their own investigation.
- **Interior long-wave radiation conserves energy and uses each zone's real geometry.** The old model sorted surfaces onto the six faces of the zone's bounding box and let each one radiate to a face-averaged temperature. That only holds for an empty rectangular room. Internal mass was counted as part of a wall face, and angled or trapezoidal surfaces fit no face, so radiation did not balance. In a LargeOffice mid-floor perimeter zone, the surfaces sent out 7.6 kW more long-wave at night than they received, which cooled those zones 4–6 K below EnergyPlus.
  - View factors now come from the surfaces' own polygons (exact for a convex zone). Windows and doors are cut out of their walls, and internal mass sees and is seen in proportion to area, as in EnergyPlus. A zone where a surface lacks vertices uses EnergyPlus's area-weighted approximation.
  - The factors are made reciprocal and complete (EnergyPlus `FixViewFactors`) and turned into grey-body interchange factors (EnergyPlus `CalcScriptF`). A zone's net long-wave exchange now sums to zero, and a deployed interior shade updates its window's emissivity in the exchange.
  - LargeOffice January nights: the perimeter zones match EnergyPlus within 0.1–0.3 K (were 4–6 K low), and per-surface long-wave within a few percent (window +4.81 vs +4.83 kW).
  - ASHRAE 140: envelope-case heating falls 1–3% and cooling rises 0.4–2.4%; no new failures. Case 660 heating is now 20 kWh below its range (was 10, #148), and case 660 cooling sits at the top of its range (3,340.3 against 3,340, passing after rounding).
- **No outdoor air enters a zone while its air loop is off.** When a VAV or packaged loop that mixes the zone's outdoor air was off (scheduled off, or a fan cycled off), the envelope let the zone's design outdoor-air flow straight in at outdoor temperature. The code called this a "plenum effect", but EnergyPlus admits no outdoor air through an idle OA system. In the LargeOffice this took about 20 kW from each mid-floor core all night (17–18 °C against E+'s 25.6 °C). Unmet heating hours fall sharply in `office_vrf` (5,612 → 210), `hotel_pthp` (861 → 60) and `doe_retail_standalone` (−35%).
- **Beam solar is distributed geometrically in zones with internal mass.** With `full_interior_and_exterior`, a zone fell back to a fixed split by surface type whenever any surface lacked vertices, and internal mass never has them. A LargeOffice perimeter floor got about 20% of the solar instead of E+'s 55%. Internal mass now takes only diffuse and reflected solar, as in EnergyPlus.
- **VAV supply temperature follows EnergyPlus's Warmest setpoint whichever way the zones lean.** The controller turned off the cooling coil and the economizer whenever more zones wanted heat than cooling, and the central heating coil only protected against frost. The mixer and both coils now aim at the Warmest setpoint less any fan heat still downstream (EnergyPlus `SetpointManager:MixedAir`). The reheat boxes warm the zones that setpoint overcools. In January the LargeOffice mid-floor system supplied 18–20 °C air against E+'s 14.4–16 °C; it now supplies 14.4–15 °C. This changes most VAV results (e.g. `vav_reheat` cooling +99%, `vav_chw_plant` gas +19%); ASHRAE 140 is unchanged.
- **A zone's design airflow uses its outdoor air as a minimum, not as an extra load.** Sizing added each zone's outdoor-air cooling load, at the hottest design-day temperature, to its cooling airflow, and the code said E+ does the same. E+ does not. It sizes the zone on its own load and raises the flow to the zone's minimum outdoor-air flow only if that is larger (`UpdateZoneSizing`); the central coil meets the outdoor-air load. Zones with outdoor air now get smaller design airflows, and so do the fans and boxes sized from them. In a LargeOffice core, 10.7 kW of the 95 kW its airflow was sized on was outdoor air.
- **VAV boxes are sized with the cooling sizing factor.** The VAV control already scaled each zone's design flow by `cooling_sizing_factor`, but the box was autosized to the unscaled flow. The box minimum, a fraction of its maximum, was therefore 13% below E+'s at the usual factor of 1.15. Boxes now get the flow the control uses, as in E+. VAV and dual-duct supply fans are sized on the same flows, where they summed the unscaled ones: at the usual 1.15, the boxes together could call for 15% more air than the fan's design flow.
- **The VAV ventilation rate procedure uses the site's air density, and stops while outdoor air is off.** It turned supply mass flows into volumes at sea-level density (1.204 kg/m³), where E+ uses the site's standard density (about 0.98 kg/m³ in Denver and Boulder). That overstated the outdoor-air fraction by about 23% at altitude. It also ran while the loop's minimum outdoor air was zero, during unoccupied hours or with a minimum-OA schedule at zero. E+'s mechanical-ventilation controller is unavailable then, so night cycling drew outdoor air that E+ does not. Only loops whose zone terminals give `per_area_oa` or `per_person_oa` use the procedure: the Hospital and the LargeOffice.
- **A single-zone unit with a cycling fan no longer counts outdoor air it does not draw.** The cooling part-load ratio of PSZ, PTAC and PTHP units adds the outdoor air's sensible load to both the load and the capacity, because a fan that keeps running keeps drawing outdoor air while the compressor is off. A cycling fan draws none then, but the term was applied anyway. In the LargeOffice basement data centre, cold economizer air cancelled the whole 373 kW IT load. The unit sat at its 1.3% PLR floor, and the room ran at 64 °C in January and 153 °C in July, heating the basement and the floor above (E+: 27 °C). The term now applies only to continuous fans. No ASHRAE 140 case is affected: its cycling-fan cases carry no outdoor air.
- **Single-zone and CRAC/CRAH fans are sized on the flow their controls use.** A loop whose supply temperature differs from the building's coldest (a data centre at 15.89 °C beside 12.8 °C VAV systems) gets its zone flow resized for that temperature. The control used the resized flow, but the fan kept the flow sized at the coldest supply temperature: 26.7 m³/s against the 38.4 kg/s the basement data centre moves (E+: 38.7 m³/s).
- **Doors take their area out of their wall.** An opaque surface with a `parent_surface` (an EnergyPlus door) now subtracts its area from the parent, as windows do.
- **Zone air-balance and surface convection outputs were always zero.** `zone:q_surf_conv_*`, `zone:q_infiltration_sensible`, `zone:q_thermal_mass` and `surface:conv_to_zone` were listed as outputs but the writer never read them. They now report the values the envelope computes.
- **LargeOffice inputs now follow its EnergyPlus model.** `prototype_tests/large_office/envelope_from_idf.py` rebuilds the envelope from the reference IDF, and `INPUT_AUDIT.md` lists every input checked and the E+ inputs OpenBSE cannot express yet (daylight saving, thermostat schedules, two-speed DX and others). The envelope fixes include the mid floors' height, partitions, data-centre walls, top-floor roof and basement. Other inputs fixed: the data-centre IT schedule, outdoor air (area-based for every zone), VAV box minimums and reheat limits, gas data-centre heating with E+'s DX sizing and curves, hot-water reset and ΔT, chiller curves, and removing a condenser loop that E+ does not have. Annual results against E+ (472.5 MWh gas, 589.5 cooling, 1,043 fans, 135 pumps): gas 313 MWh (−34%), cooling 578 (−2%), fans 1,106 (+6%), pumps 88 (−35%). The remaining heating gap is in reheat (−23% on the mid floors, mostly the east, west and north zones) and the central heating coil (E+ 42 MWh, none here), alongside the gaps listed in the audit.
- **Water heaters exchange heat with the zone they sit in, and heat pump water heaters follow EnergyPlus** (#61).
  - **`ambient_zone` now works.** It was accepted but ignored, so every tank lost heat to a fixed 20 °C. The tank now loses heat at that zone's air temperature, and the losses warm the zone. This changes `ApartmentMidRise_Boulder` (tank in G Corridor, as in its E+ model): DHW gas −0.5%, heating gas −1.6%, cooling +0.9%.
  - **Heat pump capacity and COP vary.** They are now the rated values times curves of evaporator-inlet wet-bulb and tank water temperature, after EnergyPlus `Coil:WaterHeating:AirToWaterHeatPump`, instead of a fixed COP. COP falls as the tank heats up and rises with warmer air. `efficiency` is the COP at the rating point (19.7 °C dry-bulb / 13.5 °C wet-bulb air, 57.5 °C water). By default the curves are EnergyPlus's example HPWH curves rescaled to 1.0 at that point; `capacity_curve` and `cop_curve` override them.
  - **The heat pump cools and dries its zone.** Its evaporator draws the tank heat less the compressor power from the zone air. The split between cooling and drying uses EnergyPlus's bypass-factor method from `rated_shr` (default 0.85), so dry air gives a dry coil.
  - **Compressor cutout and backup element.** Below `min_inlet_air_temp` (default 10 °C) the compressor is off and only `backup_capacity` (a resistance element, default 0 W) heats the tank.
  - **Unknown zone is an error.** An `ambient_zone` that names no zone is now a validation error.
- **CRAC/CRAH data-center cooling conserves the IT heat and modulates airflow** (#62).
  - **IT heat counted once.** Contained hot-aisle exhaust now mixes by mass with room air at the unit inlet. The heat it carries straight to the coil is taken out of the room's balance. Previously the IT heat was a zone gain and was counted again in the return air, so the coil cooled heat that didn't exist.
  - **Airflow modulates.** With a `vav` fan, airflow tracks the load between the loop's `min_vav_fraction` and design flow, instead of running at full flow all year.
  - **Return temperature updates every HVAC iteration.**
  - **Example fixed.** `examples/datacenter_crah.yaml` had no design days, so none of its `autosize` fields were ever sized, and its server airflow was about 40 times too low. Together these drove the data hall to thousands of degrees. With both fixed, the hall holds 24 °C, and the CRAH fan uses 39% less energy than at constant volume.
  - **New warning.** Validation now warns when `airflow_m3_per_s_per_kw` implies a server air temperature rise above 30 K.
- **Evaporative cooler: indirect stage in series, configurable pump and fan power, water use** (#59). The indirect stage now works as EnergyPlus's CelDekPad does: the outdoor secondary air is cooled by its own pad, then cools the supply across the heat exchanger. The old single-product formula was only right when supply and secondary air were the same. New inputs: `pump_power` (default 100 W, previously hard-coded), `secondary_fan_power` and `secondary_air_flow_ratio`. Pump and fan power are drawn only while the cooler is cooling. Evaporated water (direct pad plus secondary pad) is reported as `water_consumption` [kg/s].
- **Steam humidifier adds the steam's heat and reports its water use** (#60). The steam enters at 100 °C, so the outlet air now carries its enthalpy and warms slightly (about 0.15 K per g/kg added), where it was held isothermal. A new `standby_power` input [W] is drawn by the controls whenever the unit is available, and the make-up water is reported as the component output `water_consumption` [kg/s].
- **A construction whose conduction transfer function can't be computed now stops the run with a clear error** (#121). The message names the construction and surface and suggests the usual remedy: thick, massive layers often need fewer `timesteps_per_hour`. Previously a non-converging CTF either ran on with a U-value off by orders of magnitude or crashed with a panic, while the `.err` file said "Errors: 0". The error, and any CTF whose steady-state U is 1–5% off, are now written to and counted in the `.err` file.
- **Weather files are checked instead of silently misread** (#67). A malformed record, a gap or duplicate hour, sub-hourly data, or an unreadable latitude, longitude, time zone or elevation is now an error naming the line, where before the record was skipped (shifting every later hour) or the value became 0. A file that starts after 1 January or ends before the run period is an error instead of the run being quietly shortened. 29 February records are dropped with a warning, since the simulation calendar has no leap day; previously they shifted every later date by a day. TMY3 files that write midnight as `00:00` of the next day are read correctly. EPW header lines are recognised by keyword rather than position.
- Windows' share of radiant internal gains (people, lights, equipment, radiant panels) was computed but never applied, so it left the heat balance: about 7% of radiant gains in ASHRAE 140 case 600, 11% in the comfort example. It now warms the glass, as in EnergyPlus (#142). Annual heating falls and cooling rises slightly, by about 0.5%.
- **Surfaces wound the wrong way faced the wrong direction.** Azimuth and tilt come from vertex order, so a surface listed clockwise from outside was simulated facing backwards: wrong solar, sky exchange and convection, and the wrong face in the interior radiant model. The engine now detects these surfaces without trusting any winding and reverses them, with a warning in the `.err` file. Detection: all of a zone's vertices lie on one side of the surface's plane, or a window faces the opposite way to its parent. Fixed in the repo models, which changes their results:
  - `examples/office_vrf.yaml`: the north conference window was simulated facing south.
  - `examples/retail_rtu.yaml`: 6 interior partitions sat on the wrong face of the radiant model, so the storefront glass never exchanged radiation with them.
  - `prototype_tests/large_office/LargeOffice_Boulder*.yaml`: 48 floors and ceilings, including the top-floor ZN_3 roof, which faced the ground.
  - `prototype_tests/single_family/SingleFamily_CZ5B_Boulder*.yaml`: the basement ceiling.
  
  A new `surface_winding` test fails if any repo model needs this correction.
- Schema: the zone `comfort.solar_absorptivity` default was documented as 0.57; the engine uses 0.67.
- Legacy underscore output variable names (`zone_temperature`, `zone_heating_energy`, …) were silently dropped, so their output files were never written. They are now accepted as aliases for `zone:temperature` etc., and variables that match no data log a warning.
- Workbench: the Energy tab now shows end uses and unmet hours from the summary report; the Network tab shows VRF systems; the selected weather file is remembered per model.
- `docs/openbse_schema.json` brought in sync with the engine. It now covers `vrf_systems`, `dhw_systems`, `radiant_panels`, `exterior_equipment`, `equipment_it`, `electrical_distribution`, `shading_surfaces`, `f_factor_constructions`, `climate_zone`, and many nested fields, so these are editable in the Workbench. A new `schema_sync` test fails if the schema and engine inputs drift apart.
- Workbench: the sidebar lists any schema class not yet categorized; curve, plant-loop and schedule reference fields offer dropdowns; the Network tab's Air Side / Water Side toggles are always clickable (an empty side shows a blank diagram).
- Fan `motor_a205_file`, `drive_a205_file` and `vfd_a205_file` were ignored; they now override the RS-0003 fan's sub-components.
- Custom output files dropped components that were idle on the first timestep. For example, a chiller off at 1 a.m. on January 1 never appeared all year. Idle plant equipment and VRF outdoor units now report zeros. Exact-name component variables keep their column and warn at the end of the run if they never report data.
- Plant equipment now writes its own reported outputs (e.g. co-simulation `power_w`), not just electric, fuel and thermal power.
- Output variables in the wrong order (`zone:<name>:<variable>`) now log a warning instead of silently producing nothing. Fixed this ordering in `examples/datacenter_crah.yaml` and `examples/cosim_chiller/building.yaml`.

## [0.7.0] - 2026-10-11

### Added
- **ASHRAE 140-2023 Section 9B (CE_b) dynamic HVAC BESTEST cooling** — all 20 cases (CE300–CE545) implemented, gated, and within the §9B comparative ranges on every primary metric (cooling/compressor energy, coil loads, COP2, zone temperature and humidity). The validation gate now runs 550 checks. Portal submission spreadsheet `Std140_CE_b_Output.xls` added.
- **Seasonal (date-range) schedules** — a `seasons:` list on any schedule, each period carrying its own day profile, resolved by day-of-year. Enables the CE_b internal-gain, outdoor-air, and thermostat schedules whose daily pattern changes over the year.
- **Performance-map DX outdoor-air coupling** on the PSZ cycling path — the off-cycle supply now delivers mixed (return + outdoor) air, the outdoor-air load is carried in the compressor part-load ratio, and a continuous supply fan no longer pins the compressor on at minimum outdoor air.
- **Air-side economizer extensions** — compressor lockout, fixed temperature/enthalpy high limits combined with a differential test, and a WARMEST/mixed-air load-reset supply target (CE400–CE440).
- **Scheduled minimum outdoor air** and **seasonal-schedule-aware infiltration** (CE320–CE340, CE530–CE545); **seasonal thermostat cooling-setpoint** support on the DX path (CE350).
- **Gas-furnace draft-fan power** — opt-in `parasitic_electric_load` [W] on gas heating coils (EnergyPlus `Coil:Heating:Fuel` on-cycle parasitic electric load). Runs with the burner and adds no heat to the supply air. The Section 10 cases HE170, HE210, HE220 and HE230 now include their 50 W draft fan in reported fan energy.

### Changed
- Sub-hourly output aggregation: component rate `[W]` variables are time-averaged when downsampling to a coarser reporting frequency instead of summed (no change at a 1-timestep-per-hour reporting cadence).
- Regenerated the ASHRAE 140 portal submission spreadsheets (TF, CE_a, CE_b, HE, AE) for v0.7.0.

## [0.6.0] - 2026-06-13

### Added
- **`openbse-airloop` crate** — the seven per-system-type control signal builders (PSZ, CRAC, CRAH, DOAS, FCU, VAV, dual-duct) plus `LoopInfo`/`HvacMode` moved out of the 8.4k-line CLI driver into a focused, separately-testable crate (workspace is now 11 crates). Builders take a single `SignalCtx` instead of long per-zone-map argument lists.

### Changed
- **Component detailed-output reporting is now consistent under part-load.** When the system-level PLR block scales a component's `thermal_output`/`mass_flow`, the rate/flow columns from `detailed_outputs()` (`sensible_load`, `total_load`, `latent_load`, `cycling_loss`, etc.) are scaled by the same factor, so CSV columns for a component no longer disagree (e.g. `mass_flow=0` with `total_load≠0`). Intensive states (temperatures, `plr`, COP) remain on-cycle/pre-PLR by design; system PLR is reported separately as `__loop_plr__`. *Note: part-load detailed-output columns in results CSVs now reflect time-averaged values.*
- **Internal:** component `detailed_outputs()` replaced by a non-allocating visitor `report_outputs()` on the hot path (`detailed_outputs()` retained as a convenience wrapper); single-source `EquipmentInput::component_kind()`; CTF warnings routed through `log` instead of `eprintln!`; workspace-wide clippy cleanup.

## [0.5.1] - 2026-06-10

### Fixed
- `examples/datacenter_crah.yaml` — corrected multiple YAML format errors (vertex structs, `zone_terminals`, construction layer thicknesses, `economizer` struct, `minimum_damper_position` field name). Example now runs end-to-end and produces PUE/MLC/ELC output.
- Updated example to match its intended topology: air-cooled chiller (not water-cooled), UPS Room + PDU Room cooled by dedicated air-cooled CRACs, and an Office block served by PSZ-AC.

## [0.5.0] - 2026-06-10

### Added
- **Data center zone model** (`DataCenterConfig`) — implicit hot/cold-aisle physics with configurable rack outlet temperature, containment efficiency, and ASHRAE A1–A4 inlet-temperature limits. IT load defined via `data_center:` block on a zone (auto-generates internal gains) or top-level `equipment_it:` entries. Rack outlet temperature (`dc_rack_outlet_temp`) and CRAC/CRAH return-air temperature (`dc_return_air_temp`) are available as per-zone output variables. Closes #46.
- **CRAC and CRAH system types** — `system_type: crac` (self-contained DX, no OA mixing, high-sensible) and `system_type: crah` (chilled-water air handler, no OA). Both are cooling-only, recirculate room air, and auto-assign "datacenter" submeter to all loop components. Supply temperature is overridden from `rack_inlet_temp_max_c` when the served zone has a `DataCenterConfig`. Closes #47.
- **ASHRAE 90.4 PUE/MLC/ELC reporting** — summary report now includes a "Data Center Performance" section (text and HTML) when IT equipment loads are present, showing annual IT load, mechanical load, electrical-distribution losses, PUE, MLC (vs climate-zone limit), and ELC (limit 0.060 all zones), with PASS/FAIL indicators. Add `climate_zone: "5A"` (or any ASHRAE CZ) to the model YAML for limit-table lookup. Closes #48.
- **Electrical distribution losses** — `electrical_distribution:` model-level block with `ups:` and `transformers:` entries (efficiency, rated IT load). Losses scale with actual IT load PLR and are tracked in the `elec_dist_power` snapshot field for ELC computation.
- **`equipment_it:` model-level block** — same structure as `equipment:` but routed to `ComponentKind::ItEquipment` for 90.4 accounting; auto-assigned "datacenter" submeter.
- **`climate_zone:` model-level field** — optional ASHRAE climate zone string (e.g. `"5A"`) passed to the summary report for 90.4 MLC limit lookup.
- New example: [`examples/datacenter_crah.yaml`](examples/datacenter_crah.yaml) — mixed-use 500 kW data center (DataHall with CRAH + air-cooled chiller, UPS/PDU electrical rooms with dedicated air-cooled CRACs) plus an Office block on PSZ-AC; demonstrates 90.4 PUE/MLC/ELC reporting alongside standard 90.1 office loads.

## [0.4.0] - 2026-05-20

### Added
- **Co-simulation interface** — new `openbse-cosim` crate lets users replace any component with an external model running in Python, Modelica, MATLAB/Simulink, or any other tool. OpenBSE acts as simulation master; external models run as child processes and exchange one JSON message per timestep over stdin/stdout. No external libraries required.
  - `ExternalAirComponent` — implements `AirComponent`; replaces a fan, coil, AHU, or any air-side component. Configured as `type: external_air` in YAML.
  - `ExternalPlantComponent` — implements `PlantComponent`; replaces a chiller, boiler, pump, or any plant-side component. Configured as `type: external_plant` in YAML.
  - Both types accept a user-defined `inputs:` list (variables OpenBSE sends) and `outputs:` list (variables the external process returns). Available inputs cover inlet conditions, outdoor conditions, simulation time, and calendar position.
  - `python/openbse_cosim.py` — thin Python helper (`run_cosim(step_fn)`) that handles the stdin/stdout loop boilerplate.
  - `examples/cosim_chiller/` — reference example: VAV office building with the native chiller replaced by a Python COP+PLR model (`chiller.py`). The script is self-contained and needs no external dependencies. Closes #44.

## [0.3.0] - 2026-05-13

### Added
- **ASHRAE Standard 205 (RS0001–RS0007) equipment performance data support** — manufacturers publish equipment performance as portable CBOR (`.a205`) or JSON files; OpenBSE now consumes them directly via a new `openbse-a205` crate and matching `*_a205` components. Existing polynomial / table-curve modes are unchanged; Standard 205 is selected by adding an `a205_file:` field to a chiller, cooling coil, or fan input. Relative paths resolve against the model YAML directory; absolute paths supported.
  - **RS0001 Chiller** — `ChillerA205` plant component. Reads a 6-axis cooling map (evap flow, evap leaving temp, condenser temp/RH/flow, compressor sequence) plus standby map; supports air-cooled, water-cooled, evaporative condenser types; continuous and discrete compressor speed control with cycling degradation. YAML: `a205_file:` on `type: chiller`. Example: `examples/vav_chw_plant_a205.yaml`.
  - **RS0004 Air-to-Air DX** — `CoolingCoilDXA205` air component. Reads gross total / sensible / power from a 6-axis map; splits delivered total cooling into sensible (lowers T_db) and latent (lowers W) using the file's sensible/total ratio. YAML: `a205_file:` on `type: cooling_coil` with DX source. Example: `examples/residential_unitary_a205.yaml`. Closes #38.
  - **RS0002 Unitary wrapper** — the `a205_file:` field on a cooling coil now also accepts an RS0002 unitary file (auto-extracts the embedded RS0004 DX system). Example: `examples/residential_unitary_rs0002.yaml`. Closes #39.
  - **RS0003 Fan Assembly** — `FanA205` air component. Reads shaft power and impeller speed from a 2-D (flow, static pressure) map. Honors the file's `heat_loss_fraction` to determine fan motor heat into the airstream. YAML: `a205_file:` on `type: fan`. Example: `examples/vav_a205_fan_and_chiller.yaml`. Closes #39.
  - **Nested RS0005 / RS0006 / RS0007** — a `FanEfficiencyChain` composes motor (RS0005) + electronic drive/VFD (RS0006) + mechanical drive/belt (RS0007) sub-components from the fan file's `assembly_components` list to compute grid-side electric power from shaft power. Falls back to a configurable motor efficiency when the file's assembly is empty. Closes #39.
  - **Standalone sub-component overrides** — `motor_a205_file:`, `drive_a205_file:`, and `vfd_a205_file:` fields on a fan input let users swap in a manufacturer-supplied motor/belt/VFD file independently of the RS0003 fan file's nested assembly. Precedence: standalone override > nested in 205 file > fallback constant. Example: `examples/vav_a205_motor_override.yaml`. Closes #40.
- New `openbse-a205` crate with generic N-d linear interpolator (edge-clamp extrapolation policy), CBOR/JSON auto-detecting loader, and per-RS schema modules.

### Changed
- `openbse-io::build_graph` is preserved for backwards compatibility; new `build_graph_with_base(model, model_dir)` accepts the model directory so relative `a205_file:` paths can be resolved. The CLI uses the new entry point.
- `ChillerInput.capacity` and `CoolingCoilInput.capacity` are now optional and ignored when `a205_file:` is set (the file is authoritative on size and efficiency).
- `ChillerInput` gains named-reference fields `capft_curve` / `eirft_curve` / `eirfplr_curve` for resolving table curves from the model's `performance_curves` section (the table-curve YAML example was previously aspirational).

## [0.2.11] - 2026-04-27

### Added
- **Energy End-Use Dashboard** (`tools/editor`) — New ⚡ Energy tab shows a monthly stacked bar chart and annual summary table for electricity, natural gas, district heat/cool end uses. Zone-level unmet heating/cooling hours displayed with per-zone breakdown when any zone exceeds zero. Reads site-level `*_kwh` columns from the loaded results CSV automatically. SI (kWh) and IP (kBtu) unit systems supported. Closes #24.
- **HVAC network expanded** (`tools/editor`) — `NetworkView` now renders `evap_cooler` (sublabel from mode), `vrf_outdoor_unit`, `vrf_indoor_unit`, `radiant_panel` (sublabel from source) on the air-side graph and `thermal_storage` (sublabel shows type + strategy), `gshp` on the plant-side graph. `dual_duct_box` labeled "Dual Duct Box" in terminal nodes. All new types have distinct Tokyo Night theme colors and 20×20 SVG icons. Closes #24.
- **View Report button** (`tools/editor`) — Results toolbar "View Report" button locates the `*_summary.html` file next to the active CSV and opens it in the default browser via a new `open_in_browser` Tauri command. Shows an inline message if no report is found. Closes #24.

## [0.2.10] - 2026-04-19

### Added
- **Setpoint reset controls** — OA-based and demand-based SAT reset on `AirLoopControls` (`cooling_sat_reset`, `heating_sat_reset`); OA-based plant loop setpoint reset on `PlantLoopInput` (`setpoint_reset`). Closes #32.
- **Evaporative cooling** — `EvapCooler` component with `direct`, `indirect`, and `two_stage` modes. Integrated as air-loop equipment via `type: evap_cooler`. Closes #33.
- **Thermal energy storage** — `ThermalStorage` plant component with `chilled_water` and `ice` types. `full_storage`, `load_leveling`, and `demand_limiting` control strategies. Standby loss via `loss_ua`. Closes #37.

## [0.2.9] - 2026-04-19

### Added
- **Dual-duct CAV system** — `system_type: dual_duct` with `DualDuctBox` mixing box terminals. Hot and cold deck supply temperatures blend per zone proportionally to load. Autosizing, parametric override support. See `examples/office_dual_duct.yaml`. Closes #13.

## [0.2.8] - 2026-04-19

### Added
- **Ground-source heat pump (GSHP)** — New `GroundSourceHeatPump` component (`gshp.rs`). Uses Kusuda-Achenbach ground temperature equation as the entering water temperature (EWT) instead of a condenser water loop. Three ground temp source modes: `auto` (derives mean/amplitude/phase from hourly EPW data), `epw_monthly` (uses EPW header ground temperature table when available, falls back to `auto`), `monthly` (user-supplied 12-value table [°C]). Separate `rated_cooling_capacity` and `rated_heating_capacity` with `autosize` support. Optional capacity/EIR performance curves (`cap_ft`, `eir_ft`). Compressor energy routed to `cooling_electric` end-use. See `examples/residential_gshp.yaml`. Closes #12.

### Added
- **Table-lookup performance curves** — `PerformanceCurve::TableLookup` variant accepts manufacturer tabular data directly (no polynomial fitting). N-linear interpolation, per-axis `hold_edge`/`linear` extrapolation, optional output clamping. Axis variables are named (`outdoor_dry_bulb`, `part_load_ratio`, etc.) — order-independent and validated against slot type contracts at parse time. Closes #30.

## [0.2.6] - 2026-04-18

### Added
- **VRF systems** — `VrfOutdoorUnit` coordinates multiple `VrfIndoorUnit` fan-coil units per zone. Performance curves f(outdoor T, indoor T). Heat recovery mode routes cooling rejection to heating zones, reducing compressor work. Capacity limiting with proportional PLR scaling. See `examples/office_vrf.yaml`. Closes #10.
- **Radiant panels** — `RadiantPanel` component supporting fin-tube radiators, panel radiators, and chilled ceiling panels. Hot-water, chilled-water, and electric sources. Configurable `radiant_fraction` (default 0.50 for heating, 0.70 for cooling). Radiant portion distributed to zone surfaces via area×absorptance, affecting MRT. New `radiant_gains` field on `ZoneHvacConditions`. Closes #11.
- **HAMT moisture transport** — 1D finite-difference coupled heat-and-moisture solver. Activated per-construction when all material layers have `vapor_resistance_factor` and `sorption_isotherm` defined; falls back to CTF otherwise. New material fields: `vapor_resistance_factor`, `sorption_isotherm`, `liquid_transport_coeff`. Closes #5.

## [0.2.5] - 2026-04-18

### Added

- **DX coil dehumidification** — `autocalculate_shr` now defaults to `true` for all DX coils. Supply air humidity ratio is wired into `ZoneHvacConditions` so dehumidified coil outlet air correctly updates the zone moisture balance each timestep. Closes #7.
- **Humidity-based controls** — `max_relative_humidity` and `min_relative_humidity` setpoints on zones (%). When zone RH exceeds the max, dehumidification mode activates the DX coil even without sensible cooling demand. When RH drops below the min, the humidifier activates. Closes #15.
- **Multi-speed and variable-speed DX coils** — `CoolingCoilDXMultiSpeed` with per-speed `DXSpeedStage` entries (capacity, COP, SHR, performance curves). Two-speed control selects low/high stage based on sensible load; variable-speed interpolates capacity and COP between bounding stages. Use `source: dx_multispeed` in YAML. Closes #6.
- **Water-source heat pump (WSHP)** — New `WaterSourceHeatPump` component (`wshp.rs`). Performance curves as f(entering water temp, entering air temp). Heat rejection/absorption is energy-balanced for both heating (extracts from water loop) and cooling (rejects to water loop) modes. Connects to a condenser water plant loop. Use `source: wshp` in YAML. Closes #9.
- **Chiller lead/lag sequencing** — `staging_mode: sequential` (default, already working) or `equal_split` on `PlantLoopInput`. Sequential fills each unit to capacity before staging the next; equal split divides load evenly. Optional `staging_threshold` (default 0.9) prevents premature lag-chiller starts in sequential mode. Closes #14.

## [0.2.4] - 2026-04-18

### Added

- **PTHP system type** — `system_type: pthp` adds packaged terminal heat pump support: heat pump heating coil with ON/OFF PLR cycling (same as DX cooling, not PLR=1 water-coil modulation). Existing `HeatingCoilInput` with `source: heat_pump` wires directly in. Includes `examples/hotel_pthp.yaml`. Closes #8.
- **Advanced economizer modes** — Three new `EconomizerType` variants available in all system types (PSZ-AC and VAV): `fixed_enthalpy` (OA when outdoor enthalpy < configurable limit, default 65.2 kJ/kg), `enthalpy_with_high_limit` (differential enthalpy AND dry-bulb high-limit). Also fixes `differential_enthalpy` which was incorrectly comparing temperatures instead of enthalpies. Closes #16.
- **ComponentKind enum** — All HVAC components now implement `component_kind() -> ComponentKind` on the `AirComponent` and `PlantComponent` traits, enabling type-safe energy accounting without string matching. Closes #23.
- **`docs/CONTRIBUTING_COMPONENTS.md`** — Step-by-step guide for adding new physics components: trait implementation, YAML registration, sign conventions, unit reference table, and a worked example. Closes #23.
- **Zone air balance diagnostic outputs** — New per-zone output variables `q_surf_conv_total`, `q_surf_conv_walls`, `q_surf_conv_floors`, `q_surf_conv_roofs`, `q_surf_conv_windows`, `q_infiltration_sensible`, and `q_thermal_mass` expose each term of the zone air energy balance for validation and debugging.
- **Per-surface convection outputs** — `conv_to_zone` (h_conv × A × ΔT) and `h_conv_inside` are now available as surface-level output variables.
- **`compare_zone_balance.py`** — Zone air balance comparison script for Single Family prototype (OB vs E+ component-by-component).

### Fixed

- **Inter-floor slab solar interaction (Single Family prototype)** — The 1F/2F structural deck was modeled as an adiabatic surface, causing `FullExterior` solar distribution to send ~50% of beam solar to it. Since both faces are in the same zone the surface is adiabatic, so all absorbed energy recycled back to zone air with no loss path — an artificial gain loop not present in E+. Converted to `internal_mass` (matching E+'s approach): thermal mass is preserved, solar distribution is excluded. Heating error vs E+ reduced from +18% to +4.8%; cooling from unconstrained to +3.5%. Closes #18.
- **`differential_enthalpy` economizer** — Was comparing outdoor dry-bulb temperature against return air temperature instead of computing and comparing air enthalpies. Now correctly uses h = cp·T + hfg·w.

## [0.2.3] - 2026-04-06

### Added

- **ASHRAE 140 CI regression gate** — All 63 ASHRAE 140-2023 cases run on every push/PR; CI fails if any currently-passing case regresses.

### Fixed

- `build_140_csv.py` column matching for free-float and sun zone temperatures broken by output format changes.

## [0.2.2] - 2026-03-29

### Added

- **Output variable customization** — Outputs can now be specified for 'all' relevant parameters (zones, surfaces, components, etc) within the Workbench editor.  Note that the drop down menu functionality is not yet integrated and so requires manual text entry.


## [0.2.0] - 2026-03-25

### Added

- **Parametric run engine** — Execute multiple simulation runs with parameter overrides from a single YAML file. Supports scalar overrides on any named component field (`"Boiler-1.efficiency": 0.95`), per-run weather file swaps, and automatic sweep expansion with zip or Cartesian product modes. Each run produces a separate results CSV.
- **Sweep syntax** — Auto-generate parametric runs from `values: [0.80, 0.85, 0.90]` lists or `range: { min: 3.0, max: 5.0, step: 0.5 }` specifications. Combine multiple sweeps with `cross_product: true` for full factorial analysis.
- **Section override scaffolding** — Data structures and YAML parsing for future template-based section replacement (`include:` with `replaces:` lists). Execution not yet implemented.

## [0.1.0] - 2026-03-23

### Added

- **Simulation engine** — Graph-based execution with automatic topological sort and sub-hourly timesteps
- **Clean YAML input format** — Declarative building models replacing legacy IDD/IDF with composable, human-readable definitions
- **Building envelope physics** — Materials, constructions, windows, CTF conduction, interior/exterior convection, solar distribution (28-bin SHGC mapping), external beam shadow geometry, longwave radiation exchange, and zone air heat balance (3rd-order BDF)
- **Infiltration modeling** — ASHRAE enhanced combined model with multizone pressure-network airflow solver
- **Internal gains** — People, lights, and equipment with radiant/convective/latent splits and schedule control
- **HVAC components** — Fans, heating coils (electric, hot-water, gas), DX cooling coils with performance curves, heat-pump coils, ducts, boilers, chillers, cooling towers, heat recovery, water-to-water heat exchangers, and pumps
- **System templates** — PSZ-AC, DOAS + fan-coil, VAV with per-zone reheat, and residential unitary configurations
- **Plant loops** — Multi-loop simulation with topological ordering, supply/demand side wiring, and condenser loops
- **Controls framework** — Decoupled sensor/actuator control model with thermostats, setpoint managers, and night ventilation
- **Design-day autosizing** — Automatic equipment sizing from design-day peak loads
- **Weather support** — EPW and TMY3 CSV weather file parsing
- **Solar cache** — Disk-persistent solar pre-computation cache for faster repeated runs
- **Output reporting** — Configurable CSV time-series output and end-of-run summary reports (HTML and CSV)
- **Holiday schedules** — Named holiday dates with schedule day-type overrides
- **ASHRAE Standard 140-2023 validation** — 28 test cases implemented, 63/63 metrics passing
- **DOE prototype models** — Single-family, mid-rise apartment, large office, and hospital building models for validation
- **CLI tool (`openbse`)** — Run simulations, view summaries, and export results from the command line
- **Desktop editor (`openbse-editor`)** — Tauri + React GUI with schema-driven object editing, validation, and integrated simulation runner
- **Psychrometric library** — Moist-air property calculations based on Hyland & Wexler correlations
- **8-crate workspace** — Modular architecture: core, components, controls, envelope, weather, psychrometrics, io, cli
- **11 example models** — From simple single-zone to full VAV + chilled-water plant configurations
