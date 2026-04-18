#!/usr/bin/env python3
"""
compare_zone_balance.py — Zone air balance component comparison: OB vs E+

Compares the individual terms of the zone air energy balance for the living zone:
  Surface convection (total, walls, floors, roofs, windows) — kWh/year
  Infiltration sensible heat transfer
  Thermal mass term (OB only)
  Internal convective gains

All energy values in kWh, positive = heat INTO zone air.
"""

import csv
from pathlib import Path
from collections import defaultdict

EP_CSV  = Path("eplus_ideal_run/eplusout.csv")
OB_CSV  = Path("SingleFamily_CZ5B_Boulder_ideal_results.csv")
EP_DT_S = 600  # E+ 10-min timesteps
OB_DT_S = 600  # OB 10-min timesteps


# ── I/O helpers ───────────────────────────────────────────────────────────────

def load_ep(path):
    """Load E+ CSV rows, skipping warmup (rows before first 1/1 timestamp)."""
    with open(path) as f:
        reader = csv.reader(f)
        header = next(reader)
        rows = []
        found_jan1 = False
        for row in reader:
            ts = row[0].strip()
            if not ts:
                continue
            md = ts.split()[0].strip()
            parts = md.split("/")
            if not found_jan1:
                if int(parts[0]) == 1 and int(parts[1]) == 1:
                    found_jan1 = True
                else:
                    continue
            rows.append(row)
    return header, rows


def load_ob(path):
    with open(path) as f:
        reader = csv.reader(f)
        header = next(reader)
        rows = [r for r in reader if r[0].strip()]
    return header, rows


def ep_sum_kwh(rows, col_idx, is_energy_j=False):
    """Sum one E+ column → kWh. is_energy_j=True for [J] columns."""
    if col_idx is None:
        return None
    total = sum(float(r[col_idx]) for r in rows)
    return total / 3_600_000.0 if is_energy_j else total * EP_DT_S / 3_600_000.0


def ob_sum_kwh(rows, col_idx):
    if col_idx is None:
        return None
    return sum(float(r[col_idx]) for r in rows) * OB_DT_S / 3_600_000.0


def ep_find(header, substring):
    """First column index containing substring (case-insensitive)."""
    sub = substring.lower()
    for i, h in enumerate(header):
        if sub in h.lower():
            return i
    return None


def ob_find(header, exact):
    """Exact column match (strip whitespace)."""
    for i, h in enumerate(header):
        if h.strip().lower() == exact.lower():
            return i
    return None


def fmt_row(label, ep_val, ob_val, width=32):
    ep_s = f"{ep_val:10.1f}" if ep_val is not None else "       N/A"
    ob_s = f"{ob_val:10.1f}" if ob_val is not None else "       N/A"
    if ep_val is not None and ob_val is not None:
        d = ob_val - ep_val
        p = (d / abs(ep_val) * 100) if ep_val != 0 else float("inf")
        d_s = f"{d:+10.1f}"
        p_s = f"({p:+.1f}%)"
    else:
        d_s, p_s = "       N/A", ""
    print(f"  {label:{width}s}  E+:{ep_s}  OB:{ob_s}  Δ:{d_s}  {p_s}")


# ── load ─────────────────────────────────────────────────────────────────────

ep_h, ep_rows = load_ep(EP_CSV)
ob_h, ob_rows = load_ob(OB_CSV)
print(f"E+ rows (annual, warmup excluded): {len(ep_rows)}")
print(f"OB rows:                           {len(ob_rows)}\n")


# ── helper: find all E+ living-zone surface convection columns ────────────────

def ep_living_conv_cols(header):
    """
    Return list of (col_idx, surface_name, category) for
    'Surface Inside Face Convection Heat Gain Rate' in living zone.
    Category: walls | floors | roofs | windows | internal_mass
    """
    results = []
    for i, h in enumerate(header):
        h_up = h.upper()
        if "INSIDE FACE CONVECTION HEAT GAIN RATE" not in h_up:
            continue
        surf = h.split(":")[0].upper().strip()
        # Must be in UNIT1, not other zones
        if "UNIT1" not in surf:
            continue
        if "GARAGE" in surf or "ATTIC" in surf or "BSMT" in surf:
            continue
        # Categorise
        if "WINDOW" in surf:
            cat = "windows"
        elif "FLOOR" in surf:
            cat = "floors"
        elif "CEILING" in surf or "ROOF" in surf:
            cat = "roofs"
        elif "INTERNALMASS" in surf:
            cat = "internal_mass"
        else:
            cat = "walls"  # walls, doors
        results.append((i, surf, cat))
    return results


