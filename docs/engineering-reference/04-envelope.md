# Building envelope {#sec-envelope}

This chapter covers the building shell, in seven parts:

1. materials, constructions and conduction;
2. surface heat balances and convection;
3. long-wave exchange;
4. windows, shades and solar distribution;
5. infiltration, ventilation and the airflow network;
6. internal gains and ground heat transfer;
7. the zone air heat and moisture balances.

Each part was written from the code and compared with EnergyPlus.

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
- **Inside face.** The inside layer's solar absorptance, and its
  `thermal_absorptance_inside` if given, otherwise its thermal absorptance.

This matches EnergyPlus, except that `thermal_absorptance_inside` lets one
material list a different emissivity for use as an inside face; EnergyPlus
would need a second material. Three other construction types exist: simple
constructions (@sec-simple-constructions), F-factor floors (@sec-ffactor)
and window constructions (@sec-windows).

Conductivity, density and specific heat are required on every material
without `thermal_resistance`, and must be positive, as in EnergyPlus. A
construction with no layers, or with a layer naming an undefined material,
is a severe error.

## Conduction transfer functions {#sec-ctf}

**Source:** `openbse-envelope/src/ctf.rs`: `try_calculate_ctf`,
`build_network`, `build_state_space`, `matrix_exponential`,
`compute_ctf_from_state_space`, `add_face_storage`, `apply_ctf`, `CtfHistory`; coefficients built at initialization by
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

The past terms $t-j$ are the values $j$ CTF time steps back
(@sec-ctf-time-step), stored at the end of each zone timestep
(`CtfHistory`; see @sec-timestep-loop). The history starts at 21 °C with
zero flux.

### State-space formulation {#sec-ctf-state-space}

The construction is divided into nodes. The massed layers in the middle are
handled as follows:

- **Massed layers.** A massed layer of thickness $L$ and diffusivity
  $\alpha = k/(\rho c_p)$ gets
  $N = \min(\max(\lfloor L/\Delta x_n\rfloor, 10), 19)$ divisions of width
  $\Delta x = L/N$, with $\Delta x_n = \sqrt{2\alpha\,\Delta t_z}$ at the
  zone timestep $\Delta t_z$ (the explicit finite-difference stability
  spacing). Its $N + 1$ grid points include both of its faces.
- **Massless layers at either end** ($\rho c_p L < 1$ J/(m²·K), or NoMass) are
  a conductance $1/R$ between the surface and the face of the first or last
  massed layer.
- **Massless layers between massed ones** become one resistive division of
  conductance $1/R$ with no capacitance; adjacent ones are combined.

Each grid point carries the heat capacity of the material within
$\Delta x/2$ of it:

- $\rho c_p \Delta x$ inside a layer;
- $\tfrac12(\rho c_p\Delta x)_\ell + \tfrac12(\rho c_p\Delta x)_{\ell+1}$
  at a layer interface;
- $C_\text{f} = \tfrac12\rho c_p \Delta x$ at the outer face of the first and
  last massed layer.

This is the finite-volume network with nodes on the faces, whose spatial
error is second order in $\Delta x$. The total capacitance is conserved. The
grid points are the states $\theta$, except a face that is itself the
surface: its temperature is a CTF input, and its half cell enters as a face
storage term (below). Behind a NoMass resistance the face is a state like
any other. With the surface temperatures
$u = (T_\text{so}, T_\text{si})^\mathsf{T}$ as inputs and the surface fluxes
$y = (q''_\text{ko}, q''_\text{ki})^\mathsf{T}$ as outputs:

