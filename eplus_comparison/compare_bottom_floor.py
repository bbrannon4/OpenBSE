#!/usr/bin/env python3
"""
Compare hourly zone temperatures and heating/cooling rates between
EnergyPlus and OpenBSE for bottom-floor zones in the Large Office model.

E+ data:  output_office_simplified/eplusout.csv
OpenBSE:  LargeOffice_Boulder_zone_results.csv

Bottom-floor zones analysed:
    Core_bottom, Perimeter_bot_ZN_1..4, Basement
"""

import pandas as pd
import numpy as np
import os

# ---------------------------------------------------------------------------
# Paths
# ---------------------------------------------------------------------------
BASE = os.path.dirname(os.path.abspath(__file__))
EP_CSV = os.path.join(BASE, "output_office_simplified", "eplusout.csv")
OB_CSV = os.path.join(BASE, "LargeOffice_Boulder_zone_results.csv")

# ---------------------------------------------------------------------------
# Zone name mapping  (OpenBSE name -> E+ prefix)
# ---------------------------------------------------------------------------
ZONES = [
    "Core_bottom",
    "Perimeter_bot_ZN_1",
    "Perimeter_bot_ZN_2",
    "Perimeter_bot_ZN_3",
    "Perimeter_bot_ZN_4",
    "Basement",
]

# E+ column names are UPPER CASE, OpenBSE uses mixed case
def ep_zone(z):
    return z.upper()

# ---------------------------------------------------------------------------
# Load E+ data
# ---------------------------------------------------------------------------
print("=" * 80)
print("Loading EnergyPlus data ...")
ep = pd.read_csv(EP_CSV)

# Parse E+ Date/Time  " MM/DD  HH:00:00" -> month, day, hour
def parse_ep_datetime(s):
    s = s.strip()
    date_part, time_part = s.split()
    mm, dd = date_part.split("/")
    hh = int(time_part.split(":")[0])
    return int(mm), int(dd), hh

ep[["month", "day", "hour"]] = ep["Date/Time"].apply(
    lambda x: pd.Series(parse_ep_datetime(x))
)

# ---------------------------------------------------------------------------
# Load OpenBSE data
# ---------------------------------------------------------------------------
print("Loading OpenBSE data ...")
ob = pd.read_csv(OB_CSV)
ob.rename(columns={"Month": "month", "Day": "day", "Hour": "hour"}, inplace=True)

# ---------------------------------------------------------------------------
# Merge on (month, day, hour)
# ---------------------------------------------------------------------------
merged = pd.merge(ep, ob, on=["month", "day", "hour"], how="inner", suffixes=("_ep", "_ob"))
print(f"Merged rows: {len(merged)}  (expect ~8760)")

# ---------------------------------------------------------------------------
# Build convenient column lookup helpers
# ---------------------------------------------------------------------------
def ep_heat_col(zone):
    """E+ zone sensible heating rate column."""
    return f"{ep_zone(zone)}:Zone Air System Sensible Heating Rate [W](Hourly)"

def ep_cool_col(zone):
    """E+ zone sensible cooling rate column."""
    return f"{ep_zone(zone)}:Zone Air System Sensible Cooling Rate [W](Hourly)"

def ob_temp_col(zone):
    return f"{zone}:zone_temperature [\u00b0C]"

def ob_heat_col(zone):
    return f"{zone}:zone_heating_rate [W]"

def ob_cool_col(zone):
    return f"{zone}:zone_cooling_rate [W]"

# Verify columns exist
print("\nVerifying columns ...")
missing = []
for z in ZONES:
    for col in [ep_heat_col(z), ep_cool_col(z)]:
        if col not in merged.columns:
            missing.append(("E+", col))
    for col in [ob_temp_col(z), ob_heat_col(z), ob_cool_col(z)]:
        if col not in merged.columns:
            missing.append(("OpenBSE", col))
if missing:
    print("  WARNING -- missing columns:")
    for src, c in missing:
        print(f"    [{src}] {c}")
else:
    print("  All columns found.")

# Check if E+ has zone temperature columns (it may not)
ep_has_temp = {}
for z in ZONES:
    candidate = f"{ep_zone(z)}:Zone Mean Air Temperature [C](Hourly)"
    ep_has_temp[z] = candidate if candidate in merged.columns else None

any_ep_temp = any(v is not None for v in ep_has_temp.values())
if not any_ep_temp:
    print("\n  NOTE: E+ output does not contain Zone Mean Air Temperature columns.")
    print("        Temperature comparison will show OpenBSE temps only.")
    print("        Heating / cooling rate comparison is available for both engines.")

