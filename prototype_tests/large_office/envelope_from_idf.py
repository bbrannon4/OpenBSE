#!/usr/bin/env python3
"""Rebuild the Large Office envelope in the OpenBSE model from the E+ IDF.

Replaces `materials`, `constructions`, `simple_constructions`,
`window_constructions` and `surfaces`, and each zone's `internal_mass`, with a
direct translation of the reference IDF (eplus_run/in.idf), so the geometry,
layers and boundary conditions are E+'s own:

- Every BuildingSurface:Detailed, FenestrationSurface:Detailed and
  InternalMass of a modelled zone, with E+'s names and vertices (zone origins
  added; the data centre zones have non-zero origins).
- Zones with an E+ multiplier of 10 are expanded into floors _f2.._f11, each
  an exact copy at E+'s coordinates; interzone partners map to the same floor.
- Plenum zones are left out: every surface between them and the occupied
  zones is adiabatic in the IDF, so they exchange no heat with the building.
- Boundaries: Outdoors -> outdoor, GroundFCfactorMethod -> ground,
  Adiabatic and self-referencing Surface -> adiabatic (E+ stores Adiabatic as
  a surface facing itself; OpenBSE's adiabatic is the same T_out = T_in),
  Surface -> !zone <partner's zone>.
- Construction:CfactorUndergroundWall -> the two layers E+ builds internally
  (HeatBalanceManager CreateFCfactorConstructs, Material.cc ~FC_Concrete): a
  resistance-only layer R = 1/C + R_soil(H) - R_concrete outside 0.15 m of
  concrete (k 1.95, rho 2240, cp 900).
- Door subsurfaces -> wall surfaces with parent_surface (area comes off the wall).
- Material:NoMass -> material with thermal_resistance.
- WindowMaterial:SimpleGlazingSystem -> window construction (U, SHGC, VT).

usage: envelope_from_idf.py <in.idf> <model.yaml>   (rewrites model.yaml)
"""
import sys
from collections import defaultdict

from ruamel.yaml import YAML
from ruamel.yaml.comments import CommentedMap, CommentedSeq, TaggedScalar

FLOORS = range(2, 12)
ROUGHNESS = {
    "veryrough": "very_rough", "rough": "rough", "mediumrough": "medium_rough",
    "mediumsmooth": "medium_smooth", "smooth": "smooth", "verysmooth": "very_smooth",
}


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
            objs[fields[0].lower()].append(fields[1:])
            cur = []
    return objs


def num(fields, i, default):
    return float(fields[i]) if len(fields) > i and fields[i] != "" else default


def c_factor_layers(c_factor, height):
    """E+ CreateFCfactorConstructs: R_fic = 1/C + R_soil(H) - R_concrete."""
    if height <= 0.25:
        r_soil = 0.12
    elif height >= 2.5:
        r_soil = 0.92
    else:
        r_soil = 0.0607 + 0.3479 * height
    return 1.0 / c_factor + r_soil - 0.15 / 1.95


def point(x, y, z):
    p = CommentedMap()
    p["x"], p["y"], p["z"] = round(x, 6), round(y, 6), round(z, 6)
    return p


