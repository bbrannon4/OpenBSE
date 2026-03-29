"""
Comparison charts: EnergyPlus vs OpenBSE heat gains for PERIMETER_BOT_ZN_1 (south perimeter, bottom floor).

Data sources:
  - EnergyPlus: prototype_tests/large_office/eplus_run_ideal/eplusout.eso
  - OpenBSE:    prototype_tests/large_office/LargeOffice_Boulder_ideal_results.csv

Chart panels:
  1. Internal gains (people + lights + equip)
  2. Window solar gain (transmitted solar only)
  3. Window conduction (frame + glass thermal conduction)
  4. Net window (solar + conduction combined)
  5. Infiltration heat transfer
  6. Opaque wall conduction (E+ south+east walls; OB opaque_conduction)
"""

import os
import sys
import numpy as np
import pandas as pd
import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt

BASE = os.path.dirname(os.path.abspath(__file__))
ESO_PATH = os.path.join(BASE, "eplus_run_ideal", "eplusout.eso")
CSV_PATH = os.path.join(BASE, "LargeOffice_Boulder_ideal_results.csv")
OUT_SUMMER = os.path.join(BASE, "gains_comparison_summer.png")
OUT_WINTER = os.path.join(BASE, "gains_comparison_winter.png")

# Summer: July 19 = sim_day 200;  Winter: January 10 = sim_day 10
SUMMER_MONTH, SUMMER_DAY_OF_MONTH = 7, 19
WINTER_MONTH, WINTER_DAY_OF_MONTH = 1, 10


# ---------------------------------------------------------------------------
# 1.  Parse ESO
# ---------------------------------------------------------------------------
def parse_eso(path):
    """
    Returns:
      var_map: dict  id (int) -> {'key': str, 'var': str}
      df:      DataFrame with one row per hourly timestep,
               columns = {variable ids} plus 'sim_day', 'month', 'day_of_month', 'hour'
    """
    var_map = {}
    records = []

    with open(path) as f:
        # --- header section ---
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
            varname_full = parts[3].strip()
            if "!" in varname_full:
                varname_full = varname_full[:varname_full.index("!")].strip()
            if "[" in varname_full:
                varname_full = varname_full[:varname_full.index("[")].strip()
            var_map[vid] = {"key": key, "var": varname_full}

        # --- data section ---
        current_time = {}
        row_buf = {}

        for raw in f:
            line = raw.strip()
            if not line:
                continue
            parts = line.split(",")
            try:
                rid = int(parts[0])
            except ValueError:
                continue

            if rid == 1:
                pass  # environment title line
            elif rid == 2:
                # time record: sim_day, month, day_of_month, DST, hour, startmin, endmin, daytype
                if len(parts) >= 8:
                    if row_buf and current_time:
                        row_buf.update(current_time)
                        records.append(row_buf)
                    row_buf = {}
                    current_time = {
                        "sim_day":      int(parts[1]),
                        "month":        int(parts[2]),
                        "day_of_month": int(parts[3]),
                        "hour":         int(parts[5]),
                    }
            elif rid in var_map:
                try:
                    val = float(parts[1])
                except (IndexError, ValueError):
                    val = 0.0
                row_buf[rid] = val

        # flush last record
        if row_buf and current_time:
            row_buf.update(current_time)
            records.append(row_buf)

    df = pd.DataFrame(records)
    return var_map, df


print("Parsing ESO file...")
var_map, eso_df = parse_eso(ESO_PATH)
print(f"  ESO: {len(var_map)} variables, {len(eso_df)} hourly rows")
if not eso_df.empty:
    sim_days = sorted(eso_df["sim_day"].unique())
    months = sorted(eso_df["month"].unique())
    print(f"  sim_day range: {sim_days[0]}–{sim_days[-1]},  months: {months}")

# Show which solar-related variables are in the ESO
print("\n  Solar-related variables in ESO (sample):")
for vid, info in list(var_map.items())[:999]:
    if "SOLAR" in info["var"].upper() or "TRANSMITTED" in info["var"].upper():
        if "PERIMETER" in info["key"].upper() or "ENCLOSURE" in info["key"].upper():
            print(f"    id={vid}  key={info['key']!r}  var={info['var']!r}")


