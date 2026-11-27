#!/usr/bin/env python3
"""Generate the ASHRAE 140-2023 Section 7.2.3 in-depth case YAMLs and their
conformance-spec entries.

The in-depth cases (7.2.3: 195-320, 395-470, 800, 810) are Case 600 or 900
with single changes: no infiltration or gains, high-conductance wall elements
in place of the windows, altered surface radiative properties, a "20,20"
thermostat, constant combined surface coefficients. Informative Table 7-33
lists each case's primary and secondary base cases; every value below is from
the clause of the case or of a base case it inherits, cited in the case
description and in the spec entry's clause. The Standard's text governs; the
accompanying EnergyPlus inputs (140_tests/reference_idfs/) were a cross-check.

Run:  python3 140_tests/scripts/generate_indepth_cases.py
It rewrites 140_tests/cases/ashrae140_case<N>.yaml for each in-depth case and
the cases' entries in 140_tests/conformance_spec.json (other entries kept).
"""

import json
import os
import textwrap
from dataclasses import dataclass, field, replace

HERE = os.path.dirname(os.path.abspath(__file__))
TESTS = os.path.dirname(HERE)
CASES_DIR = os.path.join(TESTS, "cases")
SPEC_PATH = os.path.join(TESTS, "conformance_spec.json")

WEATHER = ("ASHRAE 140-2023 Accompanying FIles/Std140_TF_Files/"
           "Normative Materials/725650TY.csv")


# ---------------------------------------------------------------------------
# Case parameters
# ---------------------------------------------------------------------------
@dataclass(frozen=True)
class Case:
    num: str
    title: str
    clause: str
    bases: str                 # primary/secondary base cases (Table 7-33)
    changes: tuple             # one line per change, with its clause
    mass: str = "low"          # "low": Table 7-2; "high": Table 7-27 walls/floor
    south: str = "windows"     # "windows" | "hcwe" | "wall" | "east_west"
    shading: str = ""          # "" | "overhang" (610) | "east_west" (630)
    infiltration: float = 0.5  # ach
    gains: float = 200.0       # W
    ext_solar: float = 0.6     # exterior solar absorptance (not the raised floor)
    int_solar: float = 0.6     # interior solar absorptance
    ext_ir: float = 0.9        # exterior infrared emittance
    int_ir: float = 0.9        # interior infrared emittance
    cooling_setpoint: float = 27.0
    inside_combined: bool = False   # Table 7-46 interior hcomb
    outside_combined: bool = False  # Table 7-46 exterior hcomb
    clauses: dict = field(default_factory=dict)  # per-parameter clause


# Table 7-46 (7.2.3.21): combined coefficients [W/(m2 K)], (interior, exterior).
HCOMB = {
    "wall": (1.8, 21.6),
    "roof": (1.7, 21.8),
    "floor": (3.7, 5.2),
    "window": (4.5, 17.8),
}

# Clause tags reused by many cases.
C600 = {
    "infiltration": "7.2.1.6 infiltration 0.5 ach",
    "gains": "7.2.1.7 internal gains 200 W",
    "solar": "7.2.1.8.1 Table 7-5",
    "thermostat": "7.2.1.13.1 thermostat",
}
C220 = {
    "infiltration": "7.2.3.1.1 infiltration 0 ach",
    "gains": "7.2.3.1.2 internal gains 0 W",
    "solar": "7.2.3.1.3 Table 7-34",
    "thermostat": "7.2.3.1.5.1 20,20 thermostat",
}
C400 = {
    "infiltration": "7.2.3.15.1 infiltration 0 ach",
    "gains": "7.2.3.15.2 internal gains 0 W",
    "solar": "7.2.3.15.3 exterior solar absorptance 0.1",
    "thermostat": "7.2.1.13.1 thermostat (7.2.3.15, as Case 600)",
}

# Case 220: Case 600 with no infiltration or gains, Table 7-34 surfaces,
# high-conductance wall elements for the windows, "20,20" thermostat.
CASE220 = Case(
    num="220", title="In-depth series base case", clause="7.2.3.1", bases="600, 685",
    changes=(
        "Infiltration 0 ach (7.2.3.1.1); internal gains 0 W (7.2.3.1.2)",
        "Opaque surfaces per Table 7-34: interior solar 0.6, exterior solar 0.1,"
        " IR 0.9 both faces; raised floor exterior gets no solar (7.2.3.1.3)",
        "High-conductance wall elements replace the 12 m2 of south windows"
        " (7.2.3.1.4, Tables 7-35, 7-36)",
        "\"20,20\" nonproportional thermostat (7.2.3.1.5)",
    ),
    south="hcwe", infiltration=0.0, gains=0.0, ext_solar=0.1, int_solar=0.6,
    cooling_setpoint=20.0, clauses=C220,
)

