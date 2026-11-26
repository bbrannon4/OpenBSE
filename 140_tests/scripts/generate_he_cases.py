#!/usr/bin/env python3
"""Generate ASHRAE 140-2023 Section 10 (HE100-HE230) furnace case YAMLs.

Fuel-fired furnace BESTEST. A single adiabatic zone with constant infiltration
(0.2 kg/s) heated by a gas furnace to 20 C, run Jan-Mar. Each case varies
weather, furnace efficiency, circulating-fan mode, thermostat setback, and
furnace capacity.

Authoritative parameters from the standard's normative materials (DOE-2 and
EnergyPlus reference inputs): furnace HIR (1/efficiency), infiltration rate,
fan power, and setback schedule. Weather: the Standard's normative WYEC2 files
(HE100W, HE120W, HE130W, HE140W, HE210W), converted field for field by
convert_wy2_weather.py into 140_tests/weather/.

Run:  python3 140_tests/scripts/generate_he_cases.py
"""

import os

HERE = os.path.dirname(os.path.abspath(__file__))
CASES_DIR = os.path.join(os.path.dirname(HERE), "cases")

# Infiltration: 0.2 kg/s of outdoor air (Section 10.2 heat exchange by
# infiltration, the approach for programs without user-set convection
# coefficients). The input is a volume flow on the zone-air density basis, so
# the mass flow is temperature-independent; each weather file gets the volume
# that carries 0.2 kg/s at 20 C zone air, the file's mean January-March
# pressure and the zone humidity ratio that outdoor air brings in:
#   rho = p / (287.042 * 293.15 * (1 + 1.607858 W)),  flow = 0.2 / rho.
# HE100W-HE140W are at 101330 Pa; HE210W averages 100026 Pa over Jan-Mar.
INFIL_FLOW = {
    "HE100W": 0.16613,
    "HE120W": 0.16663,
    "HE130W": 0.16818,
    "HE140W": 0.16691,
    "HE210W": 0.16870,
}

# case -> dict(weather, efficiency, fan, furnace_kw, setback, draft)
#   fan:   None | "continuous" | "cycling"   (200 W circulating fan)
#   draft: draft-fan power [W]; cycles with the burner (DOE-2 FURNACE-AUX-KW,
#          EnergyPlus COIL:Gas:Heating parasitic electric load). The test's
#          "fan energy (both fans)" is circulating fan + draft fan.
# Circulating fan (Section 10.2, Case HE150): 200 W at 0.355 m3/s, all of its
# heat to the supply air. Total efficiency 0.5 with a 281.7 Pa rise gives
# 0.355 * 281.7 / 0.5 = 200 W. The loop's design mass flow is 0.355 m3/s at
# 20 C zone air and 101330 Pa (1.204 kg/m3); the fan converts mass to volume
# flow at that density, so its power stays 200 W whatever the supply
# temperature (the specified fan draw is constant).
FAN_FLOW = 0.355  # m3/s
FAN_DP = 281.7  # Pa
FAN_RHO = 1.204  # kg/m3
LOOP_FLOW = 0.4274  # kg/s
DRAFT_FAN_W = 50.0
CASES = {
    "HE100": dict(w="HE100W", eff=1.00, fan=None, kw=10.0, setback=False, draft=0.0),
    "HE110": dict(w="HE100W", eff=0.80, fan=None, kw=10.0, setback=False, draft=0.0),
    "HE120": dict(w="HE120W", eff=0.80, fan=None, kw=10.0, setback=False, draft=0.0),
    "HE130": dict(w="HE130W", eff=0.80, fan=None, kw=10.0, setback=False, draft=0.0),
    "HE140": dict(w="HE140W", eff=0.80, fan=None, kw=10.0, setback=False, draft=0.0),
    "HE150": dict(w="HE140W", eff=0.80, fan="continuous", kw=10.0, setback=False, draft=0.0),
    "HE160": dict(w="HE140W", eff=0.80, fan="cycling", kw=10.0, setback=False, draft=0.0),
    "HE170": dict(w="HE140W", eff=0.80, fan="continuous", kw=10.0, setback=False, draft=DRAFT_FAN_W),
    "HE210": dict(w="HE210W", eff=0.80, fan="cycling", kw=10.0, setback=False, draft=DRAFT_FAN_W),
    "HE220": dict(w="HE210W", eff=0.80, fan="cycling", kw=10.0, setback=True, draft=DRAFT_FAN_W),
    "HE230": dict(w="HE210W", eff=0.80, fan="cycling", kw=5.0, setback=True, draft=DRAFT_FAN_W),
}

