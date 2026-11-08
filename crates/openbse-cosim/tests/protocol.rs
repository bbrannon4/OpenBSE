//! Wire-protocol tests for the co-simulation proxies, against small Python
//! children (CI has python3).

use openbse_core::ports::{
    AirComponent, AirPort, PlantComponent, SimulationContext, SizingInternalGains, WaterPort,
};
use openbse_core::types::{DayType, TimeStep};
use openbse_cosim::{ExternalAirComponent, ExternalPlantComponent};
use openbse_psychrometrics::{FluidState, MoistAirState};
use std::path::PathBuf;

/// A child that logs each request to `requests.log` in its directory and
/// answers by mode: `ok` cools the water 5 K and reports it, `missing` leaves
/// out `power_w`, `sleep` never answers in time, `badheat` reports twice the
/// heat its outlet carries, `air` heats air 10 K.
const CHILD: &str = r#"
import json, sys, time
mode = sys.argv[1]
log = open("requests.log", "a")
for line in sys.stdin:
    req = json.loads(line)
    if req.get("command") == "stop":
        break
    log.write(json.dumps(req) + "\n"); log.flush()
    i = req["inputs"]
    if mode == "sleep":
        time.sleep(30)
    if mode == "air":
        out = {"outlet_temp_c": i["inlet_temp_c"] + 10.0,
               "outlet_humidity_ratio": i["inlet_humidity_ratio"], "power_w": 50.0}
    else:
        m, t = i["return_flow_kg_s"], i["return_temp_c"]
        heat = -m * 4186.0 * 5.0
        out = {"supply_temp_c": t - 5.0, "power_w": 1000.0, "thermal_output_w": heat}
        if mode == "missing":
            del out["power_w"]
        if mode == "badheat":
            out["thermal_output_w"] = 2.0 * heat
    print(json.dumps({"outputs": out}), flush=True)
"#;

fn workdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("openbse_cosim_{}_{}", tag, std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("child.py"), CHILD).unwrap();
    dir
}

fn ctx(sim_time_s: f64) -> SimulationContext {
    SimulationContext {
        timestep: TimeStep {
            month: 1,
            day: 1,
            hour: 1,
            sub_hour: 1,
            timesteps_per_hour: 1,
            sim_time_s,
            dt: 3600.0,
        },
        outdoor_air: MoistAirState::from_tdb_rh(30.0, 0.4, 101325.0),
        day_type: DayType::WeatherDay,
        is_sizing: false,
        sizing_internal_gains: SizingInternalGains::Full,
    }
}

fn plant(dir: &PathBuf, mode: &str, outputs: &[&str]) -> ExternalPlantComponent {
    ExternalPlantComponent::new(
        "Py Chiller",
        vec!["python3".into(), "child.py".into(), mode.into()],
        vec![
            "return_temp_c".into(),
            "return_flow_kg_s".into(),
            "load_request_w".into(),
        ],
        outputs.iter().map(|s| s.to_string()).collect(),
    )
    .with_working_dir(Some(dir))
}