CASE270 = replace(
    CASE220, num="270", title="South solar gains", clause="7.2.3.9", bases="220, 600",
    changes=CASE220.changes[:2] + (
        "Interior solar absorptance 0.9 on all opaque surfaces (7.2.3.9.1)",
        "Case 600 south windows replace the high-conductance wall elements"
        " (7.2.3.9.2)",
        "Interior solar distribution calculated by the program (7.2.3.9.3.1)",
        CASE220.changes[3],
    ),
    south="windows", int_solar=0.9,
    clauses=dict(C220, int_solar="7.2.3.9.1 interior solar absorptance 0.9"),
)

CASE300 = replace(
    CASE270, num="300", title="East/west window orientation", clause="7.2.3.12",
    bases="270, 620",
    changes=CASE270.changes + (
        "6 m2 of window east and 6 m2 west, none south, as Case 620 (7.2.3.12.1)",
    ),
    south="east_west",
)

CASE400 = Case(
    num="400", title="High-conductance wall elements with deadband thermostat",
    clause="7.2.3.15", bases="600, 220",
    changes=(
        "Infiltration 0 ach (7.2.3.15.1); internal gains 0 W (7.2.3.15.2)",
        "Exterior solar absorptance 0.1 on all opaque surfaces but the raised"
        " floor, high-conductance wall elements included (7.2.3.15.3)",
        "High-conductance wall elements replace the 12 m2 of south windows,"
        " properties as Case 220 (7.2.3.15.4)",
        "\"20,27\" thermostat as Case 600",
    ),
    south="hcwe", infiltration=0.0, gains=0.0, ext_solar=0.1, clauses=C400,
)

CASE410 = replace(
    CASE400, num="410", title="Infiltration", clause="7.2.3.17", bases="400, 600",
    changes=CASE400.changes + ("Infiltration 0.5 ach (7.2.3.17)",),
    infiltration=0.5, clauses=dict(C400, infiltration="7.2.3.17 infiltration 0.5 ach"),
)
CASE420 = replace(
    CASE410, num="420", title="Internal gains", clause="7.2.3.18", bases="410, 600",
    changes=CASE410.changes + (
        "Internal gains 200 W, 100% sensible, 60% radiative (7.2.3.18)",),
    gains=200.0, clauses=dict(CASE410.clauses, gains="7.2.3.18 internal gains 200 W"),
)
CASE430 = replace(
    CASE420, num="430", title="Exterior shortwave absorptance", clause="7.2.3.19",
    bases="420, 600",
    changes=CASE420.changes + (
        "Exterior solar absorptance 0.6, high-conductance wall elements"
        " included (7.2.3.19)",),
    ext_solar=0.6,
    clauses=dict(CASE420.clauses, solar="7.2.3.19 exterior solar absorptance 0.6"),
)

CASE600 = Case(
    num="600", title="", clause="7.2.1", bases="", changes=(), clauses=C600,
)

