# Large Office Prototype — E+ Comparison Notes

## Models

- **E+ reference:** `eplus_run/in.idf`, the DOE 90.1-2019 Large Office, run
  annually with EnergyPlus 25.2. Its end uses are in `eplus_run/eplustbl.csv`.
- **OpenBSE:** `LargeOffice_Boulder.yaml` recreates that IDF as a person would
  by hand: every floor explicit (no zone multipliers), envelope, internal mass
  and interzone surfaces generated from the IDF by `envelope_from_idf.py`, and
  every other input checked against its IDF object in `INPUT_AUDIT.md`, which
  also lists what OpenBSE can't yet express. The plenum zones are left out
  because every plenum surface in this IDF is adiabatic.
- **Envelope check:** `LargeOffice_Boulder_ideal.yaml`, with ideal loads in place
  of the HVAC (`IDEAL_LOADS_COMPARISON.md`).

## Weather

Both run on Denver-Aurora-Buckley AFB TMY3 (WMO 724695): E+ on
`USA_CO_Denver-Aurora-Buckley.AFB.724695_TMY3.epw`, OpenBSE on the same file as
`prototype_tests/Denver-Buckley.epw`. Despite the model's name, it does not use
Boulder weather.

## Annual end uses

| End use | E+ [kWh] | OpenBSE [kWh] | Difference |
|---|---:|---:|---:|
| Heating (gas) | 472,503 | 455,045 | −3.7% |
| Cooling | 589,511 | 589,598 | 0.0% |
| Fans | 1,042,617 | 1,038,013 | −0.4% |
| Pumps | 135,044 | 99,962 | −26.0% |
| Interior lighting | 1,558,539 | 1,558,539 | 0.0% |
| Exterior lighting | 279,583 | 279,273 | −0.1% |
| Interior equipment | 4,076,778 | 4,076,725 | 0.0% |
| Exterior equipment | 713,075 | 713,075 | 0.0% |
| Service hot water | 125,133 | 125,147 | 0.0% |

OpenBSE values are the committed baselines in `tests/model_baselines.json`.
E+ values are `eplustbl.csv` (GJ × 277.78).

## Open gaps

- **Pumps (−26%).** Not yet traced. Chilled- and hot-water pump energy follows
  the loops' flows and heads; compare them loop by loop with E+'s pump output
  variables before changing anything.
