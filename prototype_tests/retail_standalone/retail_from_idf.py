#!/usr/bin/env python3
"""Write examples/doe_retail_standalone.yaml from the DOE prototype IDF.

The model is a transcription of `eplus_run/in.idf`
(ASHRAE901_RetailStandalone_STD2019_Denver, E+ 25.2 example file, one annual
RunPeriod), object by object, as a person recreating the IDF in OpenBSE would:
no zone multipliers, E+'s names, vertices, layers and values. What OpenBSE
cannot express is listed in INPUT_AUDIT.md next to this script.

The envelope, loads and schedules are generated from the IDF; the HVAC section
is written from the IDF values below. Rerun after editing either:

    python3 retail_from_idf.py eplus_run/in.idf ../../examples/doe_retail_standalone.yaml
"""
import math
import sys
from collections import defaultdict

ROUGHNESS = {
    "veryrough": "very_rough", "rough": "rough", "mediumrough": "medium_rough",
    "mediumsmooth": "medium_smooth", "smooth": "smooth", "verysmooth": "very_smooth",
}
# E+ EMS lighting program: during unoccupied hours, lights above 0.02 W/ft²
# are cut to 0.02 W/ft² (0.093 in the program's test, 0.09290304 in its set).
EMS_LIGHT_TEST = 0.02 / 0.093          # W/m², the program's threshold
EMS_LIGHT_SET = 0.02 / 0.09290304      # W/m², the level it sets


def parse_idf(path):
    text = open(path, encoding="latin-1").read().replace("\r", "")
    objs, cur = defaultdict(list), []
    for line in text.splitlines():
        code = line.split("!")[0]
        if not cur and not code.strip():
            continue
        cur.append(code)
        if ";" in code:
            fields = [f.strip() for f in " ".join(cur).replace(";", ",").split(",")]
            while fields and fields[-1] == "":
                fields.pop()
            objs[fields[0].lower()].append(fields[1:])
            cur = []
    return objs


def num(fields, i, default=None):
    return float(fields[i]) if len(fields) > i and fields[i] != "" else default


def fmt(x):
    """Shortest exact-looking number: IDF values are copied, not rounded."""
    if isinstance(x, str):
        return x
    if float(x).is_integer() and abs(x) < 1e9:
        return f"{x:.1f}"
    return repr(float(x))


def flist(values):
    return "[" + ", ".join(fmt(v) for v in values) + "]"


# ─── Schedules ────────────────────────────────────────────────────────────────

DAY_TYPES = ("weekday", "saturday", "sunday", "holiday", "summer_dd", "winter_dd")


def compact_profiles(fields):
    """Schedule:Compact (one 'Through: 12/31' period) -> {day type: 24 values}."""
    profiles, targets, until = {}, [], []
    assert fields[2].lower().replace(" ", "") == "through:12/31", fields[:3]

    def close():
        if targets:
            vals, prev = [], 0
            for hh, v in until:
                vals += [v] * (hh - prev)
                prev = hh
            assert len(vals) == 24, (fields[0], until)
            for t in targets:
                profiles.setdefault(t, vals)

    i = 3
    while i < len(fields):
        f = fields[i]
        low = f.lower().replace(" ", "")
        if low.startswith("for"):
            close()
            words = f.split(":", 1)[1].split() if ":" in f else f.split()[1:]
            targets, until = [], []
            for w in words:
                w = w.lower()
                if w == "weekdays":
                    targets.append("weekday")
                elif w in ("saturday", "saturdays"):
                    targets.append("saturday")
                elif w == "sunday":
                    targets.append("sunday")
                elif w == "holidays":
                    targets.append("holiday")
                elif w == "summerdesignday":
                    targets.append("summer_dd")
                elif w == "winterdesignday":
                    targets.append("winter_dd")
                elif w == "weekends":
                    targets += ["saturday", "sunday"]
                elif w in ("alldays", "allotherdays"):
                    targets += [t for t in DAY_TYPES if t not in profiles and t not in targets]
                else:
                    sys.exit(f"{fields[0]}: day type {w}")
            i += 1
        elif low.startswith("until"):
            hh, mm = f.split(":", 1)[1].strip().split(":")
            assert int(mm) == 0, f
            until.append((int(hh), float(fields[i + 1])))
            i += 2
        else:
            sys.exit(f"{fields[0]}: unexpected field {f}")
    close()
    return profiles


def schedule_yaml(name, prof, comment=None):
    """OpenBSE explicit schedule: weekday, then each day type that differs from
    what OpenBSE would fall back to (saturday/sunday -> weekday,
    holiday -> sunday)."""
    out = []
    if comment:
        out.append(f"  # {comment}")
    out.append(f"  - name: {name}")
    out.append(f"    weekday:  {flist(prof['weekday'])}")
    if prof["saturday"] != prof["weekday"]:
        out.append(f"    saturday: {flist(prof['saturday'])}")
    if prof["sunday"] != prof["weekday"]:
        out.append(f"    sunday:   {flist(prof['sunday'])}")
    if prof["holiday"] != prof["sunday"]:
        out.append(f"    holiday:  {flist(prof['holiday'])}")
    return out


