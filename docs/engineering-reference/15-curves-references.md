# Performance curves and references {#sec-curves}

## Performance curves {#sec-performance-curves}

**Source:** `openbse-components/src/performance_curve.rs` (`PerformanceCurve`).

Curves modify rated performance as a function of operating conditions. A curve is named
under `performance_curves` and referenced by components, which supply its arguments by
position.

**Polynomials.**

: Polynomial curve types. {#tbl-curves}

| Type | Form | Coefficients |
|---|---|---|
| `linear` | $c_1 + c_2x$ | 2 |
| `quadratic` | $c_1 + c_2x + c_3x^2$ | 3 |
| `cubic` | $c_1 + c_2x + c_3x^2 + c_4x^3$ | 4 |
| `biquadratic` | $c_1 + c_2x + c_3x^2 + c_4y + c_5y^2 + c_6xy$ | 6 |
| `bicubic` | biquadratic $+\ c_7x^3 + c_8y^3 + c_9x^2y + c_{10}xy^2$ | 10 |

Inputs are clamped to $[x_\text{min}, x_\text{max}]$ and $[y_\text{min}, y_\text{max}]$
(default $\pm100$), and the output to the optional limits. Missing coefficients count as 0
([#264](https://github.com/bbrannon4/OpenBSE/issues/264)).

**Tables.** A `table` curve holds values on any number of named axes, stored row-major, with
N-linear interpolation. Beyond an axis's ends, it holds the edge value or extrapolates
linearly, by axis. A component that calls it by position maps $x$ and $y$ to the first two
axes. A missing variable reads as 0. Table axes can be validated against a component's slot
(`validate_table_axes`).

**Where curves are used.**

: Curve arguments by component. {#tbl-curve-args}

| Component | Curves | Arguments |
|---|---|---|
| DX cooling coil | `cap_ft_curve`, `eir_ft_curve` | $T_{wb,i}$, $T_o$ |
| DX cooling coil | `cap_fflow_curve`, `eir_fflow_curve`, `plf_curve` | flow fraction; PLR |
| Heat-pump heating coil | `cap_ft_curve`, `eir_ft_curve` | $T_o$, $T_i$ |
| VRF | capacity and EIR f(T) | $T_o$, $\bar T_z$ |
| GSHP | capacity and EIR f(T) | $T_e$, $T_\text{in}$ |
| Chiller | CAPFT, EIRFT | $T_e$, $T_c$ |
| Chiller | EIRFPLR | PLR, or $T_{c,\text{out}}$ and PLR |
| Boiler | efficiency | PLR |
| HPWH | capacity, COP | $T_{wb}$, $T_\text{tank}$ |

**Comparison with EnergyPlus.** The polynomial forms and coefficient orders match
`Curve:Linear`, `Curve:Quadratic`, `Curve:Cubic`, `Curve:Biquadratic` and `Curve:Bicubic`, and
tables play the part of `Table:Lookup` [@eplus-engref-25-2, "Performance Curves"]. EnergyPlus's
limits are required inputs, and it has more forms (quad-linear, triquadratic, exponents and so
on) ([#71](https://github.com/bbrannon4/OpenBSE/issues/71)). The arguments are *not* always in
EnergyPlus's order. The heat-pump coil and VRF evaluate $(T_o, T_i)$ where EnergyPlus evaluates
indoor first, and the GSHP's EIR acts in reverse, so curves copied from EnergyPlus need checking
([#237](https://github.com/bbrannon4/OpenBSE/issues/237),
[#57](https://github.com/bbrannon4/OpenBSE/issues/57),
[#56](https://github.com/bbrannon4/OpenBSE/issues/56)).

## References {.unnumbered}

::: {#refs}
:::
