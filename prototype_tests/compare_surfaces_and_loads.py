#!/usr/bin/env python3
"""
Compare OpenBSE and EnergyPlus surface areas and zone-level heating/cooling
for the Mid-Rise Apartment model.
"""

import yaml
import csv
import math
import os

BASE = "/Users/benjaminbrannon/Documents/GitHub/OpenBSE/prototype_tests/apartment"
YAML_FILE = os.path.join(BASE, "ApartmentMidRise_Boulder.yaml")
EPLUS_CSV = os.path.join(BASE, "output_apartment/eplustbl.csv")
ZONE_RESULTS = os.path.join(BASE, "ApartmentMidRise_Boulder_zone_results.csv")

# Custom YAML loader that handles !zone tags
class CustomLoader(yaml.SafeLoader):
    pass

def zone_constructor(loader, node):
    """Handle !zone tags by returning the scalar value as a string."""
    return loader.construct_scalar(node)

CustomLoader.add_constructor('!zone', zone_constructor)

# ─────────────────────────────────────────────────────────────────────────────
# PART 1: SURFACE AREA CHECK from OpenBSE YAML
# ─────────────────────────────────────────────────────────────────────────────

def compute_surface_area(vertices):
    """Compute area of a planar polygon from its 3D vertices using the cross-product method."""
    n = len(vertices)
    if n < 3:
        return 0.0
    # Newell's method for polygon area
    nx, ny, nz = 0.0, 0.0, 0.0
    for i in range(n):
        v_cur = vertices[i]
        v_nxt = vertices[(i + 1) % n]
        nx += (v_cur['y'] - v_nxt['y']) * (v_cur['z'] + v_nxt['z'])
        ny += (v_cur['z'] - v_nxt['z']) * (v_cur['x'] + v_nxt['x'])
        nz += (v_cur['x'] - v_nxt['x']) * (v_cur['y'] + v_nxt['y'])
    return 0.5 * math.sqrt(nx*nx + ny*ny + nz*nz)

print("=" * 80)
print("PART 1: SURFACE AREA COMPARISON - OpenBSE vs EnergyPlus")
print("=" * 80)

with open(YAML_FILE, 'r') as f:
    data = yaml.load(f, Loader=CustomLoader)

surfaces = data.get('surfaces', [])
zones = data.get('zones', [])

# Build zone multiplier map
zone_mult = {}
for z in zones:
    zone_mult[z['name']] = z.get('multiplier', 1)

# Classify surfaces
ext_walls = []       # boundary: outdoor, type: wall
ext_roofs = []       # type: roof with boundary: outdoor
windows = []         # type: window
ground_floors = []   # boundary: ground

total_ext_wall_area = 0.0
total_roof_area = 0.0
total_window_area = 0.0
total_ground_area = 0.0

# Also track by zone for comparison
zone_ext_wall = {}
zone_window = {}

for s in surfaces:
    stype = s.get('type', '')
    boundary = s.get('boundary', '')
    zone = s.get('zone', '')
    area = compute_surface_area(s.get('vertices', []))
    mult = zone_mult.get(zone, 1)
    
    # Boundary can be a string or a YAML tag result
    if isinstance(boundary, str):
        boundary_str = boundary.lower()
    else:
        boundary_str = str(boundary).lower()
    
    if stype == 'wall' and boundary_str == 'outdoor':
        ext_walls.append((s['name'], zone, area, mult))
        total_ext_wall_area += area * mult
        zone_ext_wall[zone] = zone_ext_wall.get(zone, 0) + area
    
    if (stype == 'roof' and boundary_str == 'outdoor') or \
       (stype == 'ceiling' and boundary_str == 'outdoor'):
        ext_roofs.append((s['name'], zone, area, mult))
        total_roof_area += area * mult
    
    if stype == 'window':
        windows.append((s['name'], zone, area, mult))
        total_window_area += area * mult
        zone_window[zone] = zone_window.get(zone, 0) + area
    
    if boundary_str == 'ground':
        ground_floors.append((s['name'], zone, area, mult))
        total_ground_area += area * mult

