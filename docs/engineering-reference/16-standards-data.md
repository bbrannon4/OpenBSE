# Code and standard reference data {#sec-standards}

**Source:** `openbse-standards` (data, lookups, construction solver);
`openbse-io/src/standards.rs` (applying a standard to a model); data in
`data/standards/`.

A model can name a code or standard and have lighting power and constructions
filled in from its tables when it is loaded. This chapter describes how the
values are stored, how they are applied, and how an assembly U-factor becomes
a layered construction. The data available, its file format and the
space-type keys are listed in `data/standards/README.md`.

## Stored values {#sec-standards-data}

Each standard edition is a directory holding one file per source table and a
manifest (`edition.yaml`) with the edition's climate zones, compliance
methods, envelope categories, air-film resistances and the defaults OpenBSE
uses. Every value carries its citation: edition, table number and row as the
table names it. The standard's text is not stored.

Each table is kept twice: in the unit system the edition prints, as printed,
and in SI, which the engine reads. Where only one unit system was transcribed,
the other is an exact conversion, kept to 7 significant figures, from

$$
1\ \text{ft} = 0.3048\ \text{m},\qquad
1\ \text{Btu}_\text{IT} = 1055.05585262\ \text{J},\qquad
1\ \text{°F} = \tfrac{5}{9}\ \text{K},
$$ {#eq-standards-units}

so that 1 Btu/(h·ft²·°F) = 5.678263 W/(m²·K), 1 Btu/(h·ft·°F) =
1.730735 W/(m·K) and 1 W/ft² = 10.76391 W/m². An edition printed in both unit
systems rounds each independently, so a converted value can differ from the
other edition's printed value in its last digit; the file records which it is
(`derivation`). The data is compiled into the binary and checked by a test
that rejects unknown keys, SI values that disagree with their source, gaps in
climate-zone coverage, duplicate keys and unphysical values.

Climate zones, envelope categories, assembly classes, methods and space types
are data, not code. Only the envelope elements (roof, above- and below-grade
wall, floor, slab-on-grade, opaque door, vertical fenestration, skylight) and
the two lighting methods (space-by-space, building area) are fixed, because
they decide how a value reaches the model.

## Applying a standard {#sec-standards-apply}

The model's `standard:` block names the standard, edition, method
(`prescriptive` or `appendix_g` for ASHRAE 90.1) and climate zone. It is
applied after parametric overrides and before validation, so the validated
and simulated model is the filled one. Explicit inputs always win.

**Lighting.** Each zone not already covered by a `lights` entry receives one
at the table's lighting power density (LPD), found by the zone's `space_type`
(space-by-space method) or the building area type (building area method). A
zone can name one key per method, because the prescriptive and Appendix G
tables divide some space types differently. Where the table gives LPD per unit
of height (Appendix G atria), the zone height $H = V/A_\text{floor}$, or the
vertical extent of its surfaces, gives

$$
\text{LPD} = \text{LPD}_0 + \text{LPD}_H\,H.
$$ {#eq-standards-lpd-height}

A conditioned zone without a space type, or an unknown key, is an error that
lists similar keys.

**Envelope.** Each surface without a construction is classified by type,
boundary and tilt: an outdoor wall is an above-grade wall, a ground wall a
below-grade wall, an outdoor roof or ceiling a roof, an outdoor floor a floor,
a ground floor a slab-on-grade, and a window sloped less than 60° from
horizontal a skylight (the 90.1 definition), otherwise vertical fenestration.
Interzone and adiabatic surfaces are not in the tables and need explicit
constructions. The surface's zone gives the envelope category (default: the
block's, then the edition's), and the block or the edition's defaults give the
assembly class. One construction is built per element, class and category and
shared by every surface that needs it:

- opaque assemblies are layered constructions solved to the table's U-factor
  (@sec-standards-constructions);
- slab-on-grade floors are F-factor floors (@sec-ffactor) at the table's
  F-factor;
- fenestration uses the table's U-factor and SHGC in the simple glazing model
  (@sec-windows). Tables banded by area ratio (Appendix G) are indexed by the
  window-to-wall ratio of the exterior walls of the zones the envelope covers,
  or the skylight-to-roof ratio for skylights. A ratio belongs to the first
  band whose upper bound it does not exceed.

The run summary lists every value used: what received it, the value in SI,
the table and row, and the printed value with its unit.

## Constructions from assembly U-factors {#sec-standards-constructions}

**Source:** `openbse-standards/src/constructions.rs::build_opaque`.

A standard gives an assembly U-factor $U_t$ that includes inside and outside
air films, not a layer build-up. Each assembly class maps to a typical layered
assembly (`data/standards/typical_assemblies.yaml`): fixed layers $j$ of
thickness $L_j$ and conductivity $k_j$ that carry the class's thermal mass,
plus one insulation layer. The insulation's resistance is solved so that the
layers and the films the table assumes give exactly $U_t$:

$$
R_\text{ins} = \frac{1}{U_t} - \left(R_\text{film,out} + R_\text{film,in}\right) - \sum_j \frac{L_j}{k_j}.
$$ {#eq-standards-insulation}

The films are the ones the edition states for that class. For ASHRAE 90.1
(Appendix A, Section A9.4.1) they are, in h·ft²·°F/Btu: exterior 0.17,
semiexterior 0.46, interior vertical 0.68, interior horizontal with heat flow
up 0.61 and down 0.92. Roofs with insulation above deck and metal building
roofs use exterior and heat-flow-up films, attic roofs semiexterior and
heat-flow-up, walls and doors exterior and interior vertical, and floors
semiexterior and heat-flow-down, following each class's base assembly in
Appendix A.

The insulation is a no-mass layer, so the solved resistance is exact and adds
no heat capacity. In framed classes it is an effective layer standing for the
cavity insulation and framing together, as the table's U-factor does. When
$R_\text{ins} \le 0$ the fixed layers already resist more than the target, so
the insulation is left out and the run summary reports the resulting lower U;
a table cell of `NR` builds the assembly without insulation. In simulation the
layers get the engine's own dynamic air films (@sec-convection), so the
in-situ conductance varies with conditions about the rated value.

| 90.1 class | Typical assembly (outside to inside) |
|---|---|
| Roof, insulation entirely above deck | built-up roofing 9.5 mm, insulation, metal deck 0.8 mm |
| Roof, metal building | metal panel 0.8 mm, insulation |
| Roof, attic and other | insulation, gypsum board 15.9 mm |
| Wall, mass | heavyweight concrete 203 mm, insulation, gypsum board 12.7 mm |
| Wall, metal building | metal panel 0.8 mm, insulation |
| Wall, steel-framed; wood-framed and other | stucco 25.4 mm, gypsum board 15.9 mm, insulation, gypsum board 15.9 mm |
| Floor, mass | insulation, heavyweight concrete 203 mm |
| Floor, steel joist | insulation, metal deck 0.8 mm, heavyweight concrete 102 mm |
| Floor, wood-framed and other | insulation, plywood 19 mm |
| Opaque door | metal 0.8 mm, insulation, metal 0.8 mm |

: Typical assemblies. Material properties are from the 2005 ASHRAE
Handbook—Fundamentals, as given in the EnergyPlus `ASHRAE_2005_HOF_Materials`
dataset. {#tbl-standards-assemblies}

**Comparison with EnergyPlus.** EnergyPlus has no standards import; its
prototype models (generated by openstudio-standards) use the same approach of
adjusting one insulation layer of a typical assembly to meet the code
U-factor. The constructions OpenBSE builds are ordinary layered constructions
(@sec-ctf), F-factor floors (`Construction:FfactorGroundFloor`) and simple
glazing (`WindowMaterial:SimpleGlazingSystem`), each matching its EnergyPlus
counterpart.

## Known limitations: standards data {#sec-standards-limitations}

- Below-grade walls are given as C-factors, and OpenBSE has no C-factor
  ground method (`Construction:CfactorUndergroundWall`,
  [#401](https://github.com/bbrannon4/OpenBSE/issues/401)); a below-grade
  wall without an explicit construction is an error.
- There is no door surface type, so opaque-door rows are stored but not
  applied ([#404](https://github.com/bbrannon4/OpenBSE/issues/404)).
- Visible transmittance and the VT/SHGC ratio are reported but unused: the
  engine has no daylighting model ([#407](https://github.com/bbrannon4/OpenBSE/issues/407)).
- Lighting control requirements, additional lighting power allowances and the
  Appendix G occupancy sensor reductions are not applied. Room cavity ratio
  thresholds and occupancy sensor reductions are stored ([#403](https://github.com/bbrannon4/OpenBSE/issues/403)).
- The Appendix G baseline is only its lighting and envelope: window area is
  the model's (not adjusted to the baseline window-to-wall ratio), and
  orientation, HVAC and other baseline rules are not applied ([#405](https://github.com/bbrannon4/OpenBSE/issues/405)).
- A zone has one space type; mixed-use zones need explicit lights ([#403](https://github.com/bbrannon4/OpenBSE/issues/403)).
- Prescriptive fenestration U-factor increases permitted at high elevation
  are not applied ([#406](https://github.com/bbrannon4/OpenBSE/issues/406)).
- The 90.1-2025 SI tables are converted from the I-P edition, not
  transcribed from the SI edition ([#406](https://github.com/bbrannon4/OpenBSE/issues/406)).
