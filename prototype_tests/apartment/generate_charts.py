#!/usr/bin/env python3
"""
Generate diagnostic comparison charts for the Mid-Rise Apartment model.
Compares EnergyPlus (E+) results with OpenBSE results for the G SW Apt zone.

Usage:
    python3 prototype_tests/apartment/generate_charts.py
"""

import os
import numpy as np
import pandas as pd
import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt
import matplotlib.ticker as mticker

# ---------------------------------------------------------------------------
# Paths
# ---------------------------------------------------------------------------
BASE = os.path.dirname(os.path.abspath(__file__))
EPLUS_HOURLY = os.path.join(BASE, "eplus_denver_run_hourly", "eplusout.csv")
EPLUS_TBL = os.path.join(BASE, "eplus_denver_run_hourly", "eplustbl.csv")
OB_ZONE = os.path.join(BASE, "ApartmentMidRise_Boulder_zone_results.csv")
OB_SURFACE = os.path.join(BASE, "ApartmentMidRise_Boulder_surface_results.csv")
OB_SUMMARY = os.path.join(BASE, "ApartmentMidRise_Boulder_summary.txt")
OUT_DIR = BASE

CP = 1006.0  # J/(kg*K)

# ---------------------------------------------------------------------------
# Helper: parse E+ Date/Time to a datetime index
# ---------------------------------------------------------------------------
def parse_eplus_datetime(series):
    """Parse E+ date/time strings like ' 01/01  00:10:00' into a DatetimeIndex."""
    dts = []
    for raw in series:
        s = raw.strip()
        parts = s.split()
        md = parts[0]  # MM/DD
        hms = parts[1]  # HH:MM:SS
        month, day = md.split("/")
        h, m, sec = hms.split(":")
        month, day, h, m = int(month), int(day), int(h), int(m)
        # E+ uses hour 24 for midnight of next day
        if h == 24:
            h = 0
            # advance day (simple: use pandas)
            import datetime
            d = datetime.date(2017, month, day) + datetime.timedelta(days=1)
            month, day = d.month, d.day
        dts.append(pd.Timestamp(2017, month, day, h, m))
    return pd.DatetimeIndex(dts)


def load_eplus_hourly():
    """Load E+ CSV and resample sub-hourly data to hourly means."""
    df = pd.read_csv(EPLUS_HOURLY)
    # Strip column name whitespace
    df.columns = [c.strip() for c in df.columns]
    df.index = parse_eplus_datetime(df["Date/Time"])
    df.drop(columns=["Date/Time"], inplace=True)
    return df


def _build_ob_datetime_index(df):
    """Build a DatetimeIndex from Month/Day/Hour columns (hours 1-24).

    OpenBSE Hour N covers the period (N-1):00 to N:00, matching the EPW
    end-of-hour convention.  We place each row at the START of its period
    (hour N → timestamp N-1:00) so that it aligns with E+ data resampled
    to hourly means (pandas default label='left').
    """
    import datetime
    dts = []
    for _, row in df.iterrows():
        m, d, h = int(row["Month"]), int(row["Day"]), int(row["Hour"])
        # Hour 1 → 00:00, Hour 2 → 01:00, ..., Hour 24 → 23:00
        h_start = h - 1
        if h_start == 24:
            # shouldn't happen (Hour 25), but guard
            base = datetime.date(2017, m, d) + datetime.timedelta(days=1)
            dts.append(pd.Timestamp(base.year, base.month, base.day, 0, 0))
        else:
            dts.append(pd.Timestamp(2017, m, d, h_start, 0))
    return pd.DatetimeIndex(dts)


def load_ob_zone():
    """Load OpenBSE zone results."""
    df = pd.read_csv(OB_ZONE)
    df.index = _build_ob_datetime_index(df)
    return df


def load_ob_surface():
    """Load OpenBSE surface results."""
    df = pd.read_csv(OB_SURFACE)
    df.index = _build_ob_datetime_index(df)
    return df


def parse_ob_monthly_enduse():
    """Parse OpenBSE summary.txt monthly end-use table.
    Returns dict: end_use_name -> list of 12 monthly values in kWh.
    """
    with open(OB_SUMMARY, "r") as f:
        lines = f.readlines()

    result = {}
    in_table = False
    for line in lines:
        if "Monthly Energy End-Use [kWh]" in line:
            in_table = True
            continue
        if in_table:
            stripped = line.strip()
            if stripped.startswith("---") or stripped.startswith("End Use"):
                continue
            if stripped == "" or stripped.startswith("--"):
                if result:  # we already collected data, table ended
                    break
                continue
            # Parse a data line: name followed by 13 numbers (12 months + total)
            # Name may contain spaces, values are whitespace-separated numbers
            # Split from the right: last 13 tokens are numbers
            parts = stripped.split()
            # Find where numbers start - scan from end
            nums = []
            name_parts = []
            for i, p in enumerate(parts):
                try:
                    float(p)
                    nums.append(float(p))
                except ValueError:
                    if nums:
                        # We hit a non-number after numbers started - shouldn't happen
                        break
                    name_parts.append(p)
            if len(nums) >= 13:
                name = " ".join(name_parts)
                result[name] = nums[:12]  # 12 monthly values
    return result


def parse_eplus_enduse_gj():
    """Parse E+ annual end uses from eplustbl.csv.
    Returns dict: end_use_name -> (elec_GJ, gas_GJ).
    """
    with open(EPLUS_TBL, "r") as f:
        lines = f.readlines()

    result = {}
    in_enduses = False
    for line in lines:
        if "End Uses" in line and "Subcategory" not in line and "Space Type" not in line and "By" not in line:
            in_enduses = True
            continue
        if in_enduses:
            parts = line.strip().split(",")
            if len(parts) >= 4 and parts[0] == "":
                name = parts[1].strip()
                if name and name not in ("", "Total End Uses") and "Note" not in name:
                    try:
                        elec_gj = float(parts[2]) if parts[2].strip() else 0.0
                        gas_gj = float(parts[3]) if parts[3].strip() else 0.0
                        result[name] = (elec_gj, gas_gj)
                    except (ValueError, IndexError):
                        pass
            if "Total End Uses" in line:
                in_enduses = False
    return result


