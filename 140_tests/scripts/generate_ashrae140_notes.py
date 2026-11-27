#!/usr/bin/env python3
"""Write the ASHRAE 140-2023 modeling notes (Normative Annex A2, item i).

Annex A2 requires one modeling-notes document per section of tests, written
in the normative S140outNotes.TXT form (Attachment A2.8), alongside the output
spreadsheets. This script fills report blocks A-G of that form for each
section OpenBSE runs and writes OpenBSE_S140outNotes_<section>.txt to
140_tests/results/.

The notes restate what 140_tests/CONFORMANCE.md records (equivalent inputs,
nonspecified inputs, omitted cases and results); change both together. The
form forbids references to other programs, so the notes name none.

Usage:
    python3 140_tests/scripts/generate_ashrae140_notes.py \\
        [--template ".../Std140_TF_Files/Normative Materials/S140outNotes.txt"]
"""

import argparse
import os
import re
import sys

import openbse_release

HERE = os.path.dirname(os.path.abspath(__file__))
RESULTS = os.path.join(os.path.dirname(HERE), "results")
DEFAULT_TEMPLATE = os.path.expanduser(
    "~/Downloads/Accompanying FIles/Std140_TF_Files/Normative Materials/S140outNotes.txt"
)

RULE = "-" * 79


def note(n, title, items):
    """One numbered note: items is a list of (heading, text) pairs."""
    out = [f"NOTE {n} - {title}", RULE]
    for k, (head, text) in enumerate(items, 1):
        out.append(f"{n}.{k} {head}")
        out.extend("    " + line if line else "" for line in text.strip("\n").split("\n"))
        out.append("")
    return "\n".join(out)


def notes(*blocks):
    return "\n\n".join(blocks) if blocks else "NONE"


# --------------------------------------------------------------------------
# A. Software information (same in every section)
# --------------------------------------------------------------------------
def software_items():
    v = openbse_release.version()
    return {
        1: "OpenBSE project, https://github.com/bbrannon4/OpenBSE (open source, AGPL-3.0-only)",
        2: "OpenBSE (Open Building Simulation Engine), command-line engine `openbse`",
        3: f"{v} (git tag v{v}, released {openbse_release.release_date().isoformat()})",
        4: "macOS (Apple silicon and x86_64), Linux x86_64, Windows x86_64",
        5: "About 15 MB for the engine executable",
        6: "1 GB (the Standard 140 cases peak below 0.6 GB)",
        7: "None; the engine is run from a command line and reads and writes text files",
        8: "Inputs are YAML files; results are CSV files. Building from source needs "
           "a Rust toolchain.",
    }


# --------------------------------------------------------------------------
# Shared notes
# --------------------------------------------------------------------------
def timestep_note(n, used):
    return note(n, "Simulation time step", [
        ("Describe the Effect Being Simulated:",
         "Length of the calculation time step for the zone heat balance and HVAC."),
        ("Optional Settings or Modeling Capabilities\n"
         "    (list setting or capability and its physical meaning):",
         "1 to 60 time steps per hour (`simulation: timesteps_per_hour`).\n"
         "    Physical Meaning: the heat balance, controls and equipment are solved\n"
         "    this many times per hour; weather is interpolated within the hour."),
        ("Setting or Capability Used:", used),
    ])


def dx_part_load_note(n, used):
    return note(n, "Part-load operation of the packaged unit", [
        ("Describe the Effect Being Simulated:",
         "How the unit meets a load below its capacity within a time step."),
        ("Optional Settings or Modeling Capabilities\n"
         "    (list setting or capability and its physical meaning):",
         "proportional: the unit runs for the part-load fraction of the time step\n"
         "    needed to meet the load; energy follows the part-load ratio with the\n"
         "    cycling degradation curve given in the input.\n"
         "on_off: the unit runs for the whole time step whenever the zone calls."),
        ("Setting or Capability Used:", used),
    ])