# ---------------------------------------------------------------------------
# 2.  E+ variable lookup helpers
# ---------------------------------------------------------------------------
def find_id(var_map, key_upper, var_substr):
    key_upper = key_upper.upper()
    var_substr = var_substr.upper()
    for vid, info in var_map.items():
        if info["key"].upper() == key_upper and var_substr in info["var"].upper():
            return vid
    return None


def find_ids(var_map, key_upper, var_substr):
    """Return all matching IDs (may have multiple surfaces)."""
    key_upper = key_upper.upper()
    var_substr = var_substr.upper()
    matches = []
    for vid, info in var_map.items():
        if info["key"].upper() == key_upper and var_substr in info["var"].upper():
            matches.append(vid)
    return matches


def get_eplus_series(eso_df, var_map, key, var_substr, month, dom):
    """Extract 24-hour series (one value per hour) for a given month/day."""
    vid = find_id(var_map, key, var_substr)
    if vid is None:
        print(f"  [MISSING E+] key='{key}' var='{var_substr}'")
        return None
    sub = eso_df[(eso_df["month"] == month) & (eso_df["day_of_month"] == dom)].copy()
    sub = sub.sort_values("hour")
    if vid not in sub.columns:
        print(f"  [MISSING E+ DATA col] id={vid} key='{key}'")
        return None
    vals = sub[vid].values
    if len(vals) != 24:
        print(f"  [WARN] E+ {key}/{var_substr}: got {len(vals)} rows for {month}/{dom}")
    return vals


# ---------------------------------------------------------------------------
# 3.  Parse OpenBSE CSV
# ---------------------------------------------------------------------------
print("\nParsing OpenBSE CSV...")
ob_df = pd.read_csv(CSV_PATH, low_memory=False)
# Force numeric for all data columns
for col in ob_df.columns:
    if col not in ("Month", "Day", "Hour", "SubHour"):
        ob_df[col] = pd.to_numeric(ob_df[col], errors="coerce")
print(f"  OpenBSE: {len(ob_df)} rows, {len(ob_df.columns)} columns")

# Print Perimeter_bot_ZN_1 columns
pzn_cols = [c for c in ob_df.columns if "Perimeter_bot_ZN_1:" in c]
print(f"  Perimeter_bot_ZN_1 columns ({len(pzn_cols)}):")
for c in pzn_cols:
    print(f"    {c}")


def ob_get_day(ob_df, month, dom):
    """Return the sub-DataFrame for a given month/day (144 rows at 6 timesteps/hour)."""
    return ob_df[(ob_df["Month"] == month) & (ob_df["Day"] == dom)].copy()


def ob_hourly_avg(day_df, col_name):
    """Average sub-hourly timesteps per hour → 24-element array."""
    if col_name not in day_df.columns:
        print(f"  [MISSING OB] '{col_name}'")
        return None
    n = len(day_df)
    tph = n // 24  # timesteps per hour
    if n % 24 != 0:
        print(f"  [WARN] OB col '{col_name}': {n} rows not divisible by 24")
        return None
    vals = pd.to_numeric(day_df[col_name], errors="coerce").values
    return vals.reshape(24, tph).mean(axis=1)


def ob_hourly_sum_J(day_df, col_name):
    """Sum sub-hourly energy (J) values per hour → 24-element array in J."""
    if col_name not in day_df.columns:
        print(f"  [MISSING OB] '{col_name}'")
        return None
    n = len(day_df)
    tph = n // 24
    if n % 24 != 0:
        return None
    vals = pd.to_numeric(day_df[col_name], errors="coerce").values
    return vals.reshape(24, tph).sum(axis=1)


def safe_add(*arrays):
    valid = [a for a in arrays if a is not None and np.any(np.isfinite(a))]
    if not valid:
        return None
    result = np.zeros_like(valid[0], dtype=float)
    for a in valid:
        result += np.where(np.isfinite(a), a, 0.0).astype(float)
    return result


