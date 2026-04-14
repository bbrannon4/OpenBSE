#!/usr/bin/env python3
"""Plot net zone load (heating negative, cooling positive) for living zone: E+ vs OB on 4 typical days."""

import csv
import os
from collections import defaultdict

import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
import numpy as np

BASE = os.path.dirname(os.path.abspath(__file__))
EP_CSV = os.path.join(BASE, "eplus_ideal_run", "eplusout.csv")
OB_ZONE = os.path.join(BASE, "SingleFamily_CZ5B_Boulder_ideal_ideal_zone_debug.csv")
OUT_DIR = os.path.join(BASE, "comparison_plots")
os.makedirs(OUT_DIR, exist_ok=True)

DAYS = {
    "Winter (Jan 15)": (1, 15),
    "Spring (Apr 15)": (4, 15),
    "Summer (Jul 15)": (7, 15),
    "Fall (Oct 15)":   (10, 15),
}

# ── Read E+ CSV ──
def read_ep():
    with open(EP_CSV, 'r') as f:
        reader = csv.reader(f)
        headers = [h.strip() for h in next(reader)]
        ncols = len(headers)
        data = [[] for _ in range(ncols)]
        for row in reader:
            date_str = row[0].strip() if row else ""
            if not date_str or '/' not in date_str:
                continue
            for i in range(ncols):
                if i >= len(row):
                    data[i].append(0.0 if i > 0 else date_str)
                else:
                    try:
                        data[i].append(float(row[i].strip()) if i > 0 else date_str)
                    except ValueError:
                        data[i].append(0.0 if i > 0 else date_str)
    return headers, data

def ep_col_idx(headers, partial):
    p = partial.upper()
    for i, h in enumerate(headers):
        if p in h.upper():
            return i
    return None

def ep_extract_day(headers, data, col_idx, month, day):
    hourly = defaultdict(list)
    for ri, ds in enumerate(data[0]):
        if not isinstance(ds, str):
            continue
        try:
            parts = ds.strip().split()
            md = parts[0].split('/')
            m, d = int(md[0]), int(md[1])
            hms = parts[1].split(':')
            h, minute = int(hms[0]), int(hms[1])
        except (ValueError, IndexError):
            continue
        if m == month and d == day:
            if h == 0 and minute == 0:
                continue
            ah = h if minute > 0 else h - 1
            ah = max(0, min(23, ah))
            hourly[ah].append(data[col_idx][ri])
    result = np.zeros(24)
    for h in range(24):
        if hourly[h]:
            result[h] = np.mean(hourly[h])
    return result

# ── Read OB CSV ──
def read_ob():
    with open(OB_ZONE, 'r') as f:
        reader = csv.reader(f)
        headers = [h.strip() for h in next(reader)]
        result = {h: [] for h in headers}
        for row in reader:
            for i, h in enumerate(headers):
                try:
                    result[h].append(float(row[i]))
                except (ValueError, IndexError):
                    result[h].append(0.0)
    return headers, result

def ob_extract_day(ob_data, col_name, month, day):
    months = ob_data.get("Month", [])
    days = ob_data.get("Day", [])
    hours = ob_data.get("Hour", [])
    vals = ob_data.get(col_name, [])
    if not vals:
        return np.zeros(24)
    hourly = defaultdict(list)
    for i in range(len(months)):
        if int(months[i]) == month and int(days[i]) == day:
            hourly[int(hours[i]) - 1].append(vals[i])  # OB hours are 1-indexed
    result = np.zeros(24)
    for h in range(24):
        if hourly[h]:
            result[h] = np.mean(hourly[h])
    return result

# ── Main ──
print("Reading E+ data...")
ep_h, ep_d = read_ep()
print("Reading OB data...")
ob_h, ob_d = read_ob()

# E+ columns: heating and cooling energy [J] per timestep
ep_heat_idx = ep_col_idx(ep_h, "IDEAL LOADS LIVING:Zone Ideal Loads Zone Sensible Heating Energy")
ep_cool_idx = ep_col_idx(ep_h, "IDEAL LOADS LIVING:Zone Ideal Loads Zone Sensible Cooling Energy")

# Also try rate [W] columns as fallback
if ep_heat_idx is None:
    ep_heat_idx = ep_col_idx(ep_h, "IDEAL LOADS LIVING:Zone Ideal Loads Zone Sensible Heating Rate")
if ep_cool_idx is None:
    ep_cool_idx = ep_col_idx(ep_h, "IDEAL LOADS LIVING:Zone Ideal Loads Zone Sensible Cooling Rate")

print(f"E+ heating col: {ep_h[ep_heat_idx] if ep_heat_idx else 'NOT FOUND'}")
print(f"E+ cooling col: {ep_h[ep_cool_idx] if ep_cool_idx else 'NOT FOUND'}")

# Detect if energy [J] or rate [W]
ep_heat_is_energy = ep_heat_idx and '[J]' in ep_h[ep_heat_idx]
ep_cool_is_energy = ep_cool_idx and '[J]' in ep_h[ep_cool_idx]

# OB columns
ob_heat_col = [c for c in ob_h if "living_unit1" in c.lower() and "heating_rate" in c.lower()]
ob_cool_col = [c for c in ob_h if "living_unit1" in c.lower() and "cooling_rate" in c.lower()]
print(f"OB heating col: {ob_heat_col}")
print(f"OB cooling col: {ob_cool_col}")

fig, axes = plt.subplots(2, 2, figsize=(14, 9), sharex=True)
axes = axes.flatten()
hours = np.arange(24)

for idx, (label, (month, day)) in enumerate(DAYS.items()):
    ax = axes[idx]

    # E+ net load: negative=heating, positive=cooling
    ep_heat = ep_extract_day(ep_h, ep_d, ep_heat_idx, month, day) if ep_heat_idx else np.zeros(24)
    ep_cool = ep_extract_day(ep_h, ep_d, ep_cool_idx, month, day) if ep_cool_idx else np.zeros(24)

    # Convert J to W if needed (energy per timestep → average rate)
    if ep_heat_is_energy:
        ep_heat = ep_heat / 600.0  # J per 10-min timestep → W
    if ep_cool_is_energy:
        ep_cool = ep_cool / 600.0

    # Net load: cooling positive, heating negative
    ep_net = ep_cool - ep_heat

    # OB net load
    ob_heat = ob_extract_day(ob_d, ob_heat_col[0], month, day) if ob_heat_col else np.zeros(24)
    ob_cool = ob_extract_day(ob_d, ob_cool_col[0], month, day) if ob_cool_col else np.zeros(24)
    ob_net = ob_cool - ob_heat

    ax.plot(hours, ep_net, 'b-o', markersize=3, linewidth=1.5, label='E+')
    ax.plot(hours, ob_net, 'r-s', markersize=3, linewidth=1.5, label='OB')
    ax.axhline(0, color='gray', linewidth=0.5, linestyle='--')
    ax.fill_between(hours, ep_net, ob_net, alpha=0.15, color='purple')
    ax.set_title(label, fontsize=12, fontweight='bold')
    ax.set_ylabel('Net Zone Load [W]\n(+cooling / −heating)')
    ax.set_xlim(0, 23)
    ax.grid(True, alpha=0.3)
    ax.legend(fontsize=9)

axes[2].set_xlabel('Hour of Day')
axes[3].set_xlabel('Hour of Day')

fig.suptitle('Living Zone Net Load: E+ vs OpenBSE', fontsize=14, fontweight='bold')
plt.tight_layout()
out = os.path.join(OUT_DIR, "net_zone_load.png")
plt.savefig(out, dpi=150)
print(f"Saved: {out}")
