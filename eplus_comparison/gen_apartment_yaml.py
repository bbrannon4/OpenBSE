#!/usr/bin/env python3
"""Generate OpenBSE YAML for DOE Prototype Mid-Rise Apartment."""
import yaml

# ─── Building geometry constants ───────────────────────────────────────────
APT_W = 11.582    # apartment width (x)
APT_D = 7.620     # apartment depth (y)
COR_W = 46.328    # corridor full width
COR_D = 1.676     # corridor depth
H = 3.048         # floor height

# Zone x-bounds by column
COL_X = [0, APT_W, 2*APT_W, 3*APT_W, 4*APT_W]  # [0, 11.582, 23.164, 34.746, 46.328]

# Zone y-bounds
Y_S = 0.0        # south row bottom
Y_SC = APT_D     # south row top / corridor bottom = 7.620
Y_CN = APT_D + COR_D  # corridor top / north row bottom = 9.296
Y_N = APT_D + COR_D + APT_D  # north row top = 16.916

# Floor z-bounds
FLOORS = {
    'G': (0.0, H),
    'M': (H, 2*H),
    'T': (3*H, 4*H),  # 9.144 to 12.192
}

# Window parameters (relative to zone origin)
WIN_SILL = 0.954
WIN_HEAD = 2.173
S_WIN_X1 = 2.895   # south window x offset from zone x_min
S_WIN_X2 = 8.686   # south window x offset from zone x_min
W_WIN_Y1 = 1.868   # west/east window y offset from zone y_min
W_WIN_Y2 = 5.678

# ─── Zone definitions ─────────────────────────────────────────────────────
# (name_suffix, col_idx, row, ext_walls, schedule_type)
# ext_walls: list of cardinal directions that are exterior
# schedule_type: 'working' or 'stayhome' or 'office' or 'corridor'
SOUTH_ZONES = [
    ('SW Apt', 0, 'S', ['south', 'west'], 'working'),
    ('S1 Apt', 1, 'S', ['south'], 'working'),
    ('S2 Apt', 2, 'S', ['south'], 'working'),
    ('SE Apt', 3, 'S', ['south', 'east'], 'working'),  # Office on G floor
]
NORTH_ZONES = [
    ('NW Apt', 0, 'N', ['north', 'west'], 'stayhome'),
    ('N1 Apt', 1, 'N', ['north'], 'stayhome'),
    ('N2 Apt', 2, 'N', ['north'], 'stayhome'),
    ('NE Apt', 3, 'N', ['north', 'east'], 'stayhome'),
]

APT_AREA = APT_W * APT_D   # 88.249 m2
APT_VOL = APT_AREA * H     # 268.95 m3
COR_AREA = COR_W * COR_D   # 77.66 m2
COR_VOL = COR_AREA * H     # 236.70 m3


def wall_vertices(direction, x1, x2, y1, y2, z1, z2):
    """Return 4 vertices for a wall surface in OpenBSE convention."""
    if direction == 'south':  # at y=y1
        return [
            {'x': x1, 'y': y1, 'z': z1}, {'x': x2, 'y': y1, 'z': z1},
            {'x': x2, 'y': y1, 'z': z2}, {'x': x1, 'y': y1, 'z': z2},
        ]
    elif direction == 'north':  # at y=y2
        return [
            {'x': x2, 'y': y2, 'z': z1}, {'x': x1, 'y': y2, 'z': z1},
            {'x': x1, 'y': y2, 'z': z2}, {'x': x2, 'y': y2, 'z': z2},
        ]
    elif direction == 'west':  # at x=x1
        return [
            {'x': x1, 'y': y2, 'z': z1}, {'x': x1, 'y': y1, 'z': z1},
            {'x': x1, 'y': y1, 'z': z2}, {'x': x1, 'y': y2, 'z': z2},
        ]
    elif direction == 'east':  # at x=x2
        return [
            {'x': x2, 'y': y1, 'z': z1}, {'x': x2, 'y': y2, 'z': z1},
            {'x': x2, 'y': y2, 'z': z2}, {'x': x2, 'y': y1, 'z': z2},
        ]


def roof_vertices(x1, x2, y1, y2, z):
    return [
        {'x': x1, 'y': y1, 'z': z}, {'x': x2, 'y': y1, 'z': z},
        {'x': x2, 'y': y2, 'z': z}, {'x': x1, 'y': y2, 'z': z},
    ]


