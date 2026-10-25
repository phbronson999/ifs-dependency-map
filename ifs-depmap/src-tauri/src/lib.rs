//! Thin Tauri shell around `ifsdep-core`. The UI calls three commands:
//!
//! - `pick_folder`    → native folder picker
//! - `index_folders`  → runs the indexer, returns the graph as a JSON string
//! - `open_in_editor` → opens a file at a line in VS Code (`code -g path:line`)
//!
//! The tool is read-only: nothing here writes to the indexed folders.

use std::path::PathBuf;
use std::process::Command;
use tauri_plugin_dialog::DialogExt;

#[tauri::command]
async fn pick_folder(app: tauri::AppHandle, title: Option<String>) -> Option<String> {
    let mut dialog = app.dialog().file();
    if let Some(t) = title {
        dialog = dialog.set_title(t);
    }
    // The blocking picker must not run on the main thread; async commands don't.
    dialog.blocking_pick_folder().and_then(|p| p.into_path().ok()).map(|p| p.display().to_string())
}

#[tauri::command]
async fn index_folders(core: Option<String>, cust: Option<String>, prev_core: Option<String>) -> Result<String, String> {
    if core.is_none() && cust.is_none() {
        return Err("Choose a Core or Cust folder first.".into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let core = core.map(PathBuf::from);
        let cust = cust.map(PathBuf::from);
        let prev_core = prev_core.map(PathBuf::from);
        let idx = ifsdep_core::index(&ifsdep_core::IndexOptions { core: core.as_deref(), cust: cust.as_deref(), prev_core: prev_core.as_deref() });
        ifsdep_core::json::to_string(&idx)
    })
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
fn open_in_editor(path: String, line: usize) -> Result<(), String> {
    let target = format!("{path}:{line}");
    // On Windows `code` is a .cmd shim, so it has to go through cmd.
    let result = if cfg!(windows) {
        Command::new("cmd").args(["/C", "code", "-g", &target]).spawn()
    } else {
        Command::new("code").args(["-g", &target]).spawn()
    };
    result.map(|_| ()).map_err(|e| format!("couldn't start VS Code ({e}). Is `code` on your PATH?"))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![pick_folder, index_folders, open_in_editor])
        .run(tauri::generate_context!())
        .expect("error while running IFS Dependency Map");
}
