#!/usr/bin/env python3
"""Component-by-component energy breakdown: E+ vs OB for living zone."""
import pandas as pd
import numpy as np

EP_DD = 288
SPH = 6  # steps per hour

# E+ data
ep = pd.read_csv("eplus_ideal_run/eplusout.csv")
ep.columns = ep.columns.str.strip()
ep = ep.iloc[EP_DD:]  # skip design days

# OB data
obz = pd.read_csv("SingleFamily_CZ5B_Boulder_ideal_ideal_zone_debug.csv")
obs = pd.read_csv("SingleFamily_CZ5B_Boulder_ideal_ideal_surface_debug.csv")

def ep_annual_kwh(col_name, factor=1.0):
    """Sum E+ column to annual kWh. Rate [W] → ÷SPH÷1000. Energy [J] → ÷3.6e6."""
    col = [c for c in ep.columns if col_name in c]
    if not col:
        return 0.0
    if '[J]' in col[0]:
        return ep[col[0]].sum() / 3.6e6 * factor
    elif '[W]' in col[0]:
        return ep[col[0]].sum() / SPH / 1000 * factor
    return 0.0

def ep_annual_kwh_multi(pattern, factor=1.0):
    """Sum multiple E+ columns matching pattern."""
    cols = [c for c in ep.columns if pattern in c]
    total = 0.0
    for c in cols:
        if '[J]' in c:
            total += ep[c].sum() / 3.6e6 * factor
        elif '[W]' in c:
            total += ep[c].sum() / SPH / 1000 * factor
    return total

def ob_annual_kwh(df, col_name):
    """Sum OB column to annual kWh (rate in W)."""
    cols = [c for c in df.columns if col_name in c]
    if not cols:
        return 0.0
    return sum(df[c].sum() for c in cols) / SPH / 1000

print("=" * 70)
print("COMPONENT ENERGY BREAKDOWN: E+ vs OB (living_unit1)")
print("=" * 70)
print(f"{'Component':<40} {'E+ [kWh]':>10} {'OB [kWh]':>10} {'Δ [kWh]':>10} {'Δ%':>7}")
print("-" * 70)

# 1. HVAC
ep_heat = ep_annual_kwh("IDEAL LOADS LIVING:Zone Ideal Loads Zone Sensible Heating Energy")
ep_cool = ep_annual_kwh("IDEAL LOADS LIVING:Zone Ideal Loads Zone Sensible Cooling Energy")
ob_heat = ob_annual_kwh(obz, "living_unit1:heating_rate")
ob_cool = ob_annual_kwh(obz, "living_unit1:cooling_rate")

print(f"{'Heating':<40} {ep_heat:10.1f} {ob_heat:10.1f} {ob_heat-ep_heat:+10.1f} {(ob_heat-ep_heat)/ep_heat*100:+6.1f}%")
print(f"{'Cooling':<40} {ep_cool:10.1f} {ob_cool:10.1f} {ob_cool-ep_cool:+10.1f} {(ob_cool-ep_cool)/ep_cool*100:+6.1f}%")
print()

# 2. Wall conduction (LIVING zone walls only)
# E+ wall conduction columns for UNIT1 walls
ep_wall_patterns = ["WALL_LDF_1.UNIT1", "WALL_SDR_1.UNIT1", "WALL_LDB_1.UNIT1", "WALL_SDL_1.UNIT1",
                     "WALL_LDF_2.UNIT1", "WALL_SDR_2.UNIT1", "WALL_LDB_2.UNIT1", "WALL_SDL_2.UNIT1"]
ep_wall_total = 0.0
for pat in ep_wall_patterns:
    ep_wall_total += ep_annual_kwh(f"{pat}:Surface Inside Face Conduction Heat Transfer Rate")

# OB wall conduction
ob_wall_cols = [c for c in obs.columns if 'Wall' in c and 'cond_inside' in c
                and 'Garage' not in c and 'Bsmt' not in c and 'Gable' not in c]
ob_wall_total = sum(obs[c].sum() for c in ob_wall_cols) / SPH / 1000

print(f"{'Wall conduction (inside face)':<40} {ep_wall_total:10.1f} {ob_wall_total:10.1f} {ob_wall_total-ep_wall_total:+10.1f}")
for c in ob_wall_cols:
    name = c.split(':')[0]
    val = obs[c].sum() / SPH / 1000
    print(f"  {name:<38} {val:10.1f}")

