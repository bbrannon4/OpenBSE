//! Every model YAML shipped in the repo must parse with the strict parser
//! (no unknown keys), so examples cannot drift from the input structs.

use std::path::{Path, PathBuf};

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("yaml") {
            out.push(path);
        }
    }
}

#[test]
fn shipped_models_parse() {
    let root = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."));
    let mut files = Vec::new();
    for dir in ["examples", "140_tests/cases", "prototype_tests"] {
        collect(&root.join(dir), &mut files);
    }
    files.sort();
    assert!(!files.is_empty());
    let failures: Vec<String> = files
        .iter()
        .filter_map(|f| {
            let text = std::fs::read_to_string(f).unwrap();
            openbse_io::input::parse_model_yaml(&text)
                .err()
                .map(|e| format!("{}: {e}", f.strip_prefix(&root).unwrap_or(f).display()))
        })
        .collect();
    assert!(
        failures.is_empty(),
        "{} of {} shipped models failed to parse:\n{}",
        failures.len(),
        files.len(),
        failures.join("\n")
    );
}

fn parse_error(yaml: &str) -> String {
    match openbse_io::input::parse_model_yaml(yaml) {
        Ok(_) => panic!("expected a parse error"),
        Err(e) => e.to_string(),
    }
}

#[test]
fn misspelt_keys_are_rejected() {
    // #127: an unknown key used to be dropped and its default applied.
    let top = parse_error("simulaton:\n  timesteps_per_hour: 4\n");
    assert!(top.contains("simulaton"), "{top}");

    let zone = parse_error("zones:\n  - name: Z\n    volume: 10\n    flor_area: 5\n");
    assert!(zone.contains("flor_area"), "{zone}");

    // Equipment entries are an internally tagged enum, which serde buffers;
    // the per-struct check still has to catch keys inside them.
    let fan = parse_error(
        "air_loops:\n  - name: L\n    equipment:\n      - type: fan\n        name: F\n        \
         source: constant_volume\n        design_flow_rate: 1.0\n        pressure_rise: 500\n        \
         motor_efficency: 0.9\n",
    );
    assert!(fan.contains("motor_efficency"), "{fan}");
}
