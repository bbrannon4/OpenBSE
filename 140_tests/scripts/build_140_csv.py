#!/usr/bin/env python3
"""
Build ASHRAE 140 Results Comparison CSV for OpenBSE.

Reads OpenBSE simulation output files and compares against ASHRAE 140-2023
acceptance ranges. Outputs a CSV with pass/fail status and delta information.
"""

import csv
import os
import sys

# Base directory: cases/ subdirectory of 140_tests/
BASE_DIR = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "cases")
OUTPUT_PATH = os.path.join(
    os.path.dirname(os.path.abspath(__file__)),
    "FULL_140_RESULTS.csv",
)

# ---------------------------------------------------------------------------
# ASHRAE 140-2023 Acceptance Ranges
# ---------------------------------------------------------------------------
# Loaded from ../acceptance_ranges_140_2023.json — the single, tamper-evidenced
# source of truth (CI pins its SHA-256; see .github/workflows/ci.yml). Do NOT
# inline ranges here: any change to the acceptance criteria must show up as an
# explicit diff to the JSON *and* the pinned hash in the same commit.
import json

_RANGES_PATH = os.path.join(
    os.path.dirname(os.path.dirname(os.path.abspath(__file__))),
    "acceptance_ranges_140_2023.json",
)
with open(_RANGES_PATH) as _f:
    _RANGES = json.load(_f)

# Load cases: (H_min, H_max, C_min, C_max) in kWh
LOAD_RANGES = {k: tuple(v) for k, v in _RANGES["load_ranges_kwh"].items()}

# Free-float temperature ranges: (max_lo, max_hi, min_lo, min_hi, mean_lo, mean_hi)
FF_RANGES = {k: tuple(v) for k, v in _RANGES["free_float_temp_ranges_c"].items()}

# 960 Sun Zone temperature ranges
SZ_RANGES = {k: tuple(v) for k, v in _RANGES["sun_zone_temp_ranges_c"].items()}

# ---------------------------------------------------------------------------
# Known failures — these are tracked but do not fail CI.
# Format: (case_display_name, metric_name)
# When a known failure starts passing, CI prints a notice so you can promote it.
# When a currently-passing check regresses, CI fails.
# ---------------------------------------------------------------------------
KNOWN_FAILURES = set()  # All 63 checks currently pass as of 2026-04-04


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
    """Read free-float zone temperatures from results CSV."""
    # Try custom zone_results.csv first, fall back to default results CSV
    fname = os.path.join(BASE_DIR, f"ashrae140_case{case}_zone_results.csv")
    if not os.path.exists(fname):
        fname = os.path.join(BASE_DIR, f"ashrae140_case{case}_results.csv")
    if not os.path.exists(fname):
        return None, None, None
    with open(fname) as f:
        reader = csv.reader(f)
        header = next(reader)
        col = None
        for i, h in enumerate(header):
            if 'zone_temp' in h.lower() and 'supply' not in h.lower():
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
        fname = os.path.join(BASE_DIR, "ashrae140_case960_results.csv")
    if not os.path.exists(fname):
        return None, None, None
    with open(fname) as f:
        reader = csv.reader(f)
        header = next(reader)
        # Find Sun Zone temperature column
        col = None
        for i, h in enumerate(header):
            if 'sun zone' in h.lower() and 'zone_temp' in h.lower():
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
    ci_mode = "--ci" in sys.argv

    rows = []
    header = ["Case", "Metric", "OpenBSE", "Min", "Max", "Status", "Delta", "Pct Delta"]

    pass_count = 0
    fail_count = 0
    fail_details = []
    failed_keys = set()   # (case, metric) tuples that failed
    passed_keys = set()   # (case, metric) tuples that passed
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
            key = (case, metric)
            if status == "PASS":
                pass_count += 1
                passed_keys.add(key)
                rows.append([case, metric, val, lo, hi, status, "", ""])
            else:
                fail_count += 1
                failed_keys.add(key)
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
            key = (display_case, metric)
            if status == "PASS":
                pass_count += 1
                passed_keys.add(key)
                rows.append([display_case, metric, val, lo, hi, status, "", ""])
            else:
                fail_count += 1
                failed_keys.add(key)
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
            key = (case, metric)
            if status == "PASS":
                pass_count += 1
                passed_keys.add(key)
                rows.append([case, metric, val, lo, hi, status, "", ""])
            else:
                fail_count += 1
                failed_keys.add(key)
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

    # --- CI mode: detect regressions and newly-passing cases ---
    if ci_mode:
        regressions = failed_keys - KNOWN_FAILURES
        newly_passing = KNOWN_FAILURES & passed_keys

        print()
        if newly_passing:
            print("NEW PASSES (update KNOWN_FAILURES to lock these in):")
            for case, metric in sorted(newly_passing):
                print(f"  Case {case} {metric}")

        if regressions:
            print("REGRESSIONS DETECTED:")
            for case, metric in sorted(regressions):
                print(f"  Case {case} {metric}")
            sys.exit(1)

        print("No regressions detected.")
        sys.exit(0)


if __name__ == "__main__":
    main()
