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

- **Electric.** The coil draws $\dot Q/\eta$ of electricity, with $\eta$
  the `efficiency` input (default 1).
- **Gas.** The coil burns $\dot Q/\eta_b$ of fuel, with $\eta_b$ the burner
  efficiency. An on-cycle parasitic electric load $P_\text{par}$ is drawn in
  proportion to firing, $P_\text{par}\dot Q/\dot Q_\text{cap}$. Its heat is
  lost with the flue gas.
- **Hot water.** The coil uses no energy itself. Its output is a load on its
  plant loop (@sec-plant).

**Hot-water coils on a plant loop.** A hot-water coil on a plant loop is a
cross-flow heat exchanger with both streams unmixed (`water_coil.rs`,
`HeatingUa`). With the capacity rates $C_a = \dot m c_p$ and
$C_w = \dot m_wc_{p,w}$, $C_r = C_\text{min}/C_\text{max}$ and
$\text{NTU} = UA/C_\text{min}$, its output at water flow $\dot m_w$ and entering
water temperature $T_{w,\text{in}}$ is

$$
\varepsilon = 1 - \exp\left[\frac{\text{NTU}^{0.22}}{C_r}\left(e^{-C_r\text{NTU}^{0.78}} - 1\right)\right], \qquad
\dot Q(\dot m_w) = \varepsilon\,C_\text{min}\,(T_{w,\text{in}} - T_\text{in}).
$$ {#eq-crossflow-unmixed}

$UA$ follows the flows and temperatures through its convective parts
[@wetter1999]. At the design point it splits into a water side
$UA_{w,d} = UA_d(1 + r)/r$ and an air side $UA_{a,d} = rUA_{w,d}$, with
$r = 0.5$, and

$$
UA = \left[\frac{1}{x_wUA_{w,d}(\dot m_w/\dot m_{w,d})^{0.85}} + \frac{1}{x_aUA_{a,d}(\dot m/\dot m_d)^{0.8}}\right]^{-1},
$$ {#eq-water-coil-ua}

with $x_a = 1 + 4.769\times10^{-3}(T_\text{in} - T_{\text{in},d})$ and
$x_w = 1 + s(T_{w,\text{in}} - T_{w,d})$, $s = 0.014/(1 + 0.014T_{w,d})$.

- **Sizing.** $UA_d$ is solved so the coil delivers its capacity at its design
  point (`size_water_coils` in `main.rs`, after plant sizing): the loop's
  design supply temperature $T_{w,d}$; a design water flow
  $\dot m_{w,d} = \dot Q_\text{cap}/(c_{p,w}\Delta T_d)$ from the loop's design
  temperature difference; and the air its sizing found (@sec-system-sizing),
  the mixed-air temperature and flow at the heating peak. The optional
  `rated_*` inputs override any of these, as `Coil:Heating:Water`'s
  Nominal Capacity method does. Without either, the air is E+'s rated
  16.6 °C to 32.2 °C. A design load that would need an effectiveness above
  0.99 is out of reach of the design temperatures: the coil is sized at 0.99
  and the run warns that it delivers less than its capacity.
- **Control.** The coil's valve modulates $\dot m_w$ to deliver
  $\dot Q_\text{req}$ (`HeatingUa::control`, regula falsi to 0.001 K of
  air). Its most is the smaller of $\dot m_{w,d}$ and the water its loop
  offers. Short of that, it delivers $\dot Q(\dot m_w)$ at full flow. There
  is no cap at the nominal capacity: a coil delivers what its water allows.
- **Water.** The water leaves at
  $T_{w,\text{in}} - \dot Q/(\dot m_wc_{p,w})$, and the coil reports its
  water flow to its loop (@sec-plant-loop), which supplies the
  temperature. A colder loop, a reset or a short boiler therefore lowers the
  coil's output.

A hot-water coil with no plant loop (allowed only in a model without plant
loops) is an ideal coil: @eq-heating-coil with its nominal capacity.

**Comparison with EnergyPlus.** The electric and gas coils match
`Coil:Heating:Electric` and `Coil:Heating:Fuel` with no part-load fuel curve
(`HeatingCoils.cc`). OpenBSE has no part-load curve and no off-cycle
parasitic fuel ([#71](https://github.com/bbrannon4/OpenBSE/issues/71)). The
hot-water coil matches `Coil:Heating:Water` (`WaterCoils.cc`,
`CalcSimpleHeatingCoil`, `CalcAdjustedCoilUA`), with its UA autosized at
the design point as E+'s `WaterCoilUA` sizing does and the valve held to the
setpoint as a `Controller:WaterCoil` does
[@eplus-engref-25-2, "Hot-Water-Based Air Heating Coil"]. Two
differences: E+ sizes the coil on its own design supply-air temperature, and
OpenBSE on the temperature its capacity reaches from the design mixed air,
which is the same point when the capacity is autosized; and a design point
out of reach is a warning here where E+ stops sizing.

## Known limitations: fans, heating coils and ducts {#sec-air-side-limitations-a}

- [#71](https://github.com/bbrannon4/OpenBSE/issues/71): VAV fan density
  and minimum flow fraction, on/off fan curves, gas-coil part-load curve and
  off-cycle parasitics.
- [#72](https://github.com/bbrannon4/OpenBSE/issues/72): Standard 205 fan
  at fixed static pressure; stale power after a failed map query.
- Hot-water and chilled-water coils have no fouling, no `DetailedGeometry`
  model and no humidity-ratio control; a coil's water ΔT at design is its
  loop's, with no per-coil `Design Water Temperature Difference`.
- Ducts have no inward (return-side) leakage; filed for later.

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

**Dry coil.** If $W_\text{ADP} \ge W_\text{in}$, the coil is dry. It raises
the entering humidity ratio at constant dry-bulb toward $W_\text{ADP}$
(relaxation 0.4, 1% tolerance) and re-evaluates the curves at that wet-bulb,
so air far drier than the dry-out point is not read as a weaker coil, with
$\text{SHR} = 1$. With `dry_coil_wet_bulb_iteration: false` it instead scales
the capacity to the sensible part of the path to the ADP,
$\dot Q_\text{eff} = \dot Q_\text{av}\,c_p(T_\text{in} - T_\text{ADP})/(h_\text{in} - h_\text{ADP})$
(the factor clamped to $[0.1, 1]$).

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

with the condenser fan at full power whenever the compressor runs, both
uncorrected for cycling. The cycling penalty is taken at the part load the
compressor actually runs: the loop's cycling fraction $x$ (@sec-controls)
times the coil's own $\text{PLR}_t$,

$$
\text{PLR}_c = x\,\text{PLR}_t, \qquad
\text{RTF} = \frac{\text{PLR}_c}{\text{PLF}(\text{PLR}_c)}, \qquad
\bar P = P\,\frac{x}{\text{PLF}(\text{PLR}_c)},
$$ {#eq-dx-rtf}

with $\text{PLF}$ the coil's `plf_curve`, or else $1 - C_d(1 - \text{PLR})$
with the loop's `cooling_part_load_cd` (default 0.15), floored at 0.7. A
supply fan that cycles with the compressor runs $x/\text{PLF}(x)$ of the
time; with no compressor running it runs $x$. The same applies to every DX
compressor: heat-pump heating coils (their compressor and defrost power,
not the supplemental heater), multi-speed coils cycling their lowest speed,
Standard 205 coils with the file's degradation coefficient, and water-source
heat pumps.

**Comparison with EnergyPlus.** @eq-dx-cap–@eq-dx-plr follow
`Coil:Cooling:DX:SingleSpeed` (`DXCoils.cc`, `CalcCBF`, `CalcDoe2DXCoil`)
[@eplus-engref-25-2, "Single-Speed Electric DX Air Cooling Coil"]. The
differences are ([#237](https://github.com/bbrannon4/OpenBSE/issues/237)):

- **Power.** @eq-dx-rtf is E+'s $P = \dot Q\,\text{EIR}\,\text{RTF}$
  (`CalcDoe2DXCoil`), with the PLR the unitary system gives the coil written
  as the loop's cycling fraction times the coil's own part load.
- **Dry coil.** The wet-bulb iteration is E+'s; the sensible scaling is an
  OpenBSE option E+ does not have.
- **Curves.** EnergyPlus normalizes neither curve: it warns when one is not
  1 at the rating point and requires both. OpenBSE rescales the EIR curve to
  1 there, uses the capacity curve as given, and without curves uses linear
  outdoor dry-bulb modifiers that ignore the entering wet-bulb
  (capacity $1 - 0.008(T_o - 35)$ in $[0.5, 1.05]$, EIR
  $1 + 0.012(T_o - 35)$ in $[0.4, 1.10]$, so COP stops falling above about
  43 °C).
- **Constant SHR.** EnergyPlus has no constant-SHR mode. With
  `autocalculate_shr: false` the total load is the sensible load over the
  rated SHR, so moisture is removed even from air drier than the coil's dew
  point.
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
model applies, with defrost time fraction $f_d$ (default 0.058333). The outdoor
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
P = \left(\frac{\dot Q_r\text{CAP}_{fT}\,m_Q}{\text{COP}}m_P + P_\text{def}\right)\text{PLR},
\end{gathered}
$$ {#eq-hp-heating}

with the cycling penalty of @eq-dx-rtf applied to it. The heat pump delivers
$\text{PLR}\,\dot Q_\text{net}$. The supplemental heater covers any
shortfall up to $\dot Q_\text{sup}$, at a COP of 1. The capacity and EIR
curves take the indoor dry-bulb first and the outdoor dry-bulb second, as
E+'s do.

**Comparison with EnergyPlus.** The defrost model, @eq-hp-frost and
@eq-hp-defrost, matches EnergyPlus's timed defrost (`CalcDXHeatingCoil`)
with a defrost EIR modifier of 1 [@eplus-engref-25-2, "Single-Speed
Electric Heat Pump DX Air Heating Coil"]. The differences are
([#237](https://github.com/bbrannon4/OpenBSE/issues/237)):

- **Part load.** The PLF is the loop's linear $C_d$; the coil has no PLF
  curve input.
- **Defrost load.** EnergyPlus raises the runtime instead of subtracting
  the defrost load from capacity. The two agree to first order and differ
  at full load.
- **Missing inputs.** OpenBSE has no crankcase heater, defrost EIR curve or
  flow-fraction curves, and `rated_airflow` is not used in the simulation.
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
5. **Outlet.** The outlet is held at least 0.1 K above the setpoint and at
   least $10^{-5}$ kg/kg; air past saturation is moved onto the saturation
   line at the same enthalpy. The cooling booked is then the air's own
   change, $\dot m(h_\text{in} - h_\text{out})$ and $\dot m c_p\Delta T$, and
   power is pro-rated by the total delivered.

The file's cycling degradation coefficient is meant to apply below the
minimum sequence. With the linear mapping, that branch is never reached
([#72](https://github.com/bbrannon4/OpenBSE/issues/72)).

**Comparison with EnergyPlus.** EnergyPlus reads RS0004 through
`Coil:DX:ASHRAE205:Performance` (`CoilCoolingDX205Performance`). It cycles
the first sequence below its capacity and interpolates between adjacent
sequences above it. OpenBSE snaps and pro-rates instead, which can under-
or over-deliver
([#72](https://github.com/bbrannon4/OpenBSE/issues/72)).

## Known limitations: DX and heat-pump coils {#sec-air-side-limitations-b}

- Heat-pump heating coils have no PLF curve, crankcase heater, defrost EIR
  curve or flow-fraction curves; DX coils keep linear no-curve defaults, EIR
  normalization and a constant-SHR mode E+ lacks.
- [#72](https://github.com/bbrannon4/OpenBSE/issues/72): Standard 205 DX
  coil never cycles below its minimum sequence; staging snaps.
- [#56](https://github.com/bbrannon4/OpenBSE/issues/56),
  [#57](https://github.com/bbrannon4/OpenBSE/issues/57): the same curve
  conventions in the water-source heat pumps and VRF (@sec-zone-equipment).

## Chilled-water cooling coils {#sec-chw-coil}

**Source:** `openbse-components/src/chw_cooling_coil.rs` (`CoolingCoilCHW`),
`openbse-components/src/water_coil.rs` (`CoolingUa`).

**On a plant loop** the coil is a counterflow coil with an external (air-side)
and an internal (water-side) UA, $UA_i = 3.3\,UA_e$ at the design point, each
following flow and temperature as in @eq-water-coil-ua (exponents 0.8 and
0.85). It is dry or wet by the entering dew point $T_\text{dp}$:

- **Dry** ($T_\text{dp} \le T_{w,\text{in}}$): the counterflow effectiveness on
  $UA = (1/UA_e + 1/UA_i)^{-1}$ and the sensible capacity rates, with $W$
  unchanged.
- **Wet:** the same effectiveness on enthalpy. The water's capacity rate is
  $\dot m_wc_{p,w}/c_\text{sat}$, with $c_\text{sat}$ the slope of the saturated
  enthalpy between $T_{w,\text{in}}$ and $T_\text{dp}$, and the overall
  conductance $UA_h = (c_\text{sat}/UA_i + c_{p,a}/UA_e)^{-1}$. The total is
  $\dot Q_\text{tot} = \varepsilon\,C_{\text{min},h}(h_\text{in} - h_\text{sat}(T_{w,\text{in}}))$.
  The leaving air moves from the entering state toward the surface condition
  at the air-side effectiveness $1 - e^{-UA_e/C_a}$, so it leaves short of
  saturation; if that would pass saturation it is put on the saturation line
  at the same enthalpy.
- **Mostly dry:** when the entering dew point is above the water temperature
  the coil takes the larger of the wet and dry totals. E+'s SimpleAnalysis
  always takes the wet result, which falls below the dry one on a coil that is
  only slightly wet and jumps at the dew point; taking the larger brackets the
  part-wet coil without that jump [@brandemuehl1993].

The total is always the air's enthalpy drop, $\dot m(h_\text{in} - h_\text{out})$,
and the water's heat gain.

- **Sizing.** $UA_e$ is solved (on its logarithm, regula falsi) so the coil
  delivers its nominal total capacity at its design point: the loop's design
  supply temperature, a design water flow
  $\dot Q_\text{cap}/(c_{p,w}\Delta T_d)$, and the mixed-air state and flow at
  the loop's cooling peak (@sec-system-sizing). The inputs
  `design_water_inlet_temp`, `design_water_outlet_temp` and
  `design_water_flow_rate` override the loop's values; without sizing, the
  air is E+'s rated 26.67 °C dry-bulb, 19.44 °C wet-bulb at `rated_airflow`.
  A design load out of reach is sized at a very large UA and warned about.
- **Control.** The valve modulates $\dot m_w$ until the leaving air is at the
  setpoint (0.001 K), up to the design flow or the water the loop offers.
  The leaving humidity follows from the coil, so a warmer chilled-water reset
  removes less moisture.

**Without a plant loop** (allowed only in a model without plant loops) the
coil is the idealised coil of the ASHRAE 140 §11 analytical cases. If the
entering air is wetter than saturation at $T_\text{sp}$, it leaves saturated:

$$
W_{\text{out},f} = \min\left(W_\text{in},\ W_\text{sat}(T_\text{sp})\right), \qquad
\dot Q_{\text{tot},f} = \dot m\left[h_\text{in} - h(T_\text{sp}, W_{\text{out},f})\right].
$$ {#eq-chw-coil}

With $\text{PLR} = \min(\dot Q_\text{cap}/\dot Q_{\text{tot},f},\ 1)$, the coil
delivers $\text{PLR}\,\dot Q_{\text{tot},f}$, with $W_\text{out}$ interpolated by
PLR as in @eq-dx-plr and the temperature from the leaving enthalpy. The `shr`
input applies to DX coils only.

**Comparison with EnergyPlus.** On a plant loop the coil follows
`Coil:Cooling:Water` with Type of Analysis SimpleAnalysis and its default
CounterFlow configuration (`WaterCoils.cc`, `CoolingCoil`,
`CoilCompletelyDry`, `CoilCompletelyWet`, `WetCoilOutletCondition`,
`CalcAdjustedCoilUA`), its UA solved at the design point as `InitWaterCoil`
does [@eplus-engref-25-2, "Chilled-Water-Based Air Cooling Coil"]. The
differences are the larger-of-wet-and-dry rule above and the absence of the
DetailedAnalysis part-wet iteration.

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
differences are ([#345](https://github.com/bbrannon4/OpenBSE/issues/345)):

- **Effectiveness.** EnergyPlus varies it with flow; OpenBSE does not.
- **Control.** EnergyPlus has frost control and economizer lockout, where
  OpenBSE has a fixed ±1 K bypass.

## Humidifiers {#sec-humidifier}

**Source:** `openbse-components/src/humidifier.rs` (`Humidifier`).

An electric steam humidifier has rated power $P_r$ and steam capacity
$\dot m_{s,r} = P_r/\Delta h_s$, with
$\Delta h_s = h_{fg}(100\,°\text{C}) + \bar c_{p,w}(100 - 20) \approx 2.592\times10^6$
J/kg: make-up water at 20 °C to steam at 100 °C, with $h_{fg} = 2.2565\times10^6$
J/kg and $\bar c_{p,w}$ the mean of the water's specific heat at 20 and 100 °C. Its target humidity ratio is either
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
$\dot m_s$, and the humidifier's thermal output is the enthalpy it adds to the
air, $\dot m_sh_g(100\,°\text{C}) = \dot m(h_\text{out} - h_\text{in})$.

**Comparison with EnergyPlus.** @eq-humidifier matches
`Humidifier:Steam:Electric` (`Humidifiers.cc`) [@eplus-engref-25-2,
"Air System Humidifiers"], including the 20 °C make-up water of its rated
capacity (`SizeHumidifier`). EnergyPlus takes the rated capacity and power as
separate inputs and checks them against each other; OpenBSE derives the
capacity from the power.

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
([#345](https://github.com/bbrannon4/OpenBSE/issues/345)):

- **Effectiveness.** EnergyPlus's CelDek pads derive it from the pad depth
  and air velocity.
- **Control.** The research-special coolers can be controlled to a
  setpoint; OpenBSE's cannot.
- **Water.** Drift and blowdown are not modelled.

## Known limitations: other air-side components {#sec-air-side-limitations-c}

- [#345](https://github.com/bbrannon4/OpenBSE/issues/345): heat-recovery
  effectiveness fixed with flow and no frost control; evaporative cooler has no
  setpoint; humidifier capacity always derived from its power.
