#!/usr/bin/env python3
"""Compare zone-level heating/cooling loads: E+ vs OpenBSE (Large Office)."""

import csv
import sys

# ── E+ zone loads (from Zone Air System Sensible Heating/Cooling Rate) ──
# Values already include zone multipliers (mid zones x10)
eplus = {}
eplus_csv = "output_office_simplified/eplusout.csv"
with open(eplus_csv, "r") as f:
    reader = csv.reader(f)
    header = next(reader)

    # Find heating and cooling columns
    heat_cols = {}  # zone_name -> col_idx
    cool_cols = {}
    for i, h in enumerate(header):
        h_upper = h.strip().upper()
        if "ZONE AIR SYSTEM SENSIBLE HEATING RATE" in h_upper:
            zone = h.split(":")[0].strip().upper()
            heat_cols[zone] = i
        elif "ZONE AIR SYSTEM SENSIBLE COOLING RATE" in h_upper:
            zone = h.split(":")[0].strip().upper()
            cool_cols[zone] = i

    # Accumulate (W → kWh: hourly data, so W = Wh per row, /1000 = kWh)
    heat_sums = {z: 0.0 for z in heat_cols}
    cool_sums = {z: 0.0 for z in cool_cols}
    n_rows = 0
    for row in reader:
        if not row or row[0].strip() == "":
            continue
        n_rows += 1
        for z, ci in heat_cols.items():
            try: heat_sums[z] += float(row[ci])
            except: pass
        for z, ci in cool_cols.items():
            try: cool_sums[z] += float(row[ci])
            except: pass

# Apply multipliers
mid_zones = {"CORE_MID", "PERIMETER_MID_ZN_1", "PERIMETER_MID_ZN_2",
             "PERIMETER_MID_ZN_3", "PERIMETER_MID_ZN_4",
             "MIDFLOOR_PLENUM", "DATACENTER_MID_ZN_6"}
for z in heat_sums:
    mult = 10 if z in mid_zones else 1
    heat_sums[z] = heat_sums[z] / 1000.0 * mult  # kWh
for z in cool_sums:
    mult = 10 if z in mid_zones else 1
    cool_sums[z] = cool_sums[z] / 1000.0 * mult

# ── OpenBSE zone loads ──
obse = {}
obse_csv = "LargeOffice_Boulder_zone_results.csv"
with open(obse_csv, "r") as f:
    reader = csv.reader(f)
    header = next(reader)

    heat_cols_o = {}
    cool_cols_o = {}
    for i, h in enumerate(header):
        if ":zone_heating_rate" in h.lower():
            zone = h.split(":")[0].strip()
            heat_cols_o[zone] = i
        elif ":zone_cooling_rate" in h.lower():
            zone = h.split(":")[0].strip()
            cool_cols_o[zone] = i

    heat_sums_o = {z: 0.0 for z in heat_cols_o}
    cool_sums_o = {z: 0.0 for z in cool_cols_o}
    for row in reader:
        if not row or row[0].strip() == "":
            continue
        for z, ci in heat_cols_o.items():
            try: heat_sums_o[z] += float(row[ci])
            except: pass
        for z, ci in cool_cols_o.items():
            try: cool_sums_o[z] += float(row[ci])
            except: pass

# Convert W*hr to kWh (already multiplier-scaled in OpenBSE)
for z in heat_sums_o:
    heat_sums_o[z] /= 1000.0
for z in cool_sums_o:
    cool_sums_o[z] /= 1000.0

# ── Zone name mapping (OpenBSE → E+ uppercase) ──
zone_map = {}
ep_upper = {k.upper(): k for k in heat_sums}
for oz in heat_cols_o:
    ez = oz.upper()
    if ez in ep_upper:
        zone_map[oz] = ez
    else:
        # Try without case
        for ek in heat_sums:
            if ek.upper() == ez:
                zone_map[oz] = ek
                break

# ── Print comparison ──
print("=" * 120)
print("Zone-Level Load Comparison: E+ vs OpenBSE (Large Office, Boulder)")
print("=" * 120)