# ---------------------------------------------------------------------------
# Chart 1: Annual End-Use Comparison (bar chart)
# ---------------------------------------------------------------------------
def chart_annual_enduse():
    eplus_gj = parse_eplus_enduse_gj()
    # Convert GJ to kWh: 1 GJ = 277.778 kWh
    GJ_TO_KWH = 277.778

    # Map E+ names to our display names and fuel type
    mapping = [
        ("Heating (Gas)", "Heating", "gas"),
        ("Cooling (Elec)", "Cooling", "elec"),
        ("Int. Lighting", "Interior Lighting", "elec"),
        ("Ext. Lighting", "Exterior Lighting", "elec"),
        ("Int. Equipment", "Interior Equipment", "elec"),
        ("Fans", "Fans", "elec"),
        ("Pumps", "Pumps", "elec"),
        ("DHW (Gas)", "Water Systems", "gas"),
    ]

    ob_monthly = parse_ob_monthly_enduse()

    labels = []
    eplus_vals = []
    ob_vals = []

    for display, eplus_name, fuel in mapping:
        if eplus_name in eplus_gj:
            elec_gj, gas_gj = eplus_gj[eplus_name]
            if fuel == "gas":
                ep_kwh = gas_gj * GJ_TO_KWH
            else:
                ep_kwh = elec_gj * GJ_TO_KWH
        else:
            ep_kwh = 0.0

        # OB name mapping
        ob_name_map = {
            "Heating (Gas)": "Heating (Gas)",
            "Cooling (Elec)": "Cooling (Electric)",
            "Int. Lighting": "Interior Lighting",
            "Ext. Lighting": "Exterior Lighting",
            "Int. Equipment": "Interior Equipment",
            "Fans": "Fans (Electric)",
            "Pumps": "Pumps (Electric)",
            "DHW (Gas)": "DHW (Gas)",
        }
        ob_key = ob_name_map[display]
        ob_kwh = sum(ob_monthly.get(ob_key, [0]*12))

        labels.append(display)
        eplus_vals.append(ep_kwh)
        ob_vals.append(ob_kwh)

    fig, ax = plt.subplots(figsize=(12, 6))
    x = np.arange(len(labels))
    width = 0.35

    bars1 = ax.bar(x - width/2, eplus_vals, width, label="E+", color="#4C72B0", edgecolor="black", linewidth=0.5)
    bars2 = ax.bar(x + width/2, ob_vals, width, label="OpenBSE", color="#DD8452", edgecolor="black", linewidth=0.5)

    # Add % diff labels
    for i, (ep, ob) in enumerate(zip(eplus_vals, ob_vals)):
        if ep > 0:
            diff_pct = (ob - ep) / ep * 100
        else:
            diff_pct = 0
        color = "#2ca02c" if abs(diff_pct) < 5 else "#d62728"
        ax.text(x[i], max(ep, ob) * 1.02, f"{diff_pct:+.1f}%",
                ha="center", va="bottom", fontsize=9, fontweight="bold", color=color)

    ax.set_ylabel("Energy [kWh]")
    ax.set_title("Annual End-Use Comparison: E+ vs OpenBSE (Mid-Rise Apartment, Denver)")
    ax.set_xticks(x)
    ax.set_xticklabels(labels, rotation=30, ha="right")
    ax.legend()
    ax.grid(axis="y", alpha=0.3)
    ax.set_ylim(0, max(max(eplus_vals), max(ob_vals)) * 1.15)
    fig.tight_layout()
    path = os.path.join(OUT_DIR, "annual_end_use_comparison.png")
    fig.savefig(path, dpi=150)
    plt.close(fig)
    print(f"  Saved: {path}")


# ---------------------------------------------------------------------------
# Chart 2: Monthly End-Use Comparison (heating, cooling, fans)
# ---------------------------------------------------------------------------
def chart_monthly_enduse():
    ob_monthly = parse_ob_monthly_enduse()

    # Compute E+ monthly from hourly data
    # Load E+ hourly for whole-building meters would be complex.
    # Instead, compute from the E+ GJ annual data proportioned, or from the
    # summary table. Let's compute E+ monthly heating/cooling from the hourly
    # zone data by summing all zone heating/cooling rates.
    # Actually - the E+ eplustbl.csv might have monthly data. Let me check.
    # For now, compute E+ monthly from the hourly CSV by summing zone rates.

    # Actually, we have the E+ annual in GJ. For a proper monthly comparison,
    # let's use the OB monthly data and compute E+ monthly from hourly.
    # But E+ hourly only has zone-level sensible rates for G/M/T SW zones.
    # We don't have whole-building meters. So let's just use the OpenBSE monthly
    # data and the E+ annual totals split proportionally.
    # Better approach: just parse E+ monthly if available, otherwise note it.

    # For simplicity with available data: plot OB monthly with E+ annual/12 as
    # reference line, or just plot OB monthly bars.
    # Actually the user asked for E+ monthly too. Let's try computing from hourly.

    # We can't compute whole-building E+ monthly from the zone CSV since it
    # only has 3 SW zones. Let's just plot OB monthly data with E+ annual
    # shown as horizontal dashed line.

    months = ["Jan", "Feb", "Mar", "Apr", "May", "Jun",
              "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"]
    x = np.arange(12)

    eplus_gj = parse_eplus_enduse_gj()
    GJ_TO_KWH = 277.778

    fig, axes = plt.subplots(1, 3, figsize=(16, 5))

    panels = [
        ("Heating (Gas)", "Heating (Gas)", "gas"),
        ("Cooling (Electric)", "Cooling (Elec)", "elec"),
        ("Fans (Electric)", "Fans", "elec"),
    ]

    for ax, (ob_name, display_name, fuel) in zip(axes, panels):
        ob_vals = ob_monthly.get(ob_name, [0]*12)

        # E+ annual
        eplus_name_map = {
            "Heating (Gas)": "Heating",
            "Cooling (Elec)": "Cooling",
            "Fans": "Fans",
        }
        ep_name = eplus_name_map[display_name]
        if ep_name in eplus_gj:
            elec_gj, gas_gj = eplus_gj[ep_name]
            ep_annual = (gas_gj if fuel == "gas" else elec_gj) * GJ_TO_KWH
        else:
            ep_annual = 0

        ax.bar(x, ob_vals, color="#DD8452", edgecolor="black", linewidth=0.5, label="OpenBSE")
        ax.axhline(ep_annual / 12, color="#4C72B0", linestyle="--", linewidth=1.5,
                    label=f"E+ avg ({ep_annual:.0f} kWh/yr)")
        ax.set_xticks(x)
        ax.set_xticklabels(months, rotation=45, ha="right", fontsize=8)
        ax.set_ylabel("Energy [kWh]")
        ax.set_title(display_name)
        ax.legend(fontsize=8)
        ax.grid(axis="y", alpha=0.3)

    fig.suptitle("Monthly End-Use: OpenBSE vs E+ Annual Average (Mid-Rise Apartment)", fontsize=12)
    fig.tight_layout()
    path = os.path.join(OUT_DIR, "monthly_end_use_comparison.png")
    fig.savefig(path, dpi=150)
    plt.close(fig)
    print(f"  Saved: {path}")


