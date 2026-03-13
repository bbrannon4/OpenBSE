# ASHRAE Standard 140-2023 Gap Analysis for OpenBSE

Last updated: 2026-03-13

## Section 7: Building Thermal Envelope and Fabric Load Tests

### Full Results: 27 Cases, 48/63 Pass (76.2%)

Results from `tests/ashrae140/results/FULL_140_RESULTS.csv`.

#### Load Cases (600 Series — Low-Mass)

| Case | Feature | Heating | Cooling | Status |
|------|---------|---------|---------|--------|
| 600 | Base case | 4,419 (3,993–4,504) PASS | 6,096 (5,432–6,162) PASS | 2/2 |
| 610 | South overhang | 4,509 (4,066–4,592) PASS | 4,377 (4,117–4,382) PASS | 2/2 |
| 620 | East/west windows | 4,599 (4,094–4,719) PASS | 4,212 (3,841–4,404) PASS | 2/2 |
| 630 | East/west shading | 5,009 (4,356–5,139) PASS | 2,602 (2,573–3,074) PASS | 2/2 |
| 640 | Setback thermostat | 2,642 (2,403–2,682) PASS | 5,820 (5,237–5,893) PASS | 2/2 |
| 650 | Night ventilation | 0 (0–0) PASS | 4,842 (4,186–4,945) PASS | 2/2 |
| 660 | Low-e argon windows | 3,622 (3,574–3,821) PASS | 3,318 (2,966–3,340) PASS | 2/2 |
| 670 | Single-pane windows | 5,566 (5,300–6,140) PASS | 6,396 (5,954–6,623) PASS | 2/2 |
| 680 | Increased insulation | 2,266 (1,732–2,286) PASS | 6,676 (5,932–6,529) **FAIL +2.4%** | 1/2 |
| 685 | 20/20 thermostat | 4,979 (4,532–5,042) PASS | 9,318 (8,238–9,130) **FAIL +2.2%** | 1/2 |
| 695 | Insulation + 20/20 | 2,907 (2,385–2,892) **FAIL +0.6%** | 9,524 (8,386–9,172) **FAIL +4.0%** | 0/2 |

**600 Series Total: 19/22 pass.** Failures are all cooling at the high end of the range (680, 685, 695) — likely caused by excess solar gain from the simplified interior solar distribution model.

#### Load Cases (900 Series — High-Mass)

| Case | Feature | Heating | Cooling | Status |
|------|---------|---------|---------|--------|
| 900 | Base case | 1,795 (1,379–1,814) PASS | 2,511 (2,267–2,714) PASS | 2/2 |
| 910 | South overhang | 2,124 (1,648–2,163) PASS | 1,396 (1,191–1,490) PASS | 2/2 |
| 920 | East/west windows | 3,480 (2,956–3,607) PASS | 2,850 (2,549–3,128) PASS | 2/2 |
| 930 | East/west shading | 4,280 (3,524–4,384) PASS | 1,730 (1,654–2,161) PASS | 2/2 |
| 940 | Setback thermostat | 1,220 (863–1,389) PASS | 2,453 (2,203–2,613) PASS | 2/2 |
| 950 | Night ventilation | 0 (0–0) PASS | 653 (586–707) PASS | 2/2 |
| 960 | Sunspace | 2,460 (2,522–2,860) **FAIL -2.3%** | 1,014 (789–950) **FAIL +7.4%** | 0/2 |
| 980 | Increased insulation | 450 (246–720) PASS | 3,863 (3,501–3,995) PASS | 2/2 |
| 985 | 20/20 thermostat | 2,580 (2,120–2,801) PASS | 6,583 (5,880–7,273) PASS | 2/2 |
| 995 | Insulation + 20/20 | 1,149 (755–1,330) PASS | 7,592 (6,771–7,482) **FAIL +1.5%** | 1/2 |

**900 Series Total: 17/20 pass.** Case 960 (sunspace) is the only complete failure — a complex two-zone case with interzone coupling. Case 995 cooling is marginally over.

#### Free-Float Temperature Cases

| Case | Feature | Peak Max | Peak Min | Mean | Status |
|------|---------|----------|----------|------|--------|
| 600FF | Low-mass free-float | 62.9 (62.4–68.4) PASS | -11.8 (-13.8– -9.9) PASS | 24.7 (24.3–26.1) PASS | 3/3 |
| 650FF | Night vent free-float | 61.5 (61.1–66.8) PASS | -17.2 (-17.8– -16.7) PASS | 17.6 (17.6–18.9) PASS | 3/3 |
| 680FF | Insulated free-float | 70.3 (69.8–78.5) PASS | -6.3 (-8.1– -5.7) PASS | 30.9 (30.2–33.3) PASS | 3/3 |
| 900FF | High-mass free-float | 43.6 (43.3–46.0) PASS | 1.5 (0.6–2.2) PASS | 24.9 (24.5–25.7) PASS | 3/3 |
| 950FF | Night vent free-float | 36.6 (36.1–37.1) PASS | -13.0 (-13.4– -12.5) PASS | 14.5 (14.3–15.0) PASS | 3/3 |
| 980FF | Insulated free-float | 49.1 (48.5–52.8) PASS | 10.7 (7.3–12.5) PASS | 31.0 (30.5–33.3) PASS | 3/3 |

**Free-float: 18/18 pass.** All temperature metrics within acceptance ranges.

#### Case 960 Sun Zone

| Metric | OpenBSE | Range | Status |
|--------|---------|-------|--------|
| Peak Max Temp | 47.7°C | 48.1–53.2 | **FAIL** (-0.4°C) |
| Peak Min Temp | 4.1°C | 4.2–8.0 | **FAIL** (-0.1°C) |
| Mean Temp | 26.7°C | 26.8–29.5 | **FAIL** (-0.1°C) |

**960 SZ: 0/3 pass.** All metrics are marginally below the acceptance range minimum. The sunspace zone temperatures are slightly too low, suggesting slightly too much heat loss or too little solar gain in the sun zone model.

### Summary by Failure Category

| Root Cause | Cases Affected | Impact |
|-----------|---------------|--------|
| Interior solar distribution (excess cooling) | 680, 685, 695, 995 | 4 cooling metrics over max by 1.5–4% |
| Sunspace interzone coupling | 960 | 2 load metrics + 3 temperature metrics (5 total) |
| Case 695 heating (marginal) | 695 | 1 heating metric over max by 0.6% |

### Case 600 Base Spec Details (from ASHRAE 140-2023 Section 7.2.1)
- Geometry: 8m × 6m × 2.7m, 12 m² south windows (2 × 3m × 2m)
- Walls: Plasterboard/Fiberglass/Wood siding (U ≈ 0.514 W/m²K)
- Roof: Plasterboard/Fiberglass/Roofdeck (U ≈ 0.318 W/m²K)
- Floor: Timber/Insulation raised floor (U ≈ 0.039 W/m²K), exposed to outdoor air, no solar
- Windows: Clear double-pane, U = 2.10, SHGC = 0.769, angular-dependent transmittance
- Infiltration: 0.5 ACH constant (altitude-corrected to 0.414 ACH at 1650m)
- Internal gains: 200W continuous, 60% radiative, 40% convective
- HVAC: Ideal 1000 kW heating + 1000 kW cooling, 100% efficient, 100% convective
- Thermostat: Heat ON if T < 20°C, Cool ON if T > 27°C (nonproportional/on-off)
- Interior solar distribution: Floor 64.2%, Ceiling 16.7%, Walls split among orientations
- Ground reflectance: 0.2
- Site altitude: 1650m
