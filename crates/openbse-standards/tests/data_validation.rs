//! Every embedded data file must parse with the strict structs and pass the
//! semantic checks: SI values match their printed source, every climate zone
//! has one envelope table per method, keys are unique, and every class the
//! tables name has a typical assembly. A malformed data file fails CI here.

use openbse_standards::constructions::build_opaque;
use openbse_standards::data::{Element, TableKind};
use openbse_standards::library::embedded_files;
use openbse_standards::{Library, LightingMethod};

#[test]
fn embedded_data_is_valid() {
    if let Err(e) = Library::embedded() {
        panic!("{e}");
    }
    assert!(!embedded_files().is_empty());
}

#[test]
fn ashrae_90_1_2025_has_every_table() {
    let lib = Library::embedded().unwrap();
    let ed = lib.edition("ashrae_90_1", "2025").unwrap();
    let mut tables: Vec<&str> = ed.tables.iter().map(|t| t.si.table.as_str()).collect();
    tables.sort();
    let mut want = vec![
        "9.5.1",
        "9.5.2.1-1",
        "9.5.2.1-2",
        "G3.7-1",
        "G3.7-2",
        "G3.8",
    ];
    let cz: Vec<String> = (0..=8).map(|z| format!("5.5-{z}")).collect();
    let g: Vec<String> = (1..=8).map(|z| format!("G3.4-{z}")).collect();
    want.extend(cz.iter().map(|s| s.as_str()));
    want.extend(g.iter().map(|s| s.as_str()));
    want.sort();
    assert_eq!(tables, want);
    let rows: usize = ed
        .tables
        .iter()
        .filter(|t| t.si.kind != TableKind::Envelope)
        .map(|t| t.si.lighting.len())
        .sum();
    assert_eq!(rows, 55 + 65 + 58 + 64 + 32 * 2);
}

/// Spot values against the printed tables, through the lookup API.
#[test]
fn lookups_return_printed_values_in_si() {
    let lib = Library::embedded().unwrap();
    let ed = lib.edition("ashrae_90_1", "2025").unwrap();
    let u = openbse_standards::units::U_IP_TO_SI;
    let lpd = openbse_standards::units::LPD_IP_TO_SI;

    let roof = ed
        .opaque(
            "prescriptive",
            "5B",
            Element::Roof,
            "insulation_entirely_above_deck",
            "nonresidential",
        )
        .unwrap();
    assert!((roof.si.unwrap() - 0.032 * u).abs() < 1e-6);
    assert_eq!(roof.citation.table, "5.5-5");
    assert!(roof.citation.printed.contains("0.032"));

    let slab = ed
        .opaque(
            "appendix_g",
            "4a",
            Element::SlabOnGrade,
            "unheated",
            "nonresidential",
        )
        .unwrap();
    assert!((slab.si.unwrap() - 0.730 * openbse_standards::units::F_FACTOR_IP_TO_SI).abs() < 1e-6);
    assert_eq!(slab.citation.table, "G3.4-4");

    // Climate zone 7 has no subzones; "7A" is accepted as 7.
    assert_eq!(ed.climate_zone("7A").unwrap(), "7");
    assert!(ed.climate_zone("9").is_err());

    // Appendix G glazing is banded by window-to-wall ratio; 3C has its own rows.
    let g = ed
        .fenestration(
            "appendix_g",
            "3B",
            Element::VerticalFenestration,
            "all",
            "nonresidential",
            15.0,
        )
        .unwrap();
    assert!((g.shgc.si.unwrap() - 0.25).abs() < 1e-9);
    let g3c = ed
        .fenestration(
            "appendix_g",
            "3C",
            Element::VerticalFenestration,
            "all",
            "nonresidential",
            15.0,
        )
        .unwrap();
    assert!((g3c.u_factor.si.unwrap() - 1.22 * u).abs() < 1e-6);
    assert!((g3c.shgc.si.unwrap() - 0.39).abs() < 1e-9);
    assert!(ed
        .fenestration(
            "appendix_g",
            "3B",
            Element::VerticalFenestration,
            "all",
            "nonresidential",
            45.0
        )
        .is_err());

    // Semiheated SHGC is NR in the prescriptive tables.
    let semi = ed
        .fenestration(
            "prescriptive",
            "5A",
            Element::VerticalFenestration,
            "fixed",
            "semiheated",
            30.0,
        )
        .unwrap();
    assert!(semi.shgc.si.is_none());
    assert_eq!(semi.shgc.citation.printed, "NR");

    let office = ed
        .lighting(
            "prescriptive",
            LightingMethod::SpaceBySpace,
            "office_up_to_150_sqft",
        )
        .unwrap();
    assert!((office.lpd_si(3.0) - 0.69 * lpd).abs() < 1e-5);
    assert_eq!(office.table.si.table, "9.5.2.1-1");
    let ba = ed
        .lighting("appendix_g", LightingMethod::BuildingArea, "office")
        .unwrap();
    assert!((ba.lpd_si(3.0) - 1.00 * lpd).abs() < 1e-5);
    // Atrium LPD depends on height: 0.50 + 0.025 W/ft² per ft.
    let atrium = ed
        .lighting(
            "appendix_g",
            LightingMethod::SpaceBySpace,
            "atrium_over_40ft",
        )
        .unwrap();
    let h = 15.24; // 50 ft
    assert!((atrium.lpd_si(h) - (0.50 + 0.025 * 50.0) * lpd).abs() < 1e-4);
    let miss = ed
        .lighting("prescriptive", LightingMethod::SpaceBySpace, "office_open")
        .unwrap_err();
    assert!(miss.contains("office_up_to_150_sqft"), "{miss}");
}