# ---------------------------------------------------------------------------
# Chart 3: Daily Comparison (winter + summer day)
# ---------------------------------------------------------------------------
def chart_daily_comparison():
    print("  Loading E+ hourly data (this may take a moment)...")
    ep = load_eplus_hourly()
    print("  Loading OpenBSE zone data...")
    ob_z = load_ob_zone()
    print("  Loading OpenBSE surface data...")
    ob_s = load_ob_surface()

    # Resample E+ to hourly means
    ep_h = ep.resample("1h").mean()

    # E+ column names for G SW APARTMENT
    ep_solar = "G SW APARTMENT:Enclosure Windows Total Transmitted Solar Radiation Rate [W](TimeStep)"
    ep_heat_rate = "G SW APARTMENT:Zone Air System Sensible Heating Rate [W](TimeStep)"
    ep_cool_rate = "G SW APARTMENT:Zone Air System Sensible Cooling Rate [W](TimeStep)"
    ep_infil_loss = "G SW APARTMENT:Zone Infiltration Sensible Heat Loss Energy [J](TimeStep)"
    ep_infil_gain = "G SW APARTMENT:Zone Infiltration Sensible Heat Gain Energy [J](TimeStep)"
    ep_zone_temp = "G SW APARTMENT:Zone Mean Air Temperature [C](TimeStep)"

    # E+ surface conduction columns for G SW zone
    ep_surf_cols = [
        "G SWALL SWA:Surface Inside Face Conduction Heat Transfer Rate [W](TimeStep)",
        "G WWALL SWA:Surface Inside Face Conduction Heat Transfer Rate [W](TimeStep)",
        "G NIWALL SWA:Surface Inside Face Conduction Heat Transfer Rate [W](TimeStep)",
        "G EIWALL SWA:Surface Inside Face Conduction Heat Transfer Rate [W](TimeStep)",
        "G GFLOOR SWA:Surface Inside Face Conduction Heat Transfer Rate [W](TimeStep)",
        "G CEILIN SWA:Surface Inside Face Conduction Heat Transfer Rate [W](TimeStep)",
    ]

    # E+ infiltration: energy [J] at sub-hourly timestep (600s).
    # After resampling to hourly mean of the energy values,
    # we need: rate = mean_energy_per_timestep / 600 * 6 = hourly sum / 3600
    # Actually resampling mean of J values gives mean J per timestep.
    # Total hourly J = mean_J * 6 (6 timesteps/hr)
    # Rate [W] = total_J / 3600
    # = mean_J * 6 / 3600 = mean_J / 600
    # So after hourly mean resample: rate_W = mean_J / 600

    # OB column names
    ob_zone_temp_col = "G SW Apt:zone_temperature [°C]"
    ob_heat_col = "G SW Apt:zone_heating_rate [W]"
    ob_cool_col = "G SW Apt:zone_cooling_rate [W]"
    ob_infil_col = "G SW Apt:zone_infiltration_mass_flow [kg/s]"
    ob_outdoor_col = "site_outdoor_temperature [°C]"

    # OB surface conduction columns
    ob_surf_cond_cols = [
        "G SW Apt South Wall:surface_conduction_inside [W]",
        "G SW Apt West Wall:surface_conduction_inside [W]",
        "G SW Apt North Wall:surface_conduction_inside [W]",
        "G SW Apt East Wall:surface_conduction_inside [W]",
        "G SW Apt Floor:surface_conduction_inside [W]",
        "G SW Apt Ceiling:surface_conduction_inside [W]",
    ]
    # OB transmitted solar (sum of window transmitted solar for G SW zone)
    ob_solar_cols = [
        "G SW Apt South Window:surface_transmitted_solar [W]",
        "G SW Apt West Window:surface_transmitted_solar [W]",
    ]

    days = [
        ("Jan 15 (Winter)", pd.Timestamp(2017, 1, 15), pd.Timestamp(2017, 1, 16)),
        ("Jul 15 (Summer)", pd.Timestamp(2017, 7, 15), pd.Timestamp(2017, 7, 16)),
    ]

    fig, axes = plt.subplots(2, 4, figsize=(20, 9))

    for row, (label, start, end) in enumerate(days):
        ep_day = ep_h.loc[start:end - pd.Timedelta(hours=1)]
        ob_z_day = ob_z.loc[start:end - pd.Timedelta(hours=1)]
        ob_s_day = ob_s.loc[start:end - pd.Timedelta(hours=1)]
        hours = np.arange(len(ep_day))
        ob_hours = np.arange(len(ob_z_day))

        # Get outdoor temp from OB (available) or compute from E+
        if ob_outdoor_col in ob_z_day.columns:
            ob_t_out = ob_z_day[ob_outdoor_col].values
        else:
            ob_t_out = np.zeros(len(ob_z_day))

        # --- Col 1: Transmitted Solar ---
        ax = axes[row, 0]
        ep_solar_kw = ep_day[ep_solar].values / 1000.0
        ob_solar_kw = np.zeros(len(ob_s_day))
        for col in ob_solar_cols:
            if col in ob_s_day.columns:
                ob_solar_kw += ob_s_day[col].values
        ob_solar_kw /= 1000.0

        ax.plot(hours, ep_solar_kw, "b-", linewidth=1.2, label="E+")
        ax.plot(ob_hours, ob_solar_kw, "r--", linewidth=1.2, label="OpenBSE")
        ax.set_ylabel("Transmitted Solar [kW]")
        ax.set_title(f"{label}\nTransmitted Solar")
        ax.legend(fontsize=7)
        ax.grid(alpha=0.3)
        ax.set_xlabel("Hour")
        # Outdoor temp on right axis
        ax2 = ax.twinx()
        ax2.plot(ob_hours, ob_t_out, ":", color="gray", linewidth=0.8, alpha=0.7)
        ax2.set_ylabel("Outdoor T [°C]", color="gray", fontsize=8)
        ax2.tick_params(axis="y", labelcolor="gray", labelsize=7)

        # --- Col 2: Surface Conduction ---
        ax = axes[row, 1]
        ep_cond_kw = np.zeros(len(ep_day))
        for col in ep_surf_cols:
            if col in ep_day.columns:
                ep_cond_kw += ep_day[col].values
        ep_cond_kw /= 1000.0

        ob_cond_kw = np.zeros(len(ob_s_day))
        for col in ob_surf_cond_cols:
            if col in ob_s_day.columns:
                ob_cond_kw += ob_s_day[col].values
        ob_cond_kw /= 1000.0

        ax.plot(hours, ep_cond_kw, "b-", linewidth=1.2, label="E+")
        ax.plot(ob_hours, ob_cond_kw, "r--", linewidth=1.2, label="OpenBSE")
        ax.set_ylabel("Surface Conduction [kW]")
        ax.set_title(f"{label}\nTotal Surface Conduction")
        ax.legend(fontsize=7)
        ax.grid(alpha=0.3)
        ax.set_xlabel("Hour")
        ax2 = ax.twinx()
        ax2.plot(ob_hours, ob_t_out, ":", color="gray", linewidth=0.8, alpha=0.7)
        ax2.set_ylabel("Outdoor T [°C]", color="gray", fontsize=8)
        ax2.tick_params(axis="y", labelcolor="gray", labelsize=7)

        # --- Col 3: Infiltration Heat Flow ---
        ax = axes[row, 2]
        # E+: infiltration energy [J] resampled to hourly mean
        # rate_W = -(loss - gain) / 600  for sub-hourly;
        # after hourly mean resample: the mean of the J values * 6 / 3600 = mean_J / 600
        ep_infil_loss_w = ep_day[ep_infil_loss].values / 600.0
        ep_infil_gain_w = ep_day[ep_infil_gain].values / 600.0
        ep_infil_kw = -(ep_infil_loss_w - ep_infil_gain_w) / 1000.0

        # OB: Q = -m_dot * cp * (T_zone - T_out) / 1000
        ob_m_dot = ob_z_day[ob_infil_col].values
        ob_t_zone = ob_z_day[ob_zone_temp_col].values
        ob_infil_kw = -ob_m_dot * CP * (ob_t_zone - ob_t_out) / 1000.0

        ax.plot(hours, ep_infil_kw, "b-", linewidth=1.2, label="E+")
        ax.plot(ob_hours, ob_infil_kw, "r--", linewidth=1.2, label="OpenBSE")
        ax.axhline(0, color="black", linewidth=0.5)
        ax.set_ylabel("Infiltration Heat Flow [kW]")
        ax.set_title(f"{label}\nInfiltration (neg=loss)")
        ax.legend(fontsize=7)
        ax.grid(alpha=0.3)
        ax.set_xlabel("Hour")
        ax2 = ax.twinx()
        ax2.plot(ob_hours, ob_t_out, ":", color="gray", linewidth=0.8, alpha=0.7)
        ax2.set_ylabel("Outdoor T [°C]", color="gray", fontsize=8)
        ax2.tick_params(axis="y", labelcolor="gray", labelsize=7)

        # --- Col 4: Heating / Cooling Load ---
        ax = axes[row, 3]
        ep_heat_kw = ep_day[ep_heat_rate].values / 1000.0
        ep_cool_kw = -ep_day[ep_cool_rate].values / 1000.0
        ep_load_kw = ep_heat_kw + ep_cool_kw

        ob_heat_kw = ob_z_day[ob_heat_col].values / 1000.0
        ob_cool_kw = -ob_z_day[ob_cool_col].values / 1000.0
        ob_load_kw = ob_heat_kw + ob_cool_kw

        ax.plot(hours, ep_load_kw, "b-", linewidth=1.2, label="E+")
        ax.plot(ob_hours, ob_load_kw, "r--", linewidth=1.2, label="OpenBSE")
        ax.axhline(0, color="black", linewidth=0.5)
        ax.set_ylabel("Load [kW] (heat+, cool-)")
        ax.set_title(f"{label}\nHeating/Cooling Load")
        ax.legend(fontsize=7)
        ax.grid(alpha=0.3)
        ax.set_xlabel("Hour")
        ax2 = ax.twinx()
        ax2.plot(ob_hours, ob_t_out, ":", color="gray", linewidth=0.8, alpha=0.7)
        ax2.set_ylabel("Outdoor T [°C]", color="gray", fontsize=8)
        ax2.tick_params(axis="y", labelcolor="gray", labelsize=7)

    fig.suptitle("Daily Comparison: G SW Apt Zone — E+ (blue) vs OpenBSE (red dashed)", fontsize=13)
    fig.tight_layout(rect=[0, 0, 1, 0.95])
    path = os.path.join(OUT_DIR, "daily_comparison.png")
    fig.savefig(path, dpi=150)
    plt.close(fig)
    print(f"  Saved: {path}")


