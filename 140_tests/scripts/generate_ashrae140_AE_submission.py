#!/usr/bin/env python3
"""Fill the ASHRAE 140-2023 Section 11 (AE) submission spreadsheet for OpenBSE.

Populates the coil-loads summary tables of Std140_AE_Output.xlsx — the graded
last-hour outputs — for all four air-system series:

    AE100  Fan-coil air system            (rows 61-63)
    AE200  Single-zone air system         (rows 89-95)
    AE300  Constant-volume terminal reheat (rows 137-143)
    AE400  Variable air volume            (rows 185-191)

For each case it writes the imposed zone loads, the central coil loads (heating /
preheat and cooling sensible/latent/total), the per-zone reheat loads (AE300/400),
and the cooling-coil leaving relative humidity RHcco, all read from the last hour
of the case's *_hvac_results.csv. The detailed node-state tables (S-columns) are
left for a later pass.

Run the AE cases first, then:
    python3 140_tests/scripts/generate_ashrae140_AE_submission.py

Dependencies: openpyxl (pip install openpyxl).
"""

import argparse
import csv
import math
import os
import sys

try:
    import openpyxl
except ImportError:
    sys.exit("openpyxl is required: pip install openpyxl")

HERE = os.path.dirname(os.path.abspath(__file__))
CASES_DIR = os.path.join(os.path.dirname(HERE), "cases")
DEFAULT_TEMPLATE = os.path.expanduser(
    "~/Downloads/Accompanying FIles/Std140_AE_Files/Normative Materials/Std140_AE_Output.xlsx"
)
DEFAULT_OUTPUT = os.path.join(
    os.path.dirname(HERE), "results", "OpenBSE_Std140_AE_Output.xlsx"
)

PROGRAM_NAME = "OpenBSE"
PROGRAM_VERSION = "0.6.0"

# Imposed latent gain is identical across the series (586.1 W zone 1); zone 2 in
# the two-zone cases adds 879.2 W. Values below are per-case imposed loads [kW].
KWH = 1.0 / 1000.0  # W → kW (steady state: kWh/h == kW)


def psat_pa(t_c):
    """Saturation vapor pressure [Pa] over water (ASHRAE 2017 Ch.1 Eq.6)."""
    t = t_c + 273.15
    return math.exp(
        -5800.2206 / t
        + 1.3914993
        - 0.048640239 * t
        + 4.1764768e-5 * t * t
        - 1.4452093e-8 * t * t * t
        + 6.5459673 * math.log(t)
    )


def rh_percent(t_c, w, p=101325.0):
    """Relative humidity [%] from dry-bulb [°C] and humidity ratio [kg/kg]."""
    if w <= 0:
        return 0.0
    pw = p * w / (0.62198 + w)
    return max(0.0, min(100.0, 100.0 * pw / psat_pa(t_c)))


def last_row(case):
    fp = os.path.join(CASES_DIR, f"ashrae140_case{case}_hvac_results.csv")
    if not os.path.exists(fp):
        return None
    rows = list(csv.DictReader(open(fp)))
    return rows[-1] if rows else None


def col(row, needle):
    keys = [k for k in row if needle in k]
    return float(row[keys[0]]) if keys else 0.0


# ── Single-coil series (fan-coil AE100, single-zone AE200) ────────────────────
# (case, imposed_zone_sensible_W, imposed_zone_latent_W)
SINGLE = {
    "AE101": (-2931.0, 586.1), "AE103": (1465.0, 586.1), "AE104": (2931.0, 586.1),
    "AE201": (-2931.0, 586.1), "AE203": (1465.0, 586.1), "AE204": (2931.0, 586.1),
    "AE205": (1465.0, 586.1), "AE206": (1465.0, 586.1), "AE226": (1465.0, 586.1),
    "AE245": (1465.0, 586.1),
}
SINGLE_ROWS = {
    "AE101": 61, "AE103": 62, "AE104": 63,
    "AE201": 89, "AE203": 90, "AE204": 91, "AE205": 92, "AE206": 93,
    "AE226": 94, "AE245": 95,
}

# ── Two-zone reheat series (AE300 CV, AE400 VAV) ──────────────────────────────
# (case, z1_sensible_W, z1_latent_W, z2_sensible_W, z2_latent_W)
REHEAT = {
    "AE301": (-2931.0, 586.1, -2345.0, 879.2), "AE303": (1465.0, 586.1, 2345.0, 879.2),
    "AE304": (2931.0, 586.1, 3517.0, 879.2), "AE305": (1465.0, 586.1, 2345.0, 879.2),
    "AE306": (1465.0, 586.1, 2345.0, 879.2), "AE326": (1465.0, 586.1, 2345.0, 879.2),
    "AE345": (1465.0, 586.1, 2345.0, 879.2),
    "AE401": (-2931.0, 586.1, -2345.0, 879.2), "AE403": (1465.0, 586.1, 2345.0, 879.2),
    "AE404": (2931.0, 586.1, 3517.0, 879.2), "AE405": (1465.0, 586.1, 2345.0, 879.2),
    "AE406": (1465.0, 586.1, 2345.0, 879.2), "AE426": (1465.0, 586.1, 2345.0, 879.2),
    "AE445": (1465.0, 586.1, 2345.0, 879.2),
}
REHEAT_ROWS = {
    "AE301": 137, "AE303": 138, "AE304": 139, "AE305": 140, "AE306": 141,
    "AE326": 142, "AE345": 143,
    "AE401": 185, "AE403": 186, "AE404": 187, "AE405": 188, "AE406": 189,
    "AE426": 190, "AE445": 191,
}


