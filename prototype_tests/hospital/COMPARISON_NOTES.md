# Hospital (DOE 90.1-2022, Denver) — comparison with EnergyPlus

Model: `Hospital_STD2022_Denver.yaml`, generated from PNNL's IDF by
`hospital_from_idf.py` (inputs: `INPUT_AUDIT.md`). Reference: the same IDF run
with E+ 22.1 on the same weather file
(`weather/USA_CO_Denver-Aurora-Buckley.AFB.724695_TMY3.epw`), which reproduces
PNNL's published table exactly (`eplus_run/eplustbl.csv`). Component traces
come from a rerun of that IDF with hourly `Output:Variable`s (boilers, chillers,
coils, humidifiers, wheels, fans, pumps, zone heat balance), outside the repo.

The method followed `eplus-comparison-method`: inputs first, then gains, zone
loads, and the system.

## End uses

With daylight saving, the final model, E+ 22.1 base run:

| End use | E+ [MWh] | OpenBSE [MWh] | Difference | Note |
|---|---:|---:|---:|---|
| Heating (Gas) | 665.7 | 566.3 | -14.9 % | hot-water loop load +1.7 %; split between boiler and heat-recovery chiller (System) |
| Cooling (Electric) | 411.9 | 373.4 | -9.3 % | chilled water −1.3 %; heat-recovery chiller control and chiller condenser temperature (System) |
| Interior Lighting | 649.9 | 651.8 | +0.3 % |  |
| Exterior Lighting | 34.6 | 34.6 | -0.1 % |  |
| Interior Equipment | 1,914.7 | 1,914.7 | +0.0 % |  |
| Interior Equipment (Gas) | 775.5 | 775.5 | +0.0 % |  |
| Exterior Equipment | 169.1 | 149.5 | -11.6 % | E+ transformer losses, 19.6 MWh (#508) |
| Fans (Electric) | 894.4 | 860.3 | -3.8 % |  |
| Pumps (Electric) | 65.4 | 54.4 | -16.8 % | primary pumps +28, tower pump −39 MWh (#511) |
| Heat Rejection | 11.2 | 25.2 | +124.0 % | tower model (#252) |
| Humidification | 378.9 | 408.5 | +7.8 % | per system below |
| Heat Recovery | 47.7 | 45.9 | -4.0 % |  |
| DHW (Electric) | 7.3 | 7.3 | +0.0 % |  |
| DHW (Gas) | 420.9 | 420.9 | +0.0 % |  |
| Refrigeration | 29.6 | 0.0 | -100.0 % | no refrigeration model (#509) |
| **Total** | **6,476.9** | **6,288.2** | **-2.9 %** | |

## Gains and zone loads

- **Gains.** Lighting, plug and gas equipment, DHW and exterior lighting match
  to 0.3 % or better: the schedules, densities and multiplied-zone scaling are
  right.
- **Zone design loads** (`Zone Sensible Cooling/Heating` in E+'s HVAC Sizing
  Summary, which includes zone multipliers): cooling 896 vs 894 kW (+0.3 %),
  every zone within ±3 % but three small offices (−8 to −13 %); heating 178 vs
  192 kW (−7 %), most zones −3 %, top-floor zones −6 to −9 %, the kitchen −30 %
  (its 3.4 m³/s exhaust; E+ sizes heating with it). Open: the heating design
  load of roof zones.
- **Zone design flows** match E+'s `User Design Air Flow` once the
  `DesignDayWithLimit` air-change minimums are expressed (#484).

## System

- **Reheat** — the whole hot-water coil load, because the central heating
  coils are sized to zero — is 1,206 MWh against E+'s 1,200 (+0.5 %). Before
  the engine fixes it was 60 MWh of gas in all; the causes, in order of size:
  air-change minimums not expressible (#484), operating-room setback schedule
  (#485), constant-volume reheat zones reheated to the cooling setpoint on a
  fixed cold deck (#493: ICU_Flr_2 held at 23.95 °C where E+ holds 21.00 °C),
  terminal reheat sized from 12.8 °C instead of E+'s −15 °C central heating
  design temperature (#494, the labs on 100 % outdoor air at −20 °C).
- **Hot-water loop load** = boiler + heat-recovery chiller: OpenBSE 487 + 719 =
  1,206 MWh; E+ 561 + 631 = 1,192 MWh (+1.2 %). The gas gap is the split, not
  the load: OpenBSE's heating-led chiller (#486) puts all its condenser heat
  into the hot-water loop, while E+'s EMS approximation (setpoint lowered by
  8.33 K × the rated-COP estimate of the demand) runs the chiller harder
  (617 vs 546 MWh of cooling) yet sends 183 MWh of its condenser heat to the
  tower while the boiler fires. With E+'s recovered heat, OpenBSE's boiler would
  deliver ~1,206 − 631 = 575 MWh, about 669 MWh of gas at its 0.86 seasonal
  efficiency (E+ 0.84): +0.5 %.
- **Boiler efficiency** follows the IDF's EMS curve (tabulated): seasonal 0.860
  vs E+'s 0.842; the boiler runs at lower part load in OpenBSE because the
  chiller takes more of the load.
- **Cooling.** Chilled-water production matches (OpenBSE chillers 1 + 2 +
  heat-recovery chiller 2,204 MWh, E+ 2,232). Electricity: the heat-recovery
  chiller 171 vs 194 MWh (less cooling, above); chillers 1 and 2 202 vs
  218 MWh (COP 8.2 vs 7.4): OpenBSE's water-cooled chillers read the condenser
  temperature from the wet-bulb plus an approach rather than the tower loop's
  leaving water, which E+ holds at 21.1 °C (#251).
- **Humidification.** Zone humidity ratios in the five humidistat zones now
  match E+ (0.0057 vs 0.0055–0.0057 kg/kg medians) with #487 (rated capacity)
  and the supply setpoint of E+'s `SetpointManager:MultiZone:Humidity:Minimum`
  (#503). Per system: ER 49.6 vs 47.4, OR 90.1 vs 69.3,
  ICU 94.6 vs 90.7, patient rooms 82.0 vs 100.4, labs 92.0 vs 71.1 MWh. The
  labs: E+ holds their supply near its 0.005 floor (mean 0.00509 kg/kg while
  humidifying) where OpenBSE holds 0.00558; open.
- **Wheels** (`Heat Recovery` end use = parasitic power while turning): 45.8 vs
  47.7 MWh. Frost control and the 75 %-flow effectivenesses are not modelled.
- **Fans.** Per air handler 3–11 % below E+, the kitchen unit and exhaust fans
  close. E+ computes fan power with the standard density at the site (20 °C)
  whatever the air temperature (`Fans.cc`, `rhoAirStdInit`); OpenBSE uses the
  fan's actual inlet density, so cold supply air takes less power for the same
  mass flow: about 2.5 % at 12.8 °C, ~10 % on the labs' 100 % outdoor air in
  winter. Deliberate (`air_density` reproduces E+).
- **Pumps.** Total 54.4 vs 65.4 MWh (#511). Primary chiller pumps 41.0
  vs 12.6 MWh: the heat-recovery chiller sits on CoolSys1 here (E+ has it on a
  separate bypass loop through an ideal heat exchanger), so the decoupled
  primary side runs all year; E+ also stages chiller 2's pump off with chiller
  2 (EMS). Secondary pump 2.0 vs 2.3 MWh. Tower pump 11.1 vs 49.9 MWh: E+'s
  chillers request their design condenser flow whenever they run, the
  heat-recovery chiller all year, while OpenBSE's condenser loop flow follows
  its load (design ΔT).
- **Heat rejection** 25.2 vs 11.2 MWh: OpenBSE's tower model (fan power from
  the load over the design capacity, outlet 2 K above the wet-bulb) is not the
  CoolTools variable-speed tower, and E+'s EMS also slows the tower fan when
  one chiller runs.
- **Exterior equipment** −11.6 %: the elevators match; the difference is E+'s
  19.6 MWh of transformer losses (`ElectricLoadCenter:Transformer`), which
  OpenBSE cannot express.
- **Refrigeration** (29.6 MWh in E+): no refrigeration model in OpenBSE.

## Unmet hours (0.556 K)

| | OpenBSE | E+ |
|---|---:|---:|
| Heating (any zone, all hours) | 74.2 h (Radiology_Flr_4 71.8, Kitchen 2.3) | 0 (occupied) |
| Cooling | 0.2 h | 64.5 h (occupied) |

E+ counts occupied hours only ("Time Setpoint Not Met During Occupied"); OpenBSE
counts every hour. Radiology_Flr_4 (VAV_2, 5.6 % minimum flow, 80 W design
heating load) is open.

## Open items

- Condenser flow of running water-cooled chillers (constant, as E+), and a
  heat-recovery chiller on its own loop: the pump split above (#511).
- Water-cooled chillers on the condenser loop temperature (#251).
- Variable-speed tower model (#252) and refrigeration (#509).
- Transformer losses on a metered load (#508).
- `reheat_for_zone_load` takes dry-air c_p (1006) where the zone receives moist
  air; it leaves low-mass zones slightly below their heating setpoint (#510).
- Heating design loads of roof zones (−6 to −9 %); Radiology_Flr_4 unmet
  heating hours; lab humidification.
- Memory: 18 GB RSS, so the model is tier `local` (#512).
