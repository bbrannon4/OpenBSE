//! The `standard:` block fills lighting and constructions at load, explicit
//! inputs win, and gaps fail loudly.

use openbse_io::input::parse_model_yaml;
use openbse_io::standards::apply_standards;

const U_IP: f64 = 5.678_263_340_863_113;
const LPD_IP: f64 = 10.763_910_416_709_722;

/// The SI data files keep 7 significant figures of the exact conversion.
fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-6 * b.abs()
}

fn square_zone(zone: &str, roof_construction: Option<&str>) -> String {
    let c = |s: Option<&str>| {
        s.map(|c| format!("    construction: {c}\n"))
            .unwrap_or_default()
    };
    format!(
        r#"
  - name: {zone} South
    zone: {zone}
    type: wall
    boundary: outdoor
    vertices: [{{x: 0, y: 0, z: 0}}, {{x: 10, y: 0, z: 0}}, {{x: 10, y: 0, z: 3}}, {{x: 0, y: 0, z: 3}}]
  - name: {zone} Window
    zone: {zone}
    type: window
    parent_surface: {zone} South
    vertices: [{{x: 1, y: 0, z: 1}}, {{x: 4, y: 0, z: 1}}, {{x: 4, y: 0, z: 2}}, {{x: 1, y: 0, z: 2}}]
  - name: {zone} Roof
    zone: {zone}
    type: roof
    boundary: outdoor
{roof}    vertices: [{{x: 0, y: 0, z: 3}}, {{x: 10, y: 0, z: 3}}, {{x: 10, y: 10, z: 3}}, {{x: 0, y: 10, z: 3}}]
  - name: {zone} Floor
    zone: {zone}
    type: floor
    boundary: ground
    exposed_perimeter: 10
    vertices: [{{x: 0, y: 10, z: 0}}, {{x: 10, y: 10, z: 0}}, {{x: 10, y: 0, z: 0}}, {{x: 0, y: 0, z: 0}}]
"#,
        roof = c(roof_construction)
    )
}

fn model(method: &str, extra: &str, zones: &str, surfaces: &str) -> String {
    format!(
        r#"
simulation: {{timesteps_per_hour: 1}}
weather_files: []
climate_zone: 5B
standard:
  name: ashrae_90_1
  edition: "2025"
  method: {method}
  lighting: {{method: space_by_space}}
  envelope: {{}}
{extra}
zones:
{zones}
surfaces:
{surfaces}
"#
    )
}

#[test]
fn fills_lights_and_constructions_from_the_tables() {
    let y = model(
        "prescriptive",
        "",
        "  - name: Z\n    space_type: office_over_300_sqft\n",
        &square_zone("Z", None),
    );
    let mut m = parse_model_yaml(&y).unwrap();
    let r = apply_standards(&mut m).unwrap().unwrap();
    assert_eq!(m.lights.len(), 1);
    assert!(close(m.lights[0].watts_per_area.unwrap(), 0.52 * LPD_IP));
    // Every surface now has a construction, and the model validates.
    assert!(m.surfaces.iter().all(|s| !s.construction.is_empty()));
    let v = openbse_io::validate_model(&m);
    assert_eq!(v.error_count(), 0, "{}", v.to_err_file());
    // Window from Table 5.5-5, fixed, nonresidential.
    let w = &m.window_constructions[0];
    assert!(close(w.u_factor, 0.32 * U_IP) && (w.shgc - 0.38).abs() < 1e-12);
    // Slab from the F-factor row.
    assert!(close(
        m.f_factor_constructions[0].f_factor,
        0.52 * 1.730_734_908
    ));
    // Every value is cited.
    assert!(r.applied.iter().any(|a| a.citation.table == "9.5.2.1-1"));
    assert!(r
        .applied
        .iter()
        .all(|a| a.citation.table == "9.5.2.1-1" || a.citation.table == "5.5-5"));
}

#[test]
fn layered_roof_meets_the_table_u_with_appendix_a_films() {
    let y = model(
        "prescriptive",
        "",
        "  - name: Z\n    space_type: corridor\n",
        &square_zone("Z", None),
    );
    let mut m = parse_model_yaml(&y).unwrap();
    apply_standards(&mut m).unwrap();
    let roof = m
        .constructions
        .iter()
        .find(|c| c.name.contains(" roof "))
        .unwrap();
    let mats: std::collections::HashMap<_, _> = m
        .materials
        .iter()
        .map(|x| (x.name.clone(), x.clone()))
        .collect();
    let r_layers = roof.total_resistance(&mats).unwrap();
    // Appendix A films for roofs: exterior R-0.17, interior heat flow up R-0.61.
    let r_films = (0.17 + 0.61) / U_IP;
    let u = 1.0 / (r_layers + r_films);
    assert!(close(u, 0.032 * U_IP), "U = {u}");
}