def capped_lighting(light, occ, lpd):
    """Lighting fractions with the IDF's EMS unoccupied cap applied."""
    cap = EMS_LIGHT_SET / lpd
    out = {}
    for d in DAY_TYPES:
        out[d] = [
            (cap if (o <= 0.0 and f * lpd >= EMS_LIGHT_TEST) else f)
            for f, o in zip(light[d], occ[d])
        ]
    return out


# ─── Geometry ─────────────────────────────────────────────────────────────────

def poly_area(verts):
    """Area of a planar polygon (Newell normal)."""
    nx = ny = nz = 0.0
    n = len(verts)
    for i in range(n):
        x1, y1, z1 = verts[i]
        x2, y2, z2 = verts[(i + 1) % n]
        nx += (y1 - y2) * (z1 + z2)
        ny += (z1 - z2) * (x1 + x2)
        nz += (x1 - x2) * (y1 + y2)
    return 0.5 * math.sqrt(nx * nx + ny * ny + nz * nz)


def point(v):
    return "{x: %s, y: %s, z: %s}" % tuple(fmt(round(c, 9)) for c in v)


def main(idf_path, out_path):
    idf = parse_idf(idf_path)
    L = []
    w = L.append

    zones = {}  # upper -> (name, origin, volume)
    for f in idf["zone"]:
        zones[f[0].upper()] = (f[0], (num(f, 2, 0), num(f, 3, 0), num(f, 4, 0)), num(f, 8))
    zone_order = [f[0] for f in idf["zone"]]

    # ---- schedules ----
    compact_raw = {f[0].upper(): f for f in idf["schedule:compact"]}

    class Lazy(dict):
        def __missing__(self, key):
            f = compact_raw[key]
            self[key] = (f[0], compact_profiles(f))
            return self[key]
    compact = Lazy()

    def prof(name):
        return compact[name.upper()][1]

    # ---- surfaces ----
    surf_zone = {f[0].upper(): f[3] for f in idf["buildingsurface:detailed"]}
    ffactor = {f[0].upper(): f for f in idf["construction:ffactorgroundfloor"]}
    used_cons = set()
    surfaces = []
    ext_area = defaultdict(float)  # E+ Zone ExteriorTotalSurfArea
    gross = {}
    for f in idf["buildingsurface:detailed"]:
        name, stype, cons, zone = f[0], f[1].lower(), f[2], f[3]
        bc, bc_obj = f[5].lower(), f[6]
        ox, oy, oz = zones[zone.upper()][1]
        coords = [float(x) for x in f[10:] if x != ""]
        verts = [(coords[i] + ox, coords[i + 1] + oy, coords[i + 2] + oz)
                 for i in range(1, len(coords), 3)]
        assert int(coords[0]) == len(verts), name
        gross[name.upper()] = poly_area(verts)
        used_cons.add(cons)
        s = {"name": name, "zone": zone, "type": stype, "construction": cons, "verts": verts}
        if bc == "outdoors":
            s["boundary"] = "outdoor"
            ext_area[zone.upper()] += gross[name.upper()]
        elif bc == "groundfcfactormethod":
            s["boundary"] = "ground"
            s["perimeter"] = float(ffactor[cons.upper()][3])
            assert abs(float(ffactor[cons.upper()][2]) - gross[name.upper()]) < 0.01, name
        elif bc == "surface":
            s["boundary"] = f"!zone '{surf_zone[bc_obj.upper()]}'"
            s["adjacent"] = bc_obj
        else:
            sys.exit(f"{name}: boundary {f[5]}")
        surfaces.append(s)

    subsurfaces = []
    for f in idf["fenestrationsurface:detailed"]:
        name, kind, cons, base = f[0], f[1].lower(), f[2], f[3]
        zone = surf_zone[base.upper()]
        ox, oy, oz = zones[zone.upper()][1]
        assert num(f, 7, 1.0) == 1.0, name
        coords = [float(x) for x in f[8:] if x != ""]
        verts = [(coords[i] + ox, coords[i + 1] + oy, coords[i + 2] + oz)
                 for i in range(1, len(coords), 3)]
        used_cons.add(cons)
        if kind == "door":
            # E+ counts an opaque door's gross area in ExteriorTotalSurfArea on
            # top of its parent wall's gross area (SurfaceGeometry.cc: only
            # windows are skipped), so the Flow/ExteriorArea infiltration
            # includes the doors twice.
            ext_area[zone.upper()] += poly_area(verts)
        subsurfaces.append({"name": name, "zone": zone, "kind": kind, "construction": cons,
                            "parent": base, "verts": verts, "frame": f[6] if len(f) > 6 else ""})

    # ---- materials and constructions ----
    mats = {}
    for f in idf["material"]:
        mats[f[0].upper()] = ("mass", f)
    for f in idf["material:nomass"]:
        mats[f[0].upper()] = ("nomass", f)
    glazing = {f[0].upper(): f for f in idf["windowmaterial:simpleglazingsystem"]}
    cons_by_name = {f[0].upper(): f for f in idf["construction"]}
    for f in idf["internalmass"]:
        used_cons.add(f[1])

    # =====================================================================
    w("# ============================================================================")
    w("# OpenBSE Model: DOE ASHRAE 90.1-2019 Retail Standalone Prototype (Denver, 5B)")
    w("# ============================================================================")
    w("#")
    w("# Transcribed object by object from the DOE prototype IDF")
    w("# ASHRAE901_RetailStandalone_STD2019_Denver.idf (E+ 25.2 example file), kept")
    w("# with its E+ results in prototype_tests/retail_standalone/eplus_run/.")
    w("# Envelope, loads and schedules are generated by")
    w("# prototype_tests/retail_standalone/retail_from_idf.py; E+ names, vertices")
    w("# (zone origins added) and values are kept. No zone multipliers.")
    w("# prototype_tests/retail_standalone/INPUT_AUDIT.md lists every IDF object, how")
    w("# it is represented here, and what OpenBSE cannot express yet.")
    w("#")
    w("# Building: one storey, 2,294 m², 6.096 m floor to roof; five zones (Back_Space,")
    w("# Core_Retail, Point_Of_Sale, Front_Retail, Front_Entry). Four packaged")
    w("# single-zone units (gas furnace, DX cooling) and a gas unit heater in the")
    w("# entry vestibule. Origin at the south-west corner, X east, Y north.")
    w("# ============================================================================")
    w("")
    w("simulation:")
    w("  timesteps_per_hour: 4            # IDF Timestep")
    w("  start_month: 1")
    w("  start_day: 1")
    w("  end_month: 12")
    w("  end_day: 31")
    w("  terrain: city                    # Building: Terrain City")
    w("  outside_convection_algorithm: tarp   # SurfaceConvectionAlgorithm:Outside")
    w("  solar_distribution: full_interior_and_exterior")
    w("  heating_sizing_factor: 1.2       # Sizing:Parameters")
    w("  cooling_sizing_factor: 1.2")
    w("  unmet_heating_tolerance: 0.556   # OutputControl:ReportingTolerances (1 F)")
    w("  unmet_cooling_tolerance: 0.556")
    w("  sizing_averaging_window: 6       # Sizing:Parameters timesteps in averaging window")
    w("  auto_cooling_design_days: false  # size on the IDF's two design days only")
    w("  # RunPeriodControl:SpecialDays (the weather year starts on a Sunday)")
    w("  holidays:")
    for m, d, label in [(1, 1, "New Years Day"), (1, 16, "MLK Day"), (2, 20, "Presidents Day"),
                        (5, 29, "Memorial Day"), (7, 4, "Independence Day"),
                        (9, 4, "Labor Day"), (10, 9, "Columbus Day"), (11, 11, "Veterans Day"),
                        (11, 23, "Thanksgiving"), (12, 25, "Christmas")]:
        w(f"    - {{month: {m}, day: {d}}}   # {label}")
    w("")
    w("weather_files:")
    w('  - "../weather/USA_CO_Denver-Aurora-Buckley.AFB.724695_TMY3.epw"   # as the E+ run')
    w("")

    # ---- design days ----
    w("# SizingPeriod:DesignDay")
    w("design_days:")
    for f in idf["sizingperiod:designday"]:
        heating = f[3].lower() == "winterdesignday"
        w(f"  - name: {f[0]}")
        w(f"    design_temp: {fmt(num(f, 4))}")
        w(f"    daily_range: {fmt(num(f, 5))}")
        assert f[8].lower() == "wetbulb"
        w("    humidity_type: wetbulb")
        w(f"    humidity_value: {fmt(num(f, 9))}")
        w(f"    pressure: {fmt(num(f, 14))}")
        w(f"    wind_speed: {fmt(num(f, 15))}")
        w(f"    month: {int(num(f, 1))}")
        w(f"    day: {int(num(f, 2))}")
        w(f"    day_type: {'winter' if heating else 'summer'}")
        if f[20].lower() == "ashraetau":
            w(f"    taub: {fmt(num(f, 23))}")
            w(f"    taud: {fmt(num(f, 24))}")
    w("")

    # ---- schedules ----
    occ = prof("BLDG_OCC_SCH")
    lights = {f[1].upper(): f for f in idf["lights"]}
    people_zone = {f[1].upper(): f for f in idf["people"]}
    w("# Schedule:Compact, hour 1 (00:00-01:00) first. OpenBSE falls back")
    w("# saturday/sunday -> weekday and holiday -> sunday; a day type is written only")
    w("# where the IDF differs from that fallback.")
    w("schedules:")
    plain = ["BLDG_OCC_SCH", "BLDG_EQUIP_SCH", "INFIL_SCH_PNNL", "INFIL_Door_Opening_SCH",
             "HVACOperationSchd", "MinOA_MotorizedDamper_Sched", "BLDG_SWH_SCH",
             "Exterior_lighting_schedule_a", "Exterior_lighting_schedule_b_2016"]
    for s in plain:
        name, p = compact[s.upper()]
        L.extend(schedule_yaml(name, p))
    w("")
    w("  # Lighting with the IDF's EMS programs SET_<zone>_Light_EMS_Program applied:")
    w("  # while the zone's People Occupant Count is zero, lights above 0.02 W/ft²")
    w("  # (0.215 W/m²) are set to 0.02 W/ft². People and lighting schedules are")
    w("  # fixed, so the program's result is a fixed schedule. Front_Entry has no")
    w("  # people, so its lights sit at the cap all year. (E+ reads the previous")
    w("  # timestep's occupancy, so it applies the cap one 15-minute step late at")
    w("  # each occupancy change; the hourly schedule cannot hold that.)")
    light_sched = {}
    for zone in zone_order:
        f = lights[zone.upper()]
        lpd = float(f[5])
        has_people = zone.upper() in people_zone and people_zone[zone.upper()][3].lower() == "area/person"
        zone_occ = occ if has_people else {d: [0.0] * 24 for d in DAY_TYPES}
        capped = capped_lighting(prof(f[2]), zone_occ, lpd)
        sname = f"{f[2]} {zone} EMS"
        light_sched[zone] = sname
        L.extend(schedule_yaml(sname, capped, f"{f[2]} for {zone} ({fmt(lpd)} W/m²)"))
    w("")

    # ---- materials ----
    w("# Material and Material:NoMass used by the constructions")
    w("materials:")
    written = set()
    cons_layers = {}
    window_cons = {}
    for cname in sorted(used_cons, key=str.upper):
        key = cname.upper()
        if key in ffactor:
            continue
        layers = [l for l in cons_by_name[key][1:] if l]
        if len(layers) == 1 and layers[0].upper() in glazing:
            window_cons[cname] = glazing[layers[0].upper()]
            continue
        cons_layers[cname] = layers
        for lname in layers:
            if lname.upper() in written:
                continue
            written.add(lname.upper())
            kind, f = mats[lname.upper()]
            w(f"  - name: {f[0]}")
            w(f"    roughness: {ROUGHNESS[f[1].lower()]}")
            if kind == "mass":
                w(f"    conductivity: {fmt(num(f, 3))}")
                w(f"    density: {fmt(num(f, 4))}")
                w(f"    specific_heat: {fmt(num(f, 5))}")
                w(f"    thermal_absorptance: {fmt(num(f, 6, 0.9))}")
                w(f"    solar_absorptance: {fmt(num(f, 7, 0.7))}")
                w(f"    visible_absorptance: {fmt(num(f, 8, 0.7))}")
            else:
                w(f"    thermal_resistance: {fmt(num(f, 2))}")
                w(f"    thermal_absorptance: {fmt(num(f, 3, 0.9))}")
                w(f"    solar_absorptance: {fmt(num(f, 4, 0.7))}")
                w(f"    visible_absorptance: {fmt(num(f, 5, 0.7))}")
    w("")
    w("# Construction (layers outside to inside)")
    w("constructions:")
    for cname, layers in cons_layers.items():
        w(f"  - name: {cname}")
        w("    layers:")
        for lname in layers:
            kind, f = mats[lname.upper()]
            t = num(f, 2) if kind == "mass" else 0.001  # thickness ignored for no-mass
            w(f"      - {{material: {f[0]}, thickness: {fmt(t)}}}")
    w("")
    w("# Construction:FfactorGroundFloor; ground at Site:GroundTemperature:FCfactorMethod")
    fc_temps = idf["site:groundtemperature:fcfactormethod"][0]
    w("f_factor_constructions:")
    for f in idf["construction:ffactorgroundfloor"]:
        w(f"  - name: {f[0]}")
        w(f"    f_factor: {fmt(num(f, 1))}")
        w(f"    ground_temperatures: {flist([float(x) for x in fc_temps])}")
    w("")
    w("# WindowMaterial:SimpleGlazingSystem (U, SHGC, visible transmittance)")
    w("window_constructions:")
    for cname, g in window_cons.items():
        w(f"  - name: {cname}")
        w(f"    u_factor: {fmt(num(g, 1))}")
        w(f"    shgc: {fmt(num(g, 2))}")
        w(f"    visible_transmittance: {fmt(num(g, 3))}")
    w("")

    # ---- zones ----
    masses = defaultdict(list)
    for f in idf["internalmass"]:
        masses[f[2].upper()].append((f[1], float(f[4])))
    w("# Zone (volume as the IDF; floor area from the floor polygons) and InternalMass")
    w("zones:")
    for zone in zone_order:
        name, origin, vol = zones[zone.upper()]
        w(f"  - name: {name}")
        w(f"    volume: {fmt(vol)}")
        if masses[zone.upper()]:
            w("    internal_mass:")
            for c, a in masses[zone.upper()]:
                w(f"      - {{construction: {c}, area: {fmt(a)}}}")
    w("")
    w("zone_groups:")
    w("  - name: PSZ Zones")
    w("    zones: [Back_Space, Core_Retail, Point_Of_Sale, Front_Retail]")
    w("")

    # ---- people ----
    w("# People: 6.193 m²/person, ACTIVITY_SCH 120 W, radiant 0.3, sensible fraction")
    w("# autocalculated (OpenBSE default). Front_Entry's People object has 0 people.")
    w("people:")
    for f in idf["people"]:
        if f[3].lower() != "area/person":
            assert f[3].lower() == "people" and float(f[4]) == 0.0
            continue
        w(f"  - name: {f[0]} People")
        w(f"    zones: [{f[1]}]")
        w(f"    area_per_person: {fmt(num(f, 6))}")
        assert f[9].upper() == "ACTIVITY_SCH"
        w("    activity_level: 120.0")
        w(f"    radiant_fraction: {fmt(num(f, 7))}")
        w(f"    schedule: {compact[f[2].upper()][0]}")
    w("")

    # ---- lights ----
    w("# Lights: Watts/Area; radiant_fraction = E+ Fraction Radiant 0.7 + Fraction")
    w("# Visible 0.2; return air 0. Schedules carry the EMS unoccupied cap (above).")
    w("lights:")
    for f in idf["lights"]:
        assert f[3].lower() == "watts/area"
        w(f"  - name: {f[0]}")
        w(f"    zones: [{f[1]}]")
        w(f"    watts_per_area: {fmt(num(f, 5))}")
        w(f"    radiant_fraction: {fmt(num(f, 8) + num(f, 9))}")
        w(f"    return_air_fraction: {fmt(num(f, 7))}")
        w(f"    schedule: {light_sched[f[1]]}")
    w("")

    # ---- equipment ----
    w("# ElectricEquipment (EquipmentLevel)")
    w("equipment:")
    for f in idf["electricequipment"]:
        assert f[3].lower() == "equipmentlevel"
        w(f"  - name: {f[0]}")
        w(f"    zones: [{f[1]}]")
        w(f"    power: {fmt(num(f, 4))}")
        w(f"    radiant_fraction: {fmt(num(f, 8))}")
        w(f"    latent_fraction: {fmt(num(f, 7))}")
        w(f"    lost_fraction: {fmt(num(f, 9))}")
        w(f"    schedule: {compact[f[2].upper()][0]}")
    w("")

    # ---- infiltration ----
    w("# ZoneInfiltration:DesignFlowRate. Flow/ExteriorArea is written as the design")
    w("# flow E+ computes: rate x the zone's ExteriorTotalSurfArea (gross outdoor")
    w("# walls and roofs, plus each opaque door's area again, as E+ counts it); the")
    w("# values agree with eplusout.eio. Coefficients: velocity 0.224 s/m only.")
    w("infiltration:")
    for f in idf["zoneinfiltration:designflowrate"]:
        method = f[3].lower()
        if method == "flow/exteriorarea":
            q = num(f, 6) * ext_area[f[1].upper()]
            note = f"{fmt(num(f, 6))} m³/s-m² x {ext_area[f[1].upper()]:.3f} m²"
        elif method == "flow/zone":
            q = num(f, 4)
            note = "Flow/Zone"
        else:
            sys.exit(method)
        w(f"  - name: {f[0]}")
        w(f"    zones: [{f[1]}]")
        w(f"    design_flow_rate: {q:.9f}   # {note}")
        w(f"    constant_coefficient: {fmt(num(f, 8, 1.0))}")
        w(f"    temperature_coefficient: {fmt(num(f, 9, 0.0))}")
        w(f"    wind_coefficient: {fmt(num(f, 10, 0.0))}")
        w(f"    wind_squared_coefficient: {fmt(num(f, 11, 0.0))}")
        w(f"    schedule: {compact[f[2].upper()][0]}")
    w("")

    # ---- outdoor air ----
    dsoa = {f[0].upper(): f for f in idf["designspecification:outdoorair"]}
    w("# DesignSpecification:OutdoorAir (Flow/Area) for the zones the packaged units")
    w("# serve. Front_Entry's unit heater recirculates only, so its DSOA (used by E+")
    w("# zone sizing alone) is left out.")
    w("outdoor_air:")
    for zone in zone_order[:4]:
        f = dsoa[f"SZ DSOA {zone}".upper()]
        assert f[1].lower() == "flow/area"
        w(f"  - name: {f[0]}")
        w(f"    zones: [{zone}]")
        w(f"    per_area: {fmt(num(f, 3))}")
    w("")

    # ---- surfaces ----
    w("# BuildingSurface:Detailed and FenestrationSurface:Detailed. Opaque doors are")
    w("# wall subsurfaces of their parent wall (their area comes off it), which is")
    w("# how E+ treats a Door in the heat balance. Skylight_Frame")
    w("# (WindowProperty:FrameAndDivider) has zero frame and no divider, so it adds")
    w("# nothing.")
    w("surfaces:")
    for s in surfaces:
        w(f"  - name: {s['name']}")
        w(f"    zone: {s['zone']}")
        w(f"    type: {s['type']}")
        w(f"    construction: {s['construction']}")
        w(f"    boundary: {s['boundary']}")
        if "adjacent" in s:
            w(f"    adjacent_surface: {s['adjacent']}")
        if "perimeter" in s:
            w(f"    exposed_perimeter: {fmt(s['perimeter'])}   # Construction:FfactorGroundFloor")
        if s["boundary"] != "outdoor":
            w("    sun_exposure: false")
            w("    wind_exposure: false")
        w("    vertices:")
        for v in s["verts"]:
            w(f"      - {point(v)}")
    for s in subsurfaces:
        assert s["frame"] in ("", "Skylight_Frame"), s["name"]
        w(f"  - name: {s['name']}")
        w(f"    zone: {s['zone']}")
        w(f"    type: {'wall' if s['kind'] == 'door' else 'window'}")
        w(f"    construction: {s['construction']}")
        w("    boundary: outdoor")
        w(f"    parent_surface: {s['parent']}")
        w("    vertices:")
        for v in s["verts"]:
            w(f"      - {point(v)}")
    w("")

    # ---- exterior lights ----
    w("# Exterior:Lights, AstronomicalClock (off while the sun is up)")
    w("exterior_equipment:")
    for f in idf["exterior:lights"]:
        assert f[3].lower() == "astronomicalclock"
        w(f"  - name: {f[0]}")
        w(f"    power: {fmt(num(f, 2))}")
        w(f"    schedule: {compact[f[1].upper()][0]}")
        w("    fuel: electricity")
        w("    subcategory: Exterior Lights")
        w("    astronomical_clock: true")
    w("")

    # ---- daylighting ----
    rules = idf["globalgeometryrules"][0]
    relative_points = len(rules) < 4 or rules[3].lower() == "relative"
    ref_points = {f[0].upper(): f for f in idf.get("daylighting:referencepoint", [])}
    controls = idf.get("daylighting:controls", [])
    if controls:
        w("# Daylighting:Controls (SplitFlux) and Daylighting:ReferencePoint; the")
        w("# reference points are zone-relative in the IDF, so the zone origin is added.")
        w("# The glare inputs only report glare and are left out.")
        w("daylighting_controls:")
    for f in controls:
        assert f[2].lower() == "splitflux" and not f[3], f[0]
        zname, origin, _ = zones[f[1].upper()]
        control = {"continuous": "continuous", "continuousoff": "continuous_off",
                   "stepped": "stepped"}[f[4].lower()]
        w(f"  - name: {f[0]}")
        w(f"    zone: {zname}")
        w(f"    lighting_control: {control}")
        w(f"    minimum_input_power_fraction: {fmt(num(f, 5))}")
        w(f"    minimum_light_output_fraction: {fmt(num(f, 6))}")
        if control == "stepped":
            w(f"    stepped_control_steps: {int(num(f, 7))}")
        w("    reference_points:")
        for k in range(13, len(f) - 2, 3):
            rp = ref_points[f[k].upper()]
            x, y, z = num(rp, 2), num(rp, 3), num(rp, 4)
            if relative_points:
                x, y, z = x + origin[0], y + origin[1], z + origin[2]
            w(f"      - {{x: {fmt(x)}, y: {fmt(y)}, z: {fmt(z)}, "
              f"fraction_of_zone: {fmt(num(f, k + 1))}, illuminance_setpoint: {fmt(num(f, k + 2))}}}"
              f"   # {rp[0]}")
    if controls:
        w("")

    # ---- DHW ----
    wh = idf["waterheater:mixed"][0]
    mains = idf["site:watermainstemperature"][0]
    w("# WaterHeater:Mixed 'Gas Tank', stand-alone (Peak Use Flow Rate on the tank).")
    w("# E+ draws the whole use flow from the tank and refills it with mains water;")
    w("# use_temperature at the 82.2 °C tank limit makes every fixture take tank")
    w("# water only. Off- and on-cycle parasitic gas 1,860 W each, none of it to the")
    w("# tank, so 1,860 W of gas all year. Losses to Back_space.")
    w("dhw_systems:")
    w("  - name: Gas Tank")
    w("    mains_temperature:")
    w("      correlation:")
    w(f"        annual_avg_temp: {fmt(num(mains, 2))}")
    w(f"        max_monthly_delta: {fmt(num(mains, 3))}")
    w("    water_heater:")
    w("      name: Gas Tank")
    w("      fuel_type: gas")
    w(f"      tank_volume: {num(wh, 1) * 1000:.7f}   # L")
    w(f"      capacity: {fmt(num(wh, 6))}")
    w(f"      efficiency: {fmt(num(wh, 11))}")
    w("      setpoint: 60.0                # Hot Water Setpoint Temp Schedule")
    w(f"      deadband: {num(wh, 3):.6f}")
    w(f"      ua_standby: {fmt(num(wh, 23))}")
    assert num(wh, 13) == num(wh, 16) and wh[14].lower() == "naturalgas" and wh[17] == ""
    w(f"      parasitic_power: {fmt(num(wh, 13))}")
    w(f"      ambient_zone: {zones[wh[21].upper()][0]}")
    w("    loads:")
    w("      - name: Gas Tank use")
    w(f"        peak_flow_rate: {num(wh, 27) * 1000:.6f}   # L/s")
    w(f"        schedule: {compact[wh[28].upper()][0]}")
    w(f"        use_temperature: {fmt(num(wh, 4))}")
    w("")

    # ---- thermostats ----
    dual = {f[0].upper(): f for f in idf["thermostatsetpoint:dualsetpoint"]}
    groups = defaultdict(list)  # (heating sched, cooling sched) -> zones
    for f in idf["zonecontrol:thermostat"]:
        assert f[3].lower() == "thermostatsetpoint:dualsetpoint"
        d = dual[f[4].upper()]
        groups[(d[1], d[2])].append(f[1])
    w("# ZoneControl:Thermostat / ThermostatSetpoint:DualSetpoint with the IDF's")
    w("# setpoint schedules; the design-day setpoints are their WinterDesignDay and")
    w("# SummerDesignDay day types. The IDF's EMS program")
    w("# OA_BASED_OPTIMUM_START_EMS_Program, which holds Core_Retail's setback for one")
    w("# more hour on mild mornings, is not modelled (INPUT_AUDIT.md).")
    w("thermostats:")
    for (hs, cs), zl in groups.items():
        hp, cp = prof(hs), prof(cs)
        w(f"  - name: {' '.join(zl)} Thermostat")
        w(f"    zones: [{', '.join(zl)}]")
        w(f"    heating_setpoint: {fmt(max(hp['weekday']))}")
        w(f"    cooling_setpoint: {fmt(min(cp['weekday']))}")
        w(f"    heating_setpoint_schedule: {compact[hs.upper()][0]}")
        w(f"    cooling_setpoint_schedule: {compact[cs.upper()][0]}")
        w(f"    design_day_heating_setpoints: {flist(hp['winter_dd'])}")
        w(f"    design_day_cooling_setpoints: {flist(cp['summer_dd'])}")
    w("")
    w("# Thermostat setpoint schedules [°C]")
    w("schedules_setpoints_placeholder")
    temp_sched = []
    for hs, cs in groups:
        for s in (hs, cs):
            if s not in temp_sched:
                temp_sched.append(s)
    block = []
    for s in temp_sched:
        name, p = compact[s.upper()]
        block.extend(schedule_yaml(name, p))
    k = L.index("schedules_setpoints_placeholder")
    L[k - 1:k + 1] = []
    # temperature schedules join the schedules list
    k = L.index("schedules:")
    L[k + 1:k + 1] = ["  # Thermostat setpoint schedules [°C]"] + block + [""]

    psz_loops = "".join(psz(idf, u) for u in idf["airloophvac:unitarysystem"])
    L.append(HVAC.replace("PSZ_LOOPS\n", psz_loops))
    open(out_path, "w").write("\n".join(L) + "\n")
    print(f"{len(surfaces)} surfaces, {len(subsurfaces)} subsurfaces; exterior areas "
          + ", ".join(f"{z} {a:.2f}" for z, a in ext_area.items()))


