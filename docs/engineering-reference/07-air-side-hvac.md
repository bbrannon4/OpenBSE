# Air-side HVAC components {#sec-air-side}

Air-side components sit in an air loop's equipment list or a zone unit, and
are simulated in order along the air path (@sec-graph). Each takes its
inlet air state and mass flow and returns its outlet. Controls
(@sec-controls) set their outlet setpoints and flows before each call.
Enthalpy, $c_p$ and the conversion between temperature and enthalpy follow @eq-enthalpy, with $c_p = c_p(W)$ at the inlet humidity ratio unless stated
otherwise.

## Fans {#sec-fans}

**Source:** `openbse-components/src/fan.rs` (`Fan::calculate`,
`simulate_air`); construction in `openbse-io/src/input.rs`.

**Inputs.** A fan has:

- a design flow $\dot V_d$ [m³/s];
- a pressure rise $\Delta p$ [Pa] (default 600);
- a motor efficiency $\eta_m$ (default 0.9) and an impeller efficiency
  $\eta_i$ (default 0.71), which give the total efficiency
  $\eta_t = \eta_m\eta_i$;
- the motor-in-airstream fraction $f_\text{mot}$ (default 1);
- an optional fixed density $\rho_\text{fix}$.

The power-equation density is $\rho = \rho_\text{fix}$ if given, and
otherwise the inlet air density (@eq-density).

**Power.** For a constant-volume fan,

