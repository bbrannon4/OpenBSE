# Site, weather and properties {#sec-site}

::: {.callout-warning}
## Pre-review content

This chapter holds text moved unchanged, apart from format, from the former single-file reference (`README.md`, mostly written in March 2026). Parts of it are stale or incomplete. It will be rewritten from the code, and checked against EnergyPlus, in this chapter's review ([#181](https://github.com/bbrannon4/OpenBSE/issues/181)); see @sec-status.
:::

## Psychrometrics

**Crate:** `openbse-psychrometrics` — `lib.rs`

Implements moist air property calculations using the Hyland & Wexler (1983) formulation, consistent with EnergyPlus and ASHRAE Handbook of Fundamentals.

### MoistAirState

The primary data structure representing the thermodynamic state of moist air:

| Field | Type | Unit | Description |
|-------|------|------|-------------|
| `t_db` | f64 | °C | Dry-bulb temperature |
| `w` | f64 | kg/kg | Humidity ratio (kg water per kg dry air) |
| `h` | f64 | J/kg | Specific enthalpy of moist air |
| `p_b` | f64 | Pa | Barometric pressure |

Derived properties (computed on demand):
- `rh()` — relative humidity [0.0–1.0]
- `t_wb()` — wet-bulb temperature [°C]
- `t_dp()` — dew-point temperature [°C]
- `rho()` — moist air density [kg/m³]
- `v()` — specific volume [m³/kg]
- `cp()` — specific heat [J/(kg·K)]

### FluidState

Represents a liquid fluid (water):

| Field | Type | Unit | Description |
|-------|------|------|-------------|
| `temp` | f64 | °C | Fluid temperature |
| `mass_flow` | f64 | kg/s | Mass flow rate |
| `cp` | f64 | J/(kg·K) | Specific heat (4180 for water) |

### Key Functions

**Saturation pressure** (`psat_fn_temp`):

Uses the Hyland & Wexler (1983) equations for water vapor saturation pressure over ice (T < 0 °C) and over liquid water (T ≥ 0 °C). Valid for -100 °C to 200 °C.

**Enthalpy** (`h_fn_tdb_w`):

```
h = Cp_air · T_db + w · (h_fg + Cp_vapor · T_db)
```

where:
- Cp_air = 1004.84 J/(kg·K) — dry air specific heat
- Cp_vapor = 1858.95 J/(kg·K) — water vapor specific heat
- h_fg = 2,500,940 J/kg — heat of vaporization at 0 °C

**Density** (`rho_air_fn_pb_tdb_w`):

Uses the ideal gas law for moist air:

```
ρ = p_b / (R_da · T_abs · (1 + 1.6078 · w))
```

where R_da = 287.042 J/(kg·K) and T_abs = T_db + 273.15.

**Wet-bulb temperature** (`twb_fn_tdb_w_pb`):

Uses bisection search to find T_wb such that `w_fn_tdb_twb_pb(T_db, T_wb, p_b) = w`.

### Constants

| Constant | Value | Description |
|----------|-------|-------------|
| `CP_AIR` | 1004.84 J/(kg·K) | Dry air specific heat |
| `CP_VAPOR` | 1858.95 J/(kg·K) | Water vapor specific heat |
| `HFG_WATER` | 2,500,940 J/kg | Heat of vaporization at 0°C |
| `CP_WATER` | 4180 J/(kg·K) | Liquid water specific heat |
| `RHO_WATER` | 998.2 kg/m³ | Water density at ~20°C |
| `STD_PRESSURE` | 101,325 Pa | Standard atmospheric pressure |
| `MOL_MASS_RATIO` | 0.621945 | Molar mass ratio (water/air) |
| `AUTOSIZE` | -99999.0 | Sentinel value for autosizing |

## Weather Data

**Crate:** `openbse-weather` — `lib.rs`

### EPW File Parsing

Reads standard EnergyPlus Weather (EPW) files containing 8,760 hourly records (one year, non-leap). The parser extracts:

**Location data:**

| Field | Unit | Description |
|-------|------|-------------|
| `city` | — | City name |
| `state_province` | — | State/province |
| `country` | — | Country code |
| `latitude` | degrees | Site latitude (positive north) |
| `longitude` | degrees | Site longitude (positive east) |
| `time_zone` | hours | UTC offset |
| `elevation` | m | Site elevation above sea level |

**Hourly data:**

| Field | Unit | Description |
|-------|------|-------------|
| `dry_bulb` | °C | Dry-bulb temperature |
| `dew_point` | °C | Dew-point temperature |
| `rel_humidity` | % | Relative humidity (0–100) |
| `pressure` | Pa | Atmospheric pressure |
| `global_horiz_rad` | Wh/m² | Global horizontal radiation |
| `direct_normal_rad` | Wh/m² | Direct normal radiation |
| `diffuse_horiz_rad` | Wh/m² | Diffuse horizontal radiation |
| `wind_speed` | m/s | Wind speed |
| `wind_direction` | degrees | Wind direction (from north, clockwise) |
| `horiz_ir_rad` | Wh/m² | Horizontal infrared radiation |
| `opaque_sky_cover` | tenths | Opaque sky cover (0–10) |

