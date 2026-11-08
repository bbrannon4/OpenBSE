//! End-to-end check of the `comfort_grid` output (#136): runs the example
//! model for three weeks and checks the JSON contract the Workbench relies on.

use std::path::PathBuf;
use std::process::Command;

fn workspace_root() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

#[test]
fn example_writes_consistent_comfort_json() {
    let root = workspace_root();
    let yaml = std::fs::read_to_string(root.join("examples/comfort_perimeter_office.yaml"))
        .expect("example model");
    // Three weeks keeps the run short while covering weekdays and weekends.
    let yaml =
        yaml.replacen("end_month: 12", "end_month: 1", 1)
            .replacen("end_day: 31", "end_day: 21", 1);

    let dir = std::env::temp_dir().join(format!("openbse_comfort_test_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let model = dir.join("office.yaml");
    std::fs::write(&model, yaml).unwrap();

    let status = Command::new(env!("CARGO_BIN_EXE_openbse"))
        .arg(&model)
        .arg("-w")
        .arg(root.join("140_tests/weather/725650TYCST.epw"))
        .current_dir(&dir)
        .output()
        .expect("run openbse");
    assert!(
        status.status.success(),
        "openbse failed:\n{}",
        String::from_utf8_lossy(&status.stderr)
    );

    let text = std::fs::read_to_string(dir.join("office_comfort.json")).expect("comfort json");
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    let _ = std::fs::remove_dir_all(&dir);

    assert_eq!(json["format"], "openbse-comfort-grid");
    assert_eq!(json["version"], 1);
    let hours = json["time"]["hours"].as_u64().unwrap() as usize;
    assert_eq!(hours, 21 * 24);
    assert_eq!(json["sun"]["z"].as_array().unwrap().len(), hours);

    let zones = json["zones"].as_array().unwrap();
    assert_eq!(zones.len(), 1);
    let zone = &zones[0];
    let points = zone["points"].as_array().unwrap();
    // 8 × 6 m floor, 0.5 m spacing and wall offset: 15 × 11 points.
    assert_eq!(points.len(), 165);

    let columns = zone["surfaces"].as_array().unwrap();
    let vf = zone["view_factors"].as_array().unwrap();
    assert_eq!(vf.len(), points.len() * columns.len());
    for row in vf.chunks(columns.len()) {
        let sum: f64 = row.iter().map(|v| v.as_f64().unwrap()).sum();
        assert!((sum - 1.0).abs() < 1e-3, "view factor row sums to {sum}");
    }

    let surfaces = json["surfaces"].as_array().unwrap();
    for c in columns {
        let s = &surfaces[c.as_u64().unwrap() as usize];
        assert_eq!(s["inside_temperature"].as_array().unwrap().len(), hours);
        if s["type"] == "window" {
            assert_eq!(s["beam_irradiance"].as_array().unwrap().len(), hours);
        }
    }
    assert_eq!(zone["air_temperature"].as_array().unwrap().len(), hours);

    let agg = &zone["aggregates"];
    let occupied = agg["occupied_hours"].as_u64().unwrap();
    assert!(occupied > 0);
    let get = |key: &str, i: usize| agg[key][i].as_u64().unwrap();
    for i in 0..points.len() {
        assert_eq!(
            get("comfortable_hours", i) + get("cold_hours", i) + get("hot_hours", i),
            occupied
        );
        assert!(get("sun_hours", i) <= occupied);
    }
    // The point just inside the south window sees sun far more often than the
    // one in the north-east corner.
    let index_of = |x: f64, y: f64| {
        points
            .iter()
            .position(|p| {
                (p[0].as_f64().unwrap() - x).abs() < 1e-6
                    && (p[1].as_f64().unwrap() - y).abs() < 1e-6
            })
            .unwrap()
    };
    assert!(get("sun_hours", index_of(4.0, 0.5)) > get("sun_hours", index_of(7.5, 5.5)));
}

/// Run the example for two days with `comfort_grid` given `zones`; returns
/// whether it succeeded and the `.err` text.
fn run_with_zones(tag: &str, zones: &str) -> (bool, String) {
    let root = workspace_root();
    let yaml = std::fs::read_to_string(root.join("examples/comfort_perimeter_office.yaml"))
        .unwrap()
        .replacen("end_month: 12", "end_month: 1", 1)
        .replacen("end_day: 31", "end_day: 2", 1)
        .replacen(
            "  pmv_limit: 0.5\n",
            &format!("  pmv_limit: 0.5\n  zones: {zones}\n"),
            1,
        );
    let dir = std::env::temp_dir().join(format!("openbse_comfort_{tag}_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let model = dir.join("office.yaml");
    std::fs::write(&model, yaml).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_openbse"))
        .arg(&model)
        .arg("-w")
        .arg(root.join("140_tests/weather/725650TYCST.epw"))
        .output()
        .unwrap();
    let err = std::fs::read_to_string(dir.join("office.err")).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    (out.status.success(), err)
}

/// A named zone that can't be gridded is a warning in the `.err` file.
#[test]
fn skipped_zone_is_reported() {
    let (ok, err) = run_with_zones("partial", "[Perimeter Office, Nowhere]");
    assert!(ok, "{err}");
    assert!(
        err.contains("** Warning ** Comfort grid: zone 'Nowhere' not gridded: zone not found"),
        "{err}"
    );
}

/// A grid that covers no zone stops the run with a severe error.
#[test]
fn grid_with_no_zone_is_severe() {
    let (ok, err) = run_with_zones("none", "[Nowhere]");
    assert!(!ok);
    assert!(
        err.contains("Comfort grid: no zone can be gridded: 'Nowhere'"),
        "{err}"
    );
    assert!(err.contains("Errors: 1"), "{err}");
}
