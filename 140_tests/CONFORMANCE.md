# ASHRAE Standard 140-2023 conformance record

This record checks every ASHRAE 140 case in `140_tests/cases/` against each requirement of ANSI/ASHRAE Standard 140-2023 that applies to it: the case's own clauses, every base case it builds on, and the general requirements. For each requirement it gives the clause, the specified value, the case input, the engine code path that uses that input, and a status.

Landing inside the acceptance ranges is a separate test (`README.md`). A case can land in range on wrong inputs; this record checks the inputs, the engine path and the outputs directly.

## Summary

| Section | Cases | Deviations found | Fixed | Tracked (issue) | Equivalent / not modelled |
|---|---|---|---|---|---|
| General requirements (5.1.1, Annex A1) | all 121 | 1 | 1 | 0 | 1 |
| 7 Building thermal envelope and fabric | 27 | 8 | 8 | 0 | 0 |
| 7.2.3 In-depth (195–470, 800, 810) | 25 | 0 (new) | — | 0 | 0 |
| 9.2.1–9.2.2 Space cooling, analytical (CE100–CE200) | 14 | 1 | 1 | 0 | 2 |
| 9.2.3–9.2.4 Space cooling, comparative (CE300–CE545) | 20 | 2 | 2 | 0 | 1 |
| 10 Space heating (HE100–HE230) | 11 | 6 | 6 | 0 | 0 |
| 11 Air-side HVAC (AE101–AE445) | 24 | 2 | 2 | 0 | 2 |