def floor_vertices(x1, x2, y1, y2, z):
    return [
        {'x': x1, 'y': y2, 'z': z}, {'x': x2, 'y': y2, 'z': z},
        {'x': x2, 'y': y1, 'z': z}, {'x': x1, 'y': y1, 'z': z},
    ]


def window_vertices_on_wall(direction, x1, x2, y1, y2, z_bot):
    """Return window vertices for a given wall direction."""
    zs = z_bot + WIN_SILL
    zh = z_bot + WIN_HEAD
    if direction == 'south':
        wx1 = x1 + S_WIN_X1
        wx2 = x1 + S_WIN_X2
        return [
            {'x': wx1, 'y': y1, 'z': zs}, {'x': wx2, 'y': y1, 'z': zs},
            {'x': wx2, 'y': y1, 'z': zh}, {'x': wx1, 'y': y1, 'z': zh},
        ]
    elif direction == 'north':
        wx1 = x1 + S_WIN_X1
        wx2 = x1 + S_WIN_X2
        return [
            {'x': wx2, 'y': y2, 'z': zs}, {'x': wx1, 'y': y2, 'z': zs},
            {'x': wx1, 'y': y2, 'z': zh}, {'x': wx2, 'y': y2, 'z': zh},
        ]
    elif direction == 'west':
        wy1 = y1 + W_WIN_Y1
        wy2 = y1 + W_WIN_Y2
        return [
            {'x': x1, 'y': wy2, 'z': zs}, {'x': x1, 'y': wy1, 'z': zs},
            {'x': x1, 'y': wy1, 'z': zh}, {'x': x1, 'y': wy2, 'z': zh},
        ]
    elif direction == 'east':
        wy1 = y1 + W_WIN_Y1
        wy2 = y1 + W_WIN_Y2
        return [
            {'x': x2, 'y': wy1, 'z': zs}, {'x': x2, 'y': wy2, 'z': zs},
            {'x': x2, 'y': wy2, 'z': zh}, {'x': x2, 'y': wy1, 'z': zh},
        ]


def r(v, decimals=3):
    """Round a float for YAML output."""
    return round(v, decimals)


def gen_zone_surfaces(zone_name, floor_key, col_idx, row, ext_walls, is_corridor=False):
    """Generate surfaces for a zone."""
    z_bot, z_top = FLOORS[floor_key]
    surfaces = []

    if is_corridor:
        x1, x2, y1, y2 = 0.0, COR_W, Y_SC, Y_CN
    elif row == 'S':
        x1, x2 = COL_X[col_idx], COL_X[col_idx + 1]
        y1, y2 = Y_S, Y_SC
    else:  # N
        x1, x2 = COL_X[col_idx], COL_X[col_idx + 1]
        y1, y2 = Y_CN, Y_N

    # Round coordinates
    x1, x2 = r(x1), r(x2)
    y1, y2 = r(y1), r(y2)
    z_bot, z_top = r(z_bot), r(z_top)

    wall_cons_ext = 'Res Exterior Wall' if not is_corridor else 'Nonres Exterior Wall'

    all_dirs = ['south', 'north', 'west', 'east']
    for d in all_dirs:
        is_ext = d in ext_walls
        wall_name = f"{zone_name} {d.title()} Wall"
        surf = {
            'name': wall_name,
            'zone': zone_name,
            'type': 'wall',
            'construction': wall_cons_ext if is_ext else 'Interior Partition',
            'boundary': 'outdoor' if is_ext else 'adiabatic',
            'vertices': wall_vertices(d, x1, x2, y1, y2, z_bot, z_top),
        }
        surfaces.append(surf)

        # Window on exterior walls (not on corridor)
        if is_ext and not is_corridor:
            win_name = f"{zone_name} {d.title()} Window"
            win = {
                'name': win_name,
                'zone': zone_name,
                'type': 'window',
                'construction': 'Res Window',
                'boundary': 'outdoor',
                'parent_surface': wall_name,
                'vertices': window_vertices_on_wall(d, x1, x2, y1, y2, z_bot),
            }
            surfaces.append(win)

    # Floor
    if floor_key == 'G':
        floor_cons = 'Ground Floor Slab'
        floor_bnd = 'ground'
    else:
        floor_cons = 'Interzone Floor'
        floor_bnd = 'adiabatic'
    surfaces.append({
        'name': f"{zone_name} Floor",
        'zone': zone_name,
        'type': 'floor',
        'construction': floor_cons,
        'boundary': floor_bnd,
        'vertices': floor_vertices(x1, x2, y1, y2, z_bot),
    })

    # Ceiling/Roof
    if floor_key == 'T':
        roof_cons = 'Res Roof' if not is_corridor else 'Nonres Roof'
        surfaces.append({
            'name': f"{zone_name} Roof",
            'zone': zone_name,
            'type': 'roof',
            'construction': roof_cons,
            'boundary': 'outdoor',
            'vertices': roof_vertices(x1, x2, y1, y2, z_top),
        })
    else:
        surfaces.append({
            'name': f"{zone_name} Ceiling",
            'zone': zone_name,
            'type': 'ceiling' if floor_key != 'T' else 'roof',
            'construction': 'Interzone Floor',
            'boundary': 'adiabatic',
            'vertices': roof_vertices(x1, x2, y1, y2, z_top),
        })

    return surfaces


