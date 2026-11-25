#!/usr/bin/env python3
"""Write Hospital_STD2022_Denver.yaml from the DOE 90.1-2022 Hospital IDF.

The model is a transcription of `eplus_run/ASHRAE901_Hospital_STD2022_Denver.idf`
(PNNL's prototype, E+ 22.1), object by object, as a person recreating the IDF
in OpenBSE would: E+'s names, vertices, layers, values and schedules. What
OpenBSE cannot express is listed in INPUT_AUDIT.md next to this script.

Zone multipliers: the IDF multiplies 17 of its 55 zones (x4 to x10) on air
loops that also serve single zones, which OpenBSE refuses (#286). Every
surface of a multiplied zone is either outdoors or faces its own zone, so a
multiplied zone is carried over as one zone N times as large: each surface
keeps its orientation and takes N times its area (area/azimuth/tilt, no
vertices), and the volume, floor area, people, equipment, internal mass,
infiltration, exhaust and water use are N times one instance's. The zone's
heat balance per unit area is then the same as E+'s, and the loop sees the
N instances E+ sums.

Rerun after editing either:

    python3 hospital_from_idf.py eplus_run/ASHRAE901_Hospital_STD2022_Denver.idf Hospital_STD2022_Denver.yaml
"""
import math
import sys
from collections import defaultdict

ROUGHNESS = {
    "veryrough": "very_rough", "rough": "rough", "mediumrough": "medium_rough",
    "mediumsmooth": "medium_smooth", "smooth": "smooth", "verysmooth": "very_smooth",
}
CP_WATER = 4180.0          # E+ CPHW(InitConvTemp) [J/kg-K]
RHO_WATER = 999.2          # E+ RhoH2O(InitConvTemp = 5.05 C) [kg/m3], water-use flows
H_FG = 2_454_000.0         # E+ PsyHfgAirFnWTdb at ~22 C [J/kg]
T_ZONE_WATER_USE = 22.5    # zone air for water-use sensible gains: mid deadband (21/24 C)


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


def q(name):
    """YAML-safe scalar for an E+ name."""
    if any(c in name for c in ":#{}[],&*!|>'\"%@`") or name != name.strip():
        return "'" + name.replace("'", "''") + "'"
    return name


# ─── Schedules ────────────────────────────────────────────────────────────────

DAY_TYPES = ("weekday", "saturday", "sunday", "holiday", "summer_dd", "winter_dd")


def compact_profiles(fields):
    """Schedule:Compact (one 'Through: 12/31' period) -> {day type: 24 values}.
    Sub-hourly 'Until' times are averaged over the hour (the IDF's
    Interpolate:Average)."""
    profiles, targets, until = {}, [], []
    assert fields[2].lower().replace(" ", "") == "through:12/31", fields[:3]

    def close():
        if targets:
            vals, prev = [0.0] * 24, 0.0
            for t, v in until:
                for h in range(24):
                    lo, hi = max(prev, h), min(t, h + 1)
                    if hi > lo:
                        vals[h] += v * (hi - lo)
                prev = t
            assert abs(prev - 24) < 1e-9, (fields[0], until)
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
        elif low.startswith("interpolate"):
            i += 1
        elif low.startswith("until"):
            hh, mm = f.split(":", 1)[1].strip().split(":")
            until.append((int(hh) + int(mm) / 60.0, float(fields[i + 1])))
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
    out.append(f"  - name: {q(name)}")
    out.append(f"    weekday:  {flist(prof['weekday'])}")
    if prof["saturday"] != prof["weekday"]:
        out.append(f"    saturday: {flist(prof['saturday'])}")
    if prof["sunday"] != prof["weekday"]:
        out.append(f"    sunday:   {flist(prof['sunday'])}")
    if prof["holiday"] != prof["sunday"]:
        out.append(f"    holiday:  {flist(prof['holiday'])}")
    return out


# ─── Geometry ─────────────────────────────────────────────────────────────────

def newell(verts):
    nx = ny = nz = 0.0
    n = len(verts)
    for i in range(n):
        x1, y1, z1 = verts[i]
        x2, y2, z2 = verts[(i + 1) % n]
        nx += (y1 - y2) * (z1 + z2)
        ny += (z1 - z2) * (x1 + x2)
        nz += (x1 - x2) * (y1 + y2)
    return nx, ny, nz


def poly_area(verts):
    nx, ny, nz = newell(verts)
    return 0.5 * math.sqrt(nx * nx + ny * ny + nz * nz)


def orientation(verts):
    """Outward-normal azimuth (degrees clockwise from north) and tilt."""
    nx, ny, nz = newell(verts)
    mag = math.sqrt(nx * nx + ny * ny + nz * nz)
    tilt = math.degrees(math.acos(max(-1.0, min(1.0, nz / mag))))
    az = math.degrees(math.atan2(nx, ny)) % 360.0 if abs(nx) + abs(ny) > 1e-9 * mag else 0.0
    return round(az, 6) % 360.0, round(tilt, 6)


def point(v):
    return "{x: %s, y: %s, z: %s}" % tuple(fmt(round(c, 9) + 0.0) for c in v)


def zone_of_equipment(idf, name):
    """Zone whose ZoneHVAC:EquipmentList holds the named equipment."""
    for e in idf["zonehvac:equipmentlist"]:
        if any(x.upper() == name.upper() for x in e[2:]):
            return next(c[0] for c in idf["zonehvac:equipmentconnections"] if c[1].upper() == e[0].upper())
    sys.exit(f"no zone holds {name}")


class Model:
    def __init__(self, idf):
        self.idf = idf
        self.by = {}

    def get(self, cls, name):
        if cls not in self.by:
            self.by[cls] = {f[0].upper(): f for f in self.idf[cls]}
        return self.by[cls][name.upper()]


