#!/usr/bin/env python3
"""
Reconstruct the ASHRAE 140 normative weather CSV from the in-repo EPW.

The 140 test cases reference the standard's normative TMY3 file
(`725650TY.csv`), which is part of the licensed ASHRAE 140-2023 accompanying
files and is therefore gitignored — CI checkouts don't have it. This script
regenerates it from `140_tests/weather/725650TYCST.epw` (the same Denver TMY
data, which IS in-repo), populating exactly the columns OpenBSE's TMY3 reader
consumes.

Fidelity: verified 2026-07-10 by running the full 30-case suite with both the
reconstruction and the original ASHRAE file — results identical to 0.1 kWh on
every case. Writing a TMY3 CSV (rather than pointing cases at the EPW) also
guarantees CI exercises the SAME weather-input code path as local validation:
the TMY3 path has no horizontal-IR column, so the Berdahl-Martin sky model is
engaged, exactly as in the validated runs.

Usage:
    python3 140_tests/scripts/reconstruct_140_weather.py [--force]

By default the script is a no-op if the target already exists (a real ASHRAE
original always wins); --force overwrites.
"""

import csv
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
TESTS_DIR = os.path.dirname(HERE)
EPW_PATH = os.path.join(TESTS_DIR, "weather", "725650TYCST.epw")
TARGET = os.path.join(
    TESTS_DIR,
    "cases",
    "ASHRAE 140-2023 Accompanying FIles",
    "Std140_TF_Files",
    "Normative Materials",
    "725650TY.csv",
)

# TMY3 location header for WMO 725650 (Denver Intl AP), matching the original.
LOCATION_HEADER = '725650,"DENVER INTL AP",CO,-7.0,39.833,-104.650,1650\n'


def main():
    force = "--force" in sys.argv

    if os.path.exists(TARGET) and not os.path.islink(TARGET) and not force:
        print(f"Target already exists (keeping it): {TARGET}")
        return 0

    if os.path.islink(TARGET) and not os.path.exists(TARGET):
        print("Removing broken symlink at target")
        os.unlink(TARGET)

    with open(EPW_PATH) as f:
        lines = f.read().splitlines()

    out_rows = []
    for line in lines[8:]:  # skip the 8 EPW header lines
        p = line.split(",")
        if len(p) < 24:
            continue
        year, month, day, hour = p[0], int(p[1]), int(p[2]), int(p[3])
        # EPW field indices (0-based): 6 drybulb, 7 dewpoint, 8 RH,
        # 9 pressure [Pa], 13 GHI, 14 DNI, 15 DHI, 20 wind dir,
        # 21 wind speed, 23 opaque sky cover.
        row = ["0"] * 68
        row[0] = f"{month:02d}/{day:02d}/{year}"
        row[1] = f"{hour:02d}:00"
        row[4] = p[13]  # GHI
        row[7] = p[14]  # DNI
        row[10] = p[15]  # DHI
        row[28] = p[23]  # opaque sky cover
        row[31] = p[6]  # dry bulb
        row[34] = p[7]  # dew point
        row[37] = p[8]  # RH
        row[40] = f"{float(p[9]) / 100.0:.1f}"  # Pa -> mbar
        row[43] = p[20]  # wind direction
        row[46] = p[21]  # wind speed
        out_rows.append(row)

    if len(out_rows) != 8760:
        print(f"ERROR: expected 8760 hours, got {len(out_rows)}", file=sys.stderr)
        return 1

    os.makedirs(os.path.dirname(TARGET), exist_ok=True)
    with open(TARGET, "w", newline="") as f:
        f.write(LOCATION_HEADER)
        f.write(",".join(["col"] * 68) + "\n")
        csv.writer(f).writerows(out_rows)

    print(f"Reconstructed {len(out_rows)} hours -> {TARGET}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
