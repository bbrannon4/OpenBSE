#!/usr/bin/env python3
"""Expand zone_multiplier=10 mid-floor zones into explicit per-floor zones.

The Large Office has 6 mid-floor zones (5 office + 1 datacenter) with zone_multiplier=10.
Since OpenBSE doesn't support zone multipliers, we need to create 10 explicit copies
of each zone (floors 2-11), along with their surfaces, equipment, HVAC terminals, etc.

This script reads the YAML, performs the expansion, and writes the result.
"""

import yaml
import copy
import sys


def load_yaml(path):
    with open(path) as f:
        return yaml.safe_load(f)


def save_yaml_manual(data, path):
    """Write YAML with controlled formatting to keep it readable."""
    with open(path, 'w') as f:
        yaml.dump(data, f, default_flow_style=False, sort_keys=False, width=200, allow_unicode=True)


# Mid-floor office zone names (original)
OFFICE_MID_ZONES = ['Core_mid', 'Perimeter_mid_ZN_1', 'Perimeter_mid_ZN_2', 'Perimeter_mid_ZN_3', 'Perimeter_mid_ZN_4']
DC_MID_ZONE = 'DataCenter_mid_ZN_6'
ALL_MID_ZONES = OFFICE_MID_ZONES + [DC_MID_ZONE]

# Floor numbering: floors 2-11 (10 floors)
FLOOR_RANGE = range(2, 12)

# Surface name prefixes that map to mid zones
SURFACE_ZONE_MAP = {
    'Core_mid': 'Core_mid',
    'Perimeter_mid_ZN_1': 'Perimeter_mid_ZN_1',
    'Perimeter_mid_ZN_2': 'Perimeter_mid_ZN_2',
    'Perimeter_mid_ZN_3': 'Perimeter_mid_ZN_3',
    'Perimeter_mid_ZN_4': 'Perimeter_mid_ZN_4',
    'DataCenter_mid_ZN_6': 'DataCenter_mid_ZN_6',
}


def floor_zone_name(base_zone, floor_num):
    """Generate floor-specific zone name: Core_mid_f2, Perimeter_mid_ZN_1_f3, etc."""
    return f"{base_zone}_f{floor_num}"


def floor_surface_name(base_name, floor_num):
    """Generate floor-specific surface name."""
    return f"{base_name}_f{floor_num}"


def expand_zones(data):
    """Expand zone definitions."""
    new_zones = []
    for zone in data['zones']:
        name = zone['name']
        if name in ALL_MID_ZONES:
            # Remove zone_multiplier, create 10 copies
            for fl in FLOOR_RANGE:
                new_zone = copy.deepcopy(zone)
                new_zone['name'] = floor_zone_name(name, fl)
                new_zone.pop('zone_multiplier', None)
                new_zones.append(new_zone)
        else:
            new_zones.append(zone)
    data['zones'] = new_zones


def expand_zone_groups(data):
    """Expand zone groups to include all floor copies."""
    for group in data.get('zone_groups', []):
        new_zone_list = []
        for z in group['zones']:
            if z in ALL_MID_ZONES:
                for fl in FLOOR_RANGE:
                    new_zone_list.append(floor_zone_name(z, fl))
            else:
                new_zone_list.append(z)
        group['zones'] = new_zone_list


def expand_surfaces(data):
    """Duplicate mid-floor surfaces for each floor."""
    new_surfaces = []
    for surf in data['surfaces']:
        zone = surf.get('zone', '')
        if zone in ALL_MID_ZONES:
            for fl in FLOOR_RANGE:
                new_surf = copy.deepcopy(surf)
                new_surf['name'] = floor_surface_name(surf['name'], fl)
                new_surf['zone'] = floor_zone_name(zone, fl)
                # Update parent_surface reference if it exists (for windows)
                if 'parent_surface' in new_surf:
                    new_surf['parent_surface'] = floor_surface_name(new_surf['parent_surface'], fl)
                new_surfaces.append(new_surf)
        else:
            new_surfaces.append(surf)
    data['surfaces'] = new_surfaces


def expand_internal_loads(data, section_name):
    """Expand people/lights/equipment that reference mid zones directly."""
    if section_name not in data:
        return

    new_items = []
    for item in data[section_name]:
        zones = item.get('zones', [])
        has_mid = False
        is_zone_group_ref = False

        # Check if any zone in the list is a mid zone (direct reference)
        for z in zones:
            if z in ALL_MID_ZONES:
                has_mid = True
                break

        # Check if referencing a zone group that contains mid zones
        # Zone groups are handled by expand_zone_groups, so we just need to
        # check if the reference is to a zone group name vs direct zone name
        zone_group_names = [g['name'] for g in data.get('zone_groups', [])]
        for z in zones:
            if z in zone_group_names:
                is_zone_group_ref = True

        if has_mid and not is_zone_group_ref:
            # This item directly references mid zones - need to duplicate
            # Check if it references a single mid zone or multiple
            mid_zones_in_list = [z for z in zones if z in ALL_MID_ZONES]
            non_mid_zones = [z for z in zones if z not in ALL_MID_ZONES]

            if non_mid_zones:
                # Keep original for non-mid zones
                non_mid_item = copy.deepcopy(item)
                non_mid_item['zones'] = non_mid_zones
                new_items.append(non_mid_item)

            # Create per-floor copies for mid zones
            for z in mid_zones_in_list:
                for fl in FLOOR_RANGE:
                    new_item = copy.deepcopy(item)
                    new_item['name'] = f"{item['name']}_f{fl}"
                    new_item['zones'] = [floor_zone_name(z, fl)]
                    new_items.append(new_item)
        else:
            new_items.append(item)

    data[section_name] = new_items