def dx_shr_note(n, used):
    return note(n, "DX coil sensible/latent split", [
        ("Describe the Effect Being Simulated:",
         "Division of the DX coil's total capacity into sensible and latent parts,\n"
         "including dry-coil operation."),
        ("Optional Settings or Modeling Capabilities\n"
         "    (list setting or capability and its physical meaning):",
         "autocalculate_shr true (default): the sensible heat ratio is computed\n"
         "    each time step by the apparatus dew point / bypass factor method, with\n"
         "    the bypass factor calibrated at the rated point (rated capacity, SHR\n"
         "    and airflow); a dry coil gives SHR = 1.\n"
         "autocalculate_shr false: the rated SHR is applied at all conditions.\n"
         "dry_coil_wet_bulb_iteration true (default; with autocalculate_shr): when\n"
         "    the coil is dry, capacity and EIR are read at the entering wet-bulb at\n"
         "    which the coil would just be dry, not at the actual wet-bulb.\n"
         "dry_coil_wet_bulb_iteration false: dry-coil capacity is the capacity at\n"
         "    the actual wet-bulb scaled by its sensible fraction."),
        ("Setting or Capability Used:", used),
    ])


PERFORMANCE_MAP_EQUIVALENT = [
    ("Describe the Effect Being Simulated:",
     "DX unit capacity and power as functions of entering and outdoor conditions."),
    ("Section(s) of the Standard where Relevant Inputs are Specified:", "{clauses}"),
    ("Equivalent Input(s) Used:",
     "Total capacity and EIR entered as tables on entering wet-bulb x outdoor\n"
     "dry-bulb, taken from the gross (fan heat excluded) data of the performance\n"
     "map; the sensible capacity's dependence on entering dry-bulb is computed by\n"
     "the apparatus dew point / bypass factor model."),
    ("Physical, Mathematical or Logical Justification of the Equivalent Input(s)\n"
     "    - provide supporting calculations, if relevant:",
     "In the map, total capacity and power depend on entering wet-bulb and\n"
     "outdoor dry-bulb only, and the sensible capacity's dependence on entering\n"
     "dry-bulb is the bypass relation the program computes. The program has no\n"
     "three-way table input, and {clause_closest} directs the use of the closest\n"
     "match where the program's input form differs from the map."),
]

NONE = "NONE"
NO_SOURCE_CHANGES = "NONE"


