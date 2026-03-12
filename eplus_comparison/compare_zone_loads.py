#!/usr/bin/env python3
"""
Extract annual zone-level heating and cooling loads from EnergyPlus output
for the Large Office model.

The CSV contains hourly "Zone Air System Sensible Heating Rate [W]" and
"Zone Air System Sensible Cooling Rate [W]" columns. Since the data is hourly,
each row represents a 1-hour interval, so summing the rates (W) directly gives
energy in Wh.

Mid-floor zones have a zone multiplier of 10 in the IDF.
"""

import csv
import sys
from collections import OrderedDict

CSV_PATH = "/Users/benjaminbrannon/Documents/GitHub/OpenBSE/eplus_comparison/output_office_simplified/eplusout.csv"

# Zone multipliers: mid-floor zones are multiplied by 10
# (they represent 10 identical floors in the Large Office DOE prototype)
ZONE_MULTIPLIERS = {
    "PERIMETER_MID_ZN_1": 10,
    "PERIMETER_MID_ZN_2": 10,
    "PERIMETER_MID_ZN_3": 10,
    "PERIMETER_MID_ZN_4": 10,
    "CORE_MID": 10,
    "MIDFLOOR_PLENUM": 10,
    "DATACENTER_MID_ZN_6": 10,
}

J_PER_KWH = 3_600_000  # 1 kWh = 3.6e6 J
WH_PER_KWH = 1_000      # 1 kWh = 1000 Wh


def main():
    # --- Read header to discover zone heating/cooling columns ---
    with open(CSV_PATH, "r") as f:
        reader = csv.reader(f)
        header = next(reader)

    # Build mapping: zone_name -> (heating_col_index, cooling_col_index)
    heating_suffix = ":Zone Air System Sensible Heating Rate [W](Hourly)"
    cooling_suffix = ":Zone Air System Sensible Cooling Rate [W](Hourly)"

    zone_heating_cols = {}  # zone_name -> col index
    zone_cooling_cols = {}  # zone_name -> col index

    for i, col in enumerate(header):
        if col.endswith(heating_suffix):
            zone_name = col[: -len(heating_suffix)]
            zone_heating_cols[zone_name] = i
        elif col.endswith(cooling_suffix):
            zone_name = col[: -len(cooling_suffix)]
            zone_cooling_cols[zone_name] = i

    # All zones that have both heating and cooling columns
    zones = sorted(set(zone_heating_cols.keys()) & set(zone_cooling_cols.keys()))

    if not zones:
        print("ERROR: No zone heating/cooling columns found in CSV.", file=sys.stderr)
        sys.exit(1)

    # --- Accumulate hourly sums ---
    heating_wh = {z: 0.0 for z in zones}
    cooling_wh = {z: 0.0 for z in zones}
    row_count = 0

    with open(CSV_PATH, "r") as f:
        reader = csv.reader(f)
        next(reader)  # skip header
        for row in reader:
            if not row or not row[0].strip():
                continue
            row_count += 1
            for z in zones:
                h_val = float(row[zone_heating_cols[z]])
                c_val = float(row[zone_cooling_cols[z]])
                # Each row is 1 hour, so W * 1h = Wh
                heating_wh[z] += h_val
                cooling_wh[z] += c_val

    print(f"Processed {row_count} hourly rows from E+ output\n")

    # --- Apply multipliers and convert to kWh ---
    results = OrderedDict()
    for z in zones:
        mult = ZONE_MULTIPLIERS.get(z, 1)
        h_kwh = heating_wh[z] * mult / WH_PER_KWH
        c_kwh = cooling_wh[z] * mult / WH_PER_KWH
        results[z] = {
            "multiplier": mult,
            "heating_kwh": h_kwh,
            "cooling_kwh": c_kwh,
            "total_kwh": h_kwh + c_kwh,
        }

    # --- Print summary table ---
    hdr_fmt = "{:<35s} {:>5s} {:>14s} {:>14s} {:>14s}"
    row_fmt = "{:<35s} {:>5d} {:>14,.1f} {:>14,.1f} {:>14,.1f}"
    sep = "-" * 87

    print(hdr_fmt.format("Zone", "Mult", "Heating (kWh)", "Cooling (kWh)", "Total (kWh)"))
    print(sep)

    total_heating = 0.0
    total_cooling = 0.0

    for z, r in results.items():
        print(row_fmt.format(z, r["multiplier"], r["heating_kwh"], r["cooling_kwh"], r["total_kwh"]))
        total_heating += r["heating_kwh"]
        total_cooling += r["cooling_kwh"]

    print(sep)
    print("{:<35s} {:>5s} {:>14,.1f} {:>14,.1f} {:>14,.1f}".format(
        "BUILDING TOTAL", "", total_heating, total_cooling, total_heating + total_cooling
    ))

    # Also print in MWh for easier reading
    print()
    print(f"Building total heating: {total_heating/1000:,.2f} MWh")
    print(f"Building total cooling: {total_cooling/1000:,.2f} MWh")
    print(f"Building total loads:   {(total_heating + total_cooling)/1000:,.2f} MWh")


if __name__ == "__main__":
    main()