# ---------------------------------------------------------------------------
# Helper: monthly stats
# ---------------------------------------------------------------------------
def monthly_stats(df, month_num, month_name):
    """Print detailed stats for one month."""
    m = df[df["month"] == month_num].copy()
    n_hours = len(m)

    print(f"\n{'=' * 80}")
    print(f"  {month_name}   ({n_hours} hours)")
    print("=" * 80)

    # Occupied = hours 7-22 (inclusive, 1-indexed hour of day)
    occ = m[(m["hour"] >= 7) & (m["hour"] <= 22)]
    unocc = m[(m["hour"] < 7) | (m["hour"] > 22)]

    for z in ZONES:
        print(f"\n  --- {z} ---")

        # Temperatures (OpenBSE always; E+ if available)
        t_ob = m[ob_temp_col(z)]
        t_ob_occ = occ[ob_temp_col(z)]
        t_ob_unocc = unocc[ob_temp_col(z)]

        if ep_has_temp[z]:
            t_ep = m[ep_has_temp[z]]
            t_ep_occ = occ[ep_has_temp[z]]
            t_ep_unocc = unocc[ep_has_temp[z]]
            print(f"    Avg zone temp        : E+ {t_ep.mean():7.2f} C   |  OpenBSE {t_ob.mean():7.2f} C   |  diff {t_ob.mean()-t_ep.mean():+.2f} C")
            print(f"    Avg temp (occ h7-22) : E+ {t_ep_occ.mean():7.2f} C   |  OpenBSE {t_ob_occ.mean():7.2f} C   |  diff {t_ob_occ.mean()-t_ep_occ.mean():+.2f} C")
            print(f"    Avg temp (unocc)     : E+ {t_ep_unocc.mean():7.2f} C   |  OpenBSE {t_ob_unocc.mean():7.2f} C   |  diff {t_ob_unocc.mean()-t_ep_unocc.mean():+.2f} C")
        else:
            print(f"    Avg zone temp (OpenBSE)       : {t_ob.mean():7.2f} C")
            print(f"    Avg temp occ h7-22 (OpenBSE)  : {t_ob_occ.mean():7.2f} C")
            print(f"    Avg temp unocc (OpenBSE)      : {t_ob_unocc.mean():7.2f} C")
            print(f"    Min / Max temp (OpenBSE)      : {t_ob.min():7.2f} / {t_ob.max():.2f} C")

        # Heating rates  (W -> Wh for hourly data, then kWh)
        h_ep = m[ep_heat_col(z)].sum() / 1000.0   # kWh
        h_ob = m[ob_heat_col(z)].sum() / 1000.0
        print(f"    Total heating        : E+ {h_ep:12.1f} kWh  |  OpenBSE {h_ob:12.1f} kWh  |  diff {h_ob - h_ep:+.1f} kWh")

        # Cooling rates
        c_ep = m[ep_cool_col(z)].sum() / 1000.0
        c_ob = m[ob_cool_col(z)].sum() / 1000.0
        print(f"    Total cooling        : E+ {c_ep:12.1f} kWh  |  OpenBSE {c_ob:12.1f} kWh  |  diff {c_ob - c_ep:+.1f} kWh")


# ---------------------------------------------------------------------------
# Run stats for January and July
# ---------------------------------------------------------------------------
monthly_stats(merged, 1, "JANUARY")
monthly_stats(merged, 7, "JULY")

