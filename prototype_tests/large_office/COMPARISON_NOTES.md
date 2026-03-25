# Large Office Prototype — E+ Comparison Notes

## Weather File
- **E+**: Denver-Aurora-Buckley AFB TMY3 (WMO 724695, lat 39.72°N)
- **OpenBSE**: Same file (`prototype_tests/Denver-Buckley.epw`)

## Annual Energy End-Use Comparison

| End Use | E+ [kWh] | OpenBSE [kWh] | Diff % | Status |
|---|---|---|---|---|
| Interior Lighting | 1,558,539 | 1,558,539 | -0.0% | PASS |
| Exterior Lighting | 279,464 | 292,714 | +4.7% | PASS |
| Interior Equipment | 4,076,778 | 4,072,466 | -0.1% | PASS |
| Exterior Equipment | 713,075 | 696,046 | -2.4% | PASS |
| Fans | 1,010,319 | 899,357 | -11.0% | FAIL |
| Pumps | 129,769 | 101,073 | -22.1% | FAIL |
| Cooling (Electric) | 552,933 | 647,751 | +17.1% | FAIL |
| Heating (Gas) | 404,736 | 410,850 | +1.5% | PASS |
| Heating (Electric) | 0 | 0 | 0% | PASS |
| DHW (Electric) | 125,133 | 128,981 | +3.1% | PASS |

**7/10 end uses within +/-5%**

## Key Model Differences

### Zone Multiplier Expansion
E+ uses `zone_multiplier = 10` for mid-floor zones (5 office + 1 datacenter).
OpenBSE expands these into 60 explicit zones (10 floors x 6 zones each)
since it doesn't support zone multipliers.

**Impact**: E+'s zone multiplier multiplies gains and air volume but NOT
surface areas. This gives the multiplied zone an artificially high
gains-to-surface ratio, keeping the core zone warmer (~23C on weekends
vs OpenBSE's ~18C). The explicit expansion is more physically correct
but produces different thermal behavior. This is the primary driver of
the fans (-11%) and cooling (+17%) gaps.

### Plenum Zones
E+ has 3 plenum zones (GroundFloor, MidFloor, TopFloor) with exterior
walls and infiltration. OpenBSE does not model these. In the E+ IDF all
conditioned zone ceilings have Adiabatic boundary conditions, so the
plenums are thermally decoupled from occupied zones.

### Interior Wall Coupling
E+ has interzone (Surface boundary) interior walls between Core and
Perimeter zones. OpenBSE matches this with `boundary: !zone` interzone
coupling. This allows heat transfer from the warm core to cold perimeter
zones, significantly improving the heating energy match (+1.5%).

### Floor/Ceiling Constructions
- Floors: 100mm normalweight concrete + carpet pad (matching E+ int_slab_floor)
- Ceilings: Lightweight acoustic tile (matching E+ DropCeiling)
- Both use adiabatic boundary conditions

## Root Cause of Remaining Gaps

### Fans (-11.0%)
Interzone coupling reduces perimeter heating loads (core warms perimeter),
reducing total airflow. E+'s zone multiplier concentrates gains without
scaling surfaces, producing higher peak loads and more airflow. OpenBSE's
explicit zones are more physically accurate but give lower runtime airflow.

### Cooling (+17.1%)
Higher chiller electricity from excess perimeter cooling. The explicit
zone expansion produces different temperature profiles vs E+'s multiplied
approach.

### Pumps (-22.1%)
Downstream of the fan gap -- lower HVAC runtime means less pump operation.

## Next Steps
- Implement zone multiplier support in OpenBSE engine to eliminate the
  surface area scaling discrepancy
- Add plenum zones to capture plenum envelope losses
- Investigate pump scheduling to ensure pumps run whenever any coil demands flow