# --------------------------------------------------------------------------
# Per-section content
# --------------------------------------------------------------------------
SECTIONS = {
    "7": {
        "B": notes(
            note(1, "Interior solar distribution", [
                ("Describe the Effect Being Simulated:",
                 "Distribution of window-transmitted solar radiation over the zone's\n"
                 "interior surfaces, and exterior shading of the windows."),
                ("Optional Settings or Modeling Capabilities\n"
                 "    (list setting or capability and its physical meaning):",
                 "full_exterior: transmitted beam solar falls on the floor; exterior\n"
                 "    shading of windows is calculated geometrically.\n"
                 "full_interior_and_exterior: transmitted beam solar is projected\n"
                 "    geometrically onto the interior surfaces it strikes; exterior\n"
                 "    shading as above."),
                ("Setting or Capability Used:",
                 "full_interior_and_exterior in every Section 7 case (5.1.1.4)."),
            ]),
            note(2, "Exterior surface convection", [
                ("Describe the Effect Being Simulated:",
                 "Convective heat transfer at exterior surfaces, varying with wind\n"
                 "speed, surface roughness and surface-to-air temperature difference."),
                ("Optional Settings or Modeling Capabilities\n"
                 "    (list setting or capability and its physical meaning):",
                 "doe2: natural convection plus a wind-speed forced term scaled by the\n"
                 "    surface roughness.\n"
                 "tarp: natural convection plus a forced term that also falls with the\n"
                 "    surface's size."),
                ("Setting or Capability Used:", "doe2 in every Section 7 case."),
            ]),
            timestep_note(3, "4 time steps per hour in every Section 7 case."),
            note(4, "Surface heat transfer coefficients", [
                ("Describe the Effect Being Simulated:",
                 "Convective and long-wave heat transfer at interior and exterior\n"
                 "surfaces."),
                ("Optional Settings or Modeling Capabilities\n"
                 "    (list setting or capability and its physical meaning):",
                 "Calculated (default): convection coefficients varying with each time\n"
                 "    step, and long-wave exchange with the sky, air and ground outside\n"
                 "    and among the zone surfaces inside.\n"
                 "inside_combined_coefficient / outside_combined_coefficient: a constant\n"
                 "    combined (convective plus long-wave) coefficient for a face, to the\n"
                 "    adjacent air temperature, replacing both."),
                ("Setting or Capability Used:",
                 "Calculated in every Section 7 case except Cases 450, 460 and 470, which\n"
                 "use the Table 7-46 combined coefficients as 7.2.3.21-7.2.3.23 require:\n"
                 "interior and exterior in Case 450, interior only in Case 460, exterior\n"
                 "only in Case 470."),
            ]),
        ),
        "C": NONE,
        "D": NONE,
        "E": NONE,
        "F": NO_SOURCE_CHANGES,
        "G": NONE,
    },
    "9A": {
        "B": notes(
            dx_part_load_note(1, "proportional, with cycling degradation coefficient 0.229\n"
                                 "    (CDF = 1 - 0.229 (1 - PLR)), in every case."),
            dx_shr_note(2, "autocalculate_shr and dry_coil_wet_bulb_iteration, both true, in\n"
                           "    every case."),
            timestep_note(3, "1 time step per hour in every case."),
        ),
        "C": notes(note(1, "Performance map form", [
            (h, t.format(clauses="9.2.1.10 and Tables 9-6 (CE100MAP and the case maps)",
                         clause_closest="9.2.1.10.3"))
            for h, t in PERFORMANCE_MAP_EQUIVALENT
        ])),
        "D": NONE,
        "E": NONE,
        "F": NO_SOURCE_CHANGES,
        "G": NONE,
    },
    "9B": {
        "B": notes(
            dx_part_load_note(1, "proportional, with cycling degradation coefficient 0.229\n"
                                 "    (CDF = 1 - 0.229 (1 - PLR)), in every case."),
            dx_shr_note(2, "autocalculate_shr and dry_coil_wet_bulb_iteration, both true, in\n"
                           "    every case."),
            timestep_note(3, "4 time steps per hour in every case."),
        ),
        "C": notes(note(1, "Performance map form", [
            (h, t.format(clauses="9.2.3.10 (CE300MAP and the case maps)",
                         clause_closest="9.2.1.10.3"))
            for h, t in PERFORMANCE_MAP_EQUIVALENT
        ])),
        "D": NONE,
        "E": NONE,
        "F": NO_SOURCE_CHANGES,
        "G": notes(note(1, "Zone humidity in the dry-coil cases CE530, CE540 and CE545", [
            ("Test Case(s) and Specific Results from the Case(s) that are Anomalous:",
             "CE530, CE540, CE545: zone humidity ratio and relative humidity, and the\n"
             "energy and COP results that depend on the coil's entering humidity."),
            ("Explanation of Reason for the Anomalous Results:",
             "These cases have no infiltration for the entire simulation period\n"
             "(9.2.3.5) and no latent gains, and the coil stays dry, so the zone air\n"
             "has no moisture source or sink and keeps its initial humidity ratio all\n"
             "year. The Standard specifies no initial zone state for Class I tests\n"
             "(5.1.1.7.2). The program starts the zone air at the first hour's outdoor\n"
             "dry-bulb and humidity ratio, which for the CE300 weather file is\n"
             "0.0029 kg/kg, and preconditions by repeating the first week. Results\n"
             "that depend on zone humidity therefore reflect that start state."),
        ])),
    },
    "10": {
        "B": notes(
            note(1, "Furnace part-load operation", [
                ("Describe the Effect Being Simulated:",
                 "Furnace fuel use and fan operation when the load is below capacity."),
                ("Optional Settings or Modeling Capabilities\n"
                 "    (list setting or capability and its physical meaning):",
                 "on_off cycling: the furnace and its fan run for the part-load fraction\n"
                 "    of the time step needed to meet the load.\n"
                 "plf_curve: part-load fraction as a function of part-load ratio.\n"
                 "hir_curve: heat input ratio as a function of part-load ratio."),
                ("Setting or Capability Used:",
                 "on_off cycling with hir_curve set to the Standard's HIR(PLR)\n"
                 "    correlation, in every case."),
            ]),
            timestep_note(2, "1 time step per hour in every case."),
        ),
        "C": NONE,
        "D": NONE,
        "E": NONE,
        "F": NO_SOURCE_CHANGES,
        "G": NONE,
    },
    "11": {
        "B": notes(
            note(1, "Chilled-water cooling coil", [
                ("Describe the Effect Being Simulated:",
                 "Cooling and dehumidification of the supply air by the cooling coil."),
                ("Optional Settings or Modeling Capabilities\n"
                 "    (list setting or capability and its physical meaning):",
                 "Coil on a plant loop: a counterflow coil, dry or wet by the entering\n"
                 "    dew point, sized to its design point; its valve holds the leaving\n"
                 "    air setpoint and the leaving state follows from the coil UA.\n"
                 "Coil with no plant loop: an ideal coil that cools the air to the\n"
                 "    setpoint, limited by its capacity, and condenses down to saturation\n"
                 "    at the leaving temperature when the entering air is wetter."),
                ("Setting or Capability Used:",
                 "Coil with no plant loop (ideal coil) in every case with cooling."),
            ]),
            timestep_note(2, "1 time step per hour in every case."),
        ),
        "C": notes(
            note(1, "Outdoor air replacing exhaust air", [
                ("Describe the Effect Being Simulated:",
                 "Outdoor air drawn by the air system to replace the zone exhaust."),
                ("Section(s) of the Standard where Relevant Inputs are Specified:",
                 "11.2.1.1.2.2 and 11.2.1.1.2.3"),
                ("Equivalent Input(s) Used:",
                 "Minimum outdoor-air fraction of 1/3 of the supply volume flow."),
                ("Physical, Mathematical or Logical Justification of the Equivalent Input(s)\n"
                 "    - provide supporting calculations, if relevant:",
                 "11.2.1.1.2.3 permits the program's own outdoor-air-replaces-exhaust\n"
                 "assumption in place of matching outdoor-air mass to exhaust mass."),
            ]),
            note(2, "Supply fan efficiency", [
                ("Describe the Effect Being Simulated:",
                 "Supply fan power and the fan heat added to the airstream."),
                ("Section(s) of the Standard where Relevant Inputs are Specified:",
                 "11.2.1.1.1.1"),
                ("Equivalent Input(s) Used:",
                 "498 Pa, impeller efficiency 0.7, motor efficiency 1.0 with the motor\n"
                 "in the airstream."),
                ("Physical, Mathematical or Logical Justification of the Equivalent Input(s)\n"
                 "    - provide supporting calculations, if relevant:",
                 "11.2.1.1.1.1 permits a combined efficiency of 0.7 with all fan heat\n"
                 "in the airstream. Fan power = flow x 498 Pa / 0.7 in both forms, and\n"
                 "all of it enters the air."),
            ]),
        ),
        "D": NONE,
        "E": NONE,
        "F": NO_SOURCE_CHANGES,
        "G": NONE,
    },
}


