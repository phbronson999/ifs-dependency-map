//! Thin Tauri shell around `ifsdep-core`. The UI calls these commands:
//!
//! - `pick_folder`    → native folder picker
//! - `index_folders`  → runs the indexer, returns the graph as a JSON string; on the way it
//!                       emits `index-progress` events: `[stage, done, total]`. The result is
//!                       saved (see below).
//! - `cancel_index`   → stops the running `index_folders`, which then fails with "cancelled"
//! - `trace_method`   → what the method declared at a file's line calls, reads and writes,
//!                       in order and by branch (the Trace view), as a JSON string
//! - `open_in_editor` → opens a file at a line in VS Code (`code -g path:line`)
//! - `load_saved`     → the saved index for these folders, if this build of the app saved it:
//!                       `{"saved": {saved_at, bytes, path}, "index": …}`, or null
//! - `check_saved`    → how many files changed, were added or removed since it was saved
//! - `saved_info`     → `{saved_at, bytes, path}` of the saved index, or null
//! - `reveal_saved`   → shows the saved index in Explorer
//!
//! The last index is saved in the app's own data folder (`saved-index.json`, plus
//! `saved-index.txt` with the folders, the app build and every file's size and modified
//! time). The tool is read-only: nothing here writes to the indexed folders.

use ifsdep_core::stamp::{self, Stamp};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{Emitter, Manager};
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
        let emitter = app.clone();
        let progress = move |stage: &str, done: usize, total: usize| {
            let _ = emitter.emit("index-progress", (stage, done, total));
        };
        let opts = ifsdep_core::IndexOptions {
            core: core.as_deref(),
            cust: cust.as_deref(),
            prev_core: prev_core.as_deref(),
            cancel: Some(&CANCEL),
            progress: Some(&progress),
        };
        let idx = ifsdep_core::try_index(&opts).map_err(|_| "cancelled".to_string())?;
        let json = ifsdep_core::json::to_string(&idx);
        let _ = app.emit("index-progress", ("Saving the index", 0, 0));
        let mut stamps = idx.stamps.clone();
        if let Some(prev) = prev_core.as_deref() {
            stamps.extend(stamp::stamps(&[prev]));
        }
        let roots = [&opts.core, &opts.cust, &opts.prev_core].map(|r| r.map(|p| p.display().to_string()).unwrap_or_default());
        // Not saving costs the next launch time, nothing more: say so, and still show the index.
        if let Err(e) = write_saved(&app, &roots, &json, &stamps) {
            eprintln!("couldn't save the index: {e}");
        }
        Ok(json)
    })
    .await
    .map_err(|e| e.to_string())?
}

// ------------------------------------------------------------------ saved index

const SAVED_HEADER: &str = "ifs-depmap saved index 1";

/// The saved index and its list, in the app's own data folder.
fn saved_files(app: &tauri::AppHandle) -> Result<(PathBuf, PathBuf), String> {
    let dir = app.path().app_data_dir().map_err(|e| e.to_string())?;
    Ok((dir.join("saved-index.json"), dir.join("saved-index.txt")))
}

/// This build of the app. An index saved by another build may come from different parsers,
/// so it isn't reused.
fn app_build() -> String {
    std::env::current_exe()
        .and_then(std::fs::metadata)
        .map(|m| {
            let (size, modified) = stamp::of(&m);
            format!("{size}-{modified}")
        })
        .unwrap_or_default()
}

struct SavedMeta {
    /// Core, Cust and previous Core, "" when not set.
    roots: [String; 3],
    build: String,
    saved_at: u64,
    stamps: Vec<Stamp>,
}