print("\n--- OpenBSE Surface Areas (from YAML geometry) ---\n")
print(f"  Total Exterior Wall Area:  {total_ext_wall_area:10.2f} m2")
print(f"  Total Roof Area:           {total_roof_area:10.2f} m2")
print(f"  Total Window Area:         {total_window_area:10.2f} m2")
print(f"  Total Ground Floor Area:   {total_ground_area:10.2f} m2")
print(f"  Window-Wall Ratio:         {total_window_area/total_ext_wall_area*100:10.2f} %")

# Print breakdown of exterior walls by zone
print("\n  Exterior Wall Area by Zone (unmultiplied):")
for name, zone, area, mult in sorted(ext_walls, key=lambda x: x[1]):
    print(f"    {name:40s}  zone={zone:18s}  area={area:8.2f} m2  mult={mult}")

print(f"\n  Windows by Zone (unmultiplied):")
for name, zone, area, mult in sorted(windows, key=lambda x: x[1]):
    print(f"    {name:40s}  zone={zone:18s}  area={area:8.2f} m2  mult={mult}")

print(f"\n  Roof surfaces (unmultiplied):")
for name, zone, area, mult in sorted(ext_roofs, key=lambda x: x[1]):
    print(f"    {name:40s}  zone={zone:18s}  area={area:8.2f} m2  mult={mult}")

print(f"\n  Ground Floor surfaces (unmultiplied):")
for name, zone, area, mult in sorted(ground_floors, key=lambda x: x[1]):
    print(f"    {name:40s}  zone={zone:18s}  area={area:8.2f} m2  mult={mult}")

# ─────────────────────────────────────────────────────────────────────────────
# E+ Reference Values (from eplustbl.csv)
# ─────────────────────────────────────────────────────────────────────────────

print("\n--- EnergyPlus Surface Areas (from eplustbl.csv) ---\n")
print("  From Window-Wall Ratio table (all zones):")
print(f"    Gross Wall Area:          1542.04 m2")
print(f"    Window Opening Area:       306.92 m2")
print(f"    Gross Window-Wall Ratio:    19.90 %")
print()
print("  From Conditioned Window-Wall Ratio table:")
print(f"    Gross Wall Area:          1501.17 m2")
print(f"    Window Opening Area:       300.26 m2")
print(f"    Gross Window-Wall Ratio:    20.00 %")
print()
print("  From Skylight-Roof Ratio table:")
print(f"    Gross Roof Area:           783.65 m2")
print()
print("  From Building Area Summary:")
print(f"    Total Building Area:      3134.61 m2")
print(f"    Net Conditioned Area:     2823.98 m2")
print(f"    Unconditioned Area:        310.64 m2")