# ─── HVAC (from the IDF values; see INPUT_AUDIT.md) ───────────────────────────

def psz(idf, u):
    """One AirLoopHVAC:UnitarySystem (Load control, blow-through) as a packaged
    single-zone loop: its Fan:SystemModel, Coil:Heating:Fuel,
    Coil:Cooling:DX:MultiSpeed, UnitarySystemPerformance:Multispeed and
    Controller:OutdoorAir."""
    by = lambda cls: {f[0].upper(): f for f in idf[cls]}
    name, zone = u[0], u[2]
    n = name.split(":")[1].split("_")[0]
    fan = by("fan:systemmodel")[u[8].upper()]
    heat = by("coil:heating:fuel")[u[12].upper()]
    cool = by("coil:cooling:dx:multispeed")[u[15].upper()]
    perf = by("unitarysystemperformance:multispeed")[u[-1].upper()]
    oa = by("controller:outdoorair")[f"PSZ-AC:{n}_OA_CONTROLLER"]
    assert u[1].lower() == "load" and u[9].lower() == "blowthrough" and u[10] == "HVACOperationSchd"
    assert fan[5].lower() == "discrete" and fan[11].lower() == "totalefficiencyandpressure"
    dp, motor, total = float(fan[7]), float(fan[8]), float(fan[14])
    nfan = int(fan[21])
    fan_speeds = [(float(fan[22 + 2 * k]), float(fan[23 + 2 * k])) for k in range(nfan)]
    no_load = float(perf[4])
    heat_ratios = [float(perf[5 + 2 * k]) for k in range(int(perf[1]))]
    cool_ratios = [float(perf[6 + 2 * k]) for k in range(int(perf[2]))]
    assert all(r == heat_ratios[-1] for r in heat_ratios), "single-stage heating coil"
    assert cool[9].lower() == "no" and cool[10].lower() == "no"
    nspeed = int(cool[18])
    assert nspeed == len(cool_ratios)
    speeds = []
    for k in range(nspeed):
        b = 19 + 20 * k
        speeds.append(dict(cap=float(cool[b]), shr=float(cool[b + 1]), cop=float(cool[b + 2]),
                           flow=float(cool[b + 3]), curves=cool[b + 6:b + 11], frac=cool_ratios[k]))
    curves = speeds[0]["curves"]
    assert all(s["curves"] == curves for s in speeds)
    econ = oa[7].lower()
    assert econ in ("differentialdrybulb", "noeconomizer") and oa[14].lower() == "lockoutwithheating"
    # Controller:OutdoorAir DifferentialDryBulb, LockoutWithHeating; its mixed-air
    # setpoint comes from SetpointManager:SingleZone:Cooling (10-50 °C) through
    # SetpointManager:MixedAir, so the damper opens only as far as the zone's
    # cooling load needs (OpenBSE load_reset).
    econ_yaml = ("      economizer:\n        economizer_type: differential_dry_bulb\n"
                 "        load_reset: {min_supply_temp: 10.0, max_supply_temp: 50.0}\n"
                 if econ == "differentialdrybulb" else "")
    fan_yaml = ""
    if nfan > 1 or fan_speeds != [(1.0, 1.0)]:
        fan_yaml = "        speeds:                          # Speed N Flow / Electric Power Fraction\n" + "".join(
            f"          - {{flow_fraction: {fmt(f)}, power_fraction: {fmt(pw)}}}\n" for f, pw in fan_speeds)
    speed_yaml = "".join(
        f"          - {{rated_capacity: {fmt(s['cap'])}, shr: {fmt(s['shr'])}, cop: {fmt(s['cop'])}, "
        f"rated_airflow: {fmt(s['flow'])}, flow_fraction: {fmt(s['frac'])}}}\n" for s in speeds)
    return f"""  # {name}: AirLoopHVAC:UnitarySystem, Load control, blow-through fan,
  # UnitarySystemPerformance:Multispeed {perf[0]}
  - name: PSZ-AC:{n}
    availability_schedule: HVACOperationSchd
    night_cycle:                         # AvailabilityManager:NightCycle
      control: cycle_on_any
      thermostat_tolerance: 1.0
      cycling_run_time: 1800
    controls:
      heating_supply_temp: 40.0          # Sizing:System / Sizing:Zone, Maximum Supply Air Temperature
      cooling_supply_temp: 12.8
      zone_heating_supply_humidity_ratio: 0.008   # Sizing:Zone design supply humidity ratios
      zone_cooling_supply_humidity_ratio: 0.007
      design_zone_flow: autosize
      cycling: on_off
      fan_operating_mode: continuous     # fan operating-mode schedule HVACOperationSchd
      no_load_flow_fraction: {fmt(no_load)}
      heating_flow_fraction: {fmt(heat_ratios[-1])}
      minimum_oa_schedule: MinOA_MotorizedDamper_Sched
{econ_yaml}    equipment:
      - type: fan                        # Fan:SystemModel, Discrete
        name: {fan[0]}
        source: constant_volume
        design_flow_rate: autosize
        pressure_rise: {fmt(dp)}
        motor_efficiency: {fmt(motor)}
        impeller_efficiency: {total / motor:.10g}   # total {fmt(total)} / motor {fmt(motor)}
        motor_in_airstream_fraction: {fmt(float(fan[9]))}
{fan_yaml}      - type: heating_coil               # Coil:Heating:Fuel
        name: {heat[0]}
        source: gas
        capacity: autosize
        efficiency: {fmt(float(heat[3]))}
        setpoint: 40.0
      - type: dx_multispeed              # Coil:Cooling:DX:MultiSpeed (hard-sized in the IDF)
        name: {cool[0]}
        setpoint: 12.8
        cap_ft_curve: {curves[0]}
        cap_fflow_curve: {curves[1]}
        eir_ft_curve: {curves[2]}
        eir_fflow_curve: {curves[3]}
        plf_curve: {curves[4]}
        speeds:
{speed_yaml}    zone_terminals:
      - zone: {zone}
"""