# ══════════════════════════════════════════════════════════════════════════════
print("═" * 76)
print("1. SURFACE CONVECTION TO ZONE AIR — annual kWh  (positive=heat into air)")
print("═" * 76)

ep_cols_info = ep_living_conv_cols(ep_h)
ep_by_cat = defaultdict(float)
ep_grand = 0.0
for ci, sname, cat in ep_cols_info:
    v = ep_sum_kwh(ep_rows, ci)
    ep_by_cat[cat] += v
    ep_grand += v

ob_total  = ob_sum_kwh(ob_rows, ob_find(ob_h, "living_unit1:q_surf_conv_total [-]"))
ob_walls  = ob_sum_kwh(ob_rows, ob_find(ob_h, "living_unit1:q_surf_conv_walls [-]"))
ob_floors = ob_sum_kwh(ob_rows, ob_find(ob_h, "living_unit1:q_surf_conv_floors [-]"))
ob_roofs  = ob_sum_kwh(ob_rows, ob_find(ob_h, "living_unit1:q_surf_conv_roofs [-]"))
ob_wins   = ob_sum_kwh(ob_rows, ob_find(ob_h, "living_unit1:q_surf_conv_windows [-]"))

# OB doesn't have an "internal mass" category; it's included in walls
ep_walls_plus_mass = ep_by_cat["walls"] + ep_by_cat["internal_mass"]

print()
fmt_row("Total (all surfaces)",        ep_grand,                     ob_total)
fmt_row("  Walls+Doors",               ep_by_cat["walls"],           ob_walls)
fmt_row("  Internal mass (E+ only)",   ep_by_cat["internal_mass"],   None)
fmt_row("  Walls+Doors+IntMass",       ep_walls_plus_mass,            ob_walls)
fmt_row("  Floors",                    ep_by_cat["floors"],          ob_floors)
fmt_row("  Roofs+Ceilings",            ep_by_cat["roofs"],           ob_roofs)
fmt_row("  Windows",                   ep_by_cat["windows"],         ob_wins)
print()
print(f"  E+ surfaces found: {len(ep_cols_info)}")
for ci, sname, cat in sorted(ep_cols_info, key=lambda x: x[2]):
    v = ep_sum_kwh(ep_rows, ci)
    print(f"    [{cat:14s}]  {sname:40s}  {v:+8.1f} kWh")


# ══════════════════════════════════════════════════════════════════════════════
print()
print("═" * 76)
print("2. INFILTRATION SENSIBLE HEAT TRANSFER")
print("═" * 76)
print()

ep_loss_ci = ep_find(ep_h, "LIVING_UNIT1:Zone Infiltration Sensible Heat Loss Energy")
ep_gain_ci = ep_find(ep_h, "LIVING_UNIT1:Zone Infiltration Sensible Heat Gain Energy")
ep_loss = ep_sum_kwh(ep_rows, ep_loss_ci, is_energy_j=True) if ep_loss_ci else None
ep_gain = ep_sum_kwh(ep_rows, ep_gain_ci, is_energy_j=True) if ep_gain_ci else None
ep_infil_net = (ep_gain - ep_loss) if (ep_gain and ep_loss) else None

ob_infil = ob_sum_kwh(ob_rows, ob_find(ob_h, "living_unit1:q_infiltration_sensible [-]"))

if ep_loss and ep_gain:
    print(f"  E+  loss: {ep_loss:8.1f} kWh   gain: {ep_gain:8.1f} kWh")
fmt_row("Net sensible (gain−loss)", ep_infil_net, ob_infil)
print()
print("  Note: OB q_infiltration_sensible = m_dot_total × cp × (T_outdoor − T_zone),")
print("  which includes infiltration + ventilation + nat_vent combined.")
print("  E+ Zone Infiltration figures are infiltration only.")


# ══════════════════════════════════════════════════════════════════════════════
print()
print("═" * 76)
print("3. THERMAL MASS (storage) TERM")
print("═" * 76)
print()

ob_mass = ob_sum_kwh(ob_rows, ob_find(ob_h, "living_unit1:q_thermal_mass [-]"))
print(f"  OB annual sum: {ob_mass:.1f} kWh  (should be ≈0 for complete year)")
print("  E+ equivalent not directly reported.")


