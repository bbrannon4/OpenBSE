# Building envelope {#sec-envelope}

This chapter covers the building shell, in seven parts:

1. materials, constructions and conduction;
2. surface heat balances and convection;
3. long-wave exchange;
4. windows, shades and solar distribution;
5. infiltration, ventilation and the airflow network;
6. internal gains and ground heat transfer;
7. the zone air heat and moisture balances.

They are reviewed one part at a time. Parts 1 to 5 are reviewed below;
the rest is pre-review text (@sec-envelope-prereview).

## Materials and constructions {#sec-constructions}

**Source:** `openbse-envelope/src/material.rs` (`Material`,
`Construction`, `ResolvedLayer`, `SimpleConstruction`,
`FFactorConstruction`); surface properties are assigned in
`heat_balance.rs::BuildingEnvelope::from_input_full`.

An opaque **material** has a conductivity $k$ [W/(m·K)], a density $\rho$
[kg/m³], a specific heat $c_p$ [J/(kg·K)], solar, thermal (long-wave) and
visible absorptances, and a roughness. Its thickness is given on each
construction layer, not on the material. A material with
`thermal_resistance` $R$ [m²·K/W] is massless, like EnergyPlus's
`Material:NoMass`: only $R$ is used.

A layered **construction** lists layers from outside to inside. Its
conduction resistance is $\sum_\ell L_\ell/k_\ell$, plus $R$ for massless
layers, with no air films. Each surface takes its properties from its
construction's faces:

- **Outside face.** The outside layer's solar absorptance, thermal
  absorptance and roughness.
- **Inside face.** The inside layer's solar and thermal absorptance.

This matches EnergyPlus. Three other construction types exist: simple
constructions (@sec-simple-constructions), F-factor floors (@sec-ffactor)
and window constructions (@sec-windows).