fn requests(dir: &PathBuf) -> Vec<serde_json::Value> {
    std::fs::read_to_string(dir.join("requests.log"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[test]
fn iterations_counted_within_a_timestep_and_warmup_flagged() {
    let dir = workdir("iter");
    let inlet = WaterPort::new(FluidState::water(12.0, 2.0));
    {
        let mut c = plant(&dir, "ok", &["supply_temp_c", "power_w"]);
        c.set_warmup(true);
        c.simulate_plant(&inlet, -40_000.0, &ctx(0.0));
        c.simulate_plant(&inlet, -40_000.0, &ctx(0.0));
        c.set_warmup(false);
        // Same time as the warmup step: a new step, not a repeat.
        c.simulate_plant(&inlet, -40_000.0, &ctx(0.0));
        c.simulate_plant(&inlet, -40_000.0, &ctx(0.0));
        c.simulate_plant(&inlet, -40_000.0, &ctx(0.0));
        c.simulate_plant(&inlet, -40_000.0, &ctx(3600.0));
        assert!(c.fatal_error().is_none(), "{:?}", c.fatal_error());
    }
    let reqs = requests(&dir);
    let iters: Vec<u64> = reqs
        .iter()
        .map(|r| r["iteration"].as_u64().unwrap())
        .collect();
    let warm: Vec<bool> = reqs
        .iter()
        .map(|r| r["warmup"].as_bool().unwrap())
        .collect();
    assert_eq!(iters, vec![0, 1, 0, 1, 2, 0]);
    assert_eq!(warm, vec![true, true, false, false, false, false]);
    assert_eq!(
        reqs[0]["inputs"]["load_request_w"].as_f64(),
        Some(-40_000.0)
    );
}

#[test]
fn plant_heat_comes_from_the_outlet_state() {
    let dir = workdir("heat");
    let inlet = WaterPort::new(FluidState::water(12.0, 2.0));
    let mut c = plant(
        &dir,
        "ok",
        &["supply_temp_c", "power_w", "thermal_output_w"],
    );
    let out = c.simulate_plant(&inlet, -40_000.0, &ctx(0.0));
    assert!(c.fatal_error().is_none(), "{:?}", c.fatal_error());
    // Energy closes: the heat booked equals what the water carries.
    let carried = inlet.state.mass_flow * inlet.state.cp * (out.state.temp - inlet.state.temp);
    assert!((c.thermal_output() - carried).abs() < 1e-6);
    assert!((out.state.temp - 7.0).abs() < 1e-9);
    assert_eq!(out.state.mass_flow, 2.0);
    assert_eq!(c.power_consumption(), 1000.0);
}

#[test]
fn reported_heat_that_disagrees_with_the_outlet_stops_the_run() {
    let dir = workdir("badheat");
    let inlet = WaterPort::new(FluidState::water(12.0, 2.0));
    let mut c = plant(&dir, "badheat", &["supply_temp_c", "thermal_output_w"]);
    c.simulate_plant(&inlet, -40_000.0, &ctx(0.0));
    let err = c.fatal_error().expect("disagreeing heat must fail");
    assert!(err.contains("thermal_output_w"), "{err}");
}

#[test]
fn missing_declared_output_stops_the_run() {
    let dir = workdir("missing");
    let inlet = WaterPort::new(FluidState::water(12.0, 2.0));
    let mut c = plant(&dir, "missing", &["supply_temp_c", "power_w"]);
    let out = c.simulate_plant(&inlet, -40_000.0, &ctx(0.0));
    let err = c.fatal_error().expect("missing output must fail");
    assert!(err.contains("power_w"), "{err}");
    assert_eq!(out.state.temp, 12.0);
    assert_eq!(c.power_consumption(), 0.0);
}

#[test]
fn a_hung_child_times_out() {
    let dir = workdir("sleep");
    let inlet = WaterPort::new(FluidState::water(12.0, 2.0));
    let start = std::time::Instant::now();
    {
        let mut c = plant(&dir, "sleep", &["supply_temp_c"]).with_timeout(0.3);
        c.simulate_plant(&inlet, -40_000.0, &ctx(0.0));
        let err = c.fatal_error().expect("timeout must fail");
        assert!(err.contains("no response"), "{err}");
    }
    // The hung child is killed on drop rather than waited for.
    assert!(start.elapsed().as_secs_f64() < 10.0);
}

#[test]
fn a_child_that_cannot_start_stops_the_run() {
    let dir = workdir("nostart");
    let mut c = ExternalPlantComponent::new(
        "Nothing",
        vec!["openbse-no-such-program".into()],
        vec![],
        vec!["supply_temp_c".into()],
    )
    .with_working_dir(Some(&dir));
    c.simulate_plant(
        &WaterPort::new(FluidState::water(12.0, 2.0)),
        -1000.0,
        &ctx(0.0),
    );
    assert!(c.fatal_error().unwrap().contains("failed to start"));
}

#[test]
fn air_heat_is_the_enthalpy_rise() {
    let dir = workdir("air");
    let mut c = ExternalAirComponent::new(
        "Py Heater",
        vec!["python3".into(), "child.py".into(), "air".into()],
        vec!["inlet_temp_c".into(), "inlet_humidity_ratio".into()],
        vec![
            "outlet_temp_c".into(),
            "outlet_humidity_ratio".into(),
            "power_w".into(),
        ],
    )
    .with_working_dir(Some(&dir));
    let inlet = AirPort::new(MoistAirState::new(10.0, 0.005, 101325.0), 1.5);
    let out = c.simulate_air(&inlet, &ctx(0.0));
    assert!(c.fatal_error().is_none(), "{:?}", c.fatal_error());
    assert!((out.state.t_db - 20.0).abs() < 1e-9);
    let carried = inlet.mass_flow * (out.state.h - inlet.state.h);
    assert!((c.thermal_output() - carried).abs() < 1e-6);
    assert!(c.thermal_output() > 1.5 * 1000.0 * 10.0);
}
