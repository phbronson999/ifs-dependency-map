//! Thin Tauri shell around `ifsdep-core`. The UI calls these commands:
//!
//! - `pick_folder`    → native folder picker
//! - `index_folders`  → runs the indexer, returns the graph as a JSON string; on the way it
//!                       emits `index-progress` events: `[stage, done, total]`
//! - `cancel_index`   → stops the running `index_folders`, which then fails with "cancelled"
//! - `trace_method`   → what the method declared at a file's line calls, reads and writes,
//!                       in order and by branch (the Trace view), as a JSON string
//! - `open_in_editor` → opens a file at a line in VS Code (`code -g path:line`)
//!
//! The tool is read-only: nothing here writes to the indexed folders.

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::Emitter;
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

/// Set by `cancel_index`; the indexer checks it between files and stages.
static CANCEL: AtomicBool = AtomicBool::new(false);

#[tauri::command]
fn cancel_index() {
    CANCEL.store(true, Ordering::Relaxed);
}

#[tauri::command]
async fn index_folders(app: tauri::AppHandle, core: Option<String>, cust: Option<String>, prev_core: Option<String>) -> Result<String, String> {
    if core.is_none() && cust.is_none() {
        return Err("Choose a Core or Cust folder first.".into());
    }
    CANCEL.store(false, Ordering::Relaxed);
    tauri::async_runtime::spawn_blocking(move || {
        let core = core.map(PathBuf::from);
        let cust = cust.map(PathBuf::from);
        let prev_core = prev_core.map(PathBuf::from);
        let progress = move |stage: &str, done: usize, total: usize| {
            let _ = app.emit("index-progress", (stage, done, total));
        };
        let opts = ifsdep_core::IndexOptions {
            core: core.as_deref(),
            cust: cust.as_deref(),
            prev_core: prev_core.as_deref(),
            cancel: Some(&CANCEL),
            progress: Some(&progress),
        };
        ifsdep_core::try_index(&opts).map(|idx| ifsdep_core::json::to_string(&idx)).map_err(|_| "cancelled".to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
fn trace_method(path: String, line: usize) -> Result<String, String> {
    let text = std::fs::read_to_string(&path).map_err(|e| format!("couldn't read {path} ({e})"))?;
    let trace = ifsdep_core::trace::trace_method(&text, line).ok_or_else(|| format!("no PROCEDURE or FUNCTION starts on line {line} of {path}"))?;
    Ok(ifsdep_core::json::to_string(&trace))
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
        .invoke_handler(tauri::generate_handler![pick_folder, index_folders, cancel_index, trace_method, open_in_editor])
        .run(tauri::generate_context!())
        .expect("error while running IFS Dependency Map");
}
