#!/usr/bin/env python3
"""Compare OpenBSE vs E+ ideal loads: hourly load components on representative days."""

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
OB_SURF = os.path.join(BASE, "SingleFamily_CZ5B_Boulder_ideal_ideal_surface_debug.csv")
OUT_DIR = os.path.join(BASE, "comparison_plots")
os.makedirs(OUT_DIR, exist_ok=True)

TIMESTEPS_PER_HOUR = 6
DT = 3600.0 / TIMESTEPS_PER_HOUR  # 600 seconds

# ── Representative days (month, day) ──
DAYS = {
    "Winter Cold (Jan 15)": (1, 15),
    "Spring Mild (Apr 15)": (4, 15),
    "Summer Hot (Jul 15)": (7, 15),
    "Fall Mild (Oct 15)": (10, 15),
}

# ── E+ surface name → OpenBSE surface name mapping ──
# For aggregation into categories. E+ names are UPPER CASE in CSV.
# Living zone exterior walls (1F: LDF=South, SDR=East, LDB=North, SDL=West)
EP_WALL_COND = [
    "WALL_LDF_1.UNIT1", "WALL_SDR_1.UNIT1", "WALL_LDB_1.UNIT1", "WALL_SDL_1.UNIT1",
    "WALL_LDF_2.UNIT1", "WALL_SDR_2.UNIT1", "WALL_LDB_2.UNIT1", "WALL_SDL_2.UNIT1",
    "DOOR_LDB_UNIT1",  # back door (opaque, exterior)
]
OB_WALL_COND = [
    "Wall South 1F", "Wall East 1F", "Wall North 1F", "Wall West 1F",
    "Wall South 2F", "Wall East 2F", "Wall North 2F", "Wall West 2F",
    "Back Door",
]

# Floor (living to basement interzone)
EP_FLOOR_COND = ["FLOOR_UNIT1"]
OB_FLOOR_COND = ["Floor"]

# Ceiling (living to attic interzone)
EP_CEILING_COND = ["CEILING_UNIT1"]
OB_CEILING_COND = ["Ceiling"]

# Internal mass
EP_INTMASS_COND = ["INTERNALMASS_UNIT1"]
OB_INTMASS_COND = ["living_unit1 IntMass 1"]  # OpenBSE auto-names internal mass

# Inter-floor slab (adiabatic in both)
EP_INTERFLOOR_COND = ["INTER ZONE FLOOR 1"]
OB_INTERFLOOR_COND = ["Inter-floor Slab"]

# Interzone walls to garage
EP_GARAGE_IZ_COND = ["WALL_LDB_1.GARAGE1", "INT_DOOR_GARAGE1"]
OB_GARAGE_IZ_COND = ["Wall South 1F Garage", "Door to Garage"]

# Windows (transmitted solar)
EP_WINDOWS_SOLAR = [
    "WINDOW_LDF_1.UNIT1", "WINDOW_LDB_1.UNIT1", "WINDOW_SDR_1.UNIT1", "WINDOW_SDL_1.UNIT1",
    "WINDOW_LDF_2.UNIT1", "WINDOW_LDB_2.UNIT1", "WINDOW_SDR_2.UNIT1", "WINDOW_SDL_2.UNIT1",
]
OB_WINDOWS_SOLAR = [
    "Window South 1F", "Window North 1F", "Window East 1F", "Window West 1F",
    "Window South 2F", "Window North 2F", "Window East 2F", "Window West 2F",
]


def read_ep_csv(path):
    """Read E+ CSV, return (headers, data) where data[col_idx] = list of floats."""
    with open(path, 'r') as f:
        reader = csv.reader(f)
        headers = next(reader)
        headers = [h.strip() for h in headers]
        # Build column data
        ncols = len(headers)
        data = [[] for _ in range(ncols)]
        for row in reader:
            if len(row) < 2:
                continue
            # Skip monthly rows (they have fewer filled fields or different date format)
            date_str = row[0].strip()
            if not date_str or '/' not in date_str:
                continue
            # Parse date: " 01/01  00:10:00"
            try:
                parts = date_str.split()
                md = parts[0].split('/')
                month = int(md[0])
                day = int(md[1])
                hms = parts[1].split(':')
                hour = int(hms[0])
                minute = int(hms[1])
            except (ValueError, IndexError):
                continue
            # Skip the 24:00:00 row at end of day (it's the last timestep of the day)
            # and monthly summary rows
            row_len = len(row)
            for i in range(ncols):
                if i >= row_len:
                    data[i].append(0.0 if i > 0 else date_str)
                else:
                    try:
                        data[i].append(float(row[i].strip()) if i > 0 else date_str)
                    except (ValueError, IndexError):
                        data[i].append(0.0 if i > 0 else date_str)
    return headers, data