def fill_single(ws, case, row):
    r = last_row(case)
    if r is None:
        print(f"  skip {case}: no results")
        return
    sens, lat = SINGLE[case]
    qh = col(r, "Heating Coil:thermal_output") * KWH
    qcs = col(r, "Cooling Coil:sensible_load") * KWH
    qcl = col(r, "Cooling Coil:latent_load") * KWH
    qct = col(r, "Cooling Coil:total_load") * KWH
    # RHcco: cooling-coil leaving state, when the cooling coil is active
    cco_t = col(r, "Cooling Coil:outlet_temperature")
    cco_w = col(r, "Cooling Coil:outlet_humidity_ratio")
    rhcco = rh_percent(cco_t, cco_w) if qct > 1.0 else 0.0
    ws.cell(row, 3, round(max(0.0, -sens) * KWH, 4))   # C QZHsensible
    ws.cell(row, 4, round(max(0.0, sens) * KWH, 4))    # D QZCsensible
    ws.cell(row, 5, round(lat * KWH, 4))               # E QZlatent
    ws.cell(row, 6, round(qh, 4))                       # F QH
    ws.cell(row, 7, round(qcs, 4))                      # G QCsensible
    ws.cell(row, 8, round(qcl, 4))                      # H QClatent
    ws.cell(row, 9, round(qct, 4))                      # I QCtotal
    ws.cell(row, 10, round(rhcco, 2))                   # J RHcco
    print(f"  {case}: QH={qh:.3f} QCs={qcs:.3f} QCl={qcl:.3f} QCt={qct:.3f} RHcco={rhcco:.1f}")


def fill_reheat(ws, case, row):
    r = last_row(case)
    if r is None:
        print(f"  skip {case}: no results")
        return
    z1s, z1l, z2s, z2l = REHEAT[case]
    qhpre = col(r, "Preheat Coil:thermal_output") * KWH
    qcs = col(r, "Cooling Coil:sensible_load") * KWH
    qcl = col(r, "Cooling Coil:latent_load") * KWH
    qct = col(r, "Cooling Coil:total_load") * KWH
    qh1 = col(r, "Z1 Reheat:thermal_output") * KWH
    qh2 = col(r, "Z2 Reheat:thermal_output") * KWH
    cco_t = col(r, "Cooling Coil:outlet_temperature")
    cco_w = col(r, "Cooling Coil:outlet_humidity_ratio")
    rhcco = rh_percent(cco_t, cco_w) if qct > 1.0 else 0.0
    ws.cell(row, 3, round(max(0.0, -z1s) * KWH, 4))    # C QZH1sensible
    ws.cell(row, 4, round(max(0.0, z1s) * KWH, 4))     # D QZC1sensible
    ws.cell(row, 5, round(z1l * KWH, 4))               # E QZ1latent
    ws.cell(row, 6, round(max(0.0, -z2s) * KWH, 4))    # F QZH2sensible
    ws.cell(row, 7, round(max(0.0, z2s) * KWH, 4))     # G QZC2sensible
    ws.cell(row, 8, round(z2l * KWH, 4))               # H QZ2latent
    ws.cell(row, 9, round(qhpre, 4))                   # I QHpreheat
    ws.cell(row, 10, round(qcs, 4))                    # J QCsensible
    ws.cell(row, 11, round(qcl, 4))                    # K QClatent
    ws.cell(row, 12, round(qct, 4))                    # L QCtotal
    ws.cell(row, 13, round(rhcco, 2))                  # M RHcco
    ws.cell(row, 14, round(qh1, 4))                    # N QH1reheat
    ws.cell(row, 15, round(qh2, 4))                    # O QH2reheat
    print(f"  {case}: QHpre={qhpre:.3f} QCs={qcs:.3f} QCl={qcl:.3f} QCt={qct:.3f} "
          f"reheat={qh1:.2f}/{qh2:.2f}")


def main():
    ap = argparse.ArgumentParser(description="Fill the Std 140 AE submission spreadsheet.")
    ap.add_argument("--template", default=DEFAULT_TEMPLATE)
    ap.add_argument("--output", default=DEFAULT_OUTPUT)
    args = ap.parse_args()

    if not os.path.exists(args.template):
        sys.exit(f"template not found: {args.template}")
    wb = openpyxl.load_workbook(args.template)
    ws = wb["A"]

    # Program identity (C50..C52)
    ws.cell(50, 3, f"{PROGRAM_NAME} {PROGRAM_VERSION}")

    print("Fan-coil + single-zone series:")
    for case, row in SINGLE_ROWS.items():
        fill_single(ws, case, row)
    print("Terminal-reheat series (AE300 CV / AE400 VAV):")
    for case, row in REHEAT_ROWS.items():
        fill_reheat(ws, case, row)

    os.makedirs(os.path.dirname(args.output), exist_ok=True)
    wb.save(args.output)
    print(f"\nWrote {args.output}")


if __name__ == "__main__":
    main()
