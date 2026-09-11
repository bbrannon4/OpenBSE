#!/usr/bin/env python3
"""
Generate ASHRAE 140-2023 Section 7 output report for IBPSA portal submission.

Reads OpenBSE summary/hourly result files from 140_tests/ and populates the
normative Std140_TF_Output.xlsx template with Section 7 (Thermal Fabric)
results for all Low Mass (600-series) and High Mass (900-series) cases.

Usage:
    python 140_tests/scripts/generate_ashrae140_submission.py \
        --template  "~/Downloads/Accompanying FIles/Std140_TF_Files/Normative Materials/Std140_TF_Output.xlsx" \
        --results   140_tests/ \
        --output    140_tests/results/OpenBSE_Std140_TF_Output.xlsx

Dependencies: openpyxl (pip install openpyxl)
"""

import argparse
import csv
import os
import re
import sys
from pathlib import Path

try:
    import openpyxl
except ImportError:
    sys.exit("openpyxl required: pip install openpyxl")

MONTH_ABBR = {1: "Jan", 2: "Feb", 3: "Mar", 4: "Apr", 5: "May", 6: "Jun",
              7: "Jul", 8: "Aug", 9: "Sep", 10: "Oct", 11: "Nov", 12: "Dec"}
MONTH_NUM = {v: k for k, v in MONTH_ABBR.items()}

# Non-free-float cases OpenBSE implements
CONDITIONED_CASES = [
    "600", "610", "620", "630", "640", "650", "660", "670", "680", "685", "695",
    "900", "910", "920", "930", "940", "950", "960", "980", "985", "995",
]

# Free-float cases: (case_id, summary_key, zone_col_prefix)
FREE_FLOAT_CASES = [
    ("600FF",  "600ff",  "Case600FF Zone"),
    ("900FF",  "900ff",  "Case900FF Zone"),
    ("650FF",  "650ff",  "Case650FF Zone"),
    ("950FF",  "950ff",  "Case950FF Zone"),
    ("680FF",  "680ff",  "Case680FF Zone"),
    ("980FF",  "980ff",  "Case980FF Zone"),
    ("960",    "960",    "Sun Zone"),      # 960 sunspace free-float row
]

# Template row numbers for conditioned-case data (sheet A)
CONDITIONED_ROW = {
    "600": 70, "610": 71, "620": 72, "630": 73, "640": 74, "650": 75,
    "660": 76, "670": 77, "680": 78, "685": 79, "695": 80,
    "900": 81, "910": 82, "920": 83, "930": 84, "940": 85, "950": 86,
    "960": 87, "980": 88, "985": 89, "995": 90,
}

# Template row numbers for free-float zone temperature data (sheet A)
FF_ROW = {
    "600FF": 130, "900FF": 131, "650FF": 132, "950FF": 133,
    "680FF": 134, "980FF": 135,
    "960":   136,  # sun zone
}

# Monthly data rows in sheet A: Jan=190 … Dec=201
MONTHLY_ROW = {m: 189 + m for m in range(1, 13)}