# ══════════════════════════════════════════════════════════════════════════════
print()
print("═" * 76)
print("4. INTERNAL CONVECTIVE GAINS")
print("═" * 76)
print()

# E+ reports all sensible gains together (people + lights + equip).
# These are total sensible; E+ splits conv/rad internally but only outputs totals.
ep_ppl   = ep_sum_kwh(ep_rows, ep_find(ep_h, "LIVING_UNIT1:Zone People Sensible Heating Rate"))
ep_light = ep_sum_kwh(ep_rows, ep_find(ep_h, "LIVING_UNIT1:Zone Lights Total Heating Rate"))
ep_equip = ep_sum_kwh(ep_rows, ep_find(ep_h, "LIVING_UNIT1:Zone Electric Equipment Total Heating Rate"))
ep_int_total = sum(v for v in [ep_ppl, ep_light, ep_equip] if v is not None)

ob_int = ob_sum_kwh(ob_rows, ob_find(ob_h, "living_unit1:q_internal_conv [-]"))
ob_rad = ob_sum_kwh(ob_rows, ob_find(ob_h, "living_unit1:q_internal_rad [-]"))
ob_int_all = (ob_int or 0) + (ob_rad or 0)

print(f"  E+ (total sensible, conv+rad combined):")
print(f"    people: {ep_ppl:.1f}  lights: {ep_light:.1f}  equip: {ep_equip:.1f}  total: {ep_int_total:.1f} kWh")
print(f"  OB (split conv/rad):")
print(f"    q_internal_conv: {ob_int:.1f}   q_internal_rad: {ob_rad:.1f}   total: {ob_int_all:.1f} kWh")
print(f"  → Only convective portion enters zone air; radiative warms surfaces.")
print(f"  → Convective fraction ~50% for people, ~59% lights, ~70% equip (E+ defaults).")


# ══════════════════════════════════════════════════════════════════════════════
print()
print("═" * 76)
print("5. ZONE AIR BALANCE CLOSURE (OB)")
print("═" * 76)
print()

ob_heat = ob_sum_kwh(ob_rows, ob_find(ob_h, "living_unit1:heating_load [-]"))
ob_cool = ob_sum_kwh(ob_rows, ob_find(ob_h, "living_unit1:cooling_load [-]"))
ob_solar_trans = ob_sum_kwh(ob_rows, ob_find(ob_h, "living_unit1:transmitted_solar [-]"))
ob_hvac_net = (ob_heat or 0) - (ob_cool or 0)

# In OB, q_surf_conv_total already includes window conv+absorbed,
# q_internal_conv is convective internal gains only (rad goes to surfaces),
# transmitted solar goes to surfaces (not directly to air) unless no distribution.
# The balance: surf_conv + infil + thermal_mass + internal_conv + solar_to_air + hvac = 0
print(f"  {'q_surf_conv_total':30s}  {ob_total:+10.1f} kWh")
print(f"  {'q_infiltration_sensible':30s}  {ob_infil:+10.1f} kWh")
print(f"  {'q_thermal_mass':30s}  {ob_mass:+10.1f} kWh")
print(f"  {'q_internal_conv (to air)':30s}  {ob_int:+10.1f} kWh")
print(f"  {'q_hvac_net (heat-cool)':30s}  {ob_hvac_net:+10.1f} kWh")
print(f"  {'─'*43}")
sub = (ob_total or 0) + (ob_infil or 0) + (ob_mass or 0) + (ob_int or 0) + ob_hvac_net
print(f"  {'Residual (excl solar→surfaces)':30s}  {sub:+10.1f} kWh")
print(f"  (Residual should ≈ −transmitted_solar_to_surfaces;")
print(f"   solar absorbed by surfaces raises T_surf → enters via surf_conv)")
print(f"  transmitted_solar total:                {ob_solar_trans:+10.1f} kWh")


# ══════════════════════════════════════════════════════════════════════════════
print()
print("═" * 76)
print("6. OVERALL COMPARISON: Heating/Cooling loads")
print("═" * 76)
print()

ep_heat_ci = ep_find(ep_h, "Zone Sensible Heating Energy")
ep_cool_ci = ep_find(ep_h, "Zone Sensible Cooling Energy")
ep_heat = ep_sum_kwh(ep_rows, ep_heat_ci, is_energy_j=True)
ep_cool = ep_sum_kwh(ep_rows, ep_cool_ci, is_energy_j=True)

fmt_row("Heating load", ep_heat, ob_heat)
fmt_row("Cooling load", ep_cool, ob_cool)
