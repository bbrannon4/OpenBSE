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
# The envelope's own conduction (Table 10-2, 171.6 m2 at U = 0.01 W/(m2 K) plus
# films, about 1.713 W/K) is part of that total, so infiltration carries the
# remaining 198.240 W/K: the volumes below are the full-UA volumes scaled by
# 198.240 / 199.953.
_FULL_UA_FLOW = {
    "HE100W": 0.16524,
    "HE120W": 0.16517,
    "HE130W": 0.16494,
    "HE140W": 0.16513,
    "HE210W": 0.16734,
}
ENVELOPE_UA = 171.6 / (100.0 + 0.17)  # W/K: Table 10-2 U with typical films
INFIL_FLOW = {
    w: round(v * (199.953 - ENVELOPE_UA) / 199.953, 5) for w, v in _FULL_UA_FLOW.items()
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
# Section 10.2.1.10.4, part-load operation: heat input ratio coefficients
# a, b, c, d of HIR(PLR) = a + b PLR + c PLR^2 + d PLR^3 (Henderson's
# furnace correlation). PLF = PLR / HIR.
HIR = (0.0080472574, 0.87564457, 0.29249943, -0.17624156)
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
            f"construction: HE Envelope, vertices: [{v}]}}"
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
    # Part-load fuel (Section 10.2.1.10.4): fuel = full-load fuel x HIR(PLR),
    # given as the Standard's heat input ratio curve itself.
    hir_line = f"        hir_curve: {case} Furnace HIR\n"
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
# fan={c['fan']}, setback={c['setback']}. Near-adiabatic shell (Table 10-2) + constant infiltration.

simulation:
  timesteps_per_hour: 1
  start_month: 1
  start_day: 1
  end_month: 3
  end_day: 31

weather_files:
  - "../weather/{c['w']}.csv"

# Table 10-2 (Section 10.2.1.9.2): wall 75.6 m2, floor and roof 48.0 m2, each
# k = 0.01 W/(m K), t = 1.00 m, U = 0.01 W/(m2 K) between its faces, exposed to
# outdoors (the zone is suspended above the ground, 10.2.1.7.1). No capacitance
# is specified, so the layer is resistance-only.
materials:
  - name: HE Envelope Material
    thermal_resistance: 100.0

constructions:
  - name: HE Envelope
    layers:
      - material: HE Envelope Material
        thickness: 1.0

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
{hir_line}{draft_line}{fan_block}    zones:
      - zone: {z}

performance_curves:
  # Section 10.2.1.10.4: HIR(PLR) = a + b PLR + c PLR^2 + d PLR^3.
  - name: {case} Furnace HIR
    curve_type: cubic
    coefficients: [{HIR[0]}, {HIR[1]}, {HIR[2]}, {HIR[3]}]
    min_x: 0.0
    max_x: 1.0

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