fn read_meta(path: &Path) -> Option<SavedMeta> {
    let text = std::fs::read_to_string(path).ok()?;
    let (head, files) = text.split_once("\nfiles\n")?;
    let mut lines = head.lines();
    if lines.next()? != SAVED_HEADER {
        return None;
    }
    let mut m = SavedMeta { roots: Default::default(), build: String::new(), saved_at: 0, stamps: stamp::read(files) };
    for l in lines {
        let (k, v) = l.split_once('\t').unwrap_or((l, ""));
        match k {
            "core" => m.roots[0] = v.into(),
            "cust" => m.roots[1] = v.into(),
            "prev" => m.roots[2] = v.into(),
            "build" => m.build = v.into(),
            "saved" => m.saved_at = v.parse().unwrap_or(0),
            _ => {}
        }
    }
    Some(m)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// Saves an index over the last one. The list is removed first and written last, so a list
/// on disk always belongs to a complete index.
fn write_saved(app: &tauri::AppHandle, roots: &[String; 3], json: &str, stamps: &[Stamp]) -> Result<(), String> {
    let (index_path, meta_path) = saved_files(app)?;
    std::fs::create_dir_all(index_path.parent().unwrap()).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(&meta_path);
    std::fs::write(&index_path, json).map_err(|e| e.to_string())?;
    let mut meta = format!(
        "{SAVED_HEADER}\ncore\t{}\ncust\t{}\nprev\t{}\nbuild\t{}\nsaved\t{}\nfiles\n",
        roots[0],
        roots[1],
        roots[2],
        app_build(),
        now_ms()
    );
    stamp::write(stamps, &mut meta);
    std::fs::write(&meta_path, meta).map_err(|e| e.to_string())
}

fn info_json(saved_at: u64, bytes: u64, path: &Path) -> String {
    let mut p = String::new();
    ifsdep_core::json::write_str(&path.display().to_string(), &mut p);
    format!("{{\"saved_at\":{saved_at},\"bytes\":{bytes},\"path\":{p}}}")
}

fn roots(core: Option<String>, cust: Option<String>, prev_core: Option<String>) -> [String; 3] {
    [core, cust, prev_core].map(Option::unwrap_or_default)
}

#[tauri::command]
async fn load_saved(app: tauri::AppHandle, core: Option<String>, cust: Option<String>, prev_core: Option<String>) -> Result<Option<String>, String> {
    let want = roots(core, cust, prev_core);
    tauri::async_runtime::spawn_blocking(move || {
        let (index_path, meta_path) = saved_files(&app)?;
        let Some(m) = read_meta(&meta_path) else { return Ok(None) };
        if m.roots != want || m.build != app_build() {
            return Ok(None);
        }
        let Ok(json) = std::fs::read_to_string(&index_path) else { return Ok(None) };
        Ok(Some(format!("{{\"saved\":{},\"index\":{json}}}", info_json(m.saved_at, json.len() as u64, &index_path))))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
async fn check_saved(app: tauri::AppHandle, core: Option<String>, cust: Option<String>, prev_core: Option<String>) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let (_, meta_path) = saved_files(&app)?;
        let m = read_meta(&meta_path).ok_or("no saved index")?;
        let folders: Vec<PathBuf> = [core, cust, prev_core].into_iter().flatten().map(PathBuf::from).collect();
        let now = stamp::stamps(&folders.iter().map(PathBuf::as_path).collect::<Vec<_>>());
        let c = stamp::compare(&m.stamps, &now);
        Ok(format!("{{\"changed\":{},\"added\":{},\"removed\":{}}}", c.changed.len(), c.added.len(), c.removed.len()))
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
fn saved_info(app: tauri::AppHandle) -> Option<String> {
    let (index_path, meta_path) = saved_files(&app).ok()?;
    let m = read_meta(&meta_path)?;
    let bytes = std::fs::metadata(&index_path).ok()?.len();
    Some(info_json(m.saved_at, bytes, &index_path))
}

#[tauri::command]
fn reveal_saved(app: tauri::AppHandle) -> Result<(), String> {
    let (index_path, _) = saved_files(&app)?;
    let result = if cfg!(windows) {
        Command::new("explorer").arg(format!("/select,{}", index_path.display())).spawn()
    } else {
        Command::new(if cfg!(target_os = "macos") { "open" } else { "xdg-open" }).arg(index_path.parent().unwrap()).spawn()
    };
    result.map(|_| ()).map_err(|e| e.to_string())
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
        .invoke_handler(tauri::generate_handler![pick_folder, index_folders, cancel_index, trace_method, open_in_editor, load_saved, check_saved, saved_info, reveal_saved])
        .run(tauri::generate_context!())
        .expect("error while running IFS Dependency Map");
}
