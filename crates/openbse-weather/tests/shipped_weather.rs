//! Every weather file shipped with the repo must pass the strict parser: no
//! malformed records, consecutive hours, valid location header.

use std::path::PathBuf;

#[test]
fn shipped_weather_files_parse() {
    let dir = PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../140_tests/weather"
    ));
    let mut checked = 0;
    for entry in std::fs::read_dir(&dir).expect("140_tests/weather exists") {
        let path = entry.unwrap().path();
        let is_weather = matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("epw") | Some("csv")
        );
        if !is_weather {
            continue;
        }
        let data = openbse_weather::read_weather_file(&path)
            .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let first = &data.hours[0];
        assert_eq!(
            (first.month, first.day, first.hour),
            (1, 1, 1),
            "{} must start on 01/01 hour 1",
            path.display()
        );
        checked += 1;
    }
    assert!(checked > 0, "no weather files found in {}", dir.display());
}