Machine checks: `conformance_spec.json` lists 4,442 specified values across the 121 cases with their clauses; `crates/openbse-io/tests/ashrae140_conformance.rs` parses each case with the engine's input parser and checks every value, and fails if a case has no entry. Each case's weather file is hash-pinned in `.github/workflows/ci.yml`. See [CI guards](#ci-guards).

### Fixes and issues from this audit

| PR / issue | What | Sections |
|---|---|---|
| #524 | Case 960 sun-zone solar absorptance 0.6 (was 0.75, from the accompanying EnergyPlus input) | 7 |
| #525 | The Standard's layered constructions (were U-factor + heat capacity, layers guessed by the engine) | 7 |
| #526 | Clear glass pane 3.048 mm (was 3.175 mm, the 140-2017 value) | 7 |
| #527 | One interior solar-distribution method and time step in every case | 7 |
| #528 | Night ventilation unconditional and on 18:00–07:00 in every vent case | 7 |
| #529 | Case 940 uses the Case 640 setback schedule | 7 |
| #533 | Annual peaks reported as hourly integrated values | 7 (output) |
| #530 | Normative weather for Sections 10 and 11; Section 10 infiltration for UA 199.953 W/K | 10, 11 |
| #531 | HE220/HE230 setback from 23:00 | 10 |
| #532 | Section 10 circulating fan 0.355 m³/s at 200 W | 10 |
| #519 | Thermostat schedule entries ramp within their period; Cases 640/940 use the Standard's 07:00–08:00 ramp | 7 |
| #520 | Gas coil part-load fuel curve; Section 10 cases use the Standard's HIR(PLR) | 10 |
| #521 | Zone air starts at the first hour's outdoor air conditions (was 21 °C / 0.008 kg/kg); CE530–CE545 drop the reference input's January infiltration (0.0 ach all year, 9.2.3.5) | 5.1.1, 9 |
| #523 | Outdoor humidity ratio from the dew point (was whole-percent RH) | 9, 10, 11 |
| #535 | Section 10 envelope per Table 10-2 (was adiabatic surfaces) | 10 |
| v0.8.0 | Output forms: Section 7 solar, sky-temperature and specific-day outputs (7.3.2–7.3.5, 7.3.8) and zero peaks entered as 0 (7.3.1.5); Section 11 two-zone RHcco (was 0) and outdoor-air states from the dew point; program version and dates from `Cargo.toml` and `CHANGELOG.md`; Section 10 CODENAME/COUNTRY in every block; modeling notes (Annex A2 item i) for each section | 7, 10, 11 (output) |
| v0.8.1 | Section 7.2.3 in-depth cases added (25 cases, below); engine: constant combined surface coefficients for Cases 450–470, and a zone's volume from its parent surfaces only (an opaque subsurface no longer breaks the closed-shell test) | 7 |
| #53 (existing) | `simple_constructions` layer guess tuned to 140 constructions; no 140 case uses it now | — |

## Method and citations

The requirements were read in the Standard's 2023 text; clause and table numbers below are that text's. The Standard's accompanying EnergyPlus input files and `140_tests/reference_idfs/` were used as a cross-check; where they disagree with the text, the text governs, and the line says so. The Standard and its accompanying files are licensed and are not in this repository; this record gives clause numbers and specified values only.

Each requirement was checked three ways: the **input** (the case YAML and weather file state the specified value, or an equivalent with its derivation), the **engine** (the code path that uses the input, read in the source, so a value is not ignored, defaulted or overridden), and the **output** (the reported result is the quantity the Standard defines, from the right variable). Statuses: **conforming**; **fixed** (deviation fixed, with PR); **tracked** (deviation, with issue); **equivalent** (an equivalent input the Standard permits under 5.1.1.5, with its basis); **not modelled** (with reason and expected effect).

## General requirements (all cases)

| # | Requirement | Clause | Specified | OpenBSE input | Engine path | Status |
|---|---|---|---|---|---|---|
| G1 | Time convention | 5.1.1.1 | Local standard time; hour 1 = 00:00–01:00; no DST or holidays | Hour-ending schedules (hour N = (N−1):00–N:00); no DST input | `ZoneInput::base_setpoints`, `scheduled_ventilation_flow` (`zone.rs`) take the weather hour 1–24 | conforming |
| G2 | Geometry convention | 5.1.1.2 | Interior air volume as specified | Surfaces on interior dimensions; volume from vertices (7) or stated (9–11) | `resolve_geometry` | conforming |
| G3 | Nonapplicable inputs | 5.1.1.3 | Disregard | Alternative film coefficients, solar fractions, angular tables, bypass factor not input | — | conforming |
| G4 | Consistent modelling methods | 5.1.1.4 | Same option in all cases | One solar-distribution method and 4 steps/h in all Section 7 cases | — | fixed (#527) |
| G5 | Equivalent modelling methods | 5.1.1.5 | Equivalent inputs with a basis, applied consistently | Lines marked "equivalent" below | — | conforming |
| G6 | Nonspecified inputs | 5.1.1.6 | Only alternative film coefficients, solar fractions, altitude air density | None used (films, solar distribution and density computed) | — | conforming |
| G7 | Preconditioning | 5.1.1.7.2 | Apply the program's preconditioning | First week repeated until EnergyPlus warm-up tolerances are met (≤ 28 days) | `openbse-cli/src/warmup.rs` | conforming |
| G8 | Initial zone state | 5.1.1.7.2 | Not specified for Class I (the outdoor-air start of 5.2.1.6 is for Class II; HVAC BESTEST Vol. 2 §1.2.2.5 for CE300–CE545) | zone air at the first warm-up hour's outdoor dry-bulb and humidity ratio | `HeatBalance::initialize_zone_air_from_outdoor` | conforming (fixed, #521) |
| G9 | Simulation duration | 5.1.1.8 | 7, 9.2.3–9.2.4 annual; 9.2.1–9.2.2 ≥ Jan–Feb, Feb reported; 10 Jan–Mar; 11 until steady | 7, 9B Jan–Dec; 9A Jan–Mar, Feb reported; 10 Jan–Mar; 11 Jan 1–7, last hour | `simulation` block; output scripts | conforming |
| G10 | Weather data | Annex A1; per section | The named normative file | Normative files converted field for field (W1–W5); hash-pinned | `openbse-weather::read_tmy3` | fixed for 10, 11 (#530) |

| # | Cases | Normative file | Case file | Field check | Status |
|---|---|---|---|---|---|
| W1 | Section 7 | 725650TY.CSV (7.2.1.1.1.1, A1.1.1.1) | the file itself locally; in CI rebuilt from `weather/725650TYCST.epw` (`reconstruct_140_weather.py`) | Rebuild identical in every column the engine reads (GHI, DNI, DHI, opaque sky cover, dry-bulb, dew point, RH, pressure, wind); total sky cover not rebuilt (not read) | conforming (equivalent in CI) |
| W2 | CE100–CE200 | CE100A, CE110A, CE165A, CE200A .TM2 (9.2.1.1, 9.2.2) | `cases/CE1xxA.csv` (`convert_tm2_weather.py`) | identical | conforming |
| W3 | CE300–CE545 | CE300.TM2 (9.2.3.1) | `cases/CE300.csv` | identical | conforming |
| W4 | HE100–HE230 | HE100W, HE120W, HE130W, HE140W, HE210W .WY2 (10.2.1.1, 10.2.2.2–10.2.2.4, 10.2.3.1.1) | `weather/HE*W.csv` (`convert_wy2_weather.py`) | identical; RH from dew point (WYEC2 has none); timestamps by record order (record 40 is mislabelled in the files). Was: BLAST-derived EPWs for HE100–HE140 (pressure 101,730 vs 101,330 Pa, RH 50% constant) | fixed (#530) |
| W5 | AE101–AE445 | AE101, AE103–AE106 .TM2 (11.2.1.1.7; Table 11-4 ff.) | `weather/AE10x.csv` (`convert_tm2_weather.py`) | identical. Was: EPWs from the reference inputs' wet-bulb (dew point 0.2–5.8 K off, pressure 101,325 Pa) | fixed (#530) |

The engine takes the outdoor humidity ratio from the dew point and pressure, as EnergyPlus does (`WeatherHour::humidity_ratio`; fixed, #523). It used to take it from the RH column, which TMY files hold as whole percent; the change moved CE and AE coil loads and humidities by ≤ 0.2%.

## Section 7 — Building thermal envelope and fabric load tests

Shared engine paths: conduction by CTF from layered constructions (`ctf.rs`, `try_calculate_ctf`); exterior convection DOE-2 (TARP natural + MoWiTT forced, roughness-scaled; `convection.rs`), wind at surface height from the weather-station speed with the `country` terrain profile; exterior long-wave to sky (`sky_temperature`, Clark–Allen with opaque sky cover), air and ground at outdoor dry-bulb (`heat_balance.rs`); interior convection TARP, interior long-wave grey exchange; windows by layer (`glazing.rs`, angular optics from the pane data); beam solar projected geometrically onto interior surfaces, diffuse by area × absorptance; shading by polygon clipping (`shading.rs`); ideal loads hold the set point exactly, 100% convective (`zone.rs`); infiltration and ventilation mass flow at outdoor density from the weather pressure (`infiltration.rs`).

### Case 600 (base case; all other cases inherit these lines unless listed)

| # | Requirement | Clause | Specified | OpenBSE input | Status |
|---|---|---|---|---|---|
| 600-1 | Weather | 7.2.1.1.1.1 | 725650TY.CSV | that file (W1) | conforming |
| 600-2 | Site | 7.2.1.1.2.1 | Table A1-1 latitude, longitude, altitude, time zone | read from the file header | conforming |
| 600-3 | Terrain | 7.2.1.1.2.2, 7.2.1.9.1 | Open terrain, station height 10 m | `terrain: country` | conforming |
| 600-4 | Ground reflectance | 7.2.1.1.2.5 | 0.2 | engine default 0.2 (`HeatBalance::new`) | conforming |
| 600-5 | Geometry | 7.2.1.3, Figure 7-1 | 8 × 6 × 2.7 m, two 3 × 2 m south windows, sill 0.2 m, 0.5 m from walls | vertices as specified | conforming |
| 600-6 | Wall layers | 7.2.1.4, Table 7-2 | wood siding 0.009 / fiberglass 0.066 / plasterboard 0.012 (k, ρ, cp as table) | layered construction | fixed (#525) |
| 600-7 | Roof layers | 7.2.1.4, Table 7-2 | roof deck 0.019 / fiberglass 0.1118 / plasterboard 0.010 | layered | fixed (#525) |
| 600-8 | Raised floor layers | 7.2.1.4, Table 7-2; 7.2.1.5.2 | insulation R 25.075 at minimum ρ·cp / timber 0.025 | massless R 25.075 + timber | fixed (#525) |
| 600-9 | No internal mass | 7.2.1.4.1 | none | none | conforming |
| 600-10 | Raised floor exposure | 7.2.1.5.1 | air below = outdoor air; no solar; zero wind | `boundary: outdoor`, `sun_exposure: false`, `wind_exposure: false` | conforming |
| 600-11 | Infiltration | 7.2.1.6, 7.2.1.6.1 | 0.5 ach constant; program corrects for altitude → 0.5 ach | 0.5 ach; mass flow at weather-file pressure | conforming |
| 600-12 | Internal gains | 7.2.1.7 | 200 W continuous, 100% sensible, 60% radiative | 200 W, radiant 0.6, latent 0 | conforming |
| 600-13 | Opaque radiative properties | 7.2.1.8.1, Table 7-5 | solar 0.6 / IR 0.9 both faces; floor underside no solar | every material 0.6 / 0.9 | conforming |
| 600-14 | Ground temperature for exterior IR | 7.2.1.8.2.2.1 | = outdoor dry-bulb | `t_gnd = t_outdoor` | conforming |
| 600-15 | Exterior texture | 7.2.1.9.2, Table 7-6 | walls, roof, floor rough; glass very smooth | outer layers `rough`; windows `VerySmooth` | fixed (#525; floor was default) |
| 600-16 | Exterior surface coefficients | 7.2.1.9.3a | Program calculates time-step convection and IR → use them | DOE-2 convection, explicit IR | conforming |
| 600-17 | Interior surface coefficients | 7.2.1.10.3a | Program calculates → use them; buoyancy regime | TARP natural, grey IR exchange | conforming |
| 600-18 | Window glazing | 7.2.1.11.2, Table 7-10 | 2 panes 3.048 mm, k 1.0, τ 0.834, ρ 0.075, IR ε 0.84 (0.9 allowed), 12.0 mm air gap, no frame | as specified | fixed (#526; pane was 3.175 mm) |
| 600-19 | Angular optics | 7.2.1.11.3a | Program calculates from Table 7-10 → skip Table 7-12 | computed | conforming |
| 600-20 | Interior solar distribution | 7.2.1.12.1 | Program calculates → ignore Table 7-13 | geometric beam projection (`full_interior_and_exterior`) | fixed (#527) |
| 600-21 | Mechanical system | 7.2.1.13 | 100% convective, air-temperature thermostat, nonproportional, no latent | ideal loads | conforming |
| 600-22 | Thermostat | 7.2.1.13.1.1 | heat < 20 °C, cool > 27 °C | 20 / 27 | conforming |
| 600-23 | Capacities | 7.2.1.13.2 | 1000 kW each, 100% efficient, no fan heat | 1 000 000 W each | conforming |
| 600-24 | Time step | 5.1.1.4 | Same in all cases | 4 per hour | fixed (#527) |
| 600-25 | Annual heating, sensible cooling (MWh) | 7.3.1.1–7.3.1.2 | Annual totals | summary `Heating:` / `Cooling:` (integrated zone rates) | conforming |
| 600-26 | Annual peaks | 7.3.1.3–7.3.1.4 | Hourly integrated peak (kW) with month/day/hour | from hourly zone results (hour-averaged rates) | fixed (#533; was single time step) |
| 600-27 | Monthly loads and peaks (600, 900) | 7.3.2.4 | Monthly totals and hourly integrated peaks | summary monthly table; hourly zone results | conforming |
| 600-28 | Incident and transmitted solar, sky temperature | 7.3.2.1–7.3.2.3, 7.3.3–7.3.5 | Annual incident unshaded solar (horizontal, N, E, S, W); transmitted solar per window area (600, 660, 670 south; 620 west; 610, 630 shaded); sky temperature mean, min and max (the program applies one) | `surface:incident_solar`, `surface:transmitted_solar` (optical transmission; absorbed-and-conducted heat excluded) and `site:sky_temperature` in each case's `solar_results.csv`; entered by `generate_ashrae140_submission.py` | conforming (fixed, v0.8.0) |
| 600-29 | Daily hourly output | 7.3.8, Table 7-47 | Hourly integrated incident and transmitted solar, sky temperature, loads (heating +, cooling −), conditioned and free-float temperatures on Feb 1, May 4, Jul 14 from annual runs | hourly results of the annual runs; entered by `generate_ashrae140_submission.py` | conforming (fixed, v0.8.0) |

### Section 7 basic-test cases

Each case is Case 600 plus the lines listed (7.2.2, Table B1-2 base cases).

| Case | Builds on | Requirement | Clause | Specified | OpenBSE input | Status |
|---|---|---|---|---|---|---|
| 610 | 600 | South overhang | 7.2.2.1.1, Figure 7-3 | 1.0 m deep at roof level, full wall length; absorptance 1, IR ε 0, both sides shade, minimal thickness | `shading_surfaces` 0–8 m × 1.0 m at z 2.7; engine shading surfaces are opaque, zero-thickness, and neither reflect nor emit | conforming |
| 620 | 600 | East/west windows | 7.2.2.1.2.1, Figure 7-4 | 6 m² east, 6 m² west, none south | vertices as specified | conforming |
| 630 | 620, 610 | Overhang and fins on E/W windows | 7.2.2.1.3, Figures 7-5, 7-6 | 1.0 m overhang at roof level over each window; 1.0 m fins at the window edges, full wall height; properties as 610 | window `shading`: overhang depth 1.0, 0.5 m above the window head; fins depth 1.0 from ground to eave (same geometry as the accompanying EnergyPlus input) | conforming |
| 640 | 600 | Setback | 7.2.2.1.4.1.1 | heat < 10 °C 23:00–07:00, linear ramp 07:00–08:00 reached at the end of each sub-hourly step, < 20 °C 08:00–23:00; cool > 27 °C | hours 1–7, 24 at 10 °C; hour 8 ramps 10 → 20 °C (`heating_setpoint_end`), read at the end of each 15-minute step: 12.5, 15, 17.5, 20 °C; hours 9–23 at 20 °C | conforming (fixed, #519) |
| 650 | 600 | Vent schedule | 7.2.2.1.5.1.1 | vent ON 18:00–07:00 (hours 19–24, 1–7), unconditional; heating off; cooling > 27 °C 07:00–18:00 only | hours 1–7, 19–24; no temperature condition; heating capacity 0; cooling set point 100 °C in vent hours | fixed (#528; was outdoor-cooler condition) |
| 650 | 600 | Vent rate | 7.2.2.1.5.2, Table 7-16 | 1700 m³/h, altitude-corrected programs = 13.12 ach; no fan heat | 13.12 ach at outdoor density | conforming |
| 660 | 600 | Low-e argon window | 7.2.2.1.6.1.2, Table 7-17 | outer CSR42 3.180 mm, τ 0.452, ρf 0.359, ρb 0.397, inside ε 0.047; inner clear 3.048 mm; 12.0 mm argon | as specified | conforming |
| 670 | 600 | Single pane | 7.2.2.1.7.1.2, Table 7-21 | 1 × 3.048 mm clear | as specified | conforming |
| 680 | 600 | Increased insulation | 7.2.2.1.8.1, Table 7-25 | wall foam 0.250 m (k 0.04, ρ 10, cp 1400); roof fiberglass 0.400 m; floor as 600 | layered | fixed (#525) |
| 685 | 600 | "20,20" thermostat | 7.2.2.1.9.1.1 | heat < 20, cool > 20 | 20 / 20 | conforming |
| 695 | 680, 685 | Insulation + 20,20 | 7.2.2.1.10 | as 680 with 20 / 20 | as specified | conforming |
| 900 | 600 | High-mass walls and floor | 7.2.2.2.1.1, Table 7-27 | wall siding 0.009 / foam 0.0615 / block 0.100 (0.51, 1400, 1000); floor insulation R 25.175 / slab 0.080 (1.13, 1400, 1000); roof as 600 | layered | fixed (#525) |
| 910 | 610, 900 | as 610, high mass | 7.2.2.2.2 | — | as specified | conforming |
| 920 | 620, 900 | as 620, high mass | 7.2.2.2.3 | — | as specified | conforming |
| 930 | 630, 900 | as 630, high mass | 7.2.2.2.4 | — | as specified | conforming |
| 940 | 640, 900 | as 640, high mass | 7.2.2.2.5 | 640 schedule | 640 schedule | fixed (#529, #519) |
| 950 | 650, 900 | as 650, high mass | 7.2.2.2.6 | 650 vent | hours 1–7, 19–24, 13.12 ach | fixed (#528; hour 19 was off) |
| 960 | 600, 900 | Two zones; common wall | 7.2.2.2.7.2, Table 7-29 | concrete 0.20 m (0.51, 1400, 1000), α 0.6, ε 0.9 | layered | fixed (#525) |
| 960 | 600, 900 | Back zone | 7.2.2.2.7.1 | as 600 with common wall for the south wall; 0.5 ach; 200 W gains | Case 600 layers, 0.5 ach, 200 W | conforming |
| 960 | 600, 900 | Sun zone | 7.2.2.2.7.3 | 2 × 8 × 2.7 m; Case 900 properties; windows raised 0.3 m; 0.5 ach; no gains | Case 900 layers, α 0.6, sills 0.5 m, 0.5 ach | fixed (#524; α was 0.75 from the accompanying EnergyPlus input) |
| 960 | 600, 900 | Sun-zone solar distribution | 7.2.2.2.7.4.1 | calculated by the program | geometric projection | conforming |
| 960 | 600, 900 | Controls; no interzone air | 7.2.2.2.7.5–6 | back zone as 600; sun zone free-floats; no air exchange | ideal loads on back zone only | conforming |
| 960 | 600, 900 | Output | 7.3.6 | sun-zone hourly max, min, mean | gate reads `Sun Zone` column | conforming |
| 980 | 900 | Increased insulation, high mass | 7.2.2.2.8.1, Table 7-31 | wall foam 0.2452 m; roof as 680; floor as 900 | layered | fixed (#525) |
| 985 | 900, 685 | 20,20 | 7.2.2.2.9 | 20 / 20 | as specified | conforming |
| 995 | 980, 685 | Insulation + 20,20 | 7.2.2.2.10 | as 980 with 20 / 20 | as specified | conforming |
| 600FF, 680FF, 900FF, 980FF | 600, 680, 900, 980 | No mechanical system | 7.2.2.3.1, .3, .4, .6 | none | no `ideal_loads` | conforming |
| 650FF, 950FF | 650, 950 | No mechanical system; vent continues | 7.2.2.3.2, 7.2.2.3.5 | vent 18:00–07:00 unconditional, 1700 m³/h | hours 1–7, 19–24, 13.12 ach, unconditional | fixed (#528; 650FF had the outdoor-cooler condition) |
| all FF | — | Free-float outputs | 7.3.6 | annual hourly integrated max, min (with date), mean zone air temperature | gate averages sub-hourly rows to the hour (`build_140_csv.py`) | conforming |
| 900FF | 900 | Temperature bins | 7.3.7 | 1 °C bins | template post-processor from hourly temperatures | conforming |

Cases 640/940: the ramp is the clause c(1) input itself. A thermostat schedule entry with an end set point ramps linearly across its period and is read at the end of each time step (`ThermostatScheduleEntry::setpoints_at`).

### Section 7.2.3 in-depth cases

Cases 195–320, 395–470, 800 and 810 (7.2.3a). Each is written by `scripts/generate_indepth_cases.py`, which also writes its `conformance_spec.json` entry. Required output is 7.3.1 only (7.2.3b): annual heating and sensible cooling, and annual hourly integrated peaks with their time, entered in rows 91–115 of the TF form by `generate_ashrae140_submission.py`. Annex A3 has no ranges for these cases and the gate adds none: `build_140_csv.py` reports their loads as INFO rows and fails CI only when a case gives no result.

The base cases are those of Informative Table 7-33, read clause by clause. Lines shared by the series come first; each case then lists its own.

| # | Requirement | Clause | Specified | OpenBSE input | Status |
|---|---|---|---|---|---|
| ID-1 | High-conductance wall elements (HCWE): placement | 7.2.3.1.4.1, Table 7-35 | replace the 12 m² of south windows: two 3 × 2 m units | two opaque `wall` subsurfaces of the south wall (`parent_surface: South Wall`) at the Case 600 window vertices; their area comes off the wall | conforming |
| ID-2 | HCWE layers | 7.2.3.1.4.2d, Table 7-36 | panel 3.048 mm (k 1.00, ρ 2470, cp 750) / air gap 12.0 mm (k 0.0625, ρ 1.292498, cp 1006.103271, constant) / panel | three massed layers with those values; engine material properties are constant | conforming (the accompanying EnergyPlus input uses a massless R 0.192 gap; the text gives the gap's density and specific heat, so they are input) |
| ID-3 | HCWE surfaces | 7.2.3.1.4.2a–c | shortwave transmittance 0; Table 7-34 absorptances and emittances (as varied by the case); very smooth, as the glass | opaque construction; outer and inner panel carry the case's exterior and interior values; `roughness: very_smooth` | conforming |
| ID-4 | HCWE surface coefficients | 7.2.3.1.4.3a, 7.2.3.1.4.4a | program calculates time-step convection and IR → apply them | DOE-2 outside convection (very smooth), TARP inside, explicit IR, as for the other walls | conforming |
| ID-5 | No infiltration, no gains (220 series, 400) | 7.2.3.1.1–2, 7.2.3.15.1–2 | 0 ach; 0 W | no `infiltration`, no `equipment` | conforming |
| ID-6 | Opaque radiative properties, Case 220 | 7.2.3.1.3, Table 7-34 | interior solar 0.6, exterior solar 0.1, IR 0.9 both faces; raised floor: interior solar and IR, exterior IR from the table, exterior gets no solar | outer layers α 0.1 / ε 0.9, inner layers α 0.6 / ε 0.9; floor insulation ε 0.9, `sun_exposure: false` (its exterior α stays Case 600's 0.6: no clause changes it and no solar reaches it; the accompanying input has 0.1) | conforming |
| ID-7 | "20,20" thermostat (195–310) | 7.2.3.1.5 | heat < 20 °C, cool > 20 °C, nonproportional | 20 / 20 | conforming |
| ID-8 | Interior solar distribution (270–320, 440, 810) | 7.2.3.9.3.1, 7.2.3.10.2.1, 7.2.3.12.2.1 | calculated by the program from the interior solar absorptance → skip Tables 7-43 to 7-45 | `full_interior_and_exterior` (beam projected, diffuse by area × absorptance, the windows' back transmittance losing solar to outdoors) | conforming |
| ID-9 | Output | 7.2.3b, 7.3.1 | non-free-float output only | annual loads from the summary, peaks from hourly zone results | conforming |

| Case | Builds on (Table 7-33) | Requirement | Clause | Specified | OpenBSE input | Status |
|---|---|---|---|---|---|---|
| 220 | 600, 685 | Base case | 7.2.3.1 | ID-1 to ID-7 | as specified | conforming |
| 210 | 220 | Interior IR | 7.2.3.2.1 | interior ε 0.1 on all opaque surfaces, HCWE included | plasterboard, timber flooring, inner panel ε 0.1 | conforming |
| 215 | 220 | Exterior IR | 7.2.3.3.1 | exterior ε 0.1 on all opaque surfaces, HCWE included | wood siding, roof deck, floor insulation, outer panel ε 0.1 | conforming |
| 200 | 210, 215 | Interior and exterior IR | 7.2.3.4.1 | 210 plus exterior ε 0.1 | interior and exterior ε 0.1 | conforming |
| 195 | 200, 600 | Solid conduction | 7.2.3.5 | HCWE replaced by the Table 7-2 wall, rough outside, smooth inside, Case 200 wall radiative properties | full Case 600 south wall, no subsurfaces; ε 0.1 both faces, α 0.1 out / 0.6 in | conforming |
| 230 | 220 | Infiltration | 7.2.3.6, Table 7-42 | 1.0 ach constant; program corrects for altitude → 1.0 | 1.0 ach, mass flow at weather-file pressure | conforming |
| 240 | 220 | Internal gains | 7.2.3.7 | 200 W, 100% sensible, 60% radiative | 200 W, radiant 0.6, latent 0 | conforming |
| 250 | 220, 600 | Exterior solar absorptance | 7.2.3.8 | 0.9 on all opaque surfaces but the raised floor, HCWE included | wood siding, roof deck, outer panel α 0.9 | conforming |
| 270 | 220, 600 | South windows | 7.2.3.9.1–2 | interior α 0.9; Case 600 windows in place of the HCWE | inner layers α 0.9; Case 600 window construction and vertices | conforming |
| 280 | 270 | Cavity albedo | 7.2.3.10.1 | interior α 0.1 | inner layers α 0.1 | conforming |
| 290 | 270, 610 | South overhang | 7.2.3.11 | Case 610 overhang across the whole south wall | `shading_surfaces` 0–8 m × 1.0 m at z 2.7 (as Case 610) | conforming |
| 300 | 270, 620 | East/west windows | 7.2.3.12.1 | 6 m² east, 6 m² west, none south, as Case 620 | Case 620 vertices | conforming |
| 310 | 300, 630 | East/west shading | 7.2.3.13 | Case 630 overhangs and fins | Case 630 window `shading` | conforming |
| 320 | 270, 600 | "20,27" thermostat | 7.2.3.14 | heat < 20 °C, cool > 27 °C | 20 / 27 | conforming |
| 400 | 600, 220 | Base of the "20,27" series | 7.2.3.15 | ID-1 to ID-5; exterior α 0.1 on all opaque surfaces but the raised floor, HCWE included; interior and IR as Case 600 | outer layers α 0.1; inner α 0.6; ε 0.9 | conforming |
| 395 | 400, 195 | Solid conduction | 7.2.3.16 | HCWE replaced by the Table 7-2 wall, Case 400 wall properties | full Case 600 south wall; α 0.1 out / 0.6 in | conforming |
| 410 | 400, 600 | Infiltration | 7.2.3.17 | 0.5 ach, altitude-corrected | 0.5 ach | conforming |
| 420 | 410, 600 | Internal gains | 7.2.3.18 | 200 W, as Case 600 | 200 W, radiant 0.6 | conforming |
| 430 | 420, 600 | Exterior solar absorptance | 7.2.3.19 | 0.6, HCWE included | outer layers α 0.6 | conforming |
| 440 | 600, 280 | Cavity albedo | 7.2.3.20 | Case 600 with interior α 0.1; solar distribution as 280 (ID-8) | inner layers α 0.1 | conforming |
| 450 | 600 | Constant combined coefficients, both faces | 7.2.3.21.1–2, Table 7-46 | interior / exterior: walls 1.8 / 21.6, roof 1.7 / 21.8, raised floor 3.7 / 5.2, windows 4.5 / 17.8 W/(m²·K); time-step varying coefficients prohibited | `inside_combined_coefficient` and `outside_combined_coefficient` on every surface with those values: each replaces that face's convection model and long-wave exchange (Engineering Reference, constant combined coefficients) | conforming (new engine input) |
| 460 | 600, 450 | Constant combined coefficients, interior | 7.2.3.22.1 | Table 7-46 interior; exterior as Case 600 | `inside_combined_coefficient` only | conforming (new engine input) |
| 470 | 600, 450 | Constant combined coefficients, exterior | 7.2.3.23.1 | Table 7-46 exterior; interior as Case 600 | `outside_combined_coefficient` only | conforming (new engine input) |
| 800 | 430, 900 | High mass, no solar gains | 7.2.3.24 | Case 430 with the Table 7-27 wall and floor; roof and HCWE as Case 430 | Case 900 wall and floor layers; Case 600 roof; HCWE | conforming |
| 810 | 900, 280 | High-mass cavity albedo | 7.2.3.25 | Case 900 with interior α 0.1; solar distribution as 280 (ID-8) | Case 900 layers, inner layers α 0.1 | conforming |

Cases 450–470: the Standard's informative note permits entering Table 7-46 as convection coefficients with the emittances set as low as the program allows, which the accompanying EnergyPlus input does (ε 10⁻⁶). OpenBSE takes the combined coefficient itself, so no emittance is altered and the radiant gains still land on the surfaces by their emissivities, which keep their values. Annex A2 Block B of the Section 7 notes describes the option and where it is used.

## Section 9.2.1–9.2.2 — Space-cooling equipment, analytical verification (CE100–CE200)

Engine paths: DX coil (`cooling_coil.rs`) with capacity and EIR tables on entering wet-bulb × outdoor dry-bulb, apparatus-dew-point/bypass sensible split, dry-coil wet-bulb iteration; part-load degradation applied at system level (`cooling_part_load_cd`); on/off draw-through fan (`fan.rs`) with all fan heat to the supply air; zone moisture balance (`zone.rs`).

### CE100 (base case)

| # | Requirement | Clause | Specified | OpenBSE input | Status |
|---|---|---|---|---|---|
| CE100-1 | Weather | 9.2.1.1 | CE100A.TM2 (or CE100.TMY) | `CE100A.csv` (W2) | conforming |
| CE100-2 | Geometry | 9.2.1.3 | 48 m² floor, 129.6 m³ | stated volume and area; 8 × 6 × 2.7 m surfaces | conforming |
| CE100-3 | Envelope | 9.2.1.4, Table 9-1 | near-adiabatic, U 0.010 W/(m²·K) surface to surface, UA 1.713 W/K; no capacitance | U 0.01, heat capacity 0 | conforming |
| CE100-4 | Infiltration | 9.2.1.5 | 0.0 ach | none | conforming |
| CE100-5 | Internal gains | 9.2.1.6 | 5400 W sensible, 0 latent, 100% convective | 5400 W, radiant 0 | conforming |
| CE100-6 | Surface radiative properties | 9.2.1.7 | exterior solar 0.1, IR 0.9; interior 0.6 / 0.9 | 0.1 / 0.9 | conforming |
| CE100-7 | Surface coefficients | 9.2.1.8–9.2.1.9 | program calculates → use them | DOE-2 / TARP | conforming |
| CE100-8 | System | 9.2.1.10.1 | split DX, no outdoor air, fans cycle with compressor, condensate drained | air loop, `minimum_damper_position: 0`, on/off fan | conforming |
| CE100-9 | Thermostat | 9.2.1.10.2 | cool > 22.2 °C, heat off, no humidity control, nonproportional, ideal | cooling 22.2 °C; heating set point 10 °C (never reached: gains always positive) | conforming |
| CE100-10 | Performance data | 9.2.1.10.3 | Tables 9-6 (gross, SI); wet-coil data; dry coil where sensible ≥ total | CapFT/EIRFT tables from CE100MAP gross data on EWB × ODB, rated 8181.3 W, COP 4.3986 | equivalent: the map's EDB dependence enters through the ADP/bypass sensible model (5.1.1.5) |
| CE100-11 | Extrapolation | 9.2.1.10.3.1 | program extrapolates as needed | linear extrapolation of the tables | conforming |
| CE100-12 | Part load | 9.2.1.10.4 | CDF = 1 − 0.229 (1 − PLR); run time = PLR / CDF | `cooling_part_load_cd: 0.229` | conforming |
| CE100-13 | Indoor fan | 9.2.1.10.6.1 | 230 W, 0.425 m³/s, η 0.5, 271 Pa, all heat to air, draw-through, cycles | 0.425 m³/s, 271 Pa, η 0.5, in airstream | conforming |
| CE100-14 | Outdoor fan | 9.2.1.10.6.2 | 108 W, cycles with compressor | `condenser_fan_power: 108` | conforming |
| CE100-15 | Duration | 5.1.1.8.3 | ≥ Jan–Feb; report Feb | Jan–Mar; Feb extracted | conforming |
| CE100-16 | Energy outputs | 9.3.1.1 | Feb compressor, indoor fan, outdoor fan, total (kWh) | `generate_ashrae140_CE_submission.py` from hourly component powers | conforming |
| CE100-17 | Coil and zone loads | 9.3.1.2–9.3.1.3 | total/sensible/latent coil loads; zone loads (coil minus fan heat) | hourly coil loads; zone sensible = coil sensible − supply-fan heat | conforming |
| CE100-18 | Means, extremes | 9.3.1.4 | COP = net refrigeration effect / total energy; IDB; zone W; hourly max/min | as defined | conforming |
| CE100-19 | Outdoor humidity | 9.2.1.1, A1.2 | weather dew point / RH | humidity ratio from the dew point | conforming (fixed, #523) |

### CE110–CE200

| Case | Builds on | Weather | Cooling set point (°C) | Sensible / latent gains (W) | Clause | Status |
|---|---|---|---|---|---|---|
| CE110 | CE100 | CE110A | 22.2 | 5400 / 0 | 9.2.2.1.1 | conforming |
| CE120 | CE110 | CE110A | 26.7 | 5400 / 0 | 9.2.2.1.2 | conforming |
| CE130 | CE100 | CE100A | 22.2 | 270 / 0 | 9.2.2.1.3 | conforming |
| CE140 | CE130 | CE110A | 22.2 | 270 / 0 | 9.2.2.1.4 | conforming |
| CE150 | CE110 | CE110A | 22.2 | 5400 / 1100 | 9.2.2.2.1 | conforming |
| CE160 | CE150 | CE110A | 26.7 | 5400 / 1100 | 9.2.2.2.2 | conforming |
| CE165 | CE160 | CE165A | 23.3 | 5400 / 1100 | 9.2.2.2.3 | conforming |
| CE170 | CE150 | CE110A | 22.2 | 2100 / 1100 | 9.2.2.2.4 | conforming |
| CE180 | CE170 | CE110A | 22.2 | 2100 / 4400 | 9.2.2.2.5 | conforming |
| CE185 | CE180 | CE100A | 22.2 | 2100 / 4400 | 9.2.2.2.6 | conforming |
| CE190 | CE180 | CE110A | 22.2 | 270 / 550 | 9.2.2.2.7 | conforming |
| CE195 | CE190 | CE100A | 22.2 | 270 / 550 | 9.2.2.2.8 | conforming |
| CE200 | CE150 | CE200A | 26.7 | 6120 / 1817 | 9.2.2.2.9 | conforming |

Latent gains enter the zone moisture balance with h_fg = 2501 kJ/kg; the clause lets the modeller pick the h_fg used for condensation at the coil (9.2.2.2.1.1), so this is conforming.

## Section 9.2.3–9.2.4 — Space-cooling equipment, comparative (CE300–CE545)

### CE300 (base case)

| # | Requirement | Clause | Specified | OpenBSE input | Status |
|---|---|---|---|---|---|
| CE300-1 | Weather | 9.2.3.1 | CE300.TM2 | `CE300.csv` (W3) | conforming |
| CE300-2 | Building | 9.2.3.3–9.2.3.4 | 14 × 14 × 3 m, 588 m³, near-adiabatic, no capacitance | as specified, R 325 m²·K/W massless | conforming |
| CE300-3 | Infiltration | 9.2.3.5 | 0.0 ach whole simulation | 0 (except CE530–CE545, below) | conforming |
| CE300-4 | Internal gains | 9.2.3.6 | scheduled sensible (convective) and latent per case | seasonal schedules from the generator | conforming |
| CE300-5 | Outdoor air | 9.2.3.10 | 1.734 ach (15%) scheduled | 0.2832 m³/s, minimum OA schedule | conforming |
| CE300-6 | Thermostat | 9.2.3.10 | cool > 25 °C, heat off | 25 °C; heating set point far below | conforming |
| CE300-7 | Performance map | 9.2.3.10, CE300MAP | expanded map (EWB × ODB × EDB) | tables of the map on EWB × ODB (`generate_ce_b_cases.py`) | equivalent (as CE100-10) |
| CE300-8 | Part load | 9.2.3.10 | CDF = 1 − 0.229 (1 − PLR) | PLF curve 0.771 + 0.229 PLR | conforming |
| CE300-9 | Fans | 9.2.3.10 | supply fan continuous (cycles from CE500), rated at standard density; condenser fan cycles | fan `air_density` standard; condenser fan with compressor | conforming |
| CE300-10 | Outputs | 9.3.2 | annual sums, means, hourly maxima/minima (extremes after Mar 31 where stated) | `ce_b_results.py` | conforming |
| CE300-11 | Outdoor humidity | 9.2.3.1 | weather dew point / RH | humidity ratio from the dew point | conforming (fixed, #523) |

### CE310–CE545

| Case | Builds on | Change (9.2.3, 9.2.4) | OpenBSE input | Status |
|---|---|---|---|---|
| CE310 | CE300 | high latent gains | latent peak 9379 W | conforming |
| CE320 | CE300 | infiltration 11.558 ach scheduled, no OA | 1.888 m³/s infiltration, OA 0 | conforming |
| CE330 | CE300 | OA 11.558 ach scheduled | 1.888 m³/s OA | conforming |
| CE340 | CE300 | infiltration and OA 5.779 ach each | 0.944 m³/s each | conforming |
| CE350 | CE300 | set-up to 35 °C on schedule | set-up schedule | conforming |
| CE360 | CE300 | high sensible gains (undersized) | sensible peak 29,310 W | conforming |
| CE400 | CE300 | dry-bulb economizer | `differential_dry_bulb` | conforming |
| CE410 | CE400 | compressor lockout | `compressor_lockout` | conforming |
| CE420 | CE400 | 20 °C ODB limit | `max_outdoor_temp: 20` | conforming |
| CE430 | CE300 | enthalpy economizer | `differential_enthalpy` | conforming |
| CE440 | CE430 | outdoor enthalpy limit | `max_outdoor_enthalpy: 47250` | conforming |
| CE500 | — | 0% OA, mid gains, fan cycles, 25 °C | as specified | conforming |
| CE510 | CE500 | high PLR gains | 21,103 / 8573 W peaks | conforming |
| CE520, CE522, CE525 | CE500 | set point 15, 20, 35 °C | as specified | conforming |
| CE530 | CE500 | dry coil (latent 0); infiltration 0.0 ach for the entire simulation (9.2.3.5) | latent 0; no infiltration | conforming (fixed, #521; was 1 m³/s Jan 1–7 from the reference EnergyPlus input) |
| CE540, CE545 | CE530 | set point 15, 35 °C | as CE530 | conforming (fixed, #521) |

CE530–CE545: no infiltration, as Section 9.2.3.5 requires. The first week is repeated as preconditioning (5.1.1.7.2), and the zone air starts at the first hour's outdoor conditions (the Class II rule of 5.2.1.6; Class I specifies no start state). With no moisture source or sink, a dry-coil zone keeps that humidity all year: 0.0029 kg/kg from CE300's 1 January 01:00. The Standard's reference EnergyPlus input instead adds first-week January infiltration, contrary to 9.2.3.5. The example programs' humidity and energy results therefore reflect other start states, and the gate reports these cases' energy, COP and humidity without gating them (`ce_b_informative`). Coil loads and zone temperatures stay gated.

## Section 10 — Space-heating equipment (HE100–HE230)

Engine paths: fuel-fired heating coil (`heating_coil.rs`): fuel = output / efficiency, `parasitic_electric_load` for the draft fan (electric, not added to the air); on/off loop control; infiltration at zone-air density (`infiltration.rs`).

### HE100 (base case)

| # | Requirement | Clause | Specified | OpenBSE input | Status |
|---|---|---|---|---|---|
| HE100-1 | Weather | 10.2.1.1 | HE100W.WY2 | `HE100W.csv` (W4) | fixed (#530) |
| HE100-2 | Geometry | 10.2.1.3 | 48 m², 129.6 m³ | as specified | conforming |
| HE100-3 | Heat-loss method | 10.2.1.7, 10.2.1.9 | programs without user-set convection coefficients use the infiltration method instead of the roof-coefficient method, complying with all of 10.2.1 except 10.2.1.4.1, 10.2.1.5, 10.2.1.7 and 10.2.1.8 | the engine has no user-set coefficients; infiltration method, envelope per Table 10-2 | conforming |
| HE100-4 | Envelope | 10.2.1.9.2, Table 10-2 | wall 75.6 m², floor and roof 48.0 m², k 0.01 W/(m·K), t 1.00 m, U 0.01 W/(m²·K) face to face; zone above ground (10.2.1.7.1) | all six surfaces exposed to outdoors, one resistance-only layer R = 100 m²·K/W (no capacitance is specified) | conforming (fixed; was adiabatic surfaces, "equivalent", which 10.2.1.9.2 does not permit) |
| HE100-5 | Infiltration | 10.2.1.9.3 | ≈ 0.2 kg/s; total effective UA, including residual flow through the envelope, 199.953 W/K | infiltration UA 199.953 − 1.713 (Table 10-2 envelope with films) = 198.240 W/K, as mass flow / cp(W), volume for each file's mean pressure and humidity at 20 °C | fixed (#530; envelope share subtracted with HE100-4) |
| HE100-6 | Internal gains | 10.2.1.6 | none | none | conforming |
| HE100-7 | System | 10.2.1.10.1 | 100% convective, sealed combustion, no pilot, no duct loss, fan heat to air, draft heat to flue | gas coil to supply air; draft fan as parasitic electric | conforming |
| HE100-8 | Thermostat | 10.2.1.10.2 | heat < 20 °C, ideal, nonproportional | 20 °C | conforming |
| HE100-9 | Capacity, efficiency | 10.2.1.10.3 | 10 kW, 100% | 10 000 W, 1.0 | conforming |
| HE100-10 | Part-load curve | 10.2.1.10.4 | PLF = PLR / HIR(PLR), HIR coefficients a–d | `hir_curve` with the Standard's coefficients; fuel = full-load fuel × HIR at the timestep's part load | conforming (fixed, #520) |
| HE100-11 | HHV | 10.2.1.10.5 | 38 MJ/m³ at STP | `GAS_HHV_J_PER_M3 = 38e6` (submission script) | conforming |
| HE100-12 | Fans | 10.2.1.10.6 | 0 W | none | conforming |
| HE100-13 | Duration and outputs | 5.1.1.8.4, 10.3 | Jan–Mar; load, input, fuel, fan energy, zone temperatures | Jan–Mar; `generate_ashrae140_HE_submission.py` | conforming |
| HE100-14 | Outdoor humidity | 10.2.1.1 | WYEC2 dew point | RH computed from dew point in conversion | conforming |

### HE110–HE230

| Case | Builds on | Change | Clause | OpenBSE input | Status |
|---|---|---|---|---|---|
| HE110 | HE100 | efficiency 80% | 10.2.2.1 | 0.80 | conforming |
| HE120 | HE110 | HE120W weather (part load) | 10.2.2.2 | `HE120W.csv` | fixed (#530) |
| HE130 | HE110 | HE130W weather (no load) | 10.2.2.3 | `HE130W.csv` | fixed (#530) |
| HE140 | HE110 | HE140W weather (varying load) | 10.2.2.4 | `HE140W.csv` | fixed (#530) |
| HE150 | HE140 | circulating fan 200 W, 0.355 m³/s, continuous, heat to air | 10.2.2.5 | 0.355 m³/s, 281.7 Pa / η 0.5 = 200 W at fixed 1.204 kg/m³, continuous | fixed (#532; was 0.30 m³/s on a 0.5 kg/s loop) |
| HE160 | HE150 | fan cycles with burner | 10.2.2.6 | `fan_operating_mode: cycling` | conforming |
| HE170 | HE150 | draft fan 50 W, cycles, heat to flue | 10.2.2.7 | `parasitic_electric_load: 50` | conforming |
| HE210 | HE170 | HE210W weather; circulating fan cycles | 10.2.3.1 | `HE210W.csv`; cycling fan | conforming (weather unchanged in content) |
| HE220 | HE210 | 15 °C for 23h00 < t < 6h00, 20 °C otherwise | 10.2.3.2, Figure 10-5 | 15 °C hours 24 and 1–6; 20 °C hours 7–23 | fixed (#531; hour 24 was 20 °C) |
| HE230 | HE220 | 5 kW furnace | 10.2.3.3 | 5000 W | conforming |

## Section 11 — Air-side HVAC equipment (AE101–AE445)

Engine paths: single-zone and multizone air loops (`openbse-airloop`), electric heating and hydronic cooling coils sized above load, single-zone SAT reset, VAV/CV reheat terminals (`vav_box.rs`), economizers, supply/return fans (`fan.rs`).

### AE101 (base case) and series requirements

| # | Requirement | Clause | Specified | OpenBSE input | Status |
|---|---|---|---|---|---|
| AE-1 | Ambient conditions | 11.2.1.1.7, Table 11-4 | constant conditions, dew point exact; programs without constant-condition input apply AE101.TM2 | `AE101.csv` (W5) | fixed (#530) |
| AE-2 | Zone loads | 11.2.1.1.4, Table 11-2 | sensible heating 2931 W, latent 586.1 W at the zone temperature; adiabatic zone with negative gains allowed | −2931 W and 586.1 W gains, adiabatic surfaces | conforming |
| AE-3 | Zone temperature | 11.2.1.1.6 | 21.111 °C, nonproportional | heating = cooling set point 21.111 °C | conforming |
| AE-4 | Supply airflow | 11.2.1.1.2.1, Table 11-1 | 283.17 L/s at fan inlet, constant | 0.28317 m³/s | conforming |
| AE-5 | Exhaust and outdoor air | 11.2.1.1.2.2–3 | exhaust 94.39 L/s at zone conditions; OA mass = exhaust mass, or the program's own OA-replaces-exhaust assumption | minimum OA fraction 1/3 of supply volume | equivalent (permitted alternative in 11.2.1.1.2.3) |
| AE-6 | Supply fan | 11.2.1.1.1.1 | 498 Pa, mechanical efficiency 0.7, fan heat to air; or combined 0.7 in the airstream | 498 Pa, impeller 0.7, motor 1.0 in airstream | equivalent (permitted alternative) |
| AE-7 | Frictionless, no leakage | 11.2.1.1.2.4–5 | none | none modelled | conforming |
| AE-8 | Coils | 11.2.1.1.3 | modulate to the exact supply temperature, capacity ≥ load, cooling BF = 0 (leaving air saturated when wet) | coils sized above load; ideal leaving condition | conforming |
| AE-9 | Duration | 5.1.1.8.5 | until steady | Jan 1–7, last hour reported | conforming |
| AE-10 | Outputs | 11.3 | coil loads (kWh/h) and node states | `generate_ashrae140_AE_submission.py`; node-state outdoor W from the weather files as the engine computes it (#530) | conforming |
| AE-11 | Outdoor humidity | Table 11-4 note | dew point exact (RH equivalent, < 0.3% effect) | humidity ratio from the dew point | conforming (fixed, #523) |

### AE cases

| Case | System (11.2.1–11.2.4) | Ambient file | Zone loads and temperatures (Tables 11-2 ff.) | Economizer | Status |
|---|---|---|---|---|---|
| AE101, AE103, AE104 | Four-pipe fan coil | AE101, AE103, AE104 | −2931 / 21.111; 1465 / 23.333; 2931 / 23.889 W / °C; latent 586.1 W | none | conforming (weather fixed #530) |
| AE201, AE203–AE206 | Single-zone, supply and return fans | AE101, AE103, AE104, AE105, AE106 | as AE101–AE104; AE205, AE206 1465 W / 23.333 °C | none | conforming (weather fixed #530) |
| AE226, AE245 | Single-zone | AE106, AE105 | 1465 W / 23.333 °C | differential dry-bulb; differential enthalpy | conforming |
| AE301, AE303–AE306, AE326, AE345 | Two-zone CV reheat | AE101 … AE106 | zone 1 / zone 2: −2931/21.111 & −2345/22.222 (301); 1465/23.333 & 2345/24.444 (303, 305, 306, 326, 345); 2931/23.889 & 3517/25.0 (304); latent 586.1 / 879.2 W | none / dry-bulb / enthalpy | conforming (weather fixed #530) |
| AE401, AE403–AE406, AE426, AE445 | Two-zone VAV reheat | as 300 series | as 300 series | as 300 series | conforming (weather fixed #530) |

## Cases and sections not run

| Section / cases | Reason | Effect |
|---|---|---|
| 6 Weather drivers (WD100–WD600) | Not implemented in the suite | Solar and sky models are tested only through Section 7 results |
| 8 Ground-coupled slab-on-grade (GC) | Not implemented | Ground model not tested by 140 |
| 12 Class II tests | Not implemented | — |

## Submission package

Normative Annex A2 forms, in `results/`, generated from the case results by `scripts/generate_ashrae140_*submission.py` and `scripts/generate_ashrae140_notes.py` (templates from the Standard's accompanying files):

| Section | Output form | Modeling notes | Annex A3 test group |
|---|---|---|---|
| 7 | `OpenBSE_Std140_TF_Output.xlsx` | `OpenBSE_S140outNotes_7.txt` | Thermal fabric, low and high mass |
| 9.2.1–9.2.2 | `OpenBSE_Std140_CE_a_Output.xls` | `OpenBSE_S140outNotes_9A.txt` | — (no ranges) |
| 9.2.3–9.2.4 | `OpenBSE_Std140_CE_b_Output.xls` | `OpenBSE_S140outNotes_9B.txt` | Cooling equipment |
| 10 | `OpenBSE_Std140_HE_Output.xls` | `OpenBSE_S140outNotes_10.txt` | Heating equipment |
| 11 | `OpenBSE_Std140_AE_Output.xlsx` | `OpenBSE_S140outNotes_11.txt` | Air-side equipment |

The Section 7 notes omit no case (Block E: none); the in-depth results fill rows 91–115 of the TF form's conditioned-loads block. The sections in [Cases and sections not run](#cases-and-sections-not-run) have no forms.

## CI guards

- **Specified inputs:** `crates/openbse-io/tests/ashrae140_conformance.rs` checks 4,442 values in `conformance_spec.json` against each parsed case (constructions and every material property, glazing, gains, infiltration, set points and schedules, capacities, fans, ventilation, weather file path, time step and solar-distribution method), and fails when a case has no entry.
- **Weather:** `.github/workflows/ci.yml` pins the SHA-256 of every weather file the cases read, including the Section 7 file as rebuilt in CI.
- **Acceptance ranges:** pinned separately (existing).

The record and the spec table must change together when a case input changes.