def main(idf_path, out_path):
    idf = parse_idf(idf_path)
    m = Model(idf)
    L = []
    w = L.append

    bldg = idf["building"][0]
    north = float(bldg[1])
    # E+ SurfaceGeometry: Xt = X cos(-N) - Y sin(-N), Yt = X sin(-N) + Y cos(-N)
    cn, sn = math.cos(math.radians(-north)), math.sin(math.radians(-north))

    def rotate(x, y, z):
        return (x * cn - y * sn, x * sn + y * cn, z)

    zones = {}  # upper -> dict
    for f in idf["zone"]:
        zones[f[0].upper()] = dict(name=f[0], origin=(num(f, 2, 0), num(f, 3, 0), num(f, 4, 0)),
                                   mult=int(num(f, 6, 1)), volume=num(f, 8))
    zone_order = [f[0] for f in idf["zone"]]

    def zname(z):
        return zones[z.upper()]["name"]

    def mult(z):
        return zones[z.upper()]["mult"]

    # ---- schedules ----
    compact_raw = {f[0].upper(): f for f in idf["schedule:compact"]}
    const_raw = {f[0].upper(): f for f in idf["schedule:constant"]}
    used_sched = []

    def sched(name):
        """Register a schedule for output; return its IDF name."""
        key = name.upper()
        if key not in compact_raw and key not in const_raw:
            sys.exit(f"schedule {name} not found")
        if key not in [u.upper() for u in used_sched]:
            used_sched.append(compact_raw[key][0] if key in compact_raw else const_raw[key][0])
        return compact_raw[key][0] if key in compact_raw else const_raw[key][0]

    def prof(name):
        key = name.upper()
        if key in const_raw:
            v = float(const_raw[key][2])
            return {d: [v] * 24 for d in DAY_TYPES}
        return compact_profiles(compact_raw[key])

    def const_value(name):
        p = prof(name)
        vals = {v for d in DAY_TYPES for v in p[d]}
        assert len(vals) == 1, (name, vals)
        return vals.pop()

    # ---- surfaces ----
    surf_zone = {f[0].upper(): f[3] for f in idf["buildingsurface:detailed"]}
    surfaces, subsurfaces, used_cons = [], [], set()
    ext_wall_area = defaultdict(float)  # E+ Zone ExtGrossWallArea (one instance)
    floor_area = defaultdict(float)
    cfactor = {f[0].upper(): f for f in idf["construction:cfactorundergroundwall"]}

    def verts_of(f, start, zone):
        ox, oy, oz = zones[zone.upper()]["origin"]
        coords = [float(x) for x in f[start:] if x != ""]
        n = int(coords[0])
        v = [rotate(coords[i] + ox, coords[i + 1] + oy, coords[i + 2] + oz)
             for i in range(1, 3 * n + 1, 3)]
        assert len(v) == n
        return v

    for f in idf["buildingsurface:detailed"]:
        name, stype, cons, zone = f[0], f[1].lower(), f[2], f[3]
        bc, bc_obj = f[5].lower(), f[6]
        verts = verts_of(f, 10, zone)
        area = poly_area(verts)
        used_cons.add(cons)
        s = {"name": name, "zone": zname(zone), "type": stype, "construction": cons,
             "verts": verts, "area": area, "mult": mult(zone)}
        if stype == "floor":
            floor_area[zone.upper()] += area
        if bc == "outdoors":
            s["boundary"] = "outdoor"
            if stype == "wall":
                ext_wall_area[zone.upper()] += area
        elif bc == "groundfcfactormethod":
            assert cons.upper() in cfactor, name
            s["boundary"] = "ground"
        elif bc == "adiabatic":
            s["boundary"] = "adiabatic"
        elif bc == "surface":
            other = surf_zone[bc_obj.upper()]
            s["boundary"] = "!zone '" + zname(other).replace("'", "''") + "'"
            if bc_obj.upper() == name.upper():
                s["self"] = True
            else:
                assert mult(other) == mult(zone), name
                s["adjacent"] = bc_obj
        else:
            sys.exit(f"{name}: boundary {f[5]}")
        surfaces.append(s)

    for f in idf["fenestrationsurface:detailed"]:
        name, kind, cons, base = f[0], f[1].lower(), f[2], f[3]
        zone = surf_zone[base.upper()]
        assert num(f, 7, 1.0) == 1.0 and f[6] == "", name
        verts = verts_of(f, 8, zone)
        used_cons.add(cons)
        subsurfaces.append({"name": name, "zone": zname(zone), "kind": kind, "construction": cons,
                            "parent": base, "verts": verts, "area": poly_area(verts),
                            "mult": mult(zone)})

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
    w("# OpenBSE Model: DOE ASHRAE 90.1-2022 Hospital Prototype (Denver, 5B)")
    w("# ============================================================================")
    w("#")
    w("# Transcribed object by object from PNNL's prototype IDF")
    w("# ASHRAE901_Hospital_STD2022_Denver.idf (E+ 22.1), kept with its E+ results in")
    w("# prototype_tests/hospital/eplus_run/. Generated by")
    w("# prototype_tests/hospital/hospital_from_idf.py; E+ names, vertices (rotated by")
    w(f"# the Building's north axis, {fmt(north)} deg), layers and values are kept.")
    w("# prototype_tests/hospital/INPUT_AUDIT.md lists every IDF object, how it is")
    w("# represented here, and what OpenBSE cannot express.")
    w("#")
    w("# Building: basement + 5 storeys, 22,422 m², 55 zones. Seven VAV reheat air")
    w("# loops and a constant-volume kitchen unit; chilled water from two")
    w("# water-cooled chillers and a heat-recovery chiller, hot water from a gas")
    w("# boiler. The IDF's 17 multiplied zones are each one zone N times as large")
    w("# (module docstring of the generator). Origin at the IDF's, X east, Y north.")
    w("# ============================================================================")
    w("")
    sp = idf["sizing:parameters"][0]
    w("simulation:")
    w(f"  timesteps_per_hour: {int(idf['timestep'][0][0])}            # IDF Timestep")
    w("  start_month: 1")
    w("  start_day: 1")
    w("  end_month: 12")
    w("  end_day: 31")
    w(f"  terrain: {bldg[2].lower()}                    # Building: Terrain")
    assert bldg[5].lower() == "fullinteriorandexterior"
    w("  solar_distribution: full_interior_and_exterior")
    w(f"  heating_sizing_factor: {fmt(num(sp, 0))}       # Sizing:Parameters")
    w(f"  cooling_sizing_factor: {fmt(num(sp, 1))}")
    tol = idf["outputcontrol:reportingtolerances"][0]
    w(f"  unmet_heating_tolerance: {fmt(num(tol, 0))}   # OutputControl:ReportingTolerances (1 F)")
    w(f"  unmet_cooling_tolerance: {fmt(num(tol, 1))}")
    w(f"  sizing_averaging_window: {int(num(sp, 2))}       # Sizing:Parameters timesteps in averaging window")
    w("  auto_cooling_design_days: false  # size on the IDF's two design days only")
    for f in idf.get("runperiodcontrol:daylightsavingtime", []):
        w(f"  daylight_saving_time: {{start: {f[0]}, end: {f[1]}}}   # RunPeriodControl:DaylightSavingTime")
    fc_temps = [float(x) for x in idf["site:groundtemperature:fcfactormethod"][0]]
    w("  # Site:GroundTemperature:FCfactorMethod: the only ground surfaces are the")
    w("  # basement's Construction:CfactorUndergroundWall walls, which see these.")
    w(f"  ground_surface_temperatures: {flist(fc_temps)}")
    w("  # RunPeriodControl:SpecialDays (the run starts on a Sunday, 2017 calendar)")
    w("  holidays:")
    for mo, d, label in [(1, 1, "New Years Day"), (1, 16, "MLK Day"), (2, 20, "Presidents Day"),
                         (5, 29, "Memorial Day"), (7, 4, "Independence Day"),
                         (9, 4, "Labor Day"), (10, 9, "Columbus Day"), (11, 11, "Veterans Day"),
                         (11, 23, "Thanksgiving"), (12, 25, "Christmas")]:
        w(f"    - {{month: {mo}, day: {d}}}   # {label}")
    w("")
    w("weather_files:")
    w('  - "../../weather/USA_CO_Denver-Aurora-Buckley.AFB.724695_TMY3.epw"   # as the E+ run')
    w("")

    # ---- design days ----
    w("# SizingPeriod:DesignDay")
    w("design_days:")
    for f in idf["sizingperiod:designday"]:
        heating = f[3].lower() == "winterdesignday"
        w(f"  - name: {q(f[0])}")
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

    # ---- materials ----
    w("# Material and Material:NoMass used by the constructions")
    w("materials:")
    written, cons_layers, window_cons = set(), {}, {}
    for cname in sorted(used_cons, key=str.upper):
        key = cname.upper()
        if key in cfactor:
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
            w(f"  - name: {q(f[0])}")
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
    # Construction:CfactorUndergroundWall: E+ (HeatBalanceManager
    # CreateFCfactorConstructions) builds a fictitious no-mass insulation layer
    # outside 0.15 m of concrete (k 1.95, rho 2240, cp 900), with
    # R_fic = 1/C + R_soil(H) - 0.15/1.95 and
    # R_soil = 0.0607 + 0.3479 H (0.25 < H < 2.5 m).
    cf_layers = {}
    for f in idf["construction:cfactorundergroundwall"]:
        c, h = num(f, 1), num(f, 2)
        rsoil = 0.12 if h <= 0.25 else (0.92 if h >= 2.5 else 0.0607 + 0.3479 * h)
        rfic = 1.0 / c + rsoil - 0.15 / 1.95
        lname = f"{f[0]} fictitious insulation"
        cf_layers[f[0]] = lname
        w(f"  - name: {q(lname)}   # C {fmt(c)} W/m2-K, height {fmt(h)} m")
        w("    roughness: medium_rough")
        w(f"    thermal_resistance: {rfic:.10g}")
        w("    thermal_absorptance: 0.9")
        w("    solar_absorptance: 0.7")
        w("    visible_absorptance: 0.7")
    w("  - name: FC concrete   # E+ ~FC_Concrete")
    w("    roughness: medium_rough")
    w("    conductivity: 1.95")
    w("    density: 2240.0")
    w("    specific_heat: 900.0")
    w("    thermal_absorptance: 0.9")
    w("    solar_absorptance: 0.7")
    w("    visible_absorptance: 0.7")
    w("")
    w("# Construction (layers outside to inside)")
    w("constructions:")
    for cname, layers in cons_layers.items():
        w(f"  - name: {q(cname)}")
        w("    layers:")
        for lname in layers:
            kind, f = mats[lname.upper()]
            t = num(f, 2) if kind == "mass" else 0.001  # thickness ignored for no-mass
            w(f"      - {{material: {q(f[0])}, thickness: {fmt(t)}}}")
    for cname, lname in cf_layers.items():
        w(f"  - name: {q(cname)}   # Construction:CfactorUndergroundWall")
        w("    layers:")
        w(f"      - {{material: {q(lname)}, thickness: 0.001}}")
        w("      - {material: FC concrete, thickness: 0.15}")
    w("")
    w("# WindowMaterial:SimpleGlazingSystem (U, SHGC, visible transmittance)")
    w("window_constructions:")
    for cname, g in window_cons.items():
        w(f"  - name: {q(cname)}")
        w(f"    u_factor: {fmt(num(g, 1))}")
        w(f"    shgc: {fmt(num(g, 2))}")
        w(f"    visible_transmittance: {fmt(num(g, 3))}")
    w("")

    # ---- zones ----
    masses = defaultdict(list)
    for f in idf["internalmass"]:
        masses[f[2].upper()].append((f[1], float(f[4])))
    humidistat = {f[1].upper(): f for f in idf["zonecontrol:humidistat"]}
    sizing_zone = {f[0].upper(): f for f in idf["sizing:zone"]}
    w("# Zone and InternalMass. Volume as the IDF; a multiplied zone (Zone Multiplier")
    w("# N) is one zone N times one instance: volume, floor area, internal mass.")
    w("# ZoneControl:Humidistat zones carry their RH limits (min and max schedules,")
    w("# both constant).")
    w("zones:")
    for zone in zone_order:
        z = zones[zone.upper()]
        n = z["mult"]
        w(f"  - name: {q(z['name'])}" + (f"   # Zone Multiplier {n}" if n > 1 else ""))
        w(f"    volume: {fmt(z['volume'] * n)}")
        if n > 1:
            w(f"    floor_area: {fmt(round(floor_area[zone.upper()] * n, 6))}")
        sz = sizing_zone[zone.upper()]
        if sz[12].lower() == "designdaywithlimit":
            if num(sz, 14):
                w(f"    minimum_cooling_flow_per_area: {fmt(num(sz, 14))}   # Sizing:Zone DesignDayWithLimit")
            if num(sz, 15):
                w(f"    minimum_cooling_flow: {fmt(num(sz, 15) * n)}   # Sizing:Zone DesignDayWithLimit")
        if zone.upper() in humidistat:
            h = humidistat[zone.upper()]
            w(f"    min_relative_humidity: {fmt(const_value(h[2]))}   # {h[2]}")
            w(f"    max_relative_humidity: {fmt(const_value(h[3]))}   # {h[3]}")
        if masses[zone.upper()]:
            w("    internal_mass:")
            for c, a in masses[zone.upper()]:
                w(f"      - {{construction: {q(c)}, area: {fmt(a * n)}}}")
    w("")

    # ---- people ----
    w("# People: number of people per instance (x N for a multiplied zone),")
    w("# ACTIVITY_SCH 120 W, radiant 0.3, sensible fraction autocalculated.")
    w("people:")
    for f in idf["people"]:
        assert f[3].lower() == "people" and f[9].upper() == "ACTIVITY_SCH"
        n = mult(f[1])
        w(f"  - name: {q(f[0])}")
        w(f"    zones: [{q(zname(f[1]))}]")
        w(f"    count: {fmt(num(f, 4) * n)}")
        w(f"    activity_level: {fmt(const_value('ACTIVITY_SCH'))}")
        w(f"    radiant_fraction: {fmt(num(f, 7))}")
        w(f"    schedule: {q(sched(f[2]))}")
    w("")

    # ---- lights ----
    w("# Lights: Watts/Area; radiant_fraction = E+ Fraction Radiant + Fraction")
    w("# Visible; return air 0.")
    w("lights:")
    for f in idf["lights"]:
        assert f[3].lower() == "watts/area"
        w(f"  - name: {q(f[0])}")
        w(f"    zones: [{q(zname(f[1]))}]")
        w(f"    watts_per_area: {fmt(num(f, 5))}")
        w(f"    radiant_fraction: {fmt(num(f, 8) + num(f, 9))}")
        w(f"    return_air_fraction: {fmt(num(f, 7))}")
        w(f"    schedule: {q(sched(f[2]))}")
    w("")

    # ---- equipment ----
    w("# ElectricEquipment and GasEquipment (EquipmentLevel, x N for a multiplied")
    w("# zone). WaterUse:Equipment zone gains follow the DHW section below.")
    w("equipment:")
    for cls, fuel in (("electricequipment", "electricity"), ("gasequipment", "natural_gas")):
        for f in idf[cls]:
            assert f[3].lower() == "equipmentlevel"
            n = mult(f[1])
            w(f"  - name: {q(f[0])}")
            w(f"    zones: [{q(zname(f[1]))}]")
            w(f"    power: {fmt(num(f, 4) * n)}")
            w(f"    fuel: {fuel}")
            w(f"    radiant_fraction: {fmt(num(f, 8))}")
            w(f"    latent_fraction: {fmt(num(f, 7))}")
            w(f"    lost_fraction: {fmt(num(f, 9))}")
            w(f"    schedule: {q(sched(f[2]))}")
    # WaterUse:Equipment zone gains (E+ WaterUse CalcEquipmentDrainTemp):
    # sensible = f_s * m * cp * (T_mixed - T_zone); moisture = f_l * m (the
    # zone's saturation limit never binds at these flows).
    wu_gain = []
    for f in idf["wateruse:equipment"]:
        if len(f) < 10 or not f[7]:
            continue
        n = mult(f[7])
        mdot = num(f, 2) * RHO_WATER * n
        t_mix = const_value(f[4])
        fs, fl = const_value(f[8]), const_value(f[9])
        sens = fs * mdot * CP_WATER * (t_mix - T_ZONE_WATER_USE)
        lat = fl * mdot * H_FG
        wu_gain.append((f, sens, lat))
        w(f"  - name: {q(f[0] + ' water use gains')}   # f_s {fmt(fs)}, f_l {fmt(fl)}, mixed {fmt(t_mix)} C")
        w(f"    zones: [{q(zname(f[7]))}]")
        w(f"    power: {sens + lat:.6g}")
        w("    fuel: none")
        w("    radiant_fraction: 0.0")
        w(f"    latent_fraction: {lat / (sens + lat):.6g}")
        w(f"    schedule: {q(sched(f[3]))}")
    w("")

    # ---- infiltration ----
    w("# ZoneInfiltration:DesignFlowRate, Flow/ExteriorWallArea: written as the")
    w("# design flow E+ computes (rate x the zone's gross outdoor wall area, x N")
    w("# for a multiplied zone); the values agree with eplusout.eio.")
    w("infiltration:")
    for f in idf["zoneinfiltration:designflowrate"]:
        assert f[3].lower() == "flow/exteriorwallarea"
        a = ext_wall_area[f[1].upper()]
        if a <= 0.0:
            continue
        flow = num(f, 6) * a * mult(f[1])
        w(f"  - name: {q(f[0])}")
        w(f"    zones: [{q(zname(f[1]))}]")
        w(f"    design_flow_rate: {flow:.9f}   # {fmt(num(f, 6))} m3/s-m2 x {a:.3f} m2"
          + (f" x {mult(f[1])}" if mult(f[1]) > 1 else ""))
        w(f"    constant_coefficient: {fmt(num(f, 8, 1.0))}")
        w(f"    temperature_coefficient: {fmt(num(f, 9, 0.0))}")
        w(f"    wind_coefficient: {fmt(num(f, 10, 0.0))}")
        w(f"    wind_squared_coefficient: {fmt(num(f, 11, 0.0))}")
        w(f"    schedule: {q(sched(f[2]))}")
    w("")

    # ---- outdoor air ----
    dsoa = {f[0].upper(): f for f in idf["designspecification:outdoorair"]}
    w("# DesignSpecification:OutdoorAir (Flow/Area) of each zone's Sizing:Zone")
    w("outdoor_air:")
    for f in idf["sizing:zone"]:
        d = dsoa[f[9].upper()]
        assert d[1].lower() == "flow/area"
        w(f"  - name: {q(d[0])}")
        w(f"    zones: [{q(zname(f[0]))}]")
        w(f"    per_area: {fmt(num(d, 3))}")
    w("")

    # ---- exhaust fans ----
    w("# Fan:ZoneExhaust (flow x N for a multiplied zone)")
    w("exhaust_fans:")
    for f in idf["fan:zoneexhaust"]:
        zone = zone_of_equipment(idf, f[0])
        eff, dp, flow = num(f, 2), num(f, 3), num(f, 4)
        w(f"  - name: {q(f[0])}")
        w(f"    zones: [{q(zname(zone))}]")
        w(f"    flow_rate: {fmt(flow * mult(zone))}")
        w(f"    pressure_rise: {fmt(dp)}")
        w(f"    total_efficiency: {fmt(eff)}")
        w(f"    schedule: {q(sched(f[1]))}")
    w("")

    # ---- surfaces ----
    w("# BuildingSurface:Detailed and FenestrationSurface:Detailed. A multiplied")
    w("# zone's surfaces are written by area (x N), azimuth and tilt. Opaque doors")
    w("# are wall subsurfaces of their parent wall. Surfaces whose outside faces")
    w("# their own zone (E+ self-referencing interzone surfaces) face it here too.")
    w("surfaces:")
    for s in surfaces:
        w(f"  - name: {q(s['name'])}")
        w(f"    zone: {q(s['zone'])}")
        w(f"    type: {s['type']}")
        w(f"    construction: {q(s['construction'])}")
        w(f"    boundary: {s['boundary']}")
        if "adjacent" in s:
            w(f"    adjacent_surface: {q(s['adjacent'])}")
        if s["boundary"] not in ("outdoor",):
            w("    sun_exposure: false")
            w("    wind_exposure: false")
        if s["mult"] > 1:
            az, tilt = orientation(s["verts"])
            w(f"    area: {s['area'] * s['mult']:.6f}   # {s['area']:.6f} m2 x {s['mult']}")
            w(f"    azimuth: {fmt(az)}")
            w(f"    tilt: {fmt(tilt)}")
        else:
            w("    vertices:")
            for v in s["verts"]:
                w(f"      - {point(v)}")
    for s in subsurfaces:
        w(f"  - name: {q(s['name'])}")
        w(f"    zone: {q(s['zone'])}")
        w(f"    type: {'wall' if s['kind'] == 'door' else 'window'}")
        w(f"    construction: {q(s['construction'])}")
        w("    boundary: outdoor")
        w(f"    parent_surface: {q(s['parent'])}")
        if s["mult"] > 1:
            az, tilt = orientation(s["verts"])
            w(f"    area: {s['area'] * s['mult']:.6f}   # {s['area']:.6f} m2 x {s['mult']}")
            w(f"    azimuth: {fmt(az)}")
            w(f"    tilt: {fmt(tilt)}")
            # window height for the ISO 15099 interior convection model
            zs = [v[2] for v in s["verts"]]
            w(f"    height: {fmt(round(max(zs) - min(zs), 9))}")
        else:
            w("    vertices:")
            for v in s["verts"]:
                w(f"      - {point(v)}")
    w("")

    # ---- exterior equipment ----
    w("# Exterior:Lights (AstronomicalClock) and Exterior:FuelEquipment")
    w("exterior_equipment:")
    for f in idf["exterior:lights"]:
        assert f[3].lower() == "astronomicalclock"
        w(f"  - name: {q(f[0])}")
        w(f"    power: {fmt(num(f, 2))}")
        w(f"    schedule: {q(sched(f[1]))}")
        w("    fuel: electricity")
        w("    subcategory: Exterior Lights")
        w("    astronomical_clock: true")
    for f in idf["exterior:fuelequipment"]:
        assert f[1].lower() == "electricity"
        w(f"  - name: {q(f[0])}")
        w(f"    power: {fmt(num(f, 3))}")
        w(f"    schedule: {q(sched(f[2]))}")
        w("    fuel: electricity")
        w(f"    subcategory: {q(f[4])}")
    w("")

    # ---- daylighting ----
    rules = idf["globalgeometryrules"][0]
    relative_points = len(rules) < 4 or rules[3].lower() == "relative"
    ref_points = {f[0].upper(): f for f in idf.get("daylighting:referencepoint", [])}
    controls = [f for f in idf.get("daylighting:controls", []) if mult(f[1]) == 1]
    if controls:
        w("# Daylighting:Controls (SplitFlux) and Daylighting:ReferencePoint, points")
        w("# zone-relative in the IDF (zone origin added, rotated). The two controls in")
        w("# multiplied zones (no vertices here) and the glare inputs are left out")
        w("# (INPUT_AUDIT.md).")
        w("daylighting_controls:")
    for f in controls:
        assert f[2].lower() == "splitflux", f[0]
        z = zones[f[1].upper()]
        control = {"continuous": "continuous", "continuousoff": "continuous_off",
                   "stepped": "stepped"}[f[4].lower()]
        w(f"  - name: {q(f[0])}")
        w(f"    zone: {q(z['name'])}")
        w(f"    lighting_control: {control}")
        w(f"    minimum_input_power_fraction: {fmt(num(f, 5))}")
        w(f"    minimum_light_output_fraction: {fmt(num(f, 6))}")
        if control == "stepped":
            w(f"    stepped_control_steps: {int(num(f, 7))}")
        w("    reference_points:")
        for k in range(13, len(f) - 2, 3):
            rp = ref_points[f[k].upper()]
            x, y, zz = num(rp, 2), num(rp, 3), num(rp, 4)
            if relative_points:
                ox, oy, oz = z["origin"]
                x, y, zz = x + ox, y + oy, zz + oz
            x, y, zz = rotate(x, y, zz)
            w(f"      - {{x: {fmt(round(x, 9))}, y: {fmt(round(y, 9))}, z: {fmt(zz)}, "
              f"fraction_of_zone: {fmt(num(f, k + 1))}, illuminance_setpoint: {fmt(num(f, k + 2))}}}"
              f"   # {rp[0]}")
    if controls:
        w("")

    # ---- thermostats ----
    dual = {f[0].upper(): f for f in idf["thermostatsetpoint:dualsetpoint"]}
    groups = defaultdict(list)
    for f in idf["zonecontrol:thermostat"]:
        assert f[3].lower() == "thermostatsetpoint:dualsetpoint"
        assert const_value(f[2]) == 4  # DualSetpoint
        d = dual[f[4].upper()]
        groups[(d[1], d[2])].append(zname(f[1]))
    w("# ZoneControl:Thermostat / ThermostatSetpoint:DualSetpoint (constant 21 / 24 C)")
    w("thermostats:")
    for (hs, cs), zl in groups.items():
        w(f"  - name: All zones Thermostat   # {hs} / {cs}")
        w(f"    zones: [{', '.join(q(z) for z in zl)}]")
        w(f"    heating_setpoint: {fmt(const_value(hs))}")
        w(f"    cooling_setpoint: {fmt(const_value(cs))}")
    w("")

    hvac = Hvac(idf, m, sched, prof, const_value, zname, mult)
    L.extend(hvac.write())

    # ---- schedules (all referenced ones, collected above) ----
    block = ["# Schedule:Compact / Schedule:Constant, hour 1 (00:00-01:00) first. OpenBSE",
             "# falls back saturday/sunday -> weekday and holiday -> sunday; a day type is",
             "# written only where the IDF differs from that fallback. Sub-hourly steps",
             "# are averaged over the hour (Interpolate:Average).",
             "schedules:"]
    for s in used_sched:
        block.extend(schedule_yaml(s, prof(s)))
    block.extend(hvac.extra_schedules)
    block.append("")
    k = L.index("design_days:") - 1
    L[k:k] = block

    L.append("outputs:")
    L.append('  - file: "zone_results.csv"')
    L.append("    frequency: hourly")
    L.append("    variables:")
    for v in ["zone:temperature", "zone:heating_rate", "zone:cooling_rate",
              "zone:humidity_ratio", "site:outdoor_temperature"]:
        L.append(f"      - {v}")
    L.append("")
    L.append("summary_report: true")
    open(out_path, "w").write("\n".join(L) + "\n")
    print(f"{len(surfaces)} surfaces, {len(subsurfaces)} subsurfaces, "
          f"{len(used_sched)} schedules")


