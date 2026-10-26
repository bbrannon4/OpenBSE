# Introduction and conventions {#sec-intro}

## Purpose and scope {#sec-purpose}

This reference documents the physics, equations and algorithms of the OpenBSE
simulation engine. For each model it states what the model computes, its
governing equations, how they are solved, the assumptions and validity limits
behind them, and where the method comes from. An engineer should be able to
check OpenBSE's physics from this document without reading the code.

The reference has two further roles.

- **Comparison with EnergyPlus.** EnergyPlus [@eplus-engref-25-2] is the
  minimum reference standard for OpenBSE: each model either matches the
  EnergyPlus method or deviates from it deliberately, for more accurate
  physics, for energy conservation, or for a project requirement. This
  reference records which, and why.
- **Equation review.** Each chapter is written from the code as it is, not
  from the original design. Writing it is a full review of the engine's
  equations and wiring. Discrepancies found while writing are filed as issues
  rather than fixed in the documentation (@sec-review).

The reference covers the engine (the Rust crates in `crates/`). It does not
cover the YAML input format, which is defined by `docs/openbse_schema.json`
and described in the user guide, or the desktop editor in `tools/editor/`.

## How to read a model section {#sec-how-to-read}

Each model section follows the same structure.

**Source.**
: The file and function(s) that implement the model, as
  `crate/src/file.rs::function`. Paths are relative to `crates/` and name
  the crate directory, e.g. `openbse-envelope/src/ctf.rs`.

**Governing equations.**
: The equations as the code evaluates them, with every symbol defined and its
  unit given. Key equations are numbered by chapter, e.g. @eq-air-component,
  and referenced by number elsewhere.

**Solution.**
: How the equations are solved: explicit evaluation, iteration, the order of
  updates within a timestep, convergence tolerances and limits.

**Comparison with EnergyPlus.**
: *Match*, with the EnergyPlus routine (`Module::Function`) and Engineering
  Reference section; or *deviation*, with the reason and its effect.

**Assumptions and validity.**
: Simplifications, input ranges, clamps and fallbacks.

**Known limitations.**
: Open issues for the model, each linked to its GitHub issue.

Textbook physics, such as Fourier conduction, energy and mass balances, and
$\dot Q = \dot m c_p \Delta T$, is stated without citation. Correlations,
empirical curves, published models, ASHRAE Handbook chapters and Standards are
cited; the bibliography is in @sec-curves.

## Engine organization {#sec-organization}

OpenBSE is a Rust workspace of eleven crates. @tbl-crates lists what each
holds; later chapters name the crate and file of every model.

| Crate | Contents |
|---|---|
| `openbse-psychrometrics` | Moist-air and water properties (`lib.rs`); thermal-comfort indices (`comfort.rs`). |
| `openbse-weather` | EPW and TMY3 readers, hourly interpolation, design days. |
| `openbse-core` | Component interfaces (`ports.rs`: `AirComponent`, `PlantComponent`, `EnvelopeSolver`), the component graph (`graph.rs`), shared types and control-signal structures. |
| `openbse-envelope` | Building heat balance: geometry, constructions and CTFs, convection, long-wave and solar exchange, shading, infiltration and airflow network, internal gains, zone heat and moisture balances, HAMT, comfort grid. |
| `openbse-components` | HVAC, plant and service-water component models, and performance curves. |
| `openbse-a205` | ASHRAE Standard 205 representation files (RS0001–RS0007) and their interpolation. |
| `openbse-controls` | Thermostats, setpoint controllers and the sensor/actuator state. |
| `openbse-airloop` | Per-system control-signal builders (PSZ, CRAC, CRAH, DOAS, FCU, VAV, dual duct). |
| `openbse-cosim` | Co-simulation with external processes over a JSON protocol. |
| `openbse-io` | YAML input and validation, output writing, design-day sizing (`sizing.rs`), plant connections and sizing (`plant.rs`), parametric runs. |
| `openbse-cli` | The simulation driver (`main.rs`): the timestep loop, HVAC–zone coupling and loop simulation; run-time conservation checks (`run_checks.rs`). |

