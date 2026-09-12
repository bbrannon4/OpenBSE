#!/usr/bin/env python3
"""Populate the ASHRAE 140-2023 Section 10 submission template.

Fills Std140_HE_Output.xls with OpenBSE's annual (Jan-Mar) furnace results for
BESTEST cases HE100-HE230, ready for the IBPSA-USA validation portal.

Usage:
    python3 140_tests/scripts/generate_ashrae140_HE_submission.py \\
        --template "~/Downloads/Accompanying FIles/Std140_HE_Files/Normative Materials/Std140_HE_Output.xls" \\
        --output   140_tests/results/OpenBSE_Std140_HE_Output.xls

Dependencies: xlrd, xlwt, xlutils. Run the HE cases first so the *_hvac_results
and *_zone_results CSVs exist.
"""

import argparse
import csv
import os
import sys

try:
    import xlrd
    import xlwt  # noqa: F401
    from xlutils.copy import copy as xl_copy
except ImportError:
    sys.exit("xlrd, xlwt and xlutils are required: pip install xlrd xlwt xlutils")

HERE = os.path.dirname(os.path.abspath(__file__))
CASES_DIR = os.path.join(os.path.dirname(HERE), "cases")
DEFAULT_TEMPLATE = os.path.expanduser(
    "~/Downloads/Accompanying FIles/Std140_HE_Files/Normative Materials/Std140_HE_Output.xls"
)
DEFAULT_OUTPUT = os.path.join(
    os.path.dirname(HERE), "results", "OpenBSE_Std140_HE_Output.xls"
)

CASES = ["HE100", "HE110", "HE120", "HE130", "HE140", "HE150", "HE160", "HE170",
         "HE210", "HE220", "HE230"]
FAN_CASES = ["HE150", "HE160", "HE170", "HE210", "HE220", "HE230"]
TEMP_CASES = ["HE210", "HE220", "HE230"]

GAS_HHV_J_PER_M3 = 38.0e6
RUN_SECONDS = 2160 * 3600

# Template data column is B (0-indexed 1). Blocks start (0-indexed rows):
LOAD_ROW0, INPUT_ROW0, FUEL_ROW0 = 19, 35, 51    # all 11 cases
FAN_ROW0, MEANT_ROW0, MAXT_ROW0, MINT_ROW0 = 67, 78, 86, 94
COL = 1


def extract(case):
    hp = os.path.join(CASES_DIR, f"ashrae140_case{case}_hvac_results.csv")
    zp = os.path.join(CASES_DIR, f"ashrae140_case{case}_zone_results.csv")
    if not os.path.exists(hp):
        return None
    with open(hp) as f:
        hvac = list(csv.DictReader(f))
    if not hvac:
        return None

    def wh_sum(needle):
        keys = [k for k in hvac[0] if needle in k]
        return sum(float(r[keys[0]]) for r in hvac) if keys else 0.0

    load = wh_sum("Gas Furnace:thermal_output") * 3600 / 1e9
    inp = wh_sum("Gas Furnace:fuel_power") * 3600 / 1e9
    out = {
        "load": load,
        "input": inp,
        "fuel": inp * 1e9 / (GAS_HHV_J_PER_M3 * RUN_SECONDS),
        "fan": wh_sum("Circulating Fan:electric_power") / 1000.0,
    }
    if os.path.exists(zp):
        with open(zp) as f:
            z = list(csv.DictReader(f))
        tk = [k for k in z[0] if "temperature" in k]
        if tk and z:
            temps = [float(r[tk[0]]) for r in z]
            out["meanT"] = sum(temps) / len(temps)
            out["maxT"] = max(temps)
            out["minT"] = min(temps)
    return out


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

    data = {c: extract(c) for c in CASES}
    for i, case in enumerate(CASES):
        r = data[case]
        if r is None:
            print(f"  WARNING: no output for {case}", file=sys.stderr)
            continue
        ws.write(LOAD_ROW0 + i, COL, round(r["load"], 3))
        ws.write(INPUT_ROW0 + i, COL, round(r["input"], 3))
        ws.write(FUEL_ROW0 + i, COL, round(r["fuel"], 7))
    for i, case in enumerate(FAN_CASES):
        r = data.get(case)
        if r:
            ws.write(FAN_ROW0 + i, COL, round(r["fan"], 1))
    for i, case in enumerate(TEMP_CASES):
        r = data.get(case)
        if r and "meanT" in r:
            ws.write(MEANT_ROW0 + i, COL, round(r["meanT"], 2))
            ws.write(MAXT_ROW0 + i, COL, round(r["maxT"], 2))
            ws.write(MINT_ROW0 + i, COL, round(r["minT"], 2))

    os.makedirs(os.path.dirname(args.output), exist_ok=True)
    wb.save(args.output)
    print(f"Wrote {sum(1 for c in CASES if data[c])}/{len(CASES)} HE cases to {args.output}")


if __name__ == "__main__":
    main()
