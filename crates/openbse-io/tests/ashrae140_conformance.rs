//! Every ASHRAE 140 case states the Standard's specified inputs.
//!
//! `140_tests/conformance_spec.json` lists, per case, values that ASHRAE
//! 140-2023 specifies (weather file, constructions, gains, set points,
//! capacities, ...), each with its clause. This test parses each case with the
//! engine's own input parser, serializes the result and checks every listed
//! value, so an input that drifts from the Standard, or a case added without a
//! specification, fails here. See `140_tests/CONFORMANCE.md` for the audit the
//! table comes from.

use serde_json::Value;
use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

/// Follow a `/`-separated path. A segment `[name=X]` selects the element of a
/// list whose `name` is X; a number indexes a list; anything else is a key.
fn select<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    let mut cur = value;
    for seg in path.split('/').skip(1) {
        cur = if let Some(name) = seg.strip_prefix("[name=").and_then(|s| s.strip_suffix(']')) {
            cur.as_array()?
                .iter()
                .find(|e| e.get("name").and_then(Value::as_str) == Some(name))?
        } else if let Ok(i) = seg.parse::<usize>() {
            cur.get(i)?
        } else {
            cur.get(seg)?
        };
    }
    Some(cur)
}

/// Whether `actual` carries the `expected` value. Numbers compare to a
/// relative 1e-9; an expected object only constrains the keys it lists.
fn matches(expected: &Value, actual: &Value) -> bool {
    match (expected, actual) {
        (Value::Number(e), Value::Number(a)) => {
            let (e, a) = (e.as_f64().unwrap(), a.as_f64().unwrap());
            (e - a).abs() <= 1e-9 * e.abs().max(a.abs()).max(1.0)
        }
        (Value::Array(e), Value::Array(a)) => {
            e.len() == a.len() && e.iter().zip(a).all(|(e, a)| matches(e, a))
        }
        (Value::Object(e), Value::Object(a)) => e
            .iter()
            .all(|(k, v)| a.get(k).is_some_and(|actual| matches(v, actual))),
        _ => expected == actual,
    }
}

#[test]
fn ashrae140_cases_state_the_specified_inputs() {
    let root = root();
    let spec: Value = serde_json::from_str(
        &std::fs::read_to_string(root.join("140_tests/conformance_spec.json")).unwrap(),
    )
    .unwrap();
    let cases = spec["cases"].as_object().unwrap();

    let mut failures = Vec::new();
    let mut checked = 0;
    for (case, checks) in cases {
        let path = root.join(format!("140_tests/cases/ashrae140_case{case}.yaml"));
        let model = match openbse_io::input::load_model(&path) {
            Ok(m) => serde_json::to_value(&m).unwrap(),
            Err(e) => {
                failures.push(format!("{case}: does not parse: {e}"));
                continue;
            }
        };
        for check in checks.as_array().unwrap() {
            let p = check["path"].as_str().unwrap();
            let expected = &check["value"];
            let clause = check["clause"].as_str().unwrap_or("");
            match select(&model, p) {
                Some(actual) if matches(expected, actual) => {}
                Some(actual) => failures.push(format!(
                    "{case} {p}: {actual} where the Standard specifies {expected} ({clause})"
                )),
                None => failures.push(format!("{case} {p}: missing ({clause})")),
            }
            checked += 1;
        }
    }
    assert!(checked > 2000, "only {checked} checks ran");
    assert!(
        failures.is_empty(),
        "{} of {checked} specified inputs differ:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn every_ashrae140_case_has_a_specification() {
    let root = root();
    let spec: Value = serde_json::from_str(
        &std::fs::read_to_string(root.join("140_tests/conformance_spec.json")).unwrap(),
    )
    .unwrap();
    let cases = spec["cases"].as_object().unwrap();
    let mut missing = Vec::new();
    for entry in std::fs::read_dir(root.join("140_tests/cases")).unwrap() {
        let name = entry.unwrap().file_name().to_string_lossy().into_owned();
        let Some(case) = name
            .strip_prefix("ashrae140_case")
            .and_then(|s| s.strip_suffix(".yaml"))
        else {
            continue;
        };
        // Scratch variants kept for diagnosis are not test cases.
        if case.ends_with("_test") {
            continue;
        }
        if !cases.contains_key(case) {
            missing.push(case.to_string());
        }
    }
    missing.sort();
    assert!(
        missing.is_empty(),
        "cases without an entry in 140_tests/conformance_spec.json: {missing:?}"
    );
}

#[test]
fn selector_and_matching_rules() {
    let v: Value = serde_json::json!({
        "items": [{"name": "a", "x": 1.0}, {"name": "b", "x": 2.0, "y": null}]
    });
    assert_eq!(
        select(&v, "/items/[name=b]/x"),
        Some(&serde_json::json!(2.0))
    );
    assert_eq!(select(&v, "/items/0/name"), Some(&serde_json::json!("a")));
    assert!(select(&v, "/items/[name=c]/x").is_none());
    assert!(matches(
        &serde_json::json!(2),
        &serde_json::json!(2.0000000001)
    ));
    assert!(!matches(&serde_json::json!(2), &serde_json::json!(2.001)));
    assert!(matches(
        &serde_json::json!({"name": "b"}),
        &serde_json::json!({"name": "b", "x": 2.0})
    ));
    assert!(matches(&Value::Null, select(&v, "/items/1/y").unwrap()));
}
