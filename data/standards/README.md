# Code and standard reference data

Reference values from building energy codes and standards, stored so that a
model can name a standard and have its lighting power and envelope filled in
(see the model's `standard:` block and the Engineering Reference chapter
"Code and standard reference data"). The files are compiled into the
`openbse` binary by `crates/openbse-standards`, so a model needs no data path
at run time.

## Copyright

Numeric values from a standard's tables are facts and are stored here with a
citation (edition, table, row). The standard's text, footnotes and user's
manual are not. Never commit a standard's PDF or copy its prose; write any
explanation in your own words.

## Layout

```
data/standards/
  typical_assemblies.yaml        layered assemblies OpenBSE builds to a target U (any standard)
  <standard>/<edition>/
    edition.yaml                 what the edition defines and OpenBSE's defaults for it
    ip/table_<number>.yaml       one file per table, values as printed in the I-P edition
    si/table_<number>.yaml       the same table in SI, which the engine reads
```

Available now: `ashrae_90_1/2025` (ANSI/ASHRAE/IES Standard 90.1-2025).

| Table | What it holds | Method |
|---|---|---|
| 9.5.1 | LPD, building area method | prescriptive |
| 9.5.2.1-1, 9.5.2.1-2 | LPD and RCR threshold, space-by-space method (common, building-specific) | prescriptive |
| 5.5-0 … 5.5-8 | Envelope criteria by climate zone: opaque U, C and F-factors; fenestration U, SHGC, VT/SHGC | prescriptive |
| G3.8 | Baseline LPD, building area method | appendix_g |
| G3.7-1, G3.7-2 | Baseline LPD and occupancy sensor reduction, space-by-space method | appendix_g |
| G3.4-1 … G3.4-8 | Baseline envelope criteria by climate zone, glazing banded by window-to-wall ratio | appendix_g |

## Units: I-P and SI kept separately

Each table is stored twice. The file in the edition's printed unit system
(`edition.yaml` `source_units`) is the record of what the standard says
(`derivation: printed`). The SI file is what the engine uses.

For 90.1-2025 only the I-P edition was transcribed, so each SI file is
`derivation: converted_from_ip`: an exact conversion (1 ft = 0.3048 m,
1 Btu_IT = 1055.05585262 J, 1 °F = 5/9 K) kept to 7 significant figures,
written by `tools/standards/convert_ip_to_si.py`. The SI edition of a
standard rounds its own values, so its printed numbers can differ from a
converted one in the last digit. When the SI edition is available, replace
each SI file's values with the printed ones and set `derivation: printed`.

`edition.yaml` values (air films) are in the source units and are converted
when loaded.

## Validation

`cargo test -p openbse-standards` fails on a malformed file. It checks:

- every key is known (all structs deny unknown keys) and every file sits in
  the directory its `standard`, `edition`, `units` and `table` say;
- each SI value matches its printed source (to 1e-6 relative when converted,
  3% when printed in an SI edition), and `NR` cells match;
- every climate zone has exactly one envelope table per method, each method
  has both lighting tables, and lighting keys are unique per method;
- each opaque class has a typical assembly, and each class builds to its
  table U-factor exactly;
- values are physical (non-negative, SHGC and VT within 0–1).

## File format

Envelope table (`kind: envelope`):

```yaml
standard: ashrae_90_1
edition: "2025"
table: "5.5-5"
title: Prescriptive building envelope criteria, climate zone 5   # own words
kind: envelope
method: prescriptive
units: ip
derivation: printed
climate_zones: [5A, 5B, 5C]
opaque:
  - element: roof                 # roof, wall_above_grade, wall_below_grade, floor,
    class: insulation_entirely_above_deck   # slab_on_grade, opaque_door
    name: Insulation entirely above deck    # as the table names it
    criteria:                     # per envelope category
      nonresidential: {u_factor: 0.032}     # u_factor | c_factor | f_factor, or NR
fenestration:
  - element: vertical_fenestration          # or skylight
    class: fixed
    name: Fixed
    area_ratio_percent: [0, 40]   # band; [2.1, null] is open-ended
    climate_zones: [3C]           # optional: subset of the table's zones
    criteria:
      nonresidential: {u_factor: 0.32, shgc: 0.38, vt_shgc_min: 1.10}  # or vt:
```

Lighting table (`kind: lighting_space_by_space` or `lighting_building_area`):

