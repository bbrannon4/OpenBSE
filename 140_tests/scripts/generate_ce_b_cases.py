#!/usr/bin/env python3
"""Generate ASHRAE 140-2023 Section 9B (CE_b) dynamic HVAC BESTEST cooling cases.

The §9B tests (CE300-CE545) drive a single near-adiabatic zone with scheduled
internal gains and a performance-map unitary DX cooling system through a full
year of New Orleans weather, probing part-load, outdoor-air, economizer, latent
and thermostat behavior. Reverse-engineered from the reference EnergyPlus inputs
(Std140_CE_b_Files/.../EnergyPlus/CaseE3xx.idf).

Base building (all cases): 14 x 14 x 3 m = 588 m^3 zone with a massless
R-325 m2K/W exterior envelope (near-adiabatic, but the zone still drifts toward
outdoor temperature when the system is idle); sensible (convective) and latent
internal gains on per-case seasonal schedules; cooling setpoint, fan mode and
outdoor air per case; draw-through supply fan; DX coil with the E300
performance map (CapFT/EIRFT biquadratic + PLF = 0.771 + 0.229*PLR). Annual run; compared to the §9B comparative ranges.

Run: python3 140_tests/scripts/generate_ce_b_cases.py
"""

import os

HERE = os.path.dirname(os.path.abspath(__file__))
CASES_DIR = os.path.join(os.path.dirname(HERE), "cases")

# ── E300 DX performance curves (from CaseE300.idf) ────────────────────────────
# Biquadratic f(Twb_entering_C, Todb_outdoor_C), normalized to 1.0 at 19.44/35.
CAPFT = (0.952735372, -0.000932873, 0.000927172, -0.001291389, -2.65899e-05, -0.000305038)
EIRFT = (0.535665387, -0.000900699, -0.000155223, 0.012769543, 0.000163848, -0.000158128)


def curve_poly(name, coeffs):
    """Emit the biquadratic exactly as the reference input gives it. The 500
    series runs the coil at 15-35 C zone setpoints, well outside the CE_a
    performance-map grid, so a tabulated copy of the curve would extrapolate
    linearly and miss the curvature."""
    return f"""  - name: {name}
    curve_type: biquadratic
    coefficients: [{", ".join(repr(v) for v in coeffs)}]
    min_x: 0.0
    max_x: 100.0
    min_y: 0.0
    max_y: 100.0"""


# ── Seasonal schedules ────────────────────────────────────────────────────────
# Each schedule is a list of (start_month, start_day, end_month, end_day,
# day-profile) periods, transcribed from the reference EnergyPlus SCHEDULE /
# WEEKSCHEDULE / DAYSCHEDULE objects (CaseE3xx/4xx/5xx.idf). A day profile is
# written run-length encoded, "value x hours" for hours 1-24: "0x8 1x12 0x4" is
# zero for hours 1-8, one for hours 9-20, zero for hours 21-24.


def rl(spec):
    """Expand a run-length day profile ("0.15x8 1x12 0.15x4") to 24 values."""
    out = []
    for tok in spec.split():
        val, n = tok.split("x")
        out += [float(val)] * int(n)
    assert len(out) == 24, spec
    return out


def sched(*periods):
    return [(sm, sd, em, ed, rl(spec)) for (sm, sd, em, ed, spec) in periods]