def parse_summary(path: Path) -> dict:
    """Extract annual loads, peak loads, and monthly loads from a summary .txt file."""
    text = path.read_text()
    result = {}

    # Annual totals
    if m := re.search(r"Heating:\s+([\d.]+)\s+kWh", text):
        result["heating_kwh"] = float(m.group(1))
    if m := re.search(r"Cooling:\s+([\d.]+)\s+kWh", text):
        result["cooling_kwh"] = float(m.group(1))

    # Peak loads
    if m := re.search(r"Peak Heating:\s+([\d.]+)\s+W\s+\(Month\s+(\d+),\s+Day\s+(\d+),\s+Hour\s+(\d+)\)", text):
        result["peak_heat_w"] = float(m.group(1))
        result["peak_heat_month"] = int(m.group(2))
        result["peak_heat_day"] = int(m.group(3))
        result["peak_heat_hour"] = int(m.group(4))
    if m := re.search(r"Peak Cooling:\s+([\d.]+)\s+W\s+\(Month\s+(\d+),\s+Day\s+(\d+),\s+Hour\s+(\d+)\)", text):
        result["peak_cool_w"] = float(m.group(1))
        result["peak_cool_month"] = int(m.group(2))
        result["peak_cool_day"] = int(m.group(3))
        result["peak_cool_hour"] = int(m.group(4))

    # Monthly breakdown — two formats:
    #   Horizontal (cases/ dir): header row "Load  Jan Feb ... Dec Total", then
    #     "Heating  v1 v2 ... v12  total" and "Cooling  v1 v2 ... v12  total"
    #   Vertical (legacy):   "Jan  h_kwh  c_kwh  total" per row
    monthly = {}
    month_col_order = None  # for horizontal format
    heating_row = None
    cooling_row = None
    lines = text.splitlines()
    i = 0
    while i < len(lines):
        line = lines[i]
        # Horizontal header: "Load   Jan  Feb  ... Dec  Total"
        if re.search(r"\bLoad\b.*\bJan\b.*\bDec\b", line):
            parts = line.split()
            # parts[0] = "Load", then month abbrs, last = "Total"
            month_col_order = [MONTH_NUM[p] for p in parts if p in MONTH_NUM]
            i += 1
            continue
        if month_col_order and re.match(r"\s*Heating\b", line):
            vals = line.split()[1:]  # skip "Heating"
            heating_row = [float(v) for v in vals[:len(month_col_order)]]
            i += 1
            continue
        if month_col_order and re.match(r"\s*Cooling\b", line):
            vals = line.split()[1:]
            cooling_row = [float(v) for v in vals[:len(month_col_order)]]
            i += 1
            break
        # Vertical format: "Month  Heating[kWh]  Cooling[kWh]..."
        if "Month  Heating" in line:
            i += 1
            while i < len(lines):
                parts = lines[i].split()
                if not parts or parts[0] == "-----":
                    i += 1
                    continue
                if parts[0] == "Total":
                    break
                try:
                    mn = MONTH_NUM.get(parts[0])
                    if mn:
                        monthly[mn] = {"heating_kwh": float(parts[1]), "cooling_kwh": float(parts[2])}
                except (ValueError, IndexError):
                    pass
                i += 1
            break
        i += 1

    if month_col_order and heating_row and cooling_row:
        for mo, h, c in zip(month_col_order, heating_row, cooling_row):
            monthly[mo] = {"heating_kwh": h, "cooling_kwh": c}

    result["monthly"] = monthly
    return result


def read_ff_zone_results(path: Path, zone_col_prefix: str) -> dict:
    """Read hourly free-float zone temperature CSV and return stats + hourly temps."""
    temps = []
    peak_min = (float("inf"), None, None, None)
    peak_max = (float("-inf"), None, None, None)

    with path.open() as f:
        reader = csv.DictReader(f)
        # Find the temperature column matching zone_col_prefix
        temp_col = None
        for col in reader.fieldnames or []:
            if zone_col_prefix in col and "zone_temperature" in col:
                temp_col = col
                break
        if temp_col is None:
            raise ValueError(f"No temperature column for '{zone_col_prefix}' in {path}")

        for row in reader:
            t = float(row[temp_col])
            mo = int(row["Month"])
            day = int(row["Day"])
            hr = int(row["Hour"])
            temps.append(t)
            if t < peak_min[0]:
                peak_min = (t, mo, day, hr)
            if t > peak_max[0]:
                peak_max = (t, mo, day, hr)

    avg = sum(temps) / len(temps) if temps else 0.0
    return {
        "hourly_temps": temps,
        "avg": round(avg, 1),
        "min": round(peak_min[0], 1),
        "min_month": peak_min[1],
        "min_day": peak_min[2],
        "min_hour": peak_min[3],
        "max": round(peak_max[0], 1),
        "max_month": peak_max[1],
        "max_day": peak_max[2],
        "max_hour": peak_max[3],
    }


def compute_monthly_peaks(csv_path: Path, heat_col: str, cool_col: str) -> dict:
    """Compute per-month peak heating/cooling loads and timestamps from hourly CSV."""
    monthly = {}
    with csv_path.open() as f:
        reader = csv.DictReader(f)
        for row in reader:
            mo = int(row["Month"])
            day = int(row["Day"])
            hr = int(row["Hour"])
            heat = float(row.get(heat_col, 0.0))
            cool = float(row.get(cool_col, 0.0))
            if mo not in monthly:
                monthly[mo] = {
                    "peak_heat_w": 0.0, "ph_day": 0, "ph_hour": 0,
                    "peak_cool_w": 0.0, "pc_day": 0, "pc_hour": 0,
                }
            if heat > monthly[mo]["peak_heat_w"]:
                monthly[mo].update({"peak_heat_w": heat, "ph_day": day, "ph_hour": hr})
            if cool > monthly[mo]["peak_cool_w"]:
                monthly[mo].update({"peak_cool_w": cool, "pc_day": day, "pc_hour": hr})
    return monthly