HVAC = """# Curve:Biquadratic / Curve:Quadratic of the DX coils
performance_curves:
  - name: PSZ-AC_Unitary_PackagecoolCapFT
    curve_type: biquadratic
    coefficients: [0.766956, 0.0107756, -0.0000414703, 0.00134961, -0.000261144, 0.000457488]
    min_x: 12.77778
    max_x: 23.88889
    min_y: 21.11111
    max_y: 46.11111
  - name: PSZ-AC_Unitary_PackagecoolEIRFT
    curve_type: biquadratic
    coefficients: [0.297145, 0.0430933, -0.000748766, 0.00597727, 0.000482112, -0.000956448]
    min_x: 12.77778
    max_x: 23.88889
    min_y: 21.11111
    max_y: 46.11111
  - name: PSZ-AC_Unitary_PackagecoolFFF
    curve_type: quadratic
    coefficients: [0.8, 0.2, 0.0]
    min_x: 0.5
    max_x: 1.5
  - name: PSZ-AC_Unitary_PackagecoolEIRFFF
    curve_type: quadratic
    coefficients: [1.156, -0.1816, 0.0256]
    min_x: 0.5
    max_x: 1.5
  - name: PSZ-AC_Unitary_PackagecoolPLR
    curve_type: quadratic
    coefficients: [0.75, 0.25, 0.0]
    min_x: 0.0
    max_x: 1.0

# Air loops (the packaged units are written by psz() from the IDF).
air_loops:
PSZ_LOOPS

  # ZoneHVAC:UnitHeater 'Front_Entry Unit Heater': availability ALWAYS_ON, no
  # outdoor air. Its Fan:ConstantVolume runs at full flow while there is a
  # heating load and stops without one (Fan Operation During No Heating = No);
  # the coil meets the load (E+ UnitHeater, continuous fan). The IDF's EMS
  # program FrontEntry_HeaterControl switches the fan and coil off while the
  # outdoor air is above 45 °F, which is what an
  # AvailabilityManager:HighTemperatureTurnOff on the outdoor air does.
  - name: Front_Entry Unit Heater
    high_temperature_turn_off: 7.222222222222222   # (45 °F - 32) / 1.8
    controls:
      heating_supply_temp: 40.0          # Sizing:Zone heating supply temperature
      cooling_supply_temp: 12.8
      zone_heating_supply_humidity_ratio: 0.008   # Sizing:Zone design supply humidity ratios
      zone_cooling_supply_humidity_ratio: 0.007
      design_zone_flow: autosize
      cycling: on_off
      fan_operating_mode: continuous_no_load_off
    equipment:
      - type: fan                        # Fan:ConstantVolume
        name: Front_Entry Unit HeaterFan
        source: constant_volume
        design_flow_rate: autosize
        pressure_rise: 622.5
        motor_efficiency: 0.895
        impeller_efficiency: 0.65        # total 0.58175 / motor 0.895
        motor_in_airstream_fraction: 1.0
      - type: heating_coil               # Coil:Heating:Fuel
        name: Front_Entry Unit Heater coil
        source: gas
        capacity: autosize
        efficiency: 0.81
        setpoint: 40.0
    zone_terminals:
      - zone: Front_Entry

outputs:
  - file: "zone_results.csv"
    frequency: hourly
    variables:
      - zone:temperature
      - zone:heating_rate
      - zone:cooling_rate
      - zone:infiltration_mass_flow
      - site:outdoor_temperature

summary_report: true"""


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])
