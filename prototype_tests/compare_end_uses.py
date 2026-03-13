#!/usr/bin/env python3
"""
Compare OpenBSE vs EnergyPlus annual energy end-use results.
Generates grouped bar charts for each prototype model.
"""

import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
import numpy as np
import os

# ── Colors ─────────────────────────────────────────────────────────────────
OPENBSE_COLOR = '#2196F3'  # Blue
EPLUS_COLOR   = '#FF9800'  # Orange

# ── Residential Single-Family ──────────────────────────────────────────────
# EnergyPlus reference values (kBtu) → convert to kWh (÷ 3.412)
# Source: Simplified IDF (no AirflowNetwork) — single_family/eplus_run/in.idf
# Values from single_family/eplus_run/eplustbl.csv "End Uses" table
eplus_res_kbtu = {
    'Heating\n(Gas)':       22822,
    'Cooling\n(Elec)':       5459,
    'Interior\nLighting':    3543,
    'Interior\nEquipment':  34405,
    'Fans':                  2907,
    'DHW\n(Gas)':            7363,
}
eplus_res = {k: v / 3.412 for k, v in eplus_res_kbtu.items()}

# OpenBSE results (kWh) from summary report
# 4-zone model: living + attic + basement + garage
openbse_res = {
    'Heating\n(Gas)':      10354.7,
    'Cooling\n(Elec)':      2156.8,
    'Interior\nLighting':   1037.6,
    'Interior\nEquipment': 10076.5,
    'Fans':                 1131.3,
    'DHW\n(Gas)':           2172.8,
}

def make_comparison_chart(categories, eplus_vals, openbse_vals, title, filename,
                          unit='kWh', figsize=(12, 6)):
    """Create a grouped bar chart comparing E+ and OpenBSE end uses."""
    x = np.arange(len(categories))
    width = 0.35

    fig, ax = plt.subplots(figsize=figsize)
    bars1 = ax.bar(x - width/2, eplus_vals, width, label='EnergyPlus',
                   color=EPLUS_COLOR, edgecolor='white', linewidth=0.5)
    bars2 = ax.bar(x + width/2, openbse_vals, width, label='OpenBSE',
                   color=OPENBSE_COLOR, edgecolor='white', linewidth=0.5)

    ax.set_ylabel(f'Annual Energy [{unit}]', fontsize=12)
    ax.set_title(title, fontsize=14, fontweight='bold')
    ax.set_xticks(x)
    ax.set_xticklabels(categories, fontsize=10)
    ax.legend(fontsize=11)
    ax.grid(axis='y', alpha=0.3)
    ax.set_axisbelow(True)

    # Add value labels on bars
    def label_bars(bars, vals):
        for bar, val in zip(bars, vals):
            height = bar.get_height()
            if height > 0:
                label = f'{val:,.0f}'
                ax.annotate(label,
                           xy=(bar.get_x() + bar.get_width()/2, height),
                           xytext=(0, 3), textcoords='offset points',
                           ha='center', va='bottom', fontsize=7.5, rotation=0)
    label_bars(bars1, eplus_vals)
    label_bars(bars2, openbse_vals)

    # Add % difference annotations below chart
    diffs = []
    for ev, ov in zip(eplus_vals, openbse_vals):
        if ev > 0:
            pct = (ov - ev) / ev * 100
            diffs.append(f'{pct:+.0f}%')
        else:
            diffs.append('N/A')

    # Add diff row
    for i, (d, xi) in enumerate(zip(diffs, x)):
        color = '#4CAF50' if abs(float(d.replace('%','').replace('+','').replace('N/A','0'))) < 15 else '#F44336'
        ax.annotate(d, xy=(xi, 0), xytext=(0, -25),
                   textcoords='offset points', ha='center', fontsize=8,
                   color=color, fontweight='bold')

    plt.tight_layout()
    plt.subplots_adjust(bottom=0.15)
    outpath = os.path.join(os.path.dirname(__file__), filename)
    plt.savefig(outpath, dpi=150, bbox_inches='tight')
    plt.close()
    print(f'Saved: {outpath}')
    return outpath

