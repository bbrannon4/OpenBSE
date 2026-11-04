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
- a maximum flow fraction during reheat $f_r$. By default it is sized to the
  zone's design heating flow over $\dot m_\text{max}$, or without design days to
  $0.002032$ m³/s per m² of floor area, bounded by $f_\text{min}$ and 1.

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
$\dot Q$ of electricity. Hot-water reheat on a plant loop is the cross-flow
coil of @sec-heating-coils: its valve delivers the reheat asked for, held to
$T_{r,\text{max}}$, on the loop's supply water, so a cooler loop can leave the
box short. It is sized at the most reheat the box ever asks for, the smaller of
$\dot Q_r$ and its heating maximum flow warmed from the loop's cooling supply
temperature to $T_{r,\text{max}}$, on the air flow that carries that load at
the limit. Without a plant loop the coil is ideal, limited by $\dot Q_r$.

**Comparison with EnergyPlus.** The sequence follows
`AirTerminal:SingleDuct:VAV:Reheat` with `ReverseWithLimits`
(`SingleDuct.cc`, `SimVAV`) [@eplus-engref-25-2, "Air System Distribution
Terminals"], including E+'s autosized heating maximum flow. E+ has no default
reheat temperature limit and recommends 35 °C when it is unknown; OpenBSE's
default is 40 °C. The air loop allocates each box its primary flow from these same
rules (@sec-controls), and the box passes exactly that flow, so the loop's supply is
the sum of its boxes' flows.

## Parallel fan-powered boxes {#sec-pfp-box}

**Source:** `openbse-components/src/pfp_box.rs` (`PFPBox`).

A parallel fan-powered box has a primary damper, a secondary fan drawing
plenum air in parallel, and an electric reheat coil. Its modes are:

- **Cooling** ($s < 0$): primary flow as in @eq-vav-cooling; the fan runs
  while the primary flow is at or below `fan_on_flow_fraction` of maximum
  (default $f_\text{min}$), so at minimum primary flow.
- **Deadband** ($s = 0$): minimum primary flow, fan off.
- **Heating** ($s > 0$): minimum primary flow, fan on, reheat on. The driver
  asks for this whenever minimum primary air would cool the zone below its
  heating setpoint, deadband included.

With the fan on, the mixed state at flow
$\dot m = \dot m_p + \dot m_s$ is

$$
T_\text{mix} = \frac{\dot m_pT_\text{in} + \dot m_sT_s}{\dot m} + \frac{P_f}{\dot m c_p},
\qquad
P_f = \frac{\dot m_s\Delta p_f}{1.2\,\eta_f},
$$ {#eq-pfp-mix}

with $\Delta p_f = 375$ Pa and $\eta_f = 0.55$. The secondary fan draws zone
air: $T_s$ and its humidity ratio are the zone's at the start of the timestep,
and the mixed humidity ratio weights the two streams' by flow. The reheat the
driver asks for holds the zone with the mix:

$$
\dot Q = \dot Q_z + \dot m_pc_p(T_z - T_\text{in}) + \dot m_sc_p(T_z - T_s) - P_f,
$$ {#eq-pfp-reheat}

limited to $\dot Q_r$ and a 35 °C outlet. The box's power is $P_f$ plus the
reheat.

**Comparison with EnergyPlus.** @eq-pfp-reheat and the fan-on rule follow
`AirTerminal:SingleDuct:ParallelPIU:Reheat` (`PoweredInductionUnits.cc`,
`CalcParallelPIU`), with E+'s autosized Fan On Flow Fraction as the default.
E+ also raises the primary flow while cooling to offset the induced air;
OpenBSE does not, which matters little with induced air near the zone
temperature. E+ draws the induced air from whatever node feeds the secondary
inlet, a return plenum or the zone's exhaust node; OpenBSE models no plenum on
this path and always draws zone air, as E+ does from the exhaust node. E+ also
offers water, fuel and steam coils
([#350](https://github.com/bbrannon4/OpenBSE/issues/350)).

## Dual-duct boxes {#sec-dual-duct-box}

**Source:** `openbse-components/src/dual_duct_box.rs` (`DualDuctBox`);
`openbse-airloop/src/lib.rs` (`build_dual_duct_signals`);
`openbse-cli/src/main.rs` (`simulate_dual_duct_decks`).

**Decks.** The loop's fan and any other shared components carry the whole
flow. After them the supply splits: the heating coils run in series on the
hot deck's flow and the cooling coils on the cold deck's. Each deck's flow is
the sum of its boxes' flows, and each coil holds its deck setpoint
(`heating_supply_temp`, `cooling_supply_temp`) while its deck carries air.
Input validation rejects a dual-duct loop that lists any component other
than heat recovery after a coil, or a heat pump.

**Box.** A dual-duct box passes its constant design flow $\dot m_d$ and splits it
between the decks so the mix meets the zone load $\dot Q_z$ (positive heating),
measured against the zone temperature $T_\text{ref}$:

$$
\begin{aligned}
T_\text{target} &= T_\text{ref} + \frac{\dot Q_z}{\dot m_d\,c_p},\\
\dot m_\text{cold} &= \dot m_d\,\frac{T_\text{hot} - T_\text{target}}{T_\text{hot} - T_\text{cold}}, \qquad 0 \le \dot m_\text{cold} \le \dot m_d,\\
\dot m_\text{hot} &= \dot m_d - \dot m_\text{cold}.
\end{aligned}
$$ {#eq-dual-duct}

$\dot Q_z$ is the predictor's load to the heating or cooling setpoint, and
$T_\text{ref}$ is that setpoint, so the zone lands on it. With no load, $T_\text{ref}$
is the zone temperature at the start of the timestep and the supply holds the
zone where it is. $T_\text{hot}$ and $T_\text{cold}$ are the decks' leaving
temperatures from the last simulation of each deck (the setpoints before a deck
has carried air), so a coil short of capacity shows in the mix. The box's outlet
then mixes the decks' actual leaving enthalpy and humidity ratio at those flows,
so the zones receive exactly the air the deck coils conditioned. With decks
within $10^{-5}$ K of each other the box takes the hot deck for a heating load
and the cold deck otherwise.

A constant-volume dual-duct system mixes the decks at every load, so both deck
coils run whenever the zones need any supply between the deck temperatures.

**Comparison with EnergyPlus.** @eq-dual-duct is EnergyPlus's
`AirTerminal:DualDuct:ConstantVolume` (`DualDuct.cc`, `SimDualDuctConstVol`):
$\dot m_\text{cold} = (Q_\text{ZnReq} - \dot m c_p T_\text{hot})/(c_p T_\text{cold} - c_p T_\text{hot})$
with $Q_\text{ZnReq} = \dot Q_z + \dot m c_p T_z$, no minimum on either deck, and
enthalpy and humidity mixing. Differences:

- **Reference temperature.** E+ takes $T_z$ from the zone node, the zone
  temperature at the end of the last timestep, for every load. OpenBSE measures
  a load against the setpoint the load is computed to reach, which delivers the
  load exactly; with no load both use the zone temperature.
- **Deck temperatures.** E+ reads the deck inlet nodes within its air-loop
  iteration; OpenBSE reads the decks' last simulated states within its HVAC
  iteration. Both converge to the same states.
- **Fan.** OpenBSE has one fan ahead of the split; E+ allows a fan on each deck.

## Known limitations: terminals {#sec-terminal-limitations}

- Dual-duct boxes are constant-volume only: there is no variable-volume or
  outdoor-air dual-duct box, and the decks have no supply-temperature reset.
- [#350](https://github.com/bbrannon4/OpenBSE/issues/350): fan-powered box
  reheat is electric only, its secondary flow cannot be autosized, and there is
  no series box.
- A fan-powered box induces zone air, never return-plenum air.

## Fan coils, PTAC and PTHP {#sec-fcu-ptac}

**Source:** `openbse-airloop/src/lib.rs` (`build_fcu_signals`). These units
are air loops with one zone. Their fans and coils are the components of @sec-air-side; this section gives what is specific to them.

**PTAC and PTHP.** The fan runs at design flow when heating or cooling. In
deadband it runs at design flow with a continuous fan, and is off otherwise.
The coils aim at the design supply temperatures, and the loop's part-load
ratio cycles the output to the zone load (@sec-controls). A PTHP's heating
coil is the heat-pump coil of @sec-hp-heating. The outdoor-air fraction is the
loop's minimum.

**Fan coils.** A fan coil meets the predictor's zone load $\dot Q_z$ (positive
heating), measured against the setpoint $T_\text{sp}$ the load is computed to
reach. In a zone that loops listed ahead of it also serve, it meets what they
leave: its load to each setpoint is the zone's less
$\sum_i \dot m_i c_p(T_i - T_\text{sp})$ over their supply streams, and that
remaining load sets its mode, as EnergyPlus distributes a zone's load over its
equipment in sequence (`RemainingOutputReqToHeatSP`, `ToCoolSP`). By default it runs EnergyPlus's `ASHRAE90VariableFan` method: with no load
the fan runs at its low speed $\dot m_\text{low} = 0.33\,\dot m_d$ with both
coils off; with a load it stays at low speed while the coil can meet the load
below the supply limit $T_\text{lim}$ (`heating_supply_temp` or
`cooling_supply_temp`), then raises the fan speed with the supply held at the
limit:

$$
\dot m = \operatorname{clamp}\left(\frac{\dot Q_z}{c_p(T_\text{lim} - T_\text{sp})},\ \dot m_\text{low},\ \dot m_d\right), \qquad
T_\text{sup} = T_\text{sp} + \frac{\dot Q_z}{\dot m c_p},
$$ {#eq-fcu}

with $T_\text{sup}$ limited to $T_\text{lim}$. The active coil aims at
$T_\text{sup}$, less the fan's design rise when the fan follows the coils. With
`cycling: on_off` it runs EnergyPlus's `CyclingFan` method instead: fan and coil
at design flow and design supply temperature, the loop's runtime fraction
meeting the load, as a PTAC does.

A fan coil mixes no outdoor air: its zone's `outdoor_air` enters directly
(@sec-infiltration) unless a DOAS serves the zone, and a
`minimum_damper_position` on a fan-coil loop is a severe error.

**Comparison with EnergyPlus.** `ZoneHVAC:PackagedTerminalAirConditioner` and
`:PackagedTerminalHeatPump` cycle to the load, as OpenBSE's do.
`ZoneHVAC:FourPipeFanCoil` with `ASHRAE90VariableFan` follows the single-zone
VAV model (`FanCoilUnits.cc`, `SZVAVModel::calcSZVAVModel`): low-speed coil
modulation up to the design supply temperature, then fan speed, as
in @eq-fcu. E+ solves the coil and fan iteratively against its zone node
temperature; OpenBSE sets the flow and coil target directly from the load
against setpoint. E+ offers other capacity-control methods, a low-speed ratio
input and an outdoor-air mixer; OpenBSE's fan coil mixes none, and its zone's
ventilation enters as if through a separate system with no heat recovery
([#364](https://github.com/bbrannon4/OpenBSE/issues/364)).

## Variable refrigerant flow {#sec-vrf}

**Source:** `openbse-components/src/vrf.rs`
(`VrfOutdoorUnit::coordinate`); `main.rs` (`vrf_zone_loads`).

A VRF system has one outdoor unit and indoor units in several zones. The mode
of each indoor unit is frozen at the start of the timestep from the zone's
load line. The load follows each envelope solve (@sec-hvac-iteration).

**Indoor units.** A unit draws zone air ($T_z$, $W_z$) and meets its zone's
sensible load up to its capacity and what its rated airflow can carry at its
fixed supply temperature (13 °C cooling, 40 °C heating), with $\rho$ and $c_p$
at the zone air state and site pressure. In heating the air leaves at $W_z$.
In cooling the coil dries it along the line to its apparatus dew point (E+
`CalcDoe2DXCoil`): with bypass factor $b$,

$$
T_\text{adp} = \frac{T_\text{sup} - bT_z}{1 - b}, \qquad
W_\text{sup} = \begin{cases}
W_\text{adp} + b(W_z - W_\text{adp}) & W_\text{adp} = W_\text{sat}(T_\text{adp}) < W_z,\\
W_z & \text{otherwise},
\end{cases}
$$ {#eq-vrf-coil}

and the sensible share of the coil's total load is
$\text{SHR} = [h(T_z, W_z) - h(T_\text{sup}, W_z)]/[h(T_z, W_z) - h(T_\text{sup}, W_\text{sup})]$.
$b$ comes from the rated SHR at the AHRI rating point (E+ `CalcCBF`), with the
rated SHR autosized as E+ does, $0.431 + 6086\,\dot V/\dot Q$ with $\dot V/\dot Q$
limited to $4.027\times10^{-5}$–$6.041\times10^{-5}$ m³/s per W. The unit's load
on the outdoor unit is its total:

$$
\dot Q_{s,i} = \min\left(|\dot Q_z|,\ \text{SHR}\,\dot Q_{\text{cap},i},\ \rho\dot V_ic_p|T_\text{sup} - T_z|\right), \qquad
\text{PLR}_i = \frac{\dot Q_{s,i}}{\text{SHR}\,\dot Q_{\text{cap},i}},
$$ {#eq-vrf-unit}

with $\text{SHR} = 1$ in heating.

Without heat recovery, the units in the mode with the smaller total demand
turn off.

**Outdoor unit.** Available capacity is the rated capacity times
$\text{CAP}_{fT}(T_o, \bar T_z)$, with $\bar T_z$ the mean zone temperature of
the operating units. If demand exceeds it, all PLRs in that mode scale down.
Compressor power is

$$
P = \begin{cases}
\dfrac{\dot Q_c}{\text{COP}_c}E_c + \dfrac{\max(0,\ \dot Q_h - \dot Q_c - P_c)}{\text{COP}_h}E_h & \text{heat recovery, both modes},\\[2ex]
\dfrac{\dot Q_c}{\text{COP}_c}E_c \ \text{ or }\ \dfrac{\dot Q_h}{\text{COP}_h}E_h & \text{otherwise},
\end{cases}
$$ {#eq-vrf-power}

where $P_c$ is the first term, and $E_c$, $E_h$ are the EIR-f(T) curve values.
It is shared among the units by PLR.

**Supply.** Each unit's flow is the one that delivers its sensible heat at its
fixed supply temperature, $\dot m = \dot Q_{s,i}/(c_p|T_\text{sup} - T_z|)$, at
$W_\text{sup}$, so the zone's moisture balance sees the coil's condensate and
the supply enthalpy closes against the compressor's total load.

**Comparison with EnergyPlus.** `AirConditioner:VariableRefrigerantFlow`
(`HVACVariableRefrigerantFlow.cc`) runs fixed fan flows with a variable
leaving temperature, the reverse of @eq-vrf-unit
([#180](https://github.com/bbrannon4/OpenBSE/issues/180)). OpenBSE has
none of the following from EnergyPlus:

- part-load EIR curves ([#57](https://github.com/bbrannon4/OpenBSE/issues/57));
- an indoor fan, piping correction, defrost, crankcase heater and operating
  limits ([#365](https://github.com/bbrannon4/OpenBSE/issues/365)).

The coil's latent split follows E+'s DX method at the unit's own fixed leaving
temperature; E+'s VRF coil instead has a variable leaving temperature at fixed
flow and its own capacity-curve inputs.

The curve arguments are $(T_o, \bar T_z)$, the reverse of EnergyPlus's order,
with dry-bulb in place of the indoor wet-bulb for cooling
([#57](https://github.com/bbrannon4/OpenBSE/issues/57)).

## Radiant panels {#sec-radiant-panel}

**Source:** `openbse-components/src/radiant_panel.rs` (`RadiantPanel`);
`main.rs` (`radiant_panel_loads`, `radiant_panel_zone_loads`).

A radiant panel is a hot-water, chilled-water or electric zone heater or
cooler. Its output $\dot Q$ divides by the radiant fraction $f_r$ (default 0.5
for hot water and electric, 0.7 for chilled ceilings):
$\dot Q_\text{conv} = (1 - f_r)\dot Q$ goes to the zone air and
$\dot Q_\text{rad} = f_r\dot Q$ to the surfaces (@sec-internal-gains).

Every panel meets its zone's load to setpoint up to what it can deliver.
Panels in a zone take the load in input order, each up to its capacity
$\dot Q_\text{avail}$:

$$
\dot Q = \min(\dot Q_z,\ \dot Q_\text{avail}), \qquad
\dot Q_\text{avail} =
\begin{cases}
\dot Q_r & \text{electric},\\
\operatorname{clamp}(UA\,(T_w - T_z),\ 0,\ \dot Q_r) & \text{hot water},\\
\operatorname{clamp}(UA\,(T_z - T_w),\ 0,\ \dot Q_r) & \text{chilled water},
\end{cases}
$$ {#eq-radiant-panel}

with $T_w$ the entering water temperature from the panel's plant loop and
$T_z$ the zone air. Without a stated `ua`, the rated capacity $\dot Q_r$ is
taken at the loop's design supply temperature $T_{w,d}$ and E+'s rating air
temperatures: $UA = \dot Q_r/(T_{w,d} - 18)$ for hot water and
$UA = \dot Q_r/(24 - T_{w,d})$ for chilled water; a loop on the wrong side of
its rating air temperature is a severe input error. A water panel's entering
temperature starts at $T_{w,d}$ and then follows its loop's supply.

Electric panels draw $\dot Q$; water panels are loads on their plant loops.

**Comparison with EnergyPlus.** `ZoneHVAC:Baseboard:RadiantConvective:Water`
and `ZoneHVAC:CoolingPanel:RadiantConvective:Water` meet the load up to the
capacity their water temperature allows, as @eq-radiant-panel does. Their
capacity comes from effectiveness with the water flow, sized at rated average
water temperature 87.78 °C and 18 °C air (baseboard) or 5 °C inlet water and
24 °C space (cooling panel). OpenBSE uses $UA\,\Delta T$ on the entering water
with no flow effect, rated at the loop's own design temperature, so a low-temperature
loop's panel keeps its rated capacity at design. EnergyPlus also distributes
radiant output to named surfaces and people by fraction, and limits chilled
panels by the zone dew point; OpenBSE does neither
([#363](https://github.com/bbrannon4/OpenBSE/issues/363)).

## Ground-source heat pumps {#sec-gshp-wshp}

**Source:** `openbse-components/src/gshp.rs` (`GroundSourceHeatPump`).

A ground-source heat pump is an air-loop component that heats or cools toward
an outlet setpoint, by sign, with a 0.1 K deadband. It is sensible only: $W$ is
unchanged.

The entering water temperature $T_e$ is the ground temperature, from a table or
the Kusuda model at the loop depth $d$:

$$
T_e(t) = \bar T - A\,e^{-d\sqrt{\pi/(365\alpha)}}\cos\left[\frac{2\pi}{365}\left(t - t_0 - \frac{d}{2}\sqrt{\frac{365}{\pi\alpha}}\right)\right],
$$ {#eq-kusuda}

with $\alpha$ the soil diffusivity [m²/day], $\bar T$ and $A$ the annual mean
and amplitude. Then

$$
\dot Q = \min\left(\dot m c_p|T_\text{in} - T_\text{sp}|,\ \dot Q_r\,\text{CAP}(T_e, T_\text{in})\right), \qquad
P = \frac{\dot Q\,\text{EIR}(T_e, T_\text{in})}{\text{COP}},
$$ {#eq-gshp}

with each mode's rated capacity, COP and curves. EIR is an energy-input-ratio
modifier: a value above 1 costs more power for the same heat. The unit reports
$\text{PLR} = \dot Q/[\dot Q_r\,\text{CAP}]$, and the loop applies the
compressor cycling penalty at that part load with the loop's
`cooling_part_load_cd` (@eq-dx-rtf). Heating-mode electricity is reported as
heating, cooling-mode electricity as cooling.

A water-source heat pump (`wshp`) is a severe input error: OpenBSE does not
connect one to a water loop, so it would have no source temperature
([#361](https://github.com/bbrannon4/OpenBSE/issues/361)).

**Comparison with EnergyPlus.** `Coil:*:WaterToAirHeatPump:EquationFit` uses
curve fits in normalized load-side and source-side temperatures and air and
water flow ratios, with total and sensible cooling capacities and its own
part-load fraction curve. @eq-gshp keeps E+'s EIR meaning and part-load
penalty, with biquadratics of $T_e$ (x) and the entering air dry-bulb (y).
The unit reads the undisturbed ground temperature, so its rejected heat never
warms the loop, unlike `GroundHeatExchanger:System`
([#362](https://github.com/bbrannon4/OpenBSE/issues/362)).

## CRAC and CRAH units {#sec-crac-crah}

**Source:** `openbse-airloop/src/lib.rs` (`build_crac_signals`,
`build_crah_signals`, `data_center_supply_flow`). CRAC units use the DX coil
of @sec-dx-cooling, with a high `shr`; CRAH units use the chilled-water coil of @sec-chw-coil.

These are cooling-only air loops for data centres. They mix no outdoor air and
their coil's inlet is the zone's return temperature, which includes hot-aisle
heat (@sec-datacenter). In cooling, the coil aims at `cooling_supply_temp`. A
constant-volume fan moves the design flow. A variable-speed fan moves

$$
\dot m = \operatorname{clamp}\left(\frac{\dot Q_c}{c_p\max(T_c - T_\text{sup}, 1)},\ f_\text{min}\dot m_d,\ \dot m_d\right).
$$ {#eq-crac-flow}

If the zone is over its maximum humidity in deadband, the coil aims 0.5 K
below the zone to dehumidify.

**Comparison with EnergyPlus.** EnergyPlus has no CRAC or CRAH object of its
own; `Coil:Cooling:DX:SingleSpeed` or a water coil with
`ZoneHVAC:` or `CoilSystem` control is the usual model, with
`ElectricEquipment:ITE:AirCooled` for the room. The units inherit the coil
differences in @sec-dx-cooling and @sec-chw-coil.

## Known limitations: zone equipment {#sec-zone-equipment-limitations}

- [#365](https://github.com/bbrannon4/OpenBSE/issues/365): VRF has no indoor
  fan, defrost, piping correction, crankcase heater or operating limits.
- [#57](https://github.com/bbrannon4/OpenBSE/issues/57),
  [#180](https://github.com/bbrannon4/OpenBSE/issues/180): VRF part load,
  curve axes and flow control.
- [#364](https://github.com/bbrannon4/OpenBSE/issues/364): one fan-coil
  capacity-control method plus on/off cycling; no fan-coil outdoor-air mixer.
- [#363](https://github.com/bbrannon4/OpenBSE/issues/363): chilled ceilings have
  no condensation limit; radiant panels have no per-surface distribution or
  water-flow effect.
- [#362](https://github.com/bbrannon4/OpenBSE/issues/362): ground-source heat
  pump has no ground-loop heat balance, EquationFit form or latent capacity.
- [#361](https://github.com/bbrannon4/OpenBSE/issues/361): no water-source heat
  pump.
