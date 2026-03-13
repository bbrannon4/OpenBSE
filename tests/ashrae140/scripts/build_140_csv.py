#!/usr/bin/env python3
"""
Build ASHRAE 140 Results Comparison CSV for OpenBSE.

Reads OpenBSE simulation output files and compares against ASHRAE 140-2023
acceptance ranges. Outputs a CSV with pass/fail status and delta information.
"""

import csv
import os
import sys

# Base directory: one level up from this script's location (results/)
BASE_DIR = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUTPUT_PATH = os.path.join(
    os.path.dirname(os.path.abspath(__file__)),
    "FULL_140_RESULTS.csv",
)

# ---------------------------------------------------------------------------
# ASHRAE 140-2023 Acceptance Ranges
# ---------------------------------------------------------------------------
# Load cases: (H_min, H_max, C_min, C_max) in kWh
LOAD_RANGES = {
    "600":  (3993, 4504, 5432, 6162),
    "610":  (4066, 4592, 4117, 4382),
    "620":  (4094, 4719, 3841, 4404),
    "630":  (4356, 5139, 2573, 3074),
    "640":  (2403, 2682, 5237, 5893),
    "650":  (0,    0,    4186, 4945),
    "660":  (3574, 3821, 2966, 3340),
    "670":  (5300, 6140, 5954, 6623),
    "680":  (1732, 2286, 5932, 6529),
    "685":  (4532, 5042, 8238, 9130),
    "695":  (2385, 2892, 8386, 9172),
    "900":  (1379, 1814, 2267, 2714),
    "910":  (1648, 2163, 1191, 1490),
    "920":  (2956, 3607, 2549, 3128),
    "930":  (3524, 4384, 1654, 2161),
    "940":  (863,  1389, 2203, 2613),
    "950":  (0,    0,    586,  707),
    "960":  (2522, 2860, 789,  950),
    "980":  (246,  720,  3501, 3995),
    "985":  (2120, 2801, 5880, 7273),
    "995":  (755,  1330, 6771, 7482),
}

# Free-float temperature ranges: (max_lo, max_hi, min_lo, min_hi, mean_lo, mean_hi)
FF_RANGES = {
    "600ff": (62.4, 68.4, -13.8, -9.9,  24.3, 26.1),
    "650ff": (61.1, 66.8, -17.8, -16.7, 17.6, 18.9),
    "680ff": (69.8, 78.5, -8.1,  -5.7,  30.2, 33.3),
    "900ff": (43.3, 46.0,  0.6,   2.2,  24.5, 25.7),
    "950ff": (36.1, 37.1, -13.4, -12.5, 14.3, 15.0),
    "980ff": (48.5, 52.8,  7.3,  12.5,  30.5, 33.3),
}

# 960 Sun Zone temperature ranges
SZ_RANGES = {
    "960 SZ": (48.1, 53.2, 4.2, 8.0, 26.8, 29.5),
}


def read_load_results(case):
    """Read heating and cooling loads from summary file."""
    summary = os.path.join(BASE_DIR, f"ashrae140_case{case}_summary.txt")
    if not os.path.exists(summary):
        return None, None
    h_val = c_val = None
    with open(summary) as f:
        for line in f:
            if 'Heating:' in line and 'kWh' in line and 'Peak' not in line:
                h_val = float(line.split()[1])
            if 'Cooling:' in line and 'kWh' in line and 'Peak' not in line:
                c_val = float(line.split()[1])
    return h_val, c_val


def read_ff_temps(case):
    """Read free-float zone temperatures from zone_results.csv."""
    fname = os.path.join(BASE_DIR, f"ashrae140_case{case}_zone_results.csv")
    if not os.path.exists(fname):
        return None, None, None
    with open(fname) as f:
        reader = csv.reader(f)
        header = next(reader)
        col = None
        for i, h in enumerate(header):
            if 'zone_temperature' in h.lower():
                col = i
                break
        if col is None:
            return None, None, None
        temps = [float(row[col]) for row in reader]
    if not temps:
        return None, None, None
    return max(temps), min(temps), sum(temps) / len(temps)


def read_960_sz_temps():
    """Read 960 Sun Zone temperatures."""
    fname = os.path.join(BASE_DIR, "ashrae140_case960_zone_results.csv")
    if not os.path.exists(fname):
        return None, None, None
    with open(fname) as f:
        reader = csv.reader(f)
        header = next(reader)
        # Find Sun Zone temperature column
        col = None
        for i, h in enumerate(header):
            if 'sun zone' in h.lower() and 'temperature' in h.lower():
                col = i
                break
        if col is None:
            return None, None, None
        temps = [float(row[col]) for row in reader]
    if not temps:
        return None, None, None
    return max(temps), min(temps), sum(temps) / len(temps)


def evaluate(value, lo, hi):
    """Return (status, delta). delta is signed distance outside range, or 0."""
    if lo <= value <= hi:
        return "PASS", 0.0
    if value < lo:
        return "FAIL", value - lo
    return "FAIL", value - hi


