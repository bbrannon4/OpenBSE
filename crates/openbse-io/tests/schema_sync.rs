//! Drift test: keeps `docs/openbse_schema.json` (the hand-written JSON Schema
//! that drives the editor's "Edit" tab) in sync with the serde input structs.
//!
//! Checks:
//! 1. Every top-level `ModelInput` key appears in the schema's top-level
//!    `properties`, and vice versa.
//! 2. Every mapping key path in every example / test model YAML that parses as
//!    `ModelInput` is described by the schema — both the raw YAML as written
//!    and the model re-serialized by the engine (which emits every struct
//!    field, including ones no example sets).
//! 3. Every schema property on an object the engine serializes is actually
//!    emitted by the engine (catches stale/removed fields in the schema).
//! 4. No example YAML uses a key that serde silently ignores (unknown keys
//!    are dropped by serde, so these are almost always typos).
//!
//! The schema is parsed with `serde_yaml` (JSON is valid YAML) so no extra
//! dependency is needed. Schema features understood here: `$ref` to
//! `#/$defs/...`, `oneOf`, `properties`, `items`, `additionalProperties`,
//! `enum`, `const`, plus the custom `x-aliases` annotation listing serde
//! aliases accepted for a property (the editor ignores unknown keywords).

use openbse_io::input::ModelInput;
use serde_yaml::{Mapping, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

fn load_schema() -> Value {
    let path = workspace_root().join("docs/openbse_schema.json");
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    serde_yaml::from_str(&text).expect("docs/openbse_schema.json must parse")
}

/// Turn `!tag value` into `{tag: value}` (serde_yaml's form for externally
/// tagged enum variants) and apply `<<` merge keys, recursively.
fn normalize(v: &Value) -> Value {
    match v {
        Value::Tagged(t) => {
            let mut m = Mapping::new();
            let tag = t.tag.to_string();
            m.insert(
                Value::String(tag.trim_start_matches('!').to_string()),
                normalize(&t.value),
            );
            Value::Mapping(m)
        }
        Value::Mapping(m) => Value::Mapping(
            m.iter()
                .map(|(k, v)| (k.clone(), normalize(v)))
                .collect::<Mapping>(),
        ),
        Value::Sequence(s) => Value::Sequence(s.iter().map(normalize).collect()),
        other => other.clone(),
    }
}

fn key_str(k: &Value) -> String {
    match k {
        Value::String(s) => s.clone(),
        other => serde_yaml::to_string(other)
            .unwrap_or_default()
            .trim()
            .to_string(),
    }
}

struct Checker<'a> {
    root: &'a Value,
}

/// Problems found while walking one value against the schema.
#[derive(Default)]
struct Report {
    /// Key paths not described by the schema.
    undescribed: BTreeSet<String>,
    /// Schema properties the engine never emits for that object.
    stale: BTreeSet<String>,
    /// Raw YAML keys that the engine dropped on deserialization.
    ignored: BTreeSet<String>,
    /// Key paths the engine emitted (stale-check bookkeeping only).
    seen: BTreeSet<String>,
}

impl Report {
    fn len(&self) -> usize {
        self.undescribed.len() + self.stale.len() + self.ignored.len()
    }
    fn merge(&mut self, other: Report) {
        self.undescribed.extend(other.undescribed);
        self.stale.extend(other.stale);
        self.ignored.extend(other.ignored);
        self.seen.extend(other.seen);
    }
}

