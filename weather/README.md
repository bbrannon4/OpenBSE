# Weather files for the shipped models

The models in `examples/` and `prototype_tests/` point here with relative paths
(`../weather/…` or `../../weather/…`). ASHRAE 140 weather stays in `140_tests/weather/`.

| File | Source | In git? | Used by |
|---|---|---|---|
| `USA_CO_Boulder.Muni.AP.720533_TMYx.2009-2023.epw` | Climate.OneBuilding TMYx | **No**: fetched by `fetch_weather.py` | most examples, Hospital, LargeOffice_expanded |
| `USA_CO_Denver-Aurora-Buckley.AFB.724695_TMY3.epw` | NREL TMY3 (public domain) | yes | Apartment, LargeOffice, LargeOffice_ideal, SingleFamily, `multi_year_parametric` (via `-w`) |
| `USA_IL_Chicago-OHare.Intl.AP.725300_TMY3.epw` | NREL TMY3 (public domain), as shipped with EnergyPlus | yes | `datacenter_crah` |
| `USA_CO_Golden-NREL.724666_TMY3.epw` | NREL TMY3 (public domain), as shipped with EnergyPlus | yes | `1zone_uncontrolled` |

Get the files that aren't committed:

```bash
python3 weather/fetch_weather.py
```

## Why the TMYx file isn't committed

Climate.OneBuilding marks its site "All Rights Reserved" and states no
redistribution licence, so the repo downloads the file rather than shipping it.
If the owner confirms redistribution is fine, commit the file and drop it from
`.gitignore`; nothing else changes.

OneBuilding re-issues files in place. Its 2025 re-issue of the Boulder file
changed only the `DESIGN CONDITIONS` header line, which the engine doesn't read.
So `fetch_weather.py` pins a hash of the hourly data (all lines after the
8-line header, line endings normalised), not of the whole file.

`prototype_tests/Denver-Buckley.epw` and `prototype_tests/Boulder.epw`, which
older checkouts had untracked, are the Denver TMY3 file above and the same
Boulder TMYx hourly data.