```yaml
lighting:
  - key: office_up_to_150_sqft    # stable machine key
    name: "Office, ≤150 ft²"
    lpd: 0.69
    lpd_per_height: 0.025         # optional: LPD = lpd + this × space height
    rcr: 8                        # optional, as printed (number, NA or a rule)
    occupancy_sensor_reduction_percent: 30   # optional (Appendix G)
    aliases: []                   # optional: former keys for this row
```

Keys stay the same across editions while the meaning holds; a renamed row
lists its old key in `aliases`.

## Adding an edition or another standard

1. Create `<standard>/<edition>/edition.yaml`: methods, climate zones,
   envelope categories, air films, default classes and the typical assembly
   for each opaque class. Climate zones, categories, classes and methods are
   data, so Title 24 (zones 1–16, its own categories) needs no code change.
2. Transcribe each table into the printed-units directory, one file per table.
3. Write the other unit system (`tools/standards/convert_ip_to_si.py` for an
   I-P source).
4. Run `cargo test -p openbse-standards`.

## Space-type and building-area keys (90.1-2025)

The prescriptive and Appendix G tables split some space types differently
(offices by size versus enclosure, for example), so some keys exist under one
method only. A zone can name a key per method:

```yaml
zones:
  - name: Office
    space_type: {prescriptive: office_over_300_sqft, appendix_g: office_open_plan}
```

### Space types (space-by-space method)

