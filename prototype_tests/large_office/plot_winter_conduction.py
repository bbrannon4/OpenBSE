#!/usr/bin/env python3
"""
Detailed conduction breakdown for PERIMETER_BOT_ZN_1 on a winter day.

Panel 1: Total envelope conduction (all surfaces)
Panels 2-8: Individual surface conduction elements
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
vm, ed = parse_eso(EP_ESO)

# Build vid lookup for PZN1 conduction surfaces
ep_cond = {}  # friendly_name -> vid
ep_zone = {}  # other zone vars

# Only match surfaces that start with PERIMETER_BOT_ZN_1 or explicitly reference it
pzn1_surface_vids = {}  # surface_key_in_eso -> vid
for vid, desc in vm.items():
    d = desc.upper()
    if 'Site Outdoor Air Drybulb Temperature' in desc:
        ep_zone['outdoor_temp'] = vid

    if 'PERIMETER_BOT_ZN_1' not in d:
        continue

    if 'Surface Inside Face Conduction Heat Transfer Rate' not in desc:
        if 'Zone Windows Total Heat Gain Rate' in desc:
            ep_zone['win_gain'] = vid
        elif 'Zone Windows Total Heat Loss Rate' in desc:
            ep_zone['win_loss'] = vid
        elif 'Zone Air System Sensible Heating Rate' in desc:
            ep_zone['hvac_htg'] = vid
        elif 'Zone Air System Sensible Cooling Rate' in desc:
            ep_zone['hvac_clg'] = vid
        continue

    key = desc.split(':')[0]
    pzn1_surface_vids[key] = vid

# Now categorize
for key, vid in pzn1_surface_vids.items():
    if key == 'PERIMETER_BOT_ZN_1_WALL_SOUTH':
        ep_cond['South Wall (opaque)'] = vid
    elif key == 'PERIMETER_BOT_ZN_1_WALL_EAST':
        ep_cond['East Wall'] = vid
    elif key == 'PERIMETER_BOT_ZN_1_FLOOR':
        ep_cond['Floor'] = vid
    elif key == 'PERIMETER_BOT_ZN_1_CEILING':
        ep_cond['Ceiling'] = vid
    elif key == 'PERIMETER_BOT_ZN_1_INTERNALMASS_1':
        ep_cond['Internal Mass'] = vid
    elif 'DOOR' in key:
        ep_cond.setdefault('South Doors (sum)', [])
        if isinstance(ep_cond['South Doors (sum)'], list):
            ep_cond['South Doors (sum)'].append(vid)
    elif 'PPAUTOCREATEOTHER' in key or 'DATACENTER' in key:
        ep_cond.setdefault('Interzone Partitions (sum)', [])
        if isinstance(ep_cond['Interzone Partitions (sum)'], list):
            ep_cond['Interzone Partitions (sum)'].append(vid)

print("E+ conduction surfaces:")
for k, v in ep_cond.items():
    print(f"  {k}: {v}")

# Winter day extraction
days_in_month = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
doy = sum(days_in_month[:WINTER_MONTH-1]) + WINTER_DAY
si, ei = (doy - 1) * 24, doy * 24
hours = np.arange(24) + 0.5

def ep_get(vid):
    return np.array(ed[vid][si:ei])

def ep_get_sum(vids):
    if isinstance(vids, list):
        return sum(ep_get(v) for v in vids)
    return ep_get(vids)

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
sph = mask.sum() // 24

def ob_get(field):
    raw = ob[field][mask]
    if sph > 1:
        return raw.reshape(24, sph).mean(axis=1)
    return raw

# OB surface conduction columns
ob_south_wall = ob_get('Surf:P_bot_ZN1 South Wall:cond_inside [-]')
ob_south_win  = ob_get('Surf:P_bot_ZN1 South Window:cond_inside [-]')
ob_floor      = ob_get('Surf:P_bot_ZN1 Floor:cond_inside [-]')
ob_ceiling    = ob_get('Surf:P_bot_ZN1 Ceiling:cond_inside [-]')
ob_north_iw   = ob_get('Surf:P_bot_ZN1 North IW:cond_inside [-]')
ob_intmass    = ob_get('Surf:Perimeter_bot_ZN_1 IntMass 1:cond_inside [-]')

# Zone aggregates
ob_opaque_total = ob_get('Perimeter_bot_ZN_1:opaque_conduction [-]')
ob_window_total = ob_get('Perimeter_bot_ZN_1:window_conduction [-]')
ob_env_total    = ob_opaque_total + ob_window_total

# E+ total envelope (all surfaces including intmass)
ep_all_cond = np.zeros(24)
for k, v in ep_cond.items():
    ep_all_cond += ep_get_sum(v)

# E+ window conduction (from zone-level gain - loss)
ep_win_net = ep_get(ep_zone['win_gain']) - ep_get(ep_zone['win_loss'])

# E+ total envelope conduction (surface cond + window zone-level)
# Note: E+ surface conduction for opaque surfaces is different from
# window heat gain/loss (which includes convection+radiation from glass).
# For total comparison, use all surface conduction + window zone gain/loss.
ep_env_total = ep_all_cond + ep_win_net  # but this double-counts if windows have surface cond too

# Actually, E+ windows don't have Surface Inside Face Conduction in this run,
# so ep_all_cond is only opaque+doors+partitions+intmass. Add window net:
ep_env_total_no_intmass = (ep_all_cond - ep_get_sum(ep_cond['Internal Mass'])) + ep_win_net

# OB total for comparison
ob_env_total_no_intmass = ob_env_total  # opaque_conduction already excludes intmass

# ═══════════════════════════════════════════════════════════════════
# Plot: 4×2 grid
# ═══════════════════════════════════════════════════════════════════
print("\nGenerating plot...")
fig, axes = plt.subplots(4, 2, figsize=(16, 18), sharex=True)
fig.suptitle(f'PERIMETER_BOT_ZN_1 — Winter Day (Dec {WINTER_DAY}) Conduction Breakdown',
             fontsize=14, fontweight='bold')

def panel(ax, ep_arr, ob_arr, title, ylabel='Rate [W]'):
    ax.plot(hours, ep_arr, 'b-', lw=1.8, label='E+')
    ax.plot(hours, ob_arr, 'r--', lw=1.8, label='OB')
    ax.set_title(title, fontsize=11)
    ax.set_ylabel(ylabel)
    ax.legend(fontsize=9)
    ax.grid(True, alpha=0.3)
    ax.axhline(0, color='k', lw=0.5, alpha=0.5)
    # Annotate daily Wh
    ep_d = ep_arr.sum()
    ob_d = ob_arr.sum()
    pct = ((ob_d - ep_d) / abs(ep_d) * 100) if abs(ep_d) > 10 else float('nan')
    txt = f'E+: {ep_d:,.0f} Wh\nOB: {ob_d:,.0f} Wh'
    if not np.isnan(pct):
        txt += f'\nΔ: {pct:+.1f}%'
    ax.text(0.02, 0.02, txt, transform=ax.transAxes, fontsize=8,
            verticalalignment='bottom', bbox=dict(boxstyle='round', facecolor='wheat', alpha=0.8))

# Row 0: Total envelope (excl intmass) and Total incl intmass
panel(axes[0, 0], ep_env_total_no_intmass, ob_env_total_no_intmass,
      'Total Envelope Conduction\n(opaque + windows + doors, excl IntMass)')
panel(axes[0, 1], ep_all_cond, ob_opaque_total,
      'Total Opaque Conduction\n(E+: all opaque surfaces; OB: opaque_conduction)')

# Row 1: South Wall and South Windows
# E+ south wall opaque only vs OB south wall opaque
ep_south_wall = ep_get(ep_cond['South Wall (opaque)'])
# E+ south doors (4 doors in south wall)
ep_south_doors = ep_get_sum(ep_cond['South Doors (sum)'])
ep_south_opaque = ep_south_wall + ep_south_doors

panel(axes[1, 0], ep_south_opaque, ob_south_wall,
      'South Wall (opaque + doors)\n(E+: wall+4 doors; OB: South Wall)')
panel(axes[1, 1], ep_win_net, ob_window_total,
      'Windows Net Heat\n(E+: Zone Win Gain−Loss [incl solar]; OB: window_conduction)')

# Row 2: Floor and Ceiling
ep_floor = ep_get(ep_cond['Floor'])
panel(axes[2, 0], ep_floor, ob_floor,
      'Floor Conduction\n(ground contact / interzone)')

ep_ceiling = ep_get(ep_cond['Ceiling'])
panel(axes[2, 1], ep_ceiling, ob_ceiling,
      'Ceiling Conduction\n(interzone to floor above)')

# Row 3: Interzone partitions and Internal Mass
ep_iz = ep_get_sum(ep_cond['Interzone Partitions (sum)'])
panel(axes[3, 0], ep_iz, ob_north_iw,
      'Interzone Partitions\n(E+: 4 auto-partitions; OB: North IW)')

ep_im = ep_get(ep_cond['Internal Mass'])
panel(axes[3, 1], ep_im, ob_intmass,
      'Internal Mass\n(furniture/partition thermal storage)')

for ax in axes[-1, :]:
    ax.set_xlabel('Hour of Day')

plt.tight_layout()
out = HERE / 'zone_conduction_winter.png'
plt.savefig(out, dpi=150, bbox_inches='tight')
print(f"→ {out}")

# Summary table
print(f"\n{'Surface':<32} {'E+ daily Wh':>12} {'OB daily Wh':>12} {'Diff%':>8}")
print('=' * 68)
rows = [
    ('Total Envelope (excl IM)',  ep_env_total_no_intmass, ob_env_total_no_intmass),
    ('South Wall+Doors',          ep_south_opaque,          ob_south_wall),
    ('Windows Net Heat',           ep_win_net,               ob_window_total),
    ('Floor',                     ep_floor,                 ob_floor),
    ('Ceiling',                   ep_ceiling,               ob_ceiling),
    ('Interzone Partitions',      ep_iz,                    ob_north_iw),
    ('Internal Mass',             ep_im,                    ob_intmass),
]
for name, ea, oa in rows:
    es, os_ = ea.sum(), oa.sum()
    pct = ((os_ - es) / abs(es) * 100) if abs(es) > 10 else float('nan')
    pct_s = f'{pct:+.1f}%' if not np.isnan(pct) else 'N/A'
    print(f'{name:<32} {es:>12,.0f} {os_:>12,.0f} {pct_s:>8}')

print("\nDone.")
