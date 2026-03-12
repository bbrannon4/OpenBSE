#!/usr/bin/env python3
"""
Compare zone heat balance components between E+ and OpenBSE
for G SW Apartment on reference winter (Jan 23) and summer (Aug 25) days.

Compares: zone temps, HVAC, internal gains, per-surface conduction,
per-surface inside temps, solar gains, infiltration, window conduction.
"""
import csv
import sys

# ── E+ output ──────────────────────────────────────────────────────
EP_CSV = "output_apartment_heatbal/eplusout.csv"
# ── OpenBSE output ─────────────────────────────────────────────────
OB_ZONE_CSV = "ApartmentMidRise_Boulder_zone_results.csv"
OB_SURF_CSV = "ApartmentMidRise_Boulder_surface_results.csv"

# ── Column indices (0-based) for E+ eplusout.csv ────────────────
EP_COLS = {
    "outdoor_temp":     1,
    "people":           2,
    "lights":          29,
    "equipment":       56,
    # Per-surface conduction [W] (E+ sign: positive = heat INTO the surface from inside air)
    "cond_south_wall":     84,
    "cond_west_wall":      88,
    "cond_north_iwall":    92,
    "cond_east_iwall":     96,
    "cond_ground_floor":  100,
    "cond_ceiling":       104,
    # Surface inside temperatures [C]
    "tsi_south_wall":      81,
    "tsi_west_wall":       85,
    "tsi_north_iwall":     89,
    "tsi_east_iwall":      93,
    "tsi_ground_floor":    97,
    "tsi_ceiling":        101,
    "tsi_south_window":   109,
    "tsi_west_window":    112,
    # Zone-level
    "zone_temp":         1014,
    "infil_loss_j":      1041,
    "infil_gain_j":      1042,
    "infil_flow":        1043,
    "hvac_heating":      1122,
    "hvac_cooling":      1123,
}

# ── Surface name mapping (E+ key → OB surface name) ────────────
SURFACE_MAP = [
    # (ep_cond_key, ep_tsi_key, ob_surface_name, label)
    ("cond_south_wall",    "tsi_south_wall",    "G SW Apt South Wall",    "South Wall (ext)"),
    ("cond_west_wall",     "tsi_west_wall",     "G SW Apt West Wall",     "West Wall (ext)"),
    ("cond_north_iwall",   "tsi_north_iwall",   "G SW Apt North Wall",    "North Wall (corr)"),
    ("cond_east_iwall",    "tsi_east_iwall",    "G SW Apt East Wall",     "East Wall (adiab)"),
    ("cond_ground_floor",  "tsi_ground_floor",  "G SW Apt Floor",         "Ground Floor"),
    ("cond_ceiling",       "tsi_ceiling",       "G SW Apt Ceiling",       "Ceiling (adiab)"),
]

WINDOW_MAP = [
    ("tsi_south_window", "G SW Apt South Window", "South Window"),
    ("tsi_west_window",  "G SW Apt West Window",  "West Window"),
]


def read_ep_csv(path, target_days):
    """Read E+ CSV and extract target day data."""
    data = {d: [] for d in target_days}
    with open(path) as f:
        reader = csv.reader(f)
        header = next(reader)
        for row in reader:
            if not row or not row[0].strip():
                continue
            dt = row[0].strip()
            try:
                parts = dt.split()
                date_part = parts[0]
                time_part = parts[1] if len(parts) > 1 else "00:00:00"
                month, day = date_part.split("/")
                month, day = int(month), int(day)
                hour = int(time_part.split(":")[0])
                if hour == 24:
                    hour = 24
            except:
                continue
            key = (month, day)
            if key in target_days:
                record = {"month": month, "day": day, "hour": hour}
                for name, col_idx in EP_COLS.items():
                    try:
                        record[name] = float(row[col_idx])
                    except (IndexError, ValueError):
                        record[name] = None
                data[key].append(record)
    return data


def read_ob_csv(path, target_days, name_filter="G SW Apt"):
    """Read OpenBSE CSV and extract target day data for matching columns."""
    data = {d: [] for d in target_days}
    with open(path) as f:
        reader = csv.reader(f)
        header = next(reader)
        col_map = {}
        for i, h in enumerate(header):
            h = h.strip()
            if name_filter in h or h in ("Month", "Day", "Hour", "site_outdoor_temperature"):
                col_map[h] = i

        for row in reader:
            if not row:
                continue
            try:
                month = int(float(row[col_map.get("Month", 0)]))
                day = int(float(row[col_map.get("Day", 1)]))
                hour = int(float(row[col_map.get("Hour", 2)]))
            except:
                continue
            key = (month, day)
            if key in target_days:
                record = {"month": month, "day": day, "hour": hour}
                for name, idx in col_map.items():
                    try:
                        record[name] = float(row[idx])
                    except:
                        record[name] = row[idx]
                data[key].append(record)
    return data