# E+ zone-level wall/window data
print("\n  E+ Zone-Level Geometry (from zone information table):")
print(f"    {'Zone':<25s} {'Mult':>4s} {'ExtWall':>10s} {'NetWall':>10s} {'Window':>10s}")
ep_zones = [
    ("G SW APARTMENT", 1, 58.52, 46.82, 11.71),
    ("G NW APARTMENT", 1, 58.52, 46.82, 11.71),
    ("OFFICE", 1, 58.52, 46.82, 11.71),
    ("G NE APARTMENT", 1, 58.52, 46.82, 11.71),
    ("G N1 APARTMENT", 1, 35.30, 28.24, 7.06),
    ("G N2 APARTMENT", 1, 35.30, 28.24, 7.06),
    ("G S1 APARTMENT", 1, 35.30, 28.24, 7.06),
    ("G S2 APARTMENT", 1, 35.30, 28.24, 7.06),
    ("M SW APARTMENT", 2, 58.52, 46.82, 11.71),
    ("M NW APARTMENT", 2, 58.52, 46.82, 11.71),
    ("M SE APARTMENT", 2, 58.52, 46.82, 11.71),
    ("M NE APARTMENT", 2, 58.52, 46.82, 11.71),
    ("M N1 APARTMENT", 2, 35.30, 28.24, 7.06),
    ("M N2 APARTMENT", 2, 35.30, 28.24, 7.06),
    ("M S1 APARTMENT", 2, 35.30, 28.24, 7.06),
    ("M S2 APARTMENT", 2, 35.30, 28.24, 7.06),
    ("T SW APARTMENT", 1, 58.52, 46.82, 11.71),
    ("T NW APARTMENT", 1, 58.52, 46.82, 11.71),
    ("T SE APARTMENT", 1, 58.52, 46.82, 11.71),
    ("T NE APARTMENT", 1, 58.52, 46.82, 11.71),
    ("T N1 APARTMENT", 1, 35.30, 28.24, 7.06),
    ("T N2 APARTMENT", 1, 35.30, 28.24, 7.06),
    ("T S1 APARTMENT", 1, 35.30, 28.24, 7.06),
    ("T S2 APARTMENT", 1, 35.30, 28.24, 7.06),
    ("T CORRIDOR", 1, 10.22, 9.10, 1.11),
    ("G CORRIDOR", 1, 10.22, 6.90, 3.32),
    ("M CORRIDOR", 2, 10.22, 9.10, 1.11),
]
ep_total_ext_wall = 0
ep_total_window = 0
for zn, m, ew, nw, win in ep_zones:
    print(f"    {zn:<25s} {m:>4d} {ew:>10.2f} {nw:>10.2f} {win:>10.2f}")
    ep_total_ext_wall += ew * m
    ep_total_window += win * m
print(f"    {'TOTAL (with multipliers)':<25s}      {ep_total_ext_wall:>10.2f} {'':>10s} {ep_total_window:>10.2f}")

# ─────────────────────────────────────────────────────────────────────────────
# COMPARISON
# ─────────────────────────────────────────────────────────────────────────────

print("\n--- Surface Area Comparison (OpenBSE vs E+) ---\n")
print(f"  {'Metric':<30s} {'OpenBSE':>12s} {'E+':>12s} {'Diff':>12s} {'Ratio':>8s}")
print(f"  {'-'*30} {'-'*12} {'-'*12} {'-'*12} {'-'*8}")

def compare(label, obse, ep):
    diff = obse - ep
    ratio = obse / ep if ep != 0 else float('inf')
    print(f"  {label:<30s} {obse:>12.2f} {ep:>12.2f} {diff:>+12.2f} {ratio:>8.3f}")
    return diff, ratio

compare("Exterior Wall Area [m2]", total_ext_wall_area, 1542.04)
compare("Window Area [m2]", total_window_area, 306.92)
compare("Roof Area [m2]", total_roof_area, 783.65)
compare("Ground Floor Area [m2]", total_ground_area, 783.66)
compare("Window-Wall Ratio [%]", total_window_area/total_ext_wall_area*100, 19.90)


# ─────────────────────────────────────────────────────────────────────────────
# PART 2: ZONE-LEVEL HEATING / COOLING from E+ and OpenBSE
# ─────────────────────────────────────────────────────────────────────────────

print("\n\n" + "=" * 80)
print("PART 2: ZONE-LEVEL ANNUAL HEATING/COOLING COMPARISON")
print("=" * 80)

