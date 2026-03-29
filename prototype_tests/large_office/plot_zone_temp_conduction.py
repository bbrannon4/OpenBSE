"""
Diagnostic: zone temperatures and per-surface conduction for PERIMETER_BOT_ZN_1.

Panels:
  1. Zone temp: E+ vs OB (Perimeter_bot_ZN_1)
  2. Zone temp: E+ vs OB (Core_bottom, adjacent interzone)
  3. OAT for reference
  4. South Wall conduction (outdoor-facing)
  5. Floor conduction (to DataCenter_basement below)
  6. Ceiling conduction (to Perimeter_mid_ZN_1_f2 above)
  7. North IW conduction (to Core_bottom)
  8. Interior Mass conduction
  9. Total opaque conduction: E+ sum vs OB aggregate

Summer: Jul 19  |  Winter: Jan 10
"""

import os
import sys
import numpy as np
import pandas as pd
import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt

BASE = os.path.dirname(os.path.abspath(__file__))
ESO_PATH  = os.path.join(BASE, "eplus_run_ideal", "eplusout.eso")
CSV_PATH  = os.path.join(BASE, "LargeOffice_Boulder_ideal_results.csv")
OUT_SUMMER = os.path.join(BASE, "zone_temp_cond_summer.png")
OUT_WINTER = os.path.join(BASE, "zone_temp_cond_winter.png")

SUMMER_MONTH, SUMMER_DOM = 7, 19
WINTER_MONTH, WINTER_DOM = 1, 10


# ── ESO parser ────────────────────────────────────────────────────────────────
def parse_eso(path):
    var_map, records = {}, []
    with open(path) as f:
        for raw in f:
            line = raw.strip()
            if line == "End of Data Dictionary":
                break
            parts = line.split(",", 3)
            if len(parts) < 4:
                continue
            try:
                vid = int(parts[0])
            except ValueError:
                continue
            key = parts[2].strip()
            vn  = parts[3].strip()
            if "!" in vn: vn = vn[:vn.index("!")].strip()
            if "[" in vn: vn = vn[:vn.index("[")].strip()
            var_map[vid] = {"key": key, "var": vn}

        current_time, row_buf = {}, {}
        for raw in f:
            line = raw.strip()
            if not line:
                continue
            parts = line.split(",")
            try:
                rid = int(parts[0])
            except ValueError:
                continue
            if rid == 2 and len(parts) >= 8:
                if row_buf and current_time:
                    row_buf.update(current_time)
                    records.append(row_buf)
                row_buf = {}
                current_time = {
                    "month":        int(parts[2]),
                    "day_of_month": int(parts[3]),
                    "hour":         int(parts[5]),
                }
            elif rid in var_map:
                try:
                    row_buf[rid] = float(parts[1])
                except (IndexError, ValueError):
                    row_buf[rid] = 0.0
        if row_buf and current_time:
            row_buf.update(current_time)
            records.append(row_buf)
    return var_map, pd.DataFrame(records)


def find_id(vm, key_upper, var_sub):
    ku, vs = key_upper.upper(), var_sub.upper()
    for vid, info in vm.items():
        if info["key"].upper() == ku and vs in info["var"].upper():
            return vid
    return None


def ep_day_series(eso_df, month, dom, vid):
    if vid is None:
        return None
    sub = eso_df[(eso_df["month"] == month) & (eso_df["day_of_month"] == dom)].sort_values("hour")
    if vid not in sub.columns:
        return None
    return sub[vid].values  # 24-element


# ── OpenBSE helpers ───────────────────────────────────────────────────────────
def ob_day(ob_df, month, dom):
    return ob_df[(ob_df["Month"] == month) & (ob_df["Day"] == dom)].copy()


def hourly_avg(day_df, col):
    if col not in day_df.columns:
        return None
    n   = len(day_df)
    tph = n // 24
    if tph == 0 or n % 24 != 0:
        return None
    vals = pd.to_numeric(day_df[col], errors="coerce").values
    return vals.reshape(24, tph).mean(axis=1)


