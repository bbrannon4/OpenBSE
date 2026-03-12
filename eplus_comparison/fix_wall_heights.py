#!/usr/bin/env python3
"""
Reduce exterior wall heights from floor-to-floor (3.9624m) to occupied zone
height (2.744m) to exclude the plenum portion. E+ models plenums as separate
zones; OpenBSE doesn't have plenums, so we reduce the wall to match E+'s
occupied zone.

The plenum's thermal impact (wall conduction + infiltration offset by return
air heating) approximately nets out, so excluding it is a reasonable
simplification.
"""

import re, sys

YAML_PATH = "/Users/benjaminbrannon/Documents/GitHub/OpenBSE/eplus_comparison/LargeOffice_Boulder.yaml"

# Floor-to-floor height and occupied zone height
FTF = 3.9624  # floor-to-floor [m]
OCC = 2.744   # E+ occupied zone height [m]
PLENUM = FTF - OCC  # 1.2184m

# Wall z_top values to change (old → new)
# Bottom floor: z_top = 0 + FTF = 3.9624 → 0 + OCC = 2.744
# Mid floor: z_top = FTF + FTF = 7.9248 → FTF + OCC = 6.7064
# Top floor: z_top = 43.5864 + FTF = 47.5488 → 43.5864 + OCC = 46.3304
z_replacements = {
    3.9624: 2.744,
    7.9248: 6.7064,
    47.5488: 46.3304,
}

# Exterior wall surface names (from the YAML — only perimeter zone exterior walls)
exterior_walls = {
    "P_bot_ZN1 South Wall", "P_bot_ZN2 East Wall",
    "P_bot_ZN3 North Wall", "P_bot_ZN4 West Wall",
    "P_mid_ZN1 South Wall", "P_mid_ZN2 East Wall",
    "P_mid_ZN3 North Wall", "P_mid_ZN4 West Wall",
    "P_top_ZN1 South Wall", "P_top_ZN2 East Wall",
    "P_top_ZN3 North Wall", "P_top_ZN4 West Wall",
}

# Also need to handle basement exterior walls — but those are below grade
# and don't have the plenum issue, so skip them.

with open(YAML_PATH, "r") as f:
    lines = f.readlines()

in_target_surface = False
vertex_count = 0
changes = 0

for i, line in enumerate(lines):
    # Detect surface name
    m = re.match(r'\s*- name:\s*(.+)', line)
    if m:
        sname = m.group(1).strip()
        in_target_surface = sname in exterior_walls
        vertex_count = 0
        continue

    # If we're in a target surface and hit "vertices:", start counting
    if in_target_surface and "vertices:" in line:
        vertex_count = 0
        continue

    # Count vertex lines and replace z in 3rd and 4th vertices (top of wall)
    if in_target_surface and re.match(r'\s*- \{x:', line):
        vertex_count += 1
        if vertex_count in (3, 4):  # top vertices
            for old_z, new_z in z_replacements.items():
                old_str = f"z: {old_z}"
                new_str = f"z: {new_z}"
                if old_str in line:
                    lines[i] = line.replace(old_str, new_str)
                    changes += 1
                    break

# Also update zone volumes for perimeter zones
# Volume = floor_area × occupied_height instead of floor_area × FTF
# Ratio = OCC / FTF = 2.744 / 3.9624 = 0.6924
volume_ratio = OCC / FTF

# Zone volumes to adjust
zone_volumes = {}  # We need to find and scale these
for i, line in enumerate(lines):
    # Look for volume: entries after zone definitions
    if "volume:" in line and "zone" not in line.lower():
        m = re.match(r'(\s*volume:\s*)([\d.]+)', line)
        if m:
            # Check which zone this belongs to by looking back
            for j in range(i-1, max(0, i-20), -1):
                zm = re.match(r'\s*- name:\s*(.+)', lines[j])
                if zm:
                    zname = zm.group(1).strip()
                    # Only adjust perimeter and core zones (not basement, not datacenter)
                    if any(p in zname for p in ["Perimeter_bot_", "Perimeter_mid_", "Perimeter_top_",
                                                  "Core_bottom", "Core_mid", "Core_top"]):
                        old_vol = float(m.group(2))
                        new_vol = round(old_vol * volume_ratio, 1)
                        lines[i] = f"{m.group(1)}{new_vol}\n"
                        zone_volumes[zname] = (old_vol, new_vol)
                        changes += 1
                    break

with open(YAML_PATH, "w") as f:
    f.writelines(lines)

print(f"Made {changes} changes to {YAML_PATH}")
print(f"\nWall height changes (z_top):")
for old_z, new_z in z_replacements.items():
    print(f"  {old_z} → {new_z}")
print(f"\nVolume changes (ratio {volume_ratio:.4f}):")
for zname, (old, new) in sorted(zone_volumes.items()):
    print(f"  {zname}: {old} → {new} m³")
