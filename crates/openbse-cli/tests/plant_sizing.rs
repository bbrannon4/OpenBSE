//! End-to-end checks that coil loads reach the plant and that autosized plant
//! equipment is sized, not run at the −99,999 W AUTOSIZE sentinel.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn workspace_root() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

fn run_model(name: &str, yaml: &str) -> (PathBuf, Output) {
    let dir = std::env::temp_dir().join(format!("openbse_plant_{name}_{}", std::process::id()));
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

/// Hourly results as column name (units stripped) → values.
fn read_results(path: &Path) -> HashMap<String, Vec<f64>> {
    let text = std::fs::read_to_string(path).expect("results csv");
    let mut lines = text.lines();
    let headers: Vec<&str> = lines
        .next()
        .expect("header row")
        .split(',')
        .map(|h| h.split(" [").next().unwrap())
        .collect();
    let mut cols: HashMap<String, Vec<f64>> = HashMap::new();
    for line in lines {
        for (h, v) in headers.iter().zip(line.split(',')) {
            if let Ok(x) = v.parse::<f64>() {
                cols.entry(h.to_string()).or_default().push(x);
            }
        }
    }
    cols
}

#[test]
fn coil_loads_reach_autosized_boiler_and_chiller() {
    let yaml = std::fs::read_to_string(workspace_root().join("examples/vav_chw_plant.yaml"))
        .expect("example model");
    // Two April weeks in Denver: the reheat boxes heat and the AHU cools.
    let yaml = yaml
        .replacen("start_month: 1", "start_month: 4", 1)
        .replacen("end_month: 12", "end_month: 4", 1)
        .replacen("end_day: 31", "end_day: 14", 1);
    let (dir, output) = run_model("vav_chw_plant", &yaml);
    let log = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(output.status.success(), "openbse failed:\n{log}");
    let cols = read_results(&dir.join("vav_chw_plant_results.csv"));
    let _ = std::fs::remove_dir_all(&dir);

    assert!(log.contains("Autosized 'HHW Boiler' capacity"), "{log}");
    assert!(
        log.contains("Autosized 'Air-Cooled Chiller' capacity"),
        "{log}"
    );

    let col = |name: &str| -> &Vec<f64> {
        cols.get(name)
            .unwrap_or_else(|| panic!("no column '{name}'"))
    };
    let sum = |name: &str| col(name).iter().sum::<f64>();

    // Every hot-water coil's heat is delivered by the boiler.
    let hot_water_load = sum("AHU Preheat Coil:thermal_output")
        + sum("VAV Box South:thermal_output")
        + sum("VAV Box North:thermal_output")
        + sum("VAV Box Core:thermal_output");
    let boiler = sum("HHW Boiler:thermal_output");
    assert!(hot_water_load > 0.0);
    assert!(
        (boiler - hot_water_load).abs() <= 0.01 * hot_water_load,
        "boiler {boiler} vs coils {hot_water_load}"
    );
    assert!(sum("HHW Boiler:fuel_power") > boiler);

    // Every chilled-water coil's heat is removed by the chiller.
    let chilled_water_load = -sum("AHU CHW Cooling Coil:thermal_output");
    let chiller = sum("Air-Cooled Chiller:thermal_output");
    assert!(chilled_water_load > 0.0);
    assert!(
        (chiller - chilled_water_load).abs() <= 0.01 * chilled_water_load,
        "chiller {chiller} vs coil {chilled_water_load}"
    );
    assert!(sum("Air-Cooled Chiller:electric_power") > 0.0);

    // A sentinel capacity shows up as a negative part-load ratio.
    for name in ["HHW Boiler:plr", "Air-Cooled Chiller:plr"] {
        for &plr in col(name) {
            assert!((0.0..=1.0).contains(&plr), "{name} = {plr}");
        }
    }
}

#[test]
fn unresolvable_plant_autosize_is_a_severe_error() {
    // Without design days the coils stay unsized, so neither the boiler nor
    // the chiller has anything to be sized from.
    let yaml = std::fs::read_to_string(workspace_root().join("examples/vav_chw_plant.yaml"))
        .expect("example model");
    let mut in_design_days = false;
    let yaml: String = yaml
        .lines()
        .filter(|line| {
            if line.starts_with("design_days:") {
                in_design_days = true;
            } else if line.starts_with(|c: char| c.is_ascii_alphabetic()) {
                in_design_days = false;
            }
            !in_design_days
        })
        .map(|line| format!("{line}\n"))
        .collect();
    let (dir, output) = run_model("no_design_days", &yaml);
    let err = std::fs::read_to_string(dir.join("no_design_days.err")).expect("err file");
    let _ = std::fs::remove_dir_all(&dir);

    assert!(!output.status.success(), "the run must not proceed");
    assert!(
        err.contains(
            "** Severe  ** heating coil 'AHU Preheat Coil' on plant loop 'HHW Loop' has no \
             sized capacity"
        ),
        "{err}"
    );
    assert!(
        err.contains("Cannot autosize capacity of 'Air-Cooled Chiller'"),
        "{err}"
    );
    assert!(err.contains("Simulation cancelled"), "{err}");
}

#[test]
fn chiller_heat_recovery_offsets_the_boiler() {
    // The example's chiller made water-cooled with condenser heat recovery
    // into the hot-water loop (E+ Chiller:Electric:EIR heat recovery). The
    // loop runs at 48.89 C, as in the DOE hospital: at the example's 82 C
    // its water returns at 71 C, above the recovery setpoint, and nothing
    // could be recovered.
    let yaml = std::fs::read_to_string(workspace_root().join("examples/vav_chw_plant.yaml"))
        .expect("example model");
    let yaml = yaml
        .replacen("start_month: 1", "start_month: 4", 1)
        .replacen("end_month: 12", "end_month: 4", 1)
        .replacen("end_day: 31", "end_day: 14", 1)
        .replacen(
            "  - name: HHW Loop\n    design_supply_temp: 82.0",
            "  - name: HHW Loop\n    design_supply_temp: 48.89",
            1,
        )
        .replacen(
            "        name: Air-Cooled Chiller\n",
            "        name: Air-Cooled Chiller\n        condenser_type: water_cooled\n        \
             heat_recovery: {plant_loop: HHW Loop, leaving_setpoint: 48.89}\n",
            1,
        );
    let (dir, output) = run_model("vav_chw_heat_recovery", &yaml);
    let log = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(output.status.success(), "openbse failed:\n{log}");
    let cols = read_results(&dir.join("vav_chw_heat_recovery_results.csv"));
    let _ = std::fs::remove_dir_all(&dir);

    let col = |name: &str| -> &Vec<f64> {
        cols.get(name)
            .unwrap_or_else(|| panic!("no column '{name}'"))
    };
    let hot_water_load: Vec<f64> = (0..col("HHW Boiler:thermal_output").len())
        .map(|i| {
            [
                "AHU Preheat Coil",
                "VAV Box South",
                "VAV Box North",
                "VAV Box Core",
            ]
            .iter()
            .map(|c| col(&format!("{c}:thermal_output"))[i])
            .sum()
        })
        .collect();
    let recovered = col("Air-Cooled Chiller:heat_recovery_rate");
    let boiler = col("HHW Boiler:thermal_output");

    let total_recovered: f64 = recovered.iter().sum();
    let total_load: f64 = hot_water_load.iter().sum();
    assert!(total_recovered > 0.0, "no heat recovered");
    for i in 0..boiler.len() {
        // Recovery never exceeds the loop's demand, and together with the
        // boiler it meets that demand.
        assert!(recovered[i] <= hot_water_load[i].max(0.0) + 1.0, "hour {i}");
        assert!(
            (boiler[i] + recovered[i] - hot_water_load[i].max(0.0)).abs()
                <= 0.01 * hot_water_load[i].abs() + 1.0,
            "hour {i}: boiler {} + recovered {} vs load {}",
            boiler[i],
            recovered[i],
            hot_water_load[i]
        );
    }
    assert!(total_recovered < total_load);
}