def aggregate_hourly(records):
    """Average sub-hourly records to hourly."""
    hourly = {}
    for r in records:
        h = r["hour"]
        if h not in hourly:
            hourly[h] = []
        hourly[h].append(r)
    result = []
    for h in sorted(hourly.keys()):
        recs = hourly[h]
        avg = {"hour": h}
        for key in recs[0]:
            if key in ("month", "day", "hour", "Month", "Day", "Hour"):
                continue
            vals = [r[key] for r in recs if isinstance(r.get(key), (int, float))]
            if vals:
                avg[key] = sum(vals) / len(vals)
        result.append(avg)
    return result


def get_ob_val(ob_r, partial_name):
    """Get value from OB record by partial column name match."""
    if ob_r is None:
        return 0.0
    for k, v in ob_r.items():
        if partial_name in str(k):
            return v if isinstance(v, (int, float)) else 0.0
    return 0.0


def main():
    winter_day = (1, 23)
    summer_day = (8, 25)
    target_days = {winter_day: [], summer_day: []}

    print("Reading E+ output...")
    ep_data = read_ep_csv(EP_CSV, target_days)

    print("Reading OpenBSE zone results...")
    ob_zone_data = read_ob_csv(OB_ZONE_CSV, target_days, "G SW Apt")

    print("Reading OpenBSE surface results...")
    ob_surf_data = read_ob_csv(OB_SURF_CSV, target_days, "G SW Apt")
    # Also read corridor wall facing G SW Apt
    ob_corr_data = read_ob_csv(OB_SURF_CSV, target_days, "G Corridor South Wall (G SW Apt)")

    for day_key, day_name in [(winter_day, "WINTER (Jan 23)"), (summer_day, "SUMMER (Aug 25)")]:
        ep_day = ep_data[day_key]
        ob_zone_day = aggregate_hourly(ob_zone_data[day_key])
        ob_surf_day = aggregate_hourly(ob_surf_data[day_key])
        # Merge corridor data into surf data
        ob_corr_day = aggregate_hourly(ob_corr_data[day_key])
        for i, r in enumerate(ob_surf_day):
            if i < len(ob_corr_day):
                for k, v in ob_corr_day[i].items():
                    if k not in r:
                        r[k] = v

        if not ep_day or not ob_zone_day:
            print(f"\n  *** Missing data for {day_name} ***")
            continue

        print(f"\n{'='*110}")
        print(f"  ZONE HEAT BALANCE COMPARISON: G SW Apartment — {day_name}")
        print(f"{'='*110}")

        # ── Hourly comparison table ────────────────────────────────
        print(f"\n{'Hour':>4}  {'T_out':>6}  {'EP T_z':>6}  {'OB T_z':>6}  "
              f"{'EP Heat':>8}  {'OB Heat':>8}  {'EP Cool':>8}  {'OB Cool':>8}")
        print("-" * 80)

        for ep_r in ep_day:
            h = ep_r["hour"]
            ob_r = next((r for r in ob_zone_day if r["hour"] == h), None)

            ep_tout = ep_r.get("outdoor_temp", 0)
            ep_tz = ep_r.get("zone_temp", 0)
            ep_heat = ep_r.get("hvac_heating", 0) or 0
            ep_cool = ep_r.get("hvac_cooling", 0) or 0
            ob_tz = get_ob_val(ob_r, "zone_temperature")
            ob_heat = get_ob_val(ob_r, "heating_rate")
            ob_cool = get_ob_val(ob_r, "cooling_rate")

            print(f"{h:4d}  {ep_tout:6.1f}  {ep_tz:6.2f}  {ob_tz:6.2f}  "
                  f"{ep_heat:8.1f}  {ob_heat:8.1f}  {ep_cool:8.1f}  {ob_cool:8.1f}")

        # ── Internal gains comparison ──────────────────────────────
        print(f"\n--- Internal Gains (daily average W) ---")
        ep_people_avg = sum(r.get("people", 0) or 0 for r in ep_day) / max(len(ep_day), 1)
        ep_lights_avg = sum(r.get("lights", 0) or 0 for r in ep_day) / max(len(ep_day), 1)
        ep_equip_avg = sum(r.get("equipment", 0) or 0 for r in ep_day) / max(len(ep_day), 1)
        ep_total_gains = ep_people_avg + ep_lights_avg + ep_equip_avg
        print(f"  E+ People: {ep_people_avg:.1f} W   Lights: {ep_lights_avg:.1f} W   Equip: {ep_equip_avg:.1f} W   TOTAL: {ep_total_gains:.1f} W")

        # ── Per-surface conduction comparison ──────────────────────
        print(f"\n--- Per-Surface Comparison (daily average, positive = heat from zone INTO surface) ---")
        print(f"  {'Surface':<22}  {'EP Cond[W]':>10}  {'OB Cond[W]':>10}  {'EP Tsi[C]':>9}  {'OB Tsi[C]':>9}  {'Ratio':>6}")
        print(f"  {'-'*22}  {'-'*10}  {'-'*10}  {'-'*9}  {'-'*9}  {'-'*6}")

        ep_cond_total = 0
        ob_cond_total = 0

        for ep_cond_key, ep_tsi_key, ob_surf_name, label in SURFACE_MAP:
            # E+ averages
            ep_cond_vals = [r.get(ep_cond_key, 0) or 0 for r in ep_day]
            ep_cond_avg = sum(ep_cond_vals) / max(len(ep_cond_vals), 1)
            ep_tsi_vals = [r.get(ep_tsi_key) for r in ep_day if r.get(ep_tsi_key) is not None]
            ep_tsi_avg = sum(ep_tsi_vals) / max(len(ep_tsi_vals), 1) if ep_tsi_vals else 0

            # OB averages
            # Look for surface_conduction_inside and surface_inside_temperature
            ob_cond_vals = []
            ob_tsi_vals = []
            for r in ob_surf_day:
                # conduction: "G SW Apt South Wall:surface_conduction_inside [W]"
                cond_key = [k for k in r if ob_surf_name in k and "conduction_inside" in k]
                tsi_key = [k for k in r if ob_surf_name in k and "inside_temperature" in k]
                if cond_key:
                    v = r[cond_key[0]]
                    if isinstance(v, (int, float)):
                        ob_cond_vals.append(v)
                if tsi_key:
                    v = r[tsi_key[0]]
                    if isinstance(v, (int, float)):
                        ob_tsi_vals.append(v)

            ob_cond_avg = sum(ob_cond_vals) / max(len(ob_cond_vals), 1) if ob_cond_vals else 0
            ob_tsi_avg = sum(ob_tsi_vals) / max(len(ob_tsi_vals), 1) if ob_tsi_vals else 0

            ep_cond_total += ep_cond_avg
            ob_cond_total += ob_cond_avg

            ratio_str = f"{ob_cond_avg/ep_cond_avg:.2f}" if abs(ep_cond_avg) > 0.1 else "n/a"
            print(f"  {label:<22}  {ep_cond_avg:10.1f}  {ob_cond_avg:10.1f}  {ep_tsi_avg:9.2f}  {ob_tsi_avg:9.2f}  {ratio_str:>6}")

        # Windows (inside temp and solar only, no CTF conduction)
        for ep_tsi_key, ob_surf_name, label in WINDOW_MAP:
            ep_tsi_vals = [r.get(ep_tsi_key) for r in ep_day if r.get(ep_tsi_key) is not None]
            ep_tsi_avg = sum(ep_tsi_vals) / max(len(ep_tsi_vals), 1) if ep_tsi_vals else 0

            ob_tsi_vals = []
            ob_solar_vals = []
            for r in ob_surf_day:
                tsi_key = [k for k in r if ob_surf_name in k and "inside_temperature" in k]
                sol_key = [k for k in r if ob_surf_name in k and "transmitted_solar" in k]
                if tsi_key:
                    v = r[tsi_key[0]]
                    if isinstance(v, (int, float)):
                        ob_tsi_vals.append(v)
                if sol_key:
                    v = r[sol_key[0]]
                    if isinstance(v, (int, float)):
                        ob_solar_vals.append(v)

            ob_tsi_avg = sum(ob_tsi_vals) / max(len(ob_tsi_vals), 1) if ob_tsi_vals else 0
            ob_solar_avg = sum(ob_solar_vals) / max(len(ob_solar_vals), 1) if ob_solar_vals else 0

            print(f"  {label:<22}  {'---':>10}  {'---':>10}  {ep_tsi_avg:9.2f}  {ob_tsi_avg:9.2f}  {'':>6}  solar_trans={ob_solar_avg:.1f}W")

        print(f"  {'TOTAL CONDUCTION':<22}  {ep_cond_total:10.1f}  {ob_cond_total:10.1f}  {'':>9}  {'':>9}  "
              f"{ob_cond_total/ep_cond_total:.2f}" if abs(ep_cond_total) > 0.1 else "n/a")

        # ── Solar gains ────────────────────────────────────────────
        print(f"\n--- Window Solar Transmission (daily average W) ---")
        total_ob_solar = 0
        for ep_tsi_key, ob_surf_name, label in WINDOW_MAP:
            ob_solar_vals = []
            for r in ob_surf_day:
                sol_key = [k for k in r if ob_surf_name in k and "transmitted_solar" in k]
                if sol_key:
                    v = r[sol_key[0]]
                    if isinstance(v, (int, float)):
                        ob_solar_vals.append(v)
            avg_solar = sum(ob_solar_vals) / max(len(ob_solar_vals), 1) if ob_solar_vals else 0
            total_ob_solar += avg_solar
            print(f"  OB {label}: {avg_solar:.1f} W")
        print(f"  OB Total window solar: {total_ob_solar:.1f} W")

        # ── Infiltration comparison ──────────────────────────────
        print(f"\n--- Infiltration ---")
        ep_infil_loss_avg = sum((r.get("infil_loss_j", 0) or 0) for r in ep_day) / max(len(ep_day), 1)
        ep_infil_gain_avg = sum((r.get("infil_gain_j", 0) or 0) for r in ep_day) / max(len(ep_day), 1)
        ep_infil_flow_avg = sum((r.get("infil_flow", 0) or 0) for r in ep_day) / max(len(ep_day), 1)
        ep_infil_net_w = (ep_infil_gain_avg - ep_infil_loss_avg) / 3600.0

        ob_infil_vals = []
        for r in ob_zone_day:
            ik = [k for k in r if "infiltration" in str(k).lower()]
            if ik:
                v = r[ik[0]]
                if isinstance(v, (int, float)):
                    ob_infil_vals.append(v)
        ob_infil_avg = sum(ob_infil_vals) / max(len(ob_infil_vals), 1) if ob_infil_vals else 0

        print(f"  E+ avg flow: {ep_infil_flow_avg:.6f} m3/s    net load: {ep_infil_net_w:.1f} W")
        print(f"  OB avg flow: {ob_infil_avg:.6f} kg/s  (÷ρ ≈ {ob_infil_avg/1.0:.6f} m3/s at altitude)")

        # ── Daily energy totals ──────────────────────────────────
        print(f"\n--- Daily Energy Totals [Wh] ---")
        ep_heat_total = sum((r.get("hvac_heating", 0) or 0) for r in ep_day)
        ep_cool_total = sum((r.get("hvac_cooling", 0) or 0) for r in ep_day)

        ob_heat_total = sum(get_ob_val(r, "heating_rate") for r in ob_zone_day)
        ob_cool_total = sum(get_ob_val(r, "cooling_rate") for r in ob_zone_day)

        print(f"  E+  heating: {ep_heat_total:8.0f} Wh    cooling: {ep_cool_total:8.0f} Wh")
        print(f"  OB  heating: {ob_heat_total:8.0f} Wh    cooling: {ob_cool_total:8.0f} Wh")
        if ep_heat_total > 0:
            print(f"  Heating ratio (OB/E+): {ob_heat_total/ep_heat_total:.2f}x")
        if ep_cool_total > 0:
            print(f"  Cooling ratio (OB/E+): {ob_cool_total/ep_cool_total:.2f}x")

        # ── Hourly per-surface conduction breakdown ─────────────
        print(f"\n--- Hourly Per-Surface Conduction [W] (EP / OB) ---")
        print(f"  {'Hour':>4}  {'S.Wall EP/OB':>14}  {'W.Wall EP/OB':>14}  {'N.Wall EP/OB':>14}  {'Floor EP/OB':>14}  {'Ceil EP/OB':>14}")
        print(f"  {'-'*4}  {'-'*14}  {'-'*14}  {'-'*14}  {'-'*14}  {'-'*14}")

        for ep_r in ep_day:
            h = ep_r["hour"]
            ob_r = next((r for r in ob_surf_day if r["hour"] == h), None)

            def get_ob_surf_cond(surf_name):
                if ob_r is None:
                    return 0.0
                ck = [k for k in ob_r if surf_name in k and "conduction_inside" in k]
                return ob_r[ck[0]] if ck and isinstance(ob_r[ck[0]], (int, float)) else 0.0

            ep_sw = ep_r.get("cond_south_wall", 0) or 0
            ep_ww = ep_r.get("cond_west_wall", 0) or 0
            ep_nw = ep_r.get("cond_north_iwall", 0) or 0
            ep_fl = ep_r.get("cond_ground_floor", 0) or 0
            ep_cl = ep_r.get("cond_ceiling", 0) or 0
            ob_sw = get_ob_surf_cond("G SW Apt South Wall")
            ob_ww = get_ob_surf_cond("G SW Apt West Wall")
            ob_nw = get_ob_surf_cond("G SW Apt North Wall")
            ob_fl = get_ob_surf_cond("G SW Apt Floor")
            ob_cl = get_ob_surf_cond("G SW Apt Ceiling")

            print(f"  {h:4d}  {ep_sw:6.0f}/{ob_sw:6.0f}  {ep_ww:6.0f}/{ob_ww:6.0f}  "
                  f"{ep_nw:6.0f}/{ob_nw:6.0f}  {ep_fl:6.0f}/{ob_fl:6.0f}  {ep_cl:6.0f}/{ob_cl:6.0f}")


if __name__ == "__main__":
    main()
