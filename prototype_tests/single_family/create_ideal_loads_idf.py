#!/usr/bin/env python3
"""Create an ideal loads IDF from the simplified single-family IDF.

Removes all HVAC equipment and replaces with ZoneHVAC:IdealLoadsAirSystem.
Adds comprehensive output variables for envelope heat balance comparison.
"""

import re
import sys
from pathlib import Path

# IDF objects to REMOVE entirely (case-insensitive match on object type)
REMOVE_OBJECTS = {
    "airloophvac:unitaryheatcool",
    "airloophvac",
    "airloophvac:supplypath",
    "airloophvac:returnpath",
    "airloophvac:zonesplitter",
    "airloophvac:zonemixer",
    "fan:onoff",
    "fan:zoneexhaust",
    "coil:cooling:dx:singlespeed",
    "coil:heating:fuel",
    "zonehvac:airdistributionunit",
    "airterminal:singleduct:constantvolume:noreheat",
    "branch",
    "branchlist",
    "availabilitymanager:scheduled",
    "availabilitymanagerassignmentlist",
    "sizing:system",
    "designspecification:outdoorair",
    "zoneventilation:designflowrate",
    "nodelist",
    "zonehvac:equipmentlist",
    "zonehvac:equipmentconnections",
    # DHW / plant loop objects (not needed for envelope comparison)
    "plantloop",
    "plantequipmentoperationschemes",
    "plantequipmentoperation:heatingload",
    "waterheater:mixed",
    "waterheater:sizing",
    "wateruse:equipment",
    "wateruse:connections",
    "connector:splitter",
    "connector:mixer",
    "connectorlist",
    "pipe:adiabatic",
    "pump:variablespeed",
    "setpointmanager:scheduled",
    "sizing:plant",
}

# Output:Variable keys to remove (HVAC-specific)
REMOVE_OUTPUT_KEYS = {
    "main fuel heating coil_unit1",
    "supply fan_unit1",
    "dx cooling coil_unit1",
    "zone inlet node_unit1",
    "zone outlet node_unit1",
}

# Output:Variable names to remove (HVAC-specific)
REMOVE_OUTPUT_VARS = {
    "zone air system sensible heating rate",
    "zone air system sensible cooling rate",
    "zone supply air mass flow rate",
    "zone mechanical ventilation mass flow rate",
}

# Output:Table:Monthly reports to remove (reference HVAC components)
REMOVE_TABLE_MONTHLY = {
    "fansplit",
    "coilloads",
    "water heater: loads",
}


def parse_idf_objects(text):
    """Parse IDF text into a list of (object_type, full_text) tuples.

    Also preserves comment-only blocks and blank lines between objects.
    """
    objects = []
    current = []
    current_type = None
    in_object = False

    for line in text.split('\n'):
        stripped = line.strip()

        # Check if this line starts a new object (non-comment, non-blank, contains a comma or semicolon)
        # IDF objects start with ObjectType, or ObjectType,
        if not in_object and stripped and not stripped.startswith('!'):
            # Check if this looks like an object start
            # Object types don't start with whitespace typically, but some IDF files indent
            match = re.match(r'\s*([A-Za-z][A-Za-z0-9:._\- ]*?)(?:\s*,|\s*;)', line)
            if match:
                # Save previous block
                if current:
                    objects.append((current_type, '\n'.join(current)))
                    current = []

                current_type = match.group(1).strip()
                current = [line]
                in_object = True

                # Check if object ends on same line (single-line object)
                if ';' in line:
                    in_object = False
                continue

        if in_object:
            current.append(line)
            if ';' in stripped and not stripped.startswith('!'):
                in_object = False
        else:
            # Between objects - comments, blank lines
            if current_type is not None:
                # Attach trailing comments/blanks to previous object
                current.append(line)
            else:
                current.append(line)

    # Save last block
    if current:
        objects.append((current_type, '\n'.join(current)))

    return objects