$$
\dot\theta = A\theta + Bu, \qquad y = C\theta + Du,
$$ {#eq-state-space}

where $A$ is tridiagonal (nodal conductances over capacitances), and $B$, $C$
and $D$ couple the first and last state to the surfaces through the
conductance between them, $k/\Delta x$ or $1/R$.

Assuming the face temperatures vary linearly within a CTF time step
$\Delta t$, the exact discrete solution uses

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
$|e_j|/|e_1| < 10^{-13}$, or at $j = n$, the number of nodes, where
$R_n = 0$ by the Cayley–Hamilton theorem and the series is exact.

**Face storage.** A face at the surface has the surface temperature, which
varies linearly within each step, so its half cell stores
$C_\text{f}(T^t - T^{t-1})/\Delta t$ per unit time over the step, exactly. The
term adds to $q''_\text{ko}$ at the outside face and subtracts from
$q''_\text{ki}$ at the inside face. Because the flux histories hold the total
flux, it is multiplied by the flux polynomial
$\Phi(z) = 1 - \sum_{j\ge1}\Phi_j z^{-j}$:

$$
X_j \mathrel{+}= \frac{C_\text{f,o}}{\Delta t}\,d_j,\qquad
Z_j \mathrel{+}= \frac{C_\text{f,i}}{\Delta t}\,d_j,\qquad
\sum_j d_j z^{-j} = (1 - z^{-1})\,\Phi(z).
$$ {#eq-ctf-face-storage}

The series gains one term (with $\Phi_j = 0$ there). Since $\sum_j d_j = 0$,
the steady-state transmittance is unchanged.

### CTF time step {#sec-ctf-time-step}

The CTF time step $\Delta t$ is the zone timestep for almost every
construction. A layer at the 19-division cap is coarser than its stability
spacing, so the CTF time step grows to that layer's Fourier-number-one step,
rounded to the nearest whole number $N$ of zone timesteps:

$$
\Delta t_n = \max_{\ell:\,N_\ell = 19} \frac{\rho_\ell c_{p,\ell}\,\Delta x_\ell^2}{k_\ell},
\qquad N = \left\lfloor \frac{\Delta t_n}{\Delta t_z} + \tfrac12 \right\rfloor,
\qquad \Delta t = N\,\Delta t_z .
$$ {#eq-ctf-time-step}

A capped layer always gives $\Delta t_n \ge 2\Delta t_z$, so $N \ge 2$ there.
The node network stays as set at the zone timestep. The series is then
rejected, $N$ grows by one and the series is recomputed while any of these
holds:

- it needs more than 18 history terms;
- its sums disagree, $\max(|\sum X - \sum Y|, |\sum Z - \sum Y|) > 0.01
  \max(\sum X, \sum Y, \sum Z)$ (in theory they are equal);
- its steady-state transmittance
  $U_\text{CTF} = \sum Z_j / (1 - \sum\Phi_j)$ is more than 5% from
  $1/\sum R_\ell$.

A CTF time step reaching 7 h is an error naming the construction and the
layer with the longest node time constant. A single layer of soil or
concrete thicker than about 2 to 2.5 m reaches it, and so do several
metre-thick ones together. An
accepted series whose $U_\text{CTF}$ is more than 1% off is reported as a
warning.

**Staggered histories.** With $N > 1$, every zone timestep's face
temperatures and fluxes are kept for $N$ times the series length. The zone
timestep $t$ reads the exact values at $t - N\Delta t_z$, $t - 2N\Delta t_z$,
and so on. The zone timesteps fall into $N$ interleaved history sets, each a
CTF recursion at $\Delta t$ in its own right. The surface balance therefore
updates every zone timestep, and each set conserves energy on its own.

**Special cases.**

- With total capacitance below 1 J/(m²·K), the CTF is steady state:
  $X_0 = Y_0 = Z_0 = U$, with no history.
- A simple construction with $\tau < 10\,\Delta t$
  (@sec-simple-constructions) is a lumped RC element:
  $X_0 = Y_0 = Z_0 = U(1 - e^{-\Delta t/\tau})$ and
  $\Phi_1 = e^{-\Delta t/\tau}$, with $\tau = RC$.

**Comparison with EnergyPlus.** The method, the stability spacing and
19-division cap, the interface capacitances, the recurrence, the CTF time
step of @eq-ctf-time-step, its escalation on more than 18 terms or a sum
mismatch above 1%, and the 7 h ceiling (`MaxAllowedTimeStep`) follow
`Construction::calculateTransferFunction`
[@eplus-engref-25-2, "Conduction Transfer Function (CTF) Calculations in EnergyPlus"]. The
differences are:

- **Node network.** EnergyPlus uses at least 6 divisions (`MinNodes`) and
  puts each surface half cell into the first node inside it
  ($1.5\,\rho c_p\Delta x$, a full $\Delta x$ from the surface), a first-order
  error in $\Delta x$. OpenBSE uses at least 10 divisions and keeps the half
  cell on the surface. Against the exact solutions, with the outside surface
  driven and the inside surface held, at a 10 min step (largest error over
  the last day or the whole response, as a percentage of each flux's
  amplitude or largest value, inside / outside):

  | Wall | Daily cycle, OpenBSE | Daily cycle, EnergyPlus network | Step, OpenBSE | Step, EnergyPlus network |
  |---|---|---|---|---|
  | Light (ASHRAE 140 LTWALL) | 0.02 / 0.73 | 0.07 / 0.73 | 0.22 / 0.02 | 0.89 / 0.11 |
  | 10 cm concrete | 0.18 / 0.24 | 1.92 / 2.02 | 1.52 / 1.50 | 7.09 / 24.2 |
  | 20 cm concrete | 0.78 / 0.39 | 7.21 / 8.45 | 1.67 / 6.19 | 7.31 / 51.4 |
  | Insulated (ASHRAE 140 HWWALL) | 0.15 / 0.71 | 2.28 / 0.76 | 0.27 / 0.03 | 1.72 / 0.10 |
  | NoMass faces, 15 cm concrete | 0.07 / 0.01 | 3.95 / 0.21 | 0.03 / 0.06 | 1.14 / 0.55 |

  The daily cycle is compared with the transmission-matrix solution; the
  step (linear over the first zone step, then held) with its exact response,
  inverted from the same matrices by the fixed Talbot contour. That step
  comparison isolates the network: the CTF is exact for linear inputs. The
  light wall's outside error at the daily cycle is the time step's phase
  error, the same for both (6% at 1 h, 1.3% at 15 min). The steady-state U
  stays exact (within $10^{-8}$ at steps of 5 min and longer), and the
  heat the step lets in matches the final profile's stored heat within
  $10^{-6}$. The finer network adds about one CTF term on average; the
  shipped models' run times change by less than their run-to-run spread.

- **Multi-step histories.** EnergyPlus (`UpdateThermalHistories`) keeps one
  master history at the start of each block of $N$ zone timesteps and
  interpolates the working histories linearly between block starts. OpenBSE
  reads the exact past values. For 60 cm of concrete at 10 min with its outside
  face stepped 10 K, the master-history interpolation stores 1.8% less heat
  than the step delivers (3.9% at 1 min); the exact histories close to 0.01%,
  as a single-step CTF does. Their periodic accuracy is the same or slightly
  better (daily cycle, inside flux 6.0% against 6.1% of its amplitude, both
  on EnergyPlus's node network).
- **Steady-state check.** OpenBSE also escalates when $U_\text{CTF}$ is more
  than 5% off the layers' U, and warns above 1%.
- **Massless end layers.** EnergyPlus gives a NoMass first or last layer the
  properties of air and nodes of its own; OpenBSE makes it a conductance
  between the surface and the massed layer's face node
  (@sec-ctf-state-space).
- **Layer thickness.** EnergyPlus rejects any layer over 3 m; OpenBSE has no
  thickness limit of its own, and a massive layer that thick reaches the 7 h
  ceiling.

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

An F-factor floor of area $A$ and exposed perimeter $P$ [m], with $F$ the
F-factor [W/(m·K)], is two layers. The F-factor includes the air films, so they
are removed to leave the floor's resistance

$$
R_\text{eff} = \frac{A}{F P} - R_\text{film,in} - R_\text{film,out},
\qquad R_\text{film,in} = 0.135,\quad R_\text{film,out} = 0.03\ \text{m²·K/W},
$$ {#eq-ffactor}

or 177 m²·K/W when $P = 0$. Inside, a fixed 15 cm of concrete ($k = 1.95$
W/(m·K), $\rho = 2240$ kg/m³, $c_p = 900$ J/(kg·K)) gives the floor its mass. Outside it, a no-mass layer of resistance
$R_\text{eff} - 0.15/k$ makes up the rest. A floor too conductive to hold the concrete is
a severe error. When $P$ is missing it defaults to $2\sqrt{A}$, with a warning. The
outside face is held at the construction's monthly ground temperatures
(constant within each month), or at the building ground temperatures
(@sec-ground-temps).

**Comparison with EnergyPlus.** This is `Construction:FfactorGroundFloor`
as built by `HeatBalanceManager::CreateFCfactorConstructions`. C-factor walls
(`Construction:CfactorUndergroundWall`) don't exist.

## Heat and moisture transfer (HAMT) {#sec-hamt}

**Source:** `openbse-envelope/src/hamt.rs`.

The module implements a one-dimensional coupled heat and moisture solver
after Künzel [@kunzel1995]. It is **not connected to the simulation**:
nothing calls it, and every surface uses CTF conduction. Its material inputs
(`vapor_resistance_factor`, `sorption_isotherm`, `liquid_transport_coeff`)
are accepted but have no effect, and validation warns when they are given. This
section will be written when the model is wired in. It simplifies EnergyPlus's
HAMT in ways that will need settling then: dry-material thermal conductivity,
vapor driven by the moisture-content gradient only, uniform node spacing and
fixed surface films.

## Known limitations: materials and constructions {#sec-constructions-limitations}

- [#53](https://github.com/bbrannon4/OpenBSE/issues/53): simple
  constructions use synthetic layers tuned to ASHRAE 140, and do not always
  conserve the specified heat capacity.
- HAMT is not wired in (@sec-hamt).

## Surface heat balances {#sec-surface-hb}

**Source:** `heat_balance.rs::solve_timestep`, step 6 (6a outside faces,
6b inside faces).

Each opaque surface has an outside and an inside face temperature, linked by
the conduction of @eq-ctf. Windows have their own balance (@sec-window-hb).
Each envelope solve repeats the following steps until, after at least two passes,
no zone air temperature changes by more than 10⁻⁴ K, no zone humidity ratio by
more than 10⁻⁷ kg/kg and no inside face by more than 2×10⁻⁴ K, or until 20
passes. A solve still changing by more than 0.001 K after 20 is counted and
reported as a warning (@sec-diagnostics). The limit is a guard only: the HVAC
iteration reads a load in which the faces follow the air (@eq-load-line), so it
stays stable however far the passes settle.

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
  Surfaces pair one-to-one: by an explicit `adjacent_surface`, else by
  coincident vertices, else by equal area (within 2%) and opposite orientation.
  A surface with no partner, two surfaces claiming one partner, a tie between
  different constructions, or paired faces whose areas differ by more than 2%
  is a severe error. A surface that names its own zone is adiabatic.

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
- **Interzone pairing.** EnergyPlus pairs interzone surfaces by name or by
  generating the reverse surface. OpenBSE also pairs them by geometry, so the
  name is optional. EnergyPlus only warns about areas that differ by more than
  2%; OpenBSE rejects them, since that heat can't be conserved. Both warn when
  the two constructions aren't mirror images.

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
stratified zone, the local air at the surface's height; @sec-stratification). **Window inside faces** use the ISO 15099 §8.3.2.2 indoor-surface correlation,
as EnergyPlus does whatever the zone algorithm
(`window_interior_convection`, E+ `CalcISO15099WindowIntConvCoeff`). With
$\gamma$ the tilt, complemented when the glass is warmer than the air:

- $0 \le \gamma < 15°$: $Nu = 0.13\,Ra^{1/3}$;
- $15° \le \gamma \le 90°$: $Nu = 0.56(Ra\sin\gamma)^{1/4}$ up to
  $Ra_{cv} = 2.5\times10^5(e^{0.72\gamma}/\sin\gamma)^{1/5}$, then
  $0.13(Ra^{1/3} - Ra_{cv}^{1/3}) + 0.56(Ra_{cv}\sin\gamma)^{1/4}$;
- $90° < \gamma \le 179°$: $Nu = 0.56(Ra\sin\gamma)^{1/4}$, with
  $Ra\sin\gamma$ bounded to $10^5$–$10^{11}$;
- $179° < \gamma \le 180°$: $Nu = 0.58\,Ra^{1/5}$, with $Ra \le 10^{11}$.

Then $h = Nu\,\lambda/H$, with $H$ the window height, and $Ra$ uses moist-air
density at the site pressure and ISO Table B.1 and B.2 properties at the film
temperature $T_z + (T_s - T_z)/4$. The height is the input value, else from the
vertices, else $\sqrt{A}$.

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
[@eplus-engref-25-2, "Outdoor/Exterior Convection", "Interior Convection"].
EnergyPlus's other
algorithms (adaptive convection, ASHRAE simple, MoWiTT alone) are not
available.

## Known limitations: surface heat balances {#sec-surface-hb-limitations}

- [#209](https://github.com/bbrannon4/OpenBSE/issues/209): no wet surfaces in rain.

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
(@sec-zone-equipment), is shared among the zone's surfaces in proportion to
area times room-facing emissivity, windows included
(`zone_radiant_gain_flux`):

$$
q''_{r,i} = \frac{\dot Q_\text{rad}\,\varepsilon_i}{\sum_k A_k\varepsilon_k},
$$ {#eq-radiant-gain-flux}

matching EnergyPlus's `ComputeIntThermalAbsorpFactors` (ITABSF). A deployed
shade counts with its own emissivity, and it absorbs the window's share.

**Comparison with EnergyPlus.** EnergyPlus uses the approximate factors of
@eq-approx-vf by default; polygon factors require user input
(`ZoneProperty:UserViewFactors`). OpenBSE computes them from the geometry,
a deliberate improvement from PR #161. `FixViewFactors`, the ScriptF
interchange (`CalcScriptF`) and the exchange (`CalcInteriorRadExchange`)
match EnergyPlus [@eplus-engref-25-2, "Internal Long-Wave Radiation Exchange"]. OpenBSE's
approximate rule also treats azimuths 350° and 10° as within 10°, which
EnergyPlus does not.

## Known limitations: long-wave exchange {#sec-lw-limitations}

- [#52](https://github.com/bbrannon4/OpenBSE/issues/52): superseded by the
  polygon view factors. The remaining limitation is that obstruction by third
  surfaces is ignored in non-convex zones.

## Windows {#sec-windows}

**Source:** `glazing.rs` (gases, gap Nusselt number, layer and system optics,
simple glazing equivalent layer, face heat balance, nominal performance);
`material.rs::WindowConstruction::glazing`; `heat_balance.rs::solve_timestep`
(step 5b, window solar; step 5c, reflected diffuse on windows; step 6,
window loop), `solve_window`, `shade_solar`, `window_glazings`;
`openbse-cli/src/main.rs::evaluate_blind_control`.

A window is a glazing system of one or more glass layers with a gas gap
between each pair. Its area is subtracted from its parent surface. A
window construction describes the glazing in one of two ways:

- **Simple glazing** (default): the NFRC U-factor $U$ (films included) and
  the normal-incidence $SHGC$. The glazing is EnergyPlus's equivalent single
  layer for those ratings (@sec-window-sgs).
- **Layered glazing** (`layers`): one to four glass layers, each a glazing
  material with its thickness, conductivity, normal-incidence solar (and
  optionally visible) transmittance and front and back reflectance, front and
  back emissivity and dirt factor (after EnergyPlus `WindowMaterial:Glazing`,
  spectral average), and between each pair a gas gap of given width filled
  with air, argon, krypton, xenon or a mixture of them by mole fraction
  (`WindowMaterial:Gas`, `WindowMaterial:GasMixture`). The window's U-factor
  and SHGC follow from the layers. Giving both forms, a malformed layer
  sequence, an unknown glazing material, mixture fractions that do not add
  up to 1, or a non-zero glass long-wave transmittance is a severe error.

A construction may also carry a shading control (@sec-shades).

### Simple glazing equivalent layer {#sec-window-sgs}

The equivalent layer follows EnergyPlus `MaterialGlass::SetupSimpleWindowGlazingSystem`
[@arasteh2009]. Its resistance is the U-factor less EnergyPlus's winter films,
which depend on $U$:

$$
R_l = \frac1U - R_{i,w} - R_{o,w},\qquad
R_{i,w} = \frac{1}{0.359073\ln U + 6.949915},\qquad
R_{o,w} = \frac{1}{0.025342\,U + 29.163853},
$$ {#eq-sgs-winter-films}

with $R_{i,w} = 1/(1.788041\,U - 2.886625)$ for $U \ge 5.85$ and $R_l$ at
least 0.001 m²·K/W. Its thickness is 2 mm when $1/R_l > 7$ W/(m²·K) and
$0.05914 - 0.00714/R_l$ m otherwise, and its conductivity is $L/R_l$. The
normal-incidence solar transmittance comes from U-dependent correlations, for
example for low U:

$$
\tau_\text{sol} =
\begin{cases}
0.41040\,SHGC & SHGC \le 0.15,\\
0.085775\,SHGC^2 + 0.963954\,SHGC - 0.084958 & \text{otherwise},
\end{cases}
$$ {#eq-sgs-tsol}

with the high-U form for $U > 4.5$ and linear interpolation between 3.4 and
4.5 W/(m²·K). The solar reflectance (the same on both faces) uses solar-condition
films, cubic and linear in $\Delta = SHGC - \tau_\text{sol}$; for $U < 3.4$, for
example:

$$
R_{i,s} = \frac{1}{199.8208\Delta^3 - 90.6397\Delta^2 + 19.7371\Delta + 6.7666},\qquad
R_{o,s} = \frac{1}{5.7634\Delta + 20.5415},
$$ {#eq-sgs-solar-films}

with a second set for $U > 4.5$ and EnergyPlus's interpolation between. Then

$$
N = \frac{R_l/2 + R_{o,s}}{R_l + R_{o,s} + R_{i,s}},\qquad
\rho_\text{sol} = 1 - \tau_\text{sol} - \frac{SHGC - \tau_\text{sol}}{N}.
$$ {#eq-sgs-films}

Both faces have emissivity 0.84. The layer's angular behaviour uses
EnergyPlus's curve set A–J for the window's U/SHGC cell (@sec-window-optics).

The equivalent layer solved with the films of @sec-window-hb at NFRC winter
conditions does not in general give back $U$, because those films are not the
correlations of @eq-sgs-winter-films. As in EnergyPlus (`CalcNominalWindowCond`,
`CoeffAdjRatio`), a film factor $f_h \in [1, 2]$ is found by bisection that
makes the nominal U-factor (@sec-window-nominal) match $U$ within
0.01 W/(m²·K), and the window's inside and outside convection coefficients
are multiplied by it through the simulation.

### Solar optics {#sec-window-optics}

The optics follow EnergyPlus `WindowManager::InitGlassOpticalCalculations` for
layers with spectral-average properties. Each layer's transmittance and
front and back reflectance are found at incidence angles 0°, 10°, …, 90° from
their normal-incidence values $\tau_0$, $\rho_{f,0}$, $\rho_{b,0}$
(`TransAndReflAtPhi`) [@ashrae2021fund]:

- **Uncoated glass** ($\rho_{f,0} = \rho_{b,0}$ within 0.1 %): the refractive
  index $n$ and absorption $\alpha d$ that reproduce $\tau_0$ and $\rho_0$ are
  found, and each polarisation $p$ has the Fresnel interface reflectance
  $r_p(\theta)$ and the internal transmittance $e^{-\alpha d/\cos\theta'}$, with
  $\cos\theta' = \sqrt{1 - \sin^2\theta/n^2}$:
  $$
  \tau_p = \frac{(1 - r_p)^2 e^{-\alpha d/\cos\theta'}}{1 - r_p^2 e^{-2\alpha d/\cos\theta'}},\qquad
  \rho_p = r_p\left(1 + \tau_p e^{-\alpha d/\cos\theta'}\right),
  $$ {#eq-fresnel-layer}
  and the layer values are the means of the two polarisations.
- **Coated glass**: normalised clear-glass ($\tau_0 > 0.645$) or bronze-glass
  angular distributions scale $\tau_0$ and blend $\rho_0$ toward 1.
- **Simple glazing**: $\tau(\theta) = \tau_\text{sol}P_T(\cos\theta)$ and
  $\rho(\theta) = \rho_\text{sol}(1 - P_R) + P_R$, where $P_T$ and $P_R$ are
  EnergyPlus's fourth-order curves A–J for the window's U/SHGC cell, interpolated
  between cells as EnergyPlus does.

The layers are combined with their inter-reflections at each angle
(`SystemPropertiesAtLambdaAndPhi`). Adding layer $j$ to the stack of layers
$i \ldots j-1$:

$$
\tau_{i..j} = \frac{\tau_{i..j-1}\,\tau_j}{1 - \rho_{f,j}\,\rho_{b,i..j-1}},\qquad
\rho_{f,i..j} = \rho_{f,i..j-1} + \frac{\tau_{i..j-1}^2\,\rho_{f,j}}{1 - \rho_{f,j}\,\rho_{b,i..j-1}},
$$ {#eq-layer-stack}

and the absorptance of layer $j$ in the system is

$$
A_j = \frac{\tau_{1..j-1}\,\alpha_{f,j}}{1 - \rho_{f,j..n}\,\rho_{b,1..j-1}}
+ \frac{\tau_{1..j}\,\rho_{f,j+1..n}\,\alpha_{b,j}}{1 - \rho_{b,1..j}\,\rho_{f,j+1..n}},
$$ {#eq-layer-abs}

with $\alpha = 1 - \tau - \rho$ for the isolated layer. The same calculation
with the layers reversed gives the back (room-side) absorptances. The system
transmittance and each layer's absorptance are fitted as
$\sum_{k=1}^{6} c_k\cos^k\theta$ by least squares over the ten angles
(`W5LsqFit`), and hemispherical values are
$\int_0^{\pi/2} X(\theta)\sin 2\theta\,d\theta$ by the trapezoidal rule over the
same angles (`DiffuseAverage`). A dirt factor scales the outermost layer's
transmittance.

Beam irradiance $I_b$ uses the fits at its incidence angle; all diffuse
irradiance $I_d$ (sky, circumsolar, horizon and ground) uses the hemispherical
values. Per unit window area the room receives $\tau(\theta)I_b + \tau_dI_d$,
which the zone's surfaces absorb (@sec-interior-solar), and glass layer $j$
absorbs

$$
S_j = A_j(\theta)\,I_b + A_{j,d}\,I_d + S_{j,\text{room}},
$$ {#eq-layer-solar}

where $S_{j,\text{room}}$ is its share of the room's reflected diffuse
(@sec-interior-solar). The SHGC is not an input to this split: the part of
$S_j$ that reaches the room follows from the heat balance.

### Window heat balance {#sec-window-hb}

The two faces of each glass layer have their own temperature, faces
$1 \ldots 2n$ from outdoors to the room, and all are solved together
(EnergyPlus `WindowManager::SolveForWindowTemperatures`). Each layer's absorbed
solar is split equally between its faces. With $k_j/L_j$ the layer's
conductance and $\theta$ face temperatures [K]:

- **Outer face.** Convection and long-wave exchange with the outdoors,
  $$
  h_{c,o}f_h(T_o - \theta_1) + \varepsilon_1\left(E_o - \sigma\theta_1^4\right)
  + \frac{k_1}{L_1}(\theta_2 - \theta_1) + \frac{S_1}{2} = 0,
  $$ {#eq-window-outer}
  where $h_{c,o}$ is the outside convection (@sec-doe2) and $E_o$ the
  long-wave irradiance from sky, air and ground with the view factors and the
  sky/air split of @eq-ext-hrad.
- **Faces across a gap** $g$ between faces $a$ and $b$,
  $$
  q_g = h_g(\theta_a - \theta_b) + \frac{\varepsilon_a\varepsilon_b\,\sigma\left(\theta_a^4 - \theta_b^4\right)}{1 - (1 - \varepsilon_a)(1 - \varepsilon_b)},
  $$ {#eq-window-gap}
  leaves face $a$ and reaches face $b$, which also exchange conduction with
  the other face of their layer and receive their halves of its absorbed
  solar.
- **Inner face.** Convection to the zone air, linearised long-wave exchange
  with the zone's surfaces (@eq-lw-linear) and the window's share of the
  radiant internal gains $q''_{r,\text{int}}$:
  $$
  \frac{k_n}{L_n}(\theta_{2n-1} - \theta_{2n}) + \frac{S_n}{2} + h_{c,i}f_h(T_z - \theta_{2n})
  + h_{r,i}(T_\text{mrt} - \theta_{2n}) + q''_{r,\text{int}} = 0,
  $$ {#eq-window-inner}
  with $h_{c,i}$ the ISO 15099 window inside convection (@sec-tarp), at least
  0.1 W/(m²·K) as EnergyPlus's `LowHConvLimit`.

The gap conductance is $h_g = Nu\,k/w$ for gap width $w$. The gas
properties at the mean gap temperature follow ISO 15099 Annex B
[@iso15099]: conductivity, viscosity and specific heat are quadratics in
temperature, and the density is the ideal gas at $10^5$ Pa. Mixtures use
ISO 15099's rules (eqs. 56–68) as EnergyPlus codes them
(`WindowGasConductance`). The Nusselt number is ISO 15099 §5.3.3.3
(`NusseltNumber`), with $Ra = Gr\,Pr$ on the gap width and $A = H/w$ for
window height $H$:

$$
Nu_{90} = \max\left(Nu_1,\; 0.242\left(\frac{Ra}{A}\right)^{0.272}\right),\qquad
Nu_1 =
\begin{cases}
1 + 1.7596678\times10^{-10}Ra^{2.2984755} & Ra \le 10^4,\\
0.028154\,Ra^{0.4134} & Ra \le 5\times10^4,\\
0.0673838\,Ra^{1/3} & \text{otherwise}.
\end{cases}
$$ {#eq-gap-nu90}

A cavity heated from above has $Nu = 1 + (Nu_{90} - 1)\sin\gamma$ at tilt
$\gamma$. Heated from below, $Nu$ interpolates linearly between $Nu_{60}$
(ISO 15099 eqs. 45–47) at 60° and $Nu_{90}$ at 90°, and below 60° it is
Hollands et al.'s correlation [@hollands1976]:

$$
Nu = 1 + 1.44\left[1 - \frac{1708}{Ra\cos\gamma}\right]^+\left(1 - \frac{1708\,(\sin 1.8\gamma)^{1.6}}{Ra\cos\gamma}\right)
+ \left[\left(\frac{Ra\cos\gamma}{5830}\right)^{1/3} - 1\right]^+.
$$ {#eq-gap-hollands}

For a window facing upward the outer face is the cavity's upper face; for one
facing downward ($\gamma > 90°$) the inner face is, and $180° - \gamma$ is
used.

Each iteration evaluates the gap conductances, the inside convection and the
long-wave coefficients at the current temperatures ($\sigma\theta^4$ is
written $(\sigma\theta^3)\,\theta$), solves the linear system, and moves the
temperatures toward the solution: fully while the steps contract, by half
once a step grows, and by a quarter after 25 iterations. It stops when the
largest step is below $10^{-5}$ K, within 100 iterations; a solve that
reaches the limit is a severe run-time error (@sec-run-checks). Each window
is solved once per surface–air pass against the zone air and the other
surfaces' temperatures from the previous pass, starting from its own faces
from the previous solve, and the passes repeat until surfaces and air have
settled (@sec-surface-hb).

The room sees the inner face: its convection enters the zone air balance
through $h_{c,i}f_hA(\theta_{2n} - T_z)$, and its long-wave exchange the
interior exchange of @sec-lw-exchange. The outer face temperature is the
window's outside surface temperature, and the conduction reported for a
window is through its inner glass layer.

### Nominal performance {#sec-window-nominal}

A glazing's nominal U-factor and SHGC are those of EnergyPlus
`CalcNominalWindowCond`: centre of glass, vertical, 1 m tall for the inside
convection and 1.524 m for the gap aspect ratio, sky at the outdoor
temperature. Winter: $T_z = 21$ °C, $T_o = -18$ °C, $h_{c,o} = 26$ W/(m²·K).
Summer: $T_z = 24$ °C, $T_o = 32$ °C, $h_{c,o} = 15$ W/(m²·K) and 783 W/m² of
beam at normal incidence. The inside convection is ISO 15099's vertical-plate
correlation for dry air. With the face temperatures solved,

$$
U = \frac{1}{R_\text{tot}},\qquad
SHGC = \tau(0) + \sum_j A_j(0)\,\frac{R_{o,j}}{R_\text{tot}},
$$ {#eq-window-nominal}

where $R_{o,j}$ is the resistance from the outdoor air to the middle of layer
$j$ and the films and gaps use their combined radiative and convective
coefficients. A layered glazing reports these values as its U-factor and SHGC.

### Interior shades and blinds {#sec-shades}

A window construction may carry a shading control that deploys a shade or
blind. It deploys when its signal exceeds the trigger and retracts when the
signal falls below the trigger minus the hysteresis. The signal is either:

- the window's incident irradiance per unit area from the previous
  timestep; or
- the zone air temperature at the start of the timestep.

A deployed device is modelled in one of two ways:

- **Assembly rating** (`blind_shgc`, `blind_u_factor`, which defaults to the
  window's U-factor). The window+blind assembly is a simple glazing system of
  its own (@sec-window-sgs), which replaces the window's glazing in the optics
  and the heat balance while the blind is down, as a shaded construction does
  in EnergyPlus.
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
  where $\rho_g$ and $\alpha_g$ are the glazing's hemispherical room-side
  reflectance and absorptance; the glass's share goes to each layer in
  proportion to its room-side absorptance, and the rest, $\rho_{sh}I_{sh}\tau_g$,
  leaves through the glass. The shade and the air in the gap behind it are two
  more nodes, solved with the glass faces: the inner glass face exchanges
  with them in place of @eq-window-inner,
  $$
  \begin{aligned}
  \text{glass:}\quad & \tfrac{k_n}{L_n}(\theta_{2n-1} - \theta_{2n}) + \tfrac{S_n}{2} = h_{r,gs}(\theta_{2n} - T_{sh}) + h_{cv}(\theta_{2n} - T_a),\\
  \text{shade:}\quad & q''_{sh} = h_{r,gs}(T_{sh} - \theta_{2n}) + h_{cv}(T_{sh} - T_a) + h_{cs}(T_{sh} - T_z) + h_{r,i}(T_{sh} - T_\text{mrt}),\\
  \text{gap air:}\quad & h_{cv}(\theta_{2n} - T_a) + h_{cv}(T_{sh} - T_a) = h_\text{vent}(T_a - T_z),
  \end{aligned}
  $$ {#eq-shade-nodes}
  with:
  - $q''_{sh}$ the shade's absorbed solar and the window's share of the radiant
    internal gains;
  - $h_{r,gs} = 4\sigma\bar T^3/(1/\varepsilon_g + 1/\varepsilon_{sh} - 1)$;
  - gap convection $h_{cv} = 2h_c + 4v$ and ventilation
    $h_\text{vent} = \rho c_p v d/H$;
  - the buoyant gap velocity $v = 0.7\sqrt{gH|T_a - T_z|/T_z}$ [@iso15099];
  - $h_{cs}$ the window inside convection at the shade.

  The discharge coefficient 0.7 was calibrated against EnergyPlus interior
  shade runs (#144). The room sees the shade as the window's inside face,
  and the gap air returned to the room is a convective gain.

**Comparison with EnergyPlus.**

- **Equivalent layer and film factor.** The thickness, conductivity, solar
  transmittance and reflectance of the equivalent layer, the film factor, and
  the nominal SHGC match EnergyPlus 25.2's eio output for six U/SHGC pairs
  within rounding (`glazing.rs` tests).
- **Layer optics and heat balance.** For one to four layers of clear and low-e
  glass with air, argon, krypton, xenon and an argon/air mixture, the nominal
  U-factor, SHGC and solar and visible transmittance match EnergyPlus 25.2's
  within 0.0025. Against WINDOW 7, which integrates the measured spectra, the
  spectral-average optics agree within 0.006 of incident solar (ASHRAE 140
  glazings ID 15 and 23).
- **ASHRAE 140.** Case 660's window is the layered low-e argon glazing of
  the EnergyPlus reference model, not a U-factor and SHGC: the U-factor of
  1.19 W/(m²·K) quoted for it is WINDOW 7's at the standard's test films,
  while the same layers have an NFRC U-factor of 1.38 W/(m²·K).
- **Deviations, all with the same converged solution or a closer one:**
  - EnergyPlus stops at a mean face change of 0.02 K; this solve stops at
    $10^{-5}$ K, so the window settles within the surface–air passes.
  - EnergyPlus under-relaxes every step by half; this solve takes full steps
    until a step grows, which reaches the same fixed point in fewer iterations.
  - EnergyPlus treats the outer face as the cavity's upper face at any tilt;
    for windows facing downward the inner face is used here.
  - EnergyPlus solves windows inside its inside-surface iteration, with
    long-wave written as absorbed irradiance less emission. Here they are
    solved once per surface–air pass with linearised interior long-wave,
    and the passes repeat to the same converged state.
  - In the film-factor bisection EnergyPlus solves the faces with the
    previous step's outside coefficient; here each step uses its own. The
    U-factors differ by less than the 0.01 W/(m²·K) tolerance.
- **Shades.** The shade layer follows `WindowMaterial:Shade` with an
  ISO 15099 vented gap, simplified to fixed air properties and one gap
  velocity.
- **Shading control.** It corresponds to `WindowShadingControl` with the
  `OnIfHighSolarOnWindow` and `OnIfHighZoneAirTemperature` triggers. The
  hysteresis is an addition.

## Known limitations: windows {#sec-windows-limitations}

- Frames and dividers, and EnergyPlus's edge-of-glass correction, are not
  modelled; the whole window area is centre of glass.
- Layer data is spectral-average; spectral data sets are not read.
- Glass is opaque to long-wave radiation, as EnergyPlus assumes in the
  bare-glazing balance.
- A deployed shade does not change the window's room-side absorptance and
  transmittance for the room's reflected diffuse.
- Gases are limited to air, argon, krypton and xenon with ISO 15099's
  coefficients; EnergyPlus's custom gas is not available.

## Interior solar distribution {#sec-interior-solar}

**Source:** `heat_balance.rs::solve_timestep`, step 5c; precomputed per
surface and applied as $q''_\text{r}$ in @eq-inside-temp.

The beam and diffuse solar transmitted by a zone's windows (@sec-window-optics)
is distributed in three steps.

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
3. **Diffuse absorbed.** The diffuse pool, transmitted diffuse plus the
   reflected beam, $S_d = S_\text{d,win} + S_b - \sum_j\alpha_jS_{b,j}$, is
   shared over all the zone's surfaces in proportion to $A_k\alpha_k$
   (EnergyPlus's VMULT), windows included with
   $\alpha_w = \tau_{d,b} + \sum_j A_{j,d,b}$, their hemispherical room-side
   transmittance and layer absorptances (@sec-window-optics):
   $$
   Q_{\text{sol},k} = \alpha_k S_{b,k} + \frac{A_k\alpha_k}{\sum_m A_m\alpha_m}\,S_d.
   $$ {#eq-vmult}
   A window's share transmitted back out leaves the zone; the share each of
   its layers absorbs is that layer's $S_{j,\text{room}}$ in @eq-layer-solar.

**Comparison with EnergyPlus.** FullExterior, the absorbed and reflected
beam, the VMULT distribution and the back-out follow
`HeatBalanceSurfaceManager::ComputeIntSWAbsorpFactors` and
`InitIntSolarDistribution` [@eplus-engref-25-2, "Solar Distribution"]. The
differences ([#68](https://github.com/bbrannon4/OpenBSE/issues/68)):

- **Fallback.** Zones that lack vertices fall back to fixed fractions or
  to the air, rather than to FullExterior.
- **Beam projection.** The projection uses the whole window, not its
  sunlit part, and has no first-hit test for non-convex zones.

## External shading {#sec-external-shading}

**Source:** `openbse-envelope/src/shading.rs` (`calculate_sunlit_fraction`,
`compute_diffuse_sky_shading_ratio`, `compute_diffuse_horizon_shading_ratio`,
`generate_overhang_vertices`, `generate_fin_vertices`);
`heat_balance.rs::precompute_solar`, `compute_diffuse_shading_ratios`, step 5a.

`simulation.shading_calculation` defaults to `detailed`, as EnergyPlus always
computes shadows (below). The explicit opt-out, `basic`, makes every outdoor
surface fully sunlit with no diffuse shaded, and warns if the model has
overhangs, fins or shading surfaces. A surface without vertices has no shape
to shade, so it stays fully sunlit and casts nothing. In `detailed`:

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
single caster of transmittance $\tau$, $f_s = 1 - (1-\tau)A_\text{shadow}/A$
exactly. With more than one caster, $f_s$ is the mean over an $8\times8$ grid of
points on the receiver of the product of the transmittances of the casters
whose shadows cover each point. That handles overlapping shadows, quantized to
the grid ([#68](https://github.com/bbrannon4/OpenBSE/issues/68)). The fractions are
computed for every timestep of the run period, in parallel, and cached in a
`.solar` file keyed by the geometry.

**Diffuse.** Two time-independent ratios are computed at initialization:

- **Sky** ($R_\text{sky}$). Rays are cast from up to $5\times5$ points on the
  receiver to $6\times24$ sky patches (altitudes 7.5°–82.5°). Each patch is
  weighted by $\cos\phi\cos\theta_i$ (projected solid angle) [@eplus-engref-25-2,
  "Shading Module"], and a ray passes the product of the transmittances of the casters it hits:
  $$
  R_\text{sky} = \frac{\sum_\text{patches} w\,\bar u}{\sum_\text{patches} w},
  $$ {#eq-sky-ratio}
  where $\bar u$ is the fraction of the receiver's points whose ray to the
  patch passes.
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

- **Transmittance.** A caster's transmittance is one fixed value. EnergyPlus
  can schedule it.
- **Windows.** The window floor and unshaded horizon
  ([#68](https://github.com/bbrannon4/OpenBSE/issues/68)).
- **Reflections.** There are no reflections from shading or building
  surfaces (EnergyPlus's `...WithReflections` options).

## Known limitations: solar distribution and shading {#sec-solar-limitations}

- [#68](https://github.com/bbrannon4/OpenBSE/issues/68): multi-caster point
  sampling; window diffuse ratio floor and unshaded horizon; interior
  distribution fallbacks and projection limits.

## Infiltration and ventilation {#sec-infiltration}

**Source:** `openbse-envelope/src/infiltration.rs`; `zone.rs`
(`scheduled_ventilation_flow`, `NaturalVentilationInput`); `heat_balance.rs::solve_timestep`
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

- **Air loop with outdoor air.** If any air loop serving the zone carries
  outdoor air (minimum outdoor-air fraction > 0), it arrives only through the
  supply. A zone served by a DOAS and a fan coil takes it from the DOAS.
- **No outdoor air in the loops.** For a zone served only by recirculating
  units (fan coils, CRAC, CRAH, or a loop whose fraction is 0), the flow enters
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
\dot V_\text{nv} = \sqrt{\left(C_w A_o f V\right)^2 + \left(C_d A_o f\sqrt{\frac{2g\,\Delta H\,|T_z - T_o|}{T_z + 273.15}}\right)^2},
$$ {#eq-natvent}

where:

- $C_w = 0.55 - 0.25\,\theta/45°$ with $\theta$ the angle between the wind and
  the opening's normal, and zero beyond 90°;
- $C_d = 0.40 + 0.0045\,|T_o - T_z|$, or the input `discharge_coefficient`;
- $\Delta H$ is the stack height difference;
- $V$ is the local wind at the zone's centroid height.

Natural ventilation is off during sizing. When the AFN is active, its
openings carry this flow instead.

**Comparison with EnergyPlus.**

- **Matches.** @eq-infiltration is `ZoneInfiltration:DesignFlowRate`,
  including E+ 25.2's outdoor density default and the zone's local wind
  (`ZoneEquipmentManager::CalcAirFlowSimple`). The exhaust treatment matches
  E+ without zone air mass balance.
- **Natural ventilation.** @eq-natvent is
  `ZoneVentilation:WindandStackOpenArea` with autocalculated coefficients.
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
Each zone's net HVAC exchange with the outdoors, outdoor air minus exhaust
(zone exhaust fans included), from the previous timestep, is a fixed flow.
Duct leaks are pressure-dependent cracks, or fixed flows when no static
pressures are given.

**Flow elements.** With $\Delta p$ the pressure difference across a path:

$$
\dot m_\text{crack} = C\left(\frac{\rho^*}{\rho_\text{ref}}\right)^{1-n}\left(\frac{\mu_\text{ref}}{\bar\mu}\right)^{2n-1}\operatorname{sgn}(\Delta p)|\Delta p|^n,
\qquad
\dot m_\text{open} = C_d A\operatorname{sgn}(\Delta p)\sqrt{2\rho^*|\Delta p|},
$$ {#eq-afn-elements}

where $\rho^* = \rho_\text{up}T_\text{up}/\bar T$ is the upstream node's
density corrected to the mean temperature of the two nodes (for an ideal
gas, the density at $\bar T$ in either flow direction), $\bar\mu$ the mean
of the two nodes' viscosities, $\mu = 1.71432\times10^{-5} + 4.828\times10^{-8}\,T$
Pa·s with $T$ in °C, and $\rho_\text{ref} = 1.2041$ kg/m³ and $\mu_\text{ref}$
the values for dry air at 20 °C and 101,325 Pa, where $C$ is defined. Below a
small $|\Delta p|$ threshold the flow follows the chord between the
threshold's two endpoints, so it is continuous through zero.

- **Two-way openings** integrate the orifice equation in closed form above
  and below the neutral plane. Over the opening height, $\Delta p$ varies
  with slope $g(\rho_b - \rho_a)$, so warm air leaves through the top while
  cold air enters below. The opening height is the surface's vertical
  extent: its `height` times $\sin\beta$, else its vertices' $z$-range, else
  $\sqrt{A}\sin\beta$; the opening fraction narrows the width. A large
  opening with no vertical extent is a one-way orifice.
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
absolute tolerance or $10^{-4}$ of the total through-flow, or when the
pressure step is below `convergence_tolerance` [Pa] with the residual
within ten times those limits (at most 30 iterations). The absolute
tolerance is `convergence_tolerance` × 0.001 m³/s at reference density,
$1.2\times10^{-4}$ kg/s at the default 0.1. After solving, each zone's outdoor inflow becomes
$\dot m_\text{inf}$. Its inflows from other zones become a mass-weighted
stream mixed with the outdoor air (`mix_advective_streams`), giving the
combined advective term of @eq-zone-ode and its humidity. Passive species
(e.g. CO₂) are then advanced by an implicit zone mass balance on the solved
flows, with sources, decay, deposition and filtration. Duct leaks carry
species too: a return leak brings ambient-zone air to the zone, and a supply
leak spills supply air into the ambient zone, its outdoor-air fraction
$f = \dot m_\text{OA}/\dot m_\text{supply}$ at the outdoor concentration
(that air never reaches the zone) and the rest at the zone's concentration.

**Comparison with EnergyPlus.** The network, element equations (E+
`generic_crack`, with its density and viscosity corrections), the
two-way and horizontal openings, the Swami–Chandra $C_p$ and the
Newton–Raphson solution follow EnergyPlus's AirflowNetwork (AIRNET)
[@eplus-engref-25-2, "AirflowNetwork Model"]. They are generated
automatically rather than from AirflowNetwork input objects. The
differences:

- **High-rise $C_p$.** It is a simple cosine rather than the Akins
  surface-average table ([#432](https://github.com/bbrannon4/OpenBSE/issues/432)).
- **No duct network.** There is no AirflowNetwork:Distribution duct
  network; duct leakage is a pair of leak paths per duct.
- **Timing.** E+ solves contaminants once per system timestep after the HVAC
  solution (`ZoneContaminantPredictorCorrector`). OpenBSE re-solves them from
  the start-of-step state on each envelope solve and keeps the last, which
  gives the same one-step integration on the final flows. Natural-ventilation
  open/close state likewise advances once per timestep.

## Known limitations: infiltration, ventilation and AFN {#sec-infiltration-limitations}

- [#432](https://github.com/bbrannon4/OpenBSE/issues/432): high-rise wind
  pressure coefficients are $0.6\cos\theta$ windward and $-0.3$ leeward,
  not E+'s Akins surface-average table.
- Scheduled ventilation lacks E+'s `ZoneVentilation:DesignFlowRate`
  coefficients and schedule (filed for later).

## Internal gains {#sec-internal-gains}

**Source:** `openbse-envelope/src/internal_gains.rs::resolve_gains_scheduled`;
`heat_balance.rs::timestep_gains`; top-level `people`, `lights`,
`equipment` and `outdoor_controlled_baseboards` objects resolved per zone in
`openbse-io/src/input.rs`.

Each gain has a design level, scaled each timestep by its schedule fraction
$f$. A design level given per floor area (people, lights, equipment, and
infiltration or ventilation flows) uses the zone's stated floor area, or else
the area of its floor surfaces (@sec-zone-geometry); such a load on a zone
with neither is a severe error. Equipment given per person uses the zone's
design number of people, the sum of its people counts; per-person
equipment in a zone without people is a severe error. With $f$ applied:

- **People.** $N$ occupants, either a count or per floor area, release
  sensible $Q_s = N f\,q_s$ and latent $Q_l = N f\,q_l$, with
  $q_s + q_l = M$, the activity level (required, W/person). By default the
  sensible heat per person follows $M$ and the zone air temperature $T_z$
  at the end of the previous timestep (@eq-people-sensible), and the latent
  heat is the remainder $q_l = M - q_s$. A given sensible fraction $\phi_s$
  sets $q_s = M\phi_s$ instead, and explicit W/person values override
  either. Of $Q_s$, the fraction $r$ (default 0.3) is radiant.
- **Lights.** Power $P f$. The return-air fraction $\phi_\text{ret}$ goes
  to the return air when an air loop carries it, and otherwise stays in the
  zone air (#51, @sec-controls). The fraction $r$ (default 0.7) of the total
  is radiant and the remainder convective.
- **Equipment.** Power $P f$. The lost fraction $\phi_\text{lost}$ leaves
  the zone, the latent fraction $\phi_l$ is moisture and the fraction $r$
  (default 0.3) is radiant, all of the total. The rest is convective:
  $$
  Q_l = Pf\phi_l,\qquad
  Q_r = Pfr,\qquad
  Q_c = Pf(1-\phi_\text{lost}-\phi_l-r).
  $$ {#eq-equipment-gain}
  The split is the same whatever the equipment's `fuel`, which only names the
  meter the input power $Pf$ is booked to (see below). Equipment with fuel
  `none` meters nothing and may have a negative $P$, a heat sink.
- **Outdoor-controlled baseboard.** Electric heat whose capacity follows the
  outdoor dry-bulb temperature $T_o$ between two points
  $(T_\text{low}, Q_\text{low})$ and $(T_\text{high}, Q_\text{high})$:
  $$
  Q = f\times\begin{cases}
  Q_\text{low}, & T_o \le T_\text{low},\\
  Q_\text{low} + (T_o - T_\text{low})\dfrac{Q_\text{high}-Q_\text{low}}{T_\text{high}-T_\text{low}}, & T_\text{low} < T_o < T_\text{high},\\
  0, & T_o \ge T_\text{high}.
  \end{cases}
  $$ {#eq-baseboard-gain}
  The fraction $r$ (default 0) is radiant and the rest convective. No
  thermostat controls it: it is a scheduled gain, not HVAC.

The default sensible heat per person is the correlation of EnergyPlus
(`InternalHeatGains::InitInternalHeatGains`), held within $[0, M]$:

$$
\begin{aligned}
q_s ={}& 6.4611027 + M\,(0.946892 + 2.55737\times10^{-5}M)\\
&+ T_z\left(7.139322 + M\,(-0.0627909 + 5.89271\times10^{-5}M)\right)\\
&+ T_z^2\left(-0.198550 + M\,(9.40018\times10^{-4} - 1.49532\times10^{-6}M)\right).
\end{aligned}
$$ {#eq-people-sensible}

At $M$ = 120 W it gives 82.7 W at 21 °C, 70.0 W at 23.9 °C and 40.8 W at
29.4 °C, so occupants of a set-back zone add less sensible and more latent
heat.

The convective parts enter the zone air as $\dot Q_\text{conv}$
(@eq-zone-ode). The radiant parts are spread over the zone's surfaces
(@eq-radiant-gain-flux). The latent parts become a moisture source
$\dot m_w = Q_l/h_g(T_z)$ in the zone moisture balance (@sec-zone-moisture).

**Energy and fuel.** Lights' power and each equipment object's whole input
power, lost heat included, are booked to the interior lighting and interior
equipment end uses, the equipment on its fuel: electricity, natural gas,
district heating water, district heating steam, or another fuel (propane,
fuel oil no. 1 or 2, diesel, gasoline, coal, other fuel 1 or 2, district
cooling). The summary report has an interior equipment row and a fuel total
for each of the first four, and groups the rest as *Other Fuels*; submeters
keep each fuel apart (end use `equipment_<fuel>`). Baseboards are electric
interior equipment.

**Sizing.** On design days, a design day's `internal_gains` mode chooses
among no gains (`off`, the heating default), design levels at every hour
(`full`, the cooling default), the normal schedules (`scheduled`), and each
gain at its design level in the hours its own schedule is above zero
(`full_when_occupied`) (@sec-sizing).

**Comparison with EnergyPlus.**

- **Model structure.** `People` and `Lights` have the structure of their
  EnergyPlus objects (`InternalHeatGains`). EnergyPlus's five equipment
  objects compute the same split from the same fractions
  (`InitInternalHeatGains`) and differ only in their meter, so OpenBSE has
  one equipment gain with a `fuel`:

  | EnergyPlus object | `fuel` | EnergyPlus meter |
  |---|---|---|
  | `ElectricEquipment` | `electricity` | Electricity:InteriorEquipment |
  | `GasEquipment` | `natural_gas` | NaturalGas:InteriorEquipment |
  | `HotWaterEquipment` | `district_heating_water` | DistrictHeatingWater:InteriorEquipment |
  | `SteamEquipment` | `district_heating_steam` | DistrictHeatingSteam:InteriorEquipment |
  | `OtherEquipment` | its fuel type, or `none` | that fuel's InteriorEquipment, or none |

  The design level is a level, per floor area or per person, as there
  (`setDesignLevel`); only `none` may be negative, as for `OtherEquipment`.
  `ZoneBaseboard:OutdoorTemperatureControlled` follows @eq-baseboard-gain,
  the same branches as `InitInternalHeatGains`, and is metered on
  Electricity:InteriorEquipment as there.
- **Fraction conventions.** Lights' and equipment's radiant, latent, lost
  and return-air fractions are each a fraction of the *total*, as in
  EnergyPlus; fractions that add up to more than 1 are a severe error. A
  lights `radiant_fraction` is EnergyPlus's Fraction Radiant + Fraction Visible.
- **People sensible heat.** The default autocalculated sensible heat
  (@eq-people-sensible), its limits $[0, M]$ and the latent remainder follow
  `InitInternalHeatGains`. As there, the zone air temperature is the one at
  the start of the timestep (EnergyPlus's mean air temperature before the
  zone solve), and design days use the same correlation. EnergyPlus requires
  an activity level schedule; OpenBSE requires a constant `activity_level`
  for each people object.
- **Defaults and checks.** Equipment's default radiant fraction is 0.3
  (EnergyPlus: 0). Per-person equipment in a zone without people is a
  severe error; EnergyPlus warns and uses 0 W.
- **Not modelled.** The carbon dioxide generation rate of `GasEquipment`
  and `OtherEquipment`: OpenBSE has no zone CO₂ balance outside the airflow
  network's species, where a zone's `species_generation` can carry it.
  EMS overrides and demand limiting of equipment are not modelled.
- **Baseboard outdoor temperature.** The site dry-bulb temperature;
  EnergyPlus uses the outdoor temperature at the zone's centroid height,
  which differs by about 0.1 K per 15 m.

## Ground heat transfer {#sec-ground-ht}

Ground contact has three forms, all described above:

- surfaces with a `ground` boundary, whose outside face is held at the
  monthly building-surface ground temperature (@sec-ground-temps, @sec-outside-face);
- F-factor floors (@sec-ffactor);
- interzone floors over unconditioned basements, modelled as zones.

There is no two- or three-dimensional ground model; EnergyPlus offers
Kiva (`Foundation:Kiva`), the Slab and Basement preprocessors, and
`Site:GroundDomain`.

## Zone geometry {#sec-zone-geometry}

**Source:** `openbse-envelope/src/geometry.rs` (`newell_normal`,
`polygon_area`, `zone_volume_from_surfaces`, `zone_floor_area`);
`heat_balance.rs::from_input_full`.

**Surfaces.** A surface given by vertices $\mathbf p_1 \dots \mathbf p_m$
(@sec-geometry) has the Newell normal

$$
\mathbf N = \sum_{k=1}^{m} (\mathbf p_k \times \mathbf p_{k+1}),\qquad
A = \tfrac12|\mathbf N|,\qquad \mathbf n = \mathbf N/|\mathbf N|,
$$ {#eq-newell}

with $\mathbf p_{m+1} = \mathbf p_1$, exact for any planar polygon. Azimuth
and tilt follow from @eq-azimuth-tilt. A surface given without vertices
uses its stated area, azimuth and tilt. The net area of a parent surface is
its area less its windows and doors.

**Zones.** The floor area, when not given, is the sum of the zone's floor
polygon areas. The volume, when not given, follows EnergyPlus
(`SurfaceGeometry::CalculateZoneVolume`):

1. **Enclosed zone.** When the walls, floors, roofs and ceilings form a closed,
   consistently wound shell, the volume comes from the divergence theorem,
   $V = \tfrac13\sum_k A_k\,\mathbf n_k\cdot\mathbf c_k$, with
   $\mathbf c_k$ the centroid of surface $k$. The shell is closed when every
   edge is crossed equally often in each direction, after splitting edges at
   any vertex lying along them; vertices within 12.7 mm are the same point.
2. **Vertical prism.** Otherwise, when the floor is flat and the walls present
   are vertical and of one height, or there are no walls and a flat ceiling
   matches the floor area, the volume is floor area × height. Zones with
   missing interior walls usually land here.
3. **Neither.** A severe error: the zone needs a `volume`, or surfaces that enclose it.
   EnergyPlus also treats this as a severe error, after trying ceiling area × height and
   opposite-wall area × distance, which OpenBSE doesn't try.

## Known limitations: gains, ground and geometry {#sec-gains-limitations}

- The summary report totals propane, fuel oils, diesel, gasoline, coal,
  other fuels 1 and 2 and district cooling together as *Other Fuels*.
- [#461](https://github.com/bbrannon4/OpenBSE/issues/461): exterior gas
  equipment is reported as heating gas.
- No multidimensional ground heat transfer model.

## Zone air heat balance {#sec-zone-air-hb}

**Source:** `heat_balance.rs::solve_timestep`, step 6c; `zone.rs`
(`solve_zone_air_temp`, `solve_zone_air_temp_with_q`, `calc_zone_loads`,
`mix_advective_streams`, `ZoneAirBalance`).

@sec-zone-time gives the zone air balance and its time discretization. Its
terms are assembled from the parts of this chapter:

- **Surface convection.** $\sum_i h_{c,i}A_i(T_{s,i} - T_{z,i})$ over the
  zone's opaque surfaces, windows (at their glass or shade temperature) and
  internal mass. In a stratified zone, $T_{z,i}$ is the local air at the
  surface's height (@sec-stratification).
- **Advection.** The outdoor streams of @sec-infiltration, plus the AFN
  interzone inflows, combined into one stream:
  $$
  \dot m_\text{adv} = \dot m_o + \dot m_\text{iz},\qquad
  T_\text{adv} = \frac{\dot m_o T_o + \dot m_\text{iz}T_\text{iz}}{\dot m_\text{adv}},
  $$ {#eq-advective-mix}
  where $T_\text{iz}$ is the flow-weighted temperature of the other zones'
  air. Humidity is mixed the same way.
- **Convective gains** $\dot Q_\text{conv}$:
  - internal gains (@sec-internal-gains), including lights' return-air
    share;
  - the HVAC-related gains of @sec-zone-time;
  - exhaust-fan motor heat;
  - the gap air returned by a window shade (@sec-shades);
  - transmitted solar sent to the air when a zone has no surface
    distribution (@sec-interior-solar).
- **HVAC**, in one of three forms:
  - supply air $\dot m_\text{sup}c_p(T_\text{sup} - T_z)$, from the air loops,
    VRF and zone equipment (@sec-hvac-iteration);
  - an ideal-loads heat rate $\dot Q_\text{HVAC}$;
  - nothing (free float).

**Ideal loads.** A zone with an ideal-loads system (@sec-sizing) runs
these steps in each solve:

1. **Mode.** The mode is chosen once per timestep from a free-float
   prediction with the previous surface state, and held. It is deadband
   when the system has no capacity in the needed direction.
2. **Heat rate.** The rate is the load to the active setpoint with the
   full backward-difference history:
   $\dot Q_\text{HVAC} = (\sum hA + \dot m_\text{adv}c_p + a_kC_z/\Delta t)\,T_\text{sp} - \sum hAT_s - \dot m_\text{adv}c_pT_\text{adv} - \dot Q_\text{conv} - a_k (C_z/\Delta t)\tilde T_k$,
   clamped to the heating or cooling capacity.
3. **Deadband override.** In deadband, if the free-float temperature lies
   outside the setpoints, the corresponding load is applied.

**Reported loads.** For HVAC-served and free-floating zones, the heating
and cooling loads reported are the residual of the balance without HVAC at
the solved temperature (`calc_zone_loads`):
$\dot Q = \sum hA(T_s - T_z) + \dot m_\text{adv}c_p(T_\text{adv} - T_z) + \dot Q_\text{conv} + a_k(C_z/\Delta t)(\tilde T_k - T_z)$.
A negative $\dot Q$ is a heating load and a positive one a cooling load.

**Balance check.** Each solve also records the five terms of
@eq-zone-ode at the final temperature: surface convection, advection,
convective gains, HVAC and storage (`ZoneAirBalance`). The run-time check
requires them to close (@sec-diagnostics).

**Comparison with EnergyPlus.** The terms match EnergyPlus's
`calcZoneOrSpaceSums` (SumIntGain, SumHA, SumHATsurf, SumMCp, SumSysMCp).
The ideal-loads heat rate matches `ZoneHVAC:IdealLoadsAirSystem` with
`NoLimit` sensible control, except that the system is a direct heat rate
rather than supply air [@eplus-engref-25-2, "Ideal Loads Air System"]. The
balance terms correspond to E+'s `DisplayZoneAirHeatBalanceOffBalance`
report.

## Zone moisture balance {#sec-zone-moisture}

**Source:** `heat_balance.rs::solve_timestep`, step 6c;
`zone.rs::solve_zone_humidity`.

The zone humidity ratio $W_z$ follows a mass balance like @eq-zone-ode,
discretized with the same backward difference over its own history
$\tilde W_k$:

$$
W_z = \frac{a_k\frac{\rho V}{\Delta t}\tilde W_k + \dot m_\text{adv}W_\text{adv} + \dot m_\text{sup}W_\text{sup} + \dot Q_l/h_g}
{a_k\frac{\rho V}{\Delta t} + \dot m_\text{adv} + \dot m_\text{sup}},
$$ {#eq-zone-moisture}

where:

- $\dot Q_l$ is the latent gain of people and equipment, plus the moisture
  carried by duct leakage;
- $h_g(T_z) = 2.50094\times10^6 + 1858.95\,T_z$ J/kg (E+ `PsyHgAirFnWTdb`).

The result is clamped to $[0, 0.1]$. It is then capped at saturation at $T_z$,
as EnergyPlus's `CorrectZoneHumRat` does; the excess condenses out of the air.
`limit_humidity_to_saturation: false` turns the cap off. Surfaces
neither absorb nor release moisture (HAMT, @sec-hamt, is not connected).

**Comparison with EnergyPlus.** @eq-zone-moisture is
`ZoneSpaceHeatBalanceData::correctHumRat` with the third-order scheme.
EnergyPlus converts latent gains with $h_g(T_z)$, always caps at
saturation, and optionally includes surface moisture (EMPD, HAMT).

## Room-air stratification {#sec-stratification}

**Source:** `zone.rs::RoomAirGradient`, `air_temp_at_height`,
`return_air_temp`, `occupied_air_temp`; `heat_balance.rs::solve_timestep`
(gradient update before step 4); `openbse-cli/src/main.rs::control_temp_for`.

A zone with a `room_air` block keeps one mean air temperature $T_z$, from @eq-zone-solve, but has a linear vertical profile over its height $H$:

$$
T(z) = T_z + g\left(z - \tfrac{H}{2}\right),
$$ {#eq-stratification}

where $g$ [K/m] is one of:

- **constant:** the input gradient, times an optional schedule;
- **Mundt-style:** $g = \dot Q_\text{conv}/(\dot m_\text{sup}c_pH)$,
  clamped to [0, the input gradient], and zero when there is no supply.

The profile is used in four places:

- each inside face convects to the air at its centroid height, in both the
  surface balance and the zone balance (@eq-inside-temp, @eq-zone-solve);
- the air loops draw return air at the return height (default: the
  ceiling);
- the thermostat may sense the air at its own height (default 1.1 m);
- reports give the occupied-height temperature.

The zone balance debits the supply's outflow at $T_z$ while the loop receives
the return at $T(z_\text{ret})$, so the HVAC iteration takes the difference,
$\dot m c_p\,(T(z_\text{ret}) - T_z)$, back out of the zone as a sensible gain.
The heat in the return is therefore the heat the zone gave up. A loop that
returns no air (a 100% outdoor-air system) draws no such debit: its outflow
still leaves at $T_z$.

**Comparison with EnergyPlus.** The constant gradient corresponds to
`RoomAir:TemperaturePattern:ConstantGradient` in the user-defined room-air
model. EnergyPlus corrects the next step's predicted load with
$(T_\text{sup} - T_\text{ret})/(T_\text{sup} - T_z)$. OpenBSE doesn't need
the factor: the debit is in the zone balance within the HVAC iteration, so the
controls see it directly. The Mundt-style
gradient is a simplification of EnergyPlus's nodal Mundt model
(`RoomAirModel:Mundt`).

## Known limitations: zone air balances {#sec-zone-hb-limitations}