# ── Load data ─────────────────────────────────────────────────────────────────
print("Parsing ESO...")
var_map, eso_df = parse_eso(ESO_PATH)
print(f"  {len(var_map)} vars, {len(eso_df)} hourly rows")

print("Parsing OpenBSE CSV (large — please wait)...")
ob_df = pd.read_csv(CSV_PATH, low_memory=False)
for c in ob_df.columns:
    if c not in ("Month", "Day", "Hour", "SubHour"):
        ob_df[c] = pd.to_numeric(ob_df[c], errors="coerce")
print(f"  {len(ob_df)} rows, {len(ob_df.columns)} cols")


# ── E+ variable IDs ───────────────────────────────────────────────────────────
ZONE_EP   = "PERIMETER_BOT_ZN_1"
ZONE_CORE = "CORE_BOTTOM"

ep_ids = {
    "pzn1_temp":    find_id(var_map, ZONE_EP,   "Zone Mean Air Temperature"),
    "core_temp":    find_id(var_map, ZONE_CORE,  "Zone Mean Air Temperature"),
    "oat":          find_id(var_map, "Environment", "Site Outdoor Air Drybulb Temperature"),
    # Surface conduction: positive = heat entering zone
    "s_wall_south": find_id(var_map, "PERIMETER_BOT_ZN_1_WALL_SOUTH",
                            "Surface Inside Face Conduction Heat Transfer Rate"),
    "s_floor":      find_id(var_map, "PERIMETER_BOT_ZN_1_FLOOR",
                            "Surface Inside Face Conduction Heat Transfer Rate"),
    "s_ceiling":    find_id(var_map, "PERIMETER_BOT_ZN_1_CEILING",
                            "Surface Inside Face Conduction Heat Transfer Rate"),
    # North interior wall — E+ uses auto-created interzone name
    "s_north_iw":   find_id(var_map, "PERIMETER_BOT_ZN_1_WALL_NORTH-PPAUTOCREATEOTHER",
                            "Surface Inside Face Conduction Heat Transfer Rate"),
    "s_intmass":    find_id(var_map, "PERIMETER_BOT_ZN_1_INTERNALMASS_1",
                            "Surface Inside Face Conduction Heat Transfer Rate"),
    # zone load for total opaque cross-check
    "htg_rate":     find_id(var_map, ZONE_EP, "Zone Ideal Loads Heat Transfer Rate"),
    "clg_rate":     find_id(var_map, ZONE_EP, "Zone Ideal Loads Cool Transfer Rate"),
}

print("\nE+ variable IDs:")
for k, v in ep_ids.items():
    print(f"  {k:20s} id={v}")

# also find east wall (P_bot_ZN_1 is south-facing; east wall borders ZN_2)
ep_ids["s_east_iw"] = find_id(var_map, "PERIMETER_BOT_ZN_1_WALL_EAST-PPAUTOCREATEOTHER",
                               "Surface Inside Face Conduction Heat Transfer Rate")
ep_ids["s_west_iw"] = find_id(var_map, "PERIMETER_BOT_ZN_1_WALL_WEST-PPAUTOCREATEOTHER",
                               "Surface Inside Face Conduction Heat Transfer Rate")
print(f"  {'s_east_iw':20s} id={ep_ids['s_east_iw']}")
print(f"  {'s_west_iw':20s} id={ep_ids['s_west_iw']}")


def ep_s(month, dom, key):
    return ep_day_series(eso_df, month, dom, ep_ids.get(key))


# ── Build day dicts ───────────────────────────────────────────────────────────
def safe_add(*arrays):
    valid = [a for a in arrays if a is not None and np.any(np.isfinite(a))]
    if not valid:
        return None
    out = np.zeros(24, dtype=float)
    for a in valid:
        out += np.where(np.isfinite(a), a, 0.0)
    return out


