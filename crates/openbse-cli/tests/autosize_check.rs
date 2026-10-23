//! A model whose `autosize` inputs nothing sizes must stop with a severe
//! error naming each one, instead of running with the sentinel or a silent
//! fallback (examples/datacenter_crah.yaml ran with its data hall at
//! thousands of °C before it had design days).

use std::path::PathBuf;
use std::process::Command;

fn workspace_root() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

const MODEL: &str = r#"
simulation:
  timesteps_per_hour: 1
  start_month: 1
  start_day: 1
  end_month: 1
  end_day: 2
weather_files: []
simple_constructions:
  - {name: Shell, u_factor: 0.5}
zones:
  - {name: Room, volume: 150.0, floor_area: 50.0}
surfaces:
  - {name: South, zone: Room, type: wall, construction: Shell, area: 30, azimuth: 180, tilt: 90}
  - {name: North, zone: Room, type: wall, construction: Shell, area: 30, azimuth: 0, tilt: 90}
  - {name: Roof, zone: Room, type: roof, construction: Shell, area: 50, azimuth: 0, tilt: 0}
  - {name: Floor, zone: Room, type: floor, construction: Shell, area: 50, azimuth: 0, tilt: 180, boundary: adiabatic}
thermostats:
  - {name: Stat, zones: [Room], heating_setpoint: 21.0, cooling_setpoint: 24.0}
air_loops:
  - name: AHU
    controls:
      design_zone_flow: autosize
    equipment:
      - {type: heating_coil, name: Coil, source: electric, capacity: autosize, setpoint: 35.0}
      - {type: fan, name: Fan, source: constant_volume, design_flow_rate: autosize, pressure_rise: 500.0}
    zone_terminals:
      - zone: Room
radiant_panels:
  - {name: Panel, zone: Room, source: electric, rated_capacity: autosize}
"#;

#[test]
fn unsized_autosize_inputs_stop_the_run() {
    let root = workspace_root();
    let dir = std::env::temp_dir().join(format!("openbse_autosize_test_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let model = dir.join("model.yaml");
    std::fs::write(&model, MODEL).unwrap();

    let run = Command::new(env!("CARGO_BIN_EXE_openbse"))
        .arg(&model)
        .arg("-w")
        .arg(root.join("140_tests/weather/725650TYCST.epw"))
        .current_dir(&dir)
        .output()
        .expect("run openbse");
    let err = std::fs::read_to_string(dir.join("model.err")).unwrap_or_default();
    let _ = std::fs::remove_dir_all(&dir);

    assert!(!run.status.success(), "run should fail:\n{err}");
    let no_design_days = "is autosize but was never sized: the model has no design_days, \
                          so air-side sizing did not run";
    for field in [
        "'design_flow_rate' of 'Fan'",
        "'capacity' of 'Coil'",
        "'controls.design_zone_flow' of air loop 'AHU'",
    ] {
        assert!(
            err.contains(&format!("** Severe  ** {field} {no_design_days}")),
            "missing {field}:\n{err}"
        );
    }
    // Design days would not help the panel: nothing sizes its capacity.
    assert!(
        err.contains(
            "** Severe  ** 'rated_capacity' of radiant panel 'Panel' is autosize but was \
             never sized: OpenBSE does not autosize it yet"
        ),
        "{err}"
    );
    assert!(err.contains("Errors: 4,"), "{err}");
}
