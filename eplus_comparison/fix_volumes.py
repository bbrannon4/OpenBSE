#!/usr/bin/env python3
"""Revert incorrect volume scaling. Original volumes were already correct."""
import re

YAML_PATH = "/Users/benjaminbrannon/Documents/GitHub/OpenBSE/eplus_comparison/LargeOffice_Boulder.yaml"

# Original volumes (before the incorrect fix_wall_heights.py scaling)
original_volumes = {
    "Core_bottom": 6849.4,
    "Core_mid": 6849.4,
    "Core_top": 6849.4,
    "Perimeter_bot_ZN_1": 860.0,
    "Perimeter_bot_ZN_2": 554.2,
    "Perimeter_bot_ZN_3": 860.0,
    "Perimeter_bot_ZN_4": 554.2,
    "Perimeter_mid_ZN_1": 860.0,
    "Perimeter_mid_ZN_2": 554.2,
    "Perimeter_mid_ZN_3": 860.0,
    "Perimeter_mid_ZN_4": 554.2,
    "Perimeter_top_ZN_1": 860.0,
    "Perimeter_top_ZN_2": 554.2,
    "Perimeter_top_ZN_3": 860.0,
    "Perimeter_top_ZN_4": 554.2,
}

with open(YAML_PATH, "r") as f:
    lines = f.readlines()

changes = 0
for i, line in enumerate(lines):
    if "volume:" not in line:
        continue
    m = re.match(r'(\s*volume:\s*)([\d.]+)', line)
    if not m:
        continue
    # Find which zone this belongs to
    for j in range(i-1, max(0, i-20), -1):
        zm = re.match(r'\s*- name:\s*(.+)', lines[j])
        if zm:
            zname = zm.group(1).strip()
            if zname in original_volumes:
                old_vol = float(m.group(2))
                new_vol = original_volumes[zname]
                if abs(old_vol - new_vol) > 0.1:
                    lines[i] = f"{m.group(1)}{new_vol}\n"
                    print(f"  {zname}: {old_vol} → {new_vol}")
                    changes += 1
            break

with open(YAML_PATH, "w") as f:
    f.writelines(lines)
print(f"\nReverted {changes} volume changes")