#[test]
fn explicit_inputs_win() {
    let extra = r#"
simple_constructions:
  - {name: My Roof, u_factor: 0.2}
lights:
  - {name: My Lights, zones: [Z], power: 123}
"#;
    let y = model(
        "prescriptive",
        extra,
        "  - name: Z\n    space_type: corridor\n",
        &square_zone("Z", Some("My Roof")),
    );
    let mut m = parse_model_yaml(&y).unwrap();
    let r = apply_standards(&mut m).unwrap().unwrap();
    assert_eq!(m.lights.len(), 1);
    assert_eq!(m.lights[0].power, 123.0);
    let roof = m.surfaces.iter().find(|s| s.name == "Z Roof").unwrap();
    assert_eq!(roof.construction, "My Roof");
    assert!(!m.constructions.iter().any(|c| c.name.contains(" roof ")));
    assert!(r.kept_explicit.iter().any(|k| k.contains("lights")));
}

#[test]
fn appendix_g_uses_baseline_tables_and_per_method_space_types() {
    let zones = "  - name: Z\n    space_type: {prescriptive: office_over_300_sqft, appendix_g: office_open_plan}\n";
    let y = model("appendix_g", "", zones, &square_zone("Z", None));
    let mut m = parse_model_yaml(&y).unwrap();
    let r = apply_standards(&mut m).unwrap().unwrap();
    assert!(close(m.lights[0].watts_per_area.unwrap(), 1.10 * LPD_IP));
    // 3 m² of window in 30 m² of wall: 10% WWR, the first G3.4-5 band.
    let w = &m.window_constructions[0];
    assert!(close(w.u_factor, 0.57 * U_IP), "{}", w.u_factor);
    assert!(r.applied.iter().any(|a| a.citation.table == "G3.4-5"));
    assert!(r.applied.iter().any(|a| a.citation.table == "G3.7-1"));
}

#[test]
fn gaps_fail_with_a_reason() {
    // A conditioned zone without a space type.
    let y = model("prescriptive", "", "  - name: Z\n", &square_zone("Z", None));
    let e = apply_standards(&mut parse_model_yaml(&y).unwrap())
        .unwrap_err()
        .to_string();
    assert!(
        e.contains("no a space_type") || e.contains("space_type"),
        "{e}"
    );

    // An unknown space type names similar keys.
    let y = model(
        "prescriptive",
        "",
        "  - name: Z\n    space_type: office_huge\n",
        &square_zone("Z", None),
    );
    let e = apply_standards(&mut parse_model_yaml(&y).unwrap())
        .unwrap_err()
        .to_string();
    assert!(e.contains("office_over_300_sqft"), "{e}");

    // A below-grade wall: no C-factor method yet, so it must be explicit.
    let wall = r#"
  - name: Basement Wall
    zone: Z
    type: wall
    boundary: ground
    vertices: [{x: 0, y: 0, z: -3}, {x: 10, y: 0, z: -3}, {x: 10, y: 0, z: 0}, {x: 0, y: 0, z: 0}]
"#;
    let y = model(
        "prescriptive",
        "",
        "  - name: Z\n    space_type: corridor\n",
        &format!("{}{wall}", square_zone("Z", None)),
    );
    let e = apply_standards(&mut parse_model_yaml(&y).unwrap())
        .unwrap_err()
        .to_string();
    assert!(e.contains("C-factor") && e.contains("Basement Wall"), "{e}");

    // An unknown edition.
    let y = model(
        "prescriptive",
        "",
        "  - name: Z\n    space_type: corridor\n",
        &square_zone("Z", None),
    )
    .replace("\"2025\"", "\"1999\"");
    let e = apply_standards(&mut parse_model_yaml(&y).unwrap())
        .unwrap_err()
        .to_string();
    assert!(e.contains("1999"), "{e}");
}

#[test]
fn surface_without_construction_or_standard_is_a_severe_error() {
    let y = model("prescriptive", "", "  - name: Z\n", &square_zone("Z", None));
    let mut m = parse_model_yaml(&y).unwrap();
    m.standard = None;
    let v = openbse_io::validate_model(&m);
    assert!(
        v.to_err_file().contains("has no construction"),
        "{}",
        v.to_err_file()
    );
}
