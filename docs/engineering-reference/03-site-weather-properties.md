# Site, weather and properties {#sec-site}

This chapter covers the boundary conditions every model shares: weather data
and design days, the sun's position and the irradiance on each surface, the
sky and ground temperatures, and the properties of moist air and water.

## Weather data {#sec-weather}

**Source:** `openbse-weather/src/lib.rs` (`read_epw`, `read_tmy3`,
`WeatherHour::interpolate`, `WeatherHour::to_air_state`).

### Files {#sec-weather-files}

OpenBSE reads EnergyPlus weather (EPW) files and TMY3 CSV files. The EPW
`LOCATION` line gives the latitude, longitude, time zone and elevation. The
`GROUND TEMPERATURES` line gives monthly ground temperatures at each depth
listed (@sec-ground-temps), and the `DATA PERIODS` line gives the day of the
week of 1 January (Monday if absent). The other header lines are not used.

Each record is one hour, ending at the hour stamped on it (@sec-time). The
reader keeps every record and fails on a malformed one rather than skipping
it, since a skipped record would shift every later hour. It removes
29 February. It then requires consecutive hourly records of a non-leap year
starting on 1 January, so record $N$ is hour $N$ of the year. TMY3 files are
read the same way, with pressure converted from mbar to Pa. Their horizontal
infrared is set to zero, which is unused in any case (@sec-sky).

EPW missing-value sentinels are replaced by fixed values (@tbl-epw-missing).

| Field | Missing when | Replaced by |
|---|---|---|
| Pressure | $\ge 999\,999$ Pa or $\le 0$ | 101 325 Pa |
| Dry-bulb | $\ge 99.9$ °C | the dew point (or 0 °C if also missing) |
| Dew point | $\ge 99.9$ °C | the dry-bulb (saturated) |
| Relative humidity | $\ge 999$ % or $\le 0$ | from dry-bulb and dew point |
| Solar and IR irradiance | $\ge 9999$ W/m² | 0 |
| Wind speed / direction | $\ge 999$ | 0 m/s / 0° |
| Opaque sky cover | $\ge 99$ | 5 tenths |