def build_model():
    zones = []
    all_surfaces = []
    people = []
    lights = []
    equipment = []
    infiltration = []
    air_loops = []

    apt_zones_working = []
    apt_zones_stayhome = []
    apt_zones_office = []
    corridor_zones = []
    all_apt_zones = []

    for floor_key in ['G', 'M', 'T']:
        mult = 2 if floor_key == 'M' else 1

        # South row apartments
        for suffix, col_idx, row, ext_walls, sched_type in SOUTH_ZONES:
            # Ground floor SE position is "Office"
            if floor_key == 'G' and suffix == 'SE Apt':
                zone_name = 'Office'
                stype = 'office'
            else:
                zone_name = f"{floor_key} {suffix}"
                stype = sched_type

            z = {'name': zone_name, 'volume': r(APT_VOL, 1), 'floor_area': r(APT_AREA, 1)}
            if mult > 1:
                z['multiplier'] = mult
            zones.append(z)

            surfs = gen_zone_surfaces(zone_name, floor_key, col_idx, row, ext_walls)
            all_surfaces.extend(surfs)

            if stype == 'working':
                apt_zones_working.append(zone_name)
            elif stype == 'office':
                apt_zones_office.append(zone_name)
            all_apt_zones.append(zone_name)

            # Air loop (PTAC)
            air_loops.append({
                'name': f"PTAC {zone_name}",
                'system_type': 'ptac',
                'controls': {
                    'heating_supply_temp': 32.2,
                    'cooling_supply_temp': 12.8,
                    'design_zone_flow': 'autosize',
                },
                'equipment': [
                    {
                        'type': 'fan',
                        'name': f"Fan {zone_name}",
                        'source': 'on_off',
                        'design_flow_rate': 'autosize',
                        'pressure_rise': 331.17,
                        'impeller_efficiency': 0.65,
                        'motor_efficiency': 0.8,
                        'motor_in_airstream_fraction': 1.0,
                    },
                    {
                        'type': 'heating_coil',
                        'name': f"HC {zone_name}",
                        'source': 'hot_water',
                        'capacity': 'autosize',
                        'setpoint': 40.0,
                        'efficiency': 1.0,
                        'plant_loop': 'HHW Loop',
                    },
                    {
                        'type': 'cooling_coil',
                        'name': f"CC {zone_name}",
                        'capacity': 'autosize',
                        'cop': 3.2,
                        'shr': 0.75,
                        'rated_airflow': 'autosize',
                        'setpoint': 12.8,
                    },
                ],
                'zones': [{'zone': zone_name}],
            })

        # North row apartments
        for suffix, col_idx, row, ext_walls, sched_type in NORTH_ZONES:
            zone_name = f"{floor_key} {suffix}"

            z = {'name': zone_name, 'volume': r(APT_VOL, 1), 'floor_area': r(APT_AREA, 1)}
            if mult > 1:
                z['multiplier'] = mult
            zones.append(z)

            surfs = gen_zone_surfaces(zone_name, floor_key, col_idx, row, ext_walls)
            all_surfaces.extend(surfs)

            apt_zones_stayhome.append(zone_name)
            all_apt_zones.append(zone_name)

            # Air loop (PTAC)
            air_loops.append({
                'name': f"PTAC {zone_name}",
                'system_type': 'ptac',
                'controls': {
                    'heating_supply_temp': 32.2,
                    'cooling_supply_temp': 12.8,
                    'design_zone_flow': 'autosize',
                },
                'equipment': [
                    {
                        'type': 'fan',
                        'name': f"Fan {zone_name}",
                        'source': 'on_off',
                        'design_flow_rate': 'autosize',
                        'pressure_rise': 331.17,
                        'impeller_efficiency': 0.65,
                        'motor_efficiency': 0.8,
                        'motor_in_airstream_fraction': 1.0,
                    },
                    {
                        'type': 'heating_coil',
                        'name': f"HC {zone_name}",
                        'source': 'hot_water',
                        'capacity': 'autosize',
                        'setpoint': 40.0,
                        'efficiency': 1.0,
                        'plant_loop': 'HHW Loop',
                    },
                    {
                        'type': 'cooling_coil',
                        'name': f"CC {zone_name}",
                        'capacity': 'autosize',
                        'cop': 3.2,
                        'shr': 0.75,
                        'rated_airflow': 'autosize',
                        'setpoint': 12.8,
                    },
                ],
                'zones': [{'zone': zone_name}],
            })

        # Corridor (unconditioned — matches E+ where corridors have no HVAC)
        cor_name = f"{floor_key} Corridor"
        z = {'name': cor_name, 'volume': r(COR_VOL, 1), 'floor_area': r(COR_AREA, 1),
             'conditioned': False}
        if mult > 1:
            z['multiplier'] = mult
        zones.append(z)

        cor_ext = ['west', 'east']
        surfs = gen_zone_surfaces(cor_name, floor_key, 0, 'S', cor_ext, is_corridor=True)
        all_surfaces.extend(surfs)
        corridor_zones.append(cor_name)

    # ─── People ────────────────────────────────────────────────────────────
    people.append({
        'name': 'Working Family People',
        'zones': apt_zones_working,
        'count': 2.5,
        'activity_level': 95.0,
        'radiant_fraction': 0.3,
        'schedule': 'Working Family Occupancy',
    })
    people.append({
        'name': 'Stay Home Family People',
        'zones': apt_zones_stayhome,
        'count': 2.5,
        'activity_level': 95.0,
        'radiant_fraction': 0.3,
        'schedule': 'Stay Home Occupancy',
    })
    if apt_zones_office:
        people.append({
            'name': 'Office People',
            'zones': apt_zones_office,
            'count': 4.75,  # 88.25 / 18.58
            'activity_level': 95.0,
            'radiant_fraction': 0.3,
            'schedule': 'Working Family Occupancy',
        })
    people.append({
        'name': 'Corridor People',
        'zones': corridor_zones,
        'count': 4.18,
        'activity_level': 95.0,
        'radiant_fraction': 0.3,
        'schedule': 'Stay Home Occupancy',
    })

    # ─── Lights ────────────────────────────────────────────────────────────
    lights.append({
        'name': 'Apartment Lights',
        'zones': [z for z in all_apt_zones if z != 'Office'],
        'watts_per_area': 11.517,
        'radiant_fraction': 0.6,
        'return_air_fraction': 0.0,
        'schedule': 'Apartment Lighting',
    })
    if apt_zones_office:
        lights.append({
            'name': 'Office Lights',
            'zones': apt_zones_office,
            'watts_per_area': 11.840,
            'radiant_fraction': 0.6,
            'return_air_fraction': 0.0,
            'schedule': 'Office Lighting',
        })
    lights.append({
        'name': 'Corridor Lights',
        'zones': corridor_zones,
        'watts_per_area': 5.818,
        'radiant_fraction': 0.6,
        'return_air_fraction': 0.0,
        'schedule': 'Corridor Lighting',
    })

    # ─── Equipment ─────────────────────────────────────────────────────────
    equipment.append({
        'name': 'Apartment Equipment',
        'zones': [z for z in all_apt_zones if z != 'Office'],
        'watts_per_area': 6.67,
        'radiant_fraction': 0.5,
        'schedule': 'Apartment Equipment Sch',
    })
    if apt_zones_office:
        equipment.append({
            'name': 'Office Equipment',
            'zones': apt_zones_office,
            'watts_per_area': 6.67,
            'radiant_fraction': 0.5,
            'schedule': 'Office Equipment Sch',
        })

    # ─── Elevator Equipment (Interior Equipment per E+ classification) ────
    # E+ defines elevator loads as ElectricEquipment in corridor zones.
    # Total elevator load is split across corridor zones.
    num_corridors = len(corridor_zones)
    if num_corridors > 0:
        equipment.append({
            'name': 'Elevator Running',
            'zones': corridor_zones,
            'power': 38362.0 / num_corridors,
            'radiant_fraction': 0.0,
            'schedule': 'Elevator Running',
        })
        equipment.append({
            'name': 'Elevator Standby',
            'zones': corridor_zones,
            'power': 1600.0 / num_corridors,
            'radiant_fraction': 0.0,
            'schedule': 'Elevator Standby',
        })
        equipment.append({
            'name': 'Elevator Lights Fan',
            'zones': corridor_zones,
            'power': 162.0 / num_corridors,
            'radiant_fraction': 0.0,
        })

    # ─── Infiltration ──────────────────────────────────────────────────────
    all_zone_names = [z['name'] for z in zones]
    infiltration.append({
        'name': 'Building Infiltration',
        'zones': all_zone_names,
        'flow_per_exterior_wall_area': 0.000570,
        'constant_coefficient': 0.0,
        'temperature_coefficient': 0.0,
        'wind_coefficient': 0.224,
        'wind_squared_coefficient': 0.0,
    })

    # ─── Build model dict ──────────────────────────────────────────────────
    model = {}

    # Simulation
    model['simulation'] = {
        'timesteps_per_hour': 4,
        'start_month': 1,
        'start_day': 1,
        'end_month': 12,
        'end_day': 31,
    }

    model['weather_files'] = ['Boulder.epw']

    model['design_days'] = [
        {
            'name': 'Boulder Heating 99.6%',
            'design_temp': -17.8,
            'daily_range': 0.0,
            'humidity_type': 'wetbulb',
            'humidity_value': -17.8,
            'pressure': 83411.0,
            'wind_speed': 2.3,
            'month': 1,
            'day': 21,
            'day_type': 'winter',
        },
        {
            'name': 'Boulder Cooling 0.4%',
            'design_temp': 34.1,
            'daily_range': 15.2,
            'humidity_type': 'wetbulb',
            'humidity_value': 15.7,
            'pressure': 83411.0,
            'wind_speed': 4.0,
            'month': 7,
            'day': 21,
            'day_type': 'summer',
        },
    ]

    # Schedules
    model['schedules'] = [
        {
            'name': 'Working Family Occupancy',
            'weekday': [1,1,1,1,1,1,1,0,0,0,0,0,0,0,0,0,0,1,1,1,1,1,1,1],
            'weekend': [1,1,1,1,1,1,1,1,1,1,0.5,0.5,0.5,0.5,0,1,1,1,1,1,1,1,1,1],
        },
        {
            'name': 'Stay Home Occupancy',
            'weekday': [1,1,1,1,1,1,0.85,0.39,0.25,0.25,0.25,0.25,0.25,0.25,0.25,0.3,0.52,0.87,0.87,0.87,1,1,1,1],
            'weekend': [1,1,1,1,1,1,0.85,0.39,0.25,0.25,0.25,0.25,0.25,0.25,0.25,0.3,0.52,0.87,0.87,0.87,1,1,1,1],
        },
        {
            'name': 'Apartment Lighting',
            'weekday': [0.01132,0.01132,0.01132,0.03395,0.07355,0.07921,0.07355,0.03395,0.02263,0.02263,0.02263,0.02263,0.02263,0.02263,0.03961,0.07921,0.11316,0.15277,0.18106,0.18106,0.12448,0.0679,0.02829,0.02829],
            'weekend': [0.01132,0.01132,0.01132,0.03395,0.07355,0.07921,0.07355,0.03395,0.02263,0.02263,0.02263,0.02263,0.02263,0.02263,0.03961,0.07921,0.11316,0.15277,0.18106,0.18106,0.12448,0.0679,0.02829,0.02829],
        },
        {
            'name': 'Office Lighting',
            'weekday': [0.18,0.18,0.18,0.18,0.18,0.18,0.18,0.9,0.9,0.9,0.9,0.8,0.9,0.9,0.9,0.9,0.18,0.18,0.18,0.18,0.18,0.18,0.18,0.18],
            'weekend': [0.18,0.18,0.18,0.18,0.18,0.18,0.18,0.18,0.18,0.18,0.18,0.18,0.18,0.18,0.18,0.18,0.18,0.18,0.18,0.18,0.18,0.18,0.18,0.18],
        },
        {
            'name': 'Corridor Lighting',
            'weekday': [1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1],
            'weekend': [1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1,1],
        },
        {
            'name': 'Apartment Equipment Sch',
            'weekday': [0.45,0.41,0.39,0.38,0.38,0.43,0.54,0.65,0.66,0.67,0.69,0.70,0.69,0.66,0.65,0.68,0.80,1.0,1.0,0.93,0.89,0.85,0.71,0.58],
            'weekend': [0.45,0.41,0.39,0.38,0.38,0.43,0.54,0.65,0.66,0.67,0.69,0.70,0.69,0.66,0.65,0.68,0.80,1.0,1.0,0.93,0.89,0.85,0.71,0.58],
        },
        {
            'name': 'Office Equipment Sch',
            'weekday': [0.219,0.219,0.219,0.219,0.219,0.219,0.331,0.926,0.926,0.926,0.926,0.870,0.926,0.926,0.926,0.926,0.331,0.219,0.219,0.219,0.219,0.219,0.219,0.219],
            'weekend': [0.219,0.219,0.219,0.219,0.219,0.219,0.219,0.219,0.219,0.219,0.219,0.219,0.219,0.219,0.219,0.219,0.219,0.219,0.219,0.219,0.219,0.219,0.219,0.219],
        },
        {
            'name': 'DHW Schedule',
            'weekday': [0.08,0.04,0.01,0.01,0.04,0.27,0.94,1.00,0.96,0.84,0.76,0.61,0.53,0.47,0.41,0.47,0.55,0.73,0.86,0.82,0.75,0.61,0.53,0.29],
            'weekend': [0.08,0.04,0.01,0.01,0.04,0.27,0.94,1.00,0.96,0.84,0.76,0.61,0.53,0.47,0.41,0.47,0.55,0.73,0.86,0.82,0.75,0.61,0.53,0.29],
        },
        {
            'name': 'Exterior Lights Schedule',
            'weekday': [1,1,1,1,1,1,0,0,0,0,0,0,0,0,0,0,0,0,1,1,1,1,1,1],
            'weekend': [1,1,1,1,1,1,0,0,0,0,0,0,0,0,0,0,0,0,1,1,1,1,1,1],
        },
        {
            'name': 'Elevator Running',
            'weekday': [0,0,0,0,0,0,0.026,0.067,0.067,0,0.026,0.067,0.026,0,0,0,0.026,0.067,0.026,0,0,0,0,0],
            'weekend': [0,0,0,0,0,0,0.026,0.067,0.067,0,0.026,0.067,0.026,0,0,0,0.026,0.067,0.026,0,0,0,0,0],
        },
        {
            'name': 'Elevator Standby',
            'weekday': [1,1,1,1,1,1,0.9,0.742,0.742,1,0.9,0.742,0.9,1,1,1,0.9,0.742,0.9,1,1,1,1,1],
            'weekend': [1,1,1,1,1,1,0.9,0.742,0.742,1,0.9,0.742,0.9,1,1,1,0.9,0.742,0.9,1,1,1,1,1],
        },
    ]

    # Materials
    model['materials'] = [
        {'name': 'Stucco', 'conductivity': 0.72, 'density': 1856, 'specific_heat': 840,
         'solar_absorptance': 0.92, 'thermal_absorptance': 0.9, 'roughness': 'smooth'},
        {'name': 'Gypsum 16mm', 'conductivity': 0.16, 'density': 800, 'specific_heat': 1090,
         'solar_absorptance': 0.4, 'thermal_absorptance': 0.9, 'roughness': 'medium_smooth'},
        {'name': 'Gypsum 13mm', 'conductivity': 0.16, 'density': 800, 'specific_heat': 1090,
         'solar_absorptance': 0.4, 'thermal_absorptance': 0.9, 'roughness': 'smooth'},
        # Res wall insulation: R=2.368, thickness=0.10m -> k=0.04223
        {'name': 'Res Wall Insulation', 'conductivity': 0.04223, 'density': 28, 'specific_heat': 1210,
         'solar_absorptance': 0.7, 'thermal_absorptance': 0.9, 'roughness': 'medium_smooth'},
        # Nonres wall insulation: R=1.713, thickness=0.10m -> k=0.05838
        {'name': 'Nonres Wall Insulation', 'conductivity': 0.05838, 'density': 28, 'specific_heat': 1210,
         'solar_absorptance': 0.7, 'thermal_absorptance': 0.9, 'roughness': 'medium_smooth'},
        {'name': 'Built-up Roofing', 'conductivity': 0.16, 'density': 1120, 'specific_heat': 1460,
         'solar_absorptance': 0.9, 'thermal_absorptance': 0.9, 'roughness': 'rough'},
        # Roof insulation: R=2.599, thickness=0.10m -> k=0.03848
        {'name': 'Roof Insulation', 'conductivity': 0.03848, 'density': 28, 'specific_heat': 1210,
         'solar_absorptance': 0.7, 'thermal_absorptance': 0.9, 'roughness': 'medium_smooth'},
        {'name': 'Metal Deck', 'conductivity': 45.28, 'density': 7824, 'specific_heat': 500,
         'solar_absorptance': 0.7, 'thermal_absorptance': 0.9, 'roughness': 'smooth'},
        # Ground floor materials: carpet pad + 8in HW concrete + virtual ground insulation
        # Matches E+ F-Factor floor: F=1.264 W/(m·K), Area=88.28 m², Peri=37.68 m
        # Effective R_total = Area/(F×Peri) = 1.855 m²·K/W
        {'name': 'Carpet Pad', 'conductivity': 0.06, 'density': 200, 'specific_heat': 1380,
         'solar_absorptance': 0.7, 'thermal_absorptance': 0.9, 'roughness': 'smooth'},
        {'name': 'Concrete 8in HW', 'conductivity': 1.311, 'density': 2240, 'specific_heat': 837,
         'solar_absorptance': 0.7, 'thermal_absorptance': 0.9, 'roughness': 'rough'},
        {'name': 'Slab Ground Insulation', 'conductivity': 0.04, 'density': 28, 'specific_heat': 1210,
         'solar_absorptance': 0.7, 'thermal_absorptance': 0.9, 'roughness': 'medium_smooth'},
    ]

    # Constructions
    model['constructions'] = [
        {'name': 'Res Exterior Wall', 'layers': [
            {'material': 'Stucco', 'thickness': 0.0254},
            {'material': 'Gypsum 16mm', 'thickness': 0.0159},
            {'material': 'Res Wall Insulation', 'thickness': 0.10},
            {'material': 'Gypsum 16mm', 'thickness': 0.0159},
        ]},
        {'name': 'Nonres Exterior Wall', 'layers': [
            {'material': 'Stucco', 'thickness': 0.0254},
            {'material': 'Gypsum 16mm', 'thickness': 0.0159},
            {'material': 'Nonres Wall Insulation', 'thickness': 0.10},
            {'material': 'Gypsum 16mm', 'thickness': 0.0159},
        ]},
        {'name': 'Res Roof', 'layers': [
            {'material': 'Built-up Roofing', 'thickness': 0.0095},
            {'material': 'Roof Insulation', 'thickness': 0.10},
            {'material': 'Metal Deck', 'thickness': 0.0008},
        ]},
        {'name': 'Nonres Roof', 'layers': [
            {'material': 'Built-up Roofing', 'thickness': 0.0095},
            {'material': 'Roof Insulation', 'thickness': 0.10},
            {'material': 'Metal Deck', 'thickness': 0.0008},
        ]},
        {'name': 'Ground Floor Slab', 'layers': [
            {'material': 'Carpet Pad', 'thickness': 0.013},
            {'material': 'Concrete 8in HW', 'thickness': 0.2032},
            {'material': 'Slab Ground Insulation', 'thickness': 0.055},
        ]},
    ]

    # Simple constructions
    model['simple_constructions'] = [
        {'name': 'Interior Partition', 'u_factor': 4.0, 'thickness': 0.0254,
         'thermal_capacity': 22000.0, 'solar_absorptance': 0.4, 'thermal_absorptance': 0.9},
        {'name': 'Interzone Floor', 'u_factor': 3.0, 'thickness': 0.12,
         'thermal_capacity': 200000.0, 'solar_absorptance': 0.7, 'thermal_absorptance': 0.9},
    ]

    # Window constructions
    model['window_constructions'] = [
        {'name': 'Res Window', 'u_factor': 3.237, 'shgc': 0.39, 'visible_transmittance': 0.429},
    ]

    model['zones'] = zones
    model['people'] = people
    model['lights'] = lights
    model['equipment'] = equipment
    model['infiltration'] = infiltration

    # Outdoor air
    model['outdoor_air'] = [
        {'name': 'Apartment Ventilation', 'zones': [z['name'] for z in zones],
         'per_area': 0.000294},
    ]

    model['surfaces'] = all_surfaces
    model['air_loops'] = air_loops

    # Plant loops
    model['plant_loops'] = [
        {
            'name': 'HHW Loop',
            'design_supply_temp': 82.0,
            'design_delta_t': 11.0,
            'supply_equipment': [
                {
                    'type': 'pump',
                    'name': 'HHW Pump',
                    'pump_type': 'variable_speed',
                    'design_flow_rate': 'autosize',
                    'design_head': 179352.0,
                    'motor_efficiency': 0.9,
                },
                {
                    'type': 'boiler',
                    'name': 'HW Boiler',
                    'capacity': 150457.0,
                    'efficiency': 0.75,
                    'design_outlet_temp': 82.0,
                    'design_water_flow_rate': 'autosize',
                },
            ],
        },
    ]

    # DHW
    model['dhw_systems'] = [
        {
            'name': 'Apartment DHW',
            'mains_temperature': 10.0,
            'water_heater': {
                'name': 'Gas Water Heater',
                'fuel_type': 'gas',
                'tank_volume': 946.0,
                'capacity': 53912.0,
                'efficiency': 0.80,
                'setpoint': 60.0,
                'ua_standby': 10.0,
            },
            'loads': [
                {
                    'name': 'Apartment DHW Load',
                    'peak_flow_rate': 0.1135,  # L/s (= 0.00011346 m3/s = 0.1135 L/s)
                    'schedule': 'DHW Schedule',
                    'use_temperature': 43.3,
                },
            ],
        },
    ]

    # Exterior equipment
    model['exterior_equipment'] = [
        {
            'name': 'Exterior Facade Lighting',
            'power': 5317.0,
            'schedule': 'Exterior Lights Schedule',
            'fuel': 'electricity',
            'subcategory': 'exterior_lights',
        },
    ]

    # Zone groups and thermostats
    all_apt_zone_names = apt_zones_working + apt_zones_stayhome + apt_zones_office
    model['zone_groups'] = [
        {'name': 'Apartment Zones', 'zones': all_apt_zone_names},
        {'name': 'Corridor Zones', 'zones': corridor_zones},
    ]

    model['thermostats'] = [
        {'name': 'Apartment Thermostat', 'zones': ['Apartment Zones'],
         'heating_setpoint': 21.1, 'cooling_setpoint': 24.0},
        # Corridors are unconditioned — no thermostat needed (matches E+)
    ]

    # Outputs
    model['outputs'] = [
        {
            'file': 'zone_results.csv',
            'frequency': 'hourly',
            'variables': [
                'zone_temperature', 'zone_heating_rate', 'zone_cooling_rate',
                'zone_infiltration_mass_flow', 'zone_supply_air_temperature',
                'zone_supply_air_mass_flow', 'site_outdoor_temperature',
            ],
        },
    ]

    model['summary_report'] = True

    return model


