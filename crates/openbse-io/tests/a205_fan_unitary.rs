//! Phase 2 integration tests: RS0003 fan + RS0002 unitary wrapper.
//!
//! Covers two example models:
//!   * `vav_a205_fan_and_chiller.yaml` — uses an RS0003 fan file alongside
//!     an RS0001 chiller file in the same model.
//!   * `residential_unitary_rs0002.yaml` — points a cooling coil at an
//!     RS0002 wrapper file, exercising the auto-unwrap path.

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
fn example_vav_a205_fan_and_chiller_builds() {
    let yaml = workspace_root().join("examples/vav_a205_fan_and_chiller.yaml");
    assert!(yaml.exists(), "example YAML not found: {}", yaml.display());
    let model = load_model(&yaml).expect("load_model");
    let graph = build_graph_with_base(&model, yaml.parent())
        .expect("build_graph_with_base should succeed for fan+chiller a205 example");
    assert!(graph.component_count() > 0);
}

#[test]
fn example_residential_unitary_rs0002_builds() {
    let yaml = workspace_root().join("examples/residential_unitary_rs0002.yaml");
    assert!(yaml.exists(), "example YAML not found: {}", yaml.display());
    let model = load_model(&yaml).expect("load_model");
    let graph = build_graph_with_base(&model, yaml.parent())
        .expect("build_graph_with_base should succeed for RS0002 unitary example");
    assert!(graph.component_count() > 0);
}

#[test]
fn missing_fan_a205_file_yields_clean_error() {
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
      - type: fan
        name: TestFan
        source: constant_volume
        pressure_rise: 700.0
        a205_file: /nonexistent/fan_does_not_exist.a205
    zone_terminals:
      - zone: Z1
"#;
    let model = parse_model_yaml(yaml).expect("parse");
    let result = build_graph_with_base(&model, None);
    let err = match result {
        Ok(_) => panic!("expected error when fan a205_file is missing"),
        Err(e) => e,
    };
    let msg = format!("{}", err);
    assert!(
        msg.contains("a205_file") && msg.contains("TestFan"),
        "error should mention the file and fan name; got: {}",
        msg
    );
}
