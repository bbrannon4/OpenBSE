# Comfort {#sec-comfort}

Comfort outputs are computed after each timestep's heat balance. They read
the solved state and never change it. Each zone has comfort inputs
(`ZoneComfortConfig`) with these defaults:

- metabolic rate $M$: 1.2 met;
- clothing insulation $I$: 0.5 clo;
- air speed $v$: 0.1 m/s;
- short-wave absorptivity of the body $\alpha_{sw}$: 0.67.

## PMV and PPD {#sec-pmv}

**Source:** `openbse-psychrometrics/src/comfort.rs` (`pmv`, `ppd`,
`clothing_surface_temp`, `convective_coeff`).

The predicted mean vote follows Fanger's model as standardized in
ISO 7730 [@iso7730] and ASHRAE 55 [@ashrae55], with no external work. The
inputs are:

- $\mathcal M = 58.15\,M$ [W/m²], the metabolic rate per body area;
- $I_{cl} = 0.155\,I$ [m²·K/W], the clothing insulation;
- $p_a = \phi\,p_\text{sat}(T_a)$ [Pa], the vapour pressure (@eq-psat);
- $T_a$, the zone air temperature, and $T_r$, the (effective) mean radiant
  temperature, both in °C.

Then

$$
\begin{aligned}
L ={}& \mathcal M - 3.05\times10^{-3}(5733 - 6.99\mathcal M - p_a) - 0.42\max(\mathcal M - 58.15, 0)\\
&- 1.7\times10^{-5}\mathcal M(5867 - p_a) - 0.0014\mathcal M(34 - T_a)\\
&- 3.96\times10^{-8}f_{cl}\left[(T_{cl} + 273.15)^4 - (T_r + 273.15)^4\right] - f_{cl}h_c(T_{cl} - T_a),\\
PMV ={}& \left(0.303\,e^{-0.036\mathcal M} + 0.028\right)L,
\end{aligned}
$$ {#eq-pmv}

with the clothing area factor, the convective coefficient and the clothing
surface temperature

$$
f_{cl} = \begin{cases}1.00 + 1.290I_{cl} & I_{cl} \le 0.078,\\ 1.05 + 0.645I_{cl} & \text{otherwise,}\end{cases}
\qquad
h_c = \max\left(2.38|T_{cl} - T_a|^{1/4},\ 12.1\sqrt v\right),
$$ {#eq-fcl-hc}

$$
T_{cl} = 35.7 - 0.028\mathcal M - I_{cl}\left\{3.96\times10^{-8}f_{cl}\left[(T_{cl} + 273.15)^4 - (T_r + 273.15)^4\right] + f_{cl}h_c(T_{cl} - T_a)\right\}.
$$ {#eq-tcl}

@eq-tcl is solved by damped fixed-point iteration, to $1.5\times10^{-4}$ K.
The predicted percentage dissatisfied is

$$
PPD = 100 - 95\exp\left(-0.03353\,PMV^4 - 0.2179\,PMV^2\right),
$$ {#eq-ppd}

with $PMV$ clamped to $[-3, 3]$. The operative temperature is
$T_o = (T_a + T_r)/2$, the ASHRAE 55 form for air speeds up to 0.2 m/s.

**Comparison with EnergyPlus.** @eq-pmv–@eq-ppd match EnergyPlus's Fanger
model (`ThermalComfort::CalcThermalComfortFanger`) and ISO 7730 [@eplus-engref-25-2,
"Occupant Thermal Comfort"]. There is one deliberate difference: the
sweat-evaporation term $0.42(\mathcal M - 58.15)$ is floored at zero, which
matters only below 1 met. EnergyPlus also offers the Pierce two-node, KSU
and adaptive models, and schedules for activity, clothing and air speed;
OpenBSE has none of these
([#110](https://github.com/bbrannon4/OpenBSE/issues/110)).

## Mean radiant temperature {#sec-mrt}

**Zone value** (`main.rs`, "Comfort metrics"). The zone's long-wave MRT is
the area-weighted mean of its inside surface temperatures, windows at their
room-facing (glass or shade) temperature:

$$
T_{r,z} = \frac{\sum_i A_iT_{si}}{\sum_i A_i}.
$$ {#eq-zone-mrt}

EnergyPlus's default `ZoneAveraged` MRT weights by area × emissivity.

**Solar increment.** When beam solar reaches an occupant, the MRT is raised
by the direct-beam term of ASHRAE 55 Annex C (SolarCal) [@ashrae55]:

$$
\Delta T_{r,\text{sol}} = \frac{\alpha_{sw}\,f_p(\beta)\,I_b}{\varepsilon_b\,4\sigma\,(T_r + 273.15)^3},
\qquad
f_p = 0.308\cos\left(0.998\beta - 0.0875\beta^2\right),
$$ {#eq-solar-mrt}

where:

- $I_b$ is the beam irradiance on the occupant;
- $\beta$ is the solar altitude [rad];
- $\varepsilon_b = 0.95$ is the body's emissivity;
- $f_p$ is the azimuth-averaged projected-area factor of a standing person
  (a fit the code attributes to Walkenhorst et al., 2004; ASHRAE 55
  Annex C tabulates $f_p$ [@ashrae55]).

The diffuse, reflected and beam-exposure terms of the full SolarCal model
are not included ([#103](https://github.com/bbrannon4/OpenBSE/issues/103)).

For the **zone** outputs, $I_b$ is the outdoor direct normal irradiance,
applied whenever any beam enters the zone. That ignores the glazing
transmittance and assumes every occupant is in the sun
([#103](https://github.com/bbrannon4/OpenBSE/issues/103)). The comfort grid
(@sec-comfort-grid) uses the beam actually transmitted to each point.

## Spatial comfort grid {#sec-comfort-grid}

**Source:** `openbse-envelope/src/comfort_grid.rs` (`ZoneGrid::build`,
`view_factor_row`, `occupant_view_factor`, `mrt`, `beam_irradiance`,
`point_comfort`); `openbse-io/src/comfort_grid.rs`.

The grid gives the radiant field within a zone. The zone air is well mixed,
so air temperature and humidity are the same at every point.

**Points.** Points are laid on the zone's floor polygons at a set spacing,
kept a set distance from the floor boundary, at the occupant's radiant
centre height. The default height is 0.6 m seated and 1.1 m standing
[@iso7726].

**View factors.** Each point's view factor to each surface (walls net of
their windows) depends on the occupant model:

- **Sphere:** solid angle over $4\pi$, from Van Oosterom and Strackee's
  triangle formula [@vanoosterom1983].
- **Seated or standing:** the ISO 7726 weighted sum of plane view factors
  for up, down and eight horizontal directions [@iso7726]. Each plane view
  factor is Lambert's contour integral over the part of the polygon in front
  of the plane. The horizontal directions are averaged because occupants
  face every way.

Each point's row is normalized to sum to 1. The unnormalized sum is reported
as its coverage.

**MRT and comfort.** Each timestep, a point's MRT is the radiant average of
the surface temperatures:

$$
T_{r,p} = \left[\sum_j F_{pj}\,(T_{sj} + 273.15)^4\right]^{1/4} - 273.15.
$$ {#eq-point-mrt}

A point in a sun patch adds @eq-solar-mrt. Its beam irradiance is that of
the window the ray toward the sun passes through: the window's transmitted
beam divided by its projected area,
$I_b = Q_{b,\text{win}}/(A_\text{win}\cos\theta)$. PMV and PPD follow
@sec-pmv with the zone air state.

**Comparison with EnergyPlus.** EnergyPlus has no spatial comfort grid. Its
nearest feature is the `AngleFactor` MRT for one user-defined position
(`ComfortViewFactorAngles`), with user-entered angle factors. OpenBSE
computes the factors from geometry.

## Known limitations {#sec-comfort-limitations}

- [#103](https://github.com/bbrannon4/OpenBSE/issues/103): only the
  direct-beam SolarCal term. Zone outputs use outdoor DNI without the glazing
  transmittance and apply it zone-wide. Zone MRT weighting.
- [#110](https://github.com/bbrannon4/OpenBSE/issues/110): PMV clean-ups
  (clothing coefficient citation, neutral test).
- [#136](https://github.com/bbrannon4/OpenBSE/issues/136): comfort grid
  follow-ups and the Workbench comfort tab.
- [#143](https://github.com/bbrannon4/OpenBSE/issues/143): radiant internal
  gains as located sources, for better spatial comfort.