def build_day(month, dom):
    ob  = ob_day(ob_df, month, dom)
    ep  = {}
    obv = {}

    for k in ep_ids:
        ep[k] = ep_s(month, dom, k)

    # OB zone temps
    obv["pzn1_temp"] = hourly_avg(ob, "Perimeter_bot_ZN_1:zone_temp [-]")
    obv["core_temp"] = hourly_avg(ob, "Core_bottom:zone_temp [-]")

    # OB per-surface conduction (W, positive = heat entering zone from inside face)
    obv["s_south_wall"] = hourly_avg(ob, "Surf:P_bot_ZN1 South Wall:cond_inside [-]")
    obv["s_floor"]      = hourly_avg(ob, "Surf:P_bot_ZN1 Floor:cond_inside [-]")
    obv["s_ceiling"]    = hourly_avg(ob, "Surf:P_bot_ZN1 Ceiling:cond_inside [-]")
    obv["s_north_iw"]   = hourly_avg(ob, "Surf:P_bot_ZN1 North IW:cond_inside [-]")
    obv["s_intmass"]    = hourly_avg(ob, "Surf:Perimeter_bot_ZN_1 IntMass 1:cond_inside [-]")
    obv["opaque_total"] = hourly_avg(ob, "Perimeter_bot_ZN_1:opaque_conduction [-]")

    # OB surface temperatures for floor (most suspect)
    obv["floor_t_inside"]  = hourly_avg(ob, "Surf:P_bot_ZN1 Floor:temp_inside [-]")
    obv["floor_t_outside"] = hourly_avg(ob, "Surf:P_bot_ZN1 Floor:temp_outside [-]")

    # E+ total opaque = sum all surface conduction (excl windows)
    ep["opaque_total"] = safe_add(
        ep["s_wall_south"], ep["s_floor"], ep["s_ceiling"],
        ep["s_north_iw"],   ep["s_east_iw"], ep["s_west_iw"], ep["s_intmass"]
    )

    return ep, obv


# ── Plot ──────────────────────────────────────────────────────────────────────
HOURS = np.arange(1, 25)


def plot_pair(ax, title, ep_arr, ob_arr, ylabel="W", note=None, ep_label="EnergyPlus", ob_label="OpenBSE"):
    if ep_arr is not None and np.any(np.isfinite(ep_arr)):
        ax.plot(HOURS, ep_arr, color="steelblue", lw=2.0, label=ep_label)
    else:
        ax.text(0.5, 0.6, "E+ N/A", transform=ax.transAxes, ha="center", color="gray", fontsize=8)
    if ob_arr is not None and np.any(np.isfinite(ob_arr)):
        ax.plot(HOURS, ob_arr, color="darkorange", lw=2.0, ls="--", label=ob_label)
    else:
        ax.text(0.5, 0.4, "OB N/A", transform=ax.transAxes, ha="center", color="gray", fontsize=8)
    ax.set_title(title, fontsize=9, fontweight="bold")
    ax.set_xlabel("Hour", fontsize=8)
    ax.set_ylabel(ylabel, fontsize=8)
    ax.set_xlim(1, 24)
    ax.set_xticks(range(1, 25, 3))
    ax.grid(True, alpha=0.3)
    ax.axhline(0, color="black", lw=0.5)
    ax.legend(fontsize=8)
    if note:
        ax.text(0.01, 0.02, note, transform=ax.transAxes, fontsize=7, color="dimgray", style="italic")


