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

# Section 9a cooling-equipment February-total ranges: {case: {metric: [lo, hi]}}
CE_RANGES = _RANGES.get("ce_february_ranges_kwh", {})

# Section 10 fuel-fired furnace ranges: {case: {metric: [lo, hi]}}
HE_RANGES = _RANGES.get("he_ranges", {})

# Section 11 air-side equipment ranges (single-zone AE200 series): {case: {metric: [lo, hi]}}
AE_RANGES = _RANGES.get("ae_ranges", {})

# Natural-gas higher heating value used to convert furnace fuel input (J) to a
# volumetric flow (m³/s) — 38 MJ/m³, from the reference programs' input/fuel
# ratio. HE cases run Jan–Mar (2160 h).
HE_GAS_HHV_J_PER_M3 = 38.0e6
HE_RUN_SECONDS = 2160 * 3600

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
KNOWN_FAILURES = set()  # All 251 checks currently pass (TF + CE + HE + AE200)


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


def _feb_rows(path):
    """Return the February hourly rows from an OpenBSE output CSV."""
    if not os.path.exists(path):
        return []
    with open(path) as f:
        return [r for r in csv.DictReader(f) if int(r["Month"]) == 2]


def _col_sum(rows, needle):
    """Sum a column (matched by substring) over rows, converting W·h → kWh."""
    if not rows:
        return 0.0
    keys = [k for k in rows[0] if needle in k]
    if not keys:
        return 0.0
    return sum(float(r[keys[0]]) for r in rows) / 1000.0


def read_ce_february(case):
    """Extract ASHRAE 140 Section 9a February totals for a CE case.

    Returns a dict of the gated metrics (kWh, plus COP), or None if the case
    output is missing. Cooling-energy split, coil load, and COP follow the
    Std140_CE_a glossary:
      cool_total = compressor + supply_fan + condenser_fan
      zone_sensible = coil_sensible − supply_fan_heat   (draw-through fan)
      cop = zone_total_load / cool_total
    """
    hvac = _feb_rows(os.path.join(BASE_DIR, f"ashrae140_case{case}_hvac_results.csv"))
    if not hvac:
        return None
    comp = _col_sum(hvac, "compressor_power")
    cond = _col_sum(hvac, "condenser_fan_power")
    sfan = _col_sum(hvac, "Supply Fan:electric_power")
    coil_total = _col_sum(hvac, "total_load")
    coil_sens = _col_sum(hvac, "sensible_load")
    coil_lat = _col_sum(hvac, "latent_load")
    cool_total = comp + cond + sfan
    zone_total = (coil_sens - sfan) + coil_lat  # fan heat is sensible only
    cop = zone_total / cool_total if cool_total > 0 else 0.0
    return {
        "cool_total": cool_total,
        "compressor": comp,
        "supply_fan": sfan,
        "cond_fan": cond,
        "coil_total": coil_total,
        "coil_sens": coil_sens,
        "coil_lat": coil_lat,
        "cop": cop,
    }


def read_he_results(case):
    """Extract ASHRAE 140 Section 10 furnace annual totals for an HE case.

    Returns furnace load and input (GJ), fuel consumption (m³/s), both-fans
    energy (kWh), and mean/max/min zone temperature (°C), or None if missing.
    """
    hp = os.path.join(BASE_DIR, f"ashrae140_case{case}_hvac_results.csv")
    zp = os.path.join(BASE_DIR, f"ashrae140_case{case}_zone_results.csv")
    if not os.path.exists(hp):
        return None
    with open(hp) as f:
        hvac = list(csv.DictReader(f))
    if not hvac:
        return None

    def wh_sum(needle):
        keys = [k for k in hvac[0] if needle in k]
        return sum(float(r[keys[0]]) for r in hvac) if keys else 0.0

    load_gj = wh_sum("Gas Furnace:thermal_output") * 3600 / 1e9
    input_gj = wh_sum("Gas Furnace:fuel_power") * 3600 / 1e9
    fuel_m3s = input_gj * 1e9 / (HE_GAS_HHV_J_PER_M3 * HE_RUN_SECONDS)
    fan_kwh = wh_sum("Circulating Fan:electric_power") / 1000.0
    result = {"load": load_gj, "input": input_gj, "fuel": fuel_m3s, "fan": fan_kwh}
    if os.path.exists(zp):
        with open(zp) as f:
            zrows = list(csv.DictReader(f))
        tkeys = [k for k in zrows[0] if "temperature" in k]
        if tkeys and zrows:
            temps = [float(r[tkeys[0]]) for r in zrows]
            result["meanT"] = sum(temps) / len(temps)
            result["maxT"] = max(temps)
            result["minT"] = min(temps)
    return result


HE_METRIC_LABELS = {
    "load": "Furnace Load (GJ)",
    "input": "Furnace Input (GJ)",
    "fuel": "Fuel Consumption (m3/s)",
    "fan": "Fan Energy (kWh)",
    "meanT": "Mean Zone Temp (C)",
    "maxT": "Max Zone Temp (C)",
    "minT": "Min Zone Temp (C)",
}