$$
P = \frac{\dot m\,\Delta p}{\eta_t\,\rho}.
$$ {#eq-fan-cv}

For a VAV fan, with the flow fraction
$x = \min(\dot m/(\rho\dot V_d), 1)$,

$$
P = \left(c_1 + c_2x + c_3x^2 + c_4x^3 + c_5x^4\right)\frac{\rho\dot V_d\,\Delta p}{\eta_t\,\rho}.
$$ {#eq-fan-vav}

The default coefficients are $(0.0407599,\ 0.0880450,\ -0.0729636,\
0.9437398,\ 0)$. An `on_off` fan uses @eq-fan-cv. It cycles through the flow
the loop controls give it (@sec-controls).

**Heat to the air.** The shaft power is $\eta_mP$. All of it, plus the
fraction $f_\text{mot}$ of the motor loss, heats the air:

$$
\dot Q_\text{air} = \eta_mP + (1 - \eta_m)P\,f_\text{mot},
\qquad
h_\text{out} = h_\text{in} + \frac{\dot Q_\text{air}}{\dot m}, \qquad W_\text{out} = W_\text{in}.
$$ {#eq-fan-heat}

**Comparison with EnergyPlus.** @eq-fan-cv–@eq-fan-heat match
`Fan:ConstantVolume` and `Fan:VariableVolume` (`Fans.cc`, `simulateConstant`,
`simulateVAV`) [@eplus-engref-25-2, "Air System Fans"], with these differences
([#71](https://github.com/bbrannon4/OpenBSE/issues/71)):

- **Density.** EnergyPlus always uses the standard density; OpenBSE uses
  the inlet density unless `air_density` is set.
- **Minimum flow fraction.** EnergyPlus evaluates the VAV curve at no less
  than its minimum flow fraction (default 0.25). OpenBSE evaluates it down
  to zero flow.
- **Coefficients.** The EnergyPlus examples use $c_3 = -0.07292612$.
- **On/off fans.** `Fan:OnOff` has power-ratio curves and takes the coil's
  runtime fraction. OpenBSE's on/off fan is the constant-volume fan.

## ASHRAE 205 fans {#sec-fan-a205}

**Source:** `openbse-components/src/fan_a205.rs` (`FanA205`);
`openbse-a205/src/rs0003.rs` (`FanInterpolator`, `FanEfficiencyChain`).

A fan with an `a205_file` uses a Standard 205 RS0003 performance map
[@ashrae205]. Each timestep, the map is interpolated at the standard-air
volume flow $\dot V = \dot m/1.204$ and the design static pressure, giving
the shaft power $P_s$ and the impeller speed $n$. The grid power follows
the drive chain:

- a belt drive (RS0007) with efficiency $\eta_d(P_s)$;
- a motor (RS0005) with efficiency $\eta_m(P, n)$, or the fan's
  `motor_efficiency` if the file has no motor;
- a VFD (RS0006) with efficiency $\eta_v(P, f)$ at the frequency
  $f = n_\text{motor}\,p/2$, for $p$ poles.

Each stage divides by its efficiency:

$$
P = \frac{P_s}{\eta_d\,\eta_m\,\eta_v}, \qquad
\dot Q_\text{air} = P_s + (1 - f_\text{loss})(P - P_s),
$$ {#eq-fan-a205}

with $f_\text{loss}$ the file's `heat_loss_fraction`, the share of the
losses that leaves the airstream. Standalone motor, drive and VFD files
replace the ones embedded in the fan file.

**Comparison with EnergyPlus.** EnergyPlus has no Standard 205 fan. Its
nearest model is `Fan:SystemModel`, which follows a duct static-pressure
curve. OpenBSE always queries the map at the design pressure, which
amounts to constant duct-static-pressure control
([#72](https://github.com/bbrannon4/OpenBSE/issues/72)). After a failed map
query, the previous timestep's power is reported again
([#72](https://github.com/bbrannon4/OpenBSE/issues/72)).

## Heating coils {#sec-heating-coils}

**Source:** `openbse-components/src/heating_coil.rs` (`HeatingCoil`);
construction in `openbse-io/src/input.rs`. Heat-pump heating coils are in @sec-hp-heating.

All three types heat the air toward the outlet setpoint $T_\text{sp}$ and
never cool it. The load is

$$
\dot Q_\text{req} = \max\left(0,\ \dot m c_p(T_\text{sp} - T_\text{in})\right), \qquad
\dot Q = \min(\dot Q_\text{req},\ \dot Q_\text{cap}), \qquad
T_\text{out} = T_\text{in} + \frac{\dot Q}{\dot m c_p},
$$ {#eq-heating-coil}

with $W$ unchanged. The coil types differ as follows:

- **Electric.** The coil draws $\dot Q$ of electricity. The `efficiency`
  input is not read: it is fixed at 1
  ([#234](https://github.com/bbrannon4/OpenBSE/issues/234)).
- **Gas.** The coil burns $\dot Q/\eta_b$ of fuel, with $\eta_b$ the burner
  efficiency. An on-cycle parasitic electric load $P_\text{par}$ is drawn in
  proportion to firing, $P_\text{par}\dot Q/\dot Q_\text{cap}$. Its heat is
  lost with the flue gas.
- **Hot water.** The coil uses no energy itself. Its output is a load on its
  plant loop (@sec-plant).

**Hot-water capacity.** The hot-water coil has two optional limits, but in
the current code neither one ever applies
([#234](https://github.com/bbrannon4/OpenBSE/issues/234)):

- A **water-side limit**, $\dot m_wc_{p,w}(T_{w,\text{in}} - 71)$. The coil
  is never given its water inlet, so this limit is replaced by the nominal
  capacity. As a result, hot-water coil output does not depend on the
  loop's supply temperature or flow.
- A **UA model** (`ua_model: true`), meant to apply a cross-flow
  effectiveness. With no water inlet, the coil falls back to the plain
  model above. The UA model also has two errors: its effectiveness has the
  exponents of the both-unmixed correlation swapped, and its UA is taken
  from the counterflow LMTD at the rated point. Together they would deliver
  86.5% of rated capacity at the rated point.

**Comparison with EnergyPlus.** The electric and gas coils match
`Coil:Heating:Electric` and `Coil:Heating:Fuel` with no part-load fuel curve
(`HeatingCoils.cc`). OpenBSE has no part-load curve and no off-cycle
parasitic fuel ([#71](https://github.com/bbrannon4/OpenBSE/issues/71)).
`Coil:Heating:Water` (`WaterCoils.cc`, `CalcSimpleHeatingCoil`) instead
computes the output from the cross-flow both-unmixed effectiveness

$$
\varepsilon = 1 - \exp\left[\frac{\text{NTU}^{0.22}}{C_r}\left(e^{-C_r\text{NTU}^{0.78}} - 1\right)\right],
$$ {#eq-crossflow-unmixed}

the water inlet temperature and both flows. Its UA is solved so that the
model meets the rated capacity [@eplus-engref-25-2, "Hot-Water-Based Air Heating Coil"]. In
EnergyPlus, the loop temperature and flow therefore limit the coil
([#234](https://github.com/bbrannon4/OpenBSE/issues/234)).

## Ducts {#sec-ducts}

**Source:** `openbse-components/src/duct.rs` (`Duct`); `main.rs`
(`simulate_loop_components`, and "Duct distribution losses → ambient
zone").

A duct of length $L$, diameter $D$ and wall U-value $U$ exchanges heat with
its ambient temperature $T_a$. Its effectiveness is that of a stream
against a constant-temperature reservoir:

$$
\varepsilon = 1 - \exp\left(-\frac{U\pi DL}{\dot m c_p}\right), \qquad
T_\text{out} = T_\text{in} - \varepsilon(T_\text{in} - T_a), \qquad
\dot Q_\text{cond} = \dot m c_p(T_\text{in} - T_\text{out}).
$$ {#eq-duct}

A fraction $f_\text{leak}$ of the inlet mass flow leaks out, so
$\dot m_\text{out} = (1 - f_\text{leak})\dot m$, with the outlet state
unchanged by the leak.

The ambient is set by `ambient_zone`:

- `outdoor`: the outdoor dry-bulb;
- `ground`: a fixed 18 °C, not the site ground temperature
  ([#235](https://github.com/bbrannon4/OpenBSE/issues/235));
- a zone name: that zone's air temperature. An unknown name silently uses
  the outdoor temperature
  ([#235](https://github.com/bbrannon4/OpenBSE/issues/235)).

**Losses to the ambient zone.** When the ambient is a zone, the losses
return to it as gains, multiplied by the duct's zone multiplier:

- sensible: $\dot Q_\text{cond} + \dot m_\text{leak}c_p(T_\text{in} - T_z)$;
- latent: $\dot m_\text{leak}(W_\text{in} - W_z)h_g(T_z)$, with $h_g$ the vapour
  enthalpy of @sec-psychrometrics.

The duct reads that zone's temperature within the HVAC iteration
(@sec-hvac-iteration), so the coupling converges with it.

**Comparison with EnergyPlus.** EnergyPlus's `Duct` object is lossless. Duct
losses need AirflowNetwork distribution, whose duct conduction has the form
of @eq-duct [@eplus-engref-25-2, "AirflowNetwork Model"]. OpenBSE adds
this as a component. It has no inward (return-side) leakage
([#235](https://github.com/bbrannon4/OpenBSE/issues/235)).

## Known limitations: fans, heating coils and ducts {#sec-air-side-limitations-a}

- [#71](https://github.com/bbrannon4/OpenBSE/issues/71): VAV fan density
  and minimum flow fraction, on/off fan curves, gas-coil part-load curve and
  off-cycle parasitics.
- [#72](https://github.com/bbrannon4/OpenBSE/issues/72): Standard 205 fan
  at fixed static pressure; stale power after a failed map query.
- [#234](https://github.com/bbrannon4/OpenBSE/issues/234): hot-water coils
  never see their water inlet; the UA model is inert and has swapped
  exponents; electric-coil efficiency is not read.
- [#235](https://github.com/bbrannon4/OpenBSE/issues/235): duct ground
  ambient fixed at 18 °C, unknown ambient zones, no inward leakage.

## DX cooling coils {#sec-dx-cooling}

**Source:** `openbse-components/src/cooling_coil.rs` (`CoolingCoilDX`:
`available_capacity`, `available_cop`, `initialize_bypass_factor`,
`simulate_air`); cycling energy in `main.rs` (system part-load scaling).

**Inputs.** A single-speed DX coil has:

- a rated total capacity $\dot Q_r$, a COP (default 3.5) and an SHR (default
  0.8), all at the AHRI point: 26.67 °C dry-bulb and 19.44 °C wet-bulb
  entering, 35 °C outdoor;
- a rated airflow $\dot V_r$;
- optional curves: $\text{CAP}_{fT}(T_{wb,i}, T_o)$ and
  $\text{EIR}_{fT}(T_{wb,i}, T_o)$, $\text{CAP}_{ff}(x)$ and
  $\text{EIR}_{ff}(x)$ of the flow fraction $x = \dot m/(\rho\dot V_r)$, and a
  part-load curve $\text{PLF}(\text{PLR})$;
- an optional condenser-fan power $P_\text{cf}$.

**Capacity and efficiency.** At the entering wet-bulb $T_{wb,i}$ and the
outdoor dry-bulb $T_o$,

$$
\dot Q_\text{av} = \dot Q_r\,\text{CAP}_{fT}\,\text{CAP}_{ff}, \qquad
\text{COP} = \frac{\text{COP}_r}{n\,\text{EIR}_{fT}\,\text{EIR}_{ff}}, \qquad
n = \frac{1}{\text{EIR}_{fT}(19.44, 35)}.
$$ {#eq-dx-cap}

The factor $n$ rescales the EIR curve to 1 at the rated point. The capacity
curve is not rescaled. Without curves, two linear modifiers stand in for
them: $1 - 0.008(T_o - 35)$ for capacity, clamped to $[0.5, 1.05]$, and
$1 + 0.012(T_o - 35)$ for EIR, clamped to $[0.4, 1.10]$.

**Bypass factor.** With `autocalculate_shr` (the default), the coil follows
the apparatus-dew-point method. At the rated point, the condition line from
the entering state has slope $\mathrm dh/\mathrm dT = c_p/\text{SHR}_r$.
Its intersection with the saturation curve, found by bisection, is the
rated ADP. The bypass factor is

$$
\text{BF} = \frac{h_{\text{out},r} - h_{\text{ADP},r}}{h_{\text{in},r} - h_{\text{ADP},r}},
\qquad h_{\text{out},r} = h_{\text{in},r} - \frac{\dot Q_r}{\rho\dot V_r},
$$ {#eq-dx-bf}

clamped to $[0, 0.9]$. Each timestep, the full-load ADP and SHR follow from
the current state:

$$
h_\text{ADP} = h_\text{in} - \frac{\dot Q_\text{av}/\dot m}{1 - \text{BF}}, \qquad
T_\text{ADP} = T_\text{sat}(h_\text{ADP}), \qquad
\text{SHR} = \min\left(\frac{h(T_\text{in}, W_\text{ADP}) - h_\text{ADP}}{h_\text{in} - h_\text{ADP}},\ 1\right).
$$ {#eq-dx-adp}

**Dry coil.** If $W_\text{ADP} \ge W_\text{in}$, the coil is dry. By
default its capacity is scaled to the sensible part of the path to the ADP,
$\dot Q_\text{eff} = \dot Q_\text{av}\,c_p(T_\text{in} - T_\text{ADP})/(h_\text{in} - h_\text{ADP})$
(the factor clamped to $[0.1, 1]$), with $\text{SHR} = 1$. With
`dry_coil_wet_bulb_iteration`, it instead raises the entering humidity
ratio at constant dry-bulb toward $W_\text{ADP}$ (relaxation 0.4, 1%
tolerance) and re-evaluates the curves at that wet-bulb.

**Part load and outlet.** The coil meets the sensible load to its outlet
setpoint:

$$
\begin{gathered}
\text{PLR} = \min\left(\frac{\dot m c_p(T_\text{in} - T_\text{sp})}{\text{SHR}\,\dot Q_\text{eff}},\ 1\right), \qquad
T_\text{out} = T_\text{in} - \frac{\text{PLR}\,\text{SHR}\,\dot Q_\text{eff}}{\dot m c_p},\\
W_\text{out} = W_\text{in} - \text{PLR}\left(W_\text{in} - W_{\text{out},f}\right), \qquad
W_{\text{out},f} = W\!\left(T_\text{in},\ h_\text{in} - (1 - \text{SHR})\dot Q_\text{eff}/\dot m\right).
\end{gathered}
$$ {#eq-dx-plr}

The delivered total is $\text{PLR}\,\dot Q_\text{eff}$. With
`autocalculate_shr: false`, the SHR is the rated one, and the latent part
$\dot Q(1/\text{SHR} - 1)$ is removed as $\Delta W = \dot Q_\text{lat}/(\dot m h_g)$,
with $h_g$ at the outlet temperature, whatever the entering humidity.

**Power and cycling.** The coil's power is

$$
P = \frac{\dot Q_\text{av}\,\text{PLR}_t}{\text{COP}} + P_\text{cf}, \qquad
\text{PLR}_t = \frac{\dot Q_\text{tot}}{\dot Q_\text{av}},
$$ {#eq-dx-power}

with the condenser fan at full power whenever the compressor runs. The
coil reports $\text{RTF} = \text{PLR}/\text{PLF}(\text{PLR})$, but that
value does not reach the energy. When the system cycles the coil, the loop
scales compressor and condenser-fan power by
$\text{RTF}_\text{loop} = x/(1 - C_d(1 - x))$, with $x$ the loop part-load
ratio and $C_d$ the loop's `cooling_part_load_cd` (default 0.15)
(@sec-controls).

**Comparison with EnergyPlus.** @eq-dx-cap–@eq-dx-plr follow
`Coil:Cooling:DX:SingleSpeed` (`DXCoils.cc`, `CalcCBF`, `CalcDoe2DXCoil`)
[@eplus-engref-25-2, "Single-Speed Electric DX Air Cooling Coil"]. The
differences are ([#237](https://github.com/bbrannon4/OpenBSE/issues/237)):

- **Power.** EnergyPlus computes $P = \dot Q\,\text{EIR}\,\text{RTF}$ inside
  the coil, from the coil's own PLF curve, whatever drives the part load.
  OpenBSE's coil PLF curve never affects energy, and part load reached
  through the outlet setpoint carries no cycling penalty.
- **Dry coil.** EnergyPlus always uses the wet-bulb iteration, which
  OpenBSE offers only as an option.
- **Curves.** EnergyPlus normalizes neither curve and requires both.
  OpenBSE rescales the EIR curve and has linear defaults.
- **Constant SHR.** EnergyPlus has no constant-SHR mode.
- **Condenser fan.** In EnergyPlus the single-speed coil's EIR includes the
  outdoor fan. $P_\text{cf}$ is OpenBSE's own addition.

## Heat-pump heating coils {#sec-hp-heating}

**Source:** `openbse-components/src/heat_pump_coil.rs`
(`HeatPumpHeatingCoil`: `available_capacity`, `available_cop`,
`defrost_adjustments`, `simulate_air`).

**Inputs.** A heat-pump heating coil has:

- a rated capacity $\dot Q_r$ and COP at the AHRI 8.33 °C outdoor point;
- optional curves $\text{CAP}_{fT}(T_o, T_i)$ and $\text{EIR}_{fT}(T_o, T_i)$
  of the outdoor and entering dry-bulbs, in that order;
- defrost inputs;
- supplemental electric capacity $\dot Q_\text{sup}$ and a compressor lockout
  temperature (default −17.78 °C).

Without curves, capacity falls 2% and COP 2.4% per kelvin below 8.33 °C,
clamped to $[0.2, 1.15]$ and $[0.3, 1.10]$. Neither rises above the rating
point.

**Defrost.** Below `defrost_onset_temp` (default 5 °C), the timed-defrost
model applies, with defrost time fraction $f_d$ (default 0.08). The outdoor
coil temperature and frost humidity depression are

$$
T_\text{coil} = 0.82T_o - 8.589, \qquad
\Delta W = \max\left(10^{-6},\ W_o - W_\text{sat}(T_\text{coil})\right),
$$ {#eq-hp-frost}

and they give the multipliers and loads

$$
\begin{gathered}
m_Q = 0.909 - 107.33\Delta W, \qquad m_P = 0.90 - 36.45\Delta W,\\
\dot Q_\text{def} = 0.01f_d(7.222 - T_o)\frac{\dot Q_r}{1.01667}, \qquad
P_\text{def} = \begin{cases}f_d\,\dot Q_r/1.01667 & \text{reverse cycle},\\ f_dP_\text{heater} & \text{resistive.}\end{cases}
\end{gathered}
$$ {#eq-hp-defrost}

**Operation.** Above the lockout temperature,

$$
\begin{gathered}
\dot Q_\text{net} = \dot Q_r\text{CAP}_{fT}\,m_Q - \dot Q_\text{def}, \qquad
\text{PLR} = \min\left(\frac{\dot m c_p(T_\text{sp} - T_\text{in})}{\dot Q_\text{net}},\ 1\right),\\
\text{RTF} = \frac{\text{PLR}}{1 - 0.15(1 - \text{PLR})}, \qquad
P = \frac{\dot Q_r\text{CAP}_{fT}\,m_Q}{\text{COP}}m_P\,\text{RTF} + P_\text{def}\,\text{RTF}.
\end{gathered}
$$ {#eq-hp-heating}

The heat pump delivers $\text{PLR}\,\dot Q_\text{net}$. The supplemental
heater covers any shortfall up to $\dot Q_\text{sup}$, at a COP of 1.

**Comparison with EnergyPlus.** The defrost model, @eq-hp-frost and
@eq-hp-defrost, matches EnergyPlus's timed defrost (`CalcDXHeatingCoil`)
with a defrost EIR modifier of 1 [@eplus-engref-25-2, "Single-Speed
Electric Heat Pump DX Air Heating Coil"]. The differences are
([#237](https://github.com/bbrannon4/OpenBSE/issues/237)):

- **Curve arguments.** EnergyPlus's biquadratic curves take the indoor
  dry-bulb first. A biquadratic copied from an EnergyPlus file is therefore
  evaluated with its axes swapped.
- **Part load.** The PLF is fixed at $C_d = 0.15$, with no curve input.
- **Defrost load.** EnergyPlus raises the runtime instead of subtracting
  the defrost load from capacity.
- **Defaults and missing inputs.** EnergyPlus's default defrost time
  fraction is 0.058333. OpenBSE has no crankcase heater, defrost EIR curve
  or flow-fraction curves.
- **Supplemental heat.** EnergyPlus models the supplemental heater as a
  separate coil.

## ASHRAE 205 DX cooling coils {#sec-dx-a205}

**Source:** `openbse-components/src/cooling_coil_a205.rs`
(`CoolingCoilDXA205`); `openbse-a205/src/rs0004.rs`.

A DX coil with an `a205_file` uses a Standard 205 RS0004 map [@ashrae205].
The map gives the gross total and sensible capacity and the power as
functions of six variables: the outdoor dry-bulb, the entering dry-bulb and
relative humidity, the air mass flow, the compressor sequence number and
the pressure. Its nominal capacity is the full-sequence value at 35 °C
outdoor and 26.7 °C, 50% RH entering, at the middle mass flow of the grid.

Each timestep, the coil goes through these steps:

1. **Load.** It finds the PLR from the sensible load and the full-sequence
   sensible capacity.
2. **Sequence.** It maps the PLR linearly onto the sequence range. A
   discrete map rounds to the nearest sequence.
3. **Output.** It re-queries the map at that sequence and delivers at most
   the sensible load. Total capacity and power are pro-rated by the
   delivered fraction.
4. **Latent.** It removes the latent part as
   $\Delta W = \dot Q_\text{lat}/(\dot m h_g)$, with $h_g$ at the outlet temperature.

The file's cycling degradation coefficient is meant to apply below the
minimum sequence. With the linear mapping, that branch is never reached
([#72](https://github.com/bbrannon4/OpenBSE/issues/72)).

**Comparison with EnergyPlus.** EnergyPlus reads RS0004 through
`Coil:DX:ASHRAE205:Performance` (`CoilCoolingDX205Performance`). It cycles
the first sequence below its capacity and interpolates between adjacent
sequences above it. OpenBSE snaps and pro-rates instead, which can under-
or over-deliver
([#72](https://github.com/bbrannon4/OpenBSE/issues/72)). It also books more
cooling than its air stream receives when the outlet is clamped
([#168](https://github.com/bbrannon4/OpenBSE/issues/168)).

## Known limitations: DX and heat-pump coils {#sec-air-side-limitations-b}

- [#237](https://github.com/bbrannon4/OpenBSE/issues/237): DX cycling
  penalty applied at loop level with a linear $C_d$ (coil PLF curves
  unused, none for setpoint part load); heat-pump curve axes swapped; dry
  coil, default curves, EIR normalization and constant-SHR mode differ from
  EnergyPlus.
- [#72](https://github.com/bbrannon4/OpenBSE/issues/72): Standard 205 DX
  coil never cycles below its minimum sequence; staging snaps.
- [#168](https://github.com/bbrannon4/OpenBSE/issues/168): Standard 205 DX
  coil books more cooling than its outlet shows.
- [#56](https://github.com/bbrannon4/OpenBSE/issues/56),
  [#57](https://github.com/bbrannon4/OpenBSE/issues/57): the same curve
  conventions in the water-source heat pumps and VRF (@sec-zone-equipment).

## Chilled-water cooling coils {#sec-chw-coil}

**Source:** `openbse-components/src/chw_cooling_coil.rs` (`CoolingCoilCHW`).

The coil cools the air to its outlet setpoint $T_\text{sp}$. If the
entering air is wetter than saturation at $T_\text{sp}$, it leaves
saturated:

$$
W_{\text{out},f} = \min\left(W_\text{in},\ W_\text{sat}(T_\text{sp})\right), \qquad
\dot Q_{\text{tot},f} = \dot m\left[h_\text{in} - h(T_\text{sp}, W_{\text{out},f})\right], \qquad
\dot Q_{s,f} = \dot m c_p(T_\text{in} - T_\text{sp}).
$$ {#eq-chw-coil}

The capacity limit is the nominal capacity $\dot Q_\text{cap}$, a total
(sensible plus latent) value. With
$\text{PLR} = \min(\dot Q_\text{cap}/\dot Q_{\text{tot},f},\ 1)$, the coil
delivers $\text{PLR}\,\dot Q_{\text{tot},f}$ and $\text{PLR}\,\dot Q_{s,f}$,
with $W_\text{out}$ interpolated by PLR as in @eq-dx-plr. Its total output
is a load on its chilled-water loop (@sec-plant).

The code also has a water-side limit,
$\dot m_wc_{p,w}(T_{w,\text{out},d} - T_{w,\text{in}})$. As with hot-water
coils, the coil is never given its water inlet, so this limit never applies,
and coil output does not depend on the chilled-water temperature or flow
([#239](https://github.com/bbrannon4/OpenBSE/issues/239)). The `shr` input
is not read.

**Comparison with EnergyPlus.** EnergyPlus `Coil:Cooling:Water`
(`WaterCoils.cc`) computes the outlet from the coil UA, the water inlet
temperature and flow, and the wet and dry parts of the coil, so the leaving
air is generally short of saturation [@eplus-engref-25-2, "Chilled-Water-Based Air Cooling Coil"].
Forcing saturation reproduces the ASHRAE 140 analytical cases, but
overstates latent removal for typical coils
([#239](https://github.com/bbrannon4/OpenBSE/issues/239)).

## Air-to-air heat recovery {#sec-heat-recovery}

**Source:** `openbse-components/src/heat_recovery.rs` (`HeatRecovery`).

An enthalpy wheel or a plate exchanger transfers heat, and for the wheel
moisture, from the exhaust air at $(T_x, W_x)$ to the outdoor air entering
it. The driver sets the exhaust state from the return air each timestep.
Let:

- $C_s = \dot m c_p$ be the supply capacity rate;
- $r$ be the exhaust-to-supply flow ratio (default 1, capped at 1), so that
  $C_\text{min} = rC_s$;
- $\varepsilon_s$ be the sensible effectiveness, or the separate cooling
  effectiveness when $T_\text{in} > T_x$;
- $\varepsilon_l$ be the latent effectiveness (zero for a plate exchanger).

Then

$$
T_\text{out} = T_\text{in} + f\,\varepsilon_s\frac{C_\text{min}}{C_s}(T_x - T_\text{in}), \qquad
W_\text{out} = W_\text{in} + f\,\varepsilon_l\,r\,(W_x - W_\text{in}),
$$ {#eq-heat-recovery}

where $f$ is the control fraction. With no supply-outlet setpoint, $f = 1$.
With one, as for E+'s `ControlToTemperatureSetPoint`, $f$ throttles the
unit so that the outlet lands on the setpoint when full recovery would cross
it. It is 1 when full recovery only moves the outlet toward the setpoint,
and 0 when recovery would move it away. The unit is bypassed when
$|T_\text{in} - T_x| < 1$ K. Its parasitic power is drawn whenever it is
not bypassed.

**Comparison with EnergyPlus.** @eq-heat-recovery is
`HeatExchanger:AirToAir:SensibleAndLatent` (`HeatRecovery.cc`,
`CalcAirToAirGenericHeatExch`) at 100% flow [@eplus-engref-25-2, "Air System
Air-To-Air Sensible and Latent Effectiveness Heat Exchanger"]. The
differences are ([#239](https://github.com/bbrannon4/OpenBSE/issues/239)):

- **Effectiveness.** EnergyPlus varies it with flow; OpenBSE does not.
- **Control.** EnergyPlus has frost control and economizer lockout, where
  OpenBSE has a fixed ±1 K bypass.

## Humidifiers {#sec-humidifier}

**Source:** `openbse-components/src/humidifier.rs` (`Humidifier`).

An electric steam humidifier has rated power $P_r$ and steam capacity
$\dot m_{s,r} = P_r/\Delta h_s$, with $\Delta h_s = 2.6157\times10^6$ J/kg
(water at 14.4 °C to steam at 100 °C). Its target humidity ratio is either
a fixed `w_setpoint`, or the humidity ratio at `min_rh_setpoint` and the
served zone's live air temperature. Below the target,

$$
\begin{gathered}
\dot m_s = \min\left(\dot m(W_\text{target} - W_\text{in}),\ \dot m_{s,r}\right), \qquad
P = P_r\frac{\dot m_s}{\dot m_{s,r}} + P_\text{standby},\\
W_\text{out} = W_\text{in} + \frac{\dot m_s}{\dot m}, \qquad
h_\text{out} = h_\text{in} + \frac{\dot m_s}{\dot m}h_g(100\,°\text{C}),
\end{gathered}
$$ {#eq-humidifier}

with $h_g(100\,°\text{C}) = 2.50094\times10^6 + 1858.95\times100$ J/kg, the
vapour enthalpy on the basis of @eq-enthalpy. The make-up water is
$\dot m_s$.

**Comparison with EnergyPlus.** @eq-humidifier matches
`Humidifier:Steam:Electric` (`Humidifiers.cc`) [@eplus-engref-25-2,
"Air System Humidifiers"]. EnergyPlus heats the water from 20 °C, which takes about 1%
less energy per kilogram than the 14.4 °C here; the code comment says the
two are the same ([#239](https://github.com/bbrannon4/OpenBSE/issues/239)).

## Evaporative coolers {#sec-evap-cooler}

**Source:** `openbse-components/src/evap_cooler.rs` (`EvapCooler`).

**Direct stage.** With saturation effectiveness $\varepsilon$ (default 0.8),
the air is cooled toward its wet-bulb at constant enthalpy:

$$
T_\text{out} = T_\text{in} - \varepsilon(T_\text{in} - T_{wb,\text{in}}), \qquad
W_\text{out} = \max\left(W(T_\text{out}, h_\text{in}),\ W_\text{in}\right).
$$ {#eq-evap-direct}

**Indirect stage.** The secondary air is outdoor air. It is first cooled by
its own pad, as in @eq-evap-direct with the same $\varepsilon$, to
$T_{2,\text{out}}$. It then cools the supply sensibly through an exchanger
of effectiveness $\varepsilon_\text{hx}$ (default 0.7):

$$
T_\text{out} = T_\text{in} - \varepsilon_\text{hx}(T_\text{in} - T_{2,\text{out}}), \qquad W_\text{out} = W_\text{in},
$$ {#eq-evap-indirect}

and does nothing if $T_{2,\text{out}} \ge T_\text{in}$. A two-stage cooler
runs the indirect stage, then the direct stage.

**Energy and water.** The cooler always runs at full effectiveness, with no
outlet setpoint. While it cools, it draws its pump power (default 100 W),
plus the secondary fan power while an indirect stage runs. Its water use is
the moisture added to the supply plus that added to the secondary air, at
the secondary flow ratio.

**Comparison with EnergyPlus.** @eq-evap-direct is
`EvaporativeCooler:Direct:ResearchSpecial` at a fixed effectiveness.
@eq-evap-indirect is the two-step form of
`EvaporativeCooler:Indirect:CelDekPad` (`EvaporativeCoolers.cc`)
[@eplus-engref-25-2, "Evaporative Coolers"]. The differences are
([#239](https://github.com/bbrannon4/OpenBSE/issues/239)):

- **Effectiveness.** EnergyPlus's CelDek pads derive it from the pad depth
  and air velocity.
- **Control.** The research-special coolers can be controlled to a
  setpoint; OpenBSE's cannot.
- **Water.** Drift and blowdown are not modelled.

## Known limitations: other air-side components {#sec-air-side-limitations-c}

- [#239](https://github.com/bbrannon4/OpenBSE/issues/239): chilled-water
  coil's water side never connected, leaving air forced to saturation,
  `shr` unread; humidifier reference temperature; heat-recovery effectiveness
  fixed with flow and no frost control; evaporative cooler has no setpoint.
