#!/usr/bin/env python3
"""
Simplify SingleFamily IDF for fair OpenBSE comparison.

1. Removes AirflowNetwork objects (infiltration + duct distribution)
2. Removes GroundHeatTransfer preprocessor objects
3. Replaces GroundSlabPreprocessorAverage boundary with Ground
4. Adds ZoneInfiltration:DesignFlowRate for each zone
   - Living zone uses ASHRAE combined model to include exhaust-induced makeup air
5. Updates Version to 25.2

Equivalent infiltration rates are calculated using the ASHRAE combined
infiltration model: Q_combined = sqrt(Q_infil^2 + Q_exhaust^2).
"""

import math
import re

INPUT  = "SingleFamily_CZ5B_Boulder.idf"
OUTPUT = "SingleFamily_CZ5B_Boulder_simplified.idf"

# ── Infiltration parameters ─────────────────────────────────────────
Q_INFIL   = 0.0238    # background infiltration (m3/s, ~0.15 ACH)
Q_EXHAUST = 0.02832   # exhaust fan flow (m3/s, 60 cfm)
Q_COMBINED = math.sqrt(Q_INFIL**2 + Q_EXHAUST**2)  # ~0.0370 m3/s

# Read the IDF
with open(INPUT, 'r') as f:
    content = f.read()

# ── 1. Remove all AirflowNetwork objects ──────────────────────────────
lines = content.split('\n')
new_lines = []
in_remove_object = False

# Object types to remove entirely
REMOVE_PREFIXES = ('AirflowNetwork:', 'GroundHeatTransfer:')

for line in lines:
    stripped = line.strip()

    # Check if this line starts an object to remove
    if any(stripped.startswith(p) for p in REMOVE_PREFIXES):
        in_remove_object = True
        continue

    if in_remove_object:
        # Check if this line ends the object (contains semicolon as terminator)
        if ';' in stripped and not stripped.startswith('!'):
            in_remove_object = False
            continue
        else:
            continue

    new_lines.append(line)

content = '\n'.join(new_lines)

# ── 2. Replace GroundSlabPreprocessorAverage boundary with Ground ─────
# The GroundHeatTransfer preprocessor creates OtherSideCoefficients objects
# at runtime. Without ExpandObjects, we replace with simple Ground boundary.
content = content.replace('GroundSlabPreprocessorAverage', 'Ground')

# ── 3. Update Version to 25.2 ────────────────────────────────────────
# The original IDF is v23.1 but we're running on E+ 25.2.
# Note: This is a minimal version bump — most objects are compatible.
content = re.sub(
    r'(Version,\s*)\d+\.\d+;',
    r'\g<1>25.2;',
    content,
    count=1
)

# ── 4. Fix v23.1→v25.2 compatibility issues ───────────────────────────
# Replace tabs with spaces in non-comment lines.
# The v23.1 IDF has tabs embedded in numeric fields (e.g. "1865.58\t   ,")
# which E+ 25.2's epJSON parser interprets as strings instead of numbers.
fixed_lines = []
for line in content.split('\n'):
    stripped = line.lstrip()
    if not stripped.startswith('!'):
        line = line.replace('\t', ' ')
    fixed_lines.append(line)
content = '\n'.join(fixed_lines)

# Fix People MRT calculation type enum: ZoneAveraged → EnclosureAveraged (E+ v24.1+)
content = content.replace('ZoneAveraged;', 'EnclosureAveraged;')