# ---------------------------------------------------------------------------
# Chart 4: Supply Air Monthly (OpenBSE only — E+ CSV lacks supply air data)
# ---------------------------------------------------------------------------
def chart_supply_air_monthly():
    ob_z = load_ob_zone()
    ep = load_eplus_hourly()

    ob_zone_temp_col = "G SW Apt:zone_temperature [°C]"
    ob_supply_temp_col = "G SW Apt:zone_supply_air_temperature [°C]"
    ob_supply_flow_col = "G SW Apt:zone_supply_air_mass_flow [kg/s]"

    # E+ supply node columns
    ep_supply_temp_col = None
    ep_supply_flow_col = None
    ep_zone_temp_col = None
    for c in ep.columns:
        if "SPLIT GSW SUPPLY INLET" in c and "Temperature" in c:
            ep_supply_temp_col = c
        if "SPLIT GSW SUPPLY INLET" in c and "Mass Flow" in c:
            ep_supply_flow_col = c
        if "G SW APARTMENT" in c and "Mean Air Temperature" in c:
            ep_zone_temp_col = c

    has_ep_supply = ep_supply_temp_col is not None and ep_supply_flow_col is not None

    fig, axes = plt.subplots(12, 3, figsize=(16, 36))

    months = ["Jan", "Feb", "Mar", "Apr", "May", "Jun",
              "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"]

    for month_idx in range(12):
        month_num = month_idx + 1
        day = 15

        start = pd.Timestamp(2017, month_num, day)
        end = start + pd.Timedelta(hours=23)
        day_ob = ob_z.loc[start:end]

        if len(day_ob) == 0:
            for c in range(3):
                axes[month_idx, c].set_visible(False)
            continue

        hours_ob = np.arange(len(day_ob))
        t_zone_ob = day_ob[ob_zone_temp_col].values
        t_supply_ob = day_ob[ob_supply_temp_col].values
        m_dot_ob = day_ob[ob_supply_flow_col].values
        delivered_ob = m_dot_ob * CP * (t_supply_ob - t_zone_ob) / 1000.0

        # E+ data — resample sub-hourly to hourly
        if has_ep_supply:
            day_ep = ep.loc[start:end]
            if len(day_ep) > 0:
                day_ep_hr = day_ep.resample("1h").mean()
                hours_ep = np.arange(len(day_ep_hr))
                t_supply_ep = day_ep_hr[ep_supply_temp_col].values
                m_dot_ep = day_ep_hr[ep_supply_flow_col].values
                t_zone_ep = day_ep_hr[ep_zone_temp_col].values if ep_zone_temp_col else t_zone_ob
                delivered_ep = m_dot_ep * CP * (t_supply_ep - t_zone_ep) / 1000.0
            else:
                has_ep_day = False
        else:
            has_ep_day = False

        has_ep_day = has_ep_supply and len(day_ep) > 0

        # Col 1: Supply air temp + zone temp
        ax = axes[month_idx, 0]
        ax.plot(hours_ob, t_supply_ob, "r-", linewidth=1.2, label="OB Supply")
        ax.plot(hours_ob, t_zone_ob, "r:", linewidth=0.8, label="OB Zone")
        if has_ep_day:
            ax.plot(hours_ep, t_supply_ep, "b-", linewidth=1.2, label="E+ Supply")
            ax.plot(hours_ep, t_zone_ep, "b:", linewidth=0.8, label="E+ Zone")
        ax.set_ylabel("Temp [°C]", fontsize=8)
        ax.set_title(f"{months[month_idx]} 15 — Supply & Zone Temp", fontsize=9)
        ax.legend(fontsize=6, loc="best")
        ax.grid(alpha=0.3)
        if month_idx == 11:
            ax.set_xlabel("Hour")

        # Col 2: Supply air mass flow
        ax = axes[month_idx, 1]
        ax.plot(hours_ob, m_dot_ob, "r-", linewidth=1.2, label="OpenBSE")
        if has_ep_day:
            ax.plot(hours_ep, m_dot_ep, "b-", linewidth=1.2, label="E+")
        ax.set_ylabel("Flow [kg/s]", fontsize=8)
        ax.set_title(f"{months[month_idx]} 15 — Supply Mass Flow", fontsize=9)
        ax.legend(fontsize=7, loc="best")
        ax.grid(alpha=0.3)
        if month_idx == 11:
            ax.set_xlabel("Hour")

        # Col 3: Delivered energy
        ax = axes[month_idx, 2]
        ax.plot(hours_ob, delivered_ob, "r-", linewidth=1.2, label="OpenBSE")
        if has_ep_day:
            ax.plot(hours_ep, delivered_ep, "b-", linewidth=1.2, label="E+")
        ax.axhline(0, color="black", linewidth=0.5)
        ax.set_ylabel("Energy [kW]", fontsize=8)
        ax.set_title(f"{months[month_idx]} 15 — Delivered Energy", fontsize=9)
        ax.legend(fontsize=7, loc="best")
        ax.grid(alpha=0.3)
        if month_idx == 11:
            ax.set_xlabel("Hour")

    fig.suptitle("Supply Air Analysis: G SW Apt — E+ (blue) vs OpenBSE (red)",
                 fontsize=13)
    fig.tight_layout(rect=[0, 0, 1, 0.98])
    path = os.path.join(OUT_DIR, "supply_air_monthly.png")
    fig.savefig(path, dpi=150)
    plt.close(fig)
    print(f"  Saved: {path}")


