//! `--output` into a folder that doesn't exist creates it, and every output
//! lands there; a folder that can't be created fails before the run.

use std::path::PathBuf;
use std::process::Command;

fn workspace_root() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

const MODEL: &str = "simulation: {start_month: 1, start_day: 1, end_month: 1, end_day: 2}
weather_files: []
";

#[test]
fn output_folder_is_created() {
    let root = workspace_root();
    let dir = std::env::temp_dir().join(format!("openbse_outdir_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let model = dir.join("m.yaml");
    std::fs::write(&model, MODEL).unwrap();
    let out_dir = dir.join("new/nested");
    let out = Command::new(env!("CARGO_BIN_EXE_openbse"))
        .arg(&model)
        .arg("-w")
        .arg(root.join("weather/USA_CO_Boulder.Muni.AP.720533_TMYx.2009-2023.epw"))
        .arg("-o")
        .arg(out_dir.join("results.csv"))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out_dir.join("results.csv").is_file());
    assert!(out_dir.join("m.err").is_file());
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn uncreatable_output_folder_fails_before_the_run() {
    let dir = std::env::temp_dir().join(format!("openbse_outdir_bad_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let model = dir.join("m.yaml");
    std::fs::write(&model, MODEL).unwrap();
    // A regular file where a folder is needed.
    std::fs::write(dir.join("blocker"), "").unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_openbse"))
        .arg(&model)
        .arg("-o")
        .arg(dir.join("blocker/results.csv"))
        .output()
        .unwrap();
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("Failed to create output folder"),
        "{stderr}"
    );
    assert!(!stderr.contains("Simulating"), "{stderr}");
    std::fs::remove_dir_all(&dir).ok();
}
