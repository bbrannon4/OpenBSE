#!/usr/bin/env python3
"""Debug window glass temperature discrepancy between E+ and OB.

Compare key terms in the window glass heat balance for south window on Jan 15.
"""
import pandas as pd
import numpy as np

# ── E+ data ──────────────────────────────────────────────────────────────────
ep = pd.read_csv("eplus_ideal_run/eplusout.csv")
ep.columns = ep.columns.str.strip()

def epcol(pat):
    matches = [c for c in ep.columns if pat in c]
    return matches[0] if matches else None

south_win = "WINDOW_LDF_1.UNIT1"
sw_conv = epcol(f"{south_win}:Surface Inside Face Convection Heat Gain Rate")
sw_hconv = epcol(f"{south_win}:Surface Inside Face Convection Heat Transfer Coefficient")
sw_hext = epcol(f"{south_win}:Surface Outside Face Convection Heat Transfer Coefficient")
sw_rad = epcol(f"{south_win}:Surface Inside Face Net Surface Thermal Radiation")
tz_col = epcol("LIVING_UNIT1:Zone Mean Air Temperature")

win_area = 4.126  # m²

# Timestep detection
n_rows = len(ep)
steps_per_hour = 6 if n_rows > 52000 else (4 if n_rows > 35000 else 1)
steps_per_day = 24 * steps_per_hour

# E+ has 2 design days (288 rows) before annual data
EP_DD_OFFSET = 288
# Jan 15
jan15_start = EP_DD_OFFSET + 14 * steps_per_day
jan15_end = EP_DD_OFFSET + 15 * steps_per_day
ep_jan15 = ep.iloc[jan15_start:jan15_end].copy()
ep_jan15['hour'] = np.arange(len(ep_jan15)) / steps_per_hour

ep_jan15['ep_tz'] = ep_jan15[tz_col]
ep_jan15['ep_hconv'] = ep_jan15[sw_hconv]
ep_jan15['ep_hext'] = ep_jan15[sw_hext]
ep_jan15['ep_q_conv_W'] = ep_jan15[sw_conv]
ep_jan15['ep_q_rad_W'] = ep_jan15[sw_rad]
# Tglass from convection: q_conv = h * A * (Tg - Tz)
ep_jan15['ep_tglass'] = ep_jan15['ep_tz'] + ep_jan15['ep_q_conv_W'] / (ep_jan15['ep_hconv'] * win_area)
# h_rad from radiation: q_rad = h_rad * A * (Tmrt - Tg), but we don't know Tmrt directly
# q_rad / A = h_rad * (Tmrt - Tg)

# ── OB data ──────────────────────────────────────────────────────────────────
ob = pd.read_csv("SingleFamily_CZ5B_Boulder_ideal_ideal_surface_debug.csv")
ob.columns = ob.columns.str.strip()

# Columns (1-indexed from grep, 0-indexed for pandas)
# inside_temperature col 90 → index 89
ob_tglass_col = "Window South 1F:inside_temperature [°C]"
ob_text_col = "Window South 1F:outside_temperature [°C]"
ob_conv_col = "Window South 1F:convection_inside [W]"
ob_rad_col = "Window South 1F:radiation_inside [W]"
ob_hrad_col = "Window South 1F:inside_radiation_coefficient [W]"
ob_tsol_col = "Window South 1F:transmitted_solar [W]"
ob_isol_col = "Window South 1F:incident_solar [W/m²]"

# Zone debug for zone temp
obz = pd.read_csv("SingleFamily_CZ5B_Boulder_ideal_ideal_zone_debug.csv")
obz.columns = obz.columns.str.strip()
tz_ob_col = [c for c in obz.columns if "living" in c.lower() and "air_temp" in c.lower()]
print(f"OB zone temp columns: {tz_ob_col}")
if not tz_ob_col:
    tz_ob_col = [c for c in obz.columns if "living" in c.lower() and "temp" in c.lower()]
    print(f"OB zone temp columns (retry): {tz_ob_col}")
if not tz_ob_col:
    print("All OB zone debug columns:")
    for c in obz.columns:
        print(f"  {c}")

# OB timestep
ob_n = len(ob)
ob_sph = 6 if ob_n > 52000 else (4 if ob_n > 35000 else 1)
ob_spd = 24 * ob_sph

ob_jan15 = ob.iloc[14*ob_spd:15*ob_spd].copy()
ob_jan15['hour'] = np.arange(len(ob_jan15)) / ob_sph

obz_jan15 = obz.iloc[14*ob_spd:15*ob_spd].copy()

# Get OB values
ob_jan15['ob_tglass'] = ob_jan15[ob_tglass_col].values
ob_jan15['ob_text'] = ob_jan15[ob_text_col].values
ob_jan15['ob_q_conv_W'] = ob_jan15[ob_conv_col].values
ob_jan15['ob_q_rad_W'] = ob_jan15[ob_rad_col].values
ob_jan15['ob_h_rad'] = ob_jan15[ob_hrad_col].values
ob_jan15['ob_tsol'] = ob_jan15[ob_tsol_col].values
ob_jan15['ob_isol'] = ob_jan15[ob_isol_col].values

if tz_ob_col:
    ob_jan15['ob_tz'] = obz_jan15[tz_ob_col[0]].values
else:
    ob_jan15['ob_tz'] = 22.2  # fallback