# 3. Floor/ceiling conduction
ep_floor = ep_annual_kwh("FLOOR_UNIT1:Surface Inside Face Conduction")
ep_ceil = ep_annual_kwh("CEILING_UNIT1:Surface Inside Face Conduction")
ep_intmass = ep_annual_kwh("INTERNALMASS_UNIT1:Surface Inside Face Conduction")
ep_interfloor = ep_annual_kwh("INTER ZONE FLOOR 1:Surface Inside Face Conduction")
ep_door = ep_annual_kwh("DOOR_LDB_UNIT1:Surface Inside Face Conduction")
ep_intdoor = ep_annual_kwh("INT_DOOR_GARAGE1:Surface Inside Face Conduction")

ob_floor = ob_annual_kwh(obs, "Floor:cond_inside")
ob_ceil = ob_annual_kwh(obs, "Ceiling:cond_inside")
ob_intmass = ob_annual_kwh(obs, "Internal Mass Living:cond_inside")
ob_interfloor = ob_annual_kwh(obs, "Inter-floor Slab:cond_inside")
ob_backdoor = ob_annual_kwh(obs, "Back Door:cond_inside")
ob_garagedoor = ob_annual_kwh(obs, "Door to Garage:cond_inside")

print(f"\n{'Floor conduction':<40} {ep_floor:10.1f} {ob_floor:10.1f} {ob_floor-ep_floor:+10.1f}")
print(f"{'Ceiling conduction':<40} {ep_ceil:10.1f} {ob_ceil:10.1f} {ob_ceil-ep_ceil:+10.1f}")
print(f"{'Internal mass conduction':<40} {ep_intmass:10.1f} {ob_intmass:10.1f} {ob_intmass-ep_intmass:+10.1f}")
print(f"{'Inter-zone floor conduction':<40} {ep_interfloor:10.1f} {ob_interfloor:10.1f} {ob_interfloor-ep_interfloor:+10.1f}")
print(f"{'Back door conduction':<40} {ep_door:10.1f} {ob_backdoor:10.1f} {ob_backdoor-ep_door:+10.1f}")
print(f"{'Garage door conduction':<40} {ep_intdoor:10.1f} {ob_garagedoor:10.1f} {ob_garagedoor-ep_intdoor:+10.1f}")

# 4. Window conduction (all 8 LIVING windows)
ep_win_cond = ep_annual_kwh_multi("WINDOW.*UNIT1.*Inside Face Conduction")
# Wait, E+ doesn't output window conduction as "Surface Inside Face Conduction"
# for windows. Let me check what's available.
# Windows in E+ have "Zone Windows Total Heat Gain/Loss Rate"
ep_win_gain = ep_annual_kwh("LIVING_UNIT1:Zone Windows Total Heat Gain Rate")
ep_win_loss = ep_annual_kwh("LIVING_UNIT1:Zone Windows Total Heat Loss Rate")

# OB window cond
ob_win_cond_cols = [c for c in obs.columns if 'Window' in c and 'cond_inside' in c]
ob_win_cond = sum(obs[c].sum() for c in ob_win_cond_cols) / SPH / 1000

print(f"\n{'Window heat gain (E+ total)':<40} {ep_win_gain:10.1f}")
print(f"{'Window heat loss (E+ total)':<40} {ep_win_loss:10.1f}")
print(f"{'Window net (gain-loss)':<40} {ep_win_gain-ep_win_loss:10.1f} {ob_win_cond:10.1f} {ob_win_cond-(ep_win_gain-ep_win_loss):+10.1f}")

# 5. Infiltration
ep_infil_loss = ep_annual_kwh("LIVING_UNIT1:Zone Infiltration Sensible Heat Loss Energy")
ep_infil_gain = ep_annual_kwh("LIVING_UNIT1:Zone Infiltration Sensible Heat Gain Energy")
ob_infil = ob_annual_kwh(obz, "living_unit1:gain_infiltration_sensible")

print(f"\n{'Infiltration loss (E+)':<40} {ep_infil_loss:10.1f}")
print(f"{'Infiltration gain (E+)':<40} {ep_infil_gain:10.1f}")
print(f"{'Infiltration net':<40} {ep_infil_gain-ep_infil_loss:10.1f} {ob_infil:10.1f} {ob_infil-(ep_infil_gain-ep_infil_loss):+10.1f}")

# 6. Solar transmitted
ep_solar = ep_annual_kwh("LIVING_UNIT1:Enclosure Windows Total Transmitted Solar")
ob_solar = ob_annual_kwh(obz, "living_unit1:gain_solar")
print(f"\n{'Solar transmitted':<40} {ep_solar:10.1f} {ob_solar:10.1f} {ob_solar-ep_solar:+10.1f}")

