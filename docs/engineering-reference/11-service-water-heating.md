# Service water heating {#sec-swh}

A DHW system has one storage water heater, a set of draws, a mains temperature and
an optional pump. It runs once per timestep, separately from the HVAC iteration.

**Source:** `openbse-components/src/water_heater.rs` (`WaterHeater`);
`openbse-io/src/input.rs` (`DhwSystemInput`, `mains_water_temperature`); `main.rs`
("DHW simulation").

## Mains temperature {#sec-mains}

The mains temperature is a constant or a correlation in the annual mean outdoor
temperature $\bar T_o$, the largest difference between monthly means $\Delta T$, the
latitude $\phi$ and the day of year $d$:

$$
T_\text{mains} = \bar T_o + 6 + 0.4127\frac{\Delta T}{2}\sin\left[\frac{2\pi}{365}\left(d - \left(35 - \frac{\phi}{2}\right) - 15\right)\right].
$$ {#eq-mains}

The default constant is 10 °C.

**Comparison with EnergyPlus.** EnergyPlus's correlation (`CalcWaterMainsTemp`),
after Hendron et al. [@eplus-engref-25-2, "Water Mains Temperatures"], is in °F:

$$
T_\text{mains} = \bar T_o + 6 + R\frac{\Delta T}{2}\sin\left[0.986\,(d - 15 - L) - 90°\right], \quad
R = 0.4 + 0.01(\bar T_o - 44), \quad L = 35 - (\bar T_o - 44).
$$ {#eq-mains-eplus}

@eq-mains is a mistranscription. Its offset is 6 K rather than 6 °F, its ratio and
lag are different, and it has no 90° phase shift. For Boulder, @eq-mains has a mean
of 15.95 °C peaking on 1 May, where @eq-mains-eplus has a mean of 13.28 °C peaking in
mid-August. Heating water to 60 °C then takes 5.7% less energy over the year
([#258](https://github.com/bbrannon4/OpenBSE/issues/258)).

## Draws {#sec-dhw-draws}

Each load has a peak flow [L/s], an optional schedule and a use temperature
$T_\text{use}$. The load mixes tank and mains water at the fixture, so the tank
supplies

$$
\dot V = \sum_\text{loads}\dot V_\text{peak}f(t)\operatorname{clamp}\left(\frac{T_\text{use} - T_\text{mains}}{T_\text{sp} - T_\text{mains}},\ 0,\ 1\right).
$$ {#eq-dhw-draw}

A pump, if present, runs in proportion to the draw with the model of @sec-pumps, and
is off otherwise.

**Comparison with EnergyPlus.** `WaterUse:Equipment` mixes at the actual hot-water
temperature, and adds sensible and latent gains to its zone. @eq-dhw-draw uses the
setpoint, and has no fixture gains or recirculation
([#258](https://github.com/bbrannon4/OpenBSE/issues/258)).

## Storage tank {#sec-water-heater}

The tank is fully mixed at temperature $T$, with mass $m$ (1 kg per litre), a standby
loss coefficient $UA$ and ambient $T_a$, the air of its `ambient_zone` or 20 °C. Each
timestep,

$$
\begin{gathered}
\dot Q_\text{draw} = \dot m\,c_{p,w}\max(T - T_\text{mains},\ 0), \qquad
\dot Q_\text{loss} = UA(T - T_a),\\
T^{t+\Delta t} = T + \frac{(\dot Q_\text{in} - \dot Q_\text{draw} - \dot Q_\text{loss})\Delta t}{m\,c_{p,w}},
\end{gathered}
$$ {#eq-tank}

with $c_{p,w} = 4186$ J/(kg·K). The tank's standby loss is a gain to its ambient zone.

**Heater.** A gas or electric heater delivers $\dot Q_\text{max} = \eta\dot Q_\text{cap}$
for an input of $\dot Q_\text{cap}$. Its control is one of:

- **On/off** (default): it fires below $T_\text{sp} - \Delta T_d$ (default 5 K) and
  stops at $T_\text{sp}$. If it would reach the setpoint within the step, it runs
  only for the fraction of the step needed,
  $(T_\text{sp} - T)mc_{p,w}/[(\dot Q_\text{max} - \dot Q_\text{draw} - \dot Q_\text{loss})\Delta t]$.
- **Modulate:** it matches $\dot Q_\text{draw} + \dot Q_\text{loss}$, up to its maximum.

A constant parasitic power adds to the input and does not heat the tank.

**Comparison with EnergyPlus.** @eq-tank is the energy balance of
`WaterHeater:Mixed` (`WaterThermalTanks.cc`) [@eplus-engref-25-2, "Mixed Water Thermal
Tank"], which integrates it analytically. The explicit form here becomes unstable
when $\dot m\Delta t/m > 1$. EnergyPlus also has on- and off-cycle loss coefficients, a
part-load curve and a parasitic heat fraction
([#258](https://github.com/bbrannon4/OpenBSE/issues/258)).

## Heat-pump water heaters {#sec-hpwh}

With `fuel_type: heat_pump`, $\eta$ is the rated COP and $\dot Q_\text{cap}$ the rated
compressor input. At the evaporator inlet wet-bulb $T_{wb}$ (the ambient air) and the
tank temperature,

$$
\dot Q_\text{max} = \dot Q_\text{cap}\,\text{COP}_r\,f_Q(T_{wb}, T), \qquad
\text{COP} = \max\left(\text{COP}_r f_C(T_{wb}, T),\ 1\right), \qquad
P = \frac{\dot Q_\text{max}}{\text{COP}},
$$ {#eq-hpwh}

with $f_Q$ and $f_C$ biquadratic curves, by default EnergyPlus's `HPWHHeatingCapFTemp`
and `HPWHHeatingCOPFTemp` rescaled to 1 at the rating point (13.5 °C wet-bulb, 57.5 °C
water).

**Evaporator.** It takes $\dot Q_\text{evap} = \dot Q_\text{in} - P$ from the ambient
zone's air, split by the DX bypass-factor method (@sec-dx-cooling) at a flow of
$5.035\times10^{-5}$ m³/s per watt of rated heating, with a rated SHR of 0.85. The
sensible and latent parts are losses to the zone.

**Lockout.** Below `min_inlet_air_temp` (default 10 °C), the compressor stops and only
a backup resistance element heats the tank.

**Comparison with EnergyPlus.** @eq-hpwh follows
`Coil:WaterHeating:AirToWaterHeatPump:Pumped` [@eplus-engref-25-2, "Heat Pump Water
Heater"], with the same curves, evaporator airflow, SHR and cut-out defaults. The tank
is mixed, where EnergyPlus also offers a stratified tank.

## Known limitations {#sec-swh-limitations}

- [#258](https://github.com/bbrannon4/OpenBSE/issues/258): mains correlation
  mistranscribed; explicit tank integration; hot fraction at the setpoint; no fixture
  gains or recirculation.
- [#200](https://github.com/bbrannon4/OpenBSE/issues/200): fixed water properties.