impl<'a> Checker<'a> {
    fn resolve(&self, mut node: &'a Value) -> &'a Value {
        for _ in 0..32 {
            let Some(r) = node.get("$ref").and_then(Value::as_str) else {
                return node;
            };
            let name = r
                .strip_prefix("#/$defs/")
                .unwrap_or_else(|| panic!("unsupported $ref {r}"));
            node = self.root["$defs"]
                .get(name)
                .unwrap_or_else(|| panic!("dangling $ref {r}"));
        }
        panic!("$ref cycle");
    }

    /// Candidate schema nodes for a value, after resolving `$ref` and
    /// flattening `oneOf`.
    fn candidates(&self, node: &'a Value, out: &mut Vec<&'a Value>) {
        let node = self.resolve(node);
        match node.get("oneOf").and_then(Value::as_sequence) {
            Some(variants) => variants.iter().for_each(|v| self.candidates(v, out)),
            None => out.push(node),
        }
    }

    /// Walk `value` against `node`. `serialized` is the engine's
    /// re-serialized counterpart of a raw YAML value (None when checking the
    /// serialized form itself, or when shapes diverge); `check_stale` enables
    /// the reverse (schema → engine) check, only valid on serialized values.
    fn check(
        &self,
        value: &Value,
        node: &'a Value,
        path: &str,
        serialized: Option<&Value>,
        check_stale: bool,
    ) -> Report {
        let mut cands = Vec::new();
        self.candidates(node, &mut cands);
        // Discriminate tagged variants by `type`.
        if let Some(tag) = value.get("type").and_then(Value::as_str) {
            let tagged: Vec<_> = cands
                .iter()
                .copied()
                .filter(|c| {
                    let t = &c["properties"]["type"];
                    t.get("const").and_then(Value::as_str) == Some(tag)
                        || t.get("enum")
                            .and_then(Value::as_sequence)
                            .is_some_and(|e| e.iter().any(|x| x.as_str() == Some(tag)))
                })
                .collect();
            if !tagged.is_empty() {
                cands = tagged;
            }
        }
        let mut best: Option<Report> = None;
        for cand in cands {
            let r = self.check_one(value, cand, path, serialized, check_stale);
            if best.as_ref().is_none_or(|b| r.len() < b.len()) {
                let done = r.len() == 0;
                best = Some(r);
                if done {
                    break;
                }
            }
        }
        best.unwrap_or_default()
    }

    fn check_one(
        &self,
        value: &Value,
        node: &'a Value,
        path: &str,
        serialized: Option<&Value>,
        check_stale: bool,
    ) -> Report {
        let mut rep = Report::default();
        let ty = node.get("type").and_then(Value::as_str);
        match value {
            Value::Mapping(m) => {
                let props = node.get("properties").and_then(Value::as_mapping);
                let additional = node.get("additionalProperties");
                if props.is_none() && additional.is_none() {
                    rep.undescribed
                        .insert(format!("{path} (object not allowed here)"));
                    return rep;
                }
                let ser_map = serialized.and_then(Value::as_mapping);
                if check_stale {
                    rep.seen
                        .extend(m.keys().map(|k| format!("{path}.{}", key_str(k))));
                }
                for (k, v) in m {
                    let key = key_str(k);
                    let sub = format!("{path}.{key}");
                    let found = props.and_then(|p| {
                        p.iter().find_map(|(pk, ps)| {
                            let pk = key_str(pk);
                            let aliased = ps
                                .get("x-aliases")
                                .and_then(Value::as_sequence)
                                .is_some_and(|a| a.iter().any(|x| x.as_str() == Some(&key)));
                            (pk == key || aliased).then_some((pk, ps))
                        })
                    });
                    let canonical = found.as_ref().map_or(key.as_str(), |(pk, _)| pk.as_str());
                    // A raw key the engine did not round-trip was dropped by
                    // serde (unknown field) — report it whether or not the
                    // schema describes it.
                    let ser_child = match ser_map {
                        Some(sm) if props.is_some() => match sm.get(canonical) {
                            Some(sv) => Some(sv),
                            None => {
                                rep.ignored.insert(sub);
                                continue;
                            }
                        },
                        Some(sm) => sm.get(canonical),
                        None => None,
                    };
                    let child = match (found, additional) {
                        (Some((_, ps)), _) => ps,
                        (None, Some(Value::Bool(true))) => continue,
                        (None, Some(ap)) if ap.is_mapping() => ap,
                        _ => {
                            rep.undescribed.insert(sub);
                            continue;
                        }
                    };
                    rep.merge(self.check(v, child, &sub, ser_child, check_stale));
                }
                if check_stale {
                    if let Some(props) = props {
                        for (pk, ps) in props {
                            // Deprecated properties are kept for reading
                            // old files but are not emitted by the engine.
                            if ps.get("deprecated").and_then(Value::as_bool) == Some(true) {
                                continue;
                            }
                            let pk = key_str(pk);
                            if !m.contains_key(pk.as_str()) {
                                rep.stale.insert(format!("{path}.{pk}"));
                            }
                        }
                    }
                }
            }
            Value::Sequence(items) => {
                let Some(item_schema) = node.get("items") else {
                    if ty != Some("array") && node.get("type").is_some() {
                        rep.undescribed
                            .insert(format!("{path} (array not allowed)"));
                    }
                    return rep;
                };
                let ser_items = serialized.and_then(Value::as_sequence);
                for (i, item) in items.iter().enumerate() {
                    let ser_item = ser_items
                        .filter(|s| s.len() == items.len())
                        .and_then(|s| s.get(i));
                    rep.merge(self.check(
                        item,
                        item_schema,
                        &format!("{path}[]"),
                        ser_item,
                        check_stale,
                    ));
                }
            }
            Value::String(s) => {
                if let Some(c) = node.get("const").and_then(Value::as_str) {
                    if c != s {
                        rep.undescribed.insert(format!("{path} (const {c} != {s})"));
                    }
                } else if let Some(e) = node.get("enum").and_then(Value::as_sequence) {
                    let aliases = node.get("x-aliases").and_then(Value::as_sequence);
                    let ok = e
                        .iter()
                        .chain(aliases.into_iter().flatten())
                        .any(|x| x.as_str() == Some(s));
                    if !ok {
                        rep.undescribed.insert(format!("{path} (enum value '{s}')"));
                    }
                } else if matches!(ty, Some("object") | Some("array")) {
                    rep.undescribed
                        .insert(format!("{path} (string not allowed)"));
                }
            }
            Value::Number(_) | Value::Bool(_) => {
                if matches!(ty, Some("object") | Some("array") | Some("string")) {
                    rep.undescribed
                        .insert(format!("{path} (scalar not allowed)"));
                }
            }
            Value::Null | Value::Tagged(_) => {}
        }
        rep
    }
}

fn collect_yaml(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            collect_yaml(&p, out);
        } else if matches!(
            p.extension().and_then(|e| e.to_str()),
            Some("yaml") | Some("yml")
        ) {
            out.push(p);
        }
    }
}

