#!/usr/bin/env python3
"""
Compare winter day zone heat balance gains for PERIMETER_BOT_ZN_1
between E+ ideal loads and OpenBSE.

6 panels:
  1. Total envelope conduction (opaque + window/door, excl. internal mass)
  2. Internal gains (people + lights + equipment)
  3. Transmitted solar (sunlight through windows into zone air)
  4. Internal mass conduction (furniture/partition thermal storage)
  5. Infiltration sensible
  6. HVAC load (heating - cooling)
"""
import csv
import numpy as np
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
from pathlib import Path

HERE = Path(__file__).parent
EP_ESO = Path("/tmp/eplus_ideal_rerun/eplusout.eso")
OB_CSV = HERE / "LargeOffice_Boulder_ideal_results.csv"

WINTER_MONTH, WINTER_DAY = 12, 21

# ═══════════════════════════════════════════════════════════════════
# Parse E+ ESO
# ═══════════════════════════════════════════════════════════════════
def parse_eso(path):
    var_map = {}
    data = {}
    with open(path) as f:
        for line in f:
            line = line.strip()
            if line.startswith("End of Data Dictionary"):
                break
            parts = line.split(',')
            if len(parts) >= 4:
                try:
                    vid = int(parts[0])
                    if vid > 5:
                        key = parts[2].strip()
                        var = parts[3].strip().split('[')[0].strip()
                        var_map[vid] = f"{key}:{var}"
                        data[vid] = []
                except ValueError:
                    pass
        for line in f:
            if line.strip().startswith("End of Data"):
                break
            parts = line.split(',')
            if len(parts) >= 2:
                try:
                    vid = int(parts[0])
                    if vid > 5 and vid in data:
                        data[vid].append(float(parts[1]))
                except (ValueError, IndexError):
                    pass
    return var_map, data

print("Parsing E+ ESO...")
var_map, eso_data = parse_eso(EP_ESO)

# Categorize PZN1 variables
pzn1 = {}  # single-value variables
envelope_cond_vids = []  # opaque walls + floor + ceiling + windows + doors (NOT intmass)
intmass_vid = None

for vid, desc in var_map.items():
    d = desc.upper()
    if 'PERIMETER_BOT_ZN_1' not in d:
        continue

    if 'Zone Mean Air Temperature' in desc:
        pzn1['zone_temp'] = vid
    elif 'Zone People Sensible Heating Rate' in desc:
        pzn1['people'] = vid
    elif 'Zone Lights Total Heating Rate' in desc:
        pzn1['lights'] = vid
    elif 'Zone Electric Equipment Total Heating Rate' in desc:
        pzn1['equipment'] = vid
    elif 'Enclosure Windows Total Transmitted Solar Radiation Rate' in desc:
        pzn1['transmitted_solar'] = vid
    elif 'Zone Infiltration Sensible Heat Loss Energy' in desc:
        pzn1['infil_loss'] = vid
    elif 'Zone Infiltration Sensible Heat Gain Energy' in desc:
        pzn1['infil_gain'] = vid
    elif 'Zone Air System Sensible Heating Rate' in desc:
        pzn1['hvac_htg'] = vid
    elif 'Zone Air System Sensible Cooling Rate' in desc:
        pzn1['hvac_clg'] = vid
    elif 'Surface Inside Face Conduction Heat Transfer Rate' in desc:
        if 'INTERNALMASS' in d:
            intmass_vid = vid
            print(f"  E+ IntMass: {desc}")
        else:
            envelope_cond_vids.append(vid)
            print(f"  E+ Envelope cond: {desc}")

# Outdoor temp
for vid, desc in var_map.items():
    if 'Site Outdoor Air Drybulb Temperature' in desc:
        pzn1['outdoor_temp'] = vid
        break

print(f"E+ variables: {len(pzn1)} zone, {len(envelope_cond_vids)} envelope cond, intmass={intmass_vid is not None}")

# Winter day extraction
days_in_month = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
doy = sum(days_in_month[:WINTER_MONTH-1]) + WINTER_DAY
si, ei = (doy - 1) * 24, doy * 24
hours = np.arange(24) + 0.5

def ep(vid):
    return np.array(eso_data[vid][si:ei])

# E+ arrays
ep_outdoor = ep(pzn1['outdoor_temp'])
ep_zone_temp = ep(pzn1['zone_temp'])
ep_people = ep(pzn1['people'])
ep_lights = ep(pzn1['lights'])
ep_equip = ep(pzn1['equipment'])
ep_internal = ep_people + ep_lights + ep_equip
ep_solar = ep(pzn1['transmitted_solar'])
ep_infil = (ep(pzn1['infil_gain']) - ep(pzn1['infil_loss'])) / 3600.0  # J→W
ep_hvac = ep(pzn1['hvac_htg']) - ep(pzn1['hvac_clg'])

# Total envelope conduction (all surfaces except internal mass)
ep_env_cond = sum(ep(v) for v in envelope_cond_vids)
ep_intmass = ep(intmass_vid) if intmass_vid else np.zeros(24)