CASES = [
    # --- "20,20" series on Case 220 ---
    CASE220,
    replace(
        CASE220, num="210", title="Interior infrared radiation", clause="7.2.3.2",
        bases="220",
        changes=CASE220.changes + (
            "Interior IR emittance 0.1 on all opaque surfaces, high-conductance"
            " wall elements included (7.2.3.2.1)",),
        int_ir=0.1, clauses=dict(C220, int_ir="7.2.3.2.1 interior IR emittance 0.1"),
    ),
    replace(
        CASE220, num="215", title="Exterior infrared radiation", clause="7.2.3.3",
        bases="220",
        changes=CASE220.changes + (
            "Exterior IR emittance 0.1 on all opaque surfaces, high-conductance"
            " wall elements included (7.2.3.3.1)",),
        ext_ir=0.1, clauses=dict(C220, ext_ir="7.2.3.3.1 exterior IR emittance 0.1"),
    ),
    replace(
        CASE220, num="200", title="Infrared radiation", clause="7.2.3.4",
        bases="210, 215",
        changes=CASE220.changes + (
            "Interior IR emittance 0.1 (7.2.3.2.1, from Case 210)",
            "Exterior IR emittance 0.1 on all opaque surfaces (7.2.3.4.1)",),
        int_ir=0.1, ext_ir=0.1,
        clauses=dict(C220, int_ir="7.2.3.2.1 interior IR emittance 0.1 (Case 210)",
                     ext_ir="7.2.3.4.1 exterior IR emittance 0.1"),
    ),
    replace(
        CASE220, num="195", title="Solid conduction test", clause="7.2.3.5",
        bases="200, 600",
        changes=CASE220.changes[:2] + (
            "Interior and exterior IR emittance 0.1 (7.2.3.4, from Case 200)",
            "Lightweight exterior wall of Table 7-2 in place of the high-conductance"
            " wall elements: rough outside, smooth inside, radiative properties of"
            " the Case 200 walls (7.2.3.5)",
            CASE220.changes[3],),
        south="wall", int_ir=0.1, ext_ir=0.1,
        clauses=dict(C220, int_ir="7.2.3.2.1 interior IR emittance 0.1 (Case 200)",
                     ext_ir="7.2.3.4.1 exterior IR emittance 0.1 (Case 200)"),
    ),
    replace(
        CASE220, num="230", title="Infiltration", clause="7.2.3.6", bases="220",
        changes=CASE220.changes + (
            "Infiltration 1.0 ach, constant; the program corrects for altitude"
            " (7.2.3.6, Table 7-42)",),
        infiltration=1.0, clauses=dict(C220, infiltration="7.2.3.6 infiltration 1.0 ach"),
    ),
    replace(
        CASE220, num="240", title="Internal gains", clause="7.2.3.7", bases="220",
        changes=CASE220.changes + (
            "Internal gains 200 W, 100% sensible, 60% radiative (7.2.3.7)",),
        gains=200.0, clauses=dict(C220, gains="7.2.3.7 internal gains 200 W"),
    ),
    replace(
        CASE220, num="250", title="Exterior shortwave absorptance", clause="7.2.3.8",
        bases="220, 600",
        changes=CASE220.changes + (
            "Exterior solar absorptance 0.9 on all opaque surfaces but the raised"
            " floor, high-conductance wall elements included (7.2.3.8)",),
        ext_solar=0.9, clauses=dict(C220, solar="7.2.3.8 exterior solar absorptance 0.9"),
    ),
    CASE270,
    replace(
        CASE270, num="280", title="Cavity albedo", clause="7.2.3.10", bases="270",
        changes=CASE270.changes + (
            "Interior solar absorptance 0.1 on all opaque surfaces (7.2.3.10.1)",),
        int_solar=0.1,
        clauses=dict(CASE270.clauses, int_solar="7.2.3.10.1 interior solar absorptance 0.1"),
    ),
    replace(
        CASE270, num="290", title="South shading", clause="7.2.3.11", bases="270, 610",
        changes=CASE270.changes + (
            "Case 610 overhang over the full length of the south wall (7.2.3.11)",),
        shading="overhang",
    ),
    CASE300,
    replace(
        CASE300, num="310", title="East/west shading", clause="7.2.3.13",
        bases="300, 630",
        changes=CASE300.changes + (
            "Case 630 overhangs and fins on the east and west windows (7.2.3.13)",),
        shading="east_west",
    ),
    replace(
        CASE270, num="320", title="Thermostat", clause="7.2.3.14", bases="270, 600",
        changes=CASE270.changes[:-1] + (
            "\"20,27\" nonproportional thermostat (7.2.3.14)",),
        cooling_setpoint=27.0,
        clauses=dict(CASE270.clauses, thermostat="7.2.3.14.1 20,27 thermostat"),
    ),
    # --- "20,27" series on Case 600/400 ---
    replace(
        CASE400, num="395", title="Solid conduction test", clause="7.2.3.16",
        bases="400, 195",
        changes=CASE400.changes[:2] + (
            "Lightweight exterior wall of Table 7-2 in place of the high-conductance"
            " wall elements: rough outside, smooth inside, radiative properties of"
            " the Case 400 walls (7.2.3.16)",
            CASE400.changes[3],),
        south="wall",
    ),
    CASE400, CASE410, CASE420, CASE430,
    replace(
        CASE600, num="440", title="Cavity albedo", clause="7.2.3.20", bases="600, 280",
        changes=(
            "Interior solar absorptance 0.1 on all opaque surfaces (7.2.3.20.1)",
            "Interior solar distribution calculated by the program (7.2.3.20.2,"
            " 7.2.3.10.2.1)",),
        int_solar=0.1,
        clauses=dict(C600, int_solar="7.2.3.20.1 interior solar absorptance 0.1"),
    ),
    replace(
        CASE600, num="450",
        title="Constant combined interior and exterior surface coefficients",
        clause="7.2.3.21", bases="600",
        changes=(
            "Constant interior combined coefficients of Table 7-46 (7.2.3.21.1)",
            "Constant exterior combined coefficients of Table 7-46 (7.2.3.21.2)",),
        inside_combined=True, outside_combined=True,
    ),
    replace(
        CASE600, num="460", title="Constant combined interior surface coefficients",
        clause="7.2.3.22", bases="600, 450",
        changes=(
            "Constant interior combined coefficients of Table 7-46 (7.2.3.22.1);"
            " exterior as Case 600",),
        inside_combined=True,
    ),
    replace(
        CASE600, num="470", title="Constant combined exterior surface coefficients",
        clause="7.2.3.23", bases="600, 450",
        changes=(
            "Constant exterior combined coefficients of Table 7-46 (7.2.3.23.1);"
            " interior as Case 600",),
        outside_combined=True,
    ),
    replace(
        CASE430, num="800", title="High-mass without solar gains", clause="7.2.3.24",
        bases="430, 900",
        changes=CASE430.changes + (
            "Case 900 exterior wall and floor materials (Table 7-27); roof and"
            " high-conductance wall elements as Case 430 (7.2.3.24)",),
        mass="high",
    ),
    replace(
        CASE600, num="810", title="High-mass cavity albedo", clause="7.2.3.25",
        bases="900, 280",
        changes=(
            "Case 900 exterior wall and floor materials (Table 7-27)",
            "Interior solar absorptance 0.1 on all opaque surfaces (7.2.3.25.1)",
            "Interior solar distribution calculated by the program (7.2.3.25.2,"
            " 7.2.3.10.2.1)",),
        mass="high", int_solar=0.1,
        clauses=dict(C600, int_solar="7.2.3.25.1 interior solar absorptance 0.1"),
    ),
]