fn top_level_keys(v: &Value) -> BTreeSet<String> {
    v.as_mapping()
        .map(|m| m.keys().map(key_str).collect())
        .unwrap_or_default()
}

#[test]
fn schema_top_level_keys_match_model_input() {
    let schema = load_schema();
    let minimal = "simulation: {}\nweather_files: []\n";
    let model: ModelInput = serde_yaml::from_str(minimal).expect("minimal model parses");
    let serialized = serde_yaml::to_value(&model).unwrap();

    let engine = top_level_keys(&serialized);
    let documented = top_level_keys(&schema["properties"]);
    let missing: Vec<_> = engine.difference(&documented).collect();
    let extra: Vec<_> = documented.difference(&engine).collect();
    assert!(
        missing.is_empty() && extra.is_empty(),
        "schema top-level properties out of sync with ModelInput\n  \
         missing from schema: {missing:?}\n  in schema but not ModelInput: {extra:?}"
    );

    // The defaults the engine fills in (e.g. the whole `simulation` block)
    // must be fully described too.
    let checker = Checker { root: &schema };
    let rep = checker.check(&normalize(&serialized), &schema, "", None, false);
    assert!(
        rep.undescribed.is_empty(),
        "schema is missing paths emitted for a minimal model:\n  {}",
        rep.undescribed
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join("\n  ")
    );
}