# ---------------------------------------------------------------------------
# 4.  E+ surface ID discovery for Perimeter_bot_ZN_1 windows
# ---------------------------------------------------------------------------
window_solar_ids = []
for vid, info in var_map.items():
    k = info["key"].upper()
    v = info["var"].upper()
    if "PERIMETER_BOT_ZN_1" in k and "WINDOW" in k and "TRANSMITTED SOLAR" in v:
        window_solar_ids.append(vid)
        print(f"  Found window solar surface: {info['key']} | {info['var']}")

enclosure_solar_ids = find_ids(var_map, "PERIMETER_BOT_ZN_1", "Enclosure Windows Total Transmitted Solar")
print(f"  Enclosure solar IDs: {enclosure_solar_ids}")

oat_id = find_id(var_map, "Environment", "Site Outdoor Air Drybulb Temperature")
zone_temp_id = find_id(var_map, "PERIMETER_BOT_ZN_1", "Zone Mean Air Temperature")
print(f"  OAT variable id: {oat_id},  zone temp id: {zone_temp_id}")

win_heat_gain_id = find_id(var_map, "PERIMETER_BOT_ZN_1", "Zone Windows Total Heat Gain Rate")
win_heat_loss_id = find_id(var_map, "PERIMETER_BOT_ZN_1", "Zone Windows Total Heat Loss Rate")
print(f"  Zone Win Gain id: {win_heat_gain_id},  Loss id: {win_heat_loss_id}")


# ---------------------------------------------------------------------------
# 5.  Build E+ day dict
# ---------------------------------------------------------------------------
def build_eplus_day(month, dom):
    d = {}

    def gs(key, var):
        return get_eplus_series(eso_df, var_map, key, var, month, dom)

    zone = "PERIMETER_BOT_ZN_1"
    sub = eso_df[(eso_df["month"] == month) & (eso_df["day_of_month"] == dom)].sort_values("hour")

    d["people"]       = gs(zone, "Zone People Sensible Heating Rate")
    d["lights"]       = gs(zone, "Zone Lights Total Heating Rate")
    d["equip"]        = gs(zone, "Zone Electric Equipment Total Heating Rate")
    d["win_gain"]     = gs(zone, "Zone Windows Total Heat Gain Rate")
    d["win_loss"]     = gs(zone, "Zone Windows Total Heat Loss Rate")
    d["infil_loss_J"] = gs(zone, "Zone Infiltration Sensible Heat Loss Energy")
    d["infil_gain_J"] = gs(zone, "Zone Infiltration Sensible Heat Gain Energy")
    d["wall_south"]   = gs("PERIMETER_BOT_ZN_1_WALL_SOUTH",
                           "Surface Inside Face Conduction Heat Transfer Rate")
    d["wall_east"]    = gs("PERIMETER_BOT_ZN_1_WALL_EAST",
                           "Surface Inside Face Conduction Heat Transfer Rate")
    d["floor"]        = gs("PERIMETER_BOT_ZN_1_FLOOR",
                           "Surface Inside Face Conduction Heat Transfer Rate")
    d["ceiling"]      = gs("PERIMETER_BOT_ZN_1_CEILING",
                           "Surface Inside Face Conduction Heat Transfer Rate")

    # Transmitted solar through windows (Enclosure variable if available, else sum per-surface)
    if enclosure_solar_ids:
        eid = enclosure_solar_ids[0]
        if eid in sub.columns:
            d["win_solar"] = sub[eid].values
        else:
            d["win_solar"] = None
    elif window_solar_ids:
        arr = np.zeros(24)
        for wid in window_solar_ids:
            if wid in sub.columns:
                arr += sub[wid].values
        d["win_solar"] = arr
    else:
        d["win_solar"] = None

    # OAT and zone temp
    if oat_id is not None and oat_id in sub.columns:
        d["oat"] = sub[oat_id].values
    else:
        d["oat"] = None
    if zone_temp_id is not None and zone_temp_id in sub.columns:
        d["zone_temp"] = sub[zone_temp_id].values
    else:
        d["zone_temp"] = None

    # Derived
    d["internal_gains"] = safe_add(d["people"], d["lights"], d["equip"])

    # Window conduction = Zone Windows Total Heat Gain Rate - Transmitted Solar
    # (E+ Zone Windows Total includes solar gain + thermal conduction through glass/frame)
    if d["win_gain"] is not None and d["win_solar"] is not None:
        d["win_cond"] = d["win_gain"] - d["win_solar"]
    else:
        d["win_cond"] = None

    # Net window (gain - loss)
    if d["win_gain"] is not None and d["win_loss"] is not None:
        d["net_window"] = d["win_gain"] - d["win_loss"]
    elif d["win_gain"] is not None:
        d["net_window"] = d["win_gain"]
    else:
        d["net_window"] = None

    # Infiltration (J → W via 3600 s/hr)
    if d["infil_gain_J"] is not None and d["infil_loss_J"] is not None:
        d["infiltration"] = (d["infil_gain_J"] - d["infil_loss_J"]) / 3600.0
    else:
        d["infiltration"] = None

    d["wall_cond"]    = safe_add(d["wall_south"], d["wall_east"])
    d["floor_cond"]   = d["floor"]
    d["ceiling_cond"] = d["ceiling"]

    return d


