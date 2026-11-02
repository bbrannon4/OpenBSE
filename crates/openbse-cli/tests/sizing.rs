//! End-to-end checks of what autosizing hands to the simulation: the design
//! flows and capacities of a real model, read from its log and sizing report.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;

fn workspace_root() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

/// Run `yaml` for two January days; return the log and the zone sizing report
/// as zone → column → value.
fn size_model(name: &str, yaml: &str) -> (String, HashMap<String, HashMap<String, f64>>) {
    let dir = std::env::temp_dir().join(format!("openbse_sizing_{name}_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let model = dir.join(format!("{name}.yaml"));
    let yaml =
        yaml.replacen("end_month: 12", "end_month: 1", 1)
            .replacen("end_day: 31", "end_day: 2", 1);
    std::fs::write(&model, yaml).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_openbse"))
        .arg(&model)
        .arg("-w")
        .arg(workspace_root().join("140_tests/weather/725650TYCST.epw"))
        .current_dir(&dir)
        .output()
        .expect("run openbse");
    let log = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(output.status.success(), "openbse failed:\n{log}");

    let report = std::fs::read_to_string(dir.join(format!("{name}_zone_sizing.csv"))).unwrap();
    let mut lines = report.lines();
    let headers: Vec<&str> = lines.next().unwrap().split(',').collect();
    let zones = lines
        .map(|line| {
            let cells: Vec<&str> = line.split(',').collect();
            let values = headers
                .iter()
                .zip(&cells)
                .filter_map(|(h, c)| Some((h.to_string(), c.parse().ok()?)))
                .collect();
            (cells[0].to_string(), values)
        })
        .collect();
    let _ = std::fs::remove_dir_all(&dir);
    (log, zones)
}

fn example(name: &str) -> String {
    std::fs::read_to_string(workspace_root().join("examples").join(name)).unwrap()
}

/// A sizing factor scales the zone's design load and flow once, and what is
/// sized from them inherits it, with no second application downstream (#49).
#[test]
fn sizing_factors_scale_zone_loads_and_flows_once() {
    let with_factor = |f: f64| {
        example("retail_rtu.yaml").replacen(
            "simulation:\n",
            &format!("simulation:\n  heating_sizing_factor: {f}\n  cooling_sizing_factor: {f}\n"),
            1,
        )
    };
    let (_, one) = size_model("factor_one", &with_factor(1.0));
    let (_, two) = size_model("factor_two", &with_factor(2.0));
    // Only the conditioned zones have design loads.
    let sized: Vec<_> = one
        .iter()
        .filter(|(_, a)| a["Design Heating Load [W]"] > 0.0)
        .collect();
    assert!(!sized.is_empty());
    for (zone, a) in sized {
        let b = &two[zone];
        for col in ["Design Heating Load [W]", "Design Cooling Load [W]"] {
            let ratio = b[col] / a[col];
            assert!((ratio - 2.0).abs() < 1e-3, "{zone} {col} scaled by {ratio}");
        }
        // A flow above its outdoor-air floor scales with the load.
        let flow = "Cooling Airflow [kg/s]";
        assert!((b[flow] / a[flow] - 2.0).abs() < 1e-2, "{zone} {flow}");
    }
}