# ── Generate residential chart ─────────────────────────────────────────────
cats = list(eplus_res.keys())
ev = [eplus_res[c] for c in cats]
ov = [openbse_res[c] for c in cats]

make_comparison_chart(
    cats, ev, ov,
    'Single-Family House (CZ5B Boulder) — Energy End-Use Comparison',
    'single_family/SingleFamily_CZ5B_comparison.png'
)

# Print tabular summary
print('\n' + '='*70)
print('Single-Family House — Energy End-Use Comparison (kWh)')
print('='*70)
print(f'{"End Use":<22} {"EnergyPlus":>12} {"OpenBSE":>12} {"Diff":>10}')
print('-'*56)
total_ep, total_ob = 0, 0
for c in cats:
    ep_val = eplus_res[c]
    ob_val = openbse_res[c]
    total_ep += ep_val
    total_ob += ob_val
    pct = (ob_val - ep_val) / ep_val * 100 if ep_val > 0 else 0
    c_flat = c.replace('\n', ' ')
    print(f'{c_flat:<22} {ep_val:>12,.1f} {ob_val:>12,.1f} {pct:>+9.1f}%')
print('-'*56)
pct_total = (total_ob - total_ep) / total_ep * 100
print(f'{"TOTAL":<22} {total_ep:>12,.1f} {total_ob:>12,.1f} {pct_total:>+9.1f}%')


# ── Large Office (Boulder, ASHRAE 90.1-2019) ─────────────────────────────
# EnergyPlus reference values (GJ → kWh: ×277.778)
# Source: LargeOffice_Denver_simplified.idf (large_office/eplus_run/eplustbl.csv)
eplus_office_gj = {
    'Heating\n(Gas)':       1457.05,
    'Cooling\n(Elec)':      1990.56,
    'Interior\nLighting':   5610.74,
    'Exterior\nLighting':   1006.07,
    'Interior\nEquipment': 14676.40,
    'Exterior\nEquipment':  2567.07,
    'Fans':                 3637.15,
    'Pumps':                 467.17,
    'DHW\n(Elec)':           450.48,
}
eplus_office = {k: v * 277.778 for k, v in eplus_office_gj.items()}

# OpenBSE results (kWh) from large_office/LargeOffice_Boulder_summary.txt
openbse_office = {
    'Heating\n(Gas)':       517439.6,
    'Cooling\n(Elec)':      743351.0,
    'Interior\nLighting':  1605569.1,
    'Exterior\nLighting':   296814.9,
    'Interior\nEquipment': 4120788.7,
    'Exterior\nEquipment':  711724.4,
    'Fans':                 813859.0,
    'Pumps':                124105.5,
    'DHW\n(Elec)':          131925.9,
}

cats_o = list(eplus_office.keys())
ev_o = [eplus_office[c] for c in cats_o]
ov_o = [openbse_office[c] for c in cats_o]

make_comparison_chart(
    cats_o, ev_o, ov_o,
    'Large Office (Boulder, ASHRAE 90.1-2019) — Energy End-Use Comparison',
    'large_office/LargeOffice_Boulder_comparison.png',
    figsize=(14, 6)
)

print('\n' + '='*70)
print('Large Office — Energy End-Use Comparison (kWh)')
print('='*70)
print(f'{"End Use":<22} {"EnergyPlus":>12} {"OpenBSE":>12} {"Diff":>10}')
print('-'*56)
total_ep_o, total_ob_o = 0, 0
for c in cats_o:
    ep_val = eplus_office[c]
    ob_val = openbse_office[c]
    total_ep_o += ep_val
    total_ob_o += ob_val
    pct = (ob_val - ep_val) / ep_val * 100 if ep_val > 0 else 0
    c_flat = c.replace('\n', ' ')
    print(f'{c_flat:<22} {ep_val:>12,.1f} {ob_val:>12,.1f} {pct:>+9.1f}%')
print('-'*56)
pct_total_o = (total_ob_o - total_ep_o) / total_ep_o * 100
print(f'{"TOTAL":<22} {total_ep_o:>12,.1f} {total_ob_o:>12,.1f} {pct_total_o:>+9.1f}%')


