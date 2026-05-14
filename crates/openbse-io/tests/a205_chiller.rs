//! Integration test: parse the example VAV+CHW model that references a
//! Standard 205 chiller file, and verify the simulation graph builds.
//! This exercises the full pipeline: YAML parsing → relative-path
//! resolution → RS0001 file load → ChillerA205 construction.

use openbse_io::input::{build_graph_with_base, load_model, parse_model_yaml};
use std::path::PathBuf;

fn workspace_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is .../crates/openbse-io; go up two levels.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

#[test]
fn example_vav_chw_plant_a205_builds() {
    let yaml = workspace_root().join("examples/vav_chw_plant_a205.yaml");
    assert!(yaml.exists(), "example YAML not found: {}", yaml.display());

    let model = load_model(&yaml).expect("load_model");
    let graph = build_graph_with_base(&model, yaml.parent())
        .expect("build_graph_with_base should succeed for example model");

    // Should have a non-trivial number of components.
    assert!(graph.component_count() > 0);
}

#[test]
fn missing_a205_file_yields_clean_error() {
    // Bare-bones model with a chiller that points at a nonexistent file.
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

plant_loops:
  - name: CHW Loop
    design_supply_temp: 7.0
    design_delta_t: 5.0
    supply_equipment:
      - type: chiller
        name: TestChiller
        chw_setpoint: 7.0
        a205_file: /nonexistent/path/does_not_exist.a205
"#;
    let model = parse_model_yaml(yaml).expect("parse");
    let result = build_graph_with_base(&model, None);
    let err = match result {
        Ok(_) => panic!("expected error when a205_file is missing"),
        Err(e) => e,
    };
    let msg = format!("{}", err);
    assert!(
        msg.contains("a205_file") && msg.contains("TestChiller"),
        "error should mention the file and chiller name; got: {}",
        msg
    );
}