# ---------------------------------------------------------------------------
# 6.  Build OpenBSE day dict
# ---------------------------------------------------------------------------
def build_ob_day(month, dom):
    day_df = ob_get_day(ob_df, month, dom)
    if len(day_df) == 0:
        print(f"  [WARN] No OB rows for {month}/{dom}")
        return {}
    d = {}
    zone = "Perimeter_bot_ZN_1"

    def col(var):
        return f"{zone}:{var} [-]"

    d["q_internal_conv"] = ob_hourly_avg(day_df, col("q_internal_conv"))
    d["q_internal_rad"]  = ob_hourly_avg(day_df, col("q_internal_rad"))
    d["opaque_cond"]     = ob_hourly_avg(day_df, col("opaque_conduction"))
    d["solar"]           = ob_hourly_avg(day_df, col("transmitted_solar"))
    d["win_cond"]        = ob_hourly_avg(day_df, col("window_conduction"))
    d["infil_flow"]      = ob_hourly_avg(day_df, col("infiltration_mass_flow"))
    d["zone_temp"]       = ob_hourly_avg(day_df, col("zone_temp"))
    d["heating_J"]       = ob_hourly_sum_J(day_df, col("heating_load"))
    d["cooling_J"]       = ob_hourly_sum_J(day_df, col("cooling_load"))

    d["internal_gains"] = safe_add(d["q_internal_conv"], d["q_internal_rad"])

    # Flag diverged columns
    for key in ["opaque_cond"]:
        if d[key] is not None:
            n_bad = np.sum(~np.isfinite(d[key]))
            if n_bad > 0:
                print(f"  [DIVERGED] OB {key}: {n_bad}/24 non-finite values → setting to None")
                d[key] = None

    return d


# ---------------------------------------------------------------------------
# 7.  Compute OB infiltration from mass flow and temperatures
# ---------------------------------------------------------------------------
def compute_ob_infiltration(ob_d, ep_d):
    """
    infil_W = infil_mass_flow [kg/s] * 1005 [J/(kg·K)] * (T_outdoor - T_zone) [K]

    OB zone_temp is NOT used here — Perimeter_bot_ZN_1 has interzone CTF surfaces
    that cause violent sub-hourly temperature oscillation (±100°C) even though the
    zone never goes fully NaN.  The mass flow itself is clean; we just need a stable
    T_zone reference, so we always use E+ zone mean air temperature.
    """
    flow = ob_d.get("infil_flow")
    t_out = ep_d.get("oat")
    t_zone = ep_d.get("zone_temp")   # always use E+ zone temp — OB is oscillating
    if t_zone is not None:
        print("  [INFO] Using E+ zone temp for OB infiltration calc (OB zone_temp is sub-hourly unstable)")

    if flow is None:
        print("  [MISSING] OB infil_flow for infiltration calc")
        return None
    if t_out is None:
        print("  [MISSING] E+ OAT for infiltration calc")
        return None
    if t_zone is None:
        print("  [MISSING] zone temp for infiltration calc; using 22°C")
        t_zone = np.full(24, 22.0)

    return flow.astype(float) * 1005.0 * (t_out.astype(float) - t_zone.astype(float))


