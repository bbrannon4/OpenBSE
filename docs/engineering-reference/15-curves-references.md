# Performance curves and references {#sec-curves}

::: {.callout-warning}
## Pre-review content

This chapter holds text moved unchanged, apart from format, from the former single-file reference (`README.md`, mostly written in March 2026). Parts of it are stale or incomplete. It will be rewritten from the code, and checked against EnergyPlus, in this chapter's review ([#181](https://github.com/bbrannon4/OpenBSE/issues/181)); see @sec-status.
:::

## Performance Curves

**Crate:** `openbse-components` — `performance_curve.rs`

**Purpose:** Reusable polynomial curves that modify rated equipment performance as a function of operating conditions (temperatures, flow ratios, etc.). Curves are defined at the top level of the model and referenced by name from equipment.

### Curve Types

| Type | Form | Coefficients |
|------|------|-------------|
| Biquadratic | f(x,y) = c1 + c2·x + c3·x² + c4·y + c5·y² + c6·x·y | 6 |
| Quadratic | f(x) = c1 + c2·x + c3·x² | 3 |
| Cubic | f(x) = c1 + c2·x + c3·x² + c4·x³ | 4 |
| Linear | f(x) = c1 + c2·x | 2 |

### Input/Output Clamping

Input values are clamped to `[min_x, max_x]` and `[min_y, max_y]` before evaluation. Output is clamped to `[min_output, max_output]` if those limits are set. This prevents extrapolation beyond the curve's valid range.

### Usage

**DX Cooling Coils:** Two biquadratic curves as functions of entering wet-bulb temperature (x) and outdoor dry-bulb temperature (y):
- `cap_ft_curve` — capacity modifier: `available_capacity = rated_capacity × cap_ft(T_wb, T_odb)`
- `eir_ft_curve` — EIR modifier: `available_COP = rated_COP / eir_ft(T_wb, T_odb)`

When curves are absent, the engine falls back to simplified linear derating with outdoor temperature.

## References {.unnumbered}

::: {#refs}
:::