def expand_infiltration(data):
    """Expand infiltration zone lists."""
    if 'infiltration' not in data:
        return

    new_items = []
    for item in data['infiltration']:
        zones = item.get('zones', [])
        mid_zones = [z for z in zones if z in ALL_MID_ZONES]

        if mid_zones:
            new_zone_list = []
            for z in zones:
                if z in ALL_MID_ZONES:
                    for fl in FLOOR_RANGE:
                        new_zone_list.append(floor_zone_name(z, fl))
                else:
                    new_zone_list.append(z)
            item['zones'] = new_zone_list
        new_items.append(item)

    data['infiltration'] = new_items


def expand_outdoor_air(data):
    """Outdoor air uses zone group references, should be handled by zone group expansion."""
    pass  # Zone groups already expanded


def expand_vav_mid_terminals(data):
    """Create 10 separate VAV_mid systems (one per floor) instead of one with 50 zones.

    This better mirrors E+'s zone multiplier behavior where each multiplied "instance"
    gets its own proportional share of the AHU capacity.
    """
    new_loops = []
    for loop in data.get('air_loops', []):
        if loop['name'] == 'VAV_mid':
            for fl in FLOOR_RANGE:
                new_loop = copy.deepcopy(loop)
                new_loop['name'] = f"VAV_mid_f{fl}"
                # Rename equipment
                for eq in new_loop.get('equipment', []):
                    if 'name' in eq:
                        eq['name'] = f"{eq['name']}_f{fl}"
                # Update zone terminals
                new_terminals = []
                for term in new_loop['zone_terminals']:
                    zone = term['zone']
                    if zone in OFFICE_MID_ZONES:
                        new_term = copy.deepcopy(term)
                        new_term['zone'] = floor_zone_name(zone, fl)
                        if 'terminal' in new_term and 'name' in new_term['terminal']:
                            new_term['terminal']['name'] = f"{term['terminal']['name']}_f{fl}"
                        new_terminals.append(new_term)
                new_loop['zone_terminals'] = new_terminals
                new_loops.append(new_loop)
        else:
            new_loops.append(loop)
    data['air_loops'] = new_loops


def expand_dc_mid_airloop(data):
    """Create 10 separate PSZ-AC air loops for DataCenter_mid floors."""
    new_loops = []
    for loop in data.get('air_loops', []):
        if loop['name'] == 'AirLoop_DataCenter_mid':
            for fl in FLOOR_RANGE:
                new_loop = copy.deepcopy(loop)
                new_loop['name'] = f"AirLoop_DataCenter_mid_f{fl}"
                # Rename equipment
                for eq in new_loop.get('equipment', []):
                    if 'name' in eq:
                        eq['name'] = f"{eq['name']}_f{fl}"
                # Update zone terminal
                for term in new_loop.get('zone_terminals', []):
                    if term.get('zone') == DC_MID_ZONE:
                        term['zone'] = floor_zone_name(DC_MID_ZONE, fl)
                new_loops.append(new_loop)
        else:
            new_loops.append(loop)
    data['air_loops'] = new_loops


def expand_thermostats(data):
    """Thermostats use zone group references - already handled by zone group expansion."""
    pass


def expand_dhw(data):
    """DHW loads: Core_mid DHW has flow already scaled by 10x.
    We need to split it into 10 loads at 1/10 the flow each."""
    for dhw in data.get('dhw_systems', []):
        new_loads = []
        for load in dhw.get('loads', []):
            if 'Core_mid' in load.get('name', ''):
                # Split the 10x-scaled flow into 10 individual loads
                base_flow = load['peak_flow_rate'] / 10.0
                for fl in FLOOR_RANGE:
                    new_load = copy.deepcopy(load)
                    new_load['name'] = f"Core_mid_f{fl} DHW"
                    new_load['peak_flow_rate'] = round(base_flow, 5)
                    new_loads.append(new_load)
            else:
                new_loads.append(load)
        dhw['loads'] = new_loads


def main():
    input_path = sys.argv[1] if len(sys.argv) > 1 else 'LargeOffice_Boulder.yaml'
    output_path = sys.argv[2] if len(sys.argv) > 2 else 'LargeOffice_Boulder_expanded.yaml'

    data = load_yaml(input_path)

    expand_zones(data)
    expand_zone_groups(data)
    expand_surfaces(data)
    expand_internal_loads(data, 'equipment')
    expand_internal_loads(data, 'lights')
    # People and lights that use zone groups don't need expansion
    expand_infiltration(data)
    expand_vav_mid_terminals(data)
    expand_dc_mid_airloop(data)
    expand_dhw(data)

    # Write output
    save_yaml_manual(data, output_path)

    # Count zones
    zone_count = len(data['zones'])
    surface_count = len(data['surfaces'])
    terminal_count = sum(len(l.get('zone_terminals', [])) for l in data.get('air_loops', []))
    print(f"Expanded YAML written to {output_path}")
    print(f"  Zones: {zone_count}")
    print(f"  Surfaces: {surface_count}")
    print(f"  Air loop terminals: {terminal_count}")


if __name__ == '__main__':
    main()
