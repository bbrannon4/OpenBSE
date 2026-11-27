#!/usr/bin/env python3
"""
Generate ASHRAE 140-2023 Section 7 output report for IBPSA portal submission.

Reads OpenBSE summary/hourly result files from 140_tests/ and populates the
normative Std140_TF_Output.xlsx template with Section 7 (Thermal Fabric)
results for the Low Mass (600-series), High Mass (900-series) and in-depth
(7.2.3: 195-470, 800, 810) cases.

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

# Non-free-float cases OpenBSE implements: the basic cases (7.2.1-7.2.2) and
# the in-depth cases (7.2.3), whose required output is 7.3.1 only.
INDEPTH_CASES = [
    "195", "200", "210", "215", "220", "230", "240", "250", "270", "280", "290",
    "300", "310", "320", "395", "400", "410", "420", "430", "440", "450", "460",
    "470", "800", "810",
]
CONDITIONED_CASES = [
    "600", "610", "620", "630", "640", "650", "660", "670", "680", "685", "695",
    "900", "910", "920", "930", "940", "950", "960", "980", "985", "995",
] + INDEPTH_CASES

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
    # In-depth cases, rows 91-115 in template order
    **{case: 91 + i for i, case in enumerate(INDEPTH_CASES)},
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
            if col.startswith(f"{zone_col_prefix}:temperature"):
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


def annual_hourly_peaks(csv_path: Path) -> dict:
    """Annual peak heating and sensible cooling loads [W] with their month, day
    and hour, from the hourly zone results.

    The Standard asks for hourly integrated peaks (Section 7.3; 140-2017
    Addendum a 6.2.1.1.3-4). Hourly output rows are time averages of the
    sub-hourly rates, i.e. hourly integrated loads; the summary report's peak
    is the largest single time step and is not used.
    """
    heat_col, cool_col = find_zone_cols(csv_path)
    peaks = {"peak_heat_w": 0.0, "peak_cool_w": 0.0}
    for mo, m in compute_monthly_peaks(csv_path, heat_col, cool_col).items():
        if m["peak_heat_w"] > peaks["peak_heat_w"]:
            peaks.update(peak_heat_w=m["peak_heat_w"], peak_heat_month=mo,
                         peak_heat_day=m["ph_day"], peak_heat_hour=m["ph_hour"])
        if m["peak_cool_w"] > peaks["peak_cool_w"]:
            peaks.update(peak_cool_w=m["peak_cool_w"], peak_cool_month=mo,
                         peak_cool_day=m["pc_day"], peak_cool_hour=m["pc_hour"])
    return peaks


def find_zone_cols(csv_path: Path) -> tuple[str, str]:
    """Return (heating_rate_col, cooling_rate_col) for the first zone in a CSV."""
    with csv_path.open() as f:
        reader = csv.DictReader(f)
        heat_col = cool_col = None
        for col in (reader.fieldnames or []):
            if col.endswith(":heating_rate [W]") and heat_col is None:
                heat_col = col
            if col.endswith(":cooling_rate [W]") and cool_col is None:
                cool_col = col
    if not heat_col or not cool_col:
        raise ValueError(f"Could not find heating/cooling columns in {csv_path}")
    return heat_col, cool_col


def cell(ws, row, col_letter):
    return ws[f"{col_letter}{row}"]


def write_cell(ws, row, col_letter, value):
    ws[f"{col_letter}{row}"] = value


# ── Solar, sky temperature and specific-day outputs (7.3.2-7.3.5, 7.3.8) ─────
# Hourly output rows are time averages of the sub-hourly values, so an hourly
# W/m² value is the hour's integrated Wh/m², and W/1000 is the hour's kWh.
FEB1, MAY4, JUL14 = (2, 1), (5, 4), (7, 14)


def hourly_rows(csv_path: Path) -> list[dict]:
    with csv_path.open() as f:
        return list(csv.DictReader(f))


def day_rows(rows: list[dict], md: tuple[int, int]) -> list[dict]:
    out = [r for r in rows if (int(r["Month"]), int(r["Day"])) == md]
    if len(out) != 24 or [int(r["Hour"]) for r in out] != list(range(1, 25)):
        raise ValueError(f"expected hours 1-24 on {md}, got {len(out)} rows")
    return out


def polygon_area(vertices: list[dict]) -> float:
    """Area of a planar polygon from its vertices (Newell's method)."""
    nx = ny = nz = 0.0
    n = len(vertices)
    for i in range(n):
        a, b = vertices[i], vertices[(i + 1) % n]
        nx += (a["y"] - b["y"]) * (a["z"] + b["z"])
        ny += (a["z"] - b["z"]) * (a["x"] + b["x"])
        nz += (a["x"] - b["x"]) * (a["y"] + b["y"])
    return 0.5 * (nx * nx + ny * ny + nz * nz) ** 0.5


def windows_on(case_yaml: Path, parent: str) -> dict[str, float]:
    """Window name -> area [m²] for the windows in the named wall."""
    import yaml

    model = yaml.safe_load(case_yaml.read_text())
    return {
        s["name"]: polygon_area(s["vertices"])
        for s in model.get("surfaces", [])
        if s.get("type") == "window" and s.get("parent_surface") == parent
    }


class SolarResults:
    """One case's hourly surface solar and sky-temperature output."""

    def __init__(self, results_dir: Path, case: str):
        self.rows = hourly_rows(results_dir / f"ashrae140_case{case}_solar_results.csv")
        self.yaml = results_dir / f"ashrae140_case{case}.yaml"

    def incident(self, surface: str, row: dict) -> float:
        return float(row[f"{surface}:incident_solar [W/m²]"])

    def transmitted(self, wall: str, row: dict, windows: dict[str, float]) -> float:
        """Transmitted solar per unit window area [W/m²] through the wall's windows."""
        w = sum(float(row[f"{n}:transmitted_solar [W]"]) for n in windows)
        return w / sum(windows.values())

    def annual_incident(self, surface: str) -> float:
        return sum(self.incident(surface, r) for r in self.rows) / 1000.0

    def annual_transmitted(self, wall: str) -> float:
        win = windows_on(self.yaml, wall)
        return sum(self.transmitted(wall, r, win) for r in self.rows) / 1000.0

    def sky(self, row: dict) -> float:
        return float(row["Site:sky_temperature [°C]"])


def signed_load_kwh(row: dict, heat_col: str, cool_col: str) -> float:
    """Heating (+) and sensible cooling (-) load of the hour [kWh] (Table 7-47)."""
    return (float(row[heat_col]) - float(row[cool_col])) / 1000.0


def fill_solar_and_sky(ws, results_dir: Path):
    s600 = SolarResults(results_dir, "600")
    # Annual incident unshaded solar, Case 600 (7.3.2.1), C155-C159
    for row, surf in zip(range(155, 160), ["Roof", "North Wall", "East Wall", "South Wall", "West Wall"]):
        ws[f"C{row}"] = round(s600.annual_incident(surf))
    # Annual transmitted solar per window area (7.3.2.2, 7.3.3-7.3.5)
    for row, case, wall in [(163, "600", "South Wall"), (164, "660", "South Wall"),
                            (165, "670", "South Wall"), (166, "620", "West Wall"),
                            (170, "610", "South Wall"), (171, "630", "West Wall")]:
        ws[f"C{row}"] = round(SolarResults(results_dir, case).annual_transmitted(wall))
    # Sky temperature, Case 600 (7.3.2.3), row 178
    sky = [(s600.sky(r), r) for r in s600.rows]
    lo, hi = min(sky, key=lambda x: x[0]), max(sky, key=lambda x: x[0])
    ws["C178"] = round(sum(t for t, _ in sky) / len(sky), 1)
    for (t, r), cols in [(lo, "DEFG"), (hi, "HIJK")]:
        ws[f"{cols[0]}178"] = round(t, 1)
        ws[f"{cols[1]}178"] = MONTH_ABBR[int(r["Month"])]
        ws[f"{cols[2]}178"] = int(r["Day"])
        ws[f"{cols[3]}178"] = int(r["Hour"])
    print(f"  Solar and sky: horizontal {ws['C155'].value} kWh/m², sky mean {ws['C178'].value} °C")


def fill_specific_days(ws, results_dir: Path):
    """Specific-day hourly output of Table 7-47 (7.3.8), rows 230-253, 262-285, 294-317."""
    s600 = SolarResults(results_dir, "600")
    trans = {c: SolarResults(results_dir, c) for c in ("600", "660", "670")}
    win = {c: windows_on(trans[c].yaml, "South Wall") for c in trans}
    for h in range(24):
        r = 230 + h
        for md, cols in [(MAY4, "CDE"), (JUL14, "FGH")]:
            row = day_rows(s600.rows, md)[h]
            for col, surf in zip(cols, ["Roof", "South Wall", "West Wall"]):
                ws[f"{col}{r}"] = round(s600.incident(surf, row), 1)
        for col, md in zip("IJK", (FEB1, MAY4, JUL14)):
            ws[f"{col}{r}"] = round(s600.sky(day_rows(s600.rows, md)[h]), 2)
        for md, cols in [(FEB1, "LMN"), (MAY4, "OPQ"), (JUL14, "RST")]:
            for col, c in zip(cols, ("600", "660", "670")):
                row = day_rows(trans[c].rows, md)[h]
                ws[f"{col}{r}"] = round(trans[c].transmitted("South Wall", row, win[c]), 1)

    def loads(case):
        csv_path = results_dir / f"ashrae140_case{case}_zone_results.csv"
        return hourly_rows(csv_path), find_zone_cols(csv_path)

    feb_cases = ["600", "640", "660", "670", "680", "685", "695", "900", "940", "980", "985", "995"]
    jul_cases = ["600", "660", "670", "680", "685", "695", "900", "980", "985", "995"]
    for col, case in zip("CDEFGHIJKLMN", feb_cases):
        rows, (hc, cc) = loads(case)
        for h, row in enumerate(day_rows(rows, FEB1)):
            ws[f"{col}{262 + h}"] = round(signed_load_kwh(row, hc, cc), 3)
    for col, case in zip("OPQRSTUVWX", jul_cases):
        rows, (hc, cc) = loads(case)
        for h, row in enumerate(day_rows(rows, JUL14)):
            ws[f"{col}{262 + h}"] = round(signed_load_kwh(row, hc, cc), 3)
    for col, case in zip("YZ", ["640", "940"]):
        rows = hourly_rows(results_dir / f"ashrae140_case{case}_zone_results.csv")
        tcol = next(k for k in rows[0] if k.endswith(":temperature [°C]"))
        for h, row in enumerate(day_rows(rows, FEB1)):
            ws[f"{col}{262 + h}"] = round(float(row[tcol]), 2)
    for col, key, prefix, md in [("C", "600ff", "Case600FF Zone", FEB1),
                                 ("D", "900ff", "Case900FF Zone", FEB1),
                                 ("E", "650ff", "Case650FF Zone", JUL14),
                                 ("F", "950ff", "Case950FF Zone", JUL14),
                                 ("G", "680ff", "Case680FF Zone", FEB1),
                                 ("H", "980ff", "Case980FF Zone", FEB1)]:
        rows = hourly_rows(results_dir / f"ashrae140_case{key}_zone_results.csv")
        tcol = f"{prefix}:temperature [°C]"
        for h, row in enumerate(day_rows(rows, md)):
            ws[f"{col}{294 + h}"] = round(float(row[tcol]), 2)
    print("  Specific-day hourly output written")


import openbse_release  # noqa: E402


def main():
    parser = argparse.ArgumentParser(description="Generate ASHRAE 140 Section 7 submission file")
    parser.add_argument("--template", required=True, help="Path to Std140_TF_Output.xlsx")
    parser.add_argument("--results", default="140_tests/cases", help="Path to 140_tests/cases/ directory (where the validated output files live)")
    parser.add_argument("--output", required=True, help="Output .xlsx path")
    parser.add_argument("--version", default=openbse_release.version(), help="OpenBSE version string (default: Cargo.toml)")
    args = parser.parse_args()

    results_dir = Path(args.results).expanduser()
    template_path = Path(args.template).expanduser()
    output_path = Path(args.output).expanduser()

    print(f"Loading template: {template_path}")
    wb = openpyxl.load_workbook(str(template_path))
    ws_a = wb["A"]
    ws_bin = wb["TMPBIN"]

    # ── Identifying information ─────────────────────────────────────────────────
    ws_a["C61"] = "OpenBSE"
    ws_a["C62"] = args.version
    ws_a["C63"] = openbse_release.submission_date().isoformat()

    # ── Conditioned-zone annual loads & peak loads ──────────────────────────────
    print("Processing conditioned cases...")
    for case in CONDITIONED_CASES:
        summary_path = results_dir / f"ashrae140_case{case.lower()}_summary.txt"
        if not summary_path.exists():
            print(f"  WARNING: no summary for case {case} — skipping")
            continue

        s = parse_summary(summary_path)
        zone_csv = results_dir / f"ashrae140_case{case.lower()}_zone_results.csv"
        if not zone_csv.exists():
            print(f"  WARNING: no zone results for case {case} — skipping")
            continue
        s.update(annual_hourly_peaks(zone_csv))
        row = CONDITIONED_ROW[case]
        if str(ws_a[f"B{row}"].value) != case:
            sys.exit(f"template row {row} is case {ws_a[f'B{row}'].value}, not {case}")

        heat_mwh = round(s.get("heating_kwh", 0.0) / 1000.0, 3)
        cool_mwh = round(s.get("cooling_kwh", 0.0) / 1000.0, 3)
        ph_kw = round(s.get("peak_heat_w", 0.0) / 1000.0, 3)
        pc_kw = round(s.get("peak_cool_w", 0.0) / 1000.0, 3)

        ws_a[f"C{row}"] = heat_mwh
        ws_a[f"D{row}"] = cool_mwh

        # Zero loads are entered as 0 (7.3.1.5); a zero peak has no time.
        ws_a[f"E{row}"] = ph_kw
        ws_a[f"I{row}"] = pc_kw
        if ph_kw > 0:
            ws_a[f"F{row}"] = MONTH_ABBR.get(s.get("peak_heat_month", 0), "")
            ws_a[f"G{row}"] = s.get("peak_heat_day", "")
            ws_a[f"H{row}"] = s.get("peak_heat_hour", "")
        if pc_kw > 0:
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
            ws_a[f"{ph_off}{row}"] = ph_kw   # 0 when there is no load (7.3.2.4.5)
            ws_a[f"{pc_off}{row}"] = pc_kw
            if ph_kw > 0:
                ws_a[f"{phd_off}{row}"] = mpeaks.get("ph_day", "")
                ws_a[f"{phh_off}{row}"] = mpeaks.get("ph_hour", "")
            if pc_kw > 0:
                ws_a[f"{pcd_off}{row}"] = mpeaks.get("pc_day", "")
                ws_a[f"{pch_off}{row}"] = mpeaks.get("pc_hour", "")

        print(f"  Case {case}: monthly loads written")

    print("Processing solar, sky temperature and specific-day outputs...")
    fill_solar_and_sky(ws_a, results_dir)
    fill_specific_days(ws_a, results_dir)

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