def main(idf_path, model_path):
    idf = parse_idf(idf_path)
    yaml = YAML(typ="rt")
    yaml.width = 200
    model = yaml.load(open(model_path))

    ob_zones = {z["name"]: z for z in model["zones"]}
    zone_info = {}  # E+ zone (upper) -> (name, multiplier, origin)
    for f in idf["zone"]:
        name = f[0]
        origin = (num(f, 2, 0.0), num(f, 3, 0.0), num(f, 4, 0.0))
        zone_info[name.upper()] = (name, int(num(f, 6, 1.0)), origin)

    def copies(ep_zone):
        """OpenBSE zone names for an E+ zone: itself, or its _f2.._f11 floors."""
        name, mult, _ = zone_info[ep_zone.upper()]
        if mult == 1:
            return [(name, "")] if name in ob_zones else []
        assert mult == len(FLOORS), f"{name}: multiplier {mult}"
        floors = [(f"{name}_f{k}", f"_f{k}") for k in FLOORS]
        return floors if all(n in ob_zones for n, _ in floors) else []

    modelled = {z for z in zone_info if copies(z)}

    # ---- surfaces ----
    surf_zone = {f[0].upper(): f[3] for f in idf["buildingsurface:detailed"]}
    used_constructions = set()
    surfaces = CommentedSeq()
    for f in idf["buildingsurface:detailed"]:
        name, stype, cons, zone = f[0], f[1].lower(), f[2], f[3]
        if zone.upper() not in modelled:
            continue
        bc, bc_obj = f[5].lower(), f[6]
        sun = f[7].lower() == "sunexposed"
        wind = f[8].lower() == "windexposed"
        ox, oy, oz = zone_info[zone.upper()][2]
        coords = [float(x) for x in f[11:] if x != ""]
        verts = [coords[i:i + 3] for i in range(0, len(coords), 3)]
        used_constructions.add(cons)
        partner_zone = None
        if bc == "surface" and bc_obj.upper() != name.upper():
            partner_zone = surf_zone[bc_obj.upper()]
            if partner_zone.upper() not in modelled:
                sys.exit(f"{name}: partner zone {partner_zone} is not modelled")
            if zone_info[partner_zone.upper()][1] != zone_info[zone.upper()][1]:
                sys.exit(f"{name}: partner zone {partner_zone} has a different multiplier")
        for ob_zone, suffix in copies(zone):
            s = CommentedMap()
            s["name"] = name + suffix
            s["zone"] = ob_zone
            s["type"] = stype
            s["construction"] = cons
            if bc == "outdoors":
                s["boundary"] = "outdoor"
            elif bc in ("groundfcfactormethod", "ground"):
                s["boundary"] = "ground"
            elif bc == "adiabatic" or (bc == "surface" and partner_zone is None):
                s["boundary"] = "adiabatic"
            elif bc == "surface":
                s["boundary"] = CommentedMap({"zone": partner_zone + suffix})
            else:
                sys.exit(f"{name}: unsupported boundary {f[5]}")
            if bc != "outdoors":
                s["sun_exposure"] = sun
                s["wind_exposure"] = wind
            s["vertices"] = CommentedSeq(point(x + ox, y + oy, z + oz) for x, y, z in verts)
            surfaces.append(s)

    for f in idf["fenestrationsurface:detailed"]:
        name, cons, base = f[0], f[2], f[3]
        zone = surf_zone[base.upper()]
        if zone.upper() not in modelled:
            continue
        kind = f[1].lower()
        if kind not in ("window", "glassdoor", "door"):
            sys.exit(f"{name}: fenestration type {f[1]} not handled")
        if num(f, 7, 1.0) != 1.0:
            sys.exit(f"{name}: window multiplier not handled")
        ox, oy, oz = zone_info[zone.upper()][2]
        coords = [float(x) for x in f[9:] if x != ""]
        verts = [coords[i:i + 3] for i in range(0, len(coords), 3)]
        used_constructions.add(cons)
        for ob_zone, suffix in copies(zone):
            s = CommentedMap()
            s["name"] = name + suffix
            s["zone"] = ob_zone
            # An opaque door is a wall surface with a parent, which takes its
            # area out of the parent wall.
            s["type"] = "wall" if kind == "door" else "window"
            s["construction"] = cons
            s["boundary"] = "outdoor"
            s["parent_surface"] = base + suffix
            s["vertices"] = CommentedSeq(point(x + ox, y + oy, z + oz) for x, y, z in verts)
            surfaces.append(s)

    # ---- internal mass ----
    masses = defaultdict(list)
    for f in idf["internalmass"]:
        if f[2].upper() in modelled:
            masses[f[2].upper()].append((f[1], float(f[4])))
            used_constructions.add(f[1])
    for ep_zone in modelled:
        for ob_zone, _ in copies(ep_zone):
            z = ob_zones[ob_zone]
            if ep_zone in masses:
                z["internal_mass"] = CommentedSeq(
                    CommentedMap({"construction": c, "area": round(a, 4)}) for c, a in masses[ep_zone])
            elif "internal_mass" in z:
                del z["internal_mass"]

    # ---- constructions and materials ----
    materials_ep = {}
    for f in idf["material"]:
        m = CommentedMap()
        m["name"] = f[0]
        m["roughness"] = ROUGHNESS[f[1].lower()]
        m["conductivity"] = float(f[3])
        m["density"] = float(f[4])
        m["specific_heat"] = float(f[5])
        m["thermal_absorptance"] = num(f, 6, 0.9)
        m["solar_absorptance"] = num(f, 7, 0.7)
        m["visible_absorptance"] = num(f, 8, 0.7)
        materials_ep[f[0].upper()] = (m, float(f[2]))
    for f in idf["material:nomass"]:
        m = CommentedMap()
        m["name"] = f[0]
        m["roughness"] = ROUGHNESS[f[1].lower()]
        m["thermal_resistance"] = float(f[2])
        m["thermal_absorptance"] = num(f, 3, 0.9)
        m["solar_absorptance"] = num(f, 4, 0.7)
        m["visible_absorptance"] = num(f, 5, 0.7)
        materials_ep[f[0].upper()] = (m, 0.001)  # thickness is ignored for no-mass layers
    glazing = {f[0].upper(): f for f in idf["windowmaterial:simpleglazingsystem"]}

    constructions, windows, materials, seen_mat = CommentedSeq(), CommentedSeq(), CommentedSeq(), set()

    def use_material(key):
        m, thickness = materials_ep[key]
        if key not in seen_mat:
            seen_mat.add(key)
            materials.append(m)
        return thickness

    cons_by_name = {f[0].upper(): f for f in idf["construction"]}
    cfactor = {f[0].upper(): f for f in idf["construction:cfactorundergroundwall"]}
    for cname in sorted(used_constructions, key=str.upper):
        key = cname.upper()
        if key in cfactor:
            f = cfactor[key]
            r_fic = c_factor_layers(float(f[1]), float(f[2]))
            fic = CommentedMap()
            fic["name"] = f"~FC_Insulation {f[0]}"
            fic["roughness"] = "medium_rough"
            fic["thermal_resistance"] = round(r_fic, 6)
            fic["thermal_absorptance"] = 0.9
            fic["solar_absorptance"] = 0.0
            fic["visible_absorptance"] = 0.0
            materials_ep[fic["name"].upper()] = (fic, 0.001)
            if "~FC_CONCRETE" not in materials_ep:
                conc = CommentedMap()
                conc["name"] = "~FC_Concrete"
                conc["roughness"] = "medium_rough"
                conc["conductivity"] = 1.95
                conc["density"] = 2240.0
                conc["specific_heat"] = 900.0
                conc["thermal_absorptance"] = 0.9
                conc["solar_absorptance"] = 0.7
                conc["visible_absorptance"] = 0.7
                materials_ep["~FC_CONCRETE"] = (conc, 0.15)
            layers = [fic["name"].upper(), "~FC_CONCRETE"]
        else:
            layers = [l.upper() for l in cons_by_name[key][1:] if l]
        if len(layers) == 1 and layers[0] in glazing:
            g = glazing[layers[0]]
            w = CommentedMap()
            w["name"] = cname
            w["u_factor"] = float(g[1])
            w["shgc"] = float(g[2])
            w["visible_transmittance"] = num(g, 3, 0.6)
            windows.append(w)
            continue
        c = CommentedMap()
        c["name"] = cname
        c["layers"] = CommentedSeq()
        for layer in layers:
            t = use_material(layer)
            c["layers"].append(CommentedMap({"material": materials_ep[layer][0]["name"], "thickness": t}))
        constructions.append(c)

    model["materials"] = materials
    model["constructions"] = constructions
    if "simple_constructions" in model:
        del model["simple_constructions"]
    model["window_constructions"] = windows
    model["surfaces"] = surfaces

    # Interzone boundaries are written the way OpenBSE expects: !zone 'name'.
    for s in surfaces:
        b = s["boundary"]
        if isinstance(b, CommentedMap):
            s["boundary"] = TaggedScalar(value=b["zone"], style="'", tag="!zone")

    yaml.dump(model, open(model_path, "w"))
    n_iz = sum(1 for s in surfaces if not isinstance(s["boundary"], str))
    print(f"{len(surfaces)} surfaces ({n_iz} interzone), {len(constructions)} constructions, "
          f"{len(windows)} window constructions, {len(materials)} materials")


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])