: Workspace crates. {#tbl-crates}

## Units {#sec-units}

The engine computes in SI units throughout (@tbl-units). Inputs use the same
units unless the schema says otherwise. Time-series outputs report rates in W;
run summaries report energy in kWh.

| Quantity | Unit | Notes |
|---|---|---|
| Temperature | °C | Kelvin ($T + 273.15$) inside radiation terms. |
| Temperature difference | K | |
| Time step | s | $\Delta t = 3600/N$ for $N$ timesteps per hour. |
| Mass flow rate | kg/s | Air flows are on a dry-air basis (@sec-air-basis). |
| Volume flow rate | m³/s | Inputs; each model states the density it converts with. |
| Pressure | Pa | Barometric pressure $p_b$; fan rise $\Delta p$. |
| Humidity ratio | kg~w~/kg~da~ | Mass of water vapour per mass of dry air. |
| Specific enthalpy | J/kg~da~ | Moist air, per kg dry air; datum dry air and liquid water at 0 °C. |
| Heat or power rate | W | Fuel rates are in W of fuel energy. |
| Energy | J | Reported as kWh ($1\ \text{kWh} = 3.6\times10^{6}$ J). |
| Heat flux | W/m² | |
| Heat-transfer coefficient | W/(m²·K) | |
| Angles | ° | Inputs and reported values; radians inside trigonometric functions. |

: Units. {#tbl-units}

### Air mass flow basis {#sec-air-basis}

Enthalpy and humidity ratio are per kilogram of dry air, as in EnergyPlus. Air
mass flow rates are therefore read as dry-air flows: the energy carried by an
air stream is $\dot m\, h$ and the water it carries is $\dot m\, W$. The
code's port type, `openbse-core/src/ports.rs::AirPort`, labels its flow only
as kg/s; this reference uses the dry-air reading wherever the distinction
matters.

## Sign conventions {#sec-signs}

The engine uses the conventions below. A model section restates the sign of
any quantity whose convention differs.

**Air-side components** (`AirComponent`). The thermal output is the heat
delivered to the air stream:

$$
\dot Q_\text{air} = \dot m \left( h_\text{out} - h_\text{in} \right),
$$ {#eq-air-component}

positive when the component adds heat (heating coil, fan) and negative when it
removes heat (cooling coil). The run-time check in @sec-diagnostics compares
@eq-air-component with each component's reported `thermal_output`.

**Plant components** (`PlantComponent`). The load requested of a plant
component is positive for heating the water and negative for cooling it; its
thermal output follows the same sign:

$$
\dot Q_\text{water} = \dot m_w c_{p,w} \left( T_\text{out} - T_\text{in} \right).
$$ {#eq-plant-component}

**Energy use.** Electric power and fuel rates are always non-negative.
Electric use and fuel use are reported separately; neither includes the
other.

**Zone loads.** Heating and cooling loads are reported as non-negative
magnitudes: a heating load of $\dot Q > 0$ means the zone needs $\dot Q$ of
heat to hold its heating setpoint, and a cooling load of $\dot Q > 0$ means
it needs $\dot Q$ removed to hold its cooling setpoint.

**Gains to a zone.** Internal gains, HVAC distribution losses and radiant
HVAC gains delivered to a zone are positive when they add heat to it.

**Surface heat fluxes.** At the inside face of a surface:

- the convective flux $q''_\text{conv}$ is positive from the surface into the
  zone air;
- the net long-wave flux $q''_\text{LW}$ is positive into the surface.

The two inside fluxes thus have opposite reference directions, as in the code
(`openbse-envelope/src/surface.rs::Surface`). Each envelope section states
the direction of every flux it uses.

## Time conventions {#sec-time}

**Calendar.** A run uses a non-leap calendar of 365 days; a weather file's
29 February is dropped. Hour $H \in \{1,\dots,24\}$ is the hour *ending* at
clock time $H{:}00$, as in EPW files.

**Timestep.** Each hour is split into $N$ timesteps of length
$\Delta t = 3600/N$ s, indexed $k = 1,\dots,N$. A timestep's state values
(temperatures, humidity ratios) are those at the end of the timestep, and its
rates (powers, loads) are averages over it. Where a model needs a single time
within the step, such as the solar position, the engine uses the midpoint,

$$
t_\text{h} = (H - 1) + \frac{k - 1/2}{N},
$$ {#eq-fractional-hour}

in hours of local standard time (`openbse-core/src/types.rs::TimeStep::fractional_hour`).

**Weather.** Within an hour, weather values are interpolated linearly between
the previous and current hourly records; solar irradiance is not
interpolated, because an hourly record is the hour's integrated value
(`openbse-weather/src/lib.rs::WeatherHour::interpolate`). @sec-site gives the
details.

## Geometry conventions {#sec-geometry}

Building coordinates are right-handed, with $x$ east, $y$ north and $z$ up,
in metres. A surface's vertices are listed counter-clockwise as seen from
outside the zone, so that its Newell normal $\mathbf n$ points outward
(`openbse-envelope/src/geometry.rs`). From the unit outward normal
$\mathbf n = (n_x, n_y, n_z)$:

$$
\phi = \operatorname{atan2}(n_x, n_y) \bmod 360°, \qquad
\beta = \arccos n_z ,
$$ {#eq-azimuth-tilt}

where the azimuth $\phi$ is measured clockwise from north (east $= 90°$, south
$= 180°$) and the tilt $\beta$ from the upward vertical (a roof facing up
has $\beta = 0°$, a wall $90°$, a floor facing down $180°$). As in EPW
headers, latitude is positive north, longitude positive east, and the time
zone is in hours from UTC.

## Notation {#sec-notation}

@tbl-symbols lists the symbols used across chapters. A chapter defines any
further symbols where it first uses them.

| Symbol | Quantity | Unit |
|---|---|---|
| $T$ | Temperature | °C |
| $W$ | Humidity ratio | kg~w~/kg~da~ |
| $h$ | Specific enthalpy of moist air | J/kg~da~ |
| $p_b$ | Barometric pressure | Pa |
| $\rho$ | Density | kg/m³ |
| $c_p$ | Specific heat at constant pressure | J/(kg·K) |
| $\dot m$ | Mass flow rate | kg/s |
| $\dot V$ | Volume flow rate | m³/s |
| $\dot Q$ | Heat rate | W |
| $q''$ | Heat flux | W/m² |
| $P$ | Electric power | W |
| $A$ | Area | m² |
| $U$ | Thermal transmittance | W/(m²·K) |
| $h_c$, $h_r$ | Convective, linearized radiative coefficient | W/(m²·K) |
| $\varepsilon$ | Long-wave emissivity | – |
| $\alpha$ | Solar absorptance | – |
| $\sigma$ | Stefan–Boltzmann constant, $5.670\times10^{-8}$ | W/(m²·K⁴) |
| $\Delta t$ | Timestep | s |
| $\mathit{PLR}$ | Part-load ratio | – |

: Common symbols. {#tbl-symbols}

Subscripts: *in*, *out* for a component's inlet and outlet; *z* for zone
air; *s* or *si*, *so* for a surface and its inside and outside faces; *o*
for outdoor air; *sup* for supply air; *w* for water; *da* for dry air.
Superscript $t$ marks the current timestep and $t-j$ the timestep $j$ steps
earlier.

## Status of this reference {#sec-status}

Chapters are written in chapter order, one review at a time, under issue
[#181](https://github.com/bbrannon4/OpenBSE/issues/181). A chapter is in one
of three states:

- **Reviewed:** written from the current code to the standards in @sec-how-to-read.
- **Pre-review:** text carried over from the former single-file reference
  (mostly written in March 2026), marked with a warning box. It may be stale.
- **Not yet written:** a placeholder naming what the chapter will cover.

## Review process {#sec-review}

When writing a section turns up a discrepancy, it is filed as a GitHub issue
with the evidence and the section it came from, and linked from that
section's *Known limitations*. The documentation is not changed to hide it,
and the engine is not changed in the documentation pull request. A
discrepancy is any of:

- an equation that is wrong;
- code that does not do what its comments or intent say;
- an undocumented deviation from EnergyPlus;
- an input that is read but never used, or never read;
- missing physics.

Issue [#181](https://github.com/bbrannon4/OpenBSE/issues/181) keeps a running
list of the issues filed by the review.

**Keeping the reference current.** A change to a model's equations updates
that model's section in the same pull request, including its comparison with
EnergyPlus and its known limitations.

## EnergyPlus references {#sec-eplus-refs}

EnergyPlus comparisons refer to version 25.2: its source code
[@eplus-source-25-2] for routine names, written `Module::Function` (e.g.
`Psychrometrics::PsyHFnTdbW`), and its Engineering Reference
[@eplus-engref-25-2] for method descriptions.
