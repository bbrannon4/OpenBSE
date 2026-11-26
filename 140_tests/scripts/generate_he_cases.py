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

# Infiltration (Section 10.2.1.9.3): about 0.2 kg/s of outdoor air, set so the
# zone's total effective UA is 199.953 W/K (its informative note), i.e. a
# mass flow of 199.953 / cp with cp = 1004.84 + 1858.95 W J/(kg K), the
# engine's moist-air specific heat. This is the Standard's approach for
# programs without user-set convection coefficients (Section 10.2.1.9). The
# input is a volume flow on the zone-air density basis (temperature-
# independent mass flow); each weather file gets the volume for 20 C zone
# air at its mean January-March pressure and humidity ratio:
#   rho = p / (287.042 * 293.15 * (1 + 1.607858 W)),  flow = m / rho.
# HE100W-HE140W are at 101330 Pa; HE210W averages 100026 Pa over Jan-Mar.
INFIL_FLOW = {
    "HE100W": 0.16524,
    "HE120W": 0.16517,
    "HE130W": 0.16494,
    "HE140W": 0.16513,
    "HE210W": 0.16734,
}

# case -> dict(weather, efficiency, fan, furnace_kw, setback, draft)
#   fan:   None | "continuous" | "cycling"   (200 W circulating fan)
#   draft: draft-fan power [W]; cycles with the burner (DOE-2 FURNACE-AUX-KW,
#          EnergyPlus COIL:Gas:Heating parasitic electric load). The test's
#          "fan energy (both fans)" is circulating fan + draft fan.
FAN_KW = 0.2006  # circulating-fan power [kW] (SUPPLY-KW basis), ~200 W
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
        # Design flow to deliver furnace capacity at ~30C supply rise.
        fan_block = f"""      - type: fan
        name: {case} Circulating Fan
        source: constant_volume
        design_flow_rate: 0.30
        pressure_rise: 227.0
        motor_efficiency: 1.0
        impeller_efficiency: 0.5
        motor_in_airstream_fraction: 1.0
"""
    fan_mode = "continuous" if c["fan"] == "continuous" else "cycling"
    # Emitted only when present so the other cases' YAML stays byte-identical.
    draft_line = f"        parasitic_electric_load: {c['draft']}\n" if c["draft"] else ""
    # Night setback (HE220/HE230): 15 C hours 1-6, 20 C otherwise (DOE-2 spec).
    heat_sp_line = "    heating_setpoint: 20.0"
    if c["setback"]:
        setback_sched = """    thermostat_schedule:
      - {start_hour: 1, end_hour: 6, heating_setpoint: 15.0, cooling_setpoint: 99.0}
      - {start_hour: 7, end_hour: 0, heating_setpoint: 20.0, cooling_setpoint: 99.0}
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
      design_zone_flow: 0.5
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