# E+ data from Sensible Heat Gain Summary (annual HVAC heating/cooling per zone) [GJ]
# NOTE: These values from E+ Sensible Heat Gain Summary ALREADY include zone multipliers.
ep_annual = {
    "G SW APARTMENT":  (1.771,  9.953, 1),
    "G NW APARTMENT":  (5.886,  5.739, 1),
    "OFFICE":          (6.620,  9.057, 1),
    "G NE APARTMENT":  (6.086,  5.548, 1),
    "G N1 APARTMENT":  (2.229,  6.035, 1),
    "G N2 APARTMENT":  (2.245,  6.023, 1),
    "G S1 APARTMENT":  (0.344, 12.064, 1),
    "G S2 APARTMENT":  (0.334, 13.012, 1),
    "M SW APARTMENT":  (0.818, 34.352, 2),
    "M NW APARTMENT":  (4.214, 21.231, 2),
    "M SE APARTMENT":  (0.767, 33.661, 2),
    "M NE APARTMENT":  (4.301, 20.632, 2),
    "M N1 APARTMENT":  (1.010, 21.268, 2),
    "M N2 APARTMENT":  (1.011, 21.218, 2),
    "M S1 APARTMENT":  (0.378, 36.105, 2),
    "M S2 APARTMENT":  (0.373, 36.038, 2),
    "T SW APARTMENT":  (8.756, 11.913, 1),
    "T NW APARTMENT": (13.633,  8.071, 1),
    "T SE APARTMENT":  (8.724, 11.348, 1),
    "T NE APARTMENT": (13.662,  7.638, 1),
    "T N1 APARTMENT":  (9.710,  6.492, 1),
    "T N2 APARTMENT":  (9.746,  6.503, 1),
    "T S1 APARTMENT":  (5.317, 10.574, 1),
    "T S2 APARTMENT":  (5.345, 10.590, 1),
    "T CORRIDOR":      (0.000,  0.000, 1),
    "G CORRIDOR":      (0.000,  0.000, 1),
    "M CORRIDOR":      (0.000,  0.000, 2),
}

ep_total_htg = sum(v[0] for v in ep_annual.values())
ep_total_clg = sum(v[1] for v in ep_annual.values())

print(f"\n--- E+ Annual Zone HVAC Sensible Heating/Cooling (from Sensible Heat Gain Summary) ---")
print(f"    NOTE: Values already include zone multipliers.\n")
print(f"  {'Zone':<25s} {'Htg [GJ]':>10s} {'Clg [GJ]':>10s} {'Htg [kWh]':>12s} {'Clg [kWh]':>12s} {'Mult':>5s}")
print(f"  {'-'*25} {'-'*10} {'-'*10} {'-'*12} {'-'*12} {'-'*5}")
for zn in ep_annual:
    h, c, m = ep_annual[zn]
    print(f"  {zn:<25s} {h:>10.3f} {c:>10.3f} {h*1e9/3.6e6:>12.1f} {c*1e9/3.6e6:>12.1f} {m:>5d}")
print(f"  {'-'*25} {'-'*10} {'-'*10} {'-'*12} {'-'*12}")
print(f"  {'TOTAL':<25s} {ep_total_htg:>10.3f} {ep_total_clg:>10.3f} {ep_total_htg*1e9/3.6e6:>12.1f} {ep_total_clg*1e9/3.6e6:>12.1f}")

print(f"\n  E+ Total HVAC Zone Heating: {ep_total_htg:.3f} GJ = {ep_total_htg*1e9/3.6e6:.1f} kWh")
print(f"  E+ Total HVAC Zone Cooling: {ep_total_clg:.3f} GJ = {ep_total_clg*1e9/3.6e6:.1f} kWh")

print(f"\n  E+ End Use table (site energy, includes system effects + boiler eff):")
print(f"    Heating end use: Natural Gas = 253.01 GJ = {253.01e9/3.6e6:.0f} kWh (thermal input)")
print(f"    Cooling end use: Electricity = 107.58 GJ = {107.58e9/3.6e6:.0f} kWh (compressor elec)")

# ─────────────────────────────────────────────────────────────────────────────
# OpenBSE zone-level loads from zone_results.csv
# ─────────────────────────────────────────────────────────────────────────────

print(f"\n--- OpenBSE Annual Zone Heating/Cooling (from zone_results.csv) ---\n")