# ── Hospital (placeholder — will be filled after simulation) ──────────────
# EnergyPlus reference values (GJ → kWh: ×277.778)
eplus_hosp_gj = {
    'Heating\n(Gas)':       2396,
    'Cooling\n(Elec)':      1483,
    'Interior\nLighting':   2340,
    'Interior\nEquipment':  9685,
    'Fans':                 3220,
    'Pumps':                 235,
    'Humidification':       1364,
    'Heat\nRecovery':        172,
    'DHW\n(Gas)':           1542,
}
eplus_hosp = {k: v * 277.778 for k, v in eplus_hosp_gj.items()}

# Check if hospital results exist
hosp_summary = os.path.join(os.path.dirname(__file__), 'hospital', 'Hospital_STD2022_Boulder_summary.txt')
if os.path.exists(hosp_summary):
    print('\n\nHospital results found — parsing...')
    # Parse the summary file for end-use values
    openbse_hosp = {}
    with open(hosp_summary) as f:
        in_enduse = False
        for line in f:
            line = line.strip()
            if 'Energy End-Use Summary' in line:
                in_enduse = True
                continue
            if in_enduse and line.startswith('Total'):
                # Parse the Total line then stop
                parts = line.rsplit(None, 1)
                if len(parts) == 2:
                    try:
                        float(parts[1].replace(',', ''))
                    except ValueError:
                        pass
                in_enduse = False
                continue
            if not in_enduse:
                continue
            if line.startswith('End Use') or line.startswith('---') or not line:
                continue
            parts = line.rsplit(None, 1)
            if len(parts) == 2:
                name = parts[0].strip()
                try:
                    val = float(parts[1].replace(',', ''))
                except ValueError:
                    continue
                # Map summary names to chart categories
                mapping = {
                    'Heating (Gas)': 'Heating\n(Gas)',
                    'Cooling (Electric)': 'Cooling\n(Elec)',
                    'Interior Lighting': 'Interior\nLighting',
                    'Interior Equipment': 'Interior\nEquipment',
                    'Fans (Electric)': 'Fans',
                    'Pumps (Electric)': 'Pumps',
                    'Humidification': 'Humidification',
                    'Heat Recovery': 'Heat\nRecovery',
                    'DHW (Gas)': 'DHW\n(Gas)',
                }
                for sname, cname in mapping.items():
                    if sname in name:
                        openbse_hosp[cname] = val

    if openbse_hosp:
        cats_h = list(eplus_hosp.keys())
        ev_h = [eplus_hosp[c] for c in cats_h]
        ov_h = [openbse_hosp.get(c, 0) for c in cats_h]

        make_comparison_chart(
            cats_h, ev_h, ov_h,
            'Hospital (ASHRAE 90.1-2022, Boulder) — Energy End-Use Comparison',
            'hospital/Hospital_STD2022_comparison.png',
            figsize=(14, 6)
        )

        print('\n' + '='*70)
        print('Hospital — Energy End-Use Comparison (kWh)')
        print('='*70)
        print(f'{"End Use":<22} {"EnergyPlus":>12} {"OpenBSE":>12} {"Diff":>10}')
        print('-'*56)
        total_ep_h, total_ob_h = 0, 0
        for c in cats_h:
            ep_val = eplus_hosp[c]
            ob_val = openbse_hosp.get(c, 0)
            total_ep_h += ep_val
            total_ob_h += ob_val
            pct = (ob_val - ep_val) / ep_val * 100 if ep_val > 0 else 0
            c_flat = c.replace('\n', ' ')
            print(f'{c_flat:<22} {ep_val:>12,.1f} {ob_val:>12,.1f} {pct:>+9.1f}%')
        print('-'*56)
        pct_total_h = (total_ob_h - total_ep_h) / total_ep_h * 100
        print(f'{"TOTAL":<22} {total_ep_h:>12,.1f} {total_ob_h:>12,.1f} {pct_total_h:>+9.1f}%')
else:
    print('\n\nHospital results not yet available — skipping chart.')
    print(f'  Expected: {hosp_summary}')