### Design Days

Design days provide extreme-condition sizing data. Each includes a design temperature, humidity condition, barometric pressure, and wind speed for a specific month and day.

Humidity can be specified as wet-bulb, dew-point, humidity ratio, or enthalpy. Day types are `SummerDesign` or `WinterDesign`.

## Solar Position and Incident Radiation

**Module:** `solar.rs`

**Purpose:** Computes solar position (altitude, azimuth), incident solar radiation on tilted surfaces using the Hay-Davies anisotropic sky model, and angular-dependent window transmittance using Fresnel optics.

### Solar Position

**Function:** `solar_position(day_of_year, solar_hour, latitude_deg)`

**Reference:** Spencer (1971)

**Algorithm:**

Day angle:
```
Γ = 2π · (day_of_year − 1) / 365
```

Declination (Spencer):
```
δ = 0.006918
  − 0.399912·cos(Γ) + 0.070257·sin(Γ)
  − 0.006758·cos(2Γ) + 0.000907·sin(2Γ)
  − 0.002697·cos(3Γ) + 0.00148·sin(3Γ)
```

Hour angle:
```
ω = (solar_hour − 12) · 15°
```

Solar altitude:
```
sin(α) = sin(φ)·sin(δ) + cos(φ)·cos(δ)·cos(ω)
```

Solar azimuth:
```
sin(γ) = −cos(δ)·sin(ω) / cos(α)
```

where φ is site latitude.

### Equation of Time

**Function:** `equation_of_time(day_of_year)` — returns correction in hours.

Converts clock time to solar time:
```
solar_hour = clock_hour + (longitude/15 − time_zone) + equation_of_time
```

### Incident Solar on Tilted Surface

**Function:** `incident_solar_components(beam_normal, diffuse_horiz, global_horiz, solar_pos, azimuth, tilt, ground_reflectance, day_of_year, elevation_m)`

**Model:** Hay-Davies (1980) anisotropic sky model

**Reference:** Hay & Davies (1980) "Calculation of the Solar Irradiance Incident on an Inclined Surface"

**Inputs:**
| Parameter | Unit | Description |
|-----------|------|-------------|
| `beam_normal` | W/m² | Direct normal irradiance |
| `diffuse_horiz` | W/m² | Diffuse horizontal irradiance |
| `global_horiz` | W/m² | Global horizontal irradiance |
| `solar_pos` | -- | Solar position struct |
| `azimuth` | degrees | Surface azimuth (from north, clockwise) |
| `tilt` | degrees | Surface tilt (from horizontal) |
| `ground_reflectance` | 0-1 | Ground albedo (default 0.2) |
| `day_of_year` | -- | Day of year (1-365) |
| `elevation_m` | m | Site elevation (reserved for future Perez model) |

**Algorithm:**

Angle of incidence:
```
cos(θ) = cos(α_solar)·cos(γ_solar − γ_surface)·sin(β_surface) + sin(α_solar)·cos(β_surface)
```

Hay-Davies anisotropy index:
```
a_i = I_beam_normal / I_extraterrestrial
```

where `I_ext = 1353 · (1 + 0.033·cos(2π·DOY/365))` W/m².

Sky view factor:
```
VF_sky = (1 + cos(tilt)) / 2
```

The diffuse is decomposed into two components:

**Circumsolar** (directional, co-located with sun direction):
```
I_circumsolar = I_diffuse_horiz · a_i · cos(θ) / cos(θ_z)
```

**Isotropic** (uniform sky dome):
```
I_isotropic = I_diffuse_horiz · (1 - a_i) · VF_sky
```

**Ground-reflected:**
```
I_ground = I_global_horiz · ρ_ground · (1 − cos(tilt)) / 2
```

**Total:**
```
I_total = I_beam + I_circumsolar + I_isotropic + I_ground
```

The circumsolar component is critical for shading: it receives the same directional shading (sunlit fraction) as the beam component, since it represents forward-scattered light from the sun's direction.

### Window Transmitted Solar (Angular SHGC)

**Function:** `window_transmitted_solar_angular(shgc, area, beam_incident, diffuse_incident, cos_aoi)`

Uses separate angular modifiers for beam and diffuse solar, based on Fresnel optics for double-pane glass.

**Beam modifier** (angular dependence):
```
beam_mod = fresnel_double_pane_modifier(cos_aoi, shgc)
```

Uses the Fresnel equations for unpolarized light at a glass-air interface (n = 1.526) to compute the transmittance ratio `T(θ)/T(0)` for a double-pane assembly, accounting for inter-pane reflections.

**Diffuse modifier** (hemispherical integration):
```
diff_mod = diffuse_shgc_modifier(shgc)
```

Computed as the hemispherical average of the beam modifier over 0-90 degrees, weighted by `cos(θ)·sin(θ)`. For SHGC = 0.789 (ASHRAE 140 clear double-pane), diff_mod = 0.864.

**Transmitted solar:**
```
Q_beam = SHGC · beam_mod · Area · I_beam
Q_diffuse = SHGC · diff_mod · Area · I_diffuse
Q_total = Q_beam + Q_diffuse
```