# ---------------------------------------------------------------------------
# Chart 5: Annual Surface Conduction Comparison (bar chart per surface)
# ---------------------------------------------------------------------------
def chart_conduction_comparison():
    print("  Loading E+ hourly data for conduction...")
    ep = load_eplus_hourly()
    print("  Loading OpenBSE surface data for conduction...")
    ob_s = load_ob_surface()

    # E+ surface conduction: W at 10-min timestep.
    # Annual energy [Wh] = sum(W) * (10/60) = sum(W) / 6
    # Convert to kWh: / 1000
    ep_surfaces = {
        "South Wall":  "G SWALL SWA:Surface Inside Face Conduction Heat Transfer Rate [W](TimeStep)",
        "West Wall":   "G WWALL SWA:Surface Inside Face Conduction Heat Transfer Rate [W](TimeStep)",
        "North Wall":  "G NIWALL SWA:Surface Inside Face Conduction Heat Transfer Rate [W](TimeStep)",
        "East Wall":   "G EIWALL SWA:Surface Inside Face Conduction Heat Transfer Rate [W](TimeStep)",
        "Floor":       "G GFLOOR SWA:Surface Inside Face Conduction Heat Transfer Rate [W](TimeStep)",
        "Ceiling":     "G CEILIN SWA:Surface Inside Face Conduction Heat Transfer Rate [W](TimeStep)",
    }

    # OB surface conduction: W at hourly timestep
    # Annual energy [Wh] = sum(W) * 1 = sum(W)
    # Convert to kWh: / 1000
    ob_surfaces = {
        "South Wall":    "G SW Apt South Wall:surface_conduction_inside [W]",
        "South Window":  "G SW Apt South Window:surface_conduction_inside [W]",
        "West Wall":     "G SW Apt West Wall:surface_conduction_inside [W]",
        "West Window":   "G SW Apt West Window:surface_conduction_inside [W]",
        "North Wall":    "G SW Apt North Wall:surface_conduction_inside [W]",
        "East Wall":     "G SW Apt East Wall:surface_conduction_inside [W]",
        "Floor":         "G SW Apt Floor:surface_conduction_inside [W]",
        "Ceiling":       "G SW Apt Ceiling:surface_conduction_inside [W]",
    }

    # E+ window columns for annual bar chart
    ep_win_cols = {
        "South Window": ("GWINDOW1 S SWA:Surface Window Heat Gain Rate [W](TimeStep)",
                         "GWINDOW1 S SWA:Surface Window Heat Loss Rate [W](TimeStep)"),
        "West Window":  ("GWINDOW1 W SWA:Surface Window Heat Gain Rate [W](TimeStep)",
                         "GWINDOW1 W SWA:Surface Window Heat Loss Rate [W](TimeStep)"),
    }

    # Separate walls and windows for cleaner comparison
    surface_labels = ["South Wall", "South Window\n(incl. solar)", "West Wall",
                      "West Window\n(incl. solar)", "North Wall", "East Wall",
                      "Floor", "Ceiling"]

    ep_vals = []
    ob_vals = []

    # South Wall (opaque only)
    ep_vals.append(ep[ep_surfaces["South Wall"]].sum() / 6.0 / 1000.0)
    ob_vals.append(ob_s[ob_surfaces["South Wall"]].sum() / 1000.0)

    # South Window: E+ gain - loss (includes solar), OB conduction
    ep_gain_col, ep_loss_col = ep_win_cols["South Window"]
    ep_vals.append((ep[ep_gain_col].sum() - ep[ep_loss_col].sum()) / 6.0 / 1000.0)
    if ob_surfaces["South Window"] in ob_s.columns:
        ob_vals.append(ob_s[ob_surfaces["South Window"]].sum() / 1000.0)
    else:
        ob_vals.append(0.0)

    # West Wall (opaque only)
    ep_vals.append(ep[ep_surfaces["West Wall"]].sum() / 6.0 / 1000.0)
    ob_vals.append(ob_s[ob_surfaces["West Wall"]].sum() / 1000.0)

    # West Window
    ep_gain_col, ep_loss_col = ep_win_cols["West Window"]
    ep_vals.append((ep[ep_gain_col].sum() - ep[ep_loss_col].sum()) / 6.0 / 1000.0)
    if ob_surfaces["West Window"] in ob_s.columns:
        ob_vals.append(ob_s[ob_surfaces["West Window"]].sum() / 1000.0)
    else:
        ob_vals.append(0.0)

    # North, East, Floor, Ceiling
    for name in ["North Wall", "East Wall", "Floor", "Ceiling"]:
        ep_vals.append(ep[ep_surfaces[name]].sum() / 6.0 / 1000.0)
        ob_vals.append(ob_s[ob_surfaces[name]].sum() / 1000.0)

    fig, ax = plt.subplots(figsize=(12, 6))
    x = np.arange(len(surface_labels))
    width = 0.35

    ax.bar(x - width/2, ep_vals, width, label="E+", color="#4C72B0", edgecolor="black", linewidth=0.5)
    ax.bar(x + width/2, ob_vals, width, label="OpenBSE", color="#DD8452", edgecolor="black", linewidth=0.5)

    for i, (ep_v, ob_v) in enumerate(zip(ep_vals, ob_vals)):
        if abs(ep_v) > 1:
            diff_pct = (ob_v - ep_v) / abs(ep_v) * 100
            color = "#2ca02c" if abs(diff_pct) < 5 else "#d62728"
            ypos = max(ep_v, ob_v) if max(ep_v, ob_v) > 0 else min(ep_v, ob_v)
            va = "bottom" if ypos >= 0 else "top"
            offset = abs(max(abs(ep_v), abs(ob_v))) * 0.03
            ypos = ypos + offset if ypos >= 0 else ypos - offset
            ax.text(x[i], ypos, f"{diff_pct:+.1f}%",
                    ha="center", va=va, fontsize=9, fontweight="bold", color=color)

    ax.set_ylabel("Annual Conduction [kWh] (positive = into zone)")
    ax.set_title("Surface Conduction Comparison: G SW Apt — E+ vs OpenBSE")
    ax.set_xticks(x)
    ax.set_xticklabels(surface_labels)
    ax.legend()
    ax.grid(axis="y", alpha=0.3)
    ax.axhline(0, color="black", linewidth=0.5)
    fig.tight_layout()
    path = os.path.join(OUT_DIR, "conduction_comparison.png")
    fig.savefig(path, dpi=150)
    plt.close(fig)
    print(f"  Saved: {path}")


