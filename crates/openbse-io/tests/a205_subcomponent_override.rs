//! Phase 3 integration test: standalone RS0005 / RS0006 / RS0007 file
//! references on a fan.

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
fn example_motor_override_builds() {
    let yaml = workspace_root().join("examples/vav_a205_motor_override.yaml");
    assert!(yaml.exists(), "example YAML not found: {}", yaml.display());
    let model = load_model(&yaml).expect("load_model");
    let graph = build_graph_with_base(&model, yaml.parent())
        .expect("build_graph_with_base should succeed for motor-override example");
    assert!(graph.component_count() > 0);
}

#[test]
fn missing_motor_a205_file_yields_clean_error() {
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
        a205_file: /nonexistent/fan.a205
        motor_a205_file: /nonexistent/motor.a205
    zone_terminals:
      - zone: Z1
"#;
    let model = parse_model_yaml(yaml).expect("parse");
    let err = match build_graph_with_base(&model, None) {
        Ok(_) => panic!("expected error"),
        Err(e) => e,
    };
    // The fan a205_file is missing first, so that error fires.  Either error
    // mentioning the fan name is acceptable.
    let msg = format!("{}", err);
    assert!(
        msg.contains("TestFan"),
        "error should mention fan name: {}",
        msg
    );
}

#[test]
fn missing_motor_override_is_loaded_not_ignored() {
    let fan_file = workspace_root().join("examples/equipment/Fan_Continuous.RS0003.a205.json");
    let yaml = format!(
        r#"
simulation: {{}}
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
        pressure_rise: 700.0
        a205_file: '{}'
        motor_a205_file: /nonexistent/motor.a205.json
    zone_terminals:
      - zone: Z1
"#,
        fan_file.display()
    );
    let model = parse_model_yaml(&yaml).expect("parse");
    let msg = match build_graph_with_base(&model, None) {
        Ok(_) => panic!("expected missing motor_a205_file to error"),
        Err(e) => e.to_string(),
    };
    assert!(msg.contains("motor_a205_file"), "{msg}");
}
