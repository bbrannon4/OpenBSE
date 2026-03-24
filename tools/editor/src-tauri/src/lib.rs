use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use tauri::menu::{MenuBuilder, MenuItemBuilder, PredefinedMenuItem, SubmenuBuilder};
use tauri::{Emitter, Manager};

/// Resolve the schema path.
/// 1. Bundled Tauri resource (production builds)
/// 2. Relative to CWD (dev, running from repo root)
/// 3. Walk up from CWD or executable to find repo root
fn find_schema_path(app_handle: Option<&tauri::AppHandle>) -> Option<PathBuf> {
    // Try bundled resource first (production builds)
    if let Some(handle) = app_handle {
        if let Ok(resource_dir) = handle.path().resource_dir() {
            let bundled = resource_dir.join("docs/openbse_schema.json");
            if bundled.exists() {
                return Some(bundled);
            }
        }
    }

    // Try relative to CWD (works in dev when run from repo root or tools/editor)
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
            if let Some(found) = walk_up_for_schema(exe_dir) {
                return Some(found);
            }
        }
    }

    None
}

fn walk_up_for_schema(start: &std::path::Path) -> Option<PathBuf> {
    let mut dir = start;
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
fn load_schema(app_handle: tauri::AppHandle) -> Result<serde_json::Value, String> {
    let path = find_schema_path(Some(&app_handle)).ok_or_else(|| {
        "Could not find openbse_schema.json. The schema file may be missing from the installation.".to_string()
    })?;

    let contents =
        std::fs::read_to_string(&path).map_err(|e| format!("Failed to read schema: {e}"))?;

    let schema: serde_json::Value =
        serde_json::from_str(&contents).map_err(|e| format!("Failed to parse schema JSON: {e}"))?;

    Ok(schema)
}

#[tauri::command]
fn read_yaml_file(path: String) -> Result<String, String> {
    std::fs::read_to_string(&path).map_err(|e| format!("Failed to read file {path}: {e}"))
}

#[tauri::command]
fn write_yaml_file(path: String, contents: String) -> Result<(), String> {
    std::fs::write(&path, &contents).map_err(|e| format!("Failed to write file {path}: {e}"))
}

/// Find the openbse CLI binary.
/// 1. Next to the editor executable (bundled distribution)
/// 2. On PATH
fn find_openbse_binary() -> Result<PathBuf, String> {
    // Next to current executable
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            let candidate = exe_dir.join("openbse");
            if candidate.exists() {
                return Ok(candidate);
            }
        }
    }

    // On PATH via `which`
    which::which("openbse")
        .map_err(|_| "Could not find the 'openbse' binary. Install it or place it next to the editor executable.".to_string())
}

#[derive(Clone, serde::Serialize)]
struct SimulationOutput {
    stream: String, // "stdout" or "stderr"
    line: String,
}

#[derive(Clone, serde::Serialize)]
struct SimulationDone {
    success: bool,
    code: Option<i32>,
    output_path: Option<String>,
}