# --------------------------------------------------------------------------
# Template filling
# --------------------------------------------------------------------------
HEADER = re.compile(r"^={20,}\n([A-G])\. ", re.M)


def fill(template, content):
    """Fill item answers in block A and replace the sample notes in B-G."""
    sw = software_items()

    def answer_a(m):
        return f"{m.group(1)}{m.group(2)}: {sw[int(m.group(1))]}"
    # Block A items "N. TITLE:" (N = 1..8) at line start.
    text = re.sub(r"^([1-8])(\. [A-Z][^:\n]+):[ \t]*$", answer_a, template, flags=re.M)

    # Blocks B-G: the sample note runs from "NOTE 1 - <Title>" to the next rule.
    starts = [(m.group(1), m.start()) for m in HEADER.finditer(text)]
    out, pos = [], 0
    for i, (block, start) in enumerate(starts):
        end = starts[i + 1][1] if i + 1 < len(starts) else text.index("<END OF DOCUMENT>")
        if block == "A":
            continue
        seg = text[start:end]
        k = seg.find("NOTE 1 - <Title>")
        if k < 0:
            sys.exit(f"block {block}: sample note not found in template")
        tail = re.search(r"\n={20,}\n", seg[k:])
        stop = k + tail.start() if tail else len(seg)
        out.append(text[pos:start + k])
        out.append(content[block].rstrip() + "\n\n\n")
        pos = start + stop + 1
    out.append(text[pos:])
    return "".join(out)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--template", default=DEFAULT_TEMPLATE)
    ap.add_argument("--output-dir", default=RESULTS)
    args = ap.parse_args()
    if not os.path.exists(args.template):
        sys.exit(f"template not found: {args.template}")
    with open(args.template, encoding="latin-1") as f:
        template = f.read().replace("\r\n", "\n")
    os.makedirs(args.output_dir, exist_ok=True)
    for section, content in SECTIONS.items():
        path = os.path.join(args.output_dir, f"OpenBSE_S140outNotes_{section}.txt")
        with open(path, "w", encoding="latin-1", newline="\n") as f:
            f.write(fill(template, content))
        print(f"wrote {path}")


if __name__ == "__main__":
    main()
