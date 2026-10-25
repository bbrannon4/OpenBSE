# Weather files for the shipped models

The models in `examples/` and `prototype_tests/` point here with relative paths
(`../weather/…` or `../../weather/…`). ASHRAE 140 weather stays in `140_tests/weather/`.

| File | Source | Used by |
|---|---|---|
| `USA_CO_Boulder.Muni.AP.720533_TMYx.2009-2023.epw` | Climate.OneBuilding TMYx (redistribution confirmed fine by the owner, 2026-10-25) | most examples, Hospital, LargeOffice_expanded |
| `USA_CO_Denver-Aurora-Buckley.AFB.724695_TMY3.epw` | NREL TMY3 (public domain) | Apartment, LargeOffice, LargeOffice_ideal, SingleFamily, `multi_year_parametric` (via `-w`) |
| `USA_IL_Chicago-OHare.Intl.AP.725300_TMY3.epw` | NREL TMY3 (public domain), as shipped with EnergyPlus | `datacenter_crah` |
| `USA_CO_Golden-NREL.724666_TMY3.epw` | NREL TMY3 (public domain), as shipped with EnergyPlus | `1zone_uncontrolled` |

The Boulder file is the 2024 download the models were developed with. OneBuilding
has since re-issued it with only the `DESIGN CONDITIONS` header line changed
(2025 ASHRAE design conditions). The engine doesn't read that line, so either
copy gives identical results.

`prototype_tests/Denver-Buckley.epw` and `prototype_tests/Boulder.epw`, which
older checkouts had untracked, are the Denver TMY3 file and the Boulder file above.
