fn control() -> &'static str {
    r#"simulation: {}
weather_files: []
dhw_systems:
  - name: DHW
    water_heater:
      name: WH
      tank_volume: 100
      capacity: 1000
    mains_temperature:
      correlation:
        annual_avg_temp: 10
        max_monthly_delta: 20
"#
}

#[test]
fn synthetic_correlation_is_valid() {
    openbse_io::input::parse_model_yaml(control()).expect("valid correlation control");
}

#[test]
fn synthetic_fixed_temperature_is_valid() {
    let text = control()
        .split("    mains_temperature:")
        .next()
        .unwrap()
        .to_string()
        + "    mains_temperature: 10\n";
    openbse_io::input::parse_model_yaml(&text).expect("valid fixed-temperature control");
}

#[test]
fn outer_correlation_typo_is_rejected() {
    let text = control().replace(
        "      correlation:\n",
        "      unexpected_setting: 123\n      correlation:\n",
    );
    assert!(openbse_io::input::parse_model_yaml(&text).is_err());
}

#[test]
fn inner_correlation_typo_is_rejected() {
    let text = control().replace(
        "        annual_avg_temp: 10\n",
        "        annual_avg_temp: 10\n        unexpected_setting: 123\n",
    );
    assert!(openbse_io::input::parse_model_yaml(&text).is_err());
}