/// Known problems in example YAMLs (not schema drift). Each entry hides a
/// real bug in an example, so fix the example and delete the entry rather
/// than adding new ones.
///
/// Example keys that serde silently ignores (`"file: key path"`).
const KNOWN_IGNORED: &[&str] = &[
    // Plant pump field is `impeller_efficiency`; this value is dropped.
    "examples/datacenter_crah.yaml: .plant_loops[].supply_equipment[].pump_efficiency",
    // Typos for `unoccupied_cooling_setpoint` / `unoccupied_heating_setpoint`.
    "examples/datacenter_crah.yaml: .thermostats[].cooling_setpoint_unoccupied",
    "examples/datacenter_crah.yaml: .thermostats[].heating_setpoint_unoccupied",
    // The example documents Standard 205 motor/drive/VFD sub-component
    // overrides, but FanInput has no `motor_a205_file` field, so it's dropped.
    "examples/vav_a205_motor_override.yaml: .air_loops[].equipment[].motor_a205_file",
    // `simulation` has no daylight-saving option.
    "prototype_tests/large_office/LargeOffice_Boulder_ideal.yaml: .simulation.daylight_saving",
];

/// Example values the engine does not recognise and silently replaces with a
/// default (key path as reported).
const KNOWN_UNDESCRIBED: &[&str] = &[
    // Fan `source` only matches vav / on_off; these fall back to constant
    // volume (examples/datacenter_crah.yaml, LargeOffice_Boulder.yaml).
    ".air_loops[].equipment[].source (enum value 'variable')",
    ".air_loops[].equipment[].source (enum value 'variable_volume')",
];

/// Inline model exercising structs that no example/test YAML uses yet, so the
/// serialized-form check (which emits every field of every struct present)
/// covers them too. Values are arbitrary; it only needs to deserialize.
const COVERAGE_FIXTURE: &str = r#"
simulation: {}
weather_files: []
climate_zone: 5B
schedules:
  - name: Compact
    compact:
      weekday: "0 until 8:00, 1.0 until 18:00, 0"
    monday: [1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1]
zones:
  - name: Z1
    zone_multiplier: 2
    infiltration:
      - design_flow_rate: 0.01
        coeff_b: 0.01
    internal_gains:
      - type: people
        count: 2
      - type: lights
        power: 100
      - type: equipment
        power: 100
    ventilation_schedule:
      - start_hour: 0
        end_hour: 6
        air_changes_per_hour: 2
    exhaust_fan:
      flow_rate: 0.05
    outdoor_air:
      per_person: 0.0025
    ideal_loads: {}
    natural_ventilation:
      opening_area: 0.5
      setpoint_reset:
        heating_setpoint: 12
        cooling_setpoint: 32
equipment_it:
  - name: IT
    zones: [Z1]
    power: 1000
air_loops:
  - name: AHU
    equipment:
      - type: dx_multispeed
        name: MS
        speeds:
          - rated_capacity: 5000
            rated_airflow: 0.3
      - type: wshp
        name: WSHP
        rated_cooling_capacity: 5000
        rated_heating_capacity: 5000
      - type: duct
        name: Duct
        length: 10
      - type: evap_cooler
        name: Evap
    zone_terminals:
      - zone: Z1
        terminal:
          type: pfp_box
          name: PFP
          max_primary_flow: autosize
          secondary_fan_flow: autosize
plant_loops:
  - name: CHW
    setpoint_reset:
      type: oa_reset
      sp_min: 6
      sp_max: 10
      oa_low: 15
      oa_high: 25
    supply_equipment:
      - type: thermal_storage
        name: TES
        tes_type: ice
        capacity_wh: 100000
        max_charge_rate: 10000
        max_discharge_rate: 10000
        control_strategy:
          demand_limiting:
            threshold_w: 50000
      - type: external_plant
        name: Ext
        command: [python, plant.py]
        inputs: [inlet_temp_c]
        outputs: [outlet_temp_c]
