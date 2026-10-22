//! Keeps the repo's models wound counter-clockwise from outside.
//!
//! The engine reverses surfaces that face into their zone and warns, so a bad
//! model still simulates correctly. The repo's own models should not rely on
//! that: they are copied as templates, and the warning would hide real ones.

use std::path::{Path, PathBuf};

use openbse_envelope::surface::orient_surfaces;
use openbse_io::input::ModelInput;

fn workspace_root() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

fn collect_yaml(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            collect_yaml(&p, out);
        } else if matches!(
            p.extension().and_then(|e| e.to_str()),
            Some("yaml") | Some("yml")
        ) {
            out.push(p);
        }
    }
}

#[test]
fn repo_models_need_no_winding_fixes() {
    let root = workspace_root();
    let mut files = Vec::new();
    for dir in ["examples", "140_tests/cases", "prototype_tests"] {
        collect_yaml(&root.join(dir), &mut files);
    }

    let mut checked = 0;
    let mut problems = Vec::new();
    for file in &files {
        let Ok(text) = std::fs::read_to_string(file) else {
            continue;
        };
        // Equipment and co-simulation files in these folders are not models.
        let Ok(mut model) = serde_yaml::from_str::<ModelInput>(&text) else {
            continue;
        };
        checked += 1;
        let rel = file.strip_prefix(&root).unwrap_or(file).display();
        for (name, fix) in orient_surfaces(&mut model.surfaces) {
            problems.push(format!("{rel}: {name} ({fix:?})"));
        }
    }

    assert!(
        checked > 20,
        "only {checked} models parsed; is the path right?"
    );
    assert!(
        problems.is_empty(),
        "surfaces wound into their zone (list vertices counter-clockwise from outside):\n  {}",
        problems.join("\n  ")
    );
}