# ---------------------------------------------------------------------------
# Materials: (name, k, rho, cp) or (name, R) for massless; layer thicknesses
# ---------------------------------------------------------------------------
LOW_WALL = [("Wood Siding", 0.14, 530.0, 900.0, 0.009),
            ("Fiberglass Quilt", 0.04, 12.0, 840.0, 0.066),
            ("Plasterboard", 0.16, 950.0, 840.0, 0.012)]
HIGH_WALL = [("Wood Siding", 0.14, 530.0, 900.0, 0.009),
             ("Foam Insulation", 0.04, 10.0, 1400.0, 0.0615),
             ("Concrete Block", 0.51, 1400.0, 1000.0, 0.100)]
ROOF = [("Roof Deck", 0.14, 530.0, 900.0, 0.019),
        ("Fiberglass Quilt", 0.04, 12.0, 840.0, 0.1118),
        ("Plasterboard", 0.16, 950.0, 840.0, 0.010)]
LOW_FLOOR = [("Floor Insulation Low Mass", 25.075, 1.003),
             ("Timber Flooring", 0.14, 650.0, 1200.0, 0.025)]
HIGH_FLOOR = [("Floor Insulation High Mass", 25.175, 1.007),
              ("Concrete Slab", 1.13, 1400.0, 1000.0, 0.080)]
# Table 7-36: glass panel / constant-property air gap / glass panel.
HCWE = [("HCWE Outer Panel", 1.00, 2470.0, 750.0, 0.003048),
        ("HCWE Air Gap", 0.0625, 1.292498, 1006.103271, 0.012),
        ("HCWE Inner Panel", 1.00, 2470.0, 750.0, 0.003048)]


def fmt(v):
    return repr(float(v)) if not isinstance(v, str) else v


def material_props(case):
    """{material: dict of YAML properties} for the case. Outer layers carry
    the exterior surface properties, inner layers the interior ones; middle
    layers keep Case 600's 0.6 / 0.9 (they have no surface)."""
    wall = HIGH_WALL if case.mass == "high" else LOW_WALL
    floor = HIGH_FLOOR if case.mass == "high" else LOW_FLOOR
    out = {}

    def add(layer, solar, ir, roughness=None):
        name = layer[0]
        d = {}
        if len(layer) == 3:
            d["thermal_resistance"] = layer[1]
        else:
            d["conductivity"], d["density"], d["specific_heat"] = layer[1:4]
        d["solar_absorptance"] = solar
        d["thermal_absorptance"] = ir
        if roughness:
            d["roughness"] = roughness
        if name in out and out[name] != d:
            raise ValueError(f"{case.num}: {name} used with two property sets")
        out[name] = d

    add(wall[0], case.ext_solar, case.ext_ir, "rough")
    add(wall[1], 0.6, 0.9)
    add(wall[2], case.int_solar, case.int_ir)
    add(ROOF[0], case.ext_solar, case.ext_ir, "rough")
    add(ROOF[1], 0.6, 0.9)
    add(ROOF[2], case.int_solar, case.int_ir)
    # Raised floor: no solar outside (7.2.1.5.1); no in-depth clause changes
    # its exterior solar absorptance, so it keeps Case 600's 0.6.
    add(floor[0], 0.6, case.ext_ir, "rough")
    add(floor[1], case.int_solar, case.int_ir)
    if case.south == "hcwe":
        # 7.2.3.1.4.2: Table 7-34 (as varied by the case) radiative
        # properties; very smooth, as the window glass.
        add(HCWE[0], case.ext_solar, case.ext_ir, "very_smooth")
        add(HCWE[1], 0.6, 0.9)
        add(HCWE[2], case.int_solar, case.int_ir, "very_smooth")
    return out, wall, floor


def material_yaml(props):
    lines = ["materials:"]
    for name, d in props.items():
        lines.append(f"  - name: {name}")
        for k, v in d.items():
            lines.append(f"    {k}: {fmt(v)}")
    return "\n".join(lines)