def should_remove_object(obj_type, obj_text):
    """Check if an object should be removed."""
    if obj_type is None:
        return False

    type_lower = obj_type.lower().strip()

    # Direct type match
    if type_lower in REMOVE_OBJECTS:
        return True

    # Output:Variable - check key and variable name
    if type_lower == "output:variable":
        text_lower = obj_text.lower()
        for key in REMOVE_OUTPUT_KEYS:
            if key in text_lower:
                return True
        for var in REMOVE_OUTPUT_VARS:
            if var in text_lower:
                return True

    # Output:Table:Monthly - check report name
    if type_lower == "output:table:monthly":
        # Extract the name (first field after the object type)
        lines = obj_text.strip().split('\n')
        for line in lines:
            stripped = line.strip()
            if stripped.startswith('!') or stripped.lower().startswith('output:table:monthly'):
                continue
            # First non-comment, non-type line has the name
            name = stripped.split(',')[0].split('!')[0].strip().rstrip(';').strip()
            if name.lower() in REMOVE_TABLE_MONTHLY:
                return True
            break

    # Curve objects used only by DX coil
    if type_lower.startswith("curve:"):
        text_lower = obj_text.lower()
        # Keep curves not specific to cooling coil
        coil_curves = ["cool-cap-ft", "cool-eir-ft", "cool-plf-fplr", "constantcubic"]
        for curve_name in coil_curves:
            if curve_name in text_lower:
                return True

    return False


