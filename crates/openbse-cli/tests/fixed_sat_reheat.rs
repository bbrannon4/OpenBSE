//! A terminal-reheat zone on a fixed cold deck reheats only to its heating
//! setpoint, as E+ SimVAV: with a deadband it floats down on the cold
//! constant-volume flow and is held near the heating setpoint, not reheated up
//! to the cooling setpoint.

use std::path::PathBuf;
use std::process::Command;

fn workspace_root() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

#[test]
fn constant_volume_reheat_zone_holds_its_heating_setpoint() {
    let root = workspace_root();
    let case =
        std::fs::read_to_string(root.join("140_tests/cases/ashrae140_caseAE304.yaml")).unwrap();
    let weather = root.join("140_tests/weather/AE_269.epw");
    // AE304 has one setpoint per zone; give each a 3 K deadband.
    let yaml = case
        .replace(
            "\"../weather/AE_269.epw\"",
            &format!("\"{}\"", weather.display()),
        )
        .replacen(
            "    heating_setpoint: 23.889\n",
            "    heating_setpoint: 21.0\n",
            1,
        )
        .replacen(
            "    cooling_setpoint: 23.889\n",
            "    cooling_setpoint: 24.0\n",
            1,
        )
        .replacen(
            "    heating_setpoint: 25.0\n",
            "    heating_setpoint: 22.0\n",
            1,
        );
    assert!(yaml.contains("heating_setpoint: 21.0") && yaml.contains("heating_setpoint: 22.0"));
    let dir = std::env::temp_dir().join(format!("openbse_fixed_sat_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let model = dir.join("db.yaml");
    std::fs::write(&model, yaml).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_openbse"))
        .arg(&model)
        .current_dir(&dir)
        .output()
        .expect("run openbse");
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let zone = std::fs::read_to_string(dir.join("db_zone_results.csv")).unwrap();
    let last: Vec<f64> = zone
        .lines()
        .last()
        .unwrap()
        .split(',')
        .skip(3)
        .map(|v| v.parse().unwrap())
        .collect();
    let _ = std::fs::remove_dir_all(&dir);
    for (t, (hsp, csp)) in last.iter().zip([(21.0, 24.0), (22.0, 25.0)]) {
        assert!(
            (t - hsp).abs() < 1.0 && *t < csp - 2.0,
            "zone at {t} °C with setpoints {hsp}/{csp}: reheated toward cooling"
        );
    }
}