# Back-calculate OB h_conv: q_conv = h_conv * A * (Tg - Tz)  ... but windows q_conv_inside=0 in OB!
# The convection_inside for windows is different — check what it contains
# Actually in OB code, self.surfaces[i].q_conv_inside = 0.0 for windows
# The window convection goes through sum_ha/sum_hat. So the "convection_inside" column may be 0.

# Let's compute OB h_conv from the glass equation terms instead
# Also get h_conv from the stored value (h_conv_inside in SurfaceState)

print("\n=== Jan 15 Comparison: South Window Glass Temperature ===")
print(f"{'Hr':>3} │{'EP Tz':>6} {'OB Tz':>6}│{'EP Tg':>7} {'OB Tg':>7} {'ΔTg':>5}│{'EP hci':>6} {'EP hce':>6}│{'EP qconv':>8} {'OB qconv':>8}│{'EP qrad':>8} {'OB qrad':>8}│{'OB hrad':>6}│{'OB Isol':>7}")
print("─"*120)

# Hourly averages
for hr in range(24):
    ep_mask = (ep_jan15['hour'] >= hr) & (ep_jan15['hour'] < hr+1)
    ob_mask = (ob_jan15['hour'] >= hr) & (ob_jan15['hour'] < hr+1)

    e = ep_jan15[ep_mask].mean(numeric_only=True)
    o = ob_jan15[ob_mask].mean(numeric_only=True)

    ep_tz = e['ep_tz']
    ob_tz = o['ob_tz']
    ep_tg = e['ep_tglass']
    ob_tg = o['ob_tglass']
    dtg = ob_tg - ep_tg
    ep_hci = e['ep_hconv']
    ep_hce = e['ep_hext']
    ep_qc = e['ep_q_conv_W']
    ob_qc = o['ob_q_conv_W']
    ep_qr = e['ep_q_rad_W']
    ob_qr = o['ob_q_rad_W']
    ob_hr = o['ob_h_rad']
    ob_isol = o['ob_isol']

    print(f"{hr:3d} │{ep_tz:6.1f} {ob_tz:6.1f}│{ep_tg:7.2f} {ob_tg:7.2f} {dtg:+5.1f}│{ep_hci:6.2f} {ep_hce:6.1f}│{ep_qc:8.1f} {ob_qc:8.1f}│{ep_qr:8.1f} {ob_qr:8.1f}│{ob_hr:6.2f}│{ob_isol:7.1f}")

# Annual totals for window heat flows
print("\n\n=== Annual Window Convection & Radiation (all 8 LIVING windows) ===")
# E+: sum all 8 window convection
ep_win_conv_cols = [c for c in ep.columns if "WINDOW" in c and "UNIT1" in c and "Inside Face Convection Heat Gain Rate" in c]
ep_win_rad_cols = [c for c in ep.columns if "WINDOW" in c and "UNIT1" in c and "Net Surface Thermal Radiation" in c]

ep_total_conv = sum(ep[c].sum() for c in ep_win_conv_cols) / steps_per_hour / 6000  # W→kWh (10min)
ep_total_rad = sum(ep[c].sum() for c in ep_win_rad_cols) / steps_per_hour / 6000

# For OB: we need to sum all window convection and radiation
ob_win_conv_cols = [c for c in ob.columns if "Window" in c and "convection_inside" in c]
ob_win_rad_cols = [c for c in ob.columns if "Window" in c and "radiation_inside" in c]

print(f"OB window conv columns: {ob_win_conv_cols}")
print(f"OB window rad columns: {ob_win_rad_cols}")

ob_total_conv = sum(ob[c].sum() for c in ob_win_conv_cols) / ob_sph / 1000  # W→kWh
ob_total_rad = sum(ob[c].sum() for c in ob_win_rad_cols) / ob_sph / 1000

# Wait — EP conv is in W already, timestep is 10min = 1/6 hr
# Total energy = sum(W) × Δt where Δt = 1/steps_per_hour hour
# Skip design days (first 288 rows) for annual totals
ep_annual = ep.iloc[EP_DD_OFFSET:]
ep_total_conv_kwh = sum(ep_annual[c].sum() for c in ep_win_conv_cols) / steps_per_hour / 1000
ep_total_rad_kwh = sum(ep_annual[c].sum() for c in ep_win_rad_cols) / steps_per_hour / 1000
ob_total_conv_kwh = sum(ob[c].sum() for c in ob_win_conv_cols) / ob_sph / 1000
ob_total_rad_kwh = sum(ob[c].sum() for c in ob_win_rad_cols) / ob_sph / 1000

print(f"\nE+ window convection (annual): {ep_total_conv_kwh:8.1f} kWh")
print(f"OB window convection (annual): {ob_total_conv_kwh:8.1f} kWh")
print(f"E+ window LW radiation (annual): {ep_total_rad_kwh:8.1f} kWh")
print(f"OB window LW radiation (annual): {ob_total_rad_kwh:8.1f} kWh")

# Check if OB window convection_inside is really 0 (because q_conv_inside=0 in code)
print(f"\nOB Window South 1F convection_inside first 10 values:")
print(ob["Window South 1F:convection_inside [W]"].head(10).tolist())
print(f"OB Window South 1F radiation_inside first 10 values:")
print(ob["Window South 1F:radiation_inside [W]"].head(10).tolist())
