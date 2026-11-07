//! Thermal storage is charged by its loop's chillers: the chilled-water loop
//! closes with the charge as a load, and the store's state closes on charge,
//! discharge and standby loss once per timestep, however many HVAC iterations
//! run.

use std::path::PathBuf;
use std::process::Command;

fn workspace_root() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

/// July of vav_chw_plant with an ice store ahead of the chiller, discharged
/// 12:00–18:00. Returns the hourly result rows.
fn run_july() -> Vec<std::collections::HashMap<String, f64>> {
    let root = workspace_root();
    let model = std::fs::read_to_string(root.join("examples/vav_chw_plant.yaml"))
        .unwrap()
        .replace("start_month: 1", "start_month: 7")
        .replace("end_month: 12", "end_month: 7")
        .replace(
            "    supply_equipment:\n      - type: chiller\n        name: Air-Cooled Chiller",
            "    supply_equipment:
      - type: thermal_storage
        name: TES
        tes_type: ice
        capacity_wh: 200000
        max_charge_rate: 30000
        max_discharge_rate: 40000
        control_strategy: full_storage
        peak_hours: [12, 13, 14, 15, 16, 17, 18]
      - type: chiller
        name: Air-Cooled Chiller",
        );
    assert!(model.contains("name: TES"));
    let dir = std::env::temp_dir().join(format!("openbse_tes_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("tes.yaml");
    std::fs::write(&path, model).unwrap();
    let csv = dir.join("tes.csv");
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
fn storage_charging_is_a_chiller_load_and_its_state_closes() {
    let rows = run_july();
    let sum = |col: &str| rows.iter().map(|r| r[col]).sum::<f64>();
    let coil = -sum("AHU CHW Cooling Coil:thermal_output");
    let chiller = sum("Air-Cooled Chiller:thermal_output");
    let charge = sum("TES:charge_rate_w");
    let discharge = sum("TES:discharge_rate_w");
    assert!(charge > 1e5 && discharge > 1e5, "{charge} {discharge}");

    // The loop closes with the charge as a load: the chiller makes the coils'
    // cooling less what the store discharged, plus what it charged.
    assert!(
        (chiller + discharge - charge - coil).abs() < 1e-6 * coil,
        "chiller {chiller} + discharge {discharge} - charge {charge} != coil {coil}"
    );

    // Each hour the state moves by charge less discharge less the standby
    // loss (5 W/K × 25 K for ice), counted once per timestep.
    for pair in rows.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        let loss = (5.0 * 25.0_f64).min(a["TES:stored_energy_wh"]);
        let expected =
            a["TES:stored_energy_wh"] - loss + b["TES:charge_rate_w"] - b["TES:discharge_rate_w"];
        assert!(
            (b["TES:stored_energy_wh"] - expected).abs() < 1e-3,
            "{} vs {expected}",
            b["TES:stored_energy_wh"]
        );
    }

    // Making ice costs the chillers 1/0.85 of their charging power, booked
    // to the store.
    assert!(sum("TES:electric_power") > 0.0);
}