/// Every opaque class in every table builds to its target U exactly.
#[test]
fn every_opaque_class_builds_to_its_target_u() {
    let lib = Library::embedded().unwrap();
    for ed in &lib.editions {
        for t in ed
            .tables
            .iter()
            .filter(|t| t.si.kind == TableKind::Envelope)
        {
            let cz = &t.si.climate_zones[0];
            for row in &t.si.opaque {
                let Some(aref) = ed.assembly_ref(row.element, &row.class) else {
                    continue; // slab and below-grade use ground methods
                };
                let assembly = lib.assembly(&aref.assembly).unwrap();
                let r_films = ed.film_si(aref.outside_film) + ed.film_si(aref.inside_film);
                for cat in row.criteria.keys() {
                    let hit = ed
                        .opaque(&t.si.method, cz, row.element, &row.class, cat)
                        .unwrap();
                    let b = build_opaque("c", assembly, &lib.typical.materials, hit.si, r_films)
                        .unwrap();
                    match (hit.si, &b.note) {
                        (Some(target), None) => assert!(
                            (b.u_with_films - target).abs() < 1e-9,
                            "{} {} {} {cat}",
                            t.si.table,
                            row.element.key(),
                            row.class
                        ),
                        (Some(target), Some(_)) => assert!(b.u_with_films <= target),
                        (None, _) => assert_eq!(b.insulation_r, 0.0),
                    }
                }
            }
        }
    }
}

#[test]
fn a_malformed_file_is_rejected() {
    let mut files: Vec<(&str, &str)> = embedded_files().to_vec();
    // A misspelt key in one table must fail, naming the file.
    let i = files
        .iter()
        .position(|(p, _)| p.ends_with("si/table_9.5.1.yaml"))
        .unwrap();
    let bad = files[i].1.replacen("lpd:", "lpdd:", 1);
    files[i].1 = &bad;
    let errs = Library::from_files(&files).unwrap_err();
    assert!(
        errs.iter().any(|e| e.contains("table_9.5.1.yaml")),
        "{errs:?}"
    );

    // A converted SI value that drifts from its printed source must fail.
    let mut files: Vec<(&str, &str)> = embedded_files().to_vec();
    let i = files
        .iter()
        .position(|(p, _)| p.ends_with("ip/table_5.5-5.yaml"))
        .unwrap();
    let bad = files[i]
        .1
        .replacen("u_factor: 0.032}", "u_factor: 0.033}", 1);
    files[i].1 = &bad;
    let errs = Library::from_files(&files).unwrap_err();
    assert!(errs.iter().any(|e| e.contains("5.5-5")), "{errs:?}");
}
