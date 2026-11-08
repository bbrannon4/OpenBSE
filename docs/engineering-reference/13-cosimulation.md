# Co-simulation {#sec-cosim}

**Source:** `openbse-cosim/src/subprocess.rs` (`SubprocessTransport`),
`external_air.rs` (`ExternalAirComponent`), `external_plant.rs`
(`ExternalPlantComponent`).

An `external_air` or `external_plant` entry in an equipment list runs as a child process.
OpenBSE sends it the current state, and takes back its outlet and its energy use.

## Protocol {#sec-cosim-protocol}

The child starts the first time the component is called with flow, in the model file's
directory, so a relative script path in `command` resolves next to the model. The two exchange
newline-delimited JSON over its stdin and stdout. Each call writes

```json
{"time_s": 3600.0, "dt_s": 600.0, "iteration": 0, "warmup": false, "sizing": false,
 "inputs": {"inlet_temp_c": 13.0}}
```

and reads one line back, either `{"outputs": {...}}` or `{"error": "..."}`. At the end of the
run OpenBSE writes `{"command":"stop"}`, gives the child one second to exit, and then ends it.

**Calls.** An exchange happens at every call to the component, which is every HVAC iteration
(@sec-hvac-iteration). The exchanges of one timestep share `time_s`, and `iteration` counts
them from 0; OpenBSE keeps the outputs of the last one. A model with its own state (a tank, a
controller's integral term) evaluates each iteration from its state at the start of the step
and commits that state when a request with a new `time_s` arrives. `warmup` is true while
warmup repeats the first days of the run (@sec-warmup), so the same `time_s` values recur; a
model resets or discards its state at the first request with `warmup` false. `sizing` marks
design-day sizing calls. A component with no flow, or a plant component with no load, isn't
called.

## Variables {#sec-cosim-variables}

Both kinds send the variables named in `inputs` and expect the ones named in `outputs`.
Unknown names, and a list of outputs without the required ones, are severe errors when the
model is read.

: Co-simulation variables. {#tbl-cosim-vars}

| | Air | Plant |
|---|---|---|
| Inputs | `inlet_temp_c`, `inlet_humidity_ratio`, `inlet_mass_flow_kg_s`, `inlet_enthalpy_j_kg` | `return_temp_c`, `return_flow_kg_s`, `load_request_w` |
| Both | `outdoor_temp_c`, `outdoor_humidity_ratio`, `outdoor_pressure_pa`, `sim_time_s`, `dt_s`, `month`, `day`, `hour`, `sub_hour` | same |
| Required outputs | `outlet_temp_c`, `outlet_humidity_ratio` | `supply_temp_c` |
| Optional outputs | `outlet_mass_flow_kg_s`, `power_w`, `fuel_w`, `thermal_output_w` | `supply_flow_kg_s`, `power_w`, `fuel_w`, `thermal_output_w` |

`load_request_w` is the load the plant loop asks of the unit: positive for heating, negative
for cooling and for a condenser loop's heat rejection. Every declared output must be in every
response. Energy outputs left out of `outputs` count as 0, and flow outputs as the inlet flow.

**Energy.** OpenBSE takes `power_w` and `fuel_w` as the component's energy. The heat it gives
the fluid comes from the returned state at the inlet flow, so the loop's and the air's energy
balances close whatever the model reports:

$$
\dot{Q}_{plant} = \dot{m}_{w}\,c_{p,w}\,(T_{supply} - T_{return})
\qquad
\dot{Q}_{air} = \dot{m}_{a}\,(h_{out} - h_{in})
$$ {#eq-cosim-heat}

A reported `thermal_output_w` is a check on the model: if it differs from @eq-cosim-heat by
more than 2 % (or 10 W), the run stops. The electricity goes to an end use by the component's
name, as for other untyped components: a name with "chiller" or "cool" books
to cooling, one with "fan" to fans, and anything else to Other.

## Failure handling {#sec-cosim-failure}

Each of these stops the run with a severe error at the end of that timestep, since the
component's results would otherwise be missing from the energy and the loads:

- the child can't be started, exits, or closes its output;
- a response is not valid JSON, is an `error` reply, lacks a declared output, or holds a
  number that is not finite;
- a reported `thermal_output_w` disagrees with the outlet state;
- no response arrives within the component's `timeout_s` (default 60 s). The child is then
  ended, not asked to stop.

**Comparison with EnergyPlus.** EnergyPlus couples through `ExternalInterface` (BCVTB and
FMUs) and Python plugins. `ExternalInterface` exchanges once per zone timestep, before the
HVAC solution, so the external model's outputs are fixed while the HVAC iterates; a plugin
runs at a chosen calling point. OpenBSE instead calls the external model inside the HVAC
iteration, so an external unit takes part in the converged solution like a native one, and
the `iteration` and `warmup` fields give a stateful model what it needs to stay consistent.
It needs no library in the external model.

## Known limitations {#sec-cosim-limitations}

- An external component has no `end_use` input; its electricity is booked by its name.
