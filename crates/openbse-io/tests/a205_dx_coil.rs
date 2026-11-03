//! Integration test: parse the residential-unitary example that uses an
//! ASHRAE Standard 205 RS0004 DX cooling coil, build the simulation graph
//! end-to-end, and confirm a chiller-style missing-file error path.

use openbse_io::input::{build_graph_with_base, load_model, parse_model_yaml};
use std::path::PathBuf;

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

#[test]
fn example_residential_unitary_a205_builds() {
    let yaml = workspace_root().join("examples/residential_unitary_a205.yaml");
    assert!(yaml.exists(), "example YAML not found: {}", yaml.display());

    let model = load_model(&yaml).expect("load_model");
    let graph = build_graph_with_base(&model, yaml.parent())
        .expect("build_graph_with_base should succeed for example model");

    assert!(graph.component_count() > 0);
}

#[test]
fn missing_dx_a205_file_yields_clean_error() {
    let yaml = r#"
simulation:
  timesteps_per_hour: 1
  start_month: 1
  start_day: 1
  end_month: 1
  end_day: 1

weather_files: []

zones:
  - name: Z1
    volume: 100.0
    floor_area: 25.0

air_loops:
  - name: AHU
    equipment:
      - type: cooling_coil
        name: TestDX
        source: dx
        setpoint: 13.0
        a205_file: /nonexistent/path/does_not_exist.a205
    zone_terminals:
      - zone: Z1
"#;
    let model = parse_model_yaml(yaml).expect("parse");
    let result = build_graph_with_base(&model, None);
    let err = match result {
        Ok(_) => panic!("expected error when a205_file is missing"),
        Err(e) => e,
    };
    let msg = format!("{}", err);
    assert!(
        msg.contains("a205_file") && msg.contains("TestDX"),
        "error should mention the file and coil name; got: {}",
        msg
    );
}

/// The coil books exactly what its air stream receives, ṁ·Δh = −thermal_output,
/// across dry and humid inlets, light and heavy loads, including where the
/// outlet limits bind.
#[test]
fn a205_coil_books_what_its_air_receives() {
    use openbse_core::graph::GraphComponent;
    use openbse_core::ports::{AirPort, SimulationContext, SizingInternalGains};
    use openbse_core::types::{DayType, TimeStep};
    use openbse_psychrometrics::{self as psych, MoistAirState};

    let yaml = workspace_root().join("examples/residential_unitary_rs0002.yaml");
    let model = load_model(&yaml).expect("load_model");
    let mut graph = build_graph_with_base(&model, yaml.parent()).expect("graph");
    let idx = graph
        .node_by_name("DX Cooling Coil (RS0004)")
        .expect("coil");
    let GraphComponent::Air(coil) = graph.component_mut(idx) else {
        panic!("air component");
    };
    let mut checked = 0;
    for t_out in [25.0, 35.0, 45.0] {
        let ctx = SimulationContext {
            timestep: TimeStep {
                month: 7,
                day: 15,
                hour: 14,
                sub_hour: 1,
                timesteps_per_hour: 1,
                sim_time_s: 0.0,
                dt: 3600.0,
            },
            outdoor_air: MoistAirState::from_tdb_rh(t_out, 0.4, 101_325.0),
            day_type: DayType::WeatherDay,
            is_sizing: false,
            sizing_internal_gains: SizingInternalGains::Full,
        };
        for (t_in, w_in) in [(24.0, 0.004), (26.0, 0.010), (30.0, 0.016), (22.0, 0.012)] {
            for (setpoint, flow) in [(13.0, 0.5), (18.0, 0.3), (12.0, 1.2), (21.5, 0.4)] {
                coil.set_setpoint(setpoint);
                let inlet = AirPort::new(MoistAirState::new(t_in, w_in, 101_325.0), flow);
                let out = coil.simulate_air(&inlet, &ctx);
                let drop = flow
                    * (psych::h_fn_tdb_w(t_in, w_in)
                        - psych::h_fn_tdb_w(out.state.t_db, out.state.w));
                let booked = -coil.thermal_output();
                assert!(
                    (drop - booked).abs() <= 1e-6 * booked.abs().max(1.0),
                    "inlet {t_in}/{w_in}, sp {setpoint}, flow {flow}: air {drop} W, booked {booked} W"
                );
                let w_sat = psych::w_fn_tdp_pb(out.state.t_db, 101_325.0);
                assert!(
                    out.state.w <= w_sat * 1.001,
                    "{t_in}/{w_in} sp {setpoint}: {} {} sat {w_sat}",
                    out.state.t_db,
                    out.state.w
                );
                assert!(coil.power_consumption() >= 0.0);
                checked += 1;
            }
        }
    }
    assert_eq!(checked, 48);
}