def make_figure(season_label, ep, obv):
    fig, axes = plt.subplots(3, 3, figsize=(17, 13))
    fig.suptitle(
        f"Zone Temps & Per-Surface Conduction — PERIMETER_BOT_ZN_1\n"
        f"EnergyPlus vs OpenBSE  |  {season_label}",
        fontsize=12, fontweight="bold"
    )

    # Row 0: temperatures
    plot_pair(axes[0, 0],
              "1. Zone Temp — Perimeter_bot_ZN_1",
              ep.get("pzn1_temp"), obv.get("pzn1_temp"), ylabel="°C")

    plot_pair(axes[0, 1],
              "2. Zone Temp — Core_bottom (adjacent)",
              ep.get("core_temp"), obv.get("core_temp"), ylabel="°C")

    plot_pair(axes[0, 2],
              "3. Outdoor Air Temperature",
              ep.get("oat"), None, ylabel="°C", ob_label="_nolegend_")
    # OAT is same for both engines
    if ep.get("oat") is not None:
        axes[0, 2].get_lines()[0].set_label("OAT (shared)")
        axes[0, 2].legend(fontsize=8)

    # Row 1: per-surface conduction
    plot_pair(axes[1, 0],
              "4. South Outdoor Wall Conduction",
              ep.get("s_wall_south"), obv.get("s_south_wall"),
              note="Positive = heat into zone from outside")

    plot_pair(axes[1, 1],
              "5. Floor Conduction (to DataCenter_basement)",
              ep.get("s_floor"), obv.get("s_floor"),
              note="Positive = heat into zone from below")

    plot_pair(axes[1, 2],
              "6. Ceiling Conduction (to floor above)",
              ep.get("s_ceiling"), obv.get("s_ceiling"),
              note="Positive = heat into zone from above")

    # Row 2: interzone walls, intmass, totals
    plot_pair(axes[2, 0],
              "7. North IW Conduction (to Core_bottom)",
              ep.get("s_north_iw"), obv.get("s_north_iw"),
              note="Positive = heat into zone from Core_bottom side")

    plot_pair(axes[2, 1],
              "8. Interior Mass Conduction",
              ep.get("s_intmass"), obv.get("s_intmass"))

    plot_pair(axes[2, 2],
              "9. Total Opaque Conduction (all surfaces)",
              ep.get("opaque_total"), obv.get("opaque_total"),
              note="E+: sum of surfaces; OB: opaque_conduction aggregate")

    plt.tight_layout()
    return fig


# ── Run ───────────────────────────────────────────────────────────────────────
print(f"\nBuilding summer ({SUMMER_MONTH}/{SUMMER_DOM})...")
ep_s_day, ob_s_day = build_day(SUMMER_MONTH, SUMMER_DOM)
print(f"Building winter ({WINTER_MONTH}/{WINTER_DOM})...")
ep_w_day, ob_w_day = build_day(WINTER_MONTH, WINTER_DOM)

# Print key peak values
for label, ep_d, ob_d in [("Summer", ep_s_day, ob_s_day), ("Winter", ep_w_day, ob_w_day)]:
    print(f"\n{'='*55}")
    print(f"  {label} — key peaks")
    print(f"{'='*55}")
    def pk(a):
        if a is None: return "  N/A"
        f = a[np.isfinite(a)]
        return f"{np.max(np.abs(f)):.1f} W" if len(f) else "  ---"
    def pkc(a):
        if a is None: return "  N/A"
        f = a[np.isfinite(a)]
        return f"{np.max(np.abs(f)):.2f} °C" if len(f) else "  ---"
    rows = [
        ("PZN1 zone temp",   ep_d.get("pzn1_temp"),   ob_d.get("pzn1_temp"),   pkc),
        ("Core_bot zone temp",ep_d.get("core_temp"),  ob_d.get("core_temp"),   pkc),
        ("South wall cond",  ep_d.get("s_wall_south"),ob_d.get("s_south_wall"),pk),
        ("Floor cond",        ep_d.get("s_floor"),     ob_d.get("s_floor"),     pk),
        ("Ceiling cond",      ep_d.get("s_ceiling"),   ob_d.get("s_ceiling"),   pk),
        ("North IW cond",     ep_d.get("s_north_iw"),  ob_d.get("s_north_iw"),  pk),
        ("IntMass cond",      ep_d.get("s_intmass"),   ob_d.get("s_intmass"),   pk),
        ("Total opaque",      ep_d.get("opaque_total"),ob_d.get("opaque_total"),pk),
    ]
    for name, earr, oarr, fmt in rows:
        print(f"  {name:<22}  E+={fmt(earr):>12}  OB={fmt(oarr):>12}")

print("\nGenerating plots...")
fig_s = make_figure(f"Summer: July {SUMMER_DOM}", ep_s_day, ob_s_day)
fig_s.savefig(OUT_SUMMER, dpi=150, bbox_inches="tight")
print(f"  → {OUT_SUMMER}")

fig_w = make_figure(f"Winter: January {WINTER_DOM}", ep_w_day, ob_w_day)
fig_w.savefig(OUT_WINTER, dpi=150, bbox_inches="tight")
print(f"  → {OUT_WINTER}")

plt.close("all")
print("Done.")
