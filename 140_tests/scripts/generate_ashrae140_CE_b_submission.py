#!/usr/bin/env python3
"""Populate the ASHRAE 140-2023 Section 9B submission template.

Fills the normative Std140_CE_b_Output.xls template with OpenBSE's results for
the dynamic HVAC BESTEST cooling-equipment cases CE300-CE545, ready for the
IBPSA-USA validation portal.

Usage:
    python3 140_tests/scripts/generate_ashrae140_CE_b_submission.py \\
        --template "~/Downloads/Accompanying FIles/Std140_CE_b_Files/Normative Materials/Std140_CE_b_Output.xls" \\
        --output   140_tests/results/OpenBSE_Std140_CE_b_Output.xls

Dependencies: xlrd, xlwt, xlutils (pip install xlrd xlwt xlutils).

Run the CE_b cases first so the *_hvac_results.csv / *_zone_results.csv /
*_site_results.csv outputs exist.

Template layout (sheet "A", Excel addresses):
    B62..L82    annual sums and means, 21 rows (CE500 twice: full year and
                May-Sep; CE510 for May-Sep only)
    M62..N62    CE300 annual mean outdoor dry-bulb and humidity ratio
    Q62..AB81   hourly-integrated maxima: consumption, then sensible, latent
                and total coil load, each as value / date / hour
    AC62..AH62  CE300 weather check: maximum outdoor dry-bulb, humidity ratio
    B89..L112   CE300 hourly output for June 28
    Q89..AN108  hourly-integrated maxima and minima of COP2, IDB, zone
                humidity ratio and relative humidity
    B120..L121  CE500 average daily outputs for April 30 and June 25
    B129..L130  CE530 average daily outputs for April 30 and June 25
"""

import argparse
import datetime
import os
import sys

try:
    import xlrd
    import xlwt
    from xlutils.copy import copy as xl_copy
except ImportError:
    sys.exit("xlrd, xlwt and xlutils are required: pip install xlrd xlwt xlutils")

from ce_b_results import ANNUAL_ROWS, CASES, CaseOutput

# Program identity for the submission header — kept identical to the Section 9a
# submission (generate_ashrae140_CE_submission.py). Edit ORG / dates as needed.
PROGRAM_NAME = "OpenBSE 0.7.0"
PROGRAM_SHORT = "OpenBSE"
ORG_LONG = "OpenBSE"
ORG_SHORT = "OpenBSE"
VERSION_RELEASE_DATE = datetime.date(2026, 10, 11)
SUBMISSION_DATE = datetime.date(2026, 10, 11)

HERE = os.path.dirname(os.path.abspath(__file__))
DEFAULT_TEMPLATE = os.path.expanduser(
    "~/Downloads/Accompanying FIles/Std140_CE_b_Files/Normative Materials/Std140_CE_b_Output.xls"
)
DEFAULT_OUTPUT = os.path.join(
    os.path.dirname(HERE), "results", "OpenBSE_Std140_CE_b_Output.xls"
)

# 0-indexed rows / columns on sheet "A".
ROW_ANNUAL = 61      # row 62
ROW_EXTREMES = 88    # row 89
ROW_JUNE28 = 88      # row 89
ROW_CE500_DAILY = 119  # row 120
ROW_CE530_DAILY = 128  # row 129
ANNUAL_COLS = ["cool_total", "compressor", "cond_fan", "supply_fan", "coil_total",
               "coil_sens", "coil_lat", "cop2", "idb", "humrat", "rh"]  # B..L
DAILY_COLS = ["cool_total", "compressor", "cond_fan", "supply_fan", "coil_total",
              "coil_sens", "coil_lat", "humrat", "cop2", "odb", "edb"]  # B..L
COL_B, COL_M, COL_Q, COL_AC = 1, 12, 16, 28

# Dates are written as real date cells shown as "20-Jul". The year is
# arbitrary (the weather year is a TMY); any non-leap year works.
DATE_YEAR = 2010
DATE_STYLE = xlwt.easyxf(num_format_str="DD-MMM")
DIGITS = {"cop2": 4, "idb": 3, "humrat": 6, "rh": 3, "odb": 3, "edb": 3}

# For the cycling-fan 500 series the standard takes these extremes from
# April 1 - December 31, skipping the idle winter start.
APR_DEC_CASES = {"CE500", "CE510", "CE520", "CE522", "CE525", "CE530", "CE540", "CE545"}


def put(ws, row, col, value, digits):
    ws.write(row, col, round(value, digits))


def put_extreme(ws, row, col, extreme, digits):
    """Write value / date / hour into three adjacent cells."""
    value, month, day, hour = extreme
    ws.write(row, col, round(value, digits))
    ws.write(row, col + 1, datetime.date(DATE_YEAR, month, day), DATE_STYLE)
    ws.write(row, col + 2, hour)