def construction_yaml(case, wall, floor):
    tag = "Case900" if case.mass == "high" else "Case600"
    cons = [(f"{tag} Wall", wall, "Table 7-27" if case.mass == "high" else "Table 7-2"),
            ("Case600 Roof", ROOF, "Table 7-2"),
            (f"{tag} Floor", floor, "Table 7-27" if case.mass == "high" else "Table 7-2")]
    if case.south == "hcwe":
        cons.append(("High-Conductance Wall Element", HCWE, "Table 7-36"))
    lines = ["constructions:"]
    for name, layers, table in cons:
        lines.append(f"  # {table}, outside to inside")
        lines.append(f"  - name: {name}")
        lines.append("    layers:")
        for layer in layers:
            lines.append(f"      - material: {layer[0]}")
            lines.append(f"        thickness: {fmt(layer[-1])}")
    return "\n".join(lines), cons


GLAZING = """glazing_materials:
  # Clear glass pane of Table 7-10
  - name: Clear Glass
    thickness: 0.003048
    conductivity: 1.0
    solar_transmittance: 0.834
    solar_reflectance_front: 0.075
    solar_reflectance_back: 0.075
    emissivity_front: 0.84
    emissivity_back: 0.84

window_constructions:
  # Table 7-10: two clear panes, 12.0 mm air gap
  - name: Case600 Window
    layers:
      - glazing: Clear Glass
      - gas: air
        width: 0.012
      - glazing: Clear Glass"""


def rect(*pts):
    return "\n".join(f"      - {{x: {x}, y: {y}, z: {z}}}" for x, y, z in pts)


def surface(name, zone, typ, cons, verts, extra=""):
    return (f"  - name: {name}\n    zone: {zone}\n    type: {typ}\n"
            f"    construction: {cons}\n    boundary: outdoor\n{extra}"
            f"    vertices:\n{verts}\n")


def combined(case, kind):
    h_in, h_out = HCOMB[kind]
    s = ""
    if case.inside_combined:
        s += f"    inside_combined_coefficient: {h_in}\n"
    if case.outside_combined:
        s += f"    outside_combined_coefficient: {h_out}\n"
    return s


OVERHANG_630 = """    shading:
      overhang:
        depth: 1.0
        offset_above: 0.5        # overhang at the eave (z = 2.7 m)
      left_fin:
        depth: 1.0
        extend_below: 0.2        # fins from the ground ...
        extend_above: 0.5        # ... to the eave
      right_fin:
        depth: 1.0
        extend_below: 0.2
        extend_above: 0.5
"""


def surfaces_yaml(case):
    zone = f"Case{case.num} Zone"
    tag = "Case900" if case.mass == "high" else "Case600"
    wall, roof, floor = f"{tag} Wall", "Case600 Roof", f"{tag} Floor"
    out = ["surfaces:"]
    out.append(surface("South Wall", zone, "wall", wall,
                       rect((0.0, 0.0, 0.0), (8.0, 0.0, 0.0), (8.0, 0.0, 2.7), (0.0, 0.0, 2.7)),
                       combined(case, "wall")))
    south_openings = [("1", 0.5, 3.5), ("2", 4.5, 7.5)]
    if case.south == "windows":
        for n, x0, x1 in south_openings:
            out.append(surface(
                f"South Window {n}", zone, "window", "Case600 Window",
                rect((x0, 0.0, 0.2), (x1, 0.0, 0.2), (x1, 0.0, 2.2), (x0, 0.0, 2.2)),
                "    parent_surface: South Wall\n" + combined(case, "window")))
    elif case.south == "hcwe":
        # Table 7-35: two 3 m x 2 m units where the Case 600 windows were.
        for n, x0, x1 in south_openings:
            out.append(surface(
                f"High-Conductance Wall Element {n}", zone, "wall",
                "High-Conductance Wall Element",
                rect((x0, 0.0, 0.2), (x1, 0.0, 0.2), (x1, 0.0, 2.2), (x0, 0.0, 2.2)),
                "    parent_surface: South Wall\n" + combined(case, "wall")))
    out.append(surface("North Wall", zone, "wall", wall,
                       rect((8.0, 6.0, 0.0), (0.0, 6.0, 0.0), (0.0, 6.0, 2.7), (8.0, 6.0, 2.7)),
                       combined(case, "wall")))
    out.append(surface("East Wall", zone, "wall", wall,
                       rect((8.0, 0.0, 0.0), (8.0, 6.0, 0.0), (8.0, 6.0, 2.7), (8.0, 0.0, 2.7)),
                       combined(case, "wall")))
    if case.south == "east_west":
        out.append(surface(
            "East Window", zone, "window", "Case600 Window",
            rect((8.0, 1.5, 0.2), (8.0, 4.5, 0.2), (8.0, 4.5, 2.2), (8.0, 1.5, 2.2)),
            "    parent_surface: East Wall\n" + combined(case, "window")
            + (OVERHANG_630 if case.shading == "east_west" else "")))
    out.append(surface("West Wall", zone, "wall", wall,
                       rect((0.0, 6.0, 0.0), (0.0, 0.0, 0.0), (0.0, 0.0, 2.7), (0.0, 6.0, 2.7)),
                       combined(case, "wall")))
    if case.south == "east_west":
        out.append(surface(
            "West Window", zone, "window", "Case600 Window",
            rect((0.0, 4.5, 0.2), (0.0, 1.5, 0.2), (0.0, 1.5, 2.2), (0.0, 4.5, 2.2)),
            "    parent_surface: West Wall\n" + combined(case, "window")
            + (OVERHANG_630 if case.shading == "east_west" else "")))
    out.append(surface("Roof", zone, "roof", roof,
                       rect((0.0, 0.0, 2.7), (8.0, 0.0, 2.7), (8.0, 6.0, 2.7), (0.0, 6.0, 2.7)),
                       combined(case, "roof")))
    # 7.2.1.5.1: air below the raised floor is outdoor air; no solar, no wind.
    out.append(surface("Floor", zone, "floor", floor,
                       rect((0.0, 6.0, 0.0), (8.0, 6.0, 0.0), (8.0, 0.0, 0.0), (0.0, 0.0, 0.0)),
                       "    sun_exposure: false\n    wind_exposure: false\n"
                       + combined(case, "floor")))
    return "\n".join(out).rstrip() + "\n"