# ── 5. Add ZoneInfiltration:DesignFlowRate objects ────────────────────
infiltration_objects = f"""

! ── ZoneInfiltration (replaces AirflowNetwork) ───────────────────────
! Living zone uses ASHRAE combined infiltration to include exhaust-induced
! makeup air: Q = sqrt(Q_infil^2 + Q_exhaust^2) = {Q_COMBINED:.4f} m3/s
! Without this fix, E+ warns "Load due to induced outdoor air is neglected"
! and drastically underestimates heating load.

ZoneInfiltration:DesignFlowRate,
    Living Infiltration,       !- Name
    living_unit1,              !- Zone or ZoneList or Space or SpaceList Name
    inf_sch,                   !- Schedule Name
    Flow/Zone,                 !- Design Flow Rate Calculation Method
    {Q_COMBINED:.4f},                    !- Design Flow Rate {{m3/s}}
    ,                          !- Flow Rate per Floor Area {{m3/s-m2}}
    ,                          !- Flow Rate per Exterior Surface Area {{m3/s-m2}}
    ,                          !- Air Changes per Hour {{1/hr}}
    1.0,                       !- Constant Term Coefficient
    0.0,                       !- Temperature Term Coefficient
    0.0,                       !- Velocity Term Coefficient
    0.0;                       !- Velocity Squared Term Coefficient

ZoneInfiltration:DesignFlowRate,
    Attic Infiltration,        !- Name
    attic_unit1,               !- Zone or ZoneList or Space or SpaceList Name
    inf_sch,                   !- Schedule Name
    Flow/Zone,                 !- Design Flow Rate Calculation Method
    0.023,                     !- Design Flow Rate {{m3/s}}
    ,                          !- Flow Rate per Floor Area {{m3/s-m2}}
    ,                          !- Flow Rate per Exterior Surface Area {{m3/s-m2}}
    ,                          !- Air Changes per Hour {{1/hr}}
    1.0,                       !- Constant Term Coefficient
    0.0,                       !- Temperature Term Coefficient
    0.0,                       !- Velocity Term Coefficient
    0.0;                       !- Velocity Squared Term Coefficient

ZoneInfiltration:DesignFlowRate,
    Basement Infiltration,     !- Name
    unheatedbsmt_unit1,        !- Zone or ZoneList or Space or SpaceList Name
    inf_sch,                   !- Schedule Name
    Flow/Zone,                 !- Design Flow Rate Calculation Method
    0.020,                     !- Design Flow Rate {{m3/s}}
    ,                          !- Flow Rate per Floor Area {{m3/s-m2}}
    ,                          !- Flow Rate per Exterior Surface Area {{m3/s-m2}}
    ,                          !- Air Changes per Hour {{1/hr}}
    1.0,                       !- Constant Term Coefficient
    0.0,                       !- Temperature Term Coefficient
    0.0,                       !- Velocity Term Coefficient
    0.0;                       !- Velocity Squared Term Coefficient

ZoneInfiltration:DesignFlowRate,
    Garage Infiltration,       !- Name
    garage1,                   !- Zone or ZoneList or Space or SpaceList Name
    inf_sch,                   !- Schedule Name
    Flow/Zone,                 !- Design Flow Rate Calculation Method
    0.020,                     !- Design Flow Rate {{m3/s}}
    ,                          !- Flow Rate per Floor Area {{m3/s-m2}}
    ,                          !- Flow Rate per Exterior Surface Area {{m3/s-m2}}
    ,                          !- Air Changes per Hour {{1/hr}}
    1.0,                       !- Constant Term Coefficient
    0.0,                       !- Temperature Term Coefficient
    0.0,                       !- Velocity Term Coefficient
    0.0;                       !- Velocity Squared Term Coefficient

"""

content += infiltration_objects

# ── 6. Write the modified IDF ─────────────────────────────────────────
with open(OUTPUT, 'w') as f:
    f.write(content)

print(f"Written simplified IDF to: {OUTPUT}")
print(f"  - Removed all AirflowNetwork objects")
print(f"  - Removed GroundHeatTransfer preprocessor objects")
print(f"  - Replaced GroundSlabPreprocessorAverage boundary with Ground")
print(f"  - Updated Version to 25.2")
print(f"  - Added ZoneInfiltration:DesignFlowRate for 4 zones")
print(f"  - Living: {Q_COMBINED:.4f} m3/s (ASHRAE combined: sqrt({Q_INFIL}^2 + {Q_EXHAUST}^2))")
print(f"  - Attic: 0.023 m3/s, Basement: 0.020 m3/s, Garage: 0.020 m3/s")
