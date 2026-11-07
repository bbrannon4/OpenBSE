//! A heat exchanger's heat reaches its source loop: a waterside economizer's
//! heat is rejected by the condenser loop's tower with the chiller's, and both
//! loops close.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;

fn workspace_root() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

/// January of chw_plant_condenser with a waterside economizer ahead of the
/// chiller, its condenser loop held at 12 °C. Returns the hourly result rows.
fn run_january() -> Vec<HashMap<String, f64>> {
    let root = workspace_root();
    let chiller = "      - type: chiller\n        name: Water-Cooled Chiller";
    let model = std::fs::read_to_string(root.join("examples/chw_plant_condenser.yaml"))
        .unwrap()
        .replace("end_month: 12", "end_month: 1")
        .replace(
            "    design_supply_temp: 29.4",
            "    design_supply_temp: 12.0",
        )
        .replace(
            chiller,
            &format!(
                "      - type: heat_exchanger
        name: WSE
        source_loop: Condenser Water Loop
        control_mode: economizer
        design_flow_rate: autosize
{chiller}"
            ),
        );
    assert!(model.contains("name: WSE") && model.contains("design_supply_temp: 12.0"));
    let dir = std::env::temp_dir().join(format!("openbse_wse_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("wse.yaml");
    std::fs::write(&path, model).unwrap();
    let csv = dir.join("wse.csv");
    let weather = root.join("weather/USA_CO_Boulder.Muni.AP.720533_TMYx.2009-2023.epw");
    let run = Command::new(env!("CARGO_BIN_EXE_openbse"))
        .arg(&path)
        .arg("-w")
        .arg(weather)
        .arg("-o")
        .arg(&csv)
        .output()
        .unwrap();
    assert!(
        run.status.success(),
        "{}",
        String::from_utf8_lossy(&run.stderr)
    );
    let text = std::fs::read_to_string(&csv).unwrap();
    std::fs::remove_dir_all(&dir).ok();
    let mut lines = text.lines();
    let header: Vec<String> = lines
        .next()
        .unwrap()
        .split(',')
        .map(|h| h.trim_end_matches(" [-]").to_string())
        .collect();
    lines
        .map(|l| {
            header
                .iter()
                .zip(l.split(','))
                .filter_map(|(h, v)| v.parse().ok().map(|v| (h.clone(), v)))
                .collect()
        })
        .collect()
}

#[test]
fn economizer_heat_is_rejected_by_the_source_loop() {
    let rows = run_january();
    let sum = |col: &str| rows.iter().map(|r| r[col]).sum::<f64>();
    let wse = sum("WSE:thermal_output");
    assert!(wse > 1e3, "economizer moved {wse} Wh");

    // Chilled-water loop: the coils' cooling and the pump's heat are met by
    // the economizer and the chiller.
    let demand = -sum("CHW Cooling Coil:thermal_output") + sum("CHW Pump:thermal_output");
    let supply = wse + sum("Water-Cooled Chiller:thermal_output");
    // The dispatch stops once less than 1 W is left.
    let tolerance = rows.len() as f64;
    assert!((demand - supply).abs() < tolerance, "{demand} vs {supply}");

    // Condenser loop, every hour: the tower rejects the chiller's condenser
    // heat, the economizer's heat and the pump's heat.
    for r in &rows {
        let inflow = r["Water-Cooled Chiller:condenser_heat_rate"]
            + r["WSE:thermal_output"]
            + r["CW Pump:thermal_output"];
        assert!(
            (r["CW Tower:heat_rejected"] - inflow).abs() < 0.01,
            "tower {} vs {inflow}",
            r["CW Tower:heat_rejected"]
        );
    }
}
