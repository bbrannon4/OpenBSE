//! A zone multiplier of N must scale every end use by exactly N (#280). The run
//! simulates one instance of a zone; sizing it for all instances, or multiplying
//! its energy twice, breaks this while each piece looks fine on its own.

use std::path::{Path, PathBuf};
use std::process::Command;

fn workspace_root() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

/// January of retail_rtu, with `multiplier` on every zone.
fn run(dir: &Path, stem: &str, multiplier: u32) -> String {
    let root = workspace_root();
    let source = std::fs::read_to_string(root.join("examples/retail_rtu.yaml")).unwrap();
    let (head, zones) = source.split_once("\nzones:").unwrap();
    let (zones, tail) = zones.split_once("\n\n# ").unwrap_or((zones, ""));
    let zones = zones.replace(
        "\n  - name:",
        &format!("\n  - zone_multiplier: {multiplier}\n    name:"),
    );
    let model =
        format!("{head}\nzones:{zones}\n\n# {tail}").replace("end_month: 12", "end_month: 1");
    let path = dir.join(format!("{stem}.yaml"));
    std::fs::write(&path, model).unwrap();

    let weather = root.join("weather/USA_CO_Boulder.Muni.AP.720533_TMYx.2009-2023.epw");
    let run = Command::new(env!("CARGO_BIN_EXE_openbse"))
        .arg(&path)
        .arg("-w")
        .arg(weather)
        .arg("-o")
        .arg(dir.join(stem))
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    std::fs::read_to_string(dir.join(format!("{stem}_summary.txt"))).unwrap()
}

/// First monthly value on the summary line starting with `label`.
fn january(summary: &str, label: &str) -> f64 {
    let line = summary
        .lines()
        .find(|l| l.trim_start().starts_with(label))
        .unwrap_or_else(|| panic!("no '{label}' in summary"));
    line.trim_start()[label.len()..]
        .split_whitespace()
        .next()
        .unwrap()
        .parse()
        .unwrap()
}

#[test]
fn multiplier_scales_every_end_use_exactly() {
    let dir = std::env::temp_dir().join(format!("openbse_zone_mult_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let one = run(&dir, "one", 1);
    let two = run(&dir, "two", 2);
    let _ = std::fs::remove_dir_all(&dir);

    for label in [
        "Interior Lighting",
        "Interior Equipment",
        "Fans (Electric)",
        "Cooling (Electric)",
        "Heating (Gas)",
    ] {
        let (a, b) = (january(&one, label), january(&two, label));
        assert!(a > 0.0, "{label} should be nonzero");
        assert!(
            (b - 2.0 * a).abs() <= 0.2,
            "{label}: {a} kWh at multiplier 1, {b} kWh at multiplier 2"
        );
    }
}