def read_ob_csv(path):
    """Read OpenBSE CSV, return (headers, data) where data[col_name] = list of floats."""
    with open(path, 'r') as f:
        reader = csv.reader(f)
        headers = next(reader)
        headers = [h.strip() for h in headers]
        result = {h: [] for h in headers}
        for row in reader:
            for i, h in enumerate(headers):
                try:
                    result[h].append(float(row[i]))
                except (ValueError, IndexError):
                    result[h].append(0.0)
    return headers, result


def ep_col_idx(headers, partial_name):
    """Find column index by partial match (case-insensitive)."""
    pn = partial_name.upper()
    for i, h in enumerate(headers):
        if pn in h.upper():
            return i
    return None


def ep_get_surface_cond(headers, data, surface_name):
    """Get conduction data for an E+ surface."""
    key = f"{surface_name}:Surface Inside Face Conduction Heat Transfer Rate"
    idx = ep_col_idx(headers, key)
    if idx is None:
        return None
    return data[idx]


def extract_day(values, month, day, tph=6):
    """Extract 24 hourly averages for a given day from timestep data."""
    # OpenBSE: Month, Day, Hour, SubHour columns -> filter
    # Returns 24 values (hourly averages)
    return None  # Handled per-source below


def ep_extract_day(headers, data, col_idx, target_month, target_day):
    """Extract hourly averages for one day from E+ timestep data."""
    # E+ date format: " MM/DD  HH:MM:SS"
    hourly = defaultdict(list)
    for row_i, date_str in enumerate(data[0]):
        if not isinstance(date_str, str):
            continue
        try:
            parts = date_str.strip().split()
            md = parts[0].split('/')
            m, d = int(md[0]), int(md[1])
            hms = parts[1].split(':')
            h = int(hms[0])
            minute = int(hms[1])
        except (ValueError, IndexError):
            continue
        if m == target_month and d == target_day:
            # E+ end-of-interval timestamps:
            #   01:00:00 (m=0) = last substep of hour 0 → actual_hour = h-1
            #   01:10:00 (m>0) = first substep of hour 1 → actual_hour = h
            if h == 0 and minute == 0:
                continue  # skip midnight boundary
            actual_hour = h if minute > 0 else h - 1
            actual_hour = max(0, min(23, actual_hour))
            hourly[actual_hour].append(data[col_idx][row_i])
    result = np.zeros(24)
    for h in range(24):
        if hourly[h]:
            result[h] = np.mean(hourly[h])
    return result


def ob_extract_day(ob_data, col_name, target_month, target_day):
    """Extract hourly averages for one day from OpenBSE timestep data."""
    months = ob_data.get("Month", [])
    days = ob_data.get("Day", [])
    hours = ob_data.get("Hour", [])
    vals = ob_data.get(col_name, [])
    if not vals:
        return np.zeros(24)
    hourly = defaultdict(list)
    for i in range(len(months)):
        m, d, h = int(months[i]), int(days[i]), int(hours[i])
        if m == target_month and d == target_day:
            # OpenBSE Hour is 1-24 (end-of-hour convention), map to 0-23
            hourly[h - 1].append(vals[i])
    result = np.zeros(24)
    for h in range(24):
        if hourly[h]:
            result[h] = np.mean(hourly[h])
    return result


def ep_sum_surfaces_day(ep_h, ep_d, surface_names, var_suffix, month, day):
    """Sum conduction across multiple E+ surfaces for one day."""
    total = np.zeros(24)
    for sname in surface_names:
        key = f"{sname}:{var_suffix}"
        idx = ep_col_idx(ep_h, key)
        if idx is not None:
            total += ep_extract_day(ep_h, ep_d, idx, month, day)
    return total


def ob_sum_surfaces_day(ob_data, surface_names, col_suffix, month, day):
    """Sum conduction across multiple OpenBSE surfaces for one day."""
    total = np.zeros(24)
    for sname in surface_names:
        col = f"{sname}:{col_suffix}"
        # Try exact match first, then with unit suffix
        matched = None
        for k in ob_data.keys():
            if k.startswith(sname) and col_suffix.replace("_", " ") in k.lower():
                matched = k
                break
            # Try simpler match
            base = k.split(' [')[0].strip()
            if base == f"{sname}:{col_suffix}":
                matched = k
                break
        if matched:
            total += ob_extract_day(ob_data, matched, month, day)
    return total