# ---------------------------------------------------------------------------
# Chart 6: Hourly surface conduction line charts (winter + summer day)
# ---------------------------------------------------------------------------
def chart_conduction_daily():
    print("  Loading E+ hourly data...")
    ep = load_eplus_hourly()
    print("  Loading OpenBSE surface data...")
    ob_s = load_ob_surface()

    ep_surfaces = {
        "South Wall":  "G SWALL SWA:Surface Inside Face Conduction Heat Transfer Rate [W](TimeStep)",
        "West Wall":   "G WWALL SWA:Surface Inside Face Conduction Heat Transfer Rate [W](TimeStep)",
        "North Wall":  "G NIWALL SWA:Surface Inside Face Conduction Heat Transfer Rate [W](TimeStep)",
        "East Wall":   "G EIWALL SWA:Surface Inside Face Conduction Heat Transfer Rate [W](TimeStep)",
        "Floor":       "G GFLOOR SWA:Surface Inside Face Conduction Heat Transfer Rate [W](TimeStep)",
        "Ceiling":     "G CEILIN SWA:Surface Inside Face Conduction Heat Transfer Rate [W](TimeStep)",
    }

    ob_surfaces = {
        "South Wall":   "G SW Apt South Wall:surface_conduction_inside [W]",
        "West Wall":    "G SW Apt West Wall:surface_conduction_inside [W]",
        "North Wall":   "G SW Apt North Wall:surface_conduction_inside [W]",
        "East Wall":    "G SW Apt East Wall:surface_conduction_inside [W]",
        "Floor":        "G SW Apt Floor:surface_conduction_inside [W]",
        "Ceiling":      "G SW Apt Ceiling:surface_conduction_inside [W]",
    }

    # Also add windows combined with their parent wall
    ob_win_south = "G SW Apt South Window:surface_conduction_inside [W]"
    ob_win_west = "G SW Apt West Window:surface_conduction_inside [W]"

    days = [
        ("Jan 15 (Winter)", 1, 15),
        ("Jul 15 (Summer)", 7, 15),
    ]

    surface_names = ["South Wall", "South Window\n(incl. solar)", "West Wall", "West Window\n(incl. solar)", "North Wall", "Floor", "Ceiling"]
    ep_keys =       ["South Wall", "_south_win_", "West Wall", "_west_win_", "North Wall", "Floor", "Ceiling"]

    fig, axes = plt.subplots(len(days), len(surface_names), figsize=(26, 8), sharex=True)

    for row, (day_label, month, day) in enumerate(days):
        start = pd.Timestamp(2017, month, day)
        end = start + pd.Timedelta(hours=23)

        # E+ data — resample to hourly
        day_ep = ep.loc[start:end]
        if len(day_ep) > 0:
            day_ep_hr = day_ep.resample("1h").mean()
            hours_ep = np.arange(len(day_ep_hr))
        else:
            hours_ep = np.arange(24)

        # OB data
        day_ob = ob_s.loc[start:end]
        hours_ob = np.arange(len(day_ob))

        for col, (surf_label, ep_key) in enumerate(zip(surface_names, ep_keys)):
            ax = axes[row, col]

            if ep_key == "_south_win_":
                # E+: Window Heat Flow = Heat Gain - Heat Loss (includes solar)
                ep_gain = "GWINDOW1 S SWA:Surface Window Heat Gain Rate [W](TimeStep)"
                ep_loss = "GWINDOW1 S SWA:Surface Window Heat Loss Rate [W](TimeStep)"
                if len(day_ep) > 0 and ep_gain in day_ep.columns:
                    ep_win_flow = (day_ep_hr[ep_gain].values - day_ep_hr[ep_loss].values) / 1000.0
                    ax.plot(hours_ep, ep_win_flow, "b-", linewidth=1.5, label="E+")
                if len(day_ob) > 0 and ob_win_south in ob_s.columns:
                    ax.plot(hours_ob, day_ob[ob_win_south].values / 1000.0, "r--", linewidth=1.5, label="OpenBSE")
            elif ep_key == "_west_win_":
                ep_gain = "GWINDOW1 W SWA:Surface Window Heat Gain Rate [W](TimeStep)"
                ep_loss = "GWINDOW1 W SWA:Surface Window Heat Loss Rate [W](TimeStep)"
                if len(day_ep) > 0 and ep_gain in day_ep.columns:
                    ep_win_flow = (day_ep_hr[ep_gain].values - day_ep_hr[ep_loss].values) / 1000.0
                    ax.plot(hours_ep, ep_win_flow, "b-", linewidth=1.5, label="E+")
                if len(day_ob) > 0 and ob_win_west in ob_s.columns:
                    ax.plot(hours_ob, day_ob[ob_win_west].values / 1000.0, "r--", linewidth=1.5, label="OpenBSE")
            else:
                # Regular opaque surface
                if len(day_ep) > 0 and ep_surfaces[ep_key] in day_ep.columns:
                    ep_vals = day_ep_hr[ep_surfaces[ep_key]].values / 1000.0
                    ax.plot(hours_ep, ep_vals, "b-", linewidth=1.5, label="E+")

                if len(day_ob) > 0:
                    ob_col = ob_surfaces[ep_key]
                    ob_vals = day_ob[ob_col].values / 1000.0
                    ax.plot(hours_ob, ob_vals, "r--", linewidth=1.5, label="OpenBSE")

            ax.axhline(0, color="black", linewidth=0.3)
            ax.grid(alpha=0.3)
            ax.set_ylabel("Conduction [kW]", fontsize=8)
            if row == 0:
                ax.set_title(surf_label, fontsize=10)
            if col == 0:
                ax.text(-0.25, 0.5, day_label, transform=ax.transAxes,
                        fontsize=11, fontweight="bold", va="center", rotation=90)
            if row == len(days) - 1:
                ax.set_xlabel("Hour")
            if row == 0 and col == 0:
                ax.legend(fontsize=8)

    fig.suptitle("Hourly Surface Conduction: G SW Apt — E+ (blue) vs OpenBSE (red dashed)",
                 fontsize=13)
    fig.tight_layout(rect=[0.03, 0, 1, 0.96])
    path = os.path.join(OUT_DIR, "conduction_daily_sw.png")
    fig.savefig(path, dpi=150)
    plt.close(fig)
    print(f"  Saved: {path}")