# 7. Internal gains
ep_people = ep_annual_kwh("LIVING_UNIT1:Zone People Sensible Heating Rate")
ep_lights = ep_annual_kwh("LIVING_UNIT1:Zone Lights Total Heating Rate")
ep_equip = ep_annual_kwh("LIVING_UNIT1:Zone Electric Equipment Total Heating Rate")
ep_other = ep_annual_kwh("LIVING_UNIT1:Zone Other Equipment Total Heating Rate")

ob_people = ob_annual_kwh(obz, "living_unit1:gain_people_sensible")
ob_lights = ob_annual_kwh(obz, "living_unit1:gain_lighting")
ob_equip = ob_annual_kwh(obz, "living_unit1:gain_equipment_sensible")

print(f"\n{'People sensible':<40} {ep_people:10.1f} {ob_people:10.1f} {ob_people-ep_people:+10.1f}")
print(f"{'Lights':<40} {ep_lights:10.1f} {ob_lights:10.1f} {ob_lights-ep_lights:+10.1f}")
print(f"{'Equipment':<40} {ep_equip:10.1f} {ob_equip:10.1f} {ob_equip-ep_equip:+10.1f}")
print(f"{'Other equipment (E+)':<40} {ep_other:10.1f}")
ep_int_total = ep_people + ep_lights + ep_equip + ep_other
ob_int_total = ob_people + ob_lights + ob_equip
print(f"{'Total internal gains':<40} {ep_int_total:10.1f} {ob_int_total:10.1f} {ob_int_total-ep_int_total:+10.1f}")

# 8. Window interior heat exchange (convection + radiation)
ep_win_conv = ep_annual_kwh_multi("WINDOW.*UNIT1.*Inside Face Convection Heat Gain Rate")
ep_win_rad = ep_annual_kwh_multi("WINDOW.*UNIT1.*Net Surface Thermal Radiation")
ob_win_conv = sum(obs[c].sum() for c in [c for c in obs.columns if 'Window' in c and 'convection_inside' in c]) / SPH / 1000
ob_win_rad = sum(obs[c].sum() for c in [c for c in obs.columns if 'Window' in c and 'radiation_inside' in c]) / SPH / 1000

print(f"\n{'Window interior convection':<40} {ep_win_conv:10.1f} {ob_win_conv:10.1f} {ob_win_conv-ep_win_conv:+10.1f}")
print(f"{'Window interior LW radiation':<40} {ep_win_rad:10.1f} {ob_win_rad:10.1f} {ob_win_rad-ep_win_rad:+10.1f}")
print(f"{'Window interior total':<40} {ep_win_conv+ep_win_rad:10.1f} {ob_win_conv+ob_win_rad:10.1f} {(ob_win_conv+ob_win_rad)-(ep_win_conv+ep_win_rad):+10.1f}")

# Summary
print("\n" + "=" * 70)
print("GAP ANALYSIS")
print("=" * 70)
delta_heat = ob_heat - ep_heat
print(f"Heating gap: {delta_heat:+.1f} kWh ({delta_heat/ep_heat*100:+.1f}%)")
print(f"Components of the gap:")
gaps = {
    'Wall conduction': ob_wall_total - ep_wall_total,
    'Floor conduction': ob_floor - ep_floor,
    'Ceiling conduction': ob_ceil - ep_ceil,
    'Internal mass': ob_intmass - ep_intmass,
    'Inter-zone floor': ob_interfloor - ep_interfloor,
    'Doors': (ob_backdoor + ob_garagedoor) - (ep_door + ep_intdoor),
    'Window net': ob_win_cond - (ep_win_gain - ep_win_loss),
    'Infiltration': ob_infil - (ep_infil_gain - ep_infil_loss),
    'Solar': ob_solar - ep_solar,
    'Internal gains': ob_int_total - ep_int_total,
    'Cooling difference': -(ob_cool - ep_cool),  # cooling saves heating
}
total_explained = 0.0
for name, gap in sorted(gaps.items(), key=lambda x: abs(x[1]), reverse=True):
    if abs(gap) > 1.0:
        print(f"  {name:<35} {gap:+8.1f} kWh")
        total_explained += gap
print(f"  {'Total explained':<35} {total_explained:+8.1f} kWh")
print(f"  {'Unexplained':<35} {delta_heat-total_explained:+8.1f} kWh")