def find_ob_col(ob_data, surface_name, var_type):
    """Find the OpenBSE column name for a surface variable."""
    for k in ob_data.keys():
        # Column format: "Surface Name:var_type [unit]"
        if k.startswith(f"{surface_name}:") and var_type in k:
            return k
    return None


def main():
    print("Reading E+ data...")
    ep_h, ep_d = read_ep_csv(EP_CSV)
    print(f"  {len(ep_h)} columns, {len(ep_d[0])} timesteps")

    print("Reading OpenBSE zone data...")
    ob_zh, ob_zd = read_ob_csv(OB_ZONE)
    print(f"  {len(ob_zh)} columns, {len(ob_zd['Month'])} timesteps")

    print("Reading OpenBSE surface data...")
    ob_sh, ob_sd = read_ob_csv(OB_SURF)
    print(f"  {len(ob_sh)} columns, {len(ob_sd['Month'])} timesteps")

    hours = np.arange(24)

    for day_label, (month, day) in DAYS.items():
        print(f"\n{'='*60}")
        print(f"  {day_label} (month={month}, day={day})")
        print(f"{'='*60}")

        fig, axes = plt.subplots(4, 3, figsize=(20, 22))
        fig.suptitle(f"OpenBSE vs E+ Ideal Loads — {day_label}", fontsize=16, y=0.98)

        # ── 1. Heating Rate ──
        ax = axes[0, 0]
        ep_heat = ep_extract_day(ep_h, ep_d,
            ep_col_idx(ep_h, "Zone Ideal Loads Zone Sensible Heating Rate"), month, day)
        ob_heat = ob_extract_day(ob_zd, "living_unit1:heating_rate [W]", month, day)
        ax.plot(hours, ep_heat, 'b-', label='E+', linewidth=1.5)
        ax.plot(hours, ob_heat, 'r--', label='OB', linewidth=1.5)
        ax.set_title("Heating Rate [W]")
        ax.legend()
        ax.set_xlabel("Hour")
        ax.grid(True, alpha=0.3)

        # ── 2. Cooling Rate ──
        ax = axes[0, 1]
        ep_cool = ep_extract_day(ep_h, ep_d,
            ep_col_idx(ep_h, "Zone Ideal Loads Zone Sensible Cooling Rate"), month, day)
        ob_cool = ob_extract_day(ob_zd, "living_unit1:cooling_rate [W]", month, day)
        ax.plot(hours, ep_cool, 'b-', label='E+', linewidth=1.5)
        ax.plot(hours, ob_cool, 'r--', label='OB', linewidth=1.5)
        ax.set_title("Cooling Rate [W]")
        ax.legend()
        ax.set_xlabel("Hour")
        ax.grid(True, alpha=0.3)

        # ── 3. Zone Temperatures ──
        ax = axes[0, 2]
        ep_tz = ep_extract_day(ep_h, ep_d,
            ep_col_idx(ep_h, "LIVING_UNIT1:Zone Mean Air Temperature"), month, day)
        ob_tz = ob_extract_day(ob_zd, "living_unit1:temperature [\u00b0C]", month, day)
        # Also plot outdoor temp
        ob_tout = ob_extract_day(ob_zd, "Site:outdoor_temperature [\u00b0C]", month, day)
        ax.plot(hours, ep_tz, 'b-', label='E+ living', linewidth=1.5)
        ax.plot(hours, ob_tz, 'r--', label='OB living', linewidth=1.5)
        ax.plot(hours, ob_tout, 'k:', label='Outdoor', linewidth=1, alpha=0.5)
        ax.set_title("Zone Temperature [\u00b0C]")
        ax.legend()
        ax.set_xlabel("Hour")
        ax.grid(True, alpha=0.3)

        # ── 4. Opaque Wall Conduction (sum) ──
        ax = axes[1, 0]
        ep_wall = ep_sum_surfaces_day(ep_h, ep_d, EP_WALL_COND,
            "Surface Inside Face Conduction Heat Transfer Rate", month, day)
        ob_wall = np.zeros(24)
        for sname in OB_WALL_COND:
            col = find_ob_col(ob_sd, sname, "cond_inside")
            if col:
                ob_wall += ob_extract_day(ob_sd, col, month, day)
        ax.plot(hours, ep_wall, 'b-', label='E+', linewidth=1.5)
        ax.plot(hours, ob_wall, 'r--', label='OB', linewidth=1.5)
        ax.set_title("Opaque Walls Conduction [W]")
        ax.legend()
        ax.set_xlabel("Hour")
        ax.grid(True, alpha=0.3)

        # ── 5. Window Inside Face Convection (sum) ──
        ax = axes[1, 1]
        ep_wconv = np.zeros(24)
        for sname in EP_WINDOWS_SOLAR:  # reuse window name list
            idx = ep_col_idx(ep_h, f"{sname}:Surface Inside Face Convection Heat Gain Rate")
            if idx is not None:
                ep_wconv += ep_extract_day(ep_h, ep_d, idx, month, day)
        ob_wconv = np.zeros(24)
        for sname in OB_WINDOWS_SOLAR:
            col = find_ob_col(ob_sd, sname, "convection_inside")
            if col:
                ob_wconv += ob_extract_day(ob_sd, col, month, day)
        ax.plot(hours, -ep_wconv, 'b-', label='E+', linewidth=1.5)
        ax.plot(hours, ob_wconv, 'r--', label='OB', linewidth=1.5)
        ax.set_title("Window Inside Convection [W]")
        ax.legend()
        ax.set_xlabel("Hour")
        ax.grid(True, alpha=0.3)

        # ── 6. Floor Conduction (to basement) ──
        ax = axes[1, 2]
        ep_floor = ep_sum_surfaces_day(ep_h, ep_d, EP_FLOOR_COND,
            "Surface Inside Face Conduction Heat Transfer Rate", month, day)
        ob_floor = np.zeros(24)
        for sname in OB_FLOOR_COND:
            col = find_ob_col(ob_sd, sname, "cond_inside")
            if col:
                ob_floor += ob_extract_day(ob_sd, col, month, day)
        ax.plot(hours, ep_floor, 'b-', label='E+', linewidth=1.5)
        ax.plot(hours, ob_floor, 'r--', label='OB', linewidth=1.5)
        ax.set_title("Floor Conduction (to basement) [W]")
        ax.legend()
        ax.set_xlabel("Hour")
        ax.grid(True, alpha=0.3)

        # ── 7. Ceiling Conduction (to attic) ──
        ax = axes[2, 0]
        ep_ceil = ep_sum_surfaces_day(ep_h, ep_d, EP_CEILING_COND,
            "Surface Inside Face Conduction Heat Transfer Rate", month, day)
        ob_ceil = np.zeros(24)
        col = find_ob_col(ob_sd, "Ceiling", "cond_inside")
        if col:
            ob_ceil = ob_extract_day(ob_sd, col, month, day)
        ax.plot(hours, ep_ceil, 'b-', label='E+', linewidth=1.5)
        ax.plot(hours, ob_ceil, 'r--', label='OB', linewidth=1.5)
        ax.set_title("Ceiling Conduction (to attic) [W]")
        ax.legend()
        ax.set_xlabel("Hour")
        ax.grid(True, alpha=0.3)

        # ── 8. Internal Mass Conduction ──
        ax = axes[2, 1]
        ep_im = ep_sum_surfaces_day(ep_h, ep_d, EP_INTMASS_COND,
            "Surface Inside Face Conduction Heat Transfer Rate", month, day)
        ob_im = np.zeros(24)
        # Try to find internal mass column
        for k in ob_sd.keys():
            if "IntMass" in k and "cond_inside" in k:
                ob_im = ob_extract_day(ob_sd, k, month, day)
                break
        ax.plot(hours, ep_im, 'b-', label='E+', linewidth=1.5)
        ax.plot(hours, ob_im, 'r--', label='OB', linewidth=1.5)
        ax.set_title("Internal Mass Conduction [W]")
        ax.legend()
        ax.set_xlabel("Hour")
        ax.grid(True, alpha=0.3)

        # ── 9. Interzone Wall Conduction (to garage) ──
        ax = axes[2, 2]
        ep_gz = ep_sum_surfaces_day(ep_h, ep_d, EP_GARAGE_IZ_COND,
            "Surface Inside Face Conduction Heat Transfer Rate", month, day)
        ob_gz = np.zeros(24)
        for sname in OB_GARAGE_IZ_COND:
            col = find_ob_col(ob_sd, sname, "cond_inside")
            if col:
                ob_gz += ob_extract_day(ob_sd, col, month, day)
        ax.plot(hours, ep_gz, 'b-', label='E+', linewidth=1.5)
        ax.plot(hours, ob_gz, 'r--', label='OB', linewidth=1.5)
        ax.set_title("Interzone Walls (garage) Conduction [W]")
        ax.legend()
        ax.set_xlabel("Hour")
        ax.grid(True, alpha=0.3)

        # ── 10. Infiltration Sensible Gain ──
        ax = axes[3, 0]
        # E+ reports as separate loss and gain energies [J] per timestep
        # Convert to rate: (gain - loss) / dt
        ep_iloss = ep_extract_day(ep_h, ep_d,
            ep_col_idx(ep_h, "LIVING_UNIT1:Zone Infiltration Sensible Heat Loss Energy"), month, day)
        ep_igain = ep_extract_day(ep_h, ep_d,
            ep_col_idx(ep_h, "LIVING_UNIT1:Zone Infiltration Sensible Heat Gain Energy"), month, day)
        # These are energy in J averaged over the hour's timesteps, convert to W
        ep_infil = (ep_igain - ep_iloss) / DT
        ob_infil = ob_extract_day(ob_zd, "living_unit1:gain_infiltration_sensible [W]", month, day)
        ax.plot(hours, ep_infil, 'b-', label='E+', linewidth=1.5)
        ax.plot(hours, ob_infil, 'r--', label='OB', linewidth=1.5)
        ax.set_title("Infiltration Sensible [W]")
        ax.legend()
        ax.set_xlabel("Hour")
        ax.grid(True, alpha=0.3)

        # ── 11. Solar Gain (total transmitted through windows) ──
        ax = axes[3, 1]
        ep_solar = ep_extract_day(ep_h, ep_d,
            ep_col_idx(ep_h, "LIVING_UNIT1:Enclosure Windows Total Transmitted Solar"), month, day)
        ob_solar = ob_extract_day(ob_zd, "living_unit1:gain_solar [W]", month, day)
        ax.plot(hours, ep_solar, 'b-', label='E+', linewidth=1.5)
        ax.plot(hours, ob_solar, 'r--', label='OB', linewidth=1.5)
        ax.set_title("Total Transmitted Solar [W]")
        ax.legend()
        ax.set_xlabel("Hour")
        ax.grid(True, alpha=0.3)

        # ── 12. Inter-floor slab + unconditioned zone temps ──
        ax = axes[3, 2]
        # Unconditioned zone temperatures
        ep_tattic = ep_extract_day(ep_h, ep_d,
            ep_col_idx(ep_h, "ATTIC_UNIT1:Zone Mean Air Temperature"), month, day)
        ep_tbsmt = ep_extract_day(ep_h, ep_d,
            ep_col_idx(ep_h, "UNHEATEDBSMT_UNIT1:Zone Mean Air Temperature"), month, day)
        ep_tgarage = ep_extract_day(ep_h, ep_d,
            ep_col_idx(ep_h, "GARAGE1:Zone Mean Air Temperature"), month, day)
        ob_tattic = ob_extract_day(ob_zd, "attic:temperature [\u00b0C]", month, day)
        ob_tbsmt = ob_extract_day(ob_zd, "unheatedbsmt:temperature [\u00b0C]", month, day)
        ob_tgarage = ob_extract_day(ob_zd, "garage:temperature [\u00b0C]", month, day)
        ax.plot(hours, ep_tattic, 'b-', label='E+ attic', linewidth=1.2)
        ax.plot(hours, ob_tattic, 'b--', label='OB attic', linewidth=1.2)
        ax.plot(hours, ep_tbsmt, 'g-', label='E+ bsmt', linewidth=1.2)
        ax.plot(hours, ob_tbsmt, 'g--', label='OB bsmt', linewidth=1.2)
        ax.plot(hours, ep_tgarage, 'm-', label='E+ garage', linewidth=1.2)
        ax.plot(hours, ob_tgarage, 'm--', label='OB garage', linewidth=1.2)
        ax.set_title("Unconditioned Zone Temps [\u00b0C]")
        ax.legend(fontsize=8, ncol=2)
        ax.set_xlabel("Hour")
        ax.grid(True, alpha=0.3)

        plt.tight_layout(rect=[0, 0, 1, 0.96])
        fname = f"comparison_{month:02d}_{day:02d}.png"
        plt.savefig(os.path.join(OUT_DIR, fname), dpi=150)
        plt.close()
        print(f"  Saved {fname}")

    print(f"\nAll plots saved to {OUT_DIR}/")


if __name__ == "__main__":
    main()
