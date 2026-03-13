# ASHRAE Standard 140-2023 Validation Tests

**Standard:** ASHRAE Standard 140-2023, Section 7 — Building Thermal Envelope and Fabric Load Tests
**Weather:** 725650TYCST.epw (Denver, CO — 39.833°N, 104.650°W, 1650m elevation)

## Current Status: 48/63 pass (76.2%)

- **600 series (low-mass):** 19/22 pass — failures in Cases 680, 685, 695 (excess cooling from simplified interior solar distribution)
- **900 series (high-mass):** 17/20 pass — Case 960 (sunspace) fails, Case 995 marginal
- **Free-float temperatures:** 18/18 pass (all 6 cases)
- **960 Sun Zone temperatures:** 0/3 pass (marginally below minimum)

See `results/FULL_140_RESULTS.csv` for the complete pass/fail matrix.

## Running Tests

```bash
# Build OpenBSE
cargo build --release

# Run a single case
./target/release/openbse 140_tests/cases/ashrae140_case600.yaml

# Run all cases and generate results CSV
cd 140_tests/scripts
python3 build_140_csv.py
```

## Directory Structure

```
cases/              31 YAML input files (ashrae140_case*.yaml)
weather/            Prescribed weather data (725650TYCST.epw, CE100A.csv)
reference_idfs/     EnergyPlus IDF files for cross-reference
scripts/            Validation and analysis scripts
  build_140_csv.py    Runs all cases, compares against acceptance ranges
  solar_validation.py Solar calculation chain validation
results/            Aggregated results (FULL_140_RESULTS.csv)
```

## Test Cases

### Section 7 — Thermal Fabric (27 cases)

| Case | Description |
|------|-------------|
| 600 | Low-mass base case: south windows, 20/27°C deadband |
| 610 | South overhang shading |
| 620 | East/west window orientation |
| 630 | East/west overhang + fin shading |
| 640 | Thermostat setback schedule |
| 650 | Night ventilation cooling |
| 660 | Low-e argon double-pane windows |
| 670 | Single-pane clear windows |
| 680 | Increased wall/roof insulation |
| 685 | 20/20°C thermostat (no deadband) |
| 695 | Increased insulation + 20/20°C thermostat |
| 600FF | Free-float (no HVAC) |
| 650FF | Free-float with night ventilation |
| 680FF | Free-float with increased insulation |
| 900 | High-mass base case (concrete walls + slab) |
| 910 | High-mass with south overhang |
| 920 | High-mass with east/west windows |
| 930 | High-mass with east/west shading |
| 940 | High-mass thermostat setback |
| 950 | High-mass night ventilation |
| 960 | Sunspace (two-zone: conditioned back + free-float sun zone) |
| 980 | High-mass increased insulation |
| 985 | High-mass 20/20°C thermostat |
| 995 | High-mass insulation + 20/20°C thermostat |
| 900FF | High-mass free-float |
| 950FF | High-mass free-float with night ventilation |
| 980FF | High-mass free-float with increased insulation |

### Section 9 — Cooling Equipment (1 case)

| Case | Description |
|------|-------------|
| CE100 | Analytical DX cooling verification: adiabatic zone, 5400W constant load |
