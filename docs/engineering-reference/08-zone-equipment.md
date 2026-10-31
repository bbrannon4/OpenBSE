# Zone equipment and terminals {#sec-zone-equipment}

Terminals sit between an air loop's central equipment and a zone. Zone
equipment conditions a zone directly. Their controls are in @sec-controls.
This chapter gives the component physics and how each one is driven.

## VAV boxes {#sec-vav-box}

**Source:** `openbse-components/src/vav_box.rs` (`VAVBox`:
`simulate_air`, `reheat_for_zone_load`); `main.rs` (terminal loop in the
air-loop simulation).

**Inputs.** A box has:

- a maximum primary flow $\dot m_\text{max}$ [kg/s];
- a minimum flow fraction $f_\text{min}$;
- a reheat type (none, electric or hot water) and capacity $\dot Q_r$;
- a maximum reheat air temperature $T_{r,\text{max}}$ (default 40 °C);
- a maximum flow fraction during reheat $f_r$ (default 0.5).

Let $\dot m_\text{min} = f_\text{min}\dot m_\text{max}$ and
$\dot m_{h,\text{max}} = \max(f_r, f_\text{min})\dot m_\text{max}$.

**Control signal.** Each iteration the driver sets a signal $s \in [-1, 1]$.
A negative value is a cooling demand. A positive value is the reheat
demanded, as a fraction of $\dot Q_r$.

**Cooling and deadband.** For $s < 0$, the flow is