with open(ZONE_RESULTS, 'r') as f:
    reader = csv.reader(f)
    header = next(reader)
    
    htg_cols = {}
    clg_cols = {}
    for i, col in enumerate(header):
        if 'zone_heating_rate' in col:
            zone_name = col.split(':')[0]
            htg_cols[zone_name] = i
        if 'zone_cooling_rate' in col:
            zone_name = col.split(':')[0]
            clg_cols[zone_name] = i
    
    obse_htg = {z: 0.0 for z in htg_cols}
    obse_clg = {z: 0.0 for z in clg_cols}
    
    dt = 3600.0  # 1 hour timestep (hourly output)
    for row in reader:
        for zone, col_idx in htg_cols.items():
            val = float(row[col_idx])
            obse_htg[zone] += val * dt  # W * s = J
        for zone, col_idx in clg_cols.items():
            val = float(row[col_idx])
            obse_clg[zone] += val * dt

# Apply multipliers from YAML zones
yaml_zone_mult = {}
for z in zones:
    yaml_zone_mult[z['name']] = z.get('multiplier', 1)

print(f"  {'Zone':<25s} {'Htg [kWh]':>12s} {'Clg [kWh]':>12s} {'Mult':>5s} {'Htg*M [kWh]':>14s} {'Clg*M [kWh]':>14s}")
print(f"  {'-'*25} {'-'*12} {'-'*12} {'-'*5} {'-'*14} {'-'*14}")

obse_total_htg = 0.0
obse_total_clg = 0.0

for zone in sorted(obse_htg.keys()):
    h_kwh = obse_htg[zone] / 3.6e6
    c_kwh = obse_clg[zone] / 3.6e6
    m = yaml_zone_mult.get(zone, 1)
    obse_total_htg += h_kwh * m
    obse_total_clg += c_kwh * m
    print(f"  {zone:<25s} {h_kwh:>12.1f} {c_kwh:>12.1f} {m:>5d} {h_kwh*m:>14.1f} {c_kwh*m:>14.1f}")

print(f"  {'-'*25} {'-'*12} {'-'*12} {'-'*5} {'-'*14} {'-'*14}")
print(f"  {'TOTAL (with multipliers)':<25s} {'':>12s} {'':>12s} {'':>5s} {obse_total_htg:>14.1f} {obse_total_clg:>14.1f}")

print(f"\n  OpenBSE Total Zone Heating: {obse_total_htg:.1f} kWh ({obse_total_htg*3.6e6/1e9:.3f} GJ)")
print(f"  OpenBSE Total Zone Cooling: {obse_total_clg:.1f} kWh ({obse_total_clg*3.6e6/1e9:.3f} GJ)")

# ─────────────────────────────────────────────────────────────────────────────
# ZONE-LEVEL COMPARISON
# ─────────────────────────────────────────────────────────────────────────────

print(f"\n--- Zone-Level Heating/Cooling Comparison ---\n")

name_map = {
    "G SW Apt": "G SW APARTMENT",
    "G NW Apt": "G NW APARTMENT",
    "Office": "OFFICE",
    "G NE Apt": "G NE APARTMENT",
    "G N1 Apt": "G N1 APARTMENT",
    "G N2 Apt": "G N2 APARTMENT",
    "G S1 Apt": "G S1 APARTMENT",
    "G S2 Apt": "G S2 APARTMENT",
    "M SW Apt": "M SW APARTMENT",
    "M NW Apt": "M NW APARTMENT",
    "M SE Apt": "M SE APARTMENT",
    "M NE Apt": "M NE APARTMENT",
    "M N1 Apt": "M N1 APARTMENT",
    "M N2 Apt": "M N2 APARTMENT",
    "M S1 Apt": "M S1 APARTMENT",
    "M S2 Apt": "M S2 APARTMENT",
    "T SW Apt": "T SW APARTMENT",
    "T NW Apt": "T NW APARTMENT",
    "T SE Apt": "T SE APARTMENT",
    "T NE Apt": "T NE APARTMENT",
    "T N1 Apt": "T N1 APARTMENT",
    "T N2 Apt": "T N2 APARTMENT",
    "T S1 Apt": "T S1 APARTMENT",
    "T S2 Apt": "T S2 APARTMENT",
    "G Corridor": "G CORRIDOR",
    "M Corridor": "M CORRIDOR",
    "T Corridor": "T CORRIDOR",
}