#[tauri::command]
async fn run_simulation(
    app_handle: tauri::AppHandle,
    model_path: String,
    weather_path: Option<String>,
    output_path: String,
) -> Result<(), String> {
    let binary = find_openbse_binary()?;

    let mut args = vec![model_path.clone()];
    if let Some(ref wp) = weather_path {
        args.push("-w".to_string());
        args.push(wp.clone());
    }
    args.push("-o".to_string());
    args.push(output_path.clone());

    // Run the entire blocking process on a dedicated thread so we don't
    // block the Tauri async runtime (which would prevent events from
    // being delivered to the frontend).
    let (tx, rx) = std::sync::mpsc::channel::<Result<(), String>>();

    std::thread::spawn(move || {
        let result = (|| -> Result<(), String> {
            let mut child = Command::new(&binary)
                .args(&args)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .map_err(|e| format!("Failed to start openbse: {e}"))?;

            let stdout = child.stdout.take();
            let stderr = child.stderr.take();
            let app1 = app_handle.clone();
            let app2 = app_handle.clone();

            let stdout_handle = std::thread::spawn(move || {
                if let Some(out) = stdout {
                    for line in BufReader::new(out).lines().map_while(Result::ok) {
                        let _ = app1.emit(
                            "simulation-output",
                            SimulationOutput {
                                stream: "stdout".to_string(),
                                line,
                            },
                        );
                    }
                }
            });

            let stderr_handle = std::thread::spawn(move || {
                if let Some(err) = stderr {
                    for line in BufReader::new(err).lines().map_while(Result::ok) {
                        let _ = app2.emit(
                            "simulation-output",
                            SimulationOutput {
                                stream: "stderr".to_string(),
                                line,
                            },
                        );
                    }
                }
            });

            let status = child
                .wait()
                .map_err(|e| format!("Failed to wait for openbse: {e}"))?;

            let _ = stdout_handle.join();
            let _ = stderr_handle.join();

            let success = status.success();
            let output_exists = std::path::Path::new(&output_path).exists();

            let _ = app_handle.emit(
                "simulation-done",
                SimulationDone {
                    success,
                    code: status.code(),
                    output_path: if output_exists {
                        Some(output_path)
                    } else {
                        None
                    },
                },
            );

            if success {
                Ok(())
            } else {
                Err(format!(
                    "Simulation exited with code {}",
                    status.code().unwrap_or(-1)
                ))
            }
        })();

        let _ = tx.send(result);
    });

    // Await the result without blocking the async runtime
    tauri::async_runtime::spawn_blocking(move || {
        rx.recv().unwrap_or(Err("Thread died".to_string()))
    })
    .await
    .map_err(|e| format!("Join error: {e}"))?
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

            // Build native menu bar
            let handle = app.handle();

            let new_item = MenuItemBuilder::with_id("file_new", "New")
                .accelerator("CmdOrCtrl+N")
                .build(handle)?;
            let open_item = MenuItemBuilder::with_id("file_open", "Open...")
                .accelerator("CmdOrCtrl+O")
                .build(handle)?;
            let save_item = MenuItemBuilder::with_id("file_save", "Save")
                .accelerator("CmdOrCtrl+S")
                .build(handle)?;
            let save_as_item = MenuItemBuilder::with_id("file_save_as", "Save As...")
                .accelerator("CmdOrCtrl+Shift+S")
                .build(handle)?;

            let file_menu = SubmenuBuilder::new(handle, "File")
                .items(&[
                    &new_item,
                    &open_item,
                    &PredefinedMenuItem::separator(handle)?,
                    &save_item,
                    &save_as_item,
                    &PredefinedMenuItem::separator(handle)?,
                    &PredefinedMenuItem::quit(handle, None)?,
                ])
                .build()?;

            let edit_menu = SubmenuBuilder::new(handle, "Edit")
                .items(&[
                    &PredefinedMenuItem::undo(handle, None)?,
                    &PredefinedMenuItem::redo(handle, None)?,
                    &PredefinedMenuItem::separator(handle)?,
                    &PredefinedMenuItem::cut(handle, None)?,
                    &PredefinedMenuItem::copy(handle, None)?,
                    &PredefinedMenuItem::paste(handle, None)?,
                    &PredefinedMenuItem::select_all(handle, None)?,
                ])
                .build()?;

            let menu = MenuBuilder::new(handle)
                .items(&[&file_menu, &edit_menu])
                .build()?;
            app.set_menu(menu)?;

            // Handle File menu events → emit to frontend
            app.on_menu_event(move |app_handle, event| {
                let id = event.id().0.as_str();
                match id {
                    "file_new" | "file_open" | "file_save" | "file_save_as" => {
                        let _ = app_handle.emit("menu-action", id);
                    }
                    _ => {}
                }
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            load_schema,
            read_yaml_file,
            write_yaml_file,
            run_simulation,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
