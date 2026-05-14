//! Integration test: parse the residential-unitary example that uses an
//! ASHRAE Standard 205 RS0004 DX cooling coil, build the simulation graph
//! end-to-end, and confirm a chiller-style missing-file error path.

use openbse_io::input::{build_graph_with_base, load_model, parse_model_yaml};
use std::path::PathBuf;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

#[test]
fn example_residential_unitary_a205_builds() {
    let yaml = workspace_root().join("examples/residential_unitary_a205.yaml");
    assert!(yaml.exists(), "example YAML not found: {}", yaml.display());

    let model = load_model(&yaml).expect("load_model");
    let graph = build_graph_with_base(&model, yaml.parent())
        .expect("build_graph_with_base should succeed for example model");

    assert!(graph.component_count() > 0);
}

#[test]
fn missing_dx_a205_file_yields_clean_error() {
    let yaml = r#"
simulation:
  timesteps_per_hour: 1
  start_month: 1
  start_day: 1
  end_month: 1
  end_day: 1

weather_files: []

zones:
  - name: Z1
    volume: 100.0
    floor_area: 25.0

air_loops:
  - name: AHU
    equipment:
      - type: cooling_coil
        name: TestDX
        source: dx
        setpoint: 13.0
        a205_file: /nonexistent/path/does_not_exist.a205
    zone_terminals:
      - zone: Z1
"#;
    let model = parse_model_yaml(yaml).expect("parse");
    let result = build_graph_with_base(&model, None);
    let err = match result {
        Ok(_) => panic!("expected error when a205_file is missing"),
        Err(e) => e,
    };
    let msg = format!("{}", err);
    assert!(
        msg.contains("a205_file") && msg.contains("TestDX"),
        "error should mention the file and coil name; got: {}",
        msg
    );
}