def pct_delta(delta, lo, hi):
    """Percentage delta relative to the midpoint of the range."""
    midpoint = (lo + hi) / 2.0
    if midpoint == 0:
        return ""
    return f"{(delta / midpoint) * 100:.1f}%"


def main():
    rows = []
    header = ["Case", "Metric", "OpenBSE", "Min", "Max", "Status", "Delta", "Pct Delta"]

    pass_count = 0
    fail_count = 0
    fail_details = []
    missing = []

    # --- Load cases (Heating & Cooling) ---
    for case in sorted(LOAD_RANGES.keys(), key=lambda x: int(x)):
        h_lo, h_hi, c_lo, c_hi = LOAD_RANGES[case]
        h_val, c_val = read_load_results(case)

        if h_val is None or c_val is None:
            missing.append(case)
            continue

        h_rounded = round(h_val)
        c_rounded = round(c_val)

        for metric, val, lo, hi in [
            ("Annual Heating (kWh)", h_rounded, h_lo, h_hi),
            ("Annual Cooling (kWh)", c_rounded, c_lo, c_hi),
        ]:
            status, delta = evaluate(val, lo, hi)
            if status == "PASS":
                pass_count += 1
                rows.append([case, metric, val, lo, hi, status, "", ""])
            else:
                fail_count += 1
                pct = pct_delta(delta, lo, hi)
                rows.append([case, metric, val, lo, hi, status, f"{delta:.0f}", pct])
                fail_details.append(f"  Case {case} {metric}: OpenBSE={val}, "
                                    f"Range=[{lo}, {hi}], Delta={delta:.0f}")

    # --- Free-float temperature cases ---
    for case in sorted(FF_RANGES.keys()):
        max_lo, max_hi, min_lo, min_hi, mean_lo, mean_hi = FF_RANGES[case]
        peak_max, peak_min, mean_t = read_ff_temps(case)

        if peak_max is None:
            missing.append(case)
            continue

        # Round to 1 decimal place (matching ASHRAE 140 reporting)
        max_r = round(peak_max, 1)
        min_r = round(peak_min, 1)
        mean_r = round(mean_t, 1)

        display_case = case.upper()

        for metric, val, lo, hi in [
            ("Peak Max Temp (C)", max_r, max_lo, max_hi),
            ("Peak Min Temp (C)", min_r, min_lo, min_hi),
            ("Mean Temp (C)",     mean_r, mean_lo, mean_hi),
        ]:
            status, delta = evaluate(val, lo, hi)
            if status == "PASS":
                pass_count += 1
                rows.append([display_case, metric, val, lo, hi, status, "", ""])
            else:
                fail_count += 1
                rows.append([display_case, metric, val, lo, hi, status, f"{delta:.1f}", ""])
                fail_details.append(f"  Case {display_case} {metric}: OpenBSE={val}, "
                                    f"Range=[{lo}, {hi}], Delta={delta:.1f}")

    # --- 960 Sun Zone temperatures ---
    for case, (max_lo, max_hi, min_lo, min_hi, mean_lo, mean_hi) in SZ_RANGES.items():
        peak_max, peak_min, mean_t = read_960_sz_temps()

        if peak_max is None:
            missing.append(case)
            continue

        max_r = round(peak_max, 1)
        min_r = round(peak_min, 1)
        mean_r = round(mean_t, 1)

        for metric, val, lo, hi in [
            ("Peak Max Temp (C)", max_r, max_lo, max_hi),
            ("Peak Min Temp (C)", min_r, min_lo, min_hi),
            ("Mean Temp (C)",     mean_r, mean_lo, mean_hi),
        ]:
            status, delta = evaluate(val, lo, hi)
            if status == "PASS":
                pass_count += 1
                rows.append([case, metric, val, lo, hi, status, "", ""])
            else:
                fail_count += 1
                rows.append([case, metric, val, lo, hi, status, f"{delta:.1f}", ""])
                fail_details.append(f"  Case {case} {metric}: OpenBSE={val}, "
                                    f"Range=[{lo}, {hi}], Delta={delta:.1f}")

    # --- Write CSV ---
    with open(OUTPUT_PATH, "w", newline="") as f:
        writer = csv.writer(f)
        writer.writerow(header)
        writer.writerows(rows)

    # --- Summary ---
    total = pass_count + fail_count
    print("=" * 60)
    print("ASHRAE 140 Results Summary for OpenBSE")
    print("=" * 60)
    print(f"Total checks:  {total}")
    print(f"PASS:          {pass_count}  ({100 * pass_count / total:.1f}%)")
    print(f"FAIL:          {fail_count}  ({100 * fail_count / total:.1f}%)")
    print("-" * 60)
    if missing:
        print(f"Missing cases: {', '.join(missing)}")
    if fail_details:
        print("Failed checks:")
        for d in fail_details:
            print(d)
    else:
        print("All checks passed!")
    print("-" * 60)
    print(f"CSV written to: {OUTPUT_PATH}")


if __name__ == "__main__":
    main()