| Key | Space type | Prescriptive | Appendix G |
|---|---|---|---|
| `atrium_20_to_40ft` | Atrium, ≥20 ft and ≤40 ft in height | 9.5.2.1-1 | G3.7-1 |
| `atrium_over_40ft` | Atrium, >40 ft in height | 9.5.2.1-1 | G3.7-1 |
| `atrium_under_20ft` | Atrium, <20 ft in height | 9.5.2.1-1 | G3.7-1 |
| `audience_seating_auditorium` | Audience seating area, auditorium | 9.5.2.1-1 | G3.7-1 |
| `audience_seating_convention_center` | Audience seating area, convention center | — | G3.7-1 |
| `audience_seating_gymnasium` | Audience seating area, gymnasium | 9.5.2.1-1 | G3.7-1 |
| `audience_seating_motion_picture_theater` | Audience seating area, motion picture theater | 9.5.2.1-1 | G3.7-1 |
| `audience_seating_other` | Audience seating area, all other | 9.5.2.1-1 | G3.7-1 |
| `audience_seating_performing_arts_theater` | Audience seating area, performing arts theater | 9.5.2.1-1 | G3.7-1 |
| `audience_seating_sports_arena` | Audience seating area, sports arena | 9.5.2.1-1 | G3.7-1 |
| `banking_activity_area` | Banking activity area | 9.5.2.1-1 | G3.7-1 |
| `casino_betting_area` | Casino gaming area, betting/sportsbook/keno/bingo | 9.5.2.1-2 | G3.7-2 |
| `casino_high_limit_game_area` | Casino gaming area, high-limit game area | 9.5.2.1-2 | G3.7-2 |
| `casino_slot_machine_area` | Casino gaming area, slot machine/digital gaming | 9.5.2.1-2 | G3.7-2 |
| `casino_table_games_area` | Casino gaming area, table games | 9.5.2.1-2 | G3.7-2 |
| `classroom_other` | Classroom/lecture hall/training room, all other | 9.5.2.1-1 | G3.7-1 |
| `classroom_preschool_to_12_lab_shop` | Classroom/lecture hall/training room, preschool through 12th grade, laboratory and shop | — | G3.7-1 |
| `classroom_shop` | Classroom/lecture hall/training room, shop classroom | 9.5.2.1-1 | — |
| `computer_room` | Computer room | 9.5.2.1-1 | G3.7-1 |
| `conference_meeting_multipurpose` | Conference/meeting/multipurpose room | 9.5.2.1-1 | G3.7-1 |
| `control_editing_room` | Control/editing room or booth | 9.5.2.1-1 | — |
| `convention_center_exhibit_space` | Convention center, exhibit space | 9.5.2.1-2 | G3.7-2 |
| `copy_print_room` | Copy/print room | 9.5.2.1-1 | G3.7-1 |
| `correctional_audience_seating` | Correctional facility, audience seating area | 9.5.2.1-2 | G3.7-2 |
| `correctional_classroom` | Correctional facility, classroom/lecture hall/training room | 9.5.2.1-2 | G3.7-2 |
| `correctional_confinement_cells` | Correctional facility, confinement cells | 9.5.2.1-2 | G3.7-2 |
| `correctional_dining` | Correctional facility, dining area | 9.5.2.1-2 | G3.7-2 |
| `corridor` | Corridor | 9.5.2.1-1 | G3.7-1 |
| `courtroom` | Courtroom | 9.5.2.1-1 | G3.7-1 |
| `dining_bar_lounge_fine` | Dining area, bar/lounge or fine dining | 9.5.2.1-1 | — |
| `dining_bar_lounge_leisure` | Dining area, bar/lounge or leisure dining | — | G3.7-1 |
| `dining_cafeteria_fast_food` | Dining area, fast-food or cafeteria | 9.5.2.1-1 | G3.7-1 |
| `dining_casual` | Dining area, casual dining | 9.5.2.1-1 | — |
| `dining_family` | Dining area, family dining | — | G3.7-1 |
| `dining_other` | Dining area, all other | 9.5.2.1-1 | G3.7-1 |
| `dormitory_living_quarters` | Dormitory, living quarters | 9.5.2.1-2 | G3.7-2 |
| `dwelling_unit` | Dwelling unit | — | G3.7-1 |
| `electrical_mechanical_room` | Electrical/mechanical room | 9.5.2.1-1 | G3.7-1 |
| `emergency_vehicle_garage` | Emergency vehicle garage | 9.5.2.1-1 | G3.7-1 |
| `equipment_room` | Equipment room | 9.5.2.1-1 | G3.7-1 |
| `fire_station_sleeping_quarters` | Fire station, sleeping quarters | 9.5.2.1-2 | G3.7-2 |
| `food_preparation` | Food preparation area | 9.5.2.1-1 | G3.7-1 |
| `guest_room` | Guest room | 9.5.2.1-1 | G3.7-1 |
| `gymnasium_exercise_area` | Gymnasium/fitness center, exercise area | 9.5.2.1-2 | G3.7-2 |
| `gymnasium_playing_area` | Gymnasium/fitness center, playing area | 9.5.2.1-2 | G3.7-2 |
| `healthcare_control_room` | Health care facility, control room (MRI/CT/radiology/PET) | 9.5.2.1-2 | G3.7-2 |
| `healthcare_exam_treatment_room` | Health care facility, exam/treatment room | 9.5.2.1-2 | G3.7-2 |
| `healthcare_hospital_corridor` | Health care facility, hospital corridor | 9.5.2.1-2 | G3.7-2 |
| `healthcare_imaging_room` | Health care facility, imaging room | 9.5.2.1-2 | — |
| `healthcare_lounge` | Health care facility, lounge | 9.5.2.1-2 | G3.7-2 |
| `healthcare_medical_supply_room` | Health care facility, medical supply room | 9.5.2.1-2 | G3.7-2 |
| `healthcare_nurse_station` | Health care facility, nurse station | 9.5.2.1-2 | G3.7-2 |
| `healthcare_nursery` | Health care facility, nursery | 9.5.2.1-2 | G3.7-2 |
| `healthcare_operating_room` | Health care facility, operating room | 9.5.2.1-2 | G3.7-2 |
| `healthcare_patient_room` | Health care facility, patient room | 9.5.2.1-2 | G3.7-2 |
| `healthcare_physical_therapy_room` | Health care facility, physical therapy room | 9.5.2.1-2 | G3.7-2 |
| `healthcare_recovery_room` | Health care facility, recovery room | 9.5.2.1-2 | G3.7-2 |
| `healthcare_telemedicine` | Health care facility, telemedicine | 9.5.2.1-2 | G3.7-2 |
| `laboratory_classroom` | Laboratory, in or as a classroom | 9.5.2.1-1 | — |
| `laboratory_other` | Laboratory, all other | 9.5.2.1-1 | G3.7-1 |
| `laboratory_preschool_to_12_lab_shop` | Laboratory, preschool through 12th grade, laboratory and shop classrooms | — | G3.7-1 |
| `laundry_washing` | Laundry/washing area | 9.5.2.1-1 | G3.7-1 |
| `library_reading_area` | Library, reading area | 9.5.2.1-2 | G3.7-2 |
| `library_stacks` | Library, stacks | 9.5.2.1-2 | G3.7-2 |
| `loading_dock_interior` | Loading dock, interior | 9.5.2.1-1 | G3.7-1 |
| `lobby_elevator` | Lobby, elevator | 9.5.2.1-1 | G3.7-1 |
| `lobby_hotel` | Lobby, hotel | — | G3.7-1 |
| `lobby_motion_picture_theater` | Lobby, motion picture theater | 9.5.2.1-1 | G3.7-1 |
| `lobby_other` | Lobby, all other | 9.5.2.1-1 | G3.7-1 |
| `lobby_performing_arts_theater` | Lobby, performing arts theater | 9.5.2.1-1 | G3.7-1 |
| `locker_room` | Locker room | 9.5.2.1-1 | G3.7-1 |
| `lounge_breakroom_other` | Lounge/breakroom, all other | 9.5.2.1-1 | G3.7-1 |
| `lounge_wellness_room` | Lounge/breakroom, wellness room | 9.5.2.1-1 | G3.7-1 |
| `manufacturing_detailed` | Manufacturing facility, detailed manufacturing area | 9.5.2.1-2 | G3.7-2 |
| `manufacturing_extra_high_bay` | Manufacturing facility, extra-high bay (>50 ft floor-to-ceiling) | 9.5.2.1-2 | G3.7-2 |
| `manufacturing_high_bay` | Manufacturing facility, high bay (25 to 50 ft floor-to-ceiling) | 9.5.2.1-2 | G3.7-2 |
| `manufacturing_low_bay` | Manufacturing facility, low bay (<25 ft floor-to-ceiling) | 9.5.2.1-2 | G3.7-2 |
| `museum_general_exhibition` | Museum, general exhibition area | 9.5.2.1-2 | G3.7-2 |
| `museum_restoration` | Museum, restoration area | 9.5.2.1-2 | G3.7-2 |
| `natatorium_class_1` | Natatorium, class I facility | 9.5.2.1-2 | G3.7-2 |
| `natatorium_class_2` | Natatorium, class II facility | 9.5.2.1-2 | G3.7-2 |
| `natatorium_class_3` | Natatorium, class III facility | 9.5.2.1-2 | G3.7-2 |
| `natatorium_class_4` | Natatorium, class IV facility | 9.5.2.1-2 | G3.7-2 |
| `office_150_to_300_sqft` | Office, >150 and ≤300 ft² | 9.5.2.1-1 | — |
| `office_enclosed_over_250_sqft` | Office, enclosed and >250 ft² | — | G3.7-1 |
| `office_enclosed_up_to_250_sqft` | Office, enclosed and ≤250 ft² | — | G3.7-1 |
| `office_open_plan` | Office, open plan | — | G3.7-1 |
| `office_over_300_sqft` | Office, >300 ft² | 9.5.2.1-1 | — |
| `office_up_to_150_sqft` | Office, ≤150 ft² | 9.5.2.1-1 | — |
| `parking_daylight_transition_zone` | Parking garage, daylight transition zone | 9.5.2.1-1 | G3.7-1 |
| `parking_other` | Parking garage, all other parking and drive areas | 9.5.2.1-1 | G3.7-1 |
| `performing_arts_dressing_room` | Performing arts theater, dressing room | 9.5.2.1-2 | G3.7-2 |
| `pharmacy_area` | Pharmacy area | 9.5.2.1-1 | G3.7-1 |
| `post_office_sorting_area` | Post office, sorting area | 9.5.2.1-2 | G3.7-2 |
| `religious_audience_seating` | Religious facility, audience seating area | 9.5.2.1-2 | G3.7-2 |
| `religious_fellowship_hall` | Religious facility, fellowship hall | 9.5.2.1-2 | G3.7-2 |
| `religious_worship_pulpit_choir` | Religious facility, worship/pulpit/choir area | 9.5.2.1-2 | G3.7-2 |
| `restroom` | Restroom | 9.5.2.1-1 | G3.7-1 |
| `retail_dressing_fitting_room` | Retail facility, dressing/fitting room | 9.5.2.1-2 | G3.7-2 |
| `retail_hair_care` | Retail facility, hair care | 9.5.2.1-2 | G3.7-2 |
| `retail_mall_concourse` | Retail facility, mall concourse | 9.5.2.1-2 | G3.7-2 |
| `retail_massage` | Retail facility, massage | 9.5.2.1-2 | G3.7-2 |
| `retail_nail_care` | Retail facility, nail care | 9.5.2.1-2 | G3.7-2 |
| `sales_area` | Sales area | 9.5.2.1-1 | G3.7-1 |
| `seating_area_general` | Seating area, general | 9.5.2.1-1 | G3.7-1 |
| `security_screening_general` | Security screening, general | 9.5.2.1-1 | G3.7-1 |
| `security_screening_transportation` | Security screening, transportation | 9.5.2.1-1 | G3.7-1 |
| `security_screening_transportation_queue` | Security screening, transportation queue | 9.5.2.1-1 | G3.7-1 |
| `sports_arena_class_1` | Sports arena, class I facility | 9.5.2.1-2 | G3.7-2 |
| `sports_arena_class_2` | Sports arena, class II facility | 9.5.2.1-2 | G3.7-2 |
| `sports_arena_class_3` | Sports arena, class III facility | 9.5.2.1-2 | G3.7-2 |
| `sports_arena_class_4` | Sports arena, class IV facility | 9.5.2.1-2 | G3.7-2 |
| `stairwell` | Stairwell | 9.5.2.1-1 | G3.7-1 |
| `storage_50_sqft_and_over` | Storage room, ≥50 ft² | 9.5.2.1-1 | G3.7-1 |
| `storage_under_50_sqft` | Storage room, <50 ft² | 9.5.2.1-1 | G3.7-1 |
| `transportation_airport_hangar` | Transportation facility, airport hangar | 9.5.2.1-2 | G3.7-2 |
| `transportation_baggage_carousel` | Transportation facility, baggage/carousel area | 9.5.2.1-2 | G3.7-2 |
| `transportation_concourse` | Transportation facility, concourse | 9.5.2.1-2 | G3.7-2 |
| `transportation_passenger_loading` | Transportation facility, passenger loading area | 9.5.2.1-2 | G3.7-2 |
| `transportation_ticket_counter` | Transportation facility, ticket counter | 9.5.2.1-2 | G3.7-2 |
| `vehicular_maintenance_area` | Vehicular maintenance area | 9.5.2.1-1 | G3.7-1 |
| `visually_impaired_chapel` | Facility for the visually impaired, chapel | 9.5.2.1-2 | G3.7-2 |
| `visually_impaired_corridor` | Facility for the visually impaired, corridor | 9.5.2.1-2 | G3.7-2 |
| `visually_impaired_dining` | Facility for the visually impaired, dining | 9.5.2.1-2 | G3.7-2 |
| `visually_impaired_lobby` | Facility for the visually impaired, lobby | 9.5.2.1-2 | G3.7-2 |
| `visually_impaired_recreation_room` | Facility for the visually impaired, recreation room/common living room | 9.5.2.1-2 | G3.7-2 |
| `visually_impaired_restroom` | Facility for the visually impaired, restroom | 9.5.2.1-2 | G3.7-2 |
| `warehouse_bulky_palletized` | Warehouse storage area, medium-to-bulky palletized items | 9.5.2.1-2 | G3.7-2 |
| `warehouse_smaller_items` | Warehouse storage area, smaller items/picking areas | 9.5.2.1-2 | G3.7-2 |
| `workshop` | Workshop (including workshop classrooms) | 9.5.2.1-1 | — |
| `workshop_other` | Workshop, all other | — | G3.7-1 |
| `workshop_preschool_to_12_lab_shop` | Workshop, preschool through 12th grade, laboratory and shop classrooms | — | G3.7-1 |

### Building area types (building area method)

Tables 9.5.1 (prescriptive) and G3.8 (Appendix G) list the same types:

`automotive_facility`, `convention_center`, `courthouse`, `dining_bar_lounge_leisure`, `dining_cafeteria_fast_food`, `dining_family`, `dormitory`, `exercise_center`, `fire_station`, `gymnasium`, `health_care_clinic`, `hospital`, `hotel_motel`, `library`, `manufacturing_facility`, `motion_picture_theater`, `multifamily`, `museum`, `office`, `parking_garage`, `penitentiary`, `performing_arts_theater`, `police_station`, `post_office`, `religious_facility`, `retail`, `school_university`, `sports_arena`, `town_hall`, `transportation`, `warehouse`, `workshop`