# Human-readable metric labels for the CE February-total checks.
CE_METRIC_LABELS = {
    "cool_total": "Feb Cooling Total (kWh)",
    "compressor": "Feb Compressor (kWh)",
    "supply_fan": "Feb Supply Fan (kWh)",
    "cond_fan": "Feb Condenser Fan (kWh)",
    "coil_total": "Feb Coil Load Total (kWh)",
    "coil_sens": "Feb Coil Load Sensible (kWh)",
    "coil_lat": "Feb Coil Load Latent (kWh)",
    "cop": "COP",
}


def read_ae_results(case):
    """Extract ASHRAE 140 Section 11 single-zone air-handler steady-state coil
    loads for an AE case: the last simulated hour's heating-coil load (QH) and
    cooling-coil sensible/latent/total loads (QCsens/QClat/QCtot), in kW.

    A case has only its active coil, so the absent coil's metrics read 0.0.
    Returns a dict keyed QH/QCsens/QClat/QCtot, or None if the file is missing.
    """
    hp = os.path.join(BASE_DIR, f"ashrae140_case{case}_hvac_results.csv")
    if not os.path.exists(hp):
        return None
    with open(hp) as f:
        hvac = list(csv.DictReader(f))
    if not hvac:
        return None
    last = hvac[-1]

    def col(needle):
        keys = [k for k in last if needle in k]
        return float(last[keys[0]]) / 1000.0 if keys else 0.0

    return {
        "QH": col("Heating Coil:thermal_output"),
        "QCsens": col("Cooling Coil:sensible_load"),
        "QClat": col("Cooling Coil:latent_load"),
        "QCtot": col("Cooling Coil:total_load"),
    }


AE_METRIC_LABELS = {
    "QH": "Heating Coil Load (kW)",
    "QCsens": "Cooling Coil Sensible (kW)",
    "QClat": "Cooling Coil Latent (kW)",
    "QCtot": "Cooling Coil Total (kW)",
}


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

    # --- Section 9a cooling-equipment cases (CE100–CE200) ---
    for case in sorted(CE_RANGES.keys(), key=lambda x: int(x[2:])):
        results = read_ce_february(case)
        if results is None:
            missing.append(case)
            continue
        ranges = CE_RANGES[case]
        for metric in ["cool_total", "compressor", "supply_fan", "cond_fan",
                       "coil_total", "coil_sens", "coil_lat", "cop"]:
            if metric not in ranges:
                continue
            lo, hi = ranges[metric]
            val = round(results[metric], 3 if metric == "cop" else 1)
            label = CE_METRIC_LABELS[metric]
            status, delta = evaluate(val, lo, hi)
            key = (case, label)
            if status == "PASS":
                pass_count += 1
                passed_keys.add(key)
                rows.append([case, label, val, lo, hi, status, "", ""])
            else:
                fail_count += 1
                failed_keys.add(key)
                pct = pct_delta(delta, lo, hi)
                rows.append([case, label, val, lo, hi, status, f"{delta:.1f}", pct])
                fail_details.append(f"  Case {case} {label}: OpenBSE={val}, "
                                    f"Range=[{lo}, {hi}], Delta={delta:.1f}")

    # --- Section 10 fuel-fired furnace cases (HE100–HE230) ---
    for case in sorted(HE_RANGES.keys(), key=lambda x: int(x[2:])):
        results = read_he_results(case)
        if results is None:
            missing.append(case)
            continue
        ranges = HE_RANGES[case]
        for metric in ["load", "input", "fuel", "fan", "meanT", "maxT", "minT"]:
            if metric not in ranges or metric not in results:
                continue
            lo, hi = ranges[metric]
            digits = 6 if metric == "fuel" else 3
            val = round(results[metric], digits)
            label = HE_METRIC_LABELS[metric]
            status, delta = evaluate(val, lo, hi)
            key = (case, label)
            if status == "PASS":
                pass_count += 1
                passed_keys.add(key)
                rows.append([case, label, val, lo, hi, status, "", ""])
            else:
                fail_count += 1
                failed_keys.add(key)
                pct = pct_delta(delta, lo, hi)
                rows.append([case, label, val, lo, hi, status, f"{delta:.4g}", pct])
                fail_details.append(f"  Case {case} {label}: OpenBSE={val}, "
                                    f"Range=[{lo}, {hi}], Delta={delta:.4g}")

    # --- Section 11 air-side equipment cases (AE200 single-zone series) ---
    for case in sorted(AE_RANGES.keys(), key=lambda x: int(x[2:])):
        results = read_ae_results(case)
        if results is None:
            missing.append(case)
            continue
        ranges = AE_RANGES[case]
        for metric in ["QH", "QCsens", "QClat", "QCtot"]:
            if metric not in ranges or metric not in results:
                continue
            lo, hi = ranges[metric]
            val = round(results[metric], 4)
            label = AE_METRIC_LABELS[metric]
            status, delta = evaluate(val, lo, hi)
            key = (case, label)
            if status == "PASS":
                pass_count += 1
                passed_keys.add(key)
                rows.append([case, label, val, lo, hi, status, "", ""])
            else:
                fail_count += 1
                failed_keys.add(key)
                pct = pct_delta(delta, lo, hi)
                rows.append([case, label, val, lo, hi, status, f"{delta:.4g}", pct])
                fail_details.append(f"  Case {case} {label}: OpenBSE={val}, "
                                    f"Range=[{lo}, {hi}], Delta={delta:.4g}")

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
