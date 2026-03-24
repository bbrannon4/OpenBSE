#!/usr/bin/env python3
"""Compare EnergyPlus vs OpenBSE hourly zone behavior for Core_mid zone."""

import pandas as pd
import numpy as np

# ── EnergyPlus data ──────────────────────────────────────────────────────────
ep = pd.read_csv(
    "/Users/benjaminbrannon/Documents/GitHub/OpenBSE/prototype_tests/large_office/eplus_run/eplusout.csv"
)

# Identify columns (E+ uppercases zone names)
ep_temp_col = [c for c in ep.columns if "CORE_MID" in c and "Zone Mean Air Temperature" in c][0]
ep_htg_col  = [c for c in ep.columns if "CORE_MID" in c and "Zone Air System Sensible Heating Rate" in c][0]
ep_clg_col  = [c for c in ep.columns if "CORE_MID" in c and "Zone Air System Sensible Cooling Rate" in c][0]

# Parse month from Date/Time string (E+ uses 24:00:00 which breaks datetime parsing)
# Format: " 01/01  01:00:00" — month is characters before the first "/"
ep["month"] = ep["Date/Time"].str.strip().str.split("/").str[0].astype(int)

ep_temp = ep[ep_temp_col].values
ep_htg  = ep[ep_htg_col].values
ep_clg  = ep[ep_clg_col].values

# E+ hourly data: each row is 1 hour.  Energy (Wh) = rate (W) * 1 h
# To get kWh: sum(W) * 1h / 1000

# ── OpenBSE data ─────────────────────────────────────────────────────────────
ob = pd.read_csv(
    "/Users/benjaminbrannon/Documents/GitHub/OpenBSE/prototype_tests/large_office/LargeOffice_Boulder_results.csv"
)

ob_temp_col = "Core_mid_f2:zone_temp [°C]"
ob_htg_col  = "Core_mid_f2:heating_load [W]"
ob_clg_col  = "Core_mid_f2:cooling_load [W]"

ob["month"] = ob["Month"].values

# OpenBSE has 6 substeps per hour → each substep = 10 min = 1/6 hour
substep_hours = 1.0 / 6.0

# ── Monthly aggregation ─────────────────────────────────────────────────────
months = range(1, 13)
month_names = ["Jan", "Feb", "Mar", "Apr", "May", "Jun",
               "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"]

print("=" * 100)
print(f"{'':4s} {'Zone Mean Air Temp (°C)':^30s}  {'Zone Heating (kWh)':^30s}  {'Zone Cooling (kWh)':^30s}")
print(f"{'Mo':4s} {'E+':>10s} {'OB':>10s} {'Diff':>8s}  {'E+':>10s} {'OB':>10s} {'Diff%':>8s}  {'E+':>10s} {'OB':>10s} {'Diff%':>8s}")
print("-" * 100)

ann_ep_htg = 0.0
ann_ep_clg = 0.0
ann_ob_htg = 0.0
ann_ob_clg = 0.0

for m in months:
    # E+ monthly
    mask_ep = ep["month"] == m
    ep_t = ep.loc[mask_ep, ep_temp_col].mean()
    ep_h = ep.loc[mask_ep, ep_htg_col].sum() * 1.0 / 1000.0   # W * 1h → Wh → /1000 → kWh
    ep_c = ep.loc[mask_ep, ep_clg_col].sum() * 1.0 / 1000.0

    # OB monthly
    mask_ob = ob["month"] == m
    ob_t = ob.loc[mask_ob, ob_temp_col].mean()
    ob_h = ob.loc[mask_ob, ob_htg_col].sum() * substep_hours / 1000.0  # W * (1/6)h → Wh → /1000 → kWh
    ob_c = ob.loc[mask_ob, ob_clg_col].sum() * substep_hours / 1000.0

    ann_ep_htg += ep_h
    ann_ep_clg += ep_c
    ann_ob_htg += ob_h
    ann_ob_clg += ob_c

    t_diff = ob_t - ep_t
    h_pct = ((ob_h - ep_h) / ep_h * 100) if ep_h != 0 else float('nan')
    c_pct = ((ob_c - ep_c) / ep_c * 100) if ep_c != 0 else float('nan')

    print(f"{month_names[m-1]:4s} {ep_t:10.2f} {ob_t:10.2f} {t_diff:+8.2f}  "
          f"{ep_h:10.1f} {ob_h:10.1f} {h_pct:+8.1f}%  "
          f"{ep_c:10.1f} {ob_c:10.1f} {c_pct:+8.1f}%")

print("-" * 100)

# Annual
h_pct_ann = ((ann_ob_htg - ann_ep_htg) / ann_ep_htg * 100) if ann_ep_htg != 0 else float('nan')
c_pct_ann = ((ann_ob_clg - ann_ep_clg) / ann_ep_clg * 100) if ann_ep_clg != 0 else float('nan')

print(f"{'Ann':4s} {ep.loc[:, ep_temp_col].mean():10.2f} {ob[ob_temp_col].mean():10.2f} "
      f"{ob[ob_temp_col].mean() - ep[ep_temp_col].mean():+8.2f}  "
      f"{ann_ep_htg:10.1f} {ann_ob_htg:10.1f} {h_pct_ann:+8.1f}%  "
      f"{ann_ep_clg:10.1f} {ann_ob_clg:10.1f} {c_pct_ann:+8.1f}%")
print("=" * 100)

print(f"\nNote: E+ 'Zone Air System Sensible Cooling Rate' is negative when cooling.")
print(f"      E+ heating/cooling are instantaneous rates [W] at hourly intervals (1 h timestep).")
print(f"      OB heating/cooling are instantaneous rates [W] at 10-min intervals (6 per hour).")
