use std::path::PathBuf;

/// Resolve the schema path relative to the repo root.
/// In dev, we walk up from the executable/CWD to find docs/openbse_schema.json.
/// The schema is also bundled as a Tauri resource for production builds.
fn find_schema_path() -> Option<PathBuf> {
    // Try relative to CWD first (works in dev when run from repo root or tools/editor)
    let candidates = [
        PathBuf::from("docs/openbse_schema.json"),
        PathBuf::from("../../docs/openbse_schema.json"),
        PathBuf::from("../../../docs/openbse_schema.json"),
    ];
    for candidate in &candidates {
        if candidate.exists() {
            return Some(candidate.clone());
        }
    }

    // Walk up from current dir
    if let Ok(cwd) = std::env::current_dir() {
        if let Some(found) = walk_up_for_schema(&cwd) {
            return Some(found);
        }
    }

    // Walk up from executable path (handles Finder launch where CWD is /)
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            if let Some(found) = walk_up_for_schema(&exe_dir.to_path_buf()) {
                return Some(found);
            }
        }
    }

    None
}

fn walk_up_for_schema(start: &PathBuf) -> Option<PathBuf> {
    let mut dir = start.as_path();
    loop {
        let candidate = dir.join("docs/openbse_schema.json");
        if candidate.exists() {
            return Some(candidate);
        }
        match dir.parent() {
            Some(parent) => dir = parent,
            None => return None,
        }
    }
}

#[tauri::command]
fn load_schema() -> Result<serde_json::Value, String> {
    let path = find_schema_path().ok_or_else(|| {
        "Could not find docs/openbse_schema.json. Run from the OpenBSE repo root.".to_string()
    })?;

    let contents = std::fs::read_to_string(&path)
        .map_err(|e| format!("Failed to read schema: {e}"))?;

    let schema: serde_json::Value = serde_json::from_str(&contents)
        .map_err(|e| format!("Failed to parse schema JSON: {e}"))?;

    Ok(schema)
}

#[tauri::command]
fn read_yaml_file(path: String) -> Result<String, String> {
    std::fs::read_to_string(&path)
        .map_err(|e| format!("Failed to read file {path}: {e}"))
}

#[tauri::command]
fn write_yaml_file(path: String, contents: String) -> Result<(), String> {
    std::fs::write(&path, &contents)
        .map_err(|e| format!("Failed to write file {path}: {e}"))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .setup(|app| {
            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Info)
                        .build(),
                )?;
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            load_schema,
            read_yaml_file,
            write_yaml_file,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
