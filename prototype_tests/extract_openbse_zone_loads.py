#!/usr/bin/env python3
"""
Extract annual zone-level heating and cooling loads from the OpenBSE
Large Office zone_results.csv.

The CSV contains hourly data (8760 rows) with columns like:
    <ZoneName>:zone_heating_rate [W]
    <ZoneName>:zone_cooling_rate [W]

Since the data is hourly, energy per timestep = power [W] * 1 hr = [Wh].
We sum over the year and convert to kWh (divide by 1000).

NOTE: OpenBSE outputs are already multiplier-scaled, so mid-floor zones
already include their x10 multiplier.
"""

import csv
from collections import OrderedDict

CSV_PATH = "/Users/benjaminbrannon/Documents/GitHub/OpenBSE/prototype_tests/large_office/LargeOffice_Boulder_zone_results.csv"

TIMESTEP_HR = 1.0  # hourly data


def main():
    with open(CSV_PATH, "r") as f:
        reader = csv.reader(f)
        headers = next(reader)

        # Identify heating and cooling columns
        heating_cols = {}  # zone_name -> column index
        cooling_cols = {}  # zone_name -> column index

        for i, col in enumerate(headers):
            if ":zone_heating_rate [W]" in col:
                zone_name = col.split(":zone_heating_rate")[0]
                heating_cols[zone_name] = i
            elif ":zone_cooling_rate [W]" in col:
                zone_name = col.split(":zone_cooling_rate")[0]
                cooling_cols[zone_name] = i

        # Collect all zone names (preserve order from CSV)
        zone_names = list(OrderedDict.fromkeys(
            list(heating_cols.keys()) + list(cooling_cols.keys())
        ))

        # Accumulators: sum of W over hourly timesteps -> Wh
        heating_sum = {z: 0.0 for z in zone_names}
        cooling_sum = {z: 0.0 for z in zone_names}
        row_count = 0

        for row in reader:
            row_count += 1
            for z in zone_names:
                if z in heating_cols:
                    val = float(row[heating_cols[z]])
                    heating_sum[z] += val * TIMESTEP_HR  # Wh
                if z in cooling_cols:
                    val = float(row[cooling_cols[z]])
                    cooling_sum[z] += val * TIMESTEP_HR  # Wh

    # Convert Wh -> kWh
    heating_kwh = {z: heating_sum[z] / 1000.0 for z in zone_names}
    cooling_kwh = {z: cooling_sum[z] / 1000.0 for z in zone_names}

    # Print summary table
    print(f"OpenBSE Large Office - Annual Zone Loads  ({row_count} hourly rows)")
    print("=" * 80)
    print(f"{'Zone':<35s} {'Heating (kWh)':>14s} {'Cooling (kWh)':>14s} {'Total (kWh)':>14s}")
    print("-" * 80)

    total_heating = 0.0
    total_cooling = 0.0

    for z in zone_names:
        h = heating_kwh[z]
        c = cooling_kwh[z]
        t = h + c
        total_heating += h
        total_cooling += c
        print(f"{z:<35s} {h:>14,.1f} {c:>14,.1f} {t:>14,.1f}")

    print("-" * 80)
    print(f"{'BUILDING TOTAL':<35s} {total_heating:>14,.1f} {total_cooling:>14,.1f} {total_heating + total_cooling:>14,.1f}")
    print("=" * 80)

    # Also print in GJ for quick cross-check
    kwh_to_gj = 3.6 / 1000.0
    print(f"\nBuilding total heating: {total_heating:,.1f} kWh  ({total_heating * kwh_to_gj:,.2f} GJ)")
    print(f"Building total cooling: {total_cooling:,.1f} kWh  ({total_cooling * kwh_to_gj:,.2f} GJ)")
    print(f"Building total loads:   {total_heating + total_cooling:,.1f} kWh  ({(total_heating + total_cooling) * kwh_to_gj:,.2f} GJ)")


if __name__ == "__main__":
    main()