A layer naming an undefined material is dropped silently, and missing
conductivity, density or specific heat take defaults (1, 1 and 1000). Both
are filed as [#203](https://github.com/bbrannon4/OpenBSE/issues/203),
together with two unread inputs, `thermal_absorptance_inside` and the simple
construction's `thickness`.

## Conduction transfer functions {#sec-ctf}

**Source:** `openbse-envelope/src/ctf.rs`: `try_calculate_ctf`,
`build_state_space`, `matrix_exponential`, `compute_ctf_from_state_space`,
`apply_ctf`, `CtfHistory`; coefficients built at initialization by
`heat_balance.rs::compute_all_ctf`.

Conduction through an opaque construction is one-dimensional, with constant
properties. It is represented by conduction transfer functions (CTFs), which
give the face heat fluxes at time $t$ from the current and past face
temperatures and past fluxes [@seem1987]:

$$
\begin{aligned}
q''_\text{ki}(t) ={}& Y_0 T_\text{so}^t + \sum_{j\ge1} Y_j T_\text{so}^{t-j}
- Z_0 T_\text{si}^t - \sum_{j\ge1} Z_j T_\text{si}^{t-j}
+ \sum_{j\ge1} \Phi_j\, q''^{\,t-j}_\text{ki},\\
q''_\text{ko}(t) ={}& X_0 T_\text{so}^t + \sum_{j\ge1} X_j T_\text{so}^{t-j}
- Y_0 T_\text{si}^t - \sum_{j\ge1} Y_j T_\text{si}^{t-j}
+ \sum_{j\ge1} \Phi_j\, q''^{\,t-j}_\text{ko},
\end{aligned}
$$ {#eq-ctf}

where:

- $T_\text{so}$ and $T_\text{si}$ are the outside and inside face
  temperatures;
- $q''_\text{ki}$ is the conduction flux leaving the wall at the inside face
  (positive into the zone);
- $q''_\text{ko}$ is the conduction flux entering the wall at the outside
  face (positive inward).

The past terms $t-j$ are the values stored at the end of earlier timesteps
(`CtfHistory`, shifted once per timestep; @sec-timestep-loop). The history
starts at 21 °C with zero flux.

### State-space formulation {#sec-ctf-state-space}

The construction is divided into nodes. The massed layers in the middle are
handled as follows:

- **Massed layers.** A massed layer of thickness $L$ and diffusivity
  $\alpha = k/(\rho c_p)$ gets
  $N = \min(\max(\lceil L/\Delta x_n\rceil, 6), 18)$ divisions of width
  $\Delta x = L/N$, with $\Delta x_n = \sqrt{2\alpha\,\Delta t}$.
- **Massless layers at either end** ($\rho c_p L < 1$ J/(m²·K), or NoMass) add
  their resistance in series with the conductance $k/\Delta x$ between the
  face and the first or last node.
- **Massless layers between massed ones** become one resistive division of
  conductance $1/R$ with no capacitance; adjacent ones are combined.

The interior nodes $\theta$ (one fewer than the total number of divisions)
carry these capacitances:

- $\rho c_p \Delta x$ for a node inside a layer;
- $\tfrac12(\rho c_p\Delta x)_\ell + \tfrac12(\rho c_p\Delta x)_{\ell+1}$
  for a node at a layer interface;
- $1.5\,\rho c_p \Delta x$ for the first and last node, which also hold the
  half-cell at each face.

The total capacitance is therefore conserved. With the face temperatures
$u = (T_\text{so}, T_\text{si})^\mathsf{T}$ as inputs and the face fluxes
$y = (q''_\text{ko}, q''_\text{ki})^\mathsf{T}$ as outputs:

$$
\dot\theta = A\theta + Bu, \qquad y = C\theta + Du,
$$ {#eq-state-space}

where $A$ is tridiagonal (nodal conductances over capacitances), and $B$, $C$
and $D$ couple the first and last node to the faces through the boundary
conductances $h_\text{b} = 1/(R_\text{nomass} + \Delta x/k)$.

Assuming the face temperatures vary linearly within a timestep, the exact
discrete solution uses

$$
\Phi = e^{A\Delta t},\qquad
\Gamma_1 = A^{-1}(\Phi - I)B,\qquad
\Gamma_2 = A^{-1}\!\left(\tfrac{1}{\Delta t}\Gamma_1 - B\right).
$$ {#eq-ctf-discrete}

$e^{A\Delta t}$ is a Taylor series with scaling and squaring. The CTF
coefficients follow from the recurrence of Seem [@seem1987], which builds
the characteristic polynomial of $\Phi$:

$$
\begin{aligned}
&R_0 = I, \qquad e_j = -\tfrac{1}{j}\operatorname{tr}(\Phi R_{j-1}), \qquad R_j = \Phi R_{j-1} + e_j I,\\
&S_0 = C\Gamma_2 + D, \qquad S_j = C\left[R_{j-1}(\Gamma_1 - \Gamma_2) + R_j\Gamma_2\right] + e_j D,
\end{aligned}
$$ {#eq-ctf-recurrence}

$$
X_j = S_j^{(0,0)},\qquad Y_j = S_j^{(1,0)},\qquad Z_j = -S_j^{(1,1)},\qquad \Phi_j = -e_j.
$$ {#eq-ctf-coefficients}

The two cross terms of $S_0$ are set to the mean of their magnitudes, so the
construction is reciprocal. The series stops when
$|e_j|/|e_1| < 10^{-13}$. If it has not converged in 18 terms, the run stops
with an error.

**Checks.** The steady-state transmittance implied by the coefficients,
$U_\text{CTF} = \sum Z_j / (1 - \sum\Phi_j)$, is compared with
$1/\sum R_\ell$. A difference above 1% is reported as a warning, and above 5%
as an error.

**Special cases.**

- With total capacitance below 1 J/(m²·K), the CTF is steady state:
  $X_0 = Y_0 = Z_0 = U$, with no history.
- When the construction reduces to a single node, it is a lumped RC element:
  $X_0 = Y_0 = Z_0 = U(1 - e^{-\Delta t/\tau})$ and
  $\Phi_1 = e^{-\Delta t/\tau}$, with $\tau = RC$.

**Comparison with EnergyPlus.** The method, node rules, capacitances and
recurrence follow `Construction::calculateTransferFunction`
[@eplus-engref-25-2, "Conduction Transfer Function (CTF) Calculations"]. The
differences are:

- **Node count.** It is rounded up, where EnergyPlus truncates, and capped at
  18 rather than 19.
- **No CTF time-step escalation.** EnergyPlus lengthens the CTF time step
  (to a multiple of the zone timestep) when a layer hits the node cap, or
  when the series needs too many terms, and keeps multi-step histories.
  OpenBSE always uses the zone timestep and stops instead
  ([#204](https://github.com/bbrannon4/OpenBSE/issues/204)).
- **Acceptance test.** EnergyPlus checks $\sum X = \sum Y = \sum Z$; OpenBSE
  checks the steady-state U.

## Simple constructions {#sec-simple-constructions}

**Source:** `ctf.rs::try_calculate_ctf_simple`.

A simple construction gives only a conduction U-factor $U$, a heat capacity
$C$ [J/(m²·K)] per unit area, absorptances and a roughness. Its inside and
outside faces share the same absorptances. OpenBSE turns these into a
synthetic layered construction and applies @sec-ctf to it. Here
$\tau = C/U$ and $R = 1/U$.

| Condition (first that applies) | Synthetic construction |
|---|---|
| $U \le 0$ | Adiabatic (all coefficients zero) |
| $C < 1$ J/(m²·K) | Steady state, $X_0 = Y_0 = Z_0 = U$ |
| $\tau < 10\,\Delta t$ | Lumped RC (@sec-ctf) |
| $C < 30$ kJ/(m²·K), mass inside, $R > 20$ | Insulation + timber |
| $C < 30$ kJ/(m²·K), mass inside | Wood + insulation + plasterboard |
| $R > 2R_\text{mass}$, mass outside | Mass + insulation + 12 mm plasterboard |
| $R > 2R_\text{mass}$, $R - R_\text{mass} > 10$ | NoMass insulation + mass |
| $R > 2R_\text{mass}$ | 9 mm wood + insulation + mass |
| otherwise | One homogeneous layer, $\rho c_p = 1.6$ MJ/(m³·K) |

: Simple-construction synthesis. $R_\text{mass}$ is the resistance of the mass
layer holding $C$ ($k = 1$, $\rho = 2000$ by default). {#tbl-simple-construction}

The layer properties were chosen to reproduce the ASHRAE 140 lightweight and
heavyweight constructions. For other assemblies, the synthetic layers set the
dynamic response, not the user's inputs. The synthesis also does not always
conserve $C$ ([#53](https://github.com/bbrannon4/OpenBSE/issues/53)).
EnergyPlus has no counterpart.

## F-factor ground floors {#sec-ffactor}

**Source:** `heat_balance.rs::from_input_full` (effective U) and
`compute_all_ctf`.

An F-factor floor of area $A$ and exposed perimeter $P$ [m] gets the
conduction transmittance

$$
U_\text{eff} = \frac{F P}{A},
$$ {#eq-ffactor}

with $F$ the F-factor [W/(m·K)]. When $P$ is missing it defaults to
$2\sqrt{A}$, with a warning. $U_\text{eff}$ and the construction's
`thermal_capacity` go through @sec-simple-constructions. The outside face is
held at the construction's monthly ground temperatures (constant within each
month), or at the building ground temperatures (@sec-ground-temps).

**Comparison with EnergyPlus.** `Construction:FfactorGroundFloor`
(`HeatBalanceManager::CreateFCfactorConstructions`) builds a NoMass layer of
resistance $A/(FP) - 0.135 - 0.03 - R_\text{con}$ plus a 0.15 m concrete layer.
That removes the air films already included in $F$. OpenBSE does not remove
the films, and it uses the synthetic layers instead of the concrete
([#205](https://github.com/bbrannon4/OpenBSE/issues/205)).

## Heat and moisture transfer (HAMT) {#sec-hamt}

**Source:** `openbse-envelope/src/hamt.rs`.

The module implements a one-dimensional coupled heat and moisture solver
after Künzel [@kunzel1995]. It is **not connected to the simulation**:
nothing calls it, and every surface uses CTF conduction. Its material inputs
(`vapor_resistance_factor`, `sorption_isotherm`, `liquid_transport_coeff`)
are accepted but have no effect. The module also has three known equation
errors ([#64](https://github.com/bbrannon4/OpenBSE/issues/64)). This section
will be written when the model is fixed and wired in.

## Known limitations: materials and constructions {#sec-constructions-limitations}

- [#53](https://github.com/bbrannon4/OpenBSE/issues/53): simple
  constructions use synthetic layers tuned to ASHRAE 140, and do not always
  conserve the specified heat capacity.
- [#64](https://github.com/bbrannon4/OpenBSE/issues/64): HAMT is not wired
  in and has known equation errors.
- [#203](https://github.com/bbrannon4/OpenBSE/issues/203): unknown layer
  materials are dropped silently; defaults for required properties; unread
  inputs; debug logging.
- [#204](https://github.com/bbrannon4/OpenBSE/issues/204): no CTF time-step
  escalation for massive constructions.
- [#205](https://github.com/bbrannon4/OpenBSE/issues/205): F-factor floors
  keep the air films and use synthetic mass.

## Surface heat balances {#sec-surface-hb}

**Source:** `heat_balance.rs::solve_timestep`, step 6 (6a outside faces,
6b inside faces).

Each opaque surface has an outside and an inside face temperature, linked by
the conduction of @eq-ctf. Windows have their own balance (@sec-window-hb).
Each envelope solve makes five passes over the following steps. There is no
convergence test between passes
([#209](https://github.com/bbrannon4/OpenBSE/issues/209)):

1. Solve the outside faces.
2. Solve the windows.
3. Iterate the inside faces.
4. Solve the zone air (@sec-zone-time).

The inside faces in each pass use the zone air temperature from the previous
pass.

### Outside face {#sec-outside-face}

For a surface exposed to outdoor air, the outside face balances absorbed
solar, convection, long-wave exchange with sky, air and ground, and
conduction into the wall:

$$
q''_{\alpha} + h_{c,o}(T_\text{o} - T_\text{so})
+ h_\text{sky}(T_\text{sky} - T_\text{so}) + h_\text{air}(T_\text{o} - T_\text{so})
+ h_\text{gnd}(T_\text{gnd} - T_\text{so}) = q''_\text{ko}.
$$ {#eq-outside-balance}

Substituting @eq-ctf for $q''_\text{ko}$ gives

$$
T_\text{so} = \frac{q''_{\alpha} + (h_{c,o} + h_\text{air})T_\text{o} + h_\text{sky}T_\text{sky}
+ h_\text{gnd}T_\text{gnd} + Y_0 T_\text{si} - H_\text{o}}
{X_0 + h_{c,o} + h_\text{air} + h_\text{sky} + h_\text{gnd}},
$$ {#eq-outside-temp}

where:

- $q''_\alpha$ is the absorbed solar flux (@sec-incident-solar, times the
  outside solar absorptance and the shading treatment of @sec-external-shading);
- $H_\text{o} = \sum_{j\ge1}(X_jT_\text{so}^{t-j} - Y_jT_\text{si}^{t-j} + \Phi_j q''^{\,t-j}_\text{ko})$
  is the history part of @eq-ctf;
- the denominator is floored at 1 W/(m²·K).

The ground is taken at the outdoor air temperature,
$T_\text{gnd} = T_\text{o}$, and the sky temperature comes from @eq-sky-temp.

The long-wave coefficients use the outside thermal emissivity $\varepsilon$
and view factors $F_\text{sky} = (1 + \cos\beta)/2$ and
$F_\text{gnd} = 1 - F_\text{sky}$. The sky hemisphere is split between the
sky and the air by $\beta_s = \sqrt{F_\text{sky}}$. Each coefficient is the
exact linearization about the face temperature of the previous pass:

$$
h_\text{sky} = \sigma\varepsilon F_\text{sky}\beta_s\,\frac{T_\text{so}^4 - T_\text{sky}^4}{T_\text{so} - T_\text{sky}},\quad
h_\text{air} = \sigma\varepsilon F_\text{sky}(1 - \beta_s)\,\frac{T_\text{so}^4 - T_\text{o}^4}{T_\text{so} - T_\text{o}},\quad
h_\text{gnd} = \sigma\varepsilon F_\text{gnd}\,\frac{T_\text{so}^4 - T_\text{gnd}^4}{T_\text{so} - T_\text{gnd}},
$$ {#eq-ext-hrad}

with temperatures in kelvin. When the temperatures are within 0.1 K,
$4\sigma\varepsilon F\bar T^3$ is used instead.

**Other boundary conditions:**

- **Ground:** the outside face is held at the ground temperature
  (@sec-ground-temps), or at the F-factor table (@sec-ffactor).
- **Adiabatic:** $T_\text{so} = T_\text{si}$. This also applies to internal
  mass, which becomes one adiabatic surface per `internal_mass` entry, with
  tilt 90°.
- **Interzone:** $T_\text{so}$ is the inside face temperature of the paired
  surface in the adjacent zone, updated as soon as that surface is solved.
  Surfaces are paired by zone name only, so zones that share more than one
  surface get mismatched pairs
  ([#207](https://github.com/bbrannon4/OpenBSE/issues/207)). A surface that
  names its own zone is adiabatic.

**Comparison with EnergyPlus.** @eq-outside-temp, @eq-ext-hrad, the
sky–air split and the ground at air temperature match
`HeatBalanceSurfaceManager::CalcOutsideSurfTemp` and
`ConvectionCoefficients::InitExtConvCoeff`
[@eplus-engref-25-2, "Outside Surface Heat Balance"]. The differences:

- **No wet surfaces.** EnergyPlus treats wind-exposed faces as wet during
  rain ($h_{c,o} = 1000$ W/(m²·K), face at the outdoor wet-bulb); OpenBSE
  does not ([#209](https://github.com/bbrannon4/OpenBSE/issues/209)).
- **No surrounding-surface or ground-temperature inputs** for long-wave
  exchange.
- **Interzone pairing.** EnergyPlus pairs interzone surfaces one-to-one by
  name.

### Inside face {#sec-inside-face}

The inside face balances conduction from the wall, convection to the zone
air, long-wave exchange with the zone's other surfaces, and the radiant flux
$q''_\text{r}$ it absorbs. That flux is the transmitted solar landing on it
plus its share of the radiant internal gains (@sec-radiant-gains; solar in @sec-interior-solar):

$$
q''_\text{ki} + h_{c,i}(T_z - T_\text{si}) + h_{r,i}(T_\text{mrt} - T_\text{si}) + q''_\text{r} = 0.
$$ {#eq-inside-balance}

The long-wave term is linearized about the zone's other surfaces at their
previous-iteration temperatures, giving $h_{r,i}$ and an effective
$T_\text{mrt}$ (@sec-lw-linear). The equation is solved by
Gauss–Seidel iteration with a damping term $D = 5$ W/(m²·K):

$$
T_\text{si}^{(n+1)} = \frac{Y_0 T_\text{so} + H_\text{i} + h_{c,i}T_z + h_{r,i}T_\text{mrt} + q''_\text{r} + D\,T_\text{si}^{(n)}}
{Z_0 + h_{c,i} + h_{r,i} + D},
$$ {#eq-inside-temp}

where $H_\text{i} = \sum_{j\ge1}(Y_jT_\text{so}^{t-j} - Z_jT_\text{si}^{t-j} + \Phi_j q''^{\,t-j}_\text{ki})$.

For an adiabatic surface, $T_\text{so} = T_\text{si}$, so the denominator
becomes $Z_0 - Y_0 + h_{c,i} + h_{r,i} + D$ and the $Y_0T_\text{so}$ term
drops. Iteration stops when no inside face changes by more than 0.002 K, or
after 500 iterations. $h_{c,i}$ and $h_{r,i}$ are re-evaluated every
iteration. A face temperature that is not finite, or beyond ±200 °C, is
held at its previous value and reported (@sec-diagnostics).

**Comparison with EnergyPlus.** @eq-inside-temp, the damping constant
(`IterDampConst` = 5), the tolerance (`MaxAllowedDelTemp` = 0.002 K) and the
iteration limit match `HeatBalanceSurfaceManager::CalcHeatBalanceInsideSurf2`
[@eplus-engref-25-2, "Inside Heat Balance"]. Two differences:

- **Long-wave exchange.** EnergyPlus adds the net long-wave exchange as a
  flux computed from the previous iteration; OpenBSE linearizes it as
  $h_{r,i}(T_\text{mrt} - T_\text{si})$.
- **Zone air temperature.** EnergyPlus holds the zone air at the previous
  timestep's value for the whole surface solution. OpenBSE iterates
  surfaces and air together (@sec-surface-hb, @sec-hvac-iteration).

## Convection coefficients {#sec-convection}

**Source:** `openbse-envelope/src/convection.rs`.

### Natural convection (TARP) {#sec-tarp}

Natural convection from a surface follows the TARP correlations
[@walton1983] (`calc_natural_convection`). With $\Delta T = T_s - T_a$ and
$\cos\Sigma$ the cosine of the surface tilt, as seen from the side the air
is on:

$$
h_n =
\begin{cases}
1.31\,|\Delta T|^{1/3} & |\cos\Sigma| < 0.01 \text{ (vertical)},\\[0.5ex]
\dfrac{9.482\,|\Delta T|^{1/3}}{7.238 - |\cos\Sigma|} & \Delta T \text{ and } \cos\Sigma \text{ of the same sign (unstable)},\\[1.5ex]
\dfrac{1.810\,|\Delta T|^{1/3}}{1.382 + |\cos\Sigma|} & \text{otherwise (stable)},
\end{cases}
$$ {#eq-tarp}

with $h_n \ge 0.1$ W/(m²·K). Inside faces use $\cos\Sigma = -\cos\beta$,
since the inside face of a roof faces down; outside faces use $\cos\beta$.

**Inside faces** of opaque surfaces use @eq-tarp with the zone air (or, in a
stratified zone, the local air at the surface's height; @sec-envelope-prereview). **Window inside faces** use a separate correlation
(`window_interior_convection`):
$Nu = (Nu_T^6 + Nu_s^6)^{1/6}$, with $Nu_T = 0.0673838\,Ra^{1/3}$ and
$Nu_s = 0.242\,(Ra/A_r)^{0.272}$, where $Ra$ is based on the window height and
$A_r$ is height over width. That is the ISO 15099 glazing-cavity correlation.
EnergyPlus applies ISO 15099's indoor-surface correlation
($Nu = 0.56(Ra\sin\gamma)^{1/4}$, with tilt and heat-flow cases), which gives
about 20% more for a typical window
([#208](https://github.com/bbrannon4/OpenBSE/issues/208)).

### Outside faces (DOE-2) {#sec-doe2}

Wind-exposed outside faces use the DOE-2 model (`exterior_convection_full`).
It combines natural convection with the MoWiTT forced-convection correlation
for smooth glass [@yazdanian1994], scaled by a roughness multiplier $R_f$:

$$
h_{f,\text{glass}} =
\begin{cases}
3.26\,V^{0.89} & \text{windward},\\
3.55\,V^{0.617} & \text{leeward},
\end{cases}
\qquad
h_{c,o} = h_n + R_f\left(\sqrt{h_n^2 + h_{f,\text{glass}}^2} - h_n\right),
$$ {#eq-doe2}

with $h_n$ from @eq-tarp, $V$ the wind speed at the surface, and $R_f$ from @tbl-roughness. A surface is windward when it is within 2° of horizontal
($|\cos\beta| \ge 0.98$), or when the wind direction is within 100° of its
azimuth. Surfaces with `wind_exposure: false` use $h_n$ alone.

| Roughness | Very rough | Rough | Medium rough | Medium smooth | Smooth | Very smooth |
|---|---|---|---|---|---|---|
| $R_f$ | 2.17 | 1.67 | 1.52 | 1.13 | 1.11 | 1.00 |

: Roughness multipliers. {#tbl-roughness}

**Wind at the surface.** The weather-station wind (10 m, open country) is
moved to the surface's centroid height $z$ with the ASHRAE power law
[@ashrae2021fund, Ch. 24]:

$$
V(z) = V_\text{met}\left(\frac{270}{10}\right)^{0.14}\left(\frac{z}{\delta}\right)^{a},
$$ {#eq-wind-profile}

with the site's terrain exponent $a$ and boundary-layer height $\delta$:

| Terrain | Country | Suburbs (default) | City | Ocean |
|---|---|---|---|---|
| $a$ | 0.14 | 0.22 | 0.33 | 0.10 |
| $\delta$ [m] | 270 | 370 | 460 | 210 |

: Wind-profile parameters. {#tbl-terrain}

$z$ is the mean height of the surface's vertices. For surfaces given
without vertices, it is estimated from the zone height: floors 0.1 m, walls
mid-height, roofs at the top. That ignores the floor the zone is on.

**Comparison with EnergyPlus.** These match EnergyPlus's default algorithms,
TARP inside and DOE-2 outside (`ConvectionCoefficients::CalcASHRAETARPNatural`,
`CalcDOE2Forced`, `Windward`, `SetSurfaceWindSpeedAt`), with the same
roughness multipliers, windward test and wind profile
[@eplus-engref-25-2, "Exterior Convection", "Interior Convection"]. Window
inside faces are the exception
([#208](https://github.com/bbrannon4/OpenBSE/issues/208)). EnergyPlus's other
algorithms (adaptive convection, ASHRAE simple, MoWiTT alone) are not
available.

## Known limitations: surface heat balances {#sec-surface-hb-limitations}

- [#207](https://github.com/bbrannon4/OpenBSE/issues/207): interzone
  surfaces are paired by zone name only.
- [#208](https://github.com/bbrannon4/OpenBSE/issues/208): window inside
  convection uses the glazing-cavity correlation.
- [#209](https://github.com/bbrannon4/OpenBSE/issues/209): five fixed
  surface–air passes with no convergence test; no wet surfaces in rain.

## Interior long-wave exchange {#sec-lw-exchange}

**Source:** `openbse-envelope/src/view_factors.rs` (`direct_view_factors`,
`polygon_af`, `approximate_view_factors`, `fix_view_factors`,
`interchange_areas`, `linearised_exchange`); set up in
`heat_balance.rs::from_input_full` and used by `lw_exchange` in the inside
face iteration (@sec-inside-face).

The inside faces of a zone, windows and internal mass included, form a grey,
diffuse enclosure. The zone air does not take part.

### View factors {#sec-view-factors}

**From polygons.** When every surface of a zone has vertices (internal mass
excepted), each pair's $A_aF_{ab}$ comes from the contour-integral form of
the view factor [@siegel2002]:

$$
A_a F_{ab} = \frac{1}{2\pi}\oint_{C_a}\oint_{C_b} \ln r\ \,d\mathbf{r}_a\cdot d\mathbf{r}_b,
$$ {#eq-contour-vf}

which reduces to a sum over edge pairs (`edge_pair_integral`):

- Collinear edges use a closed form.
- Other pairs use 8-point Gauss–Legendre quadrature. The longer edge is
  bisected while the edges are closer than their length, which isolates the
  logarithmic singularity.

Before integrating, each polygon is clipped to the part in front of the
other. A pair that can only partly see each other (a non-convex zone)
therefore gets the factor between the visible parts. Obstruction by third
surfaces is ignored, so the factors are exact only for a convex zone.
Windows and doors are cut out of their parent surfaces by additivity:

$$
A F(\text{net } i \to \text{net } j) = \sum_{a \in \{i, \text{children of } i\}}\ \sum_{b \in \{j, \text{children of } j\}} s_a s_b\, A_aF_{ab},
$$ {#eq-vf-additivity}

with $s = +1$ for the parent outline and $-1$ for each child.

Internal mass has no geometry. It sees every other surface in proportion to
area. Each other surface gives up a share of its view to it, equal to the
mass's share of the area that surface sees under the approximate rule below.

**Approximate.** When any surface lacks vertices, the zone uses EnergyPlus's
approximate factors (`CalcApproximateViewFactors`):

$$
F_{ij} = \frac{A_j}{\sum_{k \in S_i} A_k},
$$ {#eq-approx-vf}

where $S_i$ is the set of surfaces $i$ sees. That excludes $i$ itself, other
floors if $i$ is a floor, and surfaces whose azimuth and tilt are both within
10° of $i$'s, unless one of the two is a floor or internal mass.

**Reciprocity and completeness.** Either set of factors then goes through
EnergyPlus's `FixViewFactors`: $A_iF_{ij}$ and $A_jF_{ji}$ are averaged, and
each row is rescaled towards $\sum_j F_{ij} = 1$, alternating until the total
error settles (0.001) or stops changing (10⁻⁵), within 400 iterations. A
surface larger than all the others together gets a self-view factor of
$\min(1.2A_k/\sum A, 0.9)$. Zones with three or fewer surfaces get
reciprocity only.

### Grey-body exchange (ScriptF) {#sec-scriptf}

With room-facing emissivities $\varepsilon_i$, which are:

- the inside thermal absorptance for opaque surfaces;
- the glass emissivity for windows;
- the shade's emissivity when an interior shade is deployed;

the radiosity balance $J = \varepsilon E_b + (1-\varepsilon)FJ$ gives
Hottel's interchange areas [@hottel1967]:

$$
G = \operatorname{diag}\!\left(\frac{A\varepsilon}{1-\varepsilon}\right)\left[I - \operatorname{diag}(1-\varepsilon)F\right]^{-1}\operatorname{diag}(\varepsilon),
\qquad G_{ii} = 0,
$$ {#eq-scriptf}

symmetrized. The emissivity is capped at 0.99999. The net long-wave gain of
surface $i$ is

$$
Q_i = \sum_j G_{ij}\,\sigma\left(T_j^4 - T_i^4\right),
$$ {#eq-lw-net}

which sums to zero over the zone. $G$ is rebuilt whenever a shade is deployed
or retracted.

### Linearization {#sec-lw-linear}

In the inside face iteration, @eq-lw-net is written exactly as
$h_{r,i}(T_\text{mrt} - T_{\text{si}})$, with the other surfaces at their
previous-iteration temperatures (in kelvin inside the brackets):

$$
h_{r,i} = \frac{1}{A_i}\sum_{j\ne i} w_{ij}, \qquad
T_\text{mrt} = \frac{\sum_j w_{ij}T_j}{\sum_j w_{ij}}, \qquad
w_{ij} = G_{ij}\,\sigma\,(T_i^2 + T_j^2)(T_i + T_j).
$$ {#eq-lw-linear}

A surface with nothing to exchange with gets $h_{r,i} = 0$.

### Radiant gains {#sec-radiant-gains}

The radiant part of internal gains, plus the radiant output of HVAC panels
(@sec-zone-equipment), is spread evenly over the zone's surfaces by area,
windows included (`zone_radiant_gain_flux`):

$$
q''_{r,\text{int}} = \frac{\dot Q_\text{rad}}{\sum_k A_k}.
$$ {#eq-radiant-gain-flux}

Each surface absorbs that flux whatever its emissivity.

**Comparison with EnergyPlus.** EnergyPlus uses the approximate factors of
@eq-approx-vf by default; polygon factors require user input
(`ZoneProperty:UserViewFactors`). OpenBSE computes them from the geometry,
a deliberate improvement from PR #161. `FixViewFactors`, the ScriptF
interchange (`CalcScriptF`) and the exchange (`CalcInteriorRadExchange`)
match EnergyPlus [@eplus-engref-25-2, "Internal Long-Wave Radiation Exchange"]. OpenBSE's
approximate rule also treats azimuths 350° and 10° as within 10°, which
EnergyPlus does not.

EnergyPlus distributes radiant gains in proportion to $A_i\varepsilon_i$
(`ComputeIntThermalAbsorpFactors`). OpenBSE ignores the emissivity, while
its own `ZoneHvacConditions` documentation claims the E+ weighting
([#211](https://github.com/bbrannon4/OpenBSE/issues/211)).

## Known limitations: long-wave exchange {#sec-lw-limitations}

- [#52](https://github.com/bbrannon4/OpenBSE/issues/52): superseded by the
  polygon view factors. The remaining limitation is that obstruction by third
  surfaces is ignored in non-convex zones.
- [#211](https://github.com/bbrannon4/OpenBSE/issues/211): radiant gains are
  spread by area only, not area × emissivity.

## Windows {#sec-windows}

**Source:** `material.rs::WindowConstruction`; `solar.rs` (glazing optics:
`SgsAngularModel`, `compute_glass_angular_params`, `angular_shgc_modifier_u`,
the Fresnel functions); `heat_balance.rs::solve_timestep` (steps 5b and 6,
window branch), `sealed_air_gap_conductance`, `solve_window_with_shade`,
`shade_solar`; `openbse-cli/src/main.rs::evaluate_blind_control`.

A window construction is described by its NFRC U-factor $U$ (films
included) and its normal-incidence $SHGC$. Optional data refine it:

- per-pane solar transmittance and reflectance;
- the number of panes;
- the gap width;
- the pane conductivity and thickness;
- the glass emissivity (default 0.84);
- a shading control.

`visible_transmittance`, `solar_absorptance` and `inside_absorbed_fraction`
are accepted but not used
([#213](https://github.com/bbrannon4/OpenBSE/issues/213)). A window is one
surface whose temperature is the inside glass face. Its area is subtracted
from its parent surface.

### Solar optics {#sec-window-optics}

The solar gain of a window is split into the transmitted part, which lands
on the zone's surfaces (@sec-interior-solar), and the absorbed-inward
part, which heats the glass:

$$
SHGC(\theta) = \tau(\theta) + N_i\,\alpha(\theta), \qquad \alpha = 1 - \tau - \rho,
$$ {#eq-shgc-split}

where $N_i$ is the inward-flowing fraction of the absorbed solar. Beam
irradiance uses the values at its incidence angle $\theta$. Diffuse (sky,
circumsolar, horizon and ground) uses hemispherical averages, computed as
$\int_0^{\pi/2} X(\theta)\cos\theta\sin\theta\,d\theta \big/ \int_0^{\pi/2}\cos\theta\sin\theta\,d\theta$
over 200 angles. Which angular model a window gets depends on its inputs
([#213](https://github.com/bbrannon4/OpenBSE/issues/213), comment):

1. **Simple glazing** (no pane data, $SHGC < 0.55$). This is EnergyPlus's
   SimpleGlazingSystem model [@arasteh2009]. The normal-incidence
   transmittance comes from the U-dependent correlations, for example for
   low U:

   $$
   \tau_\text{sol} =
   \begin{cases}
   0.41040\,SHGC & SHGC \le 0.15,\\
   0.085775\,SHGC^2 + 0.963954\,SHGC - 0.084958 & \text{otherwise},
   \end{cases}
   $$ {#eq-sgs-tsol}

   with the high-U form for $U > 4.5424$ and linear interpolation between
   3.4068 and 4.5424 W/(m²·K). The glass resistance and inward fraction use
   fixed films ($h_i = 8.29$, $h_o = 26.0$):

   $$
   R_l = \frac1U - \frac1{h_o} - \frac1{h_i},\qquad
   N = \frac{R_l/2 + 1/h_o}{R_l + 1/h_o + 1/h_i},\qquad
   \rho_\text{sol} = 1 - \tau_\text{sol} - \frac{SHGC - \tau_\text{sol}}{N}.
   $$ {#eq-sgs-films}

   The angular dependence uses EnergyPlus's curve set A–J for the window's
   U/SHGC bin:
   $\tau(\theta) = \tau_\text{sol}\,P_T(\cos\theta)$ and
   $\rho(\theta) = \rho_\text{sol}(1 - P_R + P_T) + (P_R - P_T)$, where
   $P_T$ and $P_R$ are fourth-order polynomials in $\cos\theta$.
2. **Fresnel** (per-pane $\tau_p$ and $\rho_p$ given, or no pane data and
   $SHGC \ge 0.55$). Two identical panes of refractive index $n$ and
   extinction thickness $kd$. With per-pane data, $n$ is fitted to $\rho_p$
   and $kd$ to $\tau_p$; otherwise $n = 1.526$ and $kd$ is fitted to an
   estimated transmittance. Each pane's transmittance and reflectance follow
   from the Fresnel reflectance $r = (r_s + r_p)/2$ and internal absorption
   $e^{-kd/\cos\theta'}$, and the two panes are combined with their
   inter-reflections:

   $$
   \tau_2 = \frac{\tau_1^2}{1 - \rho_1^2},\qquad \rho_2 = \rho_1 + \frac{\tau_1^2\rho_1}{1 - \rho_1^2}.
   $$ {#eq-fresnel-double}

   $N_i$ is fitted so @eq-shgc-split reproduces the SHGC at normal
   incidence. For $0.25 < SHGC < 0.55$ the angular modifier is blended with a
   U-dependent polynomial, and for $SHGC \le 0.25$ the polynomial replaces
   it.

The transmitted solar then splits into beam and diffuse parts
(@sec-interior-solar). The absorbed-inward part is
$SHGC\cdot I - \tau\cdot I$, per unit window area.

### Window heat balance {#sec-window-hb}

The glass is one node, at the inside face temperature $T_g$:

$$
\begin{gathered}
u_{e,g}\left(T_\text{ext} - T_g\right) + h_{c,i}(T_z - T_g) + h_{r,i}(T_\text{mrt} - T_g)
+ q''_\text{abs,in} + q''_{r,\text{int}} = 0,\\
u_{e,g} = \left(\frac{1}{h_e} + \frac{1}{u_g}\right)^{-1}.
\end{gathered}
$$ {#eq-window-node}

The terms are:

- **Outside film.** $h_e = h_{c,o} + h_\text{sky} + h_\text{air} + h_\text{gnd}$
  is the outside film, with @eq-doe2 and @eq-ext-hrad at the glass
  emissivity. $T_\text{ext}$ is the $h$-weighted mean of outdoor air, sky
  and ground temperature.
- **Inside film.** $h_{c,i}$ is the window inside convection
  (@sec-tarp), and $h_{r,i}$ and $T_\text{mrt}$ come from @eq-lw-linear at
  the glass emissivity.
- **Glass conductance** $u_g$:
  - **Film-stripped** (default): $u_g = (1/U - 1/26 - 1/8.29)^{-1}$.
  - **Single pane** (`num_panes: 1` with pane data): $u_g = k/L$.
  - **Gap model** (gap width and pane data):
    $u_g = (2L/k + 1/h_\text{gap})^{-1}$, with an air gap whose
    $h_\text{gap} = Nu\,k_\text{air}/s + h_{r,\text{gap}}$ follows ISO 15099
    [@iso15099]. $Nu$ comes from the vertical-cavity correlations for tilt
    ≥ 60° and from Hollands et al. [@hollands1976] for inclined gaps. The
    result is capped at 1.05 times the film-stripped value. The model assumes
    two panes, whatever `num_panes` is
    ([#214](https://github.com/bbrannon4/OpenBSE/issues/214)).

@eq-window-node is solved in five fixed iterations, updating $h_{c,i}$,
$h_{r,i}$, $T_\text{mrt}$ and the gap conductance. When
$h_{c,i} + h_{r,i} < 2$ W/(m²·K), they are floored at 0.5 and 1.5. The outside
glass face temperature is back-calculated from the conduction through $u_g$.

Any part of $q''_\text{abs,in}$ that the node conducts outdoors is added
directly to the zone air. The whole absorbed-inward gain therefore reaches
the zone, as the SHGC definition implies. The glass's convection enters the
zone air balance through $h_{c,i}A(T_g - T_z)$.

### Interior shades and blinds {#sec-shades}

A window construction may carry a shading control that deploys a shade or
blind. It deploys when its signal exceeds the trigger and retracts when the
signal falls below the trigger minus the hysteresis. The signal is either:

- the window's incident irradiance per unit area from the previous
  timestep; or
- the zone air temperature at the start of the timestep.

A deployed device is modelled in one of two ways:

- **Assembly rating** (`blind_shgc`, `blind_u_factor`). All solar gains are
  scaled by $SHGC_\text{blind}/SHGC$ (clamped to [0, 1]), and $u_g$ is the
  film-stripped blind U-factor.
- **Shade layer** (`shade`). The fabric's solar transmittance $\tau_{sh}$
  (with openness $o$), reflectance $\rho_{sh}$ and emissivity are given.
  Solar transmitted by the glass ($I_g$, beam $I_{g,b}$) bounces between
  glass and shade:

  $$
  \begin{gathered}
  I_{sh} = \frac{I_g}{1 - \rho_{sh}\rho_g},\qquad
  \text{beam} = o\,I_{g,b},\qquad \text{diffuse} = \tau_{sh}I_{sh} - o\,I_{g,b},\\
  \text{shade} = \alpha_{sh}I_{sh},\qquad \text{glass} = \rho_{sh}I_{sh}\alpha_g,
  \end{gathered}
  $$ {#eq-shade-solar}

  where $\rho_g$ and $\alpha_g$ are the glazing's room-side hemispherical
  properties; the rest, $\rho_{sh}I_{sh}\tau_g$, leaves through the glass.
  Glass, shade and gap air are then three nodes solved together, ten times
  with updated coefficients:

  $$
  \begin{aligned}
  u_{e,g}(T_\text{ext} - T_g) + q''_g &= h_{r,gs}(T_g - T_{sh}) + h_{cv}(T_g - T_a),\\
  q''_{sh} &= h_{r,gs}(T_{sh} - T_g) + h_{cv}(T_{sh} - T_a)\\
  &\quad + h_{cs}(T_{sh} - T_z) + h_{r,i}(T_{sh} - T_\text{mrt}),\\
  h_{cv}(T_g - T_a) + h_{cv}(T_{sh} - T_a) &= h_\text{vent}(T_a - T_z),
  \end{aligned}
  $$ {#eq-shade-nodes}

  with:
  - $h_{r,gs} = 4\sigma\bar T^3/(1/\varepsilon_g + 1/\varepsilon_{sh} - 1)$;
  - gap convection $h_{cv} = 2h_c + 4v$ and ventilation
    $h_\text{vent} = \rho c_p v d/H$;
  - the buoyant gap velocity $v = 0.7\sqrt{gH|T_a - T_z|/T_z}$ [@iso15099].

  The discharge coefficient 0.7 was calibrated against EnergyPlus interior
  shade runs (#144). The room sees the shade as the window's inside face.

**Comparison with EnergyPlus.**

- **Simple-glazing correlations and curves.** The transmittance
  correlations and angular curves follow
  `MaterialGlass::SetupSimpleWindowGlazingSystem`
  [@eplus-engref-25-2, "Simple Window Model"]. EnergyPlus uses U-dependent
  winter films for $R_l$, SHGC-dependent solar films for $N$, and U bounds of
  3.4 and 4.5 ([#213](https://github.com/bbrannon4/OpenBSE/issues/213)).
- **Thermal model.** EnergyPlus solves two face temperatures per glass
  layer, with each layer's absorbed solar, per-gap gas properties and the
  full ISO 15099 gap correlations (`WindowManager::SolveForWindowTemperatures`).
  The lumped node of @eq-window-node, the two-pane gap model and the caps are
  deviations ([#214](https://github.com/bbrannon4/OpenBSE/issues/214)).
- **Inside convection.** It differs (@sec-tarp,
  [#208](https://github.com/bbrannon4/OpenBSE/issues/208)).
- **Shades.** The shade layer follows `WindowMaterial:Shade` with an
  ISO 15099 vented gap, simplified to fixed air properties and one gap
  velocity.
- **Shading control.** It corresponds to `WindowShadingControl` with the
  `OnIfHighSolarOnWindow` and `OnIfHighZoneAirTemperature` triggers. The
  hysteresis is an addition.

## Known limitations: windows {#sec-windows-limitations}

- [#208](https://github.com/bbrannon4/OpenBSE/issues/208): window inside
  convection uses the glazing-cavity correlation.
- [#213](https://github.com/bbrannon4/OpenBSE/issues/213): simple-glazing
  films and bounds differ from EnergyPlus; three unread inputs; angular-model
  selection by input.
- [#214](https://github.com/bbrannon4/OpenBSE/issues/214): lumped glass node,
  two-pane air-only gap model, ad hoc caps.

## Interior solar distribution {#sec-interior-solar}

**Source:** `heat_balance.rs::solve_timestep`, step 5c; precomputed per
surface and applied as $q''_\text{r}$ in @eq-inside-temp.

The beam and diffuse solar transmitted by a zone's windows (@sec-window-optics)
is distributed in four steps.

1. **Beam landing.** `simulation.solar_distribution` selects how the beam
   lands:
   - **FullExterior** (default): all beam lands on the zone's floors, in
     proportion to their area.
   - **FullInteriorAndExterior:** each window's polygon is projected along
     the sun direction onto every inside surface that faces the sun (other
     than its parent wall), and clipped to that surface (Sutherland–Hodgman
     [@sutherland1974]). The beam landing on surface $j$ is
     $I_{b,\text{win}}\,A_{\text{overlap},j}\cos\theta_j/\cos\theta_\text{win}$.
     A zone in which any surface other than internal mass lacks vertices
     falls back to the fixed `solar_distribution` fractions of the zone, or,
     without them, to putting all transmitted solar into the zone air
     ([#68](https://github.com/bbrannon4/OpenBSE/issues/68)).
2. **Beam absorbed and reflected.** Surface $j$ absorbs
   $\alpha_j S_{b,j}$, where $S_{b,j}$ is the beam landing on it and
   $\alpha_j$ its inside solar absorptance. The rest of the beam joins the
   diffuse pool.
3. **Cavity back-out.** Part of the diffuse pool leaves the zone again
   through the windows: their back transmittance $\tau_b$ plus the outward
   share of their back absorptance $\alpha_b$. The pool kept is
   $$
   S_d = \left(S_\text{d,win} + S_b - \sum_j\alpha_jS_{b,j}\right)
   \frac{\sum_k A_k\alpha_k}{\sum_k A_k\alpha_k + \sum_w A_w\left[\tau_b + \alpha_b(1 - N_i)\right]},
   $$ {#eq-cavity-backout}
   with $k$ over opaque surfaces and $w$ over windows.
4. **Diffuse absorbed.** Every opaque surface absorbs in proportion to
   $A_k\alpha_k$ (EnergyPlus's VMULT):
   $$
   Q_{\text{sol},k} = \alpha_k S_{b,k} + \frac{A_k\alpha_k}{\sum_m A_m\alpha_m}\,S_d.
   $$ {#eq-vmult}

**Comparison with EnergyPlus.** FullExterior, the absorbed and reflected
beam, the VMULT distribution and the back-out follow
`HeatBalanceSurfaceManager::ComputeIntSWAbsorpFactors` and
`InitIntSolarDistribution` [@eplus-engref-25-2, "Solar Distribution"]. The
differences ([#68](https://github.com/bbrannon4/OpenBSE/issues/68)):

- **Windows absorb no diffuse.** EnergyPlus includes windows in the VMULT
  sum, and the glass absorbs its share of the reflected diffuse. OpenBSE
  gives that share to the opaque surfaces.
- **Fallback.** Zones that lack vertices fall back to fixed fractions or
  to the air, rather than to FullExterior.
- **Beam projection.** The projection uses the whole window, not its
  sunlit part, and has no first-hit test for non-convex zones.

## External shading {#sec-external-shading}

**Source:** `openbse-envelope/src/shading.rs` (`calculate_sunlit_fraction`,
`compute_diffuse_sky_shading_ratio`, `compute_diffuse_horizon_shading_ratio`,
`generate_overhang_vertices`, `generate_fin_vertices`);
`heat_balance.rs::precompute_solar`, `compute_diffuse_shading_ratios`, step 5a.

With `simulation.shading_calculation: basic` (the default), every outdoor
surface is fully sunlit and no diffuse is shaded. The model's overhangs,
fins, shading surfaces and the building's own surfaces are then ignored
([#216](https://github.com/bbrannon4/OpenBSE/issues/216)). With `detailed`:

**Casters** are:

- every outdoor surface with vertices (self-shading);
- each `shading_surfaces` polygon;
- the overhangs and fins generated for windows from their depth,
  extension and offset inputs.

A surface does not shade itself, and casters coplanar with the receiver
(e.g. a wall around its window) are excluded.

**Beam.** The sunlit fraction $f_s$ of a receiving surface projects each
caster along the sun direction onto the receiver's plane and clips it to the
receiver polygon. A surface facing away from the sun has $f_s = 0$. With a
single opaque caster, $f_s = 1 - A_\text{shadow}/A$ exactly. With more than
one caster, $f_s$ is the share of an $8\times8$ grid of points on the
receiver that no opaque caster covers. That handles overlapping shadows,
quantized to the grid ([#68](https://github.com/bbrannon4/OpenBSE/issues/68)).
Casters with a solar transmittance above zero cast no beam shadow
([#217](https://github.com/bbrannon4/OpenBSE/issues/217)). The fractions are
computed for every timestep of the run period, in parallel, and cached in a
`.solar` file keyed by the geometry.

**Diffuse.** Two time-independent ratios are computed at initialization:

- **Sky** ($R_\text{sky}$). Rays are cast from up to $5\times5$ points on the
  receiver to $6\times24$ sky patches (altitudes 7.5°–82.5°). Each patch is
  weighted by $\cos\phi\cos\theta_i$ (projected solid angle) [@eplus-engref-25-2,
  "Shading Module"], and a ray hit by any caster is blocked:
  $$
  R_\text{sky} = \frac{\sum_\text{patches} w\,\bar u}{\sum_\text{patches} w},
  $$ {#eq-sky-ratio}
  where $\bar u$ is the fraction of the receiver's points whose ray to the
  patch is unblocked.
- **Horizon** ($R_\text{hz}$). The same test over a near-horizon band
  (0–10°) from $3\times3$ points.

**Applied** to @eq-incident:

$$
I_\text{shaded} = f_s\left(I_\text{beam} + I_\text{cs}\right) + R_\text{sky}I_\text{iso} + R_\text{hz}I_\text{hz} + I_\text{gnd},
$$ {#eq-shaded-incident}

The ground-reflected part is not shaded. For windows, $R_\text{sky}$ is
floored at 0.75 and $R_\text{hz} = 1$.

**Comparison with EnergyPlus.** EnergyPlus projects shadows and combines
overlapping shadows by exact polygon operations (`SolarShading::SHADOW`,
`SHDGSS`), updated every 20 days by default. OpenBSE computes shadows every
timestep, but with grid sampling when shadows overlap. The 144-patch sky
ratio follows `SkyDifSolarShading`.

The differences:

- **Default.** Shading is off by default; EnergyPlus always computes it
  ([#216](https://github.com/bbrannon4/OpenBSE/issues/216)).
- **Transmittance.** Semi-transparent casters are ignored for beam and
  opaque for diffuse ([#217](https://github.com/bbrannon4/OpenBSE/issues/217)).
- **Windows.** The window floor and unshaded horizon
  ([#68](https://github.com/bbrannon4/OpenBSE/issues/68)).
- **Reflections.** There are no reflections from shading or building
  surfaces (EnergyPlus's `...WithReflections` options).

## Known limitations: solar distribution and shading {#sec-solar-limitations}

- [#68](https://github.com/bbrannon4/OpenBSE/issues/68): multi-caster point
  sampling; window diffuse ratio floor and unshaded horizon; interior
  distribution fallbacks and projection limits.
- [#216](https://github.com/bbrannon4/OpenBSE/issues/216): external shading
  off by default; shading geometry silently ignored.
- [#217](https://github.com/bbrannon4/OpenBSE/issues/217): semi-transparent
  casters.

## Infiltration and ventilation {#sec-infiltration}

**Source:** `openbse-envelope/src/infiltration.rs`; `zone.rs`
(`scheduled_ventilation_flow`, `NaturalVentilationInput`); `heat_balance.rs::solve_timestep`,
steps 4 and 6c; top-level inputs resolved in `openbse-io/src/input.rs::resolve_zone_loads`.

Every outdoor-air stream enters the zone air balance (@eq-zone-ode) at the
outdoor temperature, as part of $\dot m_\text{inf}$. The streams are
computed each solve from the start-of-solve zone temperature $T_z$, the
outdoor temperature $T_o$ and the wind.

**Infiltration (design flow rate).** Each infiltration object gives a
design flow $\dot V_d$ (absolute, ACH, per floor area or per exterior wall
area), resolved per zone:

$$
\dot V_\text{inf} = \dot V_d\, f_\text{sch}\,\max\!\left(0,\ A + B|T_z - T_o| + C\,V + D\,V^2\right),
\qquad \dot m_\text{inf} = \rho\,\dot V_\text{inf},
$$ {#eq-infiltration}

where:

- $f_\text{sch}$ is the schedule fraction;
- $V$ is the local wind speed at the zone's centroid height (@eq-wind-profile);
- $\rho$ is the outdoor-air density by default, or the zone-air density
  with `density_basis: zone`.

**Exhaust interaction.** By default (`basic`), exhaust removes zone air
without drawing in outdoor air. With `ashrae_combined`, the outdoor flow is
$\sqrt{\dot m_\text{inf}^2 + \dot m_\text{ex,unb}^2}$, where
$\dot m_\text{ex,unb} = \max(0, \dot m_\text{ex} - \dot m_\text{oa,zone})$
is the unbalanced exhaust [@ashrae2021fund, Ch. 16].

**Scheduled ventilation.** A `ventilation` object adds a constant flow
(absolute or ACH, plus per-person and per-area parts using the design
occupancy) during an hour window. It can be gated by a minimum indoor
temperature, and by requiring the outdoor air to be cooler than the zone.
It enters at outdoor density.

**Zone outdoor air.** A zone `outdoor_air` requirement gives the flow

$$
\dot V_\text{oa} = R_p P_z(t) + R_a A_z + \dot V_\text{abs} + \text{ACH}\cdot V_z/3600
$$ {#eq-zone-oa}

(or the largest of the four), with $P_z(t)$ the scheduled occupancy. Where
the flow enters depends on what serves the zone (@sec-framework):

- **Air loop with outdoor air.** If the zone's air loop carries outdoor air
  (minimum outdoor-air fraction > 0), it arrives only through the supply.
- **No outdoor air in the loop.** For a recirculating unit, the flow enters
  the zone directly at $T_o$.
- **No air loop.** It also enters directly at $T_o$, except in sizing.

The exhaust requirement, if any, raises the zone exhaust flow.

**Exhaust fans.** A zone exhaust fan removes
$\dot m_\text{ex} = \rho_o \dot V f_\text{sch}$. Its power and heat are:

- power $P = \dot m_\text{ex}\Delta p/(\eta\rho_o)$;
- the heat that stays in the zone is the motor loss not in the airstream,
  $P - [\eta_m P + (1-\eta_m)P f_\text{air}]$;
- the rest leaves with the exhaust.

**Natural ventilation (wind and stack).** An operable opening of area
$A_o$, available by schedule, temperature limits, an optional
indoor–outdoor ΔT and a wind limit (with on/off hysteresis), passes

$$
\dot V_\text{nv} = \sqrt{\left(C_w A_o f V_\text{met}\right)^2 + \left(C_d A_o f\sqrt{\frac{2g\,\Delta H\,|T_z - T_o|}{T_z + 273.15}}\right)^2},
$$ {#eq-natvent}

where:

- $C_w = 0.55$ when the wind is within 90° of the opening's angle, and
  0.30 otherwise;
- $C_d$ is a constant (default 0.65);
- $\Delta H$ is the stack height difference;
- $V_\text{met}$ is the weather-station wind.

Natural ventilation is off during sizing. When the AFN is active, its
openings carry this flow instead.

**Comparison with EnergyPlus.**

- **Matches.** @eq-infiltration is `ZoneInfiltration:DesignFlowRate`,
  including E+ 25.2's outdoor density default and the zone's local wind
  (`ZoneEquipmentManager::CalcAirFlowSimple`). The exhaust treatment matches
  E+ without zone air mass balance.
- **Natural ventilation.** @eq-natvent differs from
  `ZoneVentilation:WindandStackOpenArea` with autocalculated coefficients.
  E+ uses $C_w = 0.55 - 0.25\theta/45°$, zero leeward, and
  $C_d = 0.40 + 0.0045|\Delta T|$ with the local wind
  ([#219](https://github.com/bbrannon4/OpenBSE/issues/219)).
- **Scheduled ventilation.** It lacks the coefficients and schedule of
  `ZoneVentilation:DesignFlowRate`.
- **Outdoor temperature.** E+'s height-dependent outdoor temperature
  (`Site:HeightVariation`) is not applied.
- **Other infiltration models.** E+'s effective-leakage-area and
  flow-coefficient models are not available.

## Airflow network {#sec-afn}

**Source:** `openbse-envelope/src/airflow_network.rs` (`build_network`,
`solve_pressures`, `flow_and_derivative`, `two_way_opening_flows`,
`horizontal_opening_flows`, `compute_path_dp`, `post_solve`),
`species.rs`; solved in `heat_balance.rs::solve_timestep`, step 4.

With `simulation.airflow_network` enabled, infiltration and interzone flows
come from a pressure network that replaces @eq-infiltration. Each zone is
one node, at a uniform pressure $P_i$ and the zone's temperature and
density; the outdoors is the reference. The network is generated from the
geometry:

- each exterior wall, roof and window gets a power-law crack to the
  outdoor node at its centroid height;
- each door gets a crack scaled by its perimeter;
- each interzone surface gets a crack between its two zones.

Crack coefficients come from per-area defaults or from leakage classes A–D,
derived from ASHRAE unit leakage areas [@ashrae2021fund, Ch. 16]. With a
target ACH50 they are all rescaled so the envelope passes it at 50 Pa.
Operable windows and doorways may be large openings, two-way openings or
horizontal openings, with scheduled or natural-ventilation-controlled areas.
Exhaust fans and the net HVAC supply–return imbalance (from the previous
timestep) are fixed flows, and duct leaks are pressure-dependent cracks.

**Flow elements.** With $\Delta p$ the pressure difference across a path
and $\bar\rho$ the mean density of its two nodes:

$$
\dot m_\text{crack} = C\left(\frac{\bar\rho}{\rho_\text{ref}}\right)^{1-n}\operatorname{sgn}(\Delta p)|\Delta p|^n,
\qquad
\dot m_\text{open} = C_d A\operatorname{sgn}(\Delta p)\sqrt{2\bar\rho|\Delta p|},
$$ {#eq-afn-elements}

linearized through zero for $|\Delta p|$ below a small threshold.

- **Two-way openings** integrate the orifice equation in closed form above
  and below the neutral plane. Over the opening height, $\Delta p$ varies
  with slope $g(\rho_b - \rho_a)$, so warm air leaves through the top while
  cold air enters below.
- **Horizontal openings** add the buoyancy-driven exchange of Cooper and
  Epstein when the upper zone is denser [@cooper1989].

**Pressure difference across a path** at height $h$ between nodes $a$ and
$b$ (reference heights $h_a$, $h_b$):

$$
\Delta p = P_a - P_b - \rho_a g(h - h_a) + \rho_b g(h - h_b) \pm \tfrac12\rho_o C_p V(h)^2,
$$ {#eq-afn-dp}

with the wind term on paths to the outdoor node. $V(h)$ comes from @eq-wind-profile, times an optional facade shelter factor. The wind
pressure coefficient $C_p$ comes from one of three models:

- **Low-rise** (Swami and Chandra [@swami1988]), with incidence angle
  $\theta$ and $G = \ln(\text{side ratio})$:
  $$
  \begin{aligned}
  C_p = 0.6\ln\Big[&1.248 - 0.703\sin\tfrac\theta2 - 1.175\sin^2\theta + 0.131\sin^3(2\theta G)\\
  &+ 0.769\cos\tfrac\theta2 + 0.07G^2\sin^2\tfrac\theta2 + 0.717\cos^2\tfrac\theta2\Big];
  \end{aligned}
  $$
- **High-rise:** $0.6\cos\theta$ windward and $-0.3$ leeward;
- **User table** by facade.

**Solution.** The zone pressures solve mass conservation at every zone
node, $R_i = \sum_\text{paths}\dot m_{\to i} = 0$, by Newton–Raphson with a
dense Jacobian and adaptive under-relaxation. It starts from the stored pressures
and stops when the largest residual is below the
absolute tolerance or $10^{-4}$ of the total through-flow (at most 30
iterations). After solving, each zone's outdoor inflow becomes
$\dot m_\text{inf}$. Its inflows from other zones become a mass-weighted
stream mixed with the outdoor air (`mix_advective_streams`), giving the
combined advective term of @eq-zone-ode and its humidity. Passive species
(e.g. CO₂) are then advanced by an implicit zone mass balance on the solved
flows, with sources, decay, deposition and filtration.

**Comparison with EnergyPlus.** The network, element equations, the
two-way and horizontal openings, the Swami–Chandra $C_p$ and the
Newton–Raphson solution follow EnergyPlus's AirflowNetwork (AIRNET)
[@eplus-engref-25-2, "Airflow Network Model"]. They are generated
automatically rather than from AirflowNetwork input objects. The
differences ([#90](https://github.com/bbrannon4/OpenBSE/issues/90)):

- **Density.** Element flows use the mean of the two node densities, not
  the upstream density, and cracks have no viscosity correction.
- **High-rise $C_p$.** It is a simple cosine rather than the Akins
  surface-average table.
- **No duct network.** There is no AirflowNetwork:Distribution duct
  network; duct leakage is a pair of leak paths per duct.
- **Per-solve stepping.** Species and natural-ventilation state advance per
  envelope solve ([#188](https://github.com/bbrannon4/OpenBSE/issues/188)).

## Known limitations: infiltration, ventilation and AFN {#sec-infiltration-limitations}

- [#90](https://github.com/bbrannon4/OpenBSE/issues/90): AFN cleanups;
  tolerance units, density convention, high-rise $C_p$.
- [#188](https://github.com/bbrannon4/OpenBSE/issues/188): species and
  natural-ventilation hysteresis advance once per envelope solve.
- [#219](https://github.com/bbrannon4/OpenBSE/issues/219): wind-and-stack
  coefficients; scheduled ventilation lacks E+'s coefficients and schedule.

## Remaining envelope models {#sec-envelope-prereview}

::: {.callout-warning}
## Pre-review content

The sections below hold text moved unchanged, apart from format, from the former single-file reference (`README.md`, mostly written in March 2026). Parts of it are stale or incomplete. They will be rewritten from the code, and checked against EnergyPlus, in the later parts of this chapter's review ([#181](https://github.com/bbrannon4/OpenBSE/issues/181)); see @sec-status.
:::

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
- Ground contact: `T_surface_outside = T_ground(depth, day_of_year)` via the Kusuda-Achenbach model (@sec-ground-temps)
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