def write_identity(ws):
    # Labels in column F, values in column J, as in the Section 9a submission.
    date_style = xlwt.easyxf(num_format_str="M/D/YYYY")
    ws.write(0, 5, "Program Name and Version (with full build detail):")  # F1
    ws.write(1, 9, PROGRAM_NAME)                                           # J2
    ws.write(2, 5, "Program Version Release Date:")                        # F3
    ws.write(2, 9, VERSION_RELEASE_DATE, date_style)                       # J3
    ws.write(3, 5, "Program Name for Tables and Charts (short):")          # F4
    ws.write(3, 9, PROGRAM_SHORT)                                          # J4
    ws.write(4, 5, "Results Submission Date:")                             # F5
    ws.write(4, 9, SUBMISSION_DATE, date_style)                            # J5
    ws.write(5, 5, "Modeler Organization for Titles (long):")              # F6
    ws.write(6, 9, ORG_LONG)                                               # J7
    ws.write(7, 5, "Modeler Organization for Tables and Charts (short):")  # F8
    ws.write(7, 9, ORG_SHORT)                                              # J8
    ws.write(17, 0, "Program Name:")                                       # A18
    ws.write(17, 2, PROGRAM_NAME)                                          # C18


def write_annual(ws, outputs):
    for i, (label, case, months) in enumerate(ANNUAL_ROWS):
        out = outputs.get(case)
        if out is None:
            continue
        totals = out.totals(out.index(months=months))
        for j, key in enumerate(ANNUAL_COLS):
            put(ws, ROW_ANNUAL + i, COL_B + j, totals[key], DIGITS.get(key, 2))


def write_maxima(ws, outputs):
    """Q62..AB81: hourly maxima of consumption and coil loads [Wh]."""
    for i, case in enumerate(CASES):
        out = outputs.get(case)
        if out is None:
            continue
        row = ROW_ANNUAL + i
        for k, series in enumerate([out.elec, out.q_sens, out.q_lat, out.q_tot]):
            put_extreme(ws, row, COL_Q + 3 * k, out.extreme(series), 2)


def write_extremes(ws, outputs):
    """Q89..AN108: hourly maxima and minima of COP2 and zone conditions."""
    for i, case in enumerate(CASES):
        out = outputs.get(case)
        if out is None:
            continue
        row = ROW_EXTREMES + i
        late = out.index(from_month=4) if case in APR_DEC_CASES else None
        # (series, hours for the maximum, hours for the minimum, digits)
        blocks = [
            (out.cop2, None, None, 4),
            (out.idb, None, late, 3),
            (out.w, None, late, 6),
            (out.rh, late, late, 3),
        ]
        for k, (series, max_idx, min_idx, digits) in enumerate(blocks):
            col = COL_Q + 6 * k
            put_extreme(ws, row, col, out.extreme(series, max_idx, largest=True), digits)
            put_extreme(ws, row, col + 3, out.extreme(series, min_idx, largest=False), digits)


def write_ce300_extras(ws, out):
    # M62..N62: annual mean outdoor dry-bulb and humidity ratio.
    put(ws, ROW_ANNUAL, COL_M, sum(out.odb) / out.n, 3)
    put(ws, ROW_ANNUAL, COL_M + 1, sum(out.out_w) / out.n, 6)
    # AC62..AH62: weather-check maxima.
    put_extreme(ws, ROW_ANNUAL, COL_AC, out.extreme(out.odb), 3)
    put_extreme(ws, ROW_ANNUAL, COL_AC + 3, out.extreme(out.out_w), 6)
    # B89..L112: June 28 hourly output.
    for h, i in enumerate(out.day_index(6, 28)):
        row = ROW_JUNE28 + h
        values = [
            (out.comp[i], 2), (out.odfan[i], 2), (out.q_tot[i], 2),
            (out.q_sens[i], 2), (out.q_lat[i], 2), (out.w[i], 6),
            (out.cop2[i] if out.cop2[i] is not None else 0.0, 4),
            (out.odb[i], 3), (out.edb[i], 3), (out.ewb[i], 3), (out.out_w[i], 6),
        ]
        for j, (value, digits) in enumerate(values):
            put(ws, row, COL_B + j, value, digits)


def write_daily(ws, out, first_row):
    """Average daily outputs for April 30 and June 25 (Wh per hour)."""
    for k, (month, day) in enumerate([(4, 30), (6, 25)]):
        avg = out.daily_average(month, day)
        for j, key in enumerate(DAILY_COLS):
            put(ws, first_row + k, COL_B + j, avg[key], DIGITS.get(key, 2))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--template", default=DEFAULT_TEMPLATE)
    ap.add_argument("--output", default=DEFAULT_OUTPUT)
    args = ap.parse_args()

    if not os.path.exists(args.template):
        sys.exit(f"Template not found: {args.template}")

    outputs = {}
    for case in CASES:
        try:
            outputs[case] = CaseOutput(case)
        except (FileNotFoundError, KeyError) as err:
            print(f"  WARNING: no usable output for {case} ({err}) — leaving blank",
                  file=sys.stderr)

    rb = xlrd.open_workbook(args.template, formatting_info=True)
    wb = xl_copy(rb)
    ws = wb.get_sheet(0)

    write_identity(ws)
    write_annual(ws, outputs)
    write_maxima(ws, outputs)
    write_extremes(ws, outputs)
    if "CE300" in outputs:
        write_ce300_extras(ws, outputs["CE300"])
    if "CE500" in outputs:
        write_daily(ws, outputs["CE500"], ROW_CE500_DAILY)
    if "CE530" in outputs:
        write_daily(ws, outputs["CE530"], ROW_CE530_DAILY)

    os.makedirs(os.path.dirname(args.output), exist_ok=True)
    wb.save(args.output)
    print(f"Wrote {len(outputs)}/{len(CASES)} CE_b cases to {args.output}")


if __name__ == "__main__":
    main()
