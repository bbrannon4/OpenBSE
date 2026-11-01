//! A model may serve some zones with ideal loads and others with air loops, and
//! both must run (#191): before, one ideal-loads zone silently turned off every
//! air loop, plant loop, VRF and radiant panel in the model.

use std::path::PathBuf;
use std::process::{Command, Output};

fn workspace_root() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

fn run(name: &str, yaml: &str) -> (PathBuf, Output) {
    let dir = std::env::temp_dir().join(format!("openbse_mixed_{name}_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let model = dir.join(format!("{name}.yaml"));
    std::fs::write(&model, yaml).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_openbse"))
        .arg(&model)
        .arg("-w")
        .arg(workspace_root().join("140_tests/weather/725650TYCST.epw"))
        .current_dir(&dir)
        .output()
        .expect("run openbse");
    (dir, output)
}

/// January `residential_unitary`, with `extra_top` added at the end of the file.
fn january_model(extra_top: &str) -> String {
    let yaml = std::fs::read_to_string(workspace_root().join("examples/residential_unitary.yaml"))
        .expect("example model");
    yaml.replacen("end_month: 12", "end_month: 1", 1) + extra_top
}

/// Annual value of a row of the summary CSV.
fn summary_total(dir: &std::path::Path, stem: &str, row: &str) -> f64 {
    let text = std::fs::read_to_string(dir.join(format!("{stem}_summary.csv"))).unwrap();
    text.lines()
        .find(|l| l.starts_with(&format!("{row},")))
        .and_then(|l| l.rsplit(',').next())
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| panic!("no '{row}' row in summary"))
}

#[test]
fn ideal_loads_zone_and_air_loop_zone_both_run() {
    let base = january_model("");
    // A second zone, one exposed wall, ideal loads; the air loop still serves
    // Living Space.
    let mixed = base.replacen(
        "zones:\n  - name: Living Space\n",
        "zones:\n  - name: Annex\n    volume: 100.0\n    floor_area: 40.0\n  - name: Living Space\n",
        1,
    ) + r#"
ideal_loads:
  - name: Annex Ideal
    zones: [Annex]
    heating_setpoint: 20.0
    cooling_setpoint: 27.0
"#;
    let mixed = mixed.replacen(
        "surfaces:\n",
        r#"surfaces:
  - name: Annex Wall
    zone: Annex
    type: wall
    construction: Exterior Wall
    boundary: outdoor
    vertices:
      - {x: 30.0, y: 0.0, z: 0.0}
      - {x: 40.0, y: 0.0, z: 0.0}
      - {x: 40.0, y: 0.0, z: 2.5}
      - {x: 30.0, y: 0.0, z: 2.5}
"#,
        1,
    );

    let (dir_base, output) = run("base", &base);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let (dir, output) = run("mixed", &mixed);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    // The air loop's zone is unchanged by the separate annex, so its furnace
    // burns the same gas as in the model without the annex (within the small
    // effect of autosizing and solar reflections).
    let gas_base = summary_total(&dir_base, "base", "Heating (Gas)");
    let gas_mixed = summary_total(&dir, "mixed", "Heating (Gas)");
    assert!(gas_base > 100.0, "January gas {gas_base}");
    assert!(
        (gas_mixed - gas_base).abs() < 0.02 * gas_base,
        "air loop stopped or changed: {gas_mixed} vs {gas_base} kWh"
    );

    // The annex's ideal heating is reported on top of the zone load the furnace meets.
    let load_base = summary_total(&dir_base, "base", "Heating");
    let load_mixed = summary_total(&dir, "mixed", "Heating");
    assert!(
        load_mixed > load_base + 10.0,
        "{load_mixed} vs {load_base} kWh"
    );

    let err = std::fs::read_to_string(dir.join("mixed.err")).unwrap();
    assert!(err.contains("0 severe error(s)"), "{err}");
    std::fs::remove_dir_all(dir).ok();
    std::fs::remove_dir_all(dir_base).ok();
}

#[test]
fn a_zone_with_ideal_loads_and_an_air_loop_is_a_severe_error() {
    let yaml = january_model(
        r#"
ideal_loads:
  - name: Conflict
    zones: [Living Space]
    heating_setpoint: 20.0
    cooling_setpoint: 27.0
"#,
    );
    let (dir, output) = run("conflict", &yaml);
    assert!(!output.status.success(), "should stop");
    let err = std::fs::read_to_string(dir.join("conflict.err")).unwrap();
    assert!(
        err.contains("Living Space") && err.contains("ideal loads"),
        "{err}"
    );
    std::fs::remove_dir_all(dir).ok();
}