print(f"  HEATING comparison (kWh, with multipliers applied):")
print(f"  {'Zone':<20s} {'OpenBSE':>10s} {'E+':>10s} {'Ratio':>8s} {'Diff':>10s}")
print(f"  {'-'*20} {'-'*10} {'-'*10} {'-'*8} {'-'*10}")

for obse_z in sorted(name_map.keys()):
    ep_z = name_map[obse_z]
    m = yaml_zone_mult.get(obse_z, 1)
    ob_h = obse_htg[obse_z] / 3.6e6 * m
    ep_h = ep_annual[ep_z][0] * 1e9 / 3.6e6
    ratio = ob_h / ep_h if ep_h > 0 else float('inf')
    print(f"  {obse_z:<20s} {ob_h:>10.1f} {ep_h:>10.1f} {ratio:>8.2f} {ob_h-ep_h:>+10.1f}")

print(f"\n  COOLING comparison (kWh, with multipliers applied):")
print(f"  {'Zone':<20s} {'OpenBSE':>10s} {'E+':>10s} {'Ratio':>8s} {'Diff':>10s}")
print(f"  {'-'*20} {'-'*10} {'-'*10} {'-'*8} {'-'*10}")

for obse_z in sorted(name_map.keys()):
    ep_z = name_map[obse_z]
    m = yaml_zone_mult.get(obse_z, 1)
    ob_c = obse_clg[obse_z] / 3.6e6 * m
    ep_c = ep_annual[ep_z][1] * 1e9 / 3.6e6
    ratio = ob_c / ep_c if ep_c > 0 else float('inf')
    print(f"  {obse_z:<20s} {ob_c:>10.1f} {ep_c:>10.1f} {ratio:>8.2f} {ob_c-ep_c:>+10.1f}")

# ─────────────────────────────────────────────────────────────────────────────
# OVERALL SUMMARY
# ─────────────────────────────────────────────────────────────────────────────

ep_htg_kwh = ep_total_htg * 1e9 / 3.6e6
ep_clg_kwh = ep_total_clg * 1e9 / 3.6e6

print(f"\n\n{'='*80}")
print("OVERALL SUMMARY")
print(f"{'='*80}")
print(f"\n  Zone Heating Loads:")
print(f"    OpenBSE:  {obse_total_htg:>10.1f} kWh")
print(f"    E+:       {ep_htg_kwh:>10.1f} kWh")
print(f"    Ratio:    {obse_total_htg/ep_htg_kwh:>10.2f}x")
print(f"\n  Zone Cooling Loads:")
print(f"    OpenBSE:  {obse_total_clg:>10.1f} kWh")
print(f"    E+:       {ep_clg_kwh:>10.1f} kWh")
print(f"    Ratio:    {obse_total_clg/ep_clg_kwh:>10.2f}x")
print(f"\n  Zone Total (Htg+Clg):")
print(f"    OpenBSE:  {obse_total_htg+obse_total_clg:>10.1f} kWh")
print(f"    E+:       {ep_htg_kwh+ep_clg_kwh:>10.1f} kWh")
print(f"    Ratio:    {(obse_total_htg+obse_total_clg)/(ep_htg_kwh+ep_clg_kwh):>10.2f}x")

print(f"\n  OpenBSE Summary Report says:")
print(f"    Heating: 100114.2 kWh  |  Cooling: 148933.6 kWh  |  Total: 249047.9 kWh")
print(f"\n  E+ End Use (site energy, system-level):")
print(f"    Gas Heating: 253.01 GJ = {253.01e9/3.6e6:.0f} kWh (gas input, ~75% boiler eff -> {253.01e9*0.75/3.6e6:.0f} kWh thermal)")
print(f"    Elec Cooling: 107.58 GJ = {107.58e9/3.6e6:.0f} kWh (compressor, COP~3.2 -> {107.58e9*3.2/3.6e6:.0f} kWh thermal removed)")

