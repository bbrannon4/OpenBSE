# Building envelope {#sec-envelope}

::: {.callout-warning}
## Pre-review content

This chapter holds text moved unchanged, apart from format, from the former single-file reference (`README.md`, mostly written in March 2026). Parts of it are stale or incomplete. It will be rewritten from the code, and checked against EnergyPlus, in this chapter's review ([#181](https://github.com/bbrannon4/OpenBSE/issues/181)); see @sec-status.
:::

## Building Envelope

**Crate:** `openbse-envelope`

The building envelope module solves the thermal physics of the building shell: heat conduction through opaque walls (via CTF), convection at interior and exterior surfaces, solar radiation processing, infiltration of outdoor air, and internal heat gains from occupants, lights, and equipment.

### Materials and Constructions

**Module:** `material.rs`

#### Material

An opaque material layer with thermal and surface properties.

| Field | Unit | Default | Description |
|-------|------|---------|-------------|
| `name` | — | required | Material name |
| `conductivity` | W/(m·K) | required | Thermal conductivity |
| `density` | kg/m³ | required | Density |
| `specific_heat` | J/(kg·K) | required | Specific heat capacity |
| `thickness` | m | 0.1 | Layer thickness |
| `solar_absorptance` | 0–1 | 0.7 | Solar radiation absorptance |
| `thermal_absorptance` | 0–1 | 0.9 | Longwave/thermal radiation absorptance |
| `visible_absorptance` | 0–1 | 0.7 | Visible light absorptance |
| `roughness` | enum | MediumRough | Surface roughness classification |

**Derived properties:**
- Thermal resistance: `R = thickness / conductivity` [m²·K/W]
- Thermal diffusivity: `α = conductivity / (density · specific_heat)` [m²/s]

#### Roughness

Affects exterior forced convection coefficient. Values and their forced-convection multipliers:

| Roughness | Multiplier |
|-----------|------------|
| VeryRough | 2.17 |
| Rough | 1.67 |
| MediumRough | 1.52 |
| MediumSmooth | 1.13 |
| Smooth | 1.11 |
| VerySmooth | 1.00 |

**Reference:** EnergyPlus `ConvectionCoefficients.cc`

#### Construction

A multi-layer opaque construction. Layers are ordered outside to inside.

| Field | Description |
|-------|-------------|
| `name` | Construction name |
| `layers` | Layer names, outside → inside |

**Derived properties:**
- Total resistance: `R_total = Σ R_layer` [m²·K/W]
- U-factor: `U = 1 / R_total` [W/(m²·K)] (no film coefficients included)

#### Window Construction

Windows use a simplified performance-based model.

| Field | Unit | Default | Description |
|-------|------|---------|-------------|
| `name` | — | required | Name |
| `u_factor` | W/(m²·K) | required | Overall U-factor (includes film coefficients) |
| `shgc` | 0–1 | required | Solar Heat Gain Coefficient at normal incidence |
| `visible_transmittance` | 0–1 | 0.6 | Visible light transmittance |

---

### Simple Constructions

**Module:** `material.rs`

**Purpose:** Provides a simplified opaque construction model defined by overall thermal properties rather than individual material layers. Ideal for early design, ASHRAE 140 test cases, and quick parametric studies where full layer-by-layer definition is unnecessary.

#### Inputs

| Parameter | Unit | Default | Description |
|-----------|------|---------|-------------|
| `name` | — | required | Construction name |
| `u_factor` | W/(m2-K) | required | Overall U-factor (conductance, no film coefficients) |
| `thickness` | m | 0.2 | Total wall thickness |
| `thermal_capacity` | J/(m2-K) | 50,000 | Thermal capacity per unit area (rho * cp * thickness sum) |
| `solar_absorptance` | 0-1 | 0.7 | Outside solar absorptance |
| `thermal_absorptance` | 0-1 | 0.9 | Thermal (longwave) absorptance |
| `roughness` | enum | MediumRough | Surface roughness classification |

The `thermal_capacity` default of 50,000 J/(m2-K) corresponds to a light construction. A heavyweight concrete wall might have 200,000-400,000 J/(m2-K).

The simple construction is converted internally to an equivalent single-layer material for CTF calculation, preserving the same steady-state and first-order transient behavior as the specified U-factor and thermal capacity.

---

### Conduction Transfer Functions (CTF)

**Module:** `ctf.rs`

