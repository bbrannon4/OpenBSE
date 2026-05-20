"""
Co-simulation chiller: replaces the native OpenBSE Chiller component.

Receives chilled water loop conditions from OpenBSE each timestep via stdin
and returns supply conditions plus power draw to stdout.

Physics
-------
COP degrades linearly with outdoor temperature above 15 °C to approximate
condenser heat rejection penalty.  A simple PLR curve peaks at ~70% load
to mimic IPLV behavior.  Both are stand-ins for a real performance map.

Usage
-----
This script is referenced from building.yaml::

    - type: external_plant
      name: Python Chiller
      command: ["python", "chiller.py"]
      inputs:  [return_temp_c, return_flow_kg_s, load_request_w, outdoor_temp_c, sim_time_s]
      outputs: [supply_temp_c, power_w, thermal_output_w]

OpenBSE launches it once at the start of the simulation and keeps it running.
"""

import json
import sys

# ── Parameters ────────────────────────────────────────────────────────────────
DESIGN_COP = 3.8          # rated COP at full load, 15 °C outdoor
COP_TEMP_PENALTY = 0.04   # COP reduction per °C above 15 °C outdoor
SUPPLY_TEMP_C = 7.0       # leaving chilled water temperature setpoint [°C]
CP_WATER = 4186.0         # specific heat of water [J/(kg·K)]


def cop_at_conditions(outdoor_temp_c: float, plr: float) -> float:
    """Return COP adjusted for outdoor temperature and part-load ratio."""
    temp_penalty = max(0.0, outdoor_temp_c - 15.0) * COP_TEMP_PENALTY
    cop_full = max(0.5, DESIGN_COP - temp_penalty)
    # IPLV-style PLR curve: efficiency peaks around 70% load
    plr_factor = 1.0 - 0.3 * (plr - 0.7) ** 2 if plr > 0 else 0.0
    return cop_full * max(0.5, plr_factor)


def step(inputs: dict, _time_s: float, _dt_s: float) -> dict:
    load_w = inputs["load_request_w"]          # positive = cooling requested [W]
    return_temp_c = inputs["return_temp_c"]    # entering chilled water temp [°C]
    flow_kg_s = inputs["return_flow_kg_s"]     # chilled water mass flow [kg/s]
    outdoor_temp_c = inputs["outdoor_temp_c"]  # outdoor dry-bulb [°C]

    if load_w <= 0.0 or flow_kg_s <= 0.0:
        return {
            "supply_temp_c": return_temp_c,
            "power_w": 0.0,
            "thermal_output_w": 0.0,
        }

    # Maximum capacity limited by flow and fixed supply temperature
    max_capacity_w = flow_kg_s * CP_WATER * max(0.0, return_temp_c - SUPPLY_TEMP_C)
    actual_load_w = min(load_w, max_capacity_w)

    if max_capacity_w <= 0.0:
        return {
            "supply_temp_c": return_temp_c,
            "power_w": 0.0,
            "thermal_output_w": 0.0,
        }

    plr = actual_load_w / max_capacity_w
    cop = cop_at_conditions(outdoor_temp_c, plr)
    power_w = actual_load_w / cop

    delta_t = actual_load_w / (flow_kg_s * CP_WATER)
    supply_temp_c = return_temp_c - delta_t

    return {
        "supply_temp_c": supply_temp_c,
        "power_w": power_w,
        "thermal_output_w": -actual_load_w,   # heat removed from water (negative)
    }


if __name__ == "__main__":
    for raw in sys.stdin:
        line = raw.strip()
        if not line:
            continue
        msg = json.loads(line)
        if msg.get("command") == "stop":
            break
        try:
            result = step(msg["inputs"], msg["time_s"], msg["dt_s"])
            sys.stdout.write(json.dumps({"outputs": result}) + "\n")
        except Exception as exc:  # noqa: BLE001
            sys.stdout.write(json.dumps({"error": str(exc)}) + "\n")
        sys.stdout.flush()