: EPW missing-value substitution (`parse_data_line`). {#tbl-epw-missing}

**Comparison with EnergyPlus.** EnergyPlus replaces a missing value with the
last valid one, starting from defaults such as 6 °C dry-bulb, 50% relative
humidity and 5 tenths of sky cover (`WeatherManager`, `wvarsMissing`).
OpenBSE's fixed substitutes are a deliberate simplification. They matter only
for files with gaps.

### Within the hour {#sec-weather-interp}

The interpolation of @eq-weather-interp applies, with weight $k/N$, to:

- dry-bulb and dew-point temperature;
- relative humidity;
- pressure;
- wind speed;
- horizontal infrared radiation;
- opaque sky cover.

Beam and diffuse irradiance and wind direction take the current hour's value
for the whole hour.

The outdoor air state is built from the interpolated dry-bulb, relative
humidity and pressure:

$$
W_\text{o} = W(T_\text{o}, \phi_\text{o}, p_b)
$$ {#eq-outdoor-w}

with @eq-w-rh (`to_air_state`).

**Comparison with EnergyPlus.** EnergyPlus interpolates the same quantities
with the same weight (`WeatherManager::SetupInterpolationValues`). It also
builds the outdoor humidity ratio from dry-bulb and relative humidity
(`PsyWFnTdbRhPb`). It differs in two respects:

- **Irradiance.** For $N > 1$ it treats each hourly irradiance record as
  centred on the half hour and interpolates between the adjacent hours.
- **Wind direction.** It interpolates wind direction across the 0/360° wrap.

Both differences are filed in
[#195](https://github.com/bbrannon4/OpenBSE/issues/195).

### Calendar {#sec-calendar}

The day of the week is counted from the weather file's 1 January
(`schedule::day_of_week`). Holidays come from the model's `simulation`
settings and are day type 8. There is no daylight saving time, and no input
to override the starting day of the week; EnergyPlus's DOE prototype models
use both ([#199](https://github.com/bbrannon4/OpenBSE/issues/199)).

## Design days {#sec-design-days}

**Source:** `openbse-io/src/sizing.rs`, `generate_heating_design_weather`,
`generate_cooling_design_weather`, `run_single_design_day`.

A design day is a synthetic 24-hour weather sequence, repeated until the
envelope reaches a periodic state (@sec-sizing). Its inputs are:

- the design dry-bulb $T_\text{dd}$;
- the daily range $\Delta T_\text{dr}$;
- a humidity value;
- the pressure and wind speed;
- the month and day.

**Heating design days** hold $T_\text{dd}$ all day, with no solar
irradiance, 10/10 opaque sky cover and the given wind speed.

**Cooling design days** follow a cosine profile, peaking at 15:00 and
reaching its minimum at 03:00:

$$
T_\text{o}(h) = T_\text{dd} - \Delta T_\text{dr}\cdot\tfrac12\left[1 - \cos\frac{\pi(h - 15)}{12}\right],
$$ {#eq-dd-temp}

and a clear-sky irradiance:

$$
I_\text{DN} = 1080\,e^{-0.174/\sin\alpha}\ \text{W/m}^2, \qquad
I_\text{dH} = 120 \sin\alpha\ \text{W/m}^2,
$$ {#eq-dd-solar}

with $1/\sin\alpha$ capped at 20. Here $\alpha$ is the solar altitude, from
Cooper's declination $\delta = 23.45° \sin[2\pi(284+n)/365]$ and an hour
angle taken at the half hour of clock time, with no longitude or
equation-of-time correction. The sky cover is 1/10.

**Humidity.** During sizing the outdoor air is built from a fixed relative
humidity: 50% on heating days and 30% on cooling days. `humidity_type` is
not read, and `humidity_value` is used only as the dew point for the sky
temperature ([#198](https://github.com/bbrannon4/OpenBSE/issues/198)).

**Comparison with EnergyPlus.** EnergyPlus `SizingPeriod:DesignDay`
(`WeatherManager::SetUpDesignDay`) builds the profile and the sky as follows:

- **Temperature.** ASHRAE's tabulated daily-range multipliers, whose minimum
  falls at 05:00–06:00 [@ashrae2021fund, Ch. 14].
- **Humidity.** From the humidity condition type (wet-bulb, dew point,
  humidity ratio or enthalpy).
- **Sky.** The ASHRAE clear sky, or the ASHRAE tau model when `taub` and
  `taud` are given.

OpenBSE accepts `taub` and `taud` but does not use them
([#50](https://github.com/bbrannon4/OpenBSE/issues/50)). The profile, the
humidity and the irradiance all deviate from EnergyPlus
([#198](https://github.com/bbrannon4/OpenBSE/issues/198)).

## Solar position {#sec-solar-position}

**Source:** `openbse-envelope/src/solar.rs::solar_position`,
`equation_of_time`, `local_solar_hour`; called once per envelope solve at the
timestep midpoint (@eq-fractional-hour).

The declination $\delta$ [rad] follows Spencer's Fourier series
[@spencer1971], with day angle $\Gamma = 2\pi(n-1)/365$ for day of year $n$:

$$
\begin{aligned}
\delta ={}& 0.006918 - 0.399912\cos\Gamma + 0.070257\sin\Gamma
- 0.006758\cos 2\Gamma \\
&+ 0.000907\sin 2\Gamma - 0.002697\cos 3\Gamma + 0.00148\sin 3\Gamma.
\end{aligned}
$$ {#eq-declination}

The equation of time [h] uses $B = 2\pi(n - 81)/364$:

$$
E = 0.1645\sin 2B - 0.1255\cos B - 0.025\sin B,
$$ {#eq-eot}

which is the common minutes form ($9.87\sin 2B - 7.53\cos B - 1.5\sin B$)
divided by 60 [@duffie2013]. Local solar time and hour angle follow from the
clock hour $t_\text{h}$, the longitude $\lambda$ (east positive) and the time
zone $Z$ [h from UTC]:

$$
t_\text{sol} = t_\text{h} + \left(\frac{\lambda}{15} - Z\right) + E, \qquad
\omega = 15°\,(t_\text{sol} - 12).
$$ {#eq-solar-time}

The altitude $\alpha$ and azimuth $\psi$ (from south, west positive) at
latitude $\phi$ are

$$
\sin\alpha = \sin\phi\sin\delta + \cos\phi\cos\delta\cos\omega,
$$ {#eq-altitude}

$$
\sin\psi = \frac{\cos\delta\sin\omega}{\cos\alpha}, \qquad
\cos\psi = \frac{\sin\alpha\sin\phi - \sin\delta}{\cos\alpha\cos\phi},
$$ {#eq-azimuth}

with $\psi = \operatorname{atan2}(\sin\psi, \cos\psi)$, and $\cos\alpha$ and
$\cos\phi$ floored at 0.001. The sun is taken as up when
$\alpha > -0.8333°$. In that band below the horizon the cosine of the zenith
angle is floored at zero.

**Comparison with EnergyPlus.** EnergyPlus computes the declination and the
equation of time from its own Fourier fits (`SolarShading::SUN4`, from
BLAST's `SUN3`) [@eplus-engref-25-2, "Solar Position"]. Both fits are
accurate to well under 0.1° and about 30 s, so the difference is negligible.
Two timing differences matter more (filed in
[#195](https://github.com/bbrannon4/OpenBSE/issues/195)):

- **Time within the step.** For $N > 1$, EnergyPlus evaluates the sun at the
  *end* of each timestep (`FigureSunCosines`), and at the hour's midpoint
  only when $N = 1$. OpenBSE always uses the midpoint.
- **Sun-up test.** EnergyPlus treats the sun as up when
  $\cos\theta_z \ge 10^{-5}$ (`DataEnvironment::SunIsUpValue`), not at
  −0.8333° as the OpenBSE comment states.

## Irradiance on surfaces {#sec-incident-solar}

**Source:** `solar.rs::incident_solar_components`,
`perez_brightness_coefficients`; called from
`heat_balance.rs::solve_timestep`, step 5.

For a surface of tilt $\beta$ and azimuth $\gamma$ (@eq-azimuth-tilt),
converted to the south-based convention, the angle of incidence $\theta$ is

$$
\cos\theta = \max\left[0,\ \sin\alpha\cos\beta + \cos\alpha\sin\beta\cos(\psi - \gamma_s)\right],
\qquad \gamma_s = \gamma - 180°.
$$ {#eq-aoi}

The irradiance on the surface is the sum of five components, each shaded
differently in @sec-envelope:

$$
I = \underbrace{I_\text{DN}\cos\theta}_\text{beam}
+ \underbrace{I_\text{dH}(1-F_1)\tfrac{1+\cos\beta}{2}}_\text{isotropic}
+ \underbrace{I_\text{dH}F_1\tfrac{\cos\theta}{\max(\cos\theta_z,\ 0.0872)}}_\text{circumsolar}
+ \underbrace{I_\text{dH}F_2\sin\beta}_\text{horizon}
+ \underbrace{I_\text{GH}\rho_g\tfrac{1-\cos\beta}{2}}_\text{ground},
$$ {#eq-incident}

where $I_\text{DN}$, $I_\text{dH}$ and $I_\text{GH}$ are the direct normal,
diffuse horizontal and global horizontal irradiance. The ground reflectance
$\rho_g$ is fixed at 0.2. Each component is floored at zero.

The brightness coefficients come from the Perez model [@perez1990]. With the
zenith angle $Z$ [rad] (its cosine floored at 0.087, i.e. $Z \le 85°$):

$$
\varepsilon = \frac{(I_\text{dH} + I_\text{DN})/I_\text{dH} + 1.041Z^3}{1 + 1.041Z^3},
\qquad
\Delta = \frac{I_\text{dH}\, m}{1353},
$$ {#eq-perez-eps}

with relative air mass

$$
m =
\begin{cases}
m_h/\cos Z & Z \le 75°,\\
m_h/\left[\cos Z + 0.15(93.9 - Z_\text{deg})^{-1.253}\right] & Z > 75°,
\end{cases}
\qquad m_h = 1 - 0.1\,\frac{z_\text{site}}{1000},
$$ {#eq-airmass}

where $z_\text{site}$ is the site elevation [m]. $m$ is capped at 40 and
$m_h$ floored at 0.5. $\varepsilon$ selects one of eight bins, whose
coefficients give

$$
F_1 = \max\left(0,\ F_{11} + F_{12}\Delta + F_{13}Z\right), \qquad
F_2 = F_{21} + F_{22}\Delta + F_{23}Z.
$$ {#eq-perez-f}

The coefficient table is EnergyPlus's (`PEREZ_F11` … `PEREZ_F23`). When
$I_\text{dH} < 1$ W/m², $F_1 = F_2 = 0$.

**Comparison with EnergyPlus.** The bins, coefficients and formulas for
$\varepsilon$, $\Delta$ and $m$ match `SolarShading::AnisoSkyViewFactors`
[@eplus-engref-25-2, "Sky Radiance Model"]. The deviations are filed in
[#196](https://github.com/bbrannon4/OpenBSE/issues/196):

- **Zenith cap.** The 85° zenith cap applies to $\varepsilon$, $\Delta$,
  $F_1$ and $F_2$. EnergyPlus caps the zenith only in the circumsolar
  denominator.
- **Horizon floor.** The horizon term is floored at zero on its own.
  EnergyPlus floors only the sum of the three sky components, so a negative
  $F_2$ (overcast skies) lowers the total there.
- **Air-mass limits.** The air-mass cap and floor have no EnergyPlus
  counterpart.
- **Ground reflectance.** It cannot be set (EnergyPlus:
  `Site:GroundReflectance`, monthly).

## Sky and ground temperatures {#sec-sky}

**Source:** `heat_balance.rs::solve_timestep`, step 1b.

The sky temperature for long-wave exchange (@sec-envelope) comes from the
Clark–Allen sky emissivity [@clark1978] with an opaque-cloud correction
[@walton1983], using the dew point $T_\text{dp}$ and opaque sky cover $N$
[tenths]:

$$
\varepsilon_\text{sky} = \left(0.787 + 0.764\ln\frac{T_\text{dp} + 273.15}{273}\right)
\left(1 + 0.0224N - 0.0035N^2 + 0.00028N^3\right),
$$ {#eq-sky-emissivity}

$$
T_\text{sky} = \varepsilon_\text{sky}^{1/4}\,(T_\text{o} + 273.15) - 273.15,
$$ {#eq-sky-temp}

with $\varepsilon_\text{sky}$ clamped to $[0.3, 1]$. The weather file's
horizontal infrared irradiance is not used.

**Comparison with EnergyPlus.** EnergyPlus uses the file's horizontal
infrared irradiance $I_\text{IR}$ whenever it is valid,
$T_\text{sky} = (I_\text{IR}/\sigma)^{1/4}$, and falls back to an emissivity
model only when it is missing (`Weather::calcSky`). Its Clark–Allen form uses
$\min(T_\text{o}, T_\text{dp})$ and divides by 273.15. OpenBSE's sky
temperature is therefore a deviation, not the E+ default that its comment
claims ([#197](https://github.com/bbrannon4/OpenBSE/issues/197)).

### Ground temperatures {#sec-ground-temps}

**Source:** `openbse-envelope/src/ground_temp.rs::GroundTempModel`; set up
in `main.rs` (step 3).

Surfaces with a ground boundary use a monthly table,
`simulation.ground_surface_temperatures`, which defaults to 18 °C every
month, as EnergyPlus does when `Site:GroundTemperature:BuildingSurface` is
absent. Each month's value is placed at mid-month, and the temperature on
day $n$ is interpolated linearly between those points, wrapping from
December to January. EnergyPlus instead holds each month's value constant
for the whole month
([#201](https://github.com/bbrannon4/OpenBSE/issues/201)). F-factor floors
use their own monthly table, held for the month (@sec-envelope). Ground-source
heat pumps use the EPW's shallowest ground temperatures or the model below
(@sec-zone-equipment).

When no table is given, the Kusuda–Achenbach model applies
[@kusuda1965]. At depth $z$ [m] on day $n$:

$$
T_g(z, n) = \bar T - A\, e^{-z\sqrt{\pi/(365\,a)}}
\cos\left[\frac{2\pi}{365}\left(n - n_0 - \frac{z}{2}\sqrt{\frac{365}{\pi a}}\right)\right],
$$ {#eq-kusuda}

where:

- $\bar T$ and $A$ are the annual mean and half the range of the weather
  file's monthly mean air temperatures;
- $n_0$ is the middle of the coldest month;
- $a = 0.04$ m²/day is the soil diffusivity;
- $z = 0.5$ m.

## Moist air properties {#sec-psychrometrics}

**Source:** `openbse-psychrometrics/src/lib.rs`.

Moist air is treated as an ideal mixture of dry air and water vapour. The
functions follow EnergyPlus's `Psychrometrics` module (described under
"Psychrometric services" in its Module Developer guide), with the constants
of @tbl-psych-constants.

| Symbol | Value | Meaning |
|---|---|---|
| $c_{p,a}$ | 1004.84 J/(kg·K) | Dry air specific heat |
| $c_{p,v}$ | 1858.95 J/(kg·K) | Water vapour specific heat |
| $h_{fg,0}$ | $2.50094\times10^6$ J/kg | Latent heat of vaporization at 0 °C |
| $\epsilon$ | 0.621945 | Molar mass ratio, water to dry air |
| $W_\text{min}$ | $10^{-5}$ | Floor on the humidity ratio |

: Psychrometric constants. {#tbl-psych-constants}

**Saturation pressure** [Pa], from Hyland and Wexler [@hyland1983], at
absolute temperature $T$ [K], over ice below the triple point (273.16 K)
and over liquid water above it:

$$
\ln p_\text{sat} = \frac{C_1}{T} + C_2 + C_3T + C_4T^2 + C_5T^3 + C_6T^4 + C_7\ln T,
$$ {#eq-psat}

with the ASHRAE coefficients $C_1 \dots C_7$ over ice and $C_8 \dots C_{13}$
over water [@ashrae2021fund, Ch. 1]. The value is clamped outside
−100 °C to 200 °C.

**Humidity ratio**, from relative humidity $\phi$, from the dew point, and
from the wet bulb (with $W^*_s = W(T_\text{wb}, 1, p_b)$):

$$
W(T, \phi, p_b) = \frac{\epsilon\,\phi\, p_\text{sat}(T)}{p_b - \phi\, p_\text{sat}(T)}, \qquad
W(T_\text{dp}, p_b) = \frac{\epsilon\, p_\text{sat}(T_\text{dp})}{p_b - p_\text{sat}(T_\text{dp})},
$$ {#eq-w-rh}

$$
W =
\begin{cases}
\dfrac{(2501 - 2.326T_\text{wb})W^*_s - 1.006(T - T_\text{wb})}{2501 + 1.86T - 4.186T_\text{wb}} & T_\text{wb} \ge 0,\\[1ex]
\dfrac{(2830 - 0.24T_\text{wb})W^*_s - 1.006(T - T_\text{wb})}{2830 + 1.86T - 2.1T_\text{wb}} & T_\text{wb} < 0,
\end{cases}
$$ {#eq-w-twb}

[@ashrae2021fund, Ch. 1, Eq. 33 and 35]. Every humidity ratio is floored at
$W_\text{min}$.

**Enthalpy, temperature and specific heat:**

$$
h = c_{p,a}T + W\left(h_{fg,0} + c_{p,v}T\right), \qquad
T = \frac{h - h_{fg,0}W}{c_{p,a} + c_{p,v}W}, \qquad
c_p = c_{p,a} + c_{p,v}W.
$$ {#eq-enthalpy}

**Density and specific volume** (dry-air basis):

$$
\rho = \frac{p_b}{287.055\,(T + 273.15)(1 + 1.6078W)}, \qquad
v = 159.473\,\frac{(1 + 1.6078W)(1.8T + 492)}{p_b}.
$$ {#eq-density}

**Inverse functions:**

- **Dew point** inverts $p_\text{sat}$ at the vapour pressure
  $p_v = p_bW/(\epsilon + W)$ by Newton iteration (`tsat_fn_press`), to
  $|\Delta p| < 0.1$ Pa.
- **Wet bulb** solves @eq-w-twb for $T_\text{wb}$ by Newton iteration,
  bounded by the dew point and the dry-bulb.
- **Saturation temperature at enthalpy $h$** bisects $h_\text{sat}(T) = h$
  on $[-50, 100]$ °C.
- **Relative humidity** is $\phi = p_v/p_\text{sat}(T)$, clamped to
  $[0, 1]$.

**Comparison with EnergyPlus.** @eq-psat, @eq-w-rh, @eq-enthalpy, the specific
volume and the inverse functions match EnergyPlus (`PsyPsatFnTemp`,
`PsyWFnTdbRhPb`, `PsyWFnTdpPb`, `PsyHFnTdbW`, `PsyTdbFnHW`, `PsyCpAirFnW`,
`PsyVFnTdbWPb`, `PsyTsatFnPb`, `PsyTwbFnTdbWPb`, `PsyRhFnTdbWPb`), up to
iteration tolerances. Two functions deviate from EnergyPlus, though the code
labels both as matches
([#200](https://github.com/bbrannon4/OpenBSE/issues/200)):

- **Density.** It uses $R = 287.055$ and 1.6078, where `PsyRhoAirFnPbTdbW`
  uses 287.0 and 1.6077687 (0.02% lower density).
- **Humidity ratio from wet bulb.** @eq-w-twb is the ASHRAE form, which
  EnergyPlus uses only inside `PsyTwbFnTdbWPb`. EnergyPlus's
  `PsyWFnTdbTwbPb` uses
  $((2501 - 2.381T_\text{wb})W^*_s - (T - T_\text{wb}))/(2501 + 1.805T - 4.186T_\text{wb})$,
  with no ice branch. OpenBSE's form is consistent with its own wet-bulb
  inverse.

## Water properties {#sec-water-properties}

**Source:** `openbse-psychrometrics/src/lib.rs::FluidState`.

Water has constant properties: $c_{p,w} = 4180$ J/(kg·K) in `FluidState`,
and $\rho_w = 998.2$ kg/m³. Some components and plant sizing use
4186 J/(kg·K) instead. There is no glycol. EnergyPlus evaluates water and
glycol properties at the fluid temperature (`FluidProperties`); between 0 and
80 °C, cp varies by about 1% and density by about 3%
([#200](https://github.com/bbrannon4/OpenBSE/issues/200)).

## Known limitations {#sec-site-limitations}

- [#50](https://github.com/bbrannon4/OpenBSE/issues/50): cooling design-day
  solar is a fixed clear-sky formula; `taub` and `taud` are ignored.
- [#195](https://github.com/bbrannon4/OpenBSE/issues/195): irradiance is held
  constant through the hour; wind direction is not interpolated; sun-position
  timing and the sun-up test differ from EnergyPlus.
- [#196](https://github.com/bbrannon4/OpenBSE/issues/196): Perez model
  details (zenith cap, horizon floor); no ground-reflectance input.
- [#197](https://github.com/bbrannon4/OpenBSE/issues/197): the sky
  temperature ignores the weather file's horizontal infrared.
- [#198](https://github.com/bbrannon4/OpenBSE/issues/198): design-day
  humidity is a fixed relative humidity; the temperature profile is not
  ASHRAE's.
- [#199](https://github.com/bbrannon4/OpenBSE/issues/199): no daylight saving
  time or start-day-of-week input.
- [#200](https://github.com/bbrannon4/OpenBSE/issues/200): constant and
  inconsistent water properties; air density and wet-bulb labels; $h_{fg}$ for
  latent gains.
- [#201](https://github.com/bbrannon4/OpenBSE/issues/201): ground-surface
  temperatures are interpolated rather than held per month; unused
  design-day types.