def dump_yaml(model, path):
    """Write model as YAML with nice formatting."""
    # Custom representer to handle 'autosize' strings in numeric contexts
    class CustomDumper(yaml.SafeDumper):
        pass

    def represent_float(dumper, value):
        if value == int(value) and abs(value) < 1e10:
            return dumper.represent_scalar('tag:yaml.org,2002:float', f'{value:.1f}')
        return dumper.represent_scalar('tag:yaml.org,2002:float', f'{value}')

    with open(path, 'w') as f:
        f.write("# OpenBSE: DOE Prototype Mid-Rise Apartment (ASHRAE 90.1-2022 Appendix G)\n")
        f.write("# 3-story (4 effective via multiplier), 27 zones, PTAC per zone, HW boiler\n")
        f.write("# Boulder, CO (TMYx weather)\n\n")
        yaml.safe_dump(model, f, default_flow_style=False, sort_keys=False, width=120)


if __name__ == '__main__':
    model = build_model()
    path = '/Users/benjaminbrannon/Documents/GitHub/OpenBSE/eplus_comparison/ApartmentMidRise_Boulder.yaml'
    dump_yaml(model, path)

    # Count
    zones = model['zones']
    surfaces = model['surfaces']
    loops = model['air_loops']
    print(f"Zones: {len(zones)}")
    print(f"Surfaces: {len(surfaces)}")
    print(f"Air loops: {len(loops)}")
    print(f"YAML written to {path}")
