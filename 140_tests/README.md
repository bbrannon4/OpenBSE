# ASHRAE Standard 140-2023 Validation Tests

**Standard:** ASHRAE Standard 140-2023 — Section 7 (thermal fabric), Section 9 (cooling equipment), Section 10 (heating equipment) and Section 11 (air-side equipment)
**Weather:** each section uses its own prescribed files (see `weather/`). Section 7 uses 725650TYCST.epw (Denver, CO — 39.833°N, 104.650°W, 1650m elevation)

## Current Status: 548/550 gate checks pass

| Section | Cases | Checks passing |
|---|---|---|
| 7 — Thermal fabric (600 and 900 series) | 27 | 62/63 |
| 9A — Cooling equipment, analytical (CE100–CE200) | 14 | 112/112 |
| 9B — Cooling equipment, dynamic (CE300–CE545) | 20 | 230/231 |
| 10 — Heating equipment (HE100–HE230) | 11 | 48/48 |
| 11 — Air-side equipment (AE101–AE445) | 24 | 96/96 |

Section 7 in detail:

- **600 series (low-mass):** 21/22 pass. Case 660 annual heating is about 12 kWh below range since windows' share of radiant gains was fixed (#148).
- **900 series (high-mass):** 20/20 pass
- **Free-float temperatures:** 18/18 pass (all 6 cases)
- **960 Sun Zone temperatures:** 3/3 pass

