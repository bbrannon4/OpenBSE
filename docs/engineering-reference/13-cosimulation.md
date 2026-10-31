# Co-simulation {#sec-cosim}

**Source:** `openbse-cosim/src/subprocess.rs` (`SubprocessTransport`),
`external_air.rs` (`ExternalAirComponent`), `external_plant.rs`
(`ExternalPlantComponent`).

An `external_air` or `external_plant` entry in an equipment list runs as a child process.
OpenBSE sends it the current state, and takes back its outlet and its energy use.

## Protocol {#sec-cosim-protocol}

The child starts the first time the component is called with flow. The two exchange
newline-delimited JSON over its stdin and stdout. Each call writes

```json
{"time_s": 3600.0, "dt_s": 600.0, "inputs": {"inlet_temp_c": 13.0}}
```

and reads one line back, either `{"outputs": {...}}` or `{"error": "..."}`. When the
component is dropped, OpenBSE writes `{"command":"stop"}` and waits for the child to exit.

**Calls.** An exchange happens at every call to the component. That is every HVAC iteration
(@sec-hvac-iteration), including warmup, with the same `time_s`, and no flag saying which
call is final. A model with its own state must handle repeats itself
([#262](https://github.com/bbrannon4/OpenBSE/issues/262)). A component with no flow, or a
plant component with no load, isn't called.

## Variables {#sec-cosim-variables}

Both kinds send the variables named in `input_vars`. Unknown names are sent as 0 with a
warning.

: Co-simulation variables. {#tbl-cosim-vars}

| | Air | Plant |
|---|---|---|
| Inputs | `inlet_temp_c`, `inlet_humidity_ratio`, `inlet_mass_flow_kg_s`, `inlet_enthalpy_j_kg` | `return_temp_c`, `return_flow_kg_s`, `load_request_w` |
| Both | `outdoor_temp_c`, `outdoor_humidity_ratio`, `outdoor_pressure_pa`, `sim_time_s`, `dt_s`, `month`, `day`, `hour`, `sub_hour` | same |
| Required outputs | `outlet_temp_c`, `outlet_humidity_ratio` | `supply_temp_c` |
| Optional outputs | `outlet_mass_flow_kg_s`, `power_w`, `fuel_w`, `thermal_output_w` | `supply_flow_kg_s`, `power_w`, `fuel_w`, `thermal_output_w` |

`load_request_w` is positive for heating and negative for cooling. Optional energy outputs
default to 0 and flow outputs to the inlet flow. "Required" outputs that are missing fall
back to the inlet value.

**Energy.** OpenBSE takes `power_w` and `fuel_w` as the component's energy, under an `Other`
end use, and `thermal_output_w` as its heat. A plant component that leaves out
`thermal_output_w` delivers nothing to the loop's accounting (@sec-plant-loop).

## Failure handling {#sec-cosim-failure}

If the child can't start, or an exchange fails or returns an error, OpenBSE logs an error
and the component passes its inlet through with zero power, and the run continues. It has
no timeout, so a child that hangs also hangs the run
([#262](https://github.com/bbrannon4/OpenBSE/issues/262)).

**Comparison with EnergyPlus.** EnergyPlus couples through `ExternalInterface` (BCVTB and
FMUs) and Python plugins, at defined points in its calling sequence and with warmup
handling. OpenBSE's interface is simpler, and needs no library in the external model, but
has none of those guarantees.

## Known limitations {#sec-cosim-limitations}

- [#262](https://github.com/bbrannon4/OpenBSE/issues/262): an exchange per iteration with
  no flag; failures pass through; outputs unchecked; no timeout.
- [#169](https://github.com/bbrannon4/OpenBSE/issues/169): the `cosim_chiller` example never
  runs its external model.