dhw_systems:
  - name: DHW
    mains_temperature: 12.0
    water_heater:
      name: WH
      tank_volume: 189
      capacity: 10000
    loads:
      - name: Sinks
        peak_flow_rate: 0.05
    pump:
      name: Recirc
      design_flow_rate: 0.0001
"#;

#[test]
fn schema_describes_all_example_models() {
    let schema = load_schema();
    let checker = Checker { root: &schema };
    let root = workspace_root();
    let mut files = Vec::new();
    for dir in ["examples", "140_tests/cases", "prototype_tests"] {
        collect_yaml(&root.join(dir), &mut files);
    }

    // path → first file it was seen in
    let mut undescribed: BTreeMap<String, String> = BTreeMap::new();
    let mut ignored: BTreeMap<String, String> = BTreeMap::new();
    let mut stale: BTreeSet<String> = BTreeSet::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut parsed = 0;

    let mut sources = vec![(
        "<COVERAGE_FIXTURE>".to_string(),
        COVERAGE_FIXTURE.to_string(),
    )];
    for file in &files {
        let Ok(text) = std::fs::read_to_string(file) else {
            continue;
        };
        let rel = file
            .strip_prefix(&root)
            .unwrap_or(file)
            .display()
            .to_string()
            .replace('\\', "/");
        sources.push((rel, text));
    }
    for (rel, text) in &sources {
        let model = match serde_yaml::from_str::<ModelInput>(text) {
            Ok(m) => m,
            Err(e) if rel.starts_with('<') => panic!("{rel} must parse as ModelInput: {e}"),
            Err(_) => continue,
        };
        parsed += 1;
        let mut raw: Value = serde_yaml::from_str(text).unwrap();
        raw.apply_merge().ok();
        let raw = normalize(&raw);
        let ser = normalize(&serde_yaml::to_value(&model).unwrap());

        let raw_rep = checker.check(&raw, &schema, "", Some(&ser), false);
        let ser_rep = checker.check(&ser, &schema, "", None, true);
        for p in raw_rep.undescribed.into_iter().chain(ser_rep.undescribed) {
            undescribed.entry(p).or_insert_with(|| rel.clone());
        }
        for p in raw_rep.ignored {
            ignored
                .entry(format!("{rel}: {p}"))
                .or_insert_with(|| rel.clone());
        }
        stale.extend(ser_rep.stale);
        seen.extend(ser_rep.seen);
    }
    assert!(parsed > 50, "only {parsed} example models parsed");

    // A schema property is stale only if no model ever serialized it.
    let stale: Vec<_> = stale.difference(&seen).collect();

    let ignored: Vec<_> = ignored
        .into_keys()
        .filter(|k| !KNOWN_IGNORED.contains(&k.as_str()))
        .collect();
    undescribed.retain(|p, _| !KNOWN_UNDESCRIBED.contains(&p.as_str()));
    let mut msg = String::new();
    if !undescribed.is_empty() {
        msg += "\nKey paths not described by docs/openbse_schema.json:\n";
        for (p, f) in &undescribed {
            msg += &format!("  {p}    (e.g. {f})\n");
        }
    }
    if !stale.is_empty() {
        msg += "\nSchema properties the engine never emits (stale?):\n";
        for p in &stale {
            msg += &format!("  {p}\n");
        }
    }
    if !ignored.is_empty() {
        msg += "\nExample YAML keys silently ignored by the engine (typos?):\n";
        for p in &ignored {
            msg += &format!("  {p}\n");
        }
    }
    assert!(
        msg.is_empty(),
        "schema drift detected ({parsed} models checked):{msg}"
    );
}
