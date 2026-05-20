"""
openbse_cosim — helper for OpenBSE co-simulation slaves.

Usage
-----
Import `run_cosim` and pass a step function that takes (inputs, time_s, dt_s)
and returns an outputs dict:

    from openbse_cosim import run_cosim

    def my_component(inputs, time_s, dt_s):
        # inputs: dict[str, float] — values for the variables listed in your YAML inputs
        # return: dict[str, float] — values for the variables listed in your YAML outputs
        return {"outlet_temp_c": 13.0, "power_w": 5000.0}

    if __name__ == "__main__":
        run_cosim(my_component)

Protocol
--------
OpenBSE writes one JSON line to stdin per timestep::

    {"time_s": 3600.0, "dt_s": 3600.0, "inputs": {"inlet_temp_c": 26.0, ...}}

The step function must return one JSON line to stdout::

    {"outputs": {"outlet_temp_c": 13.0, "power_w": 5000.0}}

On error the helper writes::

    {"error": "message"}

When OpenBSE finishes it sends ``{"command": "stop"}`` and the helper exits.

Standalone scripts
------------------
Scripts that don't need the helper can implement the loop themselves — see
the ``examples/cosim_chiller/chiller.py`` example.
"""

import json
import sys
from typing import Callable, Dict


def run_cosim(step_fn: Callable[[Dict[str, float], float, float], Dict[str, float]]) -> None:
    """Drive a co-simulation slave process.

    Reads JSON requests from stdin, calls *step_fn* for each timestep, and
    writes JSON responses to stdout.  Returns when OpenBSE sends a stop
    command or stdin is closed.

    Args:
        step_fn: Called once per timestep with ``(inputs, time_s, dt_s)``.
                 Must return a dict mapping output variable names to float values.
    """
    for raw in sys.stdin:
        line = raw.strip()
        if not line:
            continue

        try:
            msg = json.loads(line)
        except json.JSONDecodeError as exc:
            _write({"error": f"JSON decode error: {exc}"})
            continue

        if msg.get("command") == "stop":
            break

        try:
            outputs = step_fn(msg["inputs"], msg["time_s"], msg["dt_s"])
            _write({"outputs": outputs})
        except Exception as exc:  # noqa: BLE001
            _write({"error": str(exc)})


def _write(obj: dict) -> None:
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()