def case_yaml(case):
    props, wall, floor = material_props(case)
    cons_text, _ = construction_yaml(case, wall, floor)
    zone = f"Case{case.num} Zone"
    has_windows = case.south in ("windows", "east_west")
    head = [
        f"# OpenBSE ASHRAE Standard 140 — Case {case.num}",
        "#",
        f"# Case {case.num}: {case.title}",
        f"# ASHRAE Standard 140-2023, Section {case.clause} (in-depth test, 7.2.3).",
        f"# Base cases (Informative Table 7-33): {case.bases}.",
        "#",
        f"# Case {'900' if case.mass == 'high' else '600'} except:",
    ]
    for c in case.changes:
        wrapped = textwrap.wrap(c, 74)
        head += [f"#   - {wrapped[0]}"] + [f"#     {w}" for w in wrapped[1:]]
    head += [
        "#",
        "# Generated by 140_tests/scripts/generate_indepth_cases.py; edit there.",
        "# No acceptance ranges (Annex A3 has none for 7.2.3); results reported only.",
        "# Coordinates: X = east, Y = north, Z = up; vertices counter-clockwise",
        "# seen from outside.",
        "",
    ]
    sim = [
        "simulation:",
        "  timesteps_per_hour: 4",
        "  start_month: 1",
        "  start_day: 1",
        "  end_month: 12",
        "  end_day: 31",
        "  terrain: country",
        "  solar_distribution: full_interior_and_exterior",
        "",
        "weather_files:",
        f'  - "{WEATHER}"',
        "",
    ]
    body = [material_yaml(props), "", cons_text, ""]
    if has_windows:
        body += [GLAZING, ""]
    body += [
        "# Output of 7.3.1: annual loads and hourly integrated peaks.",
        "outputs:",
        '  - file: "zone_results.csv"',
        "    frequency: hourly",
        "    variables:",
        "      - zone_temperature",
        "      - zone_heating_rate",
        "      - zone_cooling_rate",
        "      - site_outdoor_temperature",
        "",
        "summary_report: true",
        "",
        "zones:",
        f"  - name: {zone}",
        "",
    ]
    if case.gains > 0:
        body += [
            "equipment:",
            "  - name: Internal Gains",
            f"    zones: [{zone}]",
            f"    power: {fmt(case.gains)}",
            "    fuel: none",
            "    radiant_fraction: 0.6",
            "",
        ]
    if case.infiltration > 0:
        body += [
            "infiltration:",
            "  - name: Zone Infiltration",
            f"    zones: [{zone}]",
            f"    air_changes_per_hour: {fmt(case.infiltration)}",
            "",
        ]
    body += [
        "# Ideal loads: nonproportional thermostat (7.2.1.13.1.2), 1000 kW each,",
        "# 100% convective.",
        "ideal_loads:",
        "  - name: Zone Ideal Loads",
        f"    zones: [{zone}]",
        "    heating_setpoint: 20.0",
        f"    cooling_setpoint: {fmt(case.cooling_setpoint)}",
        "    heating_capacity: 1000000.0",
        "    cooling_capacity: 1000000.0",
        "",
    ]
    if case.shading == "overhang":
        body += [
            "# Case 610 overhang (7.2.2.1.1): 1.0 m deep at the roof line, across",
            "# the whole south wall (7.2.3.11).",
            "shading_surfaces:",
            "  - name: South Overhang",
            "    vertices:",
            "      - {x: 0.0, y:  0.0, z: 2.7}",
            "      - {x: 0.0, y: -1.0, z: 2.7}",
            "      - {x: 8.0, y: -1.0, z: 2.7}",
            "      - {x: 8.0, y:  0.0, z: 2.7}",
            "",
        ]
    return "\n".join(head + sim + body) + "\n" + surfaces_yaml(case)