# ─────────────────────────────────────────────────────────────────────────────
# Root cause analysis
# ─────────────────────────────────────────────────────────────────────────────

print(f"\n\n{'='*80}")
print("ZONE-LEVEL ROOT CAUSE ANALYSIS")
print(f"{'='*80}")

print(f"\n  Zones sorted by EXCESS HEATING (OpenBSE - E+):")
diffs = []
for obse_z in sorted(name_map.keys()):
    ep_z = name_map[obse_z]
    m = yaml_zone_mult.get(obse_z, 1)
    ob_h = obse_htg[obse_z] / 3.6e6 * m
    ep_h = ep_annual[ep_z][0] * 1e9 / 3.6e6
    diffs.append((obse_z, ob_h, ep_h, ob_h - ep_h))

diffs.sort(key=lambda x: -x[3])
print(f"  {'Zone':<20s} {'OpenBSE':>10s} {'E+':>10s} {'Excess':>10s}")
for z, ob, ep, d in diffs:
    print(f"  {z:<20s} {ob:>10.1f} {ep:>10.1f} {d:>+10.1f}")

print(f"\n  Zones sorted by EXCESS COOLING (OpenBSE - E+):")
diffs_c = []
for obse_z in sorted(name_map.keys()):
    ep_z = name_map[obse_z]
    m = yaml_zone_mult.get(obse_z, 1)
    ob_c = obse_clg[obse_z] / 3.6e6 * m
    ep_c = ep_annual[ep_z][1] * 1e9 / 3.6e6
    diffs_c.append((obse_z, ob_c, ep_c, ob_c - ep_c))

diffs_c.sort(key=lambda x: -x[3])
print(f"  {'Zone':<20s} {'OpenBSE':>10s} {'E+':>10s} {'Excess':>10s}")
for z, ob, ep, d in diffs_c:
    print(f"  {z:<20s} {ob:>10.1f} {ep:>10.1f} {d:>+10.1f}")

# ─────────────────────────────────────────────────────────────────────────────
# Floor-specific breakdown
# ─────────────────────────────────────────────────────────────────────────────

print(f"\n\n--- Floor-Level Summary ---\n")
floors = {"Ground": [], "Mid": [], "Top": []}
for obse_z in name_map:
    if obse_z.startswith("G "): floors["Ground"].append(obse_z)
    elif obse_z.startswith("M "): floors["Mid"].append(obse_z)
    elif obse_z.startswith("T "): floors["Top"].append(obse_z)

for floor_name in ["Ground", "Mid", "Top"]:
    ob_h = sum(obse_htg[z] / 3.6e6 * yaml_zone_mult.get(z, 1) for z in floors[floor_name])
    ob_c = sum(obse_clg[z] / 3.6e6 * yaml_zone_mult.get(z, 1) for z in floors[floor_name])
    ep_h = sum(ep_annual[name_map[z]][0] * 1e9 / 3.6e6 for z in floors[floor_name])
    ep_c = sum(ep_annual[name_map[z]][1] * 1e9 / 3.6e6 for z in floors[floor_name])
    h_ratio = ob_h / ep_h if ep_h > 0 else float('inf')
    c_ratio = ob_c / ep_c if ep_c > 0 else float('inf')
    print(f"  {floor_name:<8s} Heating: OpenBSE={ob_h:>8.0f}  E+={ep_h:>8.0f}  ratio={h_ratio:.2f}x")
    print(f"  {'':<8s} Cooling: OpenBSE={ob_c:>8.0f}  E+={ep_c:>8.0f}  ratio={c_ratio:.2f}x")
    print()

print("Done.")