# CE300-series sensible gain (fraction of 18758 W).
SENS_300 = sched(
    (1, 1, 3, 10, "0.15625x24"),
    (3, 11, 4, 10, "0.15625x9 0.375x9 0.15625x6"),
    (4, 11, 4, 11, "0.15625x24"),
    (4, 12, 4, 20, "0.15625x8 0.5x11 0.15625x5"),
    (4, 21, 10, 12, "0.375x8 0.5x4 0.75x2 1x2 0.5x4 0.375x4"),
    (10, 13, 10, 18, "0.15625x8 0.5x8 0.15625x8"),
    (10, 19, 11, 5, "0.15625x8 0.5x4 0.75x2 1x2 0.5x4 0.375x4"),
    (11, 6, 12, 31, "0.15625x24"),
)
# CE360: same loads outside summer (rescaled to the 29310 W peak), full load
# around the clock Apr 21 - Oct 12.
SENS_360 = sched(
    (1, 1, 3, 10, "0.1x24"),
    (3, 11, 4, 10, "0.1x9 0.24x9 0.1x6"),
    (4, 11, 4, 11, "0.1x24"),
    (4, 12, 4, 20, "0.1x8 0.32x11 0.1x5"),
    (4, 21, 10, 12, "1x24"),
    (10, 13, 10, 18, "0.1x8 0.32x8 0.1x8"),
    (10, 19, 11, 5, "0.1x8 0.32x4 0.48x2 0.64x2 0.32x4 0.24x4"),
    (11, 6, 12, 31, "0.1x24"),
)
# CE300-series latent gain (fraction of 1466 W).
LAT_300 = sched(
    (1, 1, 3, 10, "0x8 0.25x12 0x4"),
    (3, 11, 4, 10, "0x9 1x9 0x6"),
    (4, 11, 4, 11, "0x8 0.25x12 0x4"),
    (4, 12, 4, 20, "0x8 1x11 0x5"),
    (4, 21, 10, 12, "0x8 1x12 0x4"),
    (10, 13, 10, 18, "0x8 1x8 0x8"),
    (10, 19, 11, 5, "0x8 1x12 0x4"),
    (11, 6, 12, 31, "0x8 0.25x12 0x4"),
)
# CE310 high latent gain (fraction of 9379 W).
LAT_310 = sched(
    (1, 1, 3, 10, "0x8 0.15625x12 0x4"),
    (3, 11, 4, 10, "0x9 0.75x9 0x6"),
    (4, 11, 4, 11, "0x8 0.15625x12 0x4"),
    (4, 12, 4, 20, "0x8 0.75x11 0x5"),
    (4, 21, 10, 12, "0.75x12 1x4 0.75x8"),
    (10, 13, 10, 18, "0x8 0.75x8 0x8"),
    (10, 19, 11, 5, "0.15625x8 0.75x4 1x4 0.75x8"),
    (11, 6, 12, 31, "0x8 0.15625x12 0x4"),
)
# CE500-series gain: ONE schedule drives both the sensible (18758 W) and the
# latent (7621 W) gain. No load at all in winter.
GAIN_500 = sched(
    (1, 1, 3, 10, "0x24"),
    (3, 11, 4, 10, "0x9 0.375x7 0x8"),
    (4, 11, 4, 11, "0x24"),
    (4, 12, 4, 20, "0x8 0.5x9 0x7"),
    (4, 21, 10, 11, "0.375x8 0.5x4 0.75x2 1x2 0.5x4 0.375x4"),
    (10, 12, 10, 18, "0x8 0.5x6 0x10"),
    (10, 19, 11, 5, "0x8 0.5x4 0.75x2 1x2 0.5x4 0.375x4"),
    (11, 6, 12, 31, "0x24"),
)
# CE510: rescaled to the 21103 W / 8573 W peaks, full load all summer.
GAIN_510 = sched(
    (1, 1, 3, 10, "0x24"),
    (3, 11, 4, 10, "0x9 0.33333x7 0x8"),
    (4, 11, 4, 11, "0x24"),
    (4, 12, 4, 20, "0x8 0.44444x9 0x7"),
    (4, 21, 10, 11, "1x24"),
    (10, 12, 10, 18, "0x8 0.44444x6 0x10"),
    (10, 19, 11, 5, "0x8 0.44444x4 0.66667x2 0.88889x2 0.44444x4 0.33333x4"),
    (11, 6, 12, 31, "0x24"),
)

# ── Case table ────────────────────────────────────────────────────────────────
# sens / lat: (peak W, schedule); lat=None for the dry-coil cases.
# sp: cooling setpoint [C]. fan: continuous (ContFanCycComp) or cycling
# (CycFanCycComp). oa: fixed outdoor-air flow [m3/s] (0 = no outdoor-air system).
OA_300 = 0.283166667