# ---------------------------------------------------------------------------
# 8.  Plot helper
# ---------------------------------------------------------------------------
def make_figure(season_label, ep_d, ob_d, ob_infil):
    hours = np.arange(1, 25)

    fig, axes = plt.subplots(3, 2, figsize=(14, 12))
    fig.suptitle(
        f"Heat Gain Comparison – PERIMETER_BOT_ZN_1 (South Perimeter, Bot Floor)\n"
        f"EnergyPlus vs OpenBSE  |  {season_label}",
        fontsize=13, fontweight="bold"
    )

    def plot_pair(ax, title, ep_arr, ob_arr, note=None):
        has_data = False
        if ep_arr is not None and np.any(np.isfinite(ep_arr)):
            ax.plot(hours, ep_arr, color="steelblue", linewidth=2.0, label="EnergyPlus")
            has_data = True
        else:
            ax.text(0.5, 0.60, "E+ data not available", transform=ax.transAxes,
                    ha="center", va="center", color="gray", fontsize=9)
        if ob_arr is not None and np.any(np.isfinite(ob_arr)):
            ax.plot(hours, ob_arr, color="darkorange", linewidth=2.0,
                    linestyle="--", label="OpenBSE")
            has_data = True
        else:
            ax.text(0.5, 0.40, "OB data not available / diverged", transform=ax.transAxes,
                    ha="center", va="center", color="gray", fontsize=9)
        ax.set_title(title, fontsize=10, fontweight="bold")
        ax.set_xlabel("Hour of Day", fontsize=9)
        ax.set_ylabel("W  (positive = zone heat gain)", fontsize=8)
        ax.set_xlim(1, 24)
        ax.set_xticks(range(1, 25, 2))
        ax.grid(True, alpha=0.35)
        ax.axhline(0, color="black", linewidth=0.6, linestyle="-")
        if has_data:
            ax.legend(fontsize=9)
        if note:
            ax.text(0.01, 0.02, note, transform=ax.transAxes,
                    ha="left", fontsize=7.5, color="dimgray", style="italic")

    # 1. Internal gains
    plot_pair(axes[0, 0],
              "1. Internal Gains (people + lights + equip)",
              ep_d.get("internal_gains"),
              ob_d.get("internal_gains"))

    # 2. Window solar gain (transmitted solar only)
    # E+: Enclosure Windows Total Transmitted Solar;  OB: transmitted_solar
    plot_pair(axes[0, 1],
              "2. Window Solar (transmitted through glass)",
              ep_d.get("win_solar"),
              ob_d.get("solar"),
              note="E+: Enclosure Windows Total Transmitted Solar; OB: transmitted_solar")

    # 3. Infiltration
    plot_pair(axes[1, 0],
              "3. Infiltration Heat Transfer",
              ep_d.get("infiltration"),
              ob_infil,
              note="OB: ṁ_infil × Cp × (T_out − T_zone)")

    # 4. Opaque wall conduction
    ob_opaque = ob_d.get("opaque_cond")  # may be None if diverged
    ep_opaque_all = safe_add(ep_d.get("wall_cond"), ep_d.get("floor_cond"), ep_d.get("ceiling_cond"))
    plot_pair(axes[1, 1],
              "4. Opaque Conduction (all surfaces)",
              ep_opaque_all,
              ob_opaque,
              note="E+: south+east walls + floor + ceiling; OB: opaque_conduction (may diverge)")

    # 5. Window conduction (thermal, not solar)
    # E+: Zone Windows Total Heat Gain − Transmitted Solar;  OB: window_conduction
    plot_pair(axes[2, 0],
              "5. Window Conduction (thermal, not solar)",
              ep_d.get("win_cond"),
              ob_d.get("win_cond"),
              note="E+: Win Gain Rate − Transmitted Solar; OB: window_conduction")

    # 6. Net window (solar + conduction combined)
    ob_net_win = safe_add(ob_d.get("solar"), ob_d.get("win_cond"))
    ep_net_win = ep_d.get("net_window")  # Zone Windows Total Gain - Loss
    plot_pair(axes[2, 1],
              "6. Net Window (solar + conduction − loss)",
              ep_net_win,
              ob_net_win,
              note="E+: Win Gain Rate − Win Loss Rate; OB: solar + win_cond")

    plt.tight_layout()
    return fig