WALLS = [
    ("North Wall", "wall", [(8.0, 6.0, 0.0), (0.0, 6.0, 0.0), (0.0, 6.0, 2.7), (8.0, 6.0, 2.7)]),
    ("South Wall", "wall", [(0.0, 0.0, 0.0), (8.0, 0.0, 0.0), (8.0, 0.0, 2.7), (0.0, 0.0, 2.7)]),
    ("East Wall", "wall", [(8.0, 0.0, 0.0), (8.0, 6.0, 0.0), (8.0, 6.0, 2.7), (8.0, 0.0, 2.7)]),
    ("West Wall", "wall", [(0.0, 6.0, 0.0), (0.0, 0.0, 0.0), (0.0, 0.0, 2.7), (0.0, 6.0, 2.7)]),
    ("Roof", "roof", [(0.0, 0.0, 2.7), (8.0, 0.0, 2.7), (8.0, 6.0, 2.7), (0.0, 6.0, 2.7)]),
    ("Floor", "floor", [(0.0, 6.0, 0.0), (8.0, 6.0, 0.0), (8.0, 0.0, 0.0), (0.0, 0.0, 0.0)]),
]


def surfaces(case):
    z = f"{case} Zone"
    out = []
    for name, typ, verts in WALLS:
        v = ",".join(f"{{x: {x}, y: {y}, z: {zz}}}" for x, y, zz in verts)
        out.append(
            f"  - {{name: {case} {name}, zone: {z}, type: {typ}, "
            f"construction: Adiabatic Wall, boundary: adiabatic, vertices: [{v}]}}"
        )
    return "\n".join(out)


def make_yaml(case, c):
    z = f"{case} Zone"
    fan_block = ""
    if c["fan"]:
        mode = "continuous" if c["fan"] == "continuous" else "cycling"
        fan_block = f"""      - type: fan
        name: {case} Circulating Fan
        source: constant_volume
        design_flow_rate: {FAN_FLOW}
        pressure_rise: {FAN_DP}
        air_density: {FAN_RHO}
        motor_efficiency: 1.0
        impeller_efficiency: 0.5
        motor_in_airstream_fraction: 1.0
"""
    fan_mode = "continuous" if c["fan"] == "continuous" else "cycling"
    # Emitted only when present so the other cases' YAML stays byte-identical.
    draft_line = f"        parasitic_electric_load: {c['draft']}\n" if c["draft"] else ""
    # Night setback (HE220/HE230, Section 10.2): heat below 15 C for
    # 23:00 < time < 6:00 and below 20 C for 6:00 <= time <= 23:00, i.e.
    # 15 C in hour-ending hours 24 and 1-6, 20 C in hours 7-23.
    heat_sp_line = "    heating_setpoint: 20.0"
    if c["setback"]:
        setback_sched = """    thermostat_schedule:
      - {start_hour: 24, end_hour: 6, heating_setpoint: 15.0, cooling_setpoint: 99.0}
      - {start_hour: 7, end_hour: 23, heating_setpoint: 20.0, cooling_setpoint: 99.0}
"""
    else:
        setback_sched = ""
    return f"""# ASHRAE 140-2023 Section 10 — Case {case}
# Fuel-fired furnace BESTEST. GENERATED by 140_tests/scripts/generate_he_cases.py.
# Weather {c['w']}, furnace efficiency {c['eff']:.2f}, capacity {c['kw']} kW,
# fan={c['fan']}, setback={c['setback']}. Adiabatic shell + constant infiltration.

simulation:
  timesteps_per_hour: 1
  start_month: 1
  start_day: 1
  end_month: 3
  end_day: 31

weather_files:
  - "../weather/{c['w']}.csv"

simple_constructions:
  - name: Adiabatic Wall
    u_factor: 0.01
    thermal_capacity: 0.0
    solar_absorptance: 0.1
    thermal_absorptance: 0.9
    roughness: rough

zones:
  - name: {z}
    volume: 129.6
    floor_area: 48.0

infiltration:
  - name: {case} Infiltration
    zones: [{z}]
    design_flow_rate: {INFIL_FLOW[c['w']]}
    density_basis: zone

surfaces:
{surfaces(case)}

air_loops:
  - name: Furnace
    controls:
      heating_supply_temp: 50.0
      cooling_supply_temp: 13.0
      cycling: on_off
      deadband: 1.0
      minimum_damper_position: 0.0
      design_zone_flow: {LOOP_FLOW}
      fan_operating_mode: {fan_mode}
    equipment:
      - type: heating_coil
        name: {case} Gas Furnace
        source: gas
        capacity: {c['kw'] * 1000.0}
        efficiency: {c['eff']}
        setpoint: 50.0
{draft_line}{fan_block}    zones:
      - zone: {z}

zone_groups:
  - name: {case} Zone Group
    zones: [{z}]

thermostats:
  - name: {case} Thermostat
    zones: [{case} Zone Group]
{heat_sp_line}
    cooling_setpoint: 99.0
{setback_sched}
outputs:
  - file: "hvac_results.csv"
    frequency: hourly
    variables:
      - "component:thermal_output"
      - "component:fuel_power"
      - "component:electric_power"
  - file: "zone_results.csv"
    frequency: hourly
    variables:
      - "zone:temperature"

summary_report: true
"""


def main():
    for case, c in CASES.items():
        path = os.path.join(CASES_DIR, f"ashrae140_case{case}.yaml")
        with open(path, "w") as f:
            f.write(make_yaml(case, c))
        print(f"wrote {os.path.basename(path)}")


if __name__ == "__main__":
    main()