# ═══════════════════════════════════════════════════════════════════
# Parse OB results
# ═══════════════════════════════════════════════════════════════════
print("\nParsing OB results...")
ob = {}
with open(OB_CSV) as f:
    reader = csv.reader(f)
    hdrs = next(reader)
    for h in hdrs:
        ob[h] = []
    for row in reader:
        for i, v in enumerate(row):
            try:
                ob[hdrs[i]].append(float(v))
            except (ValueError, IndexError):
                ob[hdrs[i]].append(0.0)
for k in ob:
    ob[k] = np.array(ob[k])

mask = (ob['Month'] == WINTER_MONTH) & (ob['Day'] == WINTER_DAY)
n = mask.sum()
sph = n // 24
print(f"OB: {n} timesteps on Dec {WINTER_DAY} ({sph}/hr)")

def ob_get(field):
    raw = ob[field][mask]
    if sph > 1:
        return raw.reshape(24, sph).mean(axis=1)
    return raw

ob_zone_temp = ob_get('Perimeter_bot_ZN_1:zone_temp [-]')
ob_opaque = ob_get('Perimeter_bot_ZN_1:opaque_conduction [-]')
ob_wincond = ob_get('Perimeter_bot_ZN_1:window_conduction [-]')
ob_env_cond = ob_opaque + ob_wincond  # total envelope = opaque + window
ob_solar = ob_get('Perimeter_bot_ZN_1:transmitted_solar [-]')
ob_qconv = ob_get('Perimeter_bot_ZN_1:q_internal_conv [-]')
ob_qrad = ob_get('Perimeter_bot_ZN_1:q_internal_rad [-]')
ob_internal = ob_qconv + ob_qrad

# Infiltration: mass flow × cp × (T_out - T_zone)
ob_mflow = ob_get('Perimeter_bot_ZN_1:infiltration_mass_flow [-]')
ob_infil = ob_mflow * 1005.0 * (ep_outdoor - ob_zone_temp)

# Internal mass
im_col = 'Surf:Perimeter_bot_ZN_1 IntMass 1:cond_inside [-]'
ob_intmass = ob_get(im_col) if im_col in hdrs else np.zeros(24)

# HVAC
ob_hvac = ob_get('Perimeter_bot_ZN_1:hvac_heating_rate [-]') - ob_get('Perimeter_bot_ZN_1:hvac_cooling_rate [-]')

# ═══════════════════════════════════════════════════════════════════
# Plot
# ═══════════════════════════════════════════════════════════════════
print("\nGenerating plot...")
fig, axes = plt.subplots(3, 2, figsize=(16, 14), sharex=True)
fig.suptitle(f'PERIMETER_BOT_ZN_1 — Winter Day (Dec {WINTER_DAY}) Zone Heat Balance',
             fontsize=14, fontweight='bold')

def panel(ax, ep_arr, ob_arr, title, ylabel='Rate [W]'):
    ax.plot(hours, ep_arr, 'b-', lw=1.8, label='E+')
    ax.plot(hours, ob_arr, 'r--', lw=1.8, label='OB')
    ax.set_title(title, fontsize=11)
    ax.set_ylabel(ylabel)
    ax.legend(fontsize=9)
    ax.grid(True, alpha=0.3)
    ax.axhline(0, color='k', lw=0.5, alpha=0.5)

panel(axes[0,0], ep_env_cond, ob_env_cond,
      'Total Envelope Conduction\n(opaque walls + floor + ceiling + windows/doors)')
panel(axes[0,1], ep_internal, ob_internal,
      'Total Internal Gains\n(people + lights + equipment)')
panel(axes[1,0], ep_solar, ob_solar,
      'Transmitted Solar\n(sunlight through windows into zone)')
panel(axes[1,1], ep_intmass, ob_intmass,
      'Internal Mass Conduction\n(furniture/partition thermal storage)')
panel(axes[2,0], ep_infil, ob_infil,
      'Infiltration Sensible\n(outdoor air leakage heat transfer)')
panel(axes[2,1], ep_hvac, ob_hvac,
      'HVAC Load\n(heating positive, cooling negative)')

for ax in axes[-1,:]:
    ax.set_xlabel('Hour of Day')

plt.tight_layout()
out = HERE / 'zone_gains_winter.png'
plt.savefig(out, dpi=150, bbox_inches='tight')
print(f"→ {out}")

# Summary table
print(f"\n{'Component':<30} {'E+ daily Wh':>12} {'OB daily Wh':>12} {'Diff%':>8}")
print('=' * 66)
for name, ea, oa in [
    ('Envelope Conduction', ep_env_cond, ob_env_cond),
    ('Internal Gains',      ep_internal, ob_internal),
    ('Transmitted Solar',   ep_solar,    ob_solar),
    ('Internal Mass',       ep_intmass,  ob_intmass),
    ('Infiltration',        ep_infil,    ob_infil),
    ('HVAC (htg−clg)',      ep_hvac,     ob_hvac),
]:
    es, os_ = ea.sum(), oa.sum()
    pct = ((os_ - es) / abs(es) * 100) if abs(es) > 1 else float('nan')
    print(f'{name:<30} {es:>12,.0f} {os_:>12,.0f} {pct:>+7.1f}%')

print("\nDone.")
