//! A DHW circulation pump runs in both simulation modes, its shaft heat
//! reaches the tank, and its energy books to its own submeter.

use std::path::{Path, PathBuf};
use std::process::Command;

fn workspace_root() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

const PUMP: &str = "    pump:
      name: DHW Pump
      submeter: Plumbing
      design_flow_rate: autosize
      design_head: 5000000
      motor_efficiency: 0.9
";

/// Run `model` for January in its own directory; returns the summary text.
fn run(name: &str, model: &str) -> String {
    let root = workspace_root();
    let dir = std::env::temp_dir().join(format!("openbse_dhw_{name}_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{name}.yaml"));
    std::fs::write(&path, model).unwrap();
    let weather = root.join("weather/USA_CO_Boulder.Muni.AP.720533_TMYx.2009-2023.epw");
    let out = Command::new(env!("CARGO_BIN_EXE_openbse"))
        .arg(&path)
        .arg("-w")
        .arg(weather)
        .arg("-o")
        .arg(dir.join("out.csv"))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let summary = std::fs::read_to_string(summary_path(&dir, name)).unwrap();
    std::fs::remove_dir_all(&dir).ok();
    summary
}

fn summary_path(dir: &Path, name: &str) -> PathBuf {
    dir.join(format!("{name}_summary.txt"))
}

/// The annual total [kWh] on the summary's end-use row `label`.
fn end_use(summary: &str, label: &str) -> f64 {
    let line = summary
        .lines()
        .find(|l| l.trim_start().starts_with(label))
        .unwrap_or_else(|| panic!("no '{label}' row"));
    line.split_whitespace().last().unwrap().parse().unwrap()
}

/// HVAC-only mode (no envelope): the pump runs with the draw and its shaft
/// work heats the makeup water, so the heater makes that much less.
#[test]
fn hvac_only_mode_runs_the_dhw_pump_and_its_heat_reaches_the_tank() {
    let model = |pump: &str| {
        format!(
            "simulation: {{timesteps_per_hour: 1, start_month: 1, start_day: 1, end_month: 1, end_day: 31}}
weather_files: []
dhw_systems:
  - name: DHW
    mains_temperature: 10.0
    water_heater:
      name: Heater
      fuel_type: electric
      tank_volume: 300
      capacity: 20000
      efficiency: 0.98
      setpoint: 60.0
      ua_standby: 1.5
{pump}    loads:
      - name: Draw
        peak_flow_rate: 0.05
        use_temperature: 43.3
"
        )
    };
    let with = run("with_pump", &model(PUMP));
    let without = run("no_pump", &model(""));
    let pump = end_use(&with, "Pumps (Electric)");
    assert!(pump > 100.0, "pump used {pump} kWh");
    // Shaft work = 0.9 of the pump's electricity, made by a 0.98 heater.
    let saved = end_use(&without, "DHW (Electric)") - end_use(&with, "DHW (Electric)");
    let expected = pump * 0.9 / 0.98;
    assert!(
        (saved - expected).abs() < 0.05 * expected,
        "heater saved {saved} kWh, pump shaft heat {expected} kWh"
    );
}

/// With an envelope, the pump's energy books to its submeter, not General.
#[test]
fn the_dhw_pump_books_to_its_submeter() {
    let model = std::fs::read_to_string(workspace_root().join("examples/heat_pump_rtu.yaml"))
        .unwrap()
        .replace("end_month: 12", "end_month: 1")
        .replace(
            "      ua_standby: 1.5\n    loads:",
            &format!("      ua_standby: 1.5\n{PUMP}    loads:"),
        );
    assert!(model.contains("name: DHW Pump"), "pump not inserted");
    let summary = run("rtu", &model);
    let breakdown = summary
        .split("Submeter Breakdown")
        .nth(1)
        .expect("a submeter breakdown");
    let plumbing = breakdown
        .lines()
        .find(|l| l.contains("Plumbing") && l.contains("pump"))
        .unwrap_or_else(|| panic!("no Plumbing pump row in:\n{breakdown}"));
    let kwh: f64 = plumbing.split_whitespace().last().unwrap().parse().unwrap();
    assert!(
        (kwh - end_use(&summary, "Pumps (Electric)")).abs() < 0.2,
        "{plumbing}"
    );
}

/// The summary's Submeter Breakdown row for `meter` and `end_use` [kWh].
fn submeter(summary: &str, meter: &str, end_use: &str) -> f64 {
    let breakdown = summary
        .split("Submeter Breakdown")
        .nth(1)
        .expect("a submeter breakdown");
    let row = breakdown
        .lines()
        .find(|l| l.contains(meter) && l.contains(end_use))
        .unwrap_or_else(|| panic!("no {meter} {end_use} row in:\n{breakdown}"));
    row.split_whitespace().last().unwrap().parse().unwrap()
}

/// HVAC-only mode routes energy to submeters as the full simulation does,
/// and exterior lights on an astronomical clock are off while the sun is up.
#[test]
fn hvac_only_mode_routes_submeters_and_honours_the_astronomical_clock() {
    let model = |clock: bool| {
        format!(
            "simulation: {{timesteps_per_hour: 1, start_month: 1, start_day: 1, end_month: 1, end_day: 31}}
weather_files: []
exterior_equipment:
  - name: Lot Lights
    submeter: Site
    power: 1000.0
    subcategory: Exterior Lights
    astronomical_clock: {clock}
dhw_systems:
  - name: DHW
    submeter: Plumbing
    mains_temperature: 10.0
    water_heater:
      name: Heater
      fuel_type: electric
      tank_volume: 300
      capacity: 20000
      efficiency: 0.98
      setpoint: 60.0
      ua_standby: 1.5
{PUMP}    loads:
      - name: Draw
        peak_flow_rate: 0.05
        use_temperature: 43.3
"
        )
    };
    let always = run("ext_always", &model(false));
    let clock = run("ext_clock", &model(true));
    // 31 days × 24 h × 1 kW, and only the night hours under the clock.
    assert!((end_use(&always, "Exterior Lighting") - 744.0).abs() < 0.5);
    let night = end_use(&clock, "Exterior Lighting");
    assert!(night > 300.0 && night < 600.0, "{night} kWh at night");
    // Each end use reaches its submeter in full.
    for summary in [&always, &clock] {
        let ext = end_use(summary, "Exterior Lighting");
        assert!((submeter(summary, "Site", "ext_lighting") - ext).abs() < 0.2);
        let dhw = end_use(summary, "DHW (Electric)");
        assert!((submeter(summary, "Plumbing", "dhw_electric") - dhw).abs() < 0.2);
        let pump = end_use(summary, "Pumps (Electric)");
        assert!((submeter(summary, "Plumbing", "pump") - pump).abs() < 0.2);
    }
}
