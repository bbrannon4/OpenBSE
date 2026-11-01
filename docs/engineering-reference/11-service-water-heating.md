# Service water heating {#sec-swh}

A DHW system has one storage water heater, a set of draws, a mains temperature and
an optional pump. It runs once per timestep, separately from the HVAC iteration.

**Source:** `openbse-components/src/water_heater.rs` (`WaterHeater`);
`openbse-io/src/input.rs` (`DhwSystemInput`, `mains_water_temperature`); `main.rs`
("DHW simulation").

## Mains temperature {#sec-mains}

The mains temperature is a constant (default 10 °C) or a correlation in the annual
mean outdoor temperature $\bar T_o$ and the largest difference between monthly means
$\Delta T$, both in °F, and the day of year $d$:

$$
T_\text{mains} = \bar T_o + 6 \pm R\frac{\Delta T}{2}\sin\left[0.986\,(d - 15 - L) - 90°\right], \quad
R = 0.4 + 0.01(\bar T_o - 44), \quad L = 35 - (\bar T_o - 44),
$$ {#eq-mains}

at least 32 °F, with the sign negative in the southern hemisphere. Without
`annual_avg_temp` and `max_monthly_delta`, the statistics come from the weather file.
For Boulder ($\bar T_o$ = 9.95 °C, $\Delta T$ = 23.7 K) it gives a mean of 13.28 °C, a
peak of 18.7 °C in mid-August and a minimum of 7.8 °C in mid-February.

**Comparison with EnergyPlus.** This is `WaterMainsTempFromCorrelation`
[@eplus-engref-25-2, "Water Mains Temperatures"], after Hendron et al., without the
optional multiplier and offset. Omitting the statistics matches
`CorrelationFromWeatherFile`. EnergyPlus also accepts a schedule.

## Draws {#sec-dhw-draws}

Each load has a peak flow [L/s], an optional schedule and a use temperature
$T_\text{use}$. The load mixes tank and mains water at the fixture, so the tank
supplies

$$
\dot V = \sum_\text{loads}\dot V_\text{peak}f(t)\operatorname{clamp}\left(\frac{T_\text{use} - T_\text{mains}}{T - T_\text{mains}},\ 0,\ 1\right),
$$ {#eq-dhw-draw}

with $T$ the tank temperature at the end of the last step. A depleted tank supplies
more water for the same heat, up to the whole fixture flow. A pump, if present, runs
in proportion to the draw with the model of @sec-pumps, and is off otherwise.

**Comparison with EnergyPlus.** @eq-dhw-draw is `CalcEquipmentFlowRates` of
`WaterUse:Equipment`. EnergyPlus can also add sensible and latent gains to the
fixture's zone and has recirculation; OpenBSE has neither (@sec-swh-limitations).

## Storage tank {#sec-water-heater}

The tank is fully mixed at temperature $T$, with mass $m$ (1 kg per litre), a standby
loss coefficient $UA$ and ambient $T_a$, the air of its `ambient_zone` or 20 °C:

$$
m\,c_{p,w}\frac{dT}{dt} = \dot Q - \dot m\,c_{p,w}(T - T_\text{mains}) - UA(T - T_a),
$$ {#eq-tank}

with $c_{p,w} = 4186$ J/(kg·K). For constant heat input $\dot Q$ this is solved in
closed form, with $G = \dot m c_{p,w} + UA$:

$$
T(t) = T_\infty + (T_0 - T_\infty)e^{-Gt/(mc_{p,w})}, \qquad
T_\infty = \frac{\dot Q + \dot m c_{p,w}T_\text{mains} + UA\,T_a}{G}.
$$ {#eq-tank-solution}

The step is split at each control event, the time @eq-tank-solution reaches a
control temperature, so the result doesn't depend on the timestep and is stable at
any draw. The draw heat and standby loss come from the integral of $T$ over the step,
so the tank's energy closes exactly. The standby loss is a gain to the ambient zone.

**Heater.** A gas or electric heater delivers $\dot Q_\text{max} = \eta\dot Q_\text{cap}$
for an input of $\dot Q_\text{cap}$. Its control is one of:

- **On/off** (default): it fires at full output below $T_\text{sp} - \Delta T_d$
  (default 5 K) and stops at $T_\text{sp}$, within the step. With no deadband it holds
  the setpoint, the average of infinitely fast cycling.
- **Modulate:** full output below $T_\text{sp}$, then just enough to hold it.

A constant parasitic power adds to the input and does not heat the tank.

**Comparison with EnergyPlus.** This is `CalcWaterThermalTankMixed` with `CalcTankTemp`,
`CalcTimeNeeded` and `CalcTempIntegral` (`WaterThermalTanks.cc`) [@eplus-engref-25-2,
"Mixed Water Thermal Tank"]. EnergyPlus also has separate on- and off-cycle loss
coefficients, a part-load curve, a parasitic heat fraction, a minimum capacity for
modulating heaters and venting at a maximum temperature. Venting can't occur here: the
only heat source above the setpoint would be a mains or ambient hotter than the tank's
limit.

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

- No fixture gains to zones, no recirculation, no schedule of mains temperatures, and
  no on- and off-cycle loss split, part-load curve or parasitic heat fraction.
- A heat pump's output is held at its start-of-step value within a step, where the
  tank's temperature otherwise follows @eq-tank-solution.
- [#200](https://github.com/bbrannon4/OpenBSE/issues/200): fixed water properties.