# ---------------------------------------------------------------------------
# Chart 7: Hourly conduction for G N1 Apt (north-facing, minimal solar)
# ---------------------------------------------------------------------------
def chart_conduction_daily_n1():
    print("  Loading E+ hourly data...")
    ep = load_eplus_hourly()
    print("  Loading OpenBSE surface data...")
    ob_s = load_ob_surface()

    # E+ surface names for G N1
    ep_n1 = {
        "North Wall":  "G NWALL N1A:Surface Inside Face Conduction Heat Transfer Rate [W](TimeStep)",
        "South Wall\n(interzone)": "G SIWALL N1A:Surface Inside Face Conduction Heat Transfer Rate [W](TimeStep)",
        "East Wall":   "G EIWALL N1A:Surface Inside Face Conduction Heat Transfer Rate [W](TimeStep)",
        "Floor":       "G GFLOOR N1A:Surface Inside Face Conduction Heat Transfer Rate [W](TimeStep)",
        "Ceiling":     "G CEILIN N1A:Surface Inside Face Conduction Heat Transfer Rate [W](TimeStep)",
    }

    # OpenBSE surface names for G N1
    ob_n1 = {
        "North Wall":  "G N1 Apt North Wall:surface_conduction_inside [W]",
        "South Wall\n(interzone)": "G Corridor North Wall (G N1 Apt):surface_conduction_inside [W]",
        "East Wall":   "G N1 Apt East Wall:surface_conduction_inside [W]",
        "Floor":       "G N1 Apt Floor:surface_conduction_inside [W]",
        "Ceiling":     "G N1 Apt Ceiling:surface_conduction_inside [W]",
    }

    ob_win_north = "G N1 Apt North Window:surface_conduction_inside [W]"

    # E+ window columns
    ep_win_gain = "GWINDOW1 N N1A:Surface Window Heat Gain Rate [W](TimeStep)"
    ep_win_loss = "GWINDOW1 N N1A:Surface Window Heat Loss Rate [W](TimeStep)"
    ep_solar = "G N1 APARTMENT:Enclosure Windows Total Transmitted Solar Radiation Rate [W](TimeStep)"

    days = [
        ("Jan 15 (Winter)", 1, 15),
        ("Jul 15 (Summer)", 7, 15),
    ]

    surface_names = list(ep_n1.keys()) + ["North Window\n(incl. solar)"]
    ncols = len(surface_names)

    fig, axes = plt.subplots(len(days), ncols, figsize=(22, 8), sharex=True)

    for row, (day_label, month, day) in enumerate(days):
        start = pd.Timestamp(2017, month, day)
        end = start + pd.Timedelta(hours=23)

        day_ep = ep.loc[start:end]
        if len(day_ep) > 0:
            day_ep_hr = day_ep.resample("1h").mean()
            hours_ep = np.arange(len(day_ep_hr))
        else:
            hours_ep = np.arange(24)

        day_ob = ob_s.loc[start:end]
        hours_ob = np.arange(len(day_ob))

        for col, surf_label in enumerate(surface_names):
            ax = axes[row, col]

            if surf_label == "North Window\n(incl. solar)":
                # E+: Window Heat Flow = Heat Gain - Heat Loss (includes solar)
                if len(day_ep) > 0 and ep_win_gain in day_ep.columns:
                    ep_win_flow = (day_ep_hr[ep_win_gain].values
                                   - day_ep_hr[ep_win_loss].values) / 1000.0
                    ax.plot(hours_ep, ep_win_flow, "b-", linewidth=1.5, label="E+")
                if len(day_ob) > 0 and ob_win_north in ob_s.columns:
                    ax.plot(hours_ob, day_ob[ob_win_north].values / 1000.0,
                            "r--", linewidth=1.5, label="OpenBSE")
            else:
                ep_col = ep_n1[surf_label]
                ob_col = ob_n1[surf_label]
                if len(day_ep) > 0 and ep_col in day_ep.columns:
                    ax.plot(hours_ep, day_ep_hr[ep_col].values / 1000.0,
                            "b-", linewidth=1.5, label="E+")
                if len(day_ob) > 0 and ob_col in ob_s.columns:
                    ax.plot(hours_ob, day_ob[ob_col].values / 1000.0,
                            "r--", linewidth=1.5, label="OpenBSE")

            ax.axhline(0, color="black", linewidth=0.3)
            ax.grid(alpha=0.3)
            ax.set_ylabel("Conduction [kW]", fontsize=8)
            if row == 0:
                ax.set_title(surf_label, fontsize=10)
            if col == 0:
                ax.text(-0.25, 0.5, day_label, transform=ax.transAxes,
                        fontsize=11, fontweight="bold", va="center", rotation=90)
            if row == len(days) - 1:
                ax.set_xlabel("Hour")
            if row == 0 and col == 0:
                ax.legend(fontsize=8)

    fig.suptitle("Hourly Surface Conduction: G N1 Apt (north-facing) — E+ (blue) vs OpenBSE (red dashed)",
                 fontsize=13)
    fig.tight_layout(rect=[0.03, 0, 1, 0.96])
    path = os.path.join(OUT_DIR, "conduction_daily_n1.png")
    fig.savefig(path, dpi=150)
    plt.close(fig)
    print(f"  Saved: {path}")


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------
def main():
    print("Generating Mid-Rise Apartment diagnostic charts...")
    print()

    print("[1/6] Annual End-Use Comparison")
    chart_annual_enduse()

    print("[2/6] Monthly End-Use Comparison")
    chart_monthly_enduse()

    print("[3/6] Daily Comparison (Jan 15 + Jul 15)")
    chart_daily_comparison()

    print("[4/6] Supply Air Monthly")
    chart_supply_air_monthly()

    print("[5/6] Surface Conduction Comparison (annual bars)")
    chart_conduction_comparison()

    print("[6/7] Surface Conduction Daily — G SW Apt (hourly lines)")
    chart_conduction_daily()  # saves conduction_daily_sw.png

    print("[7/7] Surface Conduction Daily — G N1 Apt (north-facing)")
    chart_conduction_daily_n1()

    print()
    print("All charts saved to:", OUT_DIR)
    print("Done.")


if __name__ == "__main__":
    main()