print(f"\n{'Zone':<30} {'E+ Heat':>10} {'OB Heat':>10} {'Δ Heat%':>8}   {'E+ Cool':>10} {'OB Cool':>10} {'Δ Cool%':>8}")
print(f"{'':30} {'[kWh]':>10} {'[kWh]':>10} {'':>8}   {'[kWh]':>10} {'[kWh]':>10} {'':>8}")
print("-" * 120)

total_ep_h = 0; total_ob_h = 0; total_ep_c = 0; total_ob_c = 0

# Sort by E+ cooling load descending for readability
sorted_zones = sorted(zone_map.items(), key=lambda x: cool_sums.get(x[1], 0), reverse=True)

for oz, ez in sorted_zones:
    ep_h = heat_sums.get(ez, 0)
    ob_h = heat_sums_o.get(oz, 0)
    ep_c = cool_sums.get(ez, 0)
    ob_c = cool_sums_o.get(oz, 0)

    dh = ((ob_h - ep_h) / ep_h * 100) if ep_h > 10 else float('nan')
    dc = ((ob_c - ep_c) / ep_c * 100) if ep_c > 10 else float('nan')

    dh_str = f"{dh:+.1f}%" if not (dh != dh) else "n/a"
    dc_str = f"{dc:+.1f}%" if not (dc != dc) else "n/a"

    total_ep_h += ep_h; total_ob_h += ob_h
    total_ep_c += ep_c; total_ob_c += ob_c

    print(f"{oz:<30} {ep_h:>10,.0f} {ob_h:>10,.0f} {dh_str:>8}   {ep_c:>10,.0f} {ob_c:>10,.0f} {dc_str:>8}")

# E+-only zones (plenums)
print("-" * 120)
print("E+-only zones (no OpenBSE equivalent):")
for ez in sorted(heat_sums.keys()):
    if ez not in zone_map.values():
        ep_h = heat_sums.get(ez, 0)
        ep_c = cool_sums.get(ez, 0)
        total_ep_h += ep_h; total_ep_c += ep_c
        if ep_h > 1 or ep_c > 1:
            print(f"  {ez:<28} {ep_h:>10,.0f} {'---':>10} {'':>8}   {ep_c:>10,.0f} {'---':>10} {'':>8}")

print("-" * 120)
dth = (total_ob_h - total_ep_h) / total_ep_h * 100
dtc = (total_ob_c - total_ep_c) / total_ep_c * 100
print(f"{'BUILDING TOTAL':<30} {total_ep_h:>10,.0f} {total_ob_h:>10,.0f} {dth:>+7.1f}%   {total_ep_c:>10,.0f} {total_ob_c:>10,.0f} {dtc:>+7.1f}%")
print("=" * 120)

# ── Highlight biggest discrepancies ──
print("\n── Largest Heating Discrepancies (absolute kWh) ──")
diffs = []
for oz, ez in zone_map.items():
    ep_h = heat_sums.get(ez, 0)
    ob_h = heat_sums_o.get(oz, 0)
    diffs.append((oz, ob_h - ep_h, ep_h, ob_h))
diffs.sort(key=lambda x: abs(x[1]), reverse=True)
for oz, d, ep, ob in diffs[:8]:
    pct = d / ep * 100 if ep > 10 else float('nan')
    pct_str = f"{pct:+.0f}%" if not (pct != pct) else "n/a"
    print(f"  {oz:<30}  E+: {ep:>10,.0f}  OB: {ob:>10,.0f}  Δ: {d:>+10,.0f} kWh  ({pct_str})")

print("\n── Largest Cooling Discrepancies (absolute kWh) ──")
diffs_c = []
for oz, ez in zone_map.items():
    ep_c = cool_sums.get(ez, 0)
    ob_c = cool_sums_o.get(oz, 0)
    diffs_c.append((oz, ob_c - ep_c, ep_c, ob_c))
diffs_c.sort(key=lambda x: abs(x[1]), reverse=True)
for oz, d, ep, ob in diffs_c[:8]:
    pct = d / ep * 100 if ep > 10 else float('nan')
    pct_str = f"{pct:+.0f}%" if not (pct != pct) else "n/a"
    print(f"  {oz:<30}  E+: {ep:>10,.0f}  OB: {ob:>10,.0f}  Δ: {d:>+10,.0f} kWh  ({pct_str})")
