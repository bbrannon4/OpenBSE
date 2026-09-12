#!/usr/bin/env python3
"""Populate the ASHRAE 140-2023 Section 9a submission template.

Fills the normative Std140_CE_a_Output.xls template with OpenBSE's February
totals for HVAC BESTEST cooling-equipment cases CE100-CE200, ready for the
IBPSA-USA validation portal.

Usage:
    python3 140_tests/scripts/generate_ashrae140_CE_submission.py \\
        --template "~/Downloads/Accompanying FIles/Std140_CE_a_Files/Normative Materials/Std140_CE_a_Output.xls" \\
        --output   140_tests/results/OpenBSE_Std140_CE_a_Output.xls

Dependencies: xlrd, xlwt, xlutils (pip install xlrd xlwt xlutils).

Run the CE cases first so the *_hvac_results.csv / *_zone_results.csv outputs
exist:  for c in CE100 ... CE200; do openbse ashrae140_case$c.yaml; done
"""

import argparse
import csv
import os
import statistics
import sys

try:
    import xlrd
    import xlwt  # noqa: F401  (needed by xlutils.copy)
    from xlutils.copy import copy as xl_copy
except ImportError:
    sys.exit("xlrd, xlwt and xlutils are required: pip install xlrd xlwt xlutils")

HERE = os.path.dirname(os.path.abspath(__file__))
CASES_DIR = os.path.join(os.path.dirname(HERE), "cases")
DEFAULT_TEMPLATE = os.path.expanduser(
    "~/Downloads/Accompanying FIles/Std140_CE_a_Files/Normative Materials/Std140_CE_a_Output.xls"
)
DEFAULT_OUTPUT = os.path.join(
    os.path.dirname(HERE), "results", "OpenBSE_Std140_CE_a_Output.xls"
)

CASES = ["CE100", "CE110", "CE120", "CE130", "CE140", "CE150", "CE160", "CE165",
         "CE170", "CE180", "CE185", "CE190", "CE195", "CE200"]

# Template column layout (0-indexed): B..N on the "A" sheet, data rows 25-38.
FIRST_DATA_ROW = 24  # xlwt 0-indexed row for template row 25 (CE100)
COLS = {  # metric -> 0-indexed column
    "cool_total": 1, "compressor": 2, "supply_fan": 3, "cond_fan": 4,
    "coil_total": 5, "coil_sens": 6, "coil_lat": 7,
    "zone_total": 8, "zone_sens": 9, "zone_lat": 10,
    "cop": 11, "idb": 12, "humrat": 13,
}


def _feb(path):
    if not os.path.exists(path):
        return []
    with open(path) as f:
        return [r for r in csv.DictReader(f) if int(r["Month"]) == 2]


def _sum(rows, needle):
    if not rows:
        return 0.0
    ks = [k for k in rows[0] if needle in k]
    return sum(float(r[ks[0]]) for r in rows) / 1000.0 if ks else 0.0


def _mean(rows, needle):
    if not rows:
        return None
    ks = [k for k in rows[0] if needle in k]
    return statistics.mean(float(r[ks[0]]) for r in rows) if ks else None


def extract(case):
    hvac = _feb(os.path.join(CASES_DIR, f"ashrae140_case{case}_hvac_results.csv"))
    zone = _feb(os.path.join(CASES_DIR, f"ashrae140_case{case}_zone_results.csv"))
    if not hvac:
        return None
    comp = _sum(hvac, "compressor_power")
    cond = _sum(hvac, "condenser_fan_power")
    sfan = _sum(hvac, "Supply Fan:electric_power")
    coil_total = _sum(hvac, "total_load")
    coil_sens = _sum(hvac, "sensible_load")
    coil_lat = _sum(hvac, "latent_load")
    zone_sens = coil_sens - sfan  # draw-through supply-fan heat is sensible
    zone_lat = coil_lat
    zone_total = zone_sens + zone_lat
    cool_total = comp + cond + sfan
    return {
        "cool_total": cool_total, "compressor": comp, "supply_fan": sfan,
        "cond_fan": cond, "coil_total": coil_total, "coil_sens": coil_sens,
        "coil_lat": coil_lat, "zone_total": zone_total, "zone_sens": zone_sens,
        "zone_lat": zone_lat,
        "cop": zone_total / cool_total if cool_total > 0 else 0.0,
        "idb": _mean(zone, "temperature"),
        "humrat": _mean(zone, "humidity_ratio"),
    }


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--template", default=DEFAULT_TEMPLATE)
    ap.add_argument("--output", default=DEFAULT_OUTPUT)
    args = ap.parse_args()

    if not os.path.exists(args.template):
        sys.exit(f"Template not found: {args.template}")

    rb = xlrd.open_workbook(args.template, formatting_info=True)
    wb = xl_copy(rb)
    ws = wb.get_sheet(0)

    written = 0
    for i, case in enumerate(CASES):
        r = extract(case)
        if r is None:
            print(f"  WARNING: no output for {case} — leaving blank", file=sys.stderr)
            continue
        row = FIRST_DATA_ROW + i
        for metric, col in COLS.items():
            v = r[metric]
            if v is None:
                continue
            ws.write(row, col, round(v, 5 if metric == "humrat" else 3))
        written += 1

    os.makedirs(os.path.dirname(args.output), exist_ok=True)
    wb.save(args.output)
    print(f"Wrote {written}/{len(CASES)} CE cases to {args.output}")


if __name__ == "__main__":
    main()