def c300(**kw):
    base = dict(sens=(18758.0, SENS_300), lat=(1466.0, LAT_300), sp=25.0,
                fan="continuous", oa=OA_300)
    base.update(kw)
    return base


def c500(**kw):
    base = dict(sens=(18758.0, GAIN_500), lat=(7621.0, GAIN_500), sp=25.0,
                fan="cycling", oa=0.0)
    base.update(kw)
    return base


CASES = {
    "CE300": c300(),
    "CE310": c300(lat=(9379.0, LAT_310)),
    "CE360": c300(sens=(29310.0, SENS_360)),
    "CE500": c500(),
    "CE510": c500(sens=(21103.0, GAIN_510), lat=(8573.0, GAIN_510)),
    "CE520": c500(sp=15.0),
    "CE522": c500(sp=20.0),
    "CE525": c500(sp=35.0),
}


def _arr(frac):
    return "[" + ", ".join(f"{v:.5f}" for v in frac) + "]"


def seasonal_sched_block(name, periods):
    """Emit a seasonal schedule. The top-level weekday is the first period
    (winter base); every period is then listed explicitly under `seasons:`."""
    lines = [f"  - name: {name}", f"    weekday: {_arr(periods[0][4])}", "    seasons:"]
    for sm, sd, em, ed, prof in periods:
        lines.append(
            f"      - {{start_month: {sm}, start_day: {sd}, "
            f"end_month: {em}, end_day: {ed}, weekday: {_arr(prof)}}}"
        )
    return "\n".join(lines)


def gain_block(case, z, c):
    """Internal-gain equipment + the schedules they reference."""
    sens_sched, lat_sched = f"{case} SensSch", f"{case} LatSch"
    scheds = [seasonal_sched_block(sens_sched, c["sens"][1])]
    equip = [f"""  - name: {case} Sensible Gain
    zones: [{z}]
    power: {c['sens'][0]}
    radiant_fraction: 0.0
    latent_fraction: 0.0
    schedule: {sens_sched}"""]
    if c["lat"] is not None:
        scheds.append(seasonal_sched_block(lat_sched, c["lat"][1]))
        equip.append(f"""  - name: {case} Latent Gain
    zones: [{z}]
    power: {c['lat'][0]}
    radiant_fraction: 0.0
    latent_fraction: 1.0
    schedule: {lat_sched}""")
    return scheds, equip