# ---------------------------------------------------------------------------
# Conformance-spec entries
# ---------------------------------------------------------------------------
def spec_entries(case):
    props, wall, floor = material_props(case)
    _, cons = construction_yaml(case, wall, floor)
    cl = dict(C600)
    cl.update(case.clauses)
    e = []

    def add(path, value, clause):
        e.append({"path": path, "value": value, "clause": clause})

    add("/weather_files/0", WEATHER, "7.2.1.1.1.1 normative weather 725650TY.csv")
    add("/simulation/timesteps_per_hour", 4, "5.1.1.4 consistent modelling methods")
    add("/simulation/solar_distribution", "full_interior_and_exterior",
        "7.2.1.12.1 interior solar distribution; 7.2.3.9.3.1; 5.1.1.4")
    if case.infiltration > 0:
        add("/infiltration/[name=Zone Infiltration]/air_changes_per_hour",
            case.infiltration, cl["infiltration"])
    else:
        add("/infiltration", [], cl["infiltration"])
    if case.gains > 0:
        add("/equipment/[name=Internal Gains]/power", case.gains, cl["gains"])
        add("/equipment/[name=Internal Gains]/radiant_fraction", 0.6,
            cl["gains"] + "; 60% radiative")
        add("/equipment/[name=Internal Gains]/latent_fraction", 0.0,
            cl["gains"] + "; 100% sensible")
    else:
        add("/equipment", [], cl["gains"])

    for cname, layers, table in cons:
        clause = {"Table 7-2": "7.2.1.4 Table 7-2", "Table 7-27": "7.2.2.2.1.1 Table 7-27",
                  "Table 7-36": "7.2.3.1.4.2 Table 7-36"}[table]
        if case.num == "800" and cname == "Case600 Roof":
            clause = "7.2.3.24 roof as Case 430; 7.2.1.4 Table 7-2"
        for i, layer in enumerate(layers):
            add(f"/constructions/[name={cname}]/layers/{i}/material", layer[0], clause)
            add(f"/constructions/[name={cname}]/layers/{i}/thickness", layer[-1], clause)
    for mname, d in props.items():
        for k in ("conductivity", "density", "specific_heat", "thermal_resistance"):
            if k in d:
                table = ("7.2.3.1.4.2 Table 7-36" if mname.startswith("HCWE")
                         else "7.2.2.2.1.1 Table 7-27"
                         if mname in ("Foam Insulation", "Concrete Block",
                                      "Floor Insulation High Mass", "Concrete Slab")
                         else "7.2.1.4 Table 7-2")
                add(f"/materials/[name={mname}]/{k}", d[k], table)
    # Surface radiative properties at the faces the engine reads them from.
    outer = [wall[0][0], ROOF[0][0]] + ([HCWE[0][0]] if case.south == "hcwe" else [])
    inner = [wall[2][0], ROOF[2][0], floor[1][0]] + (
        [HCWE[2][0]] if case.south == "hcwe" else [])
    for m in outer:
        add(f"/materials/[name={m}]/solar_absorptance", case.ext_solar, cl["solar"])
        add(f"/materials/[name={m}]/thermal_absorptance", case.ext_ir,
            cl.get("ext_ir", "7.2.1.8.1 Table 7-5; 7.2.3.1.3 Table 7-34"))
        add(f"/materials/[name={m}]/roughness",
            "very_smooth" if m.startswith("HCWE") else "rough",
            "7.2.3.1.4.2c very smooth" if m.startswith("HCWE") else "7.2.1.9.2 Table 7-6")
    add(f"/materials/[name={floor[0][0]}]/thermal_absorptance", case.ext_ir,
        cl.get("ext_ir", "7.2.1.8.1 Table 7-5; 7.2.3.1.3b Table 7-34"))
    for m in inner:
        add(f"/materials/[name={m}]/solar_absorptance", case.int_solar,
            cl.get("int_solar", "7.2.1.8.1 Table 7-5; 7.2.3.1.3 Table 7-34"))
        add(f"/materials/[name={m}]/thermal_absorptance", case.int_ir,
            cl.get("int_ir", "7.2.1.8.1 Table 7-5; 7.2.3.1.3 Table 7-34"))

    # Surfaces present (what replaced the south windows).
    if case.south == "hcwe":
        for n in ("1", "2"):
            add(f"/surfaces/[name=High-Conductance Wall Element {n}]/construction",
                "High-Conductance Wall Element", "7.2.3.1.4.1 replaces the south windows")
    if case.south in ("windows", "east_west"):
        names = (["South Window 1", "South Window 2"] if case.south == "windows"
                 else ["East Window", "West Window"])
        for n in names:
            add(f"/surfaces/[name={n}]/construction", "Case600 Window",
                "7.2.3.9.2 Case 600 windows" if case.south == "windows"
                else "7.2.3.12.1 Case 620 window orientation")
        add("/glazing_materials/[name=Clear Glass]/thickness", 0.003048, "7.2.1.11.2 Table 7-10")
        add("/glazing_materials/[name=Clear Glass]/conductivity", 1.0, "7.2.1.11.2 Table 7-10")
        add("/glazing_materials/[name=Clear Glass]/solar_transmittance", 0.834,
            "7.2.1.11.2 Table 7-10")
        add("/glazing_materials/[name=Clear Glass]/solar_reflectance_front", 0.075,
            "7.2.1.11.2 Table 7-10")
        add("/glazing_materials/[name=Clear Glass]/solar_reflectance_back", 0.075,
            "7.2.1.11.2 Table 7-10")
        add("/window_constructions/[name=Case600 Window]/layers/1/gas", "air",
            "7.2.1.11.2 Table 7-10")
        add("/window_constructions/[name=Case600 Window]/layers/1/width", 0.012,
            "7.2.1.11.2 Table 7-10")
    if case.shading == "east_west":
        for n in ("East Window", "West Window"):
            add(f"/surfaces/[name={n}]/shading/overhang/depth", 1.0, "7.2.2.1.3 Case 630")
            add(f"/surfaces/[name={n}]/shading/left_fin/depth", 1.0, "7.2.2.1.3 Case 630")
            add(f"/surfaces/[name={n}]/shading/right_fin/depth", 1.0, "7.2.2.1.3 Case 630")
    if case.shading == "overhang":
        add("/shading_surfaces/[name=South Overhang]/vertices/2",
            {"x": 8.0, "y": -1.0, "z": 2.7}, "7.2.3.11; 7.2.2.1.1 full-length 1.0 m overhang")
    add("/surfaces/[name=Floor]/sun_exposure", False, "7.2.1.5.1 no solar on the raised floor")
    add("/surfaces/[name=Floor]/wind_exposure", False, "7.2.1.5.1 zero wind")

    # Table 7-46 combined coefficients.
    kinds = {"South Wall": "wall", "North Wall": "wall", "East Wall": "wall",
             "West Wall": "wall", "Roof": "roof", "Floor": "floor",
             "South Window 1": "window", "South Window 2": "window"}
    for sname, kind in kinds.items():
        h_in, h_out = HCOMB[kind]
        if case.num in ("450", "460"):
            add(f"/surfaces/[name={sname}]/inside_combined_coefficient", h_in,
                f"{case.clause}.1 Table 7-46")
        if case.num in ("450", "470"):
            add(f"/surfaces/[name={sname}]/outside_combined_coefficient", h_out,
                f"{case.clause}.{2 if case.num == '450' else 1} Table 7-46")
        if case.num in ("470",):
            add(f"/surfaces/[name={sname}]/inside_combined_coefficient", None,
                "7.2.3.23 interior as Case 600")
        if case.num in ("460",):
            add(f"/surfaces/[name={sname}]/outside_combined_coefficient", None,
                "7.2.3.22 exterior as Case 600")

    add("/ideal_loads/[name=Zone Ideal Loads]/heating_setpoint", 20.0, cl["thermostat"])
    add("/ideal_loads/[name=Zone Ideal Loads]/cooling_setpoint", case.cooling_setpoint,
        cl["thermostat"])
    add("/ideal_loads/[name=Zone Ideal Loads]/heating_capacity", 1000000.0,
        "7.2.1.13.2 1000 kW")
    add("/ideal_loads/[name=Zone Ideal Loads]/cooling_capacity", 1000000.0,
        "7.2.1.13.2 1000 kW")
    return e


def main():
    with open(SPEC_PATH) as f:
        spec = json.load(f)
    nums = sorted((c.num for c in CASES), key=int)
    entries = {}
    for case in CASES:
        path = os.path.join(CASES_DIR, f"ashrae140_case{case.num}.yaml")
        with open(path, "w", newline="\n") as f:
            f.write(case_yaml(case))
        entries[case.num] = spec_entries(case)
        print(f"wrote {os.path.relpath(path, TESTS)} ({len(entries[case.num])} spec values)")
    # In-depth entries follow the other Section 7 cases, in case order.
    others = {k: v for k, v in spec["cases"].items() if k not in entries}
    ordered = {}
    for k, v in others.items():
        if not k[0].isdigit() and not any(n in ordered for n in nums):
            ordered.update((n, entries[n]) for n in nums)
        ordered[k] = v
    spec["cases"] = ordered
    with open(SPEC_PATH, "w", newline="\n") as f:
        json.dump(spec, f, indent=1, ensure_ascii=False)
        f.write("\n")


if __name__ == "__main__":
    main()