# ─── HVAC ─────────────────────────────────────────────────────────────────────

class Hvac:
    """Air loops, plant loops and service water heating from the IDF objects."""

    def __init__(self, idf, m, sched, prof, const_value, zname, mult):
        self.idf, self.m = idf, m
        self.sched, self.prof, self.const_value = sched, prof, const_value
        self.zname, self.mult = zname, mult
        self.extra_schedules = []
        self.curves = {}  # name -> yaml lines

    # -- helpers --
    def curve(self, name):
        """Register an IDF Curve:* by name for performance_curves."""
        key = name.upper()
        if key in self.curves:
            return self.curves[key][0]
        for cls, ctype, nc in (("curve:biquadratic", "biquadratic", 6), ("curve:quadratic", "quadratic", 3),
                               ("curve:cubic", "cubic", 4), ("curve:bicubic", "bicubic", 10)):
            for f in self.idf[cls]:
                if f[0].upper() == key:
                    coeffs = [float(x) for x in f[1:1 + nc]]
                    b = [num(f, 1 + nc + i) for i in range(4 if nc in (6, 10) else 2)]
                    lines = [f"  - name: {q(f[0])}", f"    curve_type: {ctype}",
                             f"    coefficients: {flist(coeffs)}"]
                    lines.append(f"    min_x: {fmt(b[0])}")
                    lines.append(f"    max_x: {fmt(b[1])}")
                    if nc in (6, 10):
                        lines.append(f"    min_y: {fmt(b[2])}")
                        lines.append(f"    max_y: {fmt(b[3])}")
                    lo = num(f, 1 + nc + len(b))
                    hi = num(f, 2 + nc + len(b))
                    if lo is not None:
                        lines.append(f"    min_output: {fmt(lo)}")
                    if hi is not None:
                        lines.append(f"    max_output: {fmt(hi)}")
                    self.curves[key] = (f[0], lines)
                    return f[0]
        sys.exit(f"curve {name} not found")

    def zone_of_terminal(self, adu_name):
        return zone_of_equipment(self.idf, adu_name)

    def write(self):
        L = []
        L += self.air_loops()
        L += self.plant_loops()
        L += self.dhw()
        L.append("# Curve:* used by the chillers and the boiler")
        L.append("performance_curves:")
        for _, lines in self.curves.values():
            L.extend(lines)
        L.extend(self.boiler_curve_lines)
        L.append("")
        return L

    # -- air side --
    def air_loops(self):
        idf, m = self.idf, self.m
        L = ["# AirLoopHVAC: seven VAV reheat loops and the kitchen's constant-volume unit.",
             "# Each loop: AirLoopHVAC:OutdoorAirSystem (heat-recovery wheel where the",
             "# IDF has one), Coil:Cooling:Water, Coil:Heating:Water (sized to zero by",
             "# Sizing:System, so left out: INPUT_AUDIT.md), Fan, humidifier.",
             "air_loops:"]
        # zone -> ADU -> terminal
        adu = {f[0].upper(): f for f in idf["zonehvac:airdistributionunit"]}
        term_zone = {}
        for f in idf["zonehvac:airdistributionunit"]:
            term_zone[f[3].upper()] = self.zone_of_terminal(f[0])
        sizing_zone = {f[0].upper(): f for f in idf["sizing:zone"]}
        for loop in idf["airloophvac"]:
            name = loop[0]
            ss = m.get("sizing:system", name)
            oac = next(f for f in idf["controller:outdoorair"] if f[0].upper() == f"{name}_OA_CONTROLLER".upper())
            br = next(b for b in idf["branch"] if b[0].upper() == f"{name} Air Loop Main Branch".upper())
            comps = [(br[2 + 4 * k], br[3 + 4 * k]) for k in range((len(br) - 2) // 4)]
            oa_sys = next(f for f in idf["airloophvac:outdoorairsystem"] if f[0].upper() == comps[0][1].upper())
            oa_eq = m.get("airloophvac:outdoorairsystem:equipmentlist", oa_sys[2])
            # AirLoopHVAC:OutdoorAirSystem:EquipmentList: name, then (type, name) pairs
            hx = [m.get("heatexchanger:airtoair:sensibleandlatent", oa_eq[2 + 2 * k])
                  for k in range((len(oa_eq) - 1) // 2)
                  if oa_eq[1 + 2 * k].lower().startswith("heatexchanger")]
            # zones on the loop, in the zone splitter's order
            splitter = next(f for f in idf["airloophvac:zonesplitter"] if f[0].upper().startswith(name.upper()))
            outlets = splitter[2:]
            terms = []
            for t in idf["airterminal:singleduct:vav:reheat"]:
                if t[3] in outlets:
                    terms.append(("vav", t))
            for t in idf["airterminal:singleduct:constantvolume:noreheat"]:
                if t[2] in outlets:
                    terms.append(("cv", t))
            terms.sort(key=lambda t: outlets.index(t[1][3] if t[0] == "vav" else t[1][2]))
            econ = oac[7].lower()
            fixed_sat = any(f[2].upper() == "VAV_SAT_SCH" and f[3].upper() == f"{name} Supply Equipment Outlet Node".upper()
                            for f in idf["setpointmanager:scheduled"])
            warmest = any(f[2].upper() == name.upper() for f in idf["setpointmanager:warmest"])
            single_zone = name.upper() == "CAV_KITCHEN"
            L.append(f"  # {name}: {'constant volume' if single_zone else 'VAV reheat'};"
                     f" Controller:OutdoorAir {oac[7]}, {oac[15]} minimum")
            L.append(f"  - name: {name}")
            L.append(f"    system_type: {'psz_ac' if single_zone else 'vav'}")
            L.append("    availability_schedule: " + self.sched("HVACOperationSchd"))
            nc = next(f for f in idf["availabilitymanager:nightcycle"] if f[0].upper().startswith(name.upper()))
            L.append("    night_cycle:                       # AvailabilityManager:NightCycle")
            L.append("      control: cycle_on_any")
            L.append(f"      thermostat_tolerance: {fmt(num(nc, 4))}")
            L.append(f"      cycling_run_time: {int(num(nc, 6))}")
            if not single_zone:
                L.append("    min_vav_fraction: 0.3")
            L.append("    controls:")
            L.append(f"      cooling_supply_temp: {fmt(num(ss, 8))}   # Sizing:System central cooling")
            L.append("      heating_supply_temp: 40.0          # Sizing:Zone heating supply")
            L.append(f"      central_heating_supply_temp: {fmt(num(ss, 9))}   # Sizing:System central heating (sizes reheat)")
            L.append("      design_zone_flow: autosize")
            if single_zone:
                L.append("      cycling: modulating")
                L.append("      fan_operating_mode: continuous")
            else:
                L.append("      cycling: proportional")
                L.append(f"      zone_sum: {'coincident' if ss[10].lower() == 'coincident' else 'non_coincident'}"
                         f"   # Sizing:System {ss[10]}")
                if fixed_sat:
                    L.append("      fixed_cooling_sat: true            # SetpointManager:Scheduled VAV_SAT_SCH")
            # outdoor air
            min_frac_sched = oac[17] if len(oac) > 17 else ""
            mv = oac[19] if len(oac) > 19 else ""
            if name.upper() in ("VAV_1", "VAV_2"):
                # minimum fraction BLDG_OA_FRAC_SCH (0.25 while occupied, else
                # 0), with the 62.1 VRP of Controller:MechanicalVentilation
                # (available on the same schedule) on top
                frac = max(v for d in DAY_TYPES for v in self.prof(min_frac_sched)[d])
                sname = f"{min_frac_sched} normalized"
                p = self.prof(min_frac_sched)
                self.extra_schedules += schedule_yaml(sname, {d: [v / frac for v in p[d]] for d in DAY_TYPES},
                                                      f"{min_frac_sched} / {fmt(frac)}")
                L.append(f"      minimum_damper_position: {fmt(frac)}   # {min_frac_sched}")
                L.append(f"      minimum_oa_schedule: {q(sname)}")
            elif min_frac_sched:
                frac = self.const_value(min_frac_sched)
                L.append(f"      minimum_damper_position: {fmt(frac)}   # {min_frac_sched}")
            else:
                # kitchen: minimum OA flow autosized (the zone's DSOA) x
                # Kitchen_Exhaust_SCH; the zone's outdoor_air gives the flow
                L.append(f"      minimum_oa_schedule: {q(self.sched(oac[16]))}   # x the zone's design OA")
            if econ == "differentialdrybulb":
                L.append("      economizer:")
                L.append("        economizer_type: differential_dry_bulb")
            if mv:
                L.append("    dcv: true                          # Controller:MechanicalVentilation VRP")
            L.append("    equipment:")
            for h in hx:
                L.append(f"      - type: heat_recovery              # HeatExchanger:AirToAir:SensibleAndLatent ({h[17]})")
                L.append(f"        name: {q(h[0])}")
                L.append("        source: wheel")
                L.append(f"        sensible_effectiveness: {fmt(num(h, 3))}")
                L.append(f"        cooling_sensible_effectiveness: {fmt(num(h, 7))}")
                L.append(f"        latent_effectiveness: {fmt(num(h, 4))}")
                L.append(f"        parasitic_power: {fmt(num(h, 15))}")
            fan = None
            for ctype, cname in comps[1:]:
                c = ctype.lower()
                if c == "coil:cooling:water":
                    L.append("      - type: cooling_coil               # Coil:Cooling:Water")
                    L.append(f"        name: {q(cname)}")
                    L.append("        source: chilled_water")
                    L.append("        capacity: autosize")
                    L.append(f"        setpoint: {fmt(num(ss, 8))}")
                    L.append("        plant_loop: CoolSys1")
                elif c == "coil:heating:water":
                    pass  # sized to zero (Sizing:System central heating SAT)
                elif c == "fan:variablevolume":
                    fan = m.get("fan:variablevolume", cname)
                    tot, dp, motor = num(fan, 2), num(fan, 3), num(fan, 8)
                    L.append("      - type: fan                        # Fan:VariableVolume")
                    L.append(f"        name: {q(cname)}")
                    L.append("        source: vav")
                    L.append("        design_flow_rate: autosize")
                    L.append(f"        pressure_rise: {fmt(dp)}")
                    L.append(f"        motor_efficiency: {fmt(motor)}")
                    L.append(f"        impeller_efficiency: {tot / motor:.10g}   # total {fmt(tot)} / motor {fmt(motor)}")
                    L.append(f"        motor_in_airstream_fraction: {fmt(num(fan, 9))}")
                    L.append(f"        vav_coefficients: {flist([num(fan, 10 + i) for i in range(5)])}")
                elif c == "fan:constantvolume":
                    fan = m.get("fan:constantvolume", cname)
                    tot, dp, motor = num(fan, 2), num(fan, 3), num(fan, 5)
                    L.append("      - type: fan                        # Fan:ConstantVolume")
                    L.append(f"        name: {q(cname)}")
                    L.append("        source: constant_volume")
                    L.append("        design_flow_rate: autosize")
                    L.append(f"        pressure_rise: {fmt(dp)}")
                    L.append(f"        motor_efficiency: {fmt(motor)}")
                    L.append(f"        impeller_efficiency: {tot / motor:.10g}   # total {fmt(tot)} / motor {fmt(motor)}")
                    L.append(f"        motor_in_airstream_fraction: {fmt(num(fan, 6))}")
                elif c == "humidifier:steam:electric":
                    h = m.get("humidifier:steam:electric", cname)
                    L.append("      - type: humidifier                 # Humidifier:Steam:Electric")
                    L.append(f"        name: {q(cname)}")
                    L.append(f"        rated_power: {fmt(num(h, 3))}")
                    L.append(f"        rated_capacity: {fmt(num(h, 2))}   # m3/s of water")
                    L.append(f"        standby_power: {fmt(num(h, 5, 0.0))}")
                else:
                    sys.exit(f"{name}: component {ctype}")
            L.append("    zone_terminals:")
            for kind, t in terms:
                zone = term_zone[t[0].upper()]
                L.append(f"      - zone: {q(self.zname(zone))}")
                if kind == "cv":
                    continue
                L.append("        terminal:")
                L.append("          type: vav_box")
                L.append(f"          name: {q(t[0])}")
                L.append("          max_air_flow: autosize")
                if t[5].lower() == "constant":
                    L.append(f"          min_flow_fraction: {fmt(num(t, 6))}   # Constant Minimum Air Flow Fraction")
                else:
                    # Scheduled minimum (OR_MinSA_Sched: 0.2 off-hours, 1.0 in surgery
                    # hours). With no constant fraction, E+ sizes the box on the
                    # mean of the schedule's lowest and highest values (SingleDuct.cc).
                    assert t[5].lower() == "scheduled", t[0]
                    vals = [v for d in DAY_TYPES for v in self.prof(t[8])[d]]
                    L.append(f"          min_flow_fraction: {fmt((min(vals) + max(vals)) / 2)}"
                             f"   # sizing: mean of {t[8]}'s min and max")
                    L.append(f"          min_flow_fraction_schedule: {q(self.sched(t[8]))}")
                L.append("          reheat_type: hot_water")
                L.append("          reheat_capacity: autosize")
                L.append("          plant_loop: HeatSys1")
                L.append(f"          max_reheat_temp: {fmt(num(t, 18))}")
                L.append(f"          max_reheat_fraction: {fmt(num(t, 17))}")
            L.append("")
        return L

    # -- plant --
    def plant_loops(self):
        idf, m = self.idf, self.m
        L = ["# PlantLoop / CondenserLoop. CoolSys1 is the IDF's primary loop (two",
             "# chillers, constant-speed pumps) and secondary loop (variable-speed pump)",
             "# joined by an ideal HeatExchanger:FluidToFluid, as one decoupled loop",
             "# here (the secondary pump on the demand side); the",
             "# heat-recovery chiller recovers its condenser heat to HeatSys1",
             "# (INPUT_AUDIT.md).",
             "plant_loops:"]
        # ---- hot water ----
        hw = m.get("sizing:plant", "HeatSys1")
        oar = m.get("setpointmanager:outdoorairreset", "HeatSys1 Loop Setpoint Manager")
        pump = m.get("pump:variablespeed", "HeatSys1 Pump")
        boiler = m.get("boiler:hotwater", "HeatSys1 Boiler")
        L += ["  - name: HeatSys1",
              f"    design_supply_temp: {fmt(num(hw, 2))}   # Sizing:Plant",
              f"    design_delta_t: {fmt(num(hw, 3))}",
              "    setpoint_reset:                    # SetpointManager:OutdoorAirReset",
              "      type: oa_reset",
              f"      sp_max: {fmt(num(oar, 2))}",
              f"      oa_low: {fmt(num(oar, 3))}",
              f"      sp_min: {fmt(num(oar, 4))}",
              f"      oa_high: {fmt(num(oar, 5))}",
              "    supply_equipment:"]
        L += self.pump_lines(pump, "variable_speed")
        self.boiler_curve_lines = self.boiler_curve()
        L += ["      - type: boiler                   # Boiler:HotWater; efficiency curve from the",
              "        name: HeatSys1 Boiler          # EMS programs SetBoilerPar / SetBoilerCurve",
              f"        capacity: {fmt(num(boiler, 2))}",
              f"        efficiency: {fmt(num(boiler, 3))}",
              f"        design_outlet_temp: {fmt(num(hw, 2))}",
              "        design_water_flow_rate: autosize",
              "        efficiency_curve: HeatSys1 Boiler Efficiency EMS",
              "        flow_mode: leaving_setpoint_modulated", ""]
        # ---- chilled water ----
        cw = m.get("sizing:plant", "CoolSys1_Supply")
        coar = m.get("setpointmanager:outdoorairreset", "CoolSys1 Loop Setpoint Manager")
        L += ["  - name: CoolSys1",
              f"    design_supply_temp: {fmt(num(cw, 2))}   # Sizing:Plant",
              f"    design_delta_t: {fmt(num(cw, 3))}",
              "    # PlantEquipmentOperation:CoolingLoad ranges + EMS PlantPump_EMS_Program:",
              "    # sequential (the heat-recovery chiller shares this loop; INPUT_AUDIT.md)",
              "    staging_mode: sequential",
              "    staging_threshold: 1.0",
              "    setpoint_reset:                    # SetpointManager:OutdoorAirReset",
              "      type: oa_reset",
              f"      sp_max: {fmt(num(coar, 2))}",
              f"      oa_low: {fmt(num(coar, 3))}",
              f"      sp_min: {fmt(num(coar, 4))}",
              f"      oa_high: {fmt(num(coar, 5))}",
              "    supply_equipment:"]
        p1 = m.get("pump:constantspeed", "CoolSys1 Chiller1 Pump")
        L += self.pump_lines(p1, "constant_speed", name="CoolSys1 Chiller Pumps", num_pumps=2)
        # CoolSys1_Demand's pump: the secondary side of the decoupled loop
        L += self.pump_lines(m.get("pump:variablespeed", "CoolSys1 Pump Secondary"), "variable_speed",
                             loop_side="demand")
        hrc = m.get("chiller:electric:eir", "Heat Recovery Chiller")
        L += ["      - type: chiller                  # Chiller:Electric:EIR (heat recovery)",
              f"        name: {q(hrc[0])}",
              "        model: eir",
              f"        capacity: {fmt(num(hrc, 1))}",
              f"        cop: {fmt(num(hrc, 2))}",
              "        condenser_type: water_cooled",
              "        condenser_plant_loop: TowerWaterSys",
              f"        min_plr: {fmt(num(hrc, 10))}",
              f"        min_unloading_ratio: {fmt(num(hrc, 13))}",
              f"        capft_curve: {q(self.curve(hrc[7]))}",
              f"        eirft_curve: {q(self.curve(hrc[8]))}",
              f"        eirfplr_curve: {q(self.curve(hrc[9]))}",
              "        heat_recovery:",
              "          plant_loop: HeatSys1",
              f"          leaving_setpoint: {fmt(self.const_value('Heat Recovery Chiller Setpoint'))}",
              f"          capacity_fraction: {fmt(num(hrc, 30, 1.0))}",
              "          heating_led: true              # EMS BypassHXStatus_Prg (INPUT_AUDIT.md)"]
        for cn in ("CoolSys1 Chiller1", "CoolSys1 Chiller2"):
            c = m.get("chiller:electric:reformulatedeir", cn)
            assert c[9].lower() == "leavingcondenserwatertemperature"
            L += ["      - type: chiller                  # Chiller:Electric:ReformulatedEIR",
                  f"        name: {q(c[0])}",
                  "        model: reformulated_eir",
                  f"        capacity: {fmt(num(c, 1))}",
                  f"        cop: {fmt(num(c, 2))}",
                  "        condenser_type: water_cooled",
                  "        condenser_plant_loop: TowerWaterSys",
                  f"        min_plr: {fmt(num(c, 11))}",
                  f"        min_unloading_ratio: {fmt(num(c, 14))}",
                  f"        capft_curve: {q(self.curve(c[7]))}",
                  f"        eirft_curve: {q(self.curve(c[8]))}",
                  f"        eirfplr_curve: {q(self.curve(c[10]))}"]
        L.append("")
        # ---- condenser ----
        tw = m.get("sizing:plant", "TowerWaterSys")
        tp = m.get("headeredpumps:variablespeed", "TowerWaterSys Pump")
        L += ["  - name: TowerWaterSys",
              f"    design_supply_temp: {fmt(num(tw, 2))}   # Sizing:Plant (condenser)",
              f"    design_delta_t: {fmt(num(tw, 3))}",
              "    supply_equipment:",
              "      - type: pump                     # HeaderedPumps:VariableSpeed",
              f"        name: {q(tp[0])}",
              "        pump_type: variable_speed",
              "        design_flow_rate: autosize",
              f"        design_head: {fmt(num(tp, 6))}",
              f"        motor_efficiency: {fmt(num(tp, 8))}",
              f"        impeller_efficiency: {1.0 / float(tp[-1]):.10g}   # 1 / shaft power per flow per head {tp[-1]}",
              f"        motor_heat_to_fluid_fraction: {fmt(num(tp, 9))}",
              f"        num_pumps: {int(num(tp, 4))}",
              f"        power_curve: {flist([num(tp, 10 + i) for i in range(4)])}"]
        for t in idf["coolingtower:variablespeed"]:
            fc = m.get("curve:cubic", t[11])
            L += ["      - type: cooling_tower            # CoolingTower:VariableSpeed (CoolToolsCrossFlow)",
                  f"        name: {q(t[0])}",
                  "        tower_type: variable_speed",
                  f"        design_water_flow: {fmt(num(t, 8))}",
                  f"        design_fan_power: {fmt(num(t, 10))}",
                  f"        design_range: {fmt(num(t, 7))}",
                  f"        fan_power_curve: {flist([float(x) for x in fc[1:5]] + [0.0])}   # {fc[0]}"]
        L.append("")
        return L

    def pump_lines(self, p, kind, name=None, num_pumps=None, loop_side=None):
        head = num(p, 4)
        motor = num(p, 6)
        shaft = float(p[-1])  # Design Shaft Power per Unit Flow Rate per Unit Head
        L = [f"      - type: pump                     # Pump:{'VariableSpeed' if kind == 'variable_speed' else 'ConstantSpeed'}",
             f"        name: {q(name or p[0])}",
             f"        pump_type: {kind}",
             "        design_flow_rate: autosize",
             f"        design_head: {fmt(head)}",
             f"        motor_efficiency: {fmt(motor)}",
             f"        impeller_efficiency: {1.0 / shaft:.10g}   # 1 / shaft power per flow per head {fmt(shaft)}",
             f"        motor_heat_to_fluid_fraction: {fmt(num(p, 7))}"]
        if num_pumps:
            L.append(f"        num_pumps: {num_pumps}")
        if loop_side:
            L.append(f"        loop_side: {loop_side}")
        if kind == "variable_speed":
            L.append(f"        power_curve: {flist([num(p, 8 + i) for i in range(4)])}")
        return L

    def boiler_curve(self):
        """The IDF's EMS programs SetBoilerPar and SetBoilerCurve replace the
        boiler efficiency curve every iteration with a function of the part-load
        ratio alone, so it is a fixed curve f(PLR), written as a table."""
        cap = 1549200.94 * 3.412 / 1000.0  # kBtu/h, SetBoilerPar
        plrx = 0.2 if cap > 10000 else (0.25 if cap > 5000 else 0.33)
        cubic = lambda p: 0.975 + 0.305 * p - 0.527 * p * p + 0.249 * p ** 3
        et, skin = 0.945, 0.02
        # Ecx = (LoadX + SkinEx) / InputEx at PLRx (SetBoilerPar)
        ecx = (plrx + skin) * cubic(plrx) * et / plrx
        def f(p):
            if p >= plrx:
                return cubic(p)
            pp = max(p, 0.05)
            return (pp / (pp + skin)) * (ecx / et)
        xs = [0.0, 0.01, 0.02, 0.03, 0.04, 0.05] + [round(0.05 + 0.025 * k, 3) for k in range(1, 39)]
        xs = sorted(set(xs + [plrx]))
        ys = [f(max(x, 1e-9)) for x in xs]
        return ["  # SetBoilerPar / SetBoilerCurve (EMS): rated 5,286 kBtu/h, so BoilerType 1",
                f"  # with PLRx {plrx}; above PLRx the cubic, below it skin losses",
                "  # PLR / (PLR + 0.02) x Ecx/Et, PLR held at 0.05 below 0.05; the table",
                "  # interpolates linearly between these points (0.025 apart above 0.05).",
                "  - name: HeatSys1 Boiler Efficiency EMS",
                "    table:",
                "      axes:",
                f"        - {{variable: part_load_ratio, values: {flist(xs)}}}",
                f"      values: {flist([round(y, 6) for y in ys])}"]

    # -- service water --
    def dhw(self):
        idf, m = self.idf, self.m
        mains = idf["site:watermainstemperature"][0]
        L = ["# Service water heating. SWHSys1: WaterHeater:Mixed on a plant loop with",
             "# its circulation pump, serving the WaterUse:Equipment (x N for a",
             "# multiplied zone; target 60 C, all hot). The booster and laundry heaters",
             "# are stand-alone (Peak Use Flow Rate on the tank).",
             "dhw_systems:"]
        mains_yaml = ["    mains_temperature:", "      correlation:",
                      f"        annual_avg_temp: {fmt(num(mains, 2))}",
                      f"        max_monthly_delta: {fmt(num(mains, 3))}"]
        wh = m.get("waterheater:mixed", "SWHSys1 Water Heater")
        pump = m.get("pump:constantspeed", "SWHSys1 Pump")
        L += ["  - name: SWHSys1"] + mains_yaml + self.wh_lines(wh)
        L += ["    pump:",
              f"      name: {q(pump[0])}",
              "      pump_type: constant_speed",
              "      design_flow_rate: autosize",
              f"      design_head: {fmt(num(pump, 4))}",
              f"      motor_efficiency: {fmt(num(pump, 6))}",
              f"      motor_heat_to_fluid_fraction: {fmt(num(pump, 7))}",
              "    loads:"]
        for f in idf["wateruse:equipment"]:
            n = self.mult(f[7]) if len(f) > 7 and f[7] else 1
            L += [f"      - name: {q(f[0])}",
                  f"        peak_flow_rate: {num(f, 2) * 1000 * n:.9g}   # L/s" + (f" (x {n})" if n > 1 else ""),
                  f"        schedule: {q(self.sched(f[3]))}",
                  f"        use_temperature: {fmt(self.const_value(f[4]))}"]
        for wn, inlet in (("SWH Booster E Water Heater", "Dishwashing Booster Water Inlet Temp Schedule"),
                          ("SWH Laundry Water Heater", None)):
            wh = m.get("waterheater:mixed", wn)
            L += [f"  - name: {q(wn)}"]
            if inlet:
                L += [f"    mains_temperature: {fmt(self.const_value(inlet))}   # {inlet} (from SWHSys1)"]
            else:
                L += mains_yaml
            L += self.wh_lines(wh)
            L += ["    loads:",
                  f"      - name: {q(wn + ' use')}",
                  f"        peak_flow_rate: {num(wh, 27) * 1000:.9g}   # L/s",
                  f"        schedule: {q(self.sched(wh[28]))}",
                  f"        use_temperature: {fmt(self.const_value(wh[2]))}"]
        L.append("")
        return L

    def wh_lines(self, wh):
        fuel = "gas" if wh[10].lower() == "naturalgas" else "electric"
        par = num(wh, 13, 0.0) if wh[14].lower() == "naturalgas" or fuel == "electric" else 0.0
        return ["    water_heater:",
                f"      name: {q(wh[0])}",
                f"      fuel_type: {fuel}",
                f"      tank_volume: {num(wh, 1) * 1000:.7g}   # L",
                f"      capacity: {fmt(num(wh, 6))}",
                f"      efficiency: {fmt(num(wh, 11))}",
                f"      setpoint: {fmt(self.const_value(wh[2]))}   # {wh[2]}",
                f"      deadband: {num(wh, 3):.6g}",
                f"      ua_standby: {fmt(num(wh, 23))}",
                f"      parasitic_power: {fmt(par)}",
                f"      ambient_zone: {q(self.zname(wh[21]))}"]


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])