$$
\dot m = \dot m_\text{min} + |s|\left(\dot m_\text{max} - \dot m_\text{min}\right),
$$ {#eq-vav-cooling}

where the driver derives $s$ from the flow that meets the zone cooling load.
With $s = 0$, the box holds its minimum flow, with no reheat.

**Reheat demand.** Outside cooling, in both heating and deadband, the driver
asks the box for the reheat that delivers the load $\dot Q_z$ to the heating
setpoint (negative in deadband, the heat the zone can lose before reaching
the setpoint), at the zone temperature $T_z$ and inlet temperature
$T_\text{in}$. With $c_p = 1006$ J/(kg·K),

$$
\dot Q_\text{min} = \dot Q_z + \dot m_\text{min}c_p(T_z - T_\text{in}).
$$ {#eq-vav-reheat}

The reheat is zero if $\dot Q_\text{min} \le 0$, and $\dot Q_\text{min}$ if
that keeps the outlet at or below $T_{r,\text{max}}$. Beyond that, the damper
opens (dual maximum), with the air at the limit:

$$
\dot m = \operatorname{clamp}\left(\frac{\dot Q_z}{c_p(T_{r,\text{max}} - T_z)},\ \dot m_\text{min},\ \dot m_{h,\text{max}}\right),
\qquad
\dot Q = \dot m c_p(T_{r,\text{max}} - T_\text{in}).
$$ {#eq-vav-dual-max}

The box reconstructs that flow from $s$:
$\dot m = \operatorname{clamp}(s\dot Q_r/[c_p\max(T_{r,\text{max}} - T_\text{in}, 0.1)],\ \dot m_\text{min},\ \dot m_{h,\text{max}})$.

**Outlet.** The delivered reheat is limited by $\dot Q_r$ and
$T_{r,\text{max}}$, and $W$ is unchanged. Electric reheat draws
$\dot Q$ of electricity. Hot-water reheat is a load on its plant loop. Its
water-side limit never applies, because the box never receives its water
inlet ([#234](https://github.com/bbrannon4/OpenBSE/issues/234)).

**Comparison with EnergyPlus.** The sequence follows
`AirTerminal:SingleDuct:VAV:Reheat` with `ReverseWithLimits`
(`SingleDuct.cc`, `SimVAV`) [@eplus-engref-25-2, "Air System Distribution
Terminals"]. The differences are
([#246](https://github.com/bbrannon4/OpenBSE/issues/246)):

- **Defaults.** EnergyPlus has no default reheat temperature limit
  (35 °C is recommended; the code comment says 40 °C is the default). It
  autosizes $f_r$ to $0.002032$ m³/s per m² of floor area.
- **Central flow.** In heating, `build_vav_signals` gives the air loop a
  flow from a proportional band on the zone temperature error, rising to
  half of design. That flow can differ from the one the box sets, so the fan
  and coil can see a different flow from the zone.

## Parallel fan-powered boxes {#sec-pfp-box}

**Source:** `openbse-components/src/pfp_box.rs` (`PFPBox`).

A parallel fan-powered box has a primary damper, a secondary fan drawing
plenum air in parallel, and an electric reheat coil. Its modes are:

- **Cooling** ($s < 0$): primary flow as in @eq-vav-cooling, fan off.
- **Deadband** ($s = 0$): minimum primary flow, fan off.
- **Heating** ($s > 0$): minimum primary flow, fan on, reheat on.

With the fan on, the mixed state at flow
$\dot m = \dot m_p + \dot m_s$ is

$$
T_\text{mix} = \frac{\dot m_pT_\text{in} + \dot m_sT_s}{\dot m} + \frac{P_f}{\dot m c_p},
\qquad
P_f = \frac{\dot m_s\Delta p_f}{1.2\,\eta_f},
$$ {#eq-pfp-mix}

with the secondary air temperature $T_s$ fixed at 24 °C,
$\Delta p_f = 375$ Pa and $\eta_f = 0.55$. The reheat is
$\min(s\dot Q_r, \dot Q_r)$, limited to a 35 °C outlet. The box's power is
$P_f$ plus the reheat.

**Comparison with EnergyPlus.** EnergyPlus's
`AirTerminal:SingleDuct:ParallelPIU:Reheat` runs the fan whenever the primary
flow is at or below `Fan On Flow Fraction`, draws actual induced air, and
offers water, fuel and steam coils. In OpenBSE
([#246](https://github.com/bbrannon4/OpenBSE/issues/246),
[#55](https://github.com/bbrannon4/OpenBSE/issues/55)):

- the `secondary_air_temp` input is not read, and the temperature is not
  live;
- reheat demand is the zone heating load alone, so it leaves out the
  primary-air term of @eq-vav-reheat, and there is no reheat in deadband;
- the mixed humidity ratio is the primary air's;
- the secondary fan's flow cannot be autosized.

## Dual-duct boxes {#sec-dual-duct-box}

**Source:** `openbse-components/src/dual_duct_box.rs` (`DualDuctBox`);
`openbse-airloop/src/lib.rs` (`build_dual_duct_signals`).

A dual-duct box delivers a constant design flow $\dot m_d$ blended from a
hot and a cold deck. With $\dot m_\text{min} = f_\text{min}\dot m_d$
(default $f_\text{min} = 0.2$) and the part-load ratio
$\text{PLR} = \dot Q_z/[\dot m_dc_p|T_\text{deck} - T_\text{sp}|]$,

$$
\begin{aligned}
\text{heating:}\quad & \dot m_\text{hot} = \dot m_\text{min} + \text{PLR}(\dot m_d - \dot m_\text{min}), && \dot m_\text{cold} = \dot m_d - \dot m_\text{hot},\\
\text{cooling:}\quad & \dot m_\text{cold} = \dot m_\text{min} + \text{PLR}(\dot m_d - \dot m_\text{min}), && \dot m_\text{hot} = \dot m_d - \dot m_\text{cold},\\
\text{deadband:}\quad & \dot m_\text{hot} = \dot m_\text{cold} = \dot m_d/2, &&
\end{aligned}
$$ {#eq-dual-duct}

and $T_\text{sup} = (\dot m_\text{hot}T_\text{hot} + \dot m_\text{cold}T_\text{cold})/\dot m_d$,
with the deck *setpoints* as $T_\text{hot}$ and $T_\text{cold}$.

**Comparison with EnergyPlus.** EnergyPlus's
`AirTerminal:DualDuct:ConstantVolume` (`DualDuct.cc`,
`SimDualDuctConstVol`) solves the mix from the zone load and the decks'
actual states, with no minimum on either deck, so a zero load gets supply air
at the zone temperature. @eq-dual-duct does not meet the load. A small load
has $\text{PLR} \approx 0$, so most of the air comes from the *opposite*
deck: with decks at 35 °C and 13 °C, a small cooling load gets 30.6 °C air
and a small heating load gets 17.4 °C air
([#245](https://github.com/bbrannon4/OpenBSE/issues/245)). The results for
dual-duct systems should not be relied on until this is fixed.

## Known limitations: terminals {#sec-terminal-limitations}

- [#245](https://github.com/bbrannon4/OpenBSE/issues/245): dual-duct mix
  from PLR with a damper minimum, so it does not meet the load.
- [#246](https://github.com/bbrannon4/OpenBSE/issues/246): fan-powered box
  input not read, reheat short of the primary-air term and fan-on logic;
  VAV flow seen by the air loop differs from the box's.
- [#55](https://github.com/bbrannon4/OpenBSE/issues/55): fan-powered box
  secondary air not live.
- [#234](https://github.com/bbrannon4/OpenBSE/issues/234): hot-water reheat
  never sees its water inlet.

## Remaining zone equipment {#sec-zone-equipment-remaining}

::: {.callout-note}
## Not yet written

The rest of this chapter will cover fan coils, PTAC and PTHP, VRF, radiant panels, ground- and water-source heat pumps, and CRAC/CRAH units. It is written from the code in its own review under [#181](https://github.com/bbrannon4/OpenBSE/issues/181); see @sec-status.
:::