**Purpose:** Computes heat conduction through multi-layer opaque wall constructions. CTFs encode the transient thermal response of a wall into a set of coefficients that relate surface temperatures and heat fluxes across timesteps.

**Reference:** Seem (1987), EnergyPlus Engineering Reference Chapter on CTF.

#### CTF Equations

The per-timestep conduction equations are:

```
q_inside  = Σⱼ Y[j]·T_out(t-j) − Σⱼ Z[j]·T_in(t-j) + Σⱼ Φ[j]·q_in(t-j)
q_outside = Σⱼ X[j]·T_out(t-j) − Σⱼ Y[j]·T_in(t-j) + Σⱼ Φ[j]·q_out(t-j)
```

where:
- `X` — outside surface CTF coefficients
- `Y` — cross CTF coefficients (linking outside and inside)
- `Z` — inside surface CTF coefficients
- `Φ` — flux history coefficients
- `j=0` is the current timestep, `j≥1` are past timesteps

Sign convention: `q_inside` positive = heat flowing into the zone.

#### Coefficients

| Symbol | Name | Description |
|--------|------|-------------|
| X[j] | Outside | Relate outside surface temperature to outside heat flux |
| Y[j] | Cross | Relate outside temperature to inside flux (and vice versa) |
| Z[j] | Inside | Relate inside surface temperature to inside heat flux |
| Φ[j] | Flux history | Weight previous flux values for transient response |

#### Implementation: Full State-Space Method (Seem 1987)

OpenBSE implements the full Seem (1987) state-space CTF method, matching EnergyPlus. This is implemented in `ctf.rs`.

**Key steps:**

1. **Layer discretization**: Each material layer is divided into nodes with spacing `dx = sqrt(2·α·dt)`, minimum 6 nodes per layer.

2. **State-space matrix construction**: The A (node coupling), B (boundary input), C (output), and D (feedthrough) matrices are assembled from layer thermal properties. NoMass layers are folded into boundary conductance terms.

3. **Matrix exponential**: `exp(A·dt)` computed via Taylor series with scaling and squaring for numerical stability.

4. **CTF coefficient extraction**: The R-matrix recurrence iteratively computes X, Y, Z, and Φ coefficient vectors (up to 18 terms) until convergence.

5. **Reciprocity enforcement**: Cross-term symmetry `average(|s(0,1)|, |s(1,0)|)` ensures energy conservation.

**Fallback modes:**
- **Low thermal mass** (`C_total < 1 kJ/m²·K`): Pure steady-state CTF (`X=Y=Z=U`, no flux history)
- **Degenerate cases**: First-order lumped RC as fallback if the state-space method produces NaN
- **Synthetic constructions**: Heuristic layer synthesis for `SimpleConstruction` inputs that lack explicit material layers

#### CTF History

Each surface maintains a history of past surface temperatures and heat fluxes. At each timestep, the history is shifted: current values are pushed in, the oldest values fall off.

---

### Interior Convection

**Module:** `convection.rs`

**Purpose:** Computes the convective heat transfer coefficient between interior surfaces and zone air.

**Model:** ASHRAE simple algorithm (Walton, 1983)

**Reference:** Walton (1983), EnergyPlus `ConvectionCoefficients.cc`

#### Function

```
h_conv = interior_convection(T_surface, T_zone, tilt_deg)
```

**Inputs:**
| Parameter | Unit | Description |
|-----------|------|-------------|
| `t_surface` | °C | Interior surface temperature |
| `t_zone` | °C | Zone air temperature |
| `tilt_deg` | degrees | Surface tilt from horizontal |

**Output:** Interior convection coefficient [W/(m²·K)]

#### Algorithm

**Near-vertical surfaces** (tilt 60°–120°, i.e. |cos(tilt)| < 0.5):
```
h = 1.31 · |ΔT|^(1/3)
```

**Near-horizontal surfaces** (|cos(tilt)| ≥ 0.5):

Stability is determined by the direction of buoyancy:
- **Stable** (warm surface facing down, or cool surface facing up): stratification resists convection
- **Unstable** (warm surface facing up, or cool surface facing down): buoyancy drives convection

```
Stable:    h = 1.810 · |ΔT|^(1/3) / (1.382 + |cos(tilt)|)
Unstable:  h = 9.482 · |ΔT|^(1/3) / (7.238 − |cos(tilt)|)
```

**Minimum:** 0.1 W/(m²·K) to prevent numerical issues with very small temperature differences.

---

### Exterior Convection

**Module:** `convection.rs`