# ---------------------------------------------------------------------------
# Sample hours: Jan 15 hours 1-24 for Core_bottom and Perimeter_bot_ZN_3
# ---------------------------------------------------------------------------
SAMPLE_ZONES = ["Core_bottom", "Perimeter_bot_ZN_3"]
for sample_month, sample_day, label in [(1, 15, "Jan 15"), (7, 15, "Jul 15")]:
    day_df = merged[(merged["month"] == sample_month) & (merged["day"] == sample_day)].sort_values("hour")

    if day_df.empty:
        print(f"\n  No data for {label}")
        continue

    print(f"\n{'=' * 80}")
    print(f"  SAMPLE HOURLY DATA -- {label}")
    print("=" * 80)

    # Outdoor temp
    ep_oat_col = "Environment:Site Outdoor Air Drybulb Temperature [C](Hourly)"
    ob_oat_col = "site_outdoor_temperature [\u00b0C]"
    has_ep_oat = ep_oat_col in day_df.columns
    has_ob_oat = ob_oat_col in day_df.columns

    for z in SAMPLE_ZONES:
        print(f"\n  --- {z} ---")
        header = f"  {'Hr':>3s}"
        if has_ep_oat:
            header += f"  {'OAT_EP':>8s}"
        if has_ob_oat:
            header += f"  {'OAT_OB':>8s}"

        if ep_has_temp[z]:
            header += f"  {'T_EP':>8s}"
        header += f"  {'T_OB':>8s}"
        header += f"  {'Htg_EP':>10s}  {'Htg_OB':>10s}  {'Clg_EP':>10s}  {'Clg_OB':>10s}"
        print(header)
        print("  " + "-" * (len(header) - 2))

        for _, row in day_df.iterrows():
            hr = int(row["hour"])
            line = f"  {hr:3d}"

            if has_ep_oat:
                line += f"  {row[ep_oat_col]:8.2f}"
            if has_ob_oat:
                line += f"  {row[ob_oat_col]:8.2f}"

            if ep_has_temp[z]:
                line += f"  {row[ep_has_temp[z]]:8.2f}"
            line += f"  {row[ob_temp_col(z)]:8.2f}"

            h_ep = row[ep_heat_col(z)]
            h_ob = row[ob_heat_col(z)]
            c_ep = row[ep_cool_col(z)]
            c_ob = row[ob_cool_col(z)]
            line += f"  {h_ep:10.1f}  {h_ob:10.1f}  {c_ep:10.1f}  {c_ob:10.1f}"
            print(line)

# ---------------------------------------------------------------------------
# Summary: aggregate heating/cooling across all bottom-floor zones
# ---------------------------------------------------------------------------
print(f"\n{'=' * 80}")
print("  ANNUAL SUMMARY -- Bottom-floor zones aggregate")
print("=" * 80)

for z in ZONES:
    h_ep = merged[ep_heat_col(z)].sum() / 1e6  # MWh
    h_ob = merged[ob_heat_col(z)].sum() / 1e6
    c_ep = merged[ep_cool_col(z)].sum() / 1e6
    c_ob = merged[ob_cool_col(z)].sum() / 1e6
    pct_h = ((h_ob - h_ep) / h_ep * 100) if h_ep != 0 else float("nan")
    pct_c = ((c_ob - c_ep) / c_ep * 100) if c_ep != 0 else float("nan")
    print(f"  {z:25s}  Htg: E+ {h_ep:8.2f}  OB {h_ob:8.2f} MWh ({pct_h:+6.1f}%)   "
          f"Clg: E+ {c_ep:8.2f}  OB {c_ob:8.2f} MWh ({pct_c:+6.1f}%)")

tot_h_ep = sum(merged[ep_heat_col(z)].sum() for z in ZONES) / 1e6
tot_h_ob = sum(merged[ob_heat_col(z)].sum() for z in ZONES) / 1e6
tot_c_ep = sum(merged[ep_cool_col(z)].sum() for z in ZONES) / 1e6
tot_c_ob = sum(merged[ob_cool_col(z)].sum() for z in ZONES) / 1e6
print(f"  {'TOTAL':25s}  Htg: E+ {tot_h_ep:8.2f}  OB {tot_h_ob:8.2f} MWh ({(tot_h_ob-tot_h_ep)/tot_h_ep*100 if tot_h_ep else float('nan'):+6.1f}%)   "
      f"Clg: E+ {tot_c_ep:8.2f}  OB {tot_c_ob:8.2f} MWh ({(tot_c_ob-tot_c_ep)/tot_c_ep*100 if tot_c_ep else float('nan'):+6.1f}%)")

# ---------------------------------------------------------------------------
# OpenBSE temperature summary (since E+ temps are not available)
# ---------------------------------------------------------------------------
print(f"\n{'=' * 80}")
print("  ANNUAL TEMPERATURE SUMMARY (OpenBSE only -- E+ temps not in this run)")
print("=" * 80)
print(f"  {'Zone':25s}  {'Avg':>7s}  {'Min':>7s}  {'Max':>7s}  {'Occ Avg':>8s}  {'Unocc Avg':>10s}")
print(f"  {'-'*25}  {'-'*7}  {'-'*7}  {'-'*7}  {'-'*8}  {'-'*10}")

occ_mask = (merged["hour"] >= 7) & (merged["hour"] <= 22)
for z in ZONES:
    tc = ob_temp_col(z)
    avg = merged[tc].mean()
    mn = merged[tc].min()
    mx = merged[tc].max()
    occ_avg = merged.loc[occ_mask, tc].mean()
    unocc_avg = merged.loc[~occ_mask, tc].mean()
    print(f"  {z:25s}  {avg:7.2f}  {mn:7.2f}  {mx:7.2f}  {occ_avg:8.2f}  {unocc_avg:10.2f}")

print("\nDone.")