# ---------------------------------------------------------------------------
# 9.  Build summer and winter day data
# ---------------------------------------------------------------------------
print(f"\n--- Building Summer Day ({SUMMER_MONTH}/{SUMMER_DAY_OF_MONTH}) ---")
ep_summer = build_eplus_day(SUMMER_MONTH, SUMMER_DAY_OF_MONTH)
ob_summer = build_ob_day(SUMMER_MONTH, SUMMER_DAY_OF_MONTH)
ob_infil_summer = compute_ob_infiltration(ob_summer, ep_summer)

print(f"\n--- Building Winter Day ({WINTER_MONTH}/{WINTER_DAY_OF_MONTH}) ---")
ep_winter = build_eplus_day(WINTER_MONTH, WINTER_DAY_OF_MONTH)
ob_winter = build_ob_day(WINTER_MONTH, WINTER_DAY_OF_MONTH)
ob_infil_winter = compute_ob_infiltration(ob_winter, ep_winter)


# ---------------------------------------------------------------------------
# 10.  Print peak values
# ---------------------------------------------------------------------------
def print_peaks(label, ep_d, ob_d, ob_infil):
    print(f"\n{'='*60}")
    print(f"Peak values — {label}")
    print(f"{'='*60}")

    def pk(arr):
        if arr is None:
            return "N/A"
        finite = arr[np.isfinite(arr)]
        if len(finite) == 0:
            return "diverged"
        return f"{np.max(np.abs(finite)):.1f} W"

    ep_opaque_all = safe_add(ep_d.get("wall_cond"), ep_d.get("floor_cond"), ep_d.get("ceiling_cond"))
    rows = [
        ("Internal Gains",       ep_d.get("internal_gains"),   ob_d.get("internal_gains")),
        ("Window Solar",         ep_d.get("win_solar"),        ob_d.get("solar")),
        ("Window Conduction",    ep_d.get("win_cond"),         ob_d.get("win_cond")),
        ("Net Window",           ep_d.get("net_window"),       safe_add(ob_d.get("solar"), ob_d.get("win_cond"))),
        ("Infiltration",         ep_d.get("infiltration"),     ob_infil),
        ("Opaque Cond (all)",    ep_opaque_all,                ob_d.get("opaque_cond")),
    ]
    for name, ep_arr, ob_arr in rows:
        print(f"  {name:<44}  E+={pk(ep_arr):>14}  OB={pk(ob_arr):>14}")


print_peaks("Summer Day (Jul 19)", ep_summer, ob_summer, ob_infil_summer)
print_peaks("Winter Day (Jan 10)", ep_winter, ob_winter, ob_infil_winter)


# ---------------------------------------------------------------------------
# 11.  Save figures
# ---------------------------------------------------------------------------
print("\nGenerating plots...")
fig_summer = make_figure("Summer: July 19", ep_summer, ob_summer, ob_infil_summer)
fig_summer.savefig(OUT_SUMMER, dpi=150, bbox_inches="tight")
print(f"  Saved: {OUT_SUMMER}")

fig_winter = make_figure("Winter: January 10", ep_winter, ob_winter, ob_infil_winter)
fig_winter.savefig(OUT_WINTER, dpi=150, bbox_inches="tight")
print(f"  Saved: {OUT_WINTER}")

plt.close("all")
print("\nDone.")
