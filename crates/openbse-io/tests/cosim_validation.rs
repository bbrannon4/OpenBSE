//! Co-simulation names are checked when the model is read: an unknown input
//! or output, or a missing required output, is a severe error.

fn validate(edit: impl Fn(String) -> String) -> Vec<String> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../examples/cosim_chiller/building.yaml"
    );
    let text = edit(std::fs::read_to_string(path).unwrap());
    let model = openbse_io::input::parse_model_yaml(&text).unwrap();
    openbse_io::input::validate_model(&model)
        .diagnostics
        .iter()
        .map(|d| format!("{d:?}"))
        .filter(|d| d.contains("external_plant"))
        .collect()
}

#[test]
fn shipped_cosim_model_is_clean() {
    assert!(validate(|t| t).is_empty());
}

#[test]
fn unknown_input_is_severe() {
    let errs = validate(|t| t.replace("- outdoor_temp_c       #", "- outdoor_temp       #"));
    assert_eq!(errs.len(), 1, "{errs:?}");
    assert!(errs[0].contains("unknown input 'outdoor_temp'"), "{errs:?}");
}

#[test]
fn unknown_output_and_missing_required_output_are_severe() {
    let errs = validate(|t| t.replace("- supply_temp_c        #", "- supply_temp        #"));
    assert_eq!(errs.len(), 2, "{errs:?}");
    assert!(errs
        .iter()
        .any(|e| e.contains("unknown output 'supply_temp'")));
    assert!(errs
        .iter()
        .any(|e| e.contains("must include 'supply_temp_c'")));
}
