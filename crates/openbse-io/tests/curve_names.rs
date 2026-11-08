//! A named curve that isn't in `performance_curves` is a severe error, not a
//! silent fallback to the component's default curve or constant efficiency.

fn severe(path: &str, edit: impl Fn(String) -> String) -> Vec<String> {
    let path = format!("{}/../../{path}", env!("CARGO_MANIFEST_DIR"));
    let text = edit(std::fs::read_to_string(path).unwrap());
    let model = openbse_io::input::parse_model_yaml(&text).unwrap();
    openbse_io::input::validate_model(&model)
        .diagnostics
        .iter()
        .map(|d| format!("{d:?}"))
        .filter(|d| d.contains("not found in performance_curves"))
        .collect()
}

#[test]
fn dx_coil_curve_names_resolve() {
    let model = "examples/doe_retail_standalone.yaml";
    assert!(severe(model, |t| t).is_empty());
    let errs = severe(model, |t| {
        t.replacen(
            "cap_ft_curve: DX Cool Cap fT",
            "cap_ft_curve: DX Cool Cap ft",
            1,
        )
    });
    assert_eq!(errs.len(), 1, "{errs:?}");
    assert!(errs[0].contains("Core Retail DX': cap_ft_curve 'DX Cool Cap ft'"));
}

#[test]
fn boiler_efficiency_curve_name_resolves() {
    let errs = severe("examples/vav_chw_plant.yaml", |t| {
        t.replace(
            "        efficiency: 0.85\n        design_outlet_temp: 82.0",
            "        efficiency: 0.85\n        efficiency_curve: Boiler Eff\n        design_outlet_temp: 82.0",
        )
    });
    assert_eq!(errs.len(), 1, "{errs:?}");
    assert!(errs[0].contains("Boiler 'HHW Boiler': efficiency_curve 'Boiler Eff'"));
}