def make_yaml(case, c):
    z = f"{case} Zone"
    scheds, equip = gain_block(case, z, c)
    nl = "\n"
    return f"""# ASHRAE 140-2023 Section 9B — Case {case} (dynamic HVAC BESTEST cooling)
# GENERATED by 140_tests/scripts/generate_ce_b_cases.py.
# 588 m^3 near-adiabatic zone (R-325 m2K/W exterior envelope), New Orleans
# annual weather, performance-map DX unit.

simulation:
  timesteps_per_hour: 4
  start_month: 1
  start_day: 1
  end_month: 12
  end_day: 31

weather_files:
  - "CE300.csv"

schedules:
{nl.join(scheds)}

simple_constructions:
  - name: Adiabatic Wall
    u_factor: 0.0030769
    thickness: 0.01
    thermal_capacity: 0.0
    solar_absorptance: 0.1
    thermal_absorptance: 0.9
    roughness: rough

zones:
  - name: {z}
    volume: 588.0
    floor_area: 196.0
    limit_humidity_to_saturation: true

equipment:
{nl.join(equip)}

surfaces:
  - {{name: {case} N, zone: {z}, type: wall, construction: Adiabatic Wall, boundary: outdoor, vertices: [{{x: 14.0, y: 14.0, z: 0}},{{x: 0.0, y: 14.0, z: 0}},{{x: 0.0, y: 14.0, z: 3.0}},{{x: 14.0, y: 14.0, z: 3.0}}]}}
  - {{name: {case} S, zone: {z}, type: wall, construction: Adiabatic Wall, boundary: outdoor, vertices: [{{x: 0.0, y: 0.0, z: 0}},{{x: 14.0, y: 0.0, z: 0}},{{x: 14.0, y: 0.0, z: 3.0}},{{x: 0.0, y: 0.0, z: 3.0}}]}}
  - {{name: {case} E, zone: {z}, type: wall, construction: Adiabatic Wall, boundary: outdoor, vertices: [{{x: 14.0, y: 0.0, z: 0}},{{x: 14.0, y: 14.0, z: 0}},{{x: 14.0, y: 14.0, z: 3.0}},{{x: 14.0, y: 0.0, z: 3.0}}]}}
  - {{name: {case} W, zone: {z}, type: wall, construction: Adiabatic Wall, boundary: outdoor, vertices: [{{x: 0.0, y: 14.0, z: 0}},{{x: 0.0, y: 0.0, z: 0}},{{x: 0.0, y: 0.0, z: 3.0}},{{x: 0.0, y: 14.0, z: 3.0}}]}}
  - {{name: {case} Roof, zone: {z}, type: roof, construction: Adiabatic Wall, boundary: outdoor, vertices: [{{x: 0.0, y: 0.0, z: 3.0}},{{x: 14.0, y: 0.0, z: 3.0}},{{x: 14.0, y: 14.0, z: 3.0}},{{x: 0.0, y: 14.0, z: 3.0}}]}}
  - {{name: {case} Floor, zone: {z}, type: floor, construction: Adiabatic Wall, boundary: outdoor, vertices: [{{x: 0.0, y: 14.0, z: 0}},{{x: 14.0, y: 14.0, z: 0}},{{x: 14.0, y: 0.0, z: 0}},{{x: 0.0, y: 0.0, z: 0}}]}}

air_loops:
  - name: CE DX System
    controls:
      cooling_supply_temp: 13.0
      heating_supply_temp: 40.0
      cycling: proportional
      fan_operating_mode: {c['fan']}
      deadband: 1.0
      minimum_damper_position: {round(c['oa'] / 1.888, 5)}
      design_zone_flow: 2.266
      cooling_part_load_cd: 0.229
    equipment:
      - type: cooling_coil
        name: {case} DX Cooling Coil
        source: dx
        capacity: 33280.0
        cop: 3.3256
        shr: 0.8043
        rated_airflow: 1.888
        setpoint: 13.0
        autocalculate_shr: true
        condenser_fan_power: 930.0
        cap_ft_curve: "CE300 DX CapFT"
        eir_ft_curve: "CE300 DX EIRFT"
      - type: fan
        name: {case} Supply Fan
        source: constant_volume
        design_flow_rate: 1.888
        pressure_rise: 74.7
        motor_efficiency: 0.94
        impeller_efficiency: 0.121
        motor_in_airstream_fraction: 1.0
    zones:
      - zone: {z}

zone_groups:
  - name: {case} Zone Group
    zones: [{z}]

thermostats:
  - name: {case} Thermostat
    zones: [{case} Zone Group]
    cooling_setpoint: {c['sp']}
    heating_setpoint: -50.0   # cooling-only thermostat: the zone is never heated

outputs:
  - file: "zone_results.csv"
    frequency: hourly
    variables:
      - "zone:temperature"
      - "zone:humidity_ratio"
  - file: "hvac_results.csv"
    frequency: hourly
    variables:
      - "component:compressor_power"
      - "component:condenser_fan_power"
      - "component:electric_power"
      - "component:total_load"
      - "component:sensible_load"
      - "component:latent_load"

summary_report: true

performance_curves:
{curve_poly("CE300 DX CapFT", CAPFT)}
{curve_poly("CE300 DX EIRFT", EIRFT)}
"""


def main():
    for case, c in CASES.items():
        with open(os.path.join(CASES_DIR, f"ashrae140_case{case}.yaml"), "w") as f:
            f.write(make_yaml(case, c))
        print(f"wrote ashrae140_case{case}.yaml")


if __name__ == "__main__":
    main()
