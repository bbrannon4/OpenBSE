# Data centers and electrical {#sec-datacenter}

**Source:** `openbse-envelope/src/zone.rs` (`DataCenterConfig`); `openbse-io/src/input.rs`
(IT gains, `ElecDistributionInput`); `main.rs` (`data_center_return_temps`,
`hot_aisle_return`, "Electrical distribution losses"); `openbse-io/src/output.rs`
(metrics).

## IT load {#sec-it-load}

A zone with a `data_center` block gets an equipment gain of $P_\text{IT}$, from
`it_load_kw` or `rack_count` × `kw_per_rack`, times `it_load_schedule`. The gain is
wholly sensible and convective, and tagged with the `datacenter` submeter. It is
reported under the `it_equipment` end use, not as plug load. An optional
`lighting_w_per_m2` adds lights.

**Comparison with EnergyPlus.** `ElectricEquipment:ITE:AirCooled` [@eplus-engref-25-2,
"Heat Gain from IT Equipment"] has CPU power and airflow as functions of inlet
temperature and load, separate fan power, supply and return approach temperatures and
recirculation. OpenBSE's IT power depends only on its schedule
([#260](https://github.com/bbrannon4/OpenBSE/issues/260)).

## Hot-aisle return {#sec-hot-aisle}

The servers' own airflow sets the rack exhaust temperature, whatever the CRAC or CRAH
moves. With supply temperature $T_s$, that airflow is either the input
`airflow_m3_per_s_per_kw` or the flow that gives the design rack rise:

$$
\dot m_\text{IT} = \frac{P_\text{IT}}{c_p\max(T_\text{rack} - T_s,\ 1)}, \qquad
T_\text{hot} = T_s + \frac{P_\text{IT}}{\dot m_\text{IT}c_p},
$$ {#eq-it-flow}

with `rack_outlet_temp_c` (default 35 °C) as $T_\text{rack}$. A containment efficiency
$\varepsilon_c$ (default 0.85) sets the exhaust captured directly by the unit, up to its
flow $\dot m_u$:

$$
\dot m_\text{cap} = \min(\varepsilon_c\dot m_\text{IT},\ \dot m_u), \qquad
\dot Q_\text{byp} = \dot m_\text{cap}c_p(T_\text{hot} - T_z), \qquad
T_\text{ret} = T_z + \frac{\dot Q_\text{byp}}{\dot m_uc_p}.
$$ {#eq-hot-aisle}

$\dot Q_\text{byp}$ is taken from the room air's gains, because it goes straight to the
unit's coil. It is recomputed each HVAC iteration from the current supply.

**Supply temperature.** For a CRAC or CRAH (@sec-crac-crah) whose first zone is a data
centre, the supply setpoint is replaced by `rack_inlet_temp_max_c`, or otherwise the
equipment class's allowable limit: A1 32, A2 35, A3 40, A4 45 °C. With the default
class, that makes the setpoint 35 °C, at which the coil cannot cool the room
([#260](https://github.com/bbrannon4/OpenBSE/issues/260)).

**Comparison with EnergyPlus.** @eq-hot-aisle plays the role of the ITE object's
return approach. It is a mass-weighted form of the same idea; EnergyPlus's
approach temperatures are inputs or curves.

## Electrical distribution {#sec-elec-dist}

With a total IT power $P$ and each device's rating $P_r$, efficiency $\eta$, and the
part-load ratio over all devices of its kind,

$$
x = \min\left(\frac{P}{\sum P_r},\ 1\right), \qquad
P_\text{loss} = \sum_{\text{UPS, transformers}} P_rx\left(\frac{1}{\eta} - 1\right).
$$ {#eq-elec-dist}

The defaults are $\eta = 0.95$ for a UPS and 0.985 for a transformer. These losses are
electricity use that heats nothing. The UPS `efficiency_curve` input is not read
([#260](https://github.com/bbrannon4/OpenBSE/issues/260)).

**Comparison with EnergyPlus.** EnergyPlus has `ElectricLoadCenter:Storage:Converter`
and `:Transformer` in its electric load centre, and a UPS efficiency curve on its ITE
object, with a loss fraction to the zone. OpenBSE has none of the rest of the electric
load centre (generators, storage, photovoltaics).

## ASHRAE 90.4 metrics {#sec-90-4}

The summary report gives, annually,

$$
\text{PUE} = \frac{E_\text{IT} + E_\text{mech} + E_\text{dist}}{E_\text{IT}}, \qquad
\text{MLC} = \frac{E_\text{mech}}{E_\text{IT}}, \qquad
\text{ELC} = \frac{E_\text{dist}}{E_\text{IT}},
$$ {#eq-90-4}

where $E_\text{mech}$ is the fan, cooling, pump and heat-rejection energy tagged with the
`datacenter` submeter [@ashrae904]. It compares MLC with a limit from the first digit of the
climate zone (0.05 to 0.08), and ELC with 0.06.

These limits have no citation, and have not been checked against the standard. Do not
rely on the report's PASS and FAIL. If nothing is tagged, MLC is zero. The hourly `pue`
output counts all submeter energy, so it can differ from the annual one
([#260](https://github.com/bbrannon4/OpenBSE/issues/260)).

## Known limitations {#sec-datacenter-limitations}

- [#260](https://github.com/bbrannon4/OpenBSE/issues/260): supply setpoint overwritten with
  the class's allowable limit; UPS curve unread; no IT temperature or fan model; unverified
  90.4 limits and tag-dependent MLC.
- [#62](https://github.com/bbrannon4/OpenBSE/issues/62): earlier CRAC and CRAH review.