def create_ideal_loads_idf(input_path, output_path):
    """Create ideal loads IDF from simplified IDF."""
    text = Path(input_path).read_text()

    # Simple approach: process line by line, tracking object boundaries
    lines = text.split('\n')
    output_lines = []
    skip_until_semicolon = False
    skip_object_type = None

    i = 0
    while i < len(lines):
        line = lines[i]
        stripped = line.strip()

        if skip_until_semicolon:
            if ';' in stripped and not stripped.startswith('!'):
                skip_until_semicolon = False
            i += 1
            continue

        # Check if this line starts a new IDF object
        if stripped and not stripped.startswith('!'):
            # Match object type at start of line
            match = re.match(r'\s*([A-Za-z][A-Za-z0-9:._\-]*)\s*[,;]', stripped)
            if match:
                obj_type = match.group(1).strip().lower()

                if obj_type in REMOVE_OBJECTS:
                    # Skip this entire object
                    if ';' not in stripped:
                        skip_until_semicolon = True
                    i += 1
                    continue

                # Check Output:Variable for HVAC-specific ones
                if obj_type == "output:variable":
                    text_lower = stripped.lower()
                    should_skip = False
                    for key in REMOVE_OUTPUT_KEYS:
                        if key in text_lower:
                            should_skip = True
                            break
                    if not should_skip:
                        for var in REMOVE_OUTPUT_VARS:
                            if var in text_lower:
                                should_skip = True
                                break
                    if should_skip:
                        i += 1
                        continue

                # Check Output:Table:Monthly for HVAC-specific ones
                if obj_type == "output:table:monthly":
                    # Look ahead to get the name field
                    j = i + 1
                    while j < len(lines):
                        next_stripped = lines[j].strip()
                        if next_stripped and not next_stripped.startswith('!'):
                            name = next_stripped.split(',')[0].split('!')[0].strip().rstrip(';').strip()
                            if name.lower() in REMOVE_TABLE_MONTHLY:
                                # Skip until semicolon
                                while j < len(lines):
                                    if ';' in lines[j] and not lines[j].strip().startswith('!'):
                                        break
                                    j += 1
                                i = j + 1
                                should_skip = True
                                break
                            break
                        j += 1
                    else:
                        should_skip = False
                    if 'should_skip' in dir() and should_skip:
                        should_skip = False
                        continue

                # Check Curve objects used only by cooling coil
                if obj_type.startswith("curve:"):
                    # Look ahead for curve name
                    j = i + 1
                    while j < len(lines):
                        next_stripped = lines[j].strip()
                        if next_stripped and not next_stripped.startswith('!'):
                            name = next_stripped.split(',')[0].split('!')[0].strip().rstrip(';').strip()
                            coil_curves = ["cool-cap-ft", "cool-eir-ft", "cool-plf-fplr", "constantcubic"]
                            if name.lower() in coil_curves:
                                while j < len(lines):
                                    if ';' in lines[j] and not lines[j].strip().startswith('!'):
                                        break
                                    j += 1
                                i = j + 1
                                should_skip = True
                                break
                            break
                        j += 1
                    if 'should_skip' in dir() and should_skip:
                        should_skip = False
                        continue

        output_lines.append(line)
        i += 1

    # Now add ideal loads objects and new output variables
    ideal_loads_section = """
!-   ===========  IDEAL LOADS AIR SYSTEM ===========

ZoneHVAC:IdealLoadsAirSystem,
    Ideal Loads Living,          !- Name
    ,                            !- Availability Schedule Name
    Ideal Loads Supply Node,     !- Zone Supply Air Node Name
    ,                            !- Zone Exhaust Air Node Name
    ,                            !- System Inlet Air Node Name
    50,                          !- Maximum Heating Supply Air Temperature {C}
    13,                          !- Minimum Cooling Supply Air Temperature {C}
    0.015,                       !- Maximum Heating Supply Air Humidity Ratio {kgWater/kgDryAir}
    0.01,                        !- Minimum Cooling Supply Air Humidity Ratio {kgWater/kgDryAir}
    NoLimit,                     !- Heating Limit
    ,                            !- Maximum Heating Air Flow Rate {m3/s}
    ,                            !- Maximum Sensible Heating Capacity {W}
    NoLimit,                     !- Cooling Limit
    ,                            !- Maximum Cooling Air Flow Rate {m3/s}
    ,                            !- Maximum Cooling Total Capacity {W}
    ,                            !- Heating Availability Schedule Name
    ,                            !- Cooling Availability Schedule Name
    ConstantSensibleHeatRatio,   !- Dehumidification Control Type
    0.7,                         !- Cooling Sensible Heat Ratio
    None,                        !- Humidification Control Type
    ,                            !- Design Specification Outdoor Air Object Name
    ,                            !- Outdoor Air Inlet Node Name
    None,                        !- Demand Controlled Ventilation Type
    NoEconomizer,                !- Outdoor Air Economizer Type
    None,                        !- Heat Recovery Type
    0.7,                         !- Sensible Heat Recovery Effectiveness
    0.65;                        !- Latent Heat Recovery Effectiveness

ZoneHVAC:EquipmentList,
    ZoneEquipment_unit1,         !- Name
    SequentialLoad,              !- Load Distribution Scheme
    ZoneHVAC:IdealLoadsAirSystem,!- Zone Equipment 1 Object Type
    Ideal Loads Living,          !- Zone Equipment 1 Name
    1,                           !- Zone Equipment 1 Cooling Sequence
    1,                           !- Zone Equipment 1 Heating or No-Load Sequence
    ,                            !- Zone Equipment 1 Sequential Cooling Fraction Schedule Name
    ;                            !- Zone Equipment 1 Sequential Heating Fraction Schedule Name

ZoneHVAC:EquipmentConnections,
    living_unit1,                !- Zone Name
    ZoneEquipment_unit1,         !- Zone Conditioning Equipment List Name
    Ideal Loads Supply Node,     !- Zone Air Inlet Node or NodeList Name
    ,                            !- Zone Air Exhaust Node or NodeList Name
    Zone Node_unit1,             !- Zone Air Node Name
    Zone Outlet Node_unit1;      !- Zone Return Air Node Name


!-   ===========  IDEAL LOADS OUTPUT VARIABLES ===========

! Ideal loads heating and cooling
Output:Variable,*,Zone Ideal Loads Zone Sensible Heating Rate,Timestep;
Output:Variable,*,Zone Ideal Loads Zone Sensible Cooling Rate,Timestep;
Output:Variable,*,Zone Ideal Loads Zone Sensible Heating Energy,Timestep;
Output:Variable,*,Zone Ideal Loads Zone Sensible Cooling Energy,Timestep;
Output:Variable,*,Zone Ideal Loads Supply Air Sensible Heating Rate,Timestep;
Output:Variable,*,Zone Ideal Loads Supply Air Sensible Cooling Rate,Timestep;
Output:Variable,*,Zone Ideal Loads Supply Air Total Heating Rate,Timestep;
Output:Variable,*,Zone Ideal Loads Supply Air Total Cooling Rate,Timestep;

! Zone temperatures
Output:Variable,*,Zone Mean Air Temperature,Timestep;

! Per-surface conduction (inside face) — all living zone surfaces
Output:Variable,*,Surface Inside Face Conduction Heat Transfer Rate,Timestep;

! Per-surface temperatures
Output:Variable,*,Surface Inside Face Temperature,Timestep;
Output:Variable,*,Surface Outside Face Temperature,Timestep;

! Solar
Output:Variable,*,Zone Windows Total Transmitted Solar Radiation Rate,Timestep;
Output:Variable,*,Enclosure Windows Total Transmitted Solar Radiation Rate,Timestep;
Output:Variable,*,Zone Windows Total Heat Gain Rate,Timestep;
Output:Variable,*,Zone Windows Total Heat Loss Rate,Timestep;
Output:Variable,*,Surface Window Transmitted Solar Radiation Rate,Timestep;

! Infiltration
Output:Variable,*,Zone Infiltration Sensible Heat Loss Energy,Timestep;
Output:Variable,*,Zone Infiltration Sensible Heat Gain Energy,Timestep;
Output:Variable,*,Zone Infiltration Mass Flow Rate,Timestep;

! Internal gains
Output:Variable,*,Zone People Total Heating Rate,Timestep;
Output:Variable,*,Zone Lights Total Heating Rate,Timestep;
Output:Variable,*,Zone Electric Equipment Total Heating Rate,Timestep;
Output:Variable,*,Zone Other Equipment Total Heating Rate,Timestep;
Output:Variable,*,Zone People Sensible Heating Rate,Timestep;

! Unconditioned zone temperatures (for interzone conduction validation)
Output:Variable,attic_unit1,Zone Mean Air Temperature,Timestep;
Output:Variable,unheatedbsmt_unit1,Zone Mean Air Temperature,Timestep;
Output:Variable,garage1,Zone Mean Air Temperature,Timestep;

! End-use meters (monthly)
Output:Meter,Heating:DistrictHeating,Monthly;
Output:Meter,Cooling:DistrictCooling,Monthly;
Output:Meter,InteriorLights:Electricity,Monthly;
Output:Meter,InteriorEquipment:Electricity,Monthly;
Output:Meter,InteriorEquipment:NaturalGas,Monthly;
Output:Meter,WaterSystems:NaturalGas,Monthly;
Output:Meter,ExteriorLights:Electricity,Monthly;

! Heating and cooling loads monthly table
Output:Table:Monthly,
    Heating and Cooling Loads,  !- Name
    2,                       !- Digits After Decimal
    Zone Ideal Loads Zone Sensible Cooling Energy,  !- Variable or Meter 1 Name
    SumOrAverage,            !- Aggregation Type for Variable or Meter 1
    Zone Ideal Loads Zone Sensible Heating Energy,  !- Variable or Meter 2 Name
    SumOrAverage;            !- Aggregation Type for Variable or Meter 2
"""

    # Insert before the final comment blocks
    # Find where to insert — before the trailing GPARM comments at end of file
    result = '\n'.join(output_lines)

    # Add the ideal loads section before the end
    # Find the last Output: or end of meaningful content
    result = result.rstrip() + '\n' + ideal_loads_section + '\n'

    Path(output_path).write_text(result)
    print(f"Created {output_path}")
    print(f"  Input:  {len(text.split(chr(10)))} lines")
    print(f"  Output: {len(result.split(chr(10)))} lines")


if __name__ == "__main__":
    base = Path(__file__).parent
    input_idf = base / "SingleFamily_CZ5B_Boulder_simplified.idf"
    output_idf = base / "SingleFamily_CZ5B_Boulder_ideal.idf"

    if not input_idf.exists():
        # Try main repo
        main_repo = Path("/Users/benjaminbrannon/Documents/GitHub/OpenBSE")
        input_idf = main_repo / "prototype_tests/single_family/SingleFamily_CZ5B_Boulder_simplified.idf"

    create_ideal_loads_idf(input_idf, output_idf)