def find_zone_cols(csv_path: Path) -> tuple[str, str]:
    """Return (heating_rate_col, cooling_rate_col) for the first zone in a CSV."""
    with csv_path.open() as f:
        reader = csv.DictReader(f)
        heat_col = cool_col = None
        for col in (reader.fieldnames or []):
            if "zone_heating_rate" in col and heat_col is None:
                heat_col = col
            if "zone_cooling_rate" in col and cool_col is None:
                cool_col = col
    if not heat_col or not cool_col:
        raise ValueError(f"Could not find heating/cooling columns in {csv_path}")
    return heat_col, cool_col


def cell(ws, row, col_letter):
    return ws[f"{col_letter}{row}"]


def write_cell(ws, row, col_letter, value):
    ws[f"{col_letter}{row}"] = value


def main():
    parser = argparse.ArgumentParser(description="Generate ASHRAE 140 Section 7 submission file")
    parser.add_argument("--template", required=True, help="Path to Std140_TF_Output.xlsx")
    parser.add_argument("--results", default="140_tests/cases", help="Path to 140_tests/cases/ directory (where the validated output files live)")
    parser.add_argument("--output", required=True, help="Output .xlsx path")
    parser.add_argument("--version", default="dev", help="OpenBSE version string")
    args = parser.parse_args()

    results_dir = Path(args.results).expanduser()
    template_path = Path(args.template).expanduser()
    output_path = Path(args.output).expanduser()

    print(f"Loading template: {template_path}")
    wb = openpyxl.load_workbook(str(template_path))
    ws_a = wb["A"]
    ws_bin = wb["TMPBIN"]

    # ── Identifying information ─────────────────────────────────────────────────
    import datetime
    ws_a["C61"] = "OpenBSE"
    ws_a["C62"] = args.version
    ws_a["C63"] = datetime.date.today().isoformat()

    # ── Conditioned-zone annual loads & peak loads ──────────────────────────────
    print("Processing conditioned cases...")
    for case in CONDITIONED_CASES:
        summary_path = results_dir / f"ashrae140_case{case.lower()}_summary.txt"
        if not summary_path.exists():
            print(f"  WARNING: no summary for case {case} — skipping")
            continue

        s = parse_summary(summary_path)
        row = CONDITIONED_ROW[case]

        heat_mwh = round(s.get("heating_kwh", 0.0) / 1000.0, 3)
        cool_mwh = round(s.get("cooling_kwh", 0.0) / 1000.0, 3)
        ph_kw = round(s.get("peak_heat_w", 0.0) / 1000.0, 3)
        pc_kw = round(s.get("peak_cool_w", 0.0) / 1000.0, 3)

        ws_a[f"C{row}"] = heat_mwh
        ws_a[f"D{row}"] = cool_mwh

        if ph_kw > 0:
            ws_a[f"E{row}"] = ph_kw
            ws_a[f"F{row}"] = MONTH_ABBR.get(s.get("peak_heat_month", 0), "")
            ws_a[f"G{row}"] = s.get("peak_heat_day", "")
            ws_a[f"H{row}"] = s.get("peak_heat_hour", "")
        if pc_kw > 0:
            ws_a[f"I{row}"] = pc_kw
            ws_a[f"J{row}"] = MONTH_ABBR.get(s.get("peak_cool_month", 0), "")
            ws_a[f"K{row}"] = s.get("peak_cool_day", "")
            ws_a[f"L{row}"] = s.get("peak_cool_hour", "")

        print(f"  Case {case}: heat={heat_mwh} MWh, cool={cool_mwh} MWh, "
              f"peak_heat={ph_kw} kW, peak_cool={pc_kw} kW")

    # ── Free-float zone temperatures ────────────────────────────────────────────
    print("Processing free-float cases...")
    for case_id, key, zone_prefix in FREE_FLOAT_CASES:
        csv_path = results_dir / f"ashrae140_case{key}_zone_results.csv"
        if not csv_path.exists():
            print(f"  WARNING: no zone results for {case_id} — skipping")
            continue

        ff = read_ff_zone_results(csv_path, zone_prefix)
        row = FF_ROW.get(case_id)
        if row is None:
            print(f"  WARNING: no template row for {case_id}")
            continue

        ws_a[f"C{row}"] = ff["avg"]
        ws_a[f"D{row}"] = ff["min"]
        ws_a[f"E{row}"] = MONTH_ABBR.get(ff["min_month"], "")
        ws_a[f"F{row}"] = ff["min_day"]
        ws_a[f"G{row}"] = ff["min_hour"]
        ws_a[f"H{row}"] = ff["max"]
        ws_a[f"I{row}"] = MONTH_ABBR.get(ff["max_month"], "")
        ws_a[f"J{row}"] = ff["max_day"]
        ws_a[f"K{row}"] = ff["max_hour"]

        print(f"  {case_id}: avg={ff['avg']}°C, min={ff['min']}°C, max={ff['max']}°C")

    # ── Monthly conditioned loads for Cases 600 and 900 ─────────────────────────
    print("Processing monthly loads for cases 600 and 900...")
    for case, heat_off, cool_off, ph_off, phd_off, phh_off, pc_off, pcd_off, pch_off in [
        ("600", "C", "D", "E", "F", "G", "H", "I", "J"),
        ("900", "K", "L", "M", "N", "O", "P", "Q", "R"),
    ]:
        summary_path = results_dir / f"ashrae140_case{case}_summary.txt"
        csv_path = results_dir / f"ashrae140_case{case}_zone_results.csv"
        if not summary_path.exists() or not csv_path.exists():
            print(f"  WARNING: missing data for case {case}")
            continue

        s = parse_summary(summary_path)
        heat_col, cool_col = find_zone_cols(csv_path)
        monthly_peaks = compute_monthly_peaks(csv_path, heat_col, cool_col)

        for mo in range(1, 13):
            row = MONTHLY_ROW[mo]
            mdata = s["monthly"].get(mo, {})
            mpeaks = monthly_peaks.get(mo, {})

            ws_a[f"{heat_off}{row}"] = round(mdata.get("heating_kwh", 0.0), 1)
            ws_a[f"{cool_off}{row}"] = round(mdata.get("cooling_kwh", 0.0), 1)

            ph_kw = round(mpeaks.get("peak_heat_w", 0.0) / 1000.0, 3)
            pc_kw = round(mpeaks.get("peak_cool_w", 0.0) / 1000.0, 3)
            if ph_kw > 0:
                ws_a[f"{ph_off}{row}"] = ph_kw
                ws_a[f"{phd_off}{row}"] = mpeaks.get("ph_day", "")
                ws_a[f"{phh_off}{row}"] = mpeaks.get("ph_hour", "")
            if pc_kw > 0:
                ws_a[f"{pc_off}{row}"] = pc_kw
                ws_a[f"{pcd_off}{row}"] = mpeaks.get("pc_day", "")
                ws_a[f"{pch_off}{row}"] = mpeaks.get("pc_hour", "")

        print(f"  Case {case}: monthly loads written")

    # ── TMPBIN: 900FF hourly zone temperatures ───────────────────────────────────
    print("Writing 900FF hourly temperatures to TMPBIN sheet...")
    ff900_csv = results_dir / "ashrae140_case900ff_zone_results.csv"
    if ff900_csv.exists():
        ff900 = read_ff_zone_results(ff900_csv, "Case900FF Zone")
        hourly = ff900["hourly_temps"]
        if len(hourly) != 8760:
            print(f"  WARNING: expected 8760 hours for 900FF, got {len(hourly)}")
        for i, temp in enumerate(hourly):
            ws_bin[f"E{11 + i}"] = round(temp, 2)
        print(f"  Written {len(hourly)} hourly temperatures")
    else:
        print("  WARNING: 900FF zone results not found")

    # ── Save ────────────────────────────────────────────────────────────────────
    output_path.parent.mkdir(parents=True, exist_ok=True)
    wb.save(str(output_path))
    print(f"\nSaved: {output_path}")
    print("Open in Excel to verify (bin counts will recalculate automatically).")


if __name__ == "__main__":
    main()