**Purpose:** Computes the convective heat transfer coefficient between exterior surfaces and the outdoor environment.

**Model:** TARP (Thermal Analysis Research Program) — combined natural + forced convection.

**Reference:** Walton (1983), EnergyPlus Engineering Reference

#### Function

```
h_conv = exterior_convection(T_surface, T_outdoor, wind_speed, tilt_deg, roughness)
```

#### Algorithm

**Natural convection component** — uses the same Walton correlations as interior convection:
```
H_natural = interior_convection(T_surface, T_outdoor, tilt_deg)
```

**Forced convection component** — wind-driven, with a roughness multiplier:
```
H_forced = R_f · 2.537 · V_wind^0.5
```

where R_f is the roughness multiplier (see [Roughness](#roughness) table).

**Combined (TARP model):**
```
H = sqrt(H_natural² + H_forced²)
```

**Minimum:** 0.1 W/(m²·K)

### Interior Solar Distribution

**Module:** `envelope.rs` (solar distribution logic within the zone heat balance)

**Purpose:** Distributes transmitted window solar gains onto interior zone surfaces. Two methods are implemented, selected via the `solar_distribution` YAML field.

#### FullExterior (Default)

All transmitted beam solar is distributed to floor surfaces, weighted by floor area:

```
Q_beam_floor_i = Q_beam_total × (A_floor_i / A_floor_total)
```

Diffuse transmitted solar is distributed to all interior surfaces using a VMULT factor:

```
VMULT = 1 / Σ(A_i × α_i)    for all interior surfaces
Q_diffuse_surface_i = Q_diffuse_total × A_i × α_i × VMULT
```

where `α_i` is the interior solar absorptance of surface `i`. This matches the EnergyPlus FullExterior algorithm.

#### FullInteriorAndExterior

Beam solar is geometrically projected through each window onto interior surfaces using the actual sun direction vector. The algorithm:

1. For each window, project a beam rectangle onto the plane of each interior surface using the solar direction vector.
2. Clip the projected rectangle against the receiving surface polygon using Sutherland-Hodgman polygon clipping.
3. Assign beam solar proportional to the clipped area on each surface.
4. Any beam solar that doesn't hit a surface (e.g., exits through another window) falls back to the FullExterior floor distribution.

Diffuse distribution uses the same VMULT method as FullExterior.

#### Reflected Beam Handling (Deliberate Deviation from EnergyPlus)

After beam solar is absorbed by its target surface (absorptance `α`), the reflected fraction `(1 - α)` enters the diffuse pool for redistribution via VMULT. This differs from EnergyPlus, which keeps reflected beam localized to the surface that received it.

**Rationale:** In reality, reflected beam bounces multiple times off interior surfaces. The infinite-bounce solution converges to a distribution governed by the radiosity equation `(I - ρF)⁻¹`, where `F` is the view factor matrix. Placing reflected beam into the diffuse pool (area-weighted) is a closer approximation to this converged state than E+'s single-bounce localization. The approximation omits view-factor weighting between surfaces, which is acceptable for typical rectangular rooms but could diverge for unusual geometries.

OpenBSE also omits E+'s direct solar-to-zone-air fraction, which E+ uses to account for solar absorbed by unmodeled interior objects (furniture, carpets) that release heat convectively with minimal time lag. This is a known simplification — see STATUS.md for details.

---

### External Shading

**Module:** `shading.rs`

**Purpose:** Computes the shading effect of overhangs, fins, and other external shading surfaces on building surfaces. Two types of shading are calculated: beam (directional) and diffuse (sky dome).

#### Beam Shadow Calculation

**Function:** `calculate_sunlit_fraction(surface_vertices, caster_polygons, sun_direction)`

**Algorithm:** For each timestep, the sun direction is used to project shading surface (caster) polygons onto the receiving surface plane. The projected shadow polygon is clipped against the receiving surface using the Sutherland-Hodgman polygon clipping algorithm. The sunlit fraction is `1 - (shadow_area / surface_area)`.

**Multi-caster handling:** When multiple casters shade the same surface (e.g., overhang + two fins), an 8x8 grid sampling method is used with UNION logic (a point is shaded if ANY caster shadows it). For single casters, exact polygon area is used for efficiency.

#### Diffuse Sky Shading Ratios

**Function:** `compute_diffuse_sky_shading_ratio(surface_vertices, caster_polygons)`

**Algorithm:** Samples the sky hemisphere using 144 patches (6 altitude x 24 azimuth directions). For each patch, checks if any caster blocks the line of sight from 5x5 sample points on the receiving surface. The ratio of unblocked patches (weighted by `cos(altitude)`) to the total gives the diffuse sky shading ratio.

Two ratios are computed:
- **DifShdgRatioIsoSky:** Fraction of isotropic sky dome visible (patches above 10 deg altitude)
- **DifShdgRatioHoriz:** Fraction of horizon band visible (patches below 10 deg altitude)

These ratios are precomputed once at startup and stored per surface.

#### Overhang and Fin Geometry Generation

Overhangs and fins can be specified as either:
1. **Explicit vertices** in the `shading_surfaces` section
2. **Auto-generated** from `shading: overhang/left_fin/right_fin` parameters on surfaces

Auto-generation creates overhang/fin polygons based on:
- `depth` -- projection distance from wall surface
- `offset_above` -- vertical offset above window (overhangs, default 0)
- `left_extension/right_extension` -- lateral extensions (overhangs, default 0)
- `extend_above/extend_below` -- vertical extensions (fins, default 0)

---

### Infiltration

**Module:** `infiltration.rs`

**Purpose:** Computes outdoor air infiltration into zones based on temperature difference and wind speed.

**Model:** EnergyPlus Design Flow Rate model

**Reference:** EnergyPlus `HeatBalanceAirManager.cc`

#### Inputs

| Parameter | Unit | Default | Description |
|-----------|------|---------|-------------|
| `design_flow_rate` | m³/s | 0.0 | Design infiltration volume flow |
| `air_changes_per_hour` | 1/hr | 0.0 | Alternative: ACH-based specification |
| `coeff_a` | — | 1.0 | Constant coefficient |
| `coeff_b` | 1/°C | 0.0 | Temperature difference coefficient |
| `coeff_c` | s/m | 0.0 | Wind speed coefficient |
| `coeff_d` | s²/m² | 0.0 | Wind speed squared coefficient |

If `air_changes_per_hour` is specified and `design_flow_rate` is zero, the flow rate is computed as:
```
Q_design = ACH · Volume / 3600
```

#### Algorithm

**Volume flow rate:**
```
Q = Q_design · (A + B·|T_zone − T_outdoor| + C·V_wind + D·V_wind²)
```

**Mass flow rate:**
```
ṁ = Q · ρ_outdoor
```

where `ρ_outdoor` is the outdoor air density at current conditions.

---

### Airflow Network (Multizone Pressure Solver)

**Module:** `airflow_network.rs`

**Purpose:** Solves for pressure-driven airflow through the building envelope and between zones using Newton-Raphson iteration on the mass conservation equations. Replaces the Design Flow Rate infiltration model when enabled.

**Reference:** ASHRAE Fundamentals Ch. 16; Swami & Chandra, FSEC-CR-163-86 (1988); E+ AirflowNetwork model concepts.

#### Flow Elements

| Element | Equation | Use |
|---------|----------|-----|
| Power-law crack | Q = C \|ΔP\|^n sign(ΔP), n ≈ 0.65 | Envelope cracks, interzone gaps |
| Large opening (orifice) | Q = Cd A sqrt(2ρ\|ΔP\|) sign(ΔP) | Operable windows, doors |
| Fixed flow | Q = constant | Exhaust fans, HVAC imbalance |

#### Pressure Difference

Total ΔP across a flow path includes:
- **Node pressures:** P_a - P_b (solved unknowns for zone nodes)
- **Stack effect:** -ρ_a g (h - h_ref_a) + ρ_b g (h - h_ref_b) where h is the opening midpoint height
- **Wind pressure:** 0.5 ρ Cp V_local² (exterior paths only, V_local height-corrected)

#### Wind Pressure Coefficients (Cp)

- **Swami & Chandra (1988):** For low-rise buildings. Cp is a function of the angle θ between wind direction and surface outward normal, and the building plan side ratio.
- **High-rise simplified:** Cp = 0.6 cos(θ) for windward, Cp = -0.3 for leeward.

#### Newton-Raphson Solver

1. Assemble residual R_i = sum of mass flows into zone node i
2. Build dense Jacobian J[i][j] = dR_i / dP_j (analytical derivatives)
3. Solve J δP = -R via Gaussian elimination with partial pivoting
4. Damped update: P_i += α δP_i (α = 0.75 default)
5. Converge when max|δP| < tolerance (0.1 Pa default)

#### Auto-Generation

The network is built automatically from building geometry:
- Every exterior wall/roof surface → power-law crack (leakage_per_area × net_area)
- Every exterior window → power-law crack (window_leakage_per_area × area)
- Every interzone surface → interzone crack (one path per pair)
- Zone-level `natural_ventilation` config → large opening
- Zone-level `exhaust_fan` → fixed-flow extraction

#### Integration

When active, the solved outdoor mass flow replaces `infiltration_mass_flow` in the zone heat balance MCPI term. Interzone flows enter at their source zone temperature.

---

### Internal Gains

**Module:** `internal_gains.rs`

**Purpose:** Computes convective and radiative heat gains from occupants, lighting, and equipment.

#### Gain Types

**People:**
```
Q_sensible = count · activity_level · sensible_fraction   [W]
Q_latent   = count · activity_level · (1 − sensible_fraction) [W]
Q_radiative = Q_sensible · radiant_fraction
Q_convective = Q_sensible · (1 − radiant_fraction)
```

Alternatively, `sensible_gain_per_person` and `latent_gain_per_person` can be specified directly to override the activity_level × fraction calculation.

| Parameter | Unit | Default | Description |
|-----------|------|---------|-------------|
| `count` | — | required | Number of occupants |
| `activity_level` | W/person | 120.0 | Total metabolic heat output per person |
| `sensible_fraction` | 0–1 | 0.6 | Fraction of metabolic heat that is sensible |
| `radiant_fraction` | 0–1 | 0.3 | Fraction of sensible heat that is radiative |
| `sensible_gain_per_person` | W/person | — | Override: explicit sensible gain |
| `latent_gain_per_person` | W/person | — | Override: explicit latent gain |

**Lights:**
```
Q_input = power                                            [W]
Q_return_air = power · return_air_fraction
Q_zone = power · (1 − return_air_fraction)
Q_radiative = Q_zone · radiant_fraction
Q_convective = Q_zone · (1 − radiant_fraction)
```

`Q_return_air` goes to the air leaving the zone for the return duct, as in
EnergyPlus (`ZoneEquipmentManager::CalcZoneLeavingConditions`). It raises the
zone's return-air temperature by `Q_return_air / (ṁ_return · cp)`, up to
EnergyPlus's 60 °C limit, so the coils, the economizer and heat recovery see it.
`ṁ_return` is the air the zone returns to a central loop (VAV, dual-duct, DOAS,
CRAC/CRAH) or to a PSZ unit with a continuous fan. With no such return (zonal
units such as FCU, PTAC and PTHP, cycling-fan PSZ units, a loop that is off,
ideal loads or sizing), the heat stays in the zone air as a convective gain,
which is what EnergyPlus does through `Zone::NoHeatToReturnAir`.

| Parameter | Unit | Default | Description |
|-----------|------|---------|-------------|
| `power` | W | required | Total installed lighting power |
| `radiant_fraction` | 0–1 | 0.7 | Fraction of zone heat (`Q_zone`) that is radiative |
| `return_air_fraction` | 0–1 | 0.0 | Fraction of heat to the zone's return air |

**Equipment:**
```
Q_total     = power                                        [W]
Q_to_zone   = Q_total · (1 − lost_fraction)                [W]
Q_latent    = Q_to_zone · latent_fraction                   [W]
Q_sensible  = Q_to_zone − Q_latent                          [W]
Q_radiative = Q_sensible · radiant_fraction
Q_convective = Q_sensible · (1 − radiant_fraction)
```

| Parameter | Unit | Default | Description |
|-----------|------|---------|-------------|
| `power` | W | required | Total equipment power |
| `radiant_fraction` | 0–1 | 0.3 | Fraction of sensible heat that is radiative |
| `lost_fraction` | 0–1 | 0.0 | Fraction of heat that does not enter the zone (e.g., elevator shaft) |
| `latent_fraction` | 0–1 | 0.0 | Fraction of non-lost heat that generates moisture (e.g., kitchen, laundry) |

The resolved gain output provides `convective` [W], `radiative` [W], `total` [W] (sensible only), `people_latent` [W], and `equipment_latent` [W]. Latent gains feed the zone moisture balance.

Internal gains support time-varying operation through the schedule system (see [Schedules](#schedules)). Each gain type can reference a named schedule; the schedule fraction multiplies the design power at each timestep.

### Exhaust Fans

**Module:** `zone.rs`

**Purpose:** Models zone-level exhaust fans (restroom exhaust, kitchen hoods, etc.) that remove air from the zone. The exhausted air is replaced by infiltration or transfer air from adjacent spaces, creating an additional air exchange load on the zone.

#### Inputs

| Parameter | Unit | Description |
|-----------|------|-------------|
| `flow_rate` | m3/s | Exhaust volume flow rate |
| `schedule` | — | Optional schedule name for time-varying operation (default: always on) |

The exhaust fan is specified as part of a zone definition. When the schedule is active, the fan removes air at the specified flow rate, which is converted to mass flow using outdoor air density and added to the zone's air exchange calculation.

### Vertex Geometry

**Module:** `geometry.rs`

**Purpose:** Provides 3D polygon-based geometry for building surfaces, enabling automatic calculation of surface areas, orientations, and zone volumes from vertex coordinates.

**Reference:** Newell (1972), EnergyPlus `SurfaceGeometry.cc`

#### Coordinate System

Right-hand coordinate system: X = East, Y = North, Z = Up. Vertices are ordered counter-clockwise when viewed from outside the building (Newell convention).

#### Point3D

A vertex in building coordinates:

| Field | Unit | Description |
|-------|------|-------------|
| `x` | m | East coordinate |
| `y` | m | North coordinate |
| `z` | m | Up coordinate |

#### Newell's Method

Computes the outward normal vector and area of a polygon from its vertices:

```
For each vertex pair (i, j = i+1 mod n):
    Nx += (Yi - Yj) * (Zi + Zj)
    Ny += (Zi - Zj) * (Xi + Xj)
    Nz += (Xi - Xj) * (Yi + Yj)

Area = |N| / 2
```

The magnitude of the Newell normal vector equals twice the polygon area.

#### Derived Properties

**Azimuth** (from surface normal):
```
azimuth = atan2(Nx, Ny)    [mapped to 0-360 degrees]
```
0 = North, 90 = East, 180 = South, 270 = West.

**Tilt** (from surface normal):
```
tilt = acos(Nz / |N|)      [degrees]
```
0 = face-up (horizontal roof), 90 = vertical wall, 180 = face-down (floor).

**Zone volume** (divergence theorem):
```
V = (1/3) * |Sum over faces of (N_hat . P_centroid) * A_face|
```

Requires surfaces to form a closed volume with outward-pointing normals.

**Zone floor area:**

Sum of polygon areas for all surfaces with tilt > 150 degrees (downward-pointing normals, identifying floor surfaces).

#### Auto-Calculation

When a zone's volume or floor area is specified as 0.0 in the input, the engine automatically computes these values from the zone's surface vertices using the above algorithms. This avoids manual calculation for complex geometries.

---

### Ground Temperature Model

**Module:** `ground_temp.rs`

**Purpose:** Computes ground temperature as a function of depth and time of year using the Kusuda-Achenbach sinusoidal model. Used for floor and below-grade wall boundary conditions.

**Reference:** Kusuda & Achenbach (1965), ASHRAE Handbook of Fundamentals

#### Equation

```
T(z,t) = T_mean - A * exp(-z * sqrt(pi / (365 * alpha)))
         * cos(2*pi/365 * (t - t_shift - z/2 * sqrt(365 / (pi * alpha))))
```

| Symbol | Unit | Description |
|--------|------|-------------|
| `T_mean` | C | Annual mean ground surface temperature |
| `A` | C | Annual surface temperature amplitude (half of peak-to-peak) |
| `z` | m | Depth below surface |
| `t` | days | Day of year |
| `t_shift` | days | Day of minimum surface temperature (typically ~35 for northern hemisphere) |
| `alpha` | m2/day | Soil thermal diffusivity (default 0.04 m2/day) |

#### Inputs

| Parameter | Unit | Default | Description |
|-----------|------|---------|-------------|
| `t_mean` | C | 10.0 | Annual mean ground temperature |
| `amplitude` | C | 10.0 | Surface temperature amplitude |
| `phase_day` | days | 35.0 | Day of minimum surface temperature |
| `soil_diffusivity` | m2/day | 0.04 | Soil thermal diffusivity |
| `depth` | m | 0.0 | Depth below surface |

#### Auto-Calibration from Weather Data

When weather data is available, the model parameters are automatically derived:

1. Compute monthly average dry-bulb temperatures from hourly weather data.
2. `T_mean` = annual average of monthly means.
3. `Amplitude` = (max monthly average - min monthly average) / 2.
4. `Phase_day` = mid-month day of year for the coldest month.
5. `Soil_diffusivity` = 0.04 m2/day (typical soil, not derived from weather).

#### Physical Behavior

- At the surface (z = 0), temperature follows the full annual swing.
- With increasing depth, the exponential damping factor `exp(-z * sqrt(pi/(365*alpha)))` reduces the amplitude.
- At great depth (z >> 0), temperature converges to `T_mean` (the damping factor approaches zero).
- There is also a phase lag: deeper temperatures peak later in the year due to the `z/2 * sqrt(365/(pi*alpha))` term.

---

### Zone Air Heat Balance

**Module:** `zone.rs`

**Purpose:** Solves for the zone air temperature using an energy balance that accounts for surface convection, infiltration, HVAC supply, internal gains, and zone air thermal capacitance.

**Reference:** EnergyPlus predictor-corrector method (HeatBalanceAirManager)

#### Equation

```
         SumHAT + MCPI·T_outdoor + MCPSYS·T_supply + Q_conv + Cap·T_prev
T_zone = ─────────────────────────────────────────────────────────────────
                    SumHA + MCPI + MCPSYS + Cap
```

| Term | Definition | Unit |
|------|-----------|------|
| SumHA | Σ(h_conv,i · A_i) for all zone surfaces | W/K |
| SumHAT | Σ(h_conv,i · A_i · T_surface,i) for all zone surfaces | W |
| MCPI | ṁ_infiltration · Cp_air | W/K |
| MCPSYS | ṁ_HVAC_supply · Cp_air | W/K |
| Q_conv | Total convective gains (internal + solar transmitted + window conduction) | W |
| Cap | ρ_air · V_zone · Cp_air / dt | W/K |
| T_prev | Zone temperature from previous timestep | °C |

The `Cap · T_prev` term in the numerator and `Cap` in the denominator provide the thermal capacitance of the zone air mass — this is what gives the solution its transient behavior. Without it (dt → ∞), the equation reduces to a pure steady-state balance.

#### Zone Loads

Heating and cooling loads are the residual of the energy balance without the HVAC term:

```
Load = SumHAT − SumHA·T_zone + MCPI·(T_outdoor − T_zone) + Q_conv + Cap·(T_prev − T_zone)/dt
```

- If Load > 0: heating load (zone losing more heat than it gains)
- If Load < 0: cooling load (zone gaining more heat than it loses)

---

### Zone Air Moisture Balance

**Module:** `zone.rs`

**Purpose:** Solves for the zone air humidity ratio using a mass balance that accounts for infiltration, HVAC supply, and latent gains from people and equipment. Uses the same 3rd-order Backward Differentiation Formula (BDF) integration as the temperature solver.

**Reference:** EnergyPlus ZoneAirMoisturePredictorCorrector

#### Equation

```
         Cap_w · W_prev + ṁ_infil · W_outdoor + ṁ_supply · W_supply + Q_latent / h_fg
W_zone = ──────────────────────────────────────────────────────────────────────────────
                          Cap_w + ṁ_infil + ṁ_supply
```

| Term | Definition | Unit |
|------|-----------|------|
| Cap_w | ρ_air · V_zone / dt_eff | kg/s |
| W_prev | BDF-effective previous humidity ratio | kg/kg |
| ṁ_infil | Infiltration mass flow rate | kg/s |
| W_outdoor | Outdoor air humidity ratio | kg/kg |
| ṁ_supply | HVAC supply air mass flow rate | kg/s |
| W_supply | HVAC supply air humidity ratio | kg/kg |
| Q_latent | Total latent gains (people + equipment) | W |
| h_fg | Latent heat of vaporization (2,501,000) | J/kg |

The BDF integration uses `backward_diff_effective()` to compute `dt_eff` and `W_prev` from up to 3 previous timesteps, matching the temperature solver's approach. The result is clamped to [0, 0.10] kg/kg.

#### Moisture Sources

- **Infiltration**: Outdoor air at ambient humidity
- **HVAC supply**: Mass-flow-weighted humidity from air loops serving the zone
- **People latent**: Metabolic moisture from occupants (activity_level × (1 - sensible_fraction))
- **Equipment latent**: Moisture-generating equipment (power × (1 - lost_fraction) × latent_fraction)

#### Output Variables

| Variable | Unit | Description |
|----------|------|-------------|
| `zone_humidity_ratio` | kg/kg | Zone air humidity ratio |

---

### Heat Balance Solver

**Module:** `heat_balance.rs`

**Purpose:** Orchestrates the full envelope heat balance: links solar, conduction (CTF), convection, infiltration, internal gains, and zone air balance into a coupled solution.

This is the central solver of the envelope module. It implements the `EnvelopeSolver` trait defined in `openbse-core`.

#### BuildingEnvelope

| Field | Description |
|-------|-------------|
| `zones` | Runtime zone states (temperature, humidity, loads, surface links) |
| `surfaces` | Runtime surface states (temps, fluxes, convection coefficients) |
| `ctf_coefficients` | CTF coefficients for each opaque surface |
| `ctf_histories` | Past surface temps and fluxes for each surface |
| `materials` | Material property lookup table |
| `constructions` | Construction layer lookup table |
| `window_constructions` | Window property lookup table |
| `latitude`, `longitude`, `time_zone` | Site location for solar calculations |
| `ground_reflectance` | Ground albedo (default 0.2) |

#### Initialization

When `initialize(dt)` is called:
1. Resolve material properties onto each surface (absorptances, roughness, U-factor, SHGC).
2. Identify windows vs. opaque surfaces.
3. Subtract window areas from their parent walls' net areas.
4. Assign surfaces to their zones.
5. Compute CTF coefficients for all opaque surfaces.
6. Initialize CTF histories at 21°C.

#### Per-Timestep Algorithm

```
solve_timestep(ctx, weather, hvac) → EnvelopeResults
```

**Step 1: Solar position**
- Compute day of year, equation of time, solar hour.
- Calculate solar altitude and azimuth.

**Step 2: Incident solar on each surface**
- For outdoor-facing surfaces, compute beam + diffuse + ground-reflected solar.
- For opaque surfaces: absorbed solar = solar_absorptance × incident.
- For windows: transmitted solar = SHGC × area × incident.

**Step 3: Internal gains**
- For each zone, resolve all internal gain types (people, lights, equipment) into convective and radiative components.

**Step 4: Infiltration**
- For each zone, compute infiltration mass flow from design flow rate, temperature difference, and wind speed.

**Step 5: Apply HVAC conditions**
- Read supply air temperatures and mass flows from the HVAC control signals.

**Step 6: Surface ↔ Zone coupling iteration (5 iterations)**

This inner loop iterates to converge the coupled surface temperatures and zone air temperatures:

**6a. Outside surface temperatures:**
- Outdoor surfaces: `T_surface_outside = T_outdoor + Q_solar_absorbed / h_conv_outside`
- Ground contact: `T_surface_outside = T_ground(depth, day_of_year)` via the [Kusuda-Achenbach model](#ground-temperature-model)
- Adiabatic: `T_surface_outside = T_surface_inside`
- Interzone: `T_surface_outside = T_adjacent_zone`

**6b. CTF conduction (opaque surfaces):**
Apply CTF equations to compute inside heat flux from outside/inside temperatures and history.

**6c. Inside surface temperature:**
```
T_surface_inside = T_zone + (q_ctf_in + q_radiative_flux) / h_conv_inside
```

where `q_radiative_flux` is the zone's radiative internal gains distributed proportionally to surface area.

**6d. Window heat transfer:**
```
Q_window_conduction = U_window · (T_outdoor − T_zone)    [W/m²]
Q_window_solar = SHGC · Area · I_incident                [W]
```

**6e. Zone air heat balance:**
Solve the predictor-corrector equation (see [Zone Air Heat Balance](#zone-air-heat-balance)) using:
- Surface convective fluxes (SumHA, SumHAT)
- Infiltration (MCPI)
- HVAC supply (MCPSYS)
- Convective gains (internal + solar transmitted through windows + window conduction)
- Zone air capacitance

**Step 7: Update histories**
- Shift CTF histories with current surface temperatures and fluxes.
- Update zone previous temperatures for next timestep.

**Step 8: Build results**
- Return zone temperatures, humidity ratios, heating/cooling loads, and detailed output variables for each zone.

#### Envelope Results

| Field | Type | Description |
|-------|------|-------------|
| `zone_temps` | HashMap | Zone → air temperature [°C] |
| `zone_humidity` | HashMap | Zone → humidity ratio [kg/kg] |
| `zone_heating_loads` | HashMap | Zone → heating load [W] (positive = needs heating) |
| `zone_cooling_loads` | HashMap | Zone → cooling load [W] (positive = needs cooling) |
| `zone_outputs` | HashMap | Zone → detailed output variables map |