Two more misses: CE540 annual condenser-fan energy, and CE195 February sensible coil load, 0.03 kWh below range (#295). They are listed, with the reasons, under `KNOWN_FAILURES` in `scripts/build_140_csv.py`.

`scripts/build_140_csv.py` writes the complete pass/fail matrix to `scripts/FULL_140_RESULTS.csv`. CI runs every case and this gate on each commit. The copy committed at `results/FULL_140_RESULTS.csv` is a snapshot taken with the v0.7.0 portal submission.

**Tight margins to watch:** Case 980FF mean temp (exactly at floor), Case 640/680/695 heating within 15 kWh of ceiling. (Case 960 heating gained ~80 kWh of margin once the sunspace switched to geometric interior solar distribution, and Case 670 sits mid-range since the single-pane first-principles path — see issue #101.)

### Software acceptance criteria (Normative Annex A3)

The standard's acceptance criteria are a separate, narrower test than the gate above. They define five test groups. Each group is a set of range cases (a single result, or the difference between two cases) and passes when enough of them fall in range. Scored from the v0.7.0 submission spreadsheets in `results/`:

| Test group | Submission file | In range | Needed |
|---|---|---|---|
| Thermal fabric, low mass | `OpenBSE_Std140_TF_Output.xlsx` | 21/21 | 18 |
| Thermal fabric, high mass | `OpenBSE_Std140_TF_Output.xlsx` | 19/19 | 17 |
| Cooling equipment (CE300–CE440 annual cooling energy) | `OpenBSE_Std140_CE_b_Output.xls` | 14/14 | 12 |
| Heating equipment (HE210–HE230 furnace input and fan energy) | `OpenBSE_Std140_HE_Output.xls` | 6/6 | 5 |
| Air-side equipment (coil loads) | `OpenBSE_Std140_AE_Output.xlsx` | 54/54 | 48 |

These four files are what the IBPSA-USA Standard 140 portal takes. The cooling-equipment group is scored on the Section 9B cases, so `OpenBSE_Std140_CE_a_Output.xls` is not part of that submission.

The gate's ranges for the equipment sections were derived in this repo from the standard's example results (see the `_provenance` notes in `acceptance_ranges_140_2023.json`). They are not the Annex A3 ranges, so passing the gate does not by itself mean passing Annex A3.

## Running Tests

```bash
# Build OpenBSE
cargo build --release

# Run a single case
./target/release/openbse 140_tests/cases/ashrae140_case600.yaml

# Run every case, then check the results against the gate ranges.
# The Section 7 weather CSV is licensed, so it is rebuilt from the in-repo EPW
# (a no-op if you already have the original).
python3 140_tests/scripts/reconstruct_140_weather.py
(cd 140_tests/cases && for yaml in ashrae140_case[0-9ACH]*.yaml; do
  ../../target/release/openbse "$yaml" || break
done)
python3 140_tests/scripts/build_140_csv.py
```

## Directory Structure

```
cases/              96 YAML input files (ashrae140_case*.yaml), plus two scratch variants
weather/            Prescribed weather data for each section
reference_idfs/     EnergyPlus IDF files for cross-reference
scripts/            Case generators, the gate, and submission generators
  build_140_csv.py    Reads the case outputs and compares them against the gate ranges
  generate_*_cases.py Write the case YAMLs for Sections 9A, 9B, 10 and 11
  generate_ashrae140_*submission.py  Fill the standard's output spreadsheets
  solar_validation.py Solar calculation chain validation
results/            Filled submission spreadsheets (OpenBSE_Std140_*_Output)
acceptance_ranges_140_2023.json  Gate ranges and how each was derived
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

### Section 9A — Cooling Equipment, Analytical Verification (CE100–CE200, 14 cases)

A window-type DX unit in an adiabatic zone with constant internal gains and
constant outdoor conditions, compared with the quasi-analytical solutions for
February. Generated by `scripts/generate_ce_cases.py`; gated on 112 checks
(8 metrics × 14 cases).

| Cases | Outdoor / zone setpoint (°C) | Sensible / latent gain (W) |
|-------|------------------------------|----------------------------|
| CE100 | 46.1 / 22.2 | 5400 / 0 |
| CE110, CE120 | 29.4 / 22.2 and 26.7 | 5400 / 0 |
| CE130, CE140 | 46.1 and 29.4 / 22.2 | 270 / 0 |
| CE150, CE160 | 29.4 / 22.2 and 26.7 | 5400 / 1100 |
| CE165 | 40.6 / 23.3 | 5400 / 1100 |
| CE170, CE180 | 29.4 / 22.2 | 2100 / 1100 and 4400 |
| CE185 | 46.1 / 22.2 | 2100 / 4400 |
| CE190, CE195 | 29.4 and 46.1 / 22.2 | 270 / 550 |
| CE200 | 35.0 / 26.7 | 6120 / 1817 |

### Section 9B — Dynamic Cooling Equipment (CE300–CE545, 20 cases)

Annual runs of a performance-map unitary DX system in a near-adiabatic zone
with New Orleans weather (`CE300.csv`). Generated by
`scripts/generate_ce_b_cases.py`; gated on 231 annual checks (11 metrics × 21
result rows — CE500 is reported for the full year and for May–Sep, CE510 for
May–Sep only). `scripts/generate_ashrae140_CE_b_submission.py` fills the
portal template.

| Cases | What varies |
|-------|-------------|
| CE300 | Base case: 15% outdoor air, continuous fan |
| CE310, CE360 | High latent gain; high sensible gain |
| CE320, CE330, CE340 | Scheduled infiltration; scheduled outdoor air; both |
| CE350 | Summer night setup of the cooling setpoint |
| CE400–CE440 | Economizer: dry-bulb, with compressor lockout, with a dry-bulb cap, enthalpy, with an enthalpy cap |
| CE500, CE510 | No outdoor air, cycling fan; high load |
| CE520, CE522, CE525 | Cooling setpoint 15 / 20 / 35 °C |
| CE530, CE540, CE545 | Dry coil at 25 / 15 / 35 °C |

The Section 9B example results are informative, not acceptance criteria. The
gate range for each metric is the spread of the six example programs widened by
5% of their mean (see `_provenance_ce_b` in `acceptance_ranges_140_2023.json`).

### Section 10 — Heating Equipment (HE100–HE230, 11 cases)

A fuel-fired furnace heating an adiabatic zone with constant infiltration to
20 °C, run January–March. Generated by `scripts/generate_he_cases.py`; gated on
48 checks.

| Cases | What varies |
|-------|-------------|
| HE100 | 100% efficient furnace, constant −30 °C outdoors, no fans |
| HE110 | 80% efficiency |
| HE120, HE130, HE140 | Constant 0 °C; constant 20 °C (no load); 0 ± 20 °C varying |
| HE150, HE160 | 200 W circulating fan, continuous; cycling |
| HE170 | Continuous circulating fan plus a 50 W draft fan |
| HE210 | Realistic weather, cycling circulating fan, draft fan |
| HE220 | HE210 with night setback to 15 °C |
| HE230 | HE220 with an undersized 5 kW furnace |

### Section 11 — Air-Side HVAC Equipment (AE101–AE445, 24 cases)

Adiabatic zones with imposed sensible and latent loads, served by air handlers
with outdoor-air mixing. Results are steady state: the last simulated hour is
compared with the quasi-analytical solution. Generated by
`scripts/generate_ae_cases.py` and `scripts/generate_ae_reheat_cases.py`; gated
on 96 checks (coil loads, 4 per case).

| Cases | System |
|-------|--------|
| AE101, AE103, AE104 | Fan-coil air handler, supply fan only |
| AE201–AE245 | Single-zone air handler with supply and return fans; AE226 and AE245 add dry-bulb and enthalpy economizers |
| AE301–AE345 | Two-zone constant-volume terminal reheat |
| AE401–AE445 | Two-zone VAV terminal reheat |

