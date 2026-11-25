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

/// Lighting heat that goes to the return is no zone load, on a loop whose return
/// carries it, but the central coil still meets it (#185).
#[test]
fn return_air_lighting_heat_sizes_the_coil_not_the_zone() {
    let model = |fraction: f64| {
        example("vav_reheat.yaml").replace(
            "radiant_fraction: 0.7\n    return_air_fraction: 0.0",
            &format!("radiant_fraction: 0.3\n    return_air_fraction: {fraction}"),
        )
    };
    let (log_none, none) = size_model("return_none", &model(0.0));
    let (log_half, half) = size_model("return_half", &model(0.5));
    let cooling = |zones: &HashMap<String, HashMap<String, f64>>| -> f64 {
        zones.values().map(|z| z["Design Cooling Load [W]"]).sum()
    };
    assert!(
        cooling(&half) < 0.99 * cooling(&none),
        "zone load {} vs {}",
        cooling(&half),
        cooling(&none)
    );
    let coil_load = |log: &str| -> f64 {
        log.lines()
            .find_map(|l| {
                let (_, rest) = l.split_once("design load ")?;
                rest.split(' ').next()?.parse().ok()
            })
            .expect("cooling coil sizing line")
    };
    // Airflow fell with the zone load, so the coil sees a smaller flow at a hotter
    // return; the heat itself stays on the coil, unlike a heat that simply vanished.
    let vanished = coil_load(&log_none) * cooling(&half) / cooling(&none);
    assert!(
        coil_load(&log_half) > vanished,
        "coil {} vs {vanished}",
        coil_load(&log_half)
    );
}

/// A zone's minimum design cooling flow (E+ Sizing:Zone DesignDayWithLimit)
/// raises its cooling and design flows to the limit at standard density, and
/// leaves the other zones alone.
#[test]
fn minimum_cooling_flow_limits_the_zone_design_flow() {
    let base = example("retail_rtu.yaml");
    let limited = base.replacen(
        "    floor_area: 60.0     # 12 × 5\n",
        "    floor_area: 60.0     # 12 × 5\n    minimum_cooling_flow_per_area: 0.05\n",
        1,
    );
    assert_ne!(base, limited, "the test model changed");
    let (_, a) = size_model("min_flow_base", &base);
    let (_, b) = size_model("min_flow_limited", &limited);
    let flow = "Cooling Airflow [kg/s]";
    // 0.05 m³/s·m² x 60 m² at the design days' standard density
    let rho = 83411.0 / (287.042 * 293.15);
    let expected = 0.05 * 60.0 * rho;
    assert!(a["Sales Area"][flow] < expected, "the limit must bind");
    assert!(
        (b["Sales Area"][flow] - expected).abs() < 1e-3,
        "cooling flow {} vs limit {expected}",
        b["Sales Area"][flow]
    );
    assert!(b["Sales Area"]["Design Airflow [kg/s]"] >= expected - 1e-3);
    for (zone, row) in &a {
        if zone != "Sales Area" {
            assert_eq!(row[flow], b[zone][flow], "{zone} changed");
        }
    }
}

/// Terminal reheat sized on a loop's central heating design supply
/// temperature (E+ GetHeatingSATempForSizing with a central heating coil):
/// each box's capacity grows by c_p × its heating primary flow × the drop from
/// the cooling supply temperature it was sized on before.
#[test]
fn central_heating_supply_temp_sizes_terminal_reheat() {
    let base = example("vav_chw_plant.yaml");
    let cold = base.replacen(
        "      cooling_supply_temp: 13.0\n",
        "      cooling_supply_temp: 13.0\n      central_heating_supply_temp: -15.0\n",
        1,
    );
    assert_ne!(base, cold, "the test model changed");
    let reheat = |log: &str| -> HashMap<String, f64> {
        log.lines()
            .filter_map(|l| {
                let rest = l.split("Autosized terminal '").nth(1)?;
                let (name, tail) = rest.split_once("' reheat capacity: ")?;
                Some((
                    name.to_string(),
                    tail.split_whitespace().next()?.parse().ok()?,
                ))
            })
            .collect()
    };
    let (log_a, _) = size_model("reheat_base", &base);
    let (log_b, _) = size_model("reheat_cold", &cold);
    let (a, b) = (reheat(&log_a), reheat(&log_b));
    assert!(!a.is_empty(), "no autosized reheat in the log");
    for (name, qa) in &a {
        let flow = (b[name] - qa) / (1006.0 * 28.0);
        assert!(
            (0.01..10.0).contains(&flow),
            "{name}: {qa} W -> {} W (primary flow {flow} kg/s)",
            b[name]
        );
    }
}
