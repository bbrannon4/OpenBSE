//! Warmup runs on the run period's own days, with HVAC, and stays out of the
//! results (#189).

use std::path::PathBuf;
use std::process::{Command, Output};

fn workspace_root() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

fn run(name: &str, yaml: &str) -> (PathBuf, Output) {
    let dir = std::env::temp_dir().join(format!("openbse_warmup_{name}_{}", std::process::id()));
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

/// Mean of a zone column's first `n` values, and the number of rows.
fn first_mean(dir: &std::path::Path, file: &str, column: &str, n: usize) -> (f64, usize) {
    let text = std::fs::read_to_string(dir.join(file)).unwrap();
    let mut lines = text.lines();
    let col = lines
        .next()
        .unwrap()
        .split(',')
        .position(|h| h.starts_with(column))
        .unwrap_or_else(|| panic!("no column {column}"));
    let values: Vec<f64> = lines
        .map(|l| l.split(',').nth(col).unwrap().parse().unwrap())
        .collect();
    (values.iter().take(n).sum::<f64>() / n as f64, values.len())
}

#[test]
fn a_july_run_starts_from_july_and_warmup_is_not_reported() {
    let yaml = std::fs::read_to_string(workspace_root().join("examples/residential_unitary.yaml"))
        .expect("example model")
        .replacen("start_month: 1", "start_month: 7", 1)
        .replacen("end_month: 12", "end_month: 7", 1)
        .replacen("end_day: 31", "end_day: 3", 1);
    let (dir, output) = run("july", &yaml);
    let log = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(output.status.success(), "{log}");
    assert!(log.contains("Warmup"), "{log}");

    let (first_hours, rows) = first_mean(&dir, "july_results.csv", "Living Space:zone_temp", 6);
    // Three days of results only, whatever warmup ran.
    assert_eq!(rows, 3 * 24, "warmup must stay out of the results");
    // From a January warmup, the house would start cold and spend the morning
    // recovering. Warmed on its own July days, it starts within its deadband.
    assert!(
        (20.0..=26.0).contains(&first_hours),
        "first hours at {first_hours:.1} °C"
    );
    std::fs::remove_dir_all(dir).ok();
}

/// A zone too slow to settle in 28 days gets a warning in the .err file, not a
/// silent start from an unsettled state.
#[test]
fn unconverged_warmup_is_reported_in_the_err_file() {
    let yaml = std::fs::read_to_string(workspace_root().join("examples/1zone_uncontrolled.yaml"))
        .expect("example model")
        .replacen("end_month: 12", "end_month: 1", 1);
    let volume_line = yaml
        .lines()
        .find(|l| l.trim_start().starts_with("volume:"))
        .expect("zone volume")
        .to_string();
    // About 1000 times the air, so the zone's time constant is months.
    let yaml = yaml.replacen(&volume_line, "    volume: 1000000.0", 1);
    let (dir, output) = run("slow", &yaml);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let err = std::fs::read_to_string(dir.join("slow.err")).unwrap();
    assert!(
        err.contains("Warmup did not converge in 28 days") && err.contains("ZONE ONE"),
        "{err}"
    );
    std::fs::remove_dir_all(dir).ok();
}
