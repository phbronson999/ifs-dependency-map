use crate::model::FileKind;
use std::path::{Path, PathBuf};

pub struct Scanned {
    pub abs: PathBuf,
    pub rel: String,
    pub kind: FileKind,
    /// First folder under the root, upper-cased (`shpord/...` → `SHPORD`).
    pub path_component: Option<String>,
    pub size: u64,
    /// Modified time, milliseconds since 1970.
    pub modified: u64,
}

/// Walks one root (without following symlinks) and returns every file type
/// the indexer understands.
pub fn scan_root(root: &Path) -> (Vec<Scanned>, Vec<String>) {
    let mut out = Vec::new();
    let mut notes = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) => {
                notes.push(format!("skipped {}: {e}", dir.display()));
                continue;
            }
        };
        for entry in entries.flatten() {
            let Ok(ft) = entry.file_type() else { continue };
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if ft.is_dir() {
                if !(name.starts_with('.') || name == "node_modules" || name == "target") {
                    stack.push(path);
                }
                continue;
            }
            if !ft.is_file() {
                continue;
            }
            let Some(kind) = FileKind::from_path(&path) else { continue };
            let (size, modified) = entry.metadata().map(|m| crate::stamp::of(&m)).unwrap_or((0, 0));
            let rel_path = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
            let path_component = if rel_path.components().count() > 1 {
                rel_path
                    .components()
                    .next()
                    .and_then(|c| c.as_os_str().to_str())
                    .map(|s| s.to_ascii_uppercase())
            } else {
                None
            };
            out.push(Scanned {
                rel: rel_path.to_string_lossy().replace('\\', "/"),
                abs: path,
                kind,
                path_component,
                size,
                modified,
            });
        }
    }
    out.sort_by(|a, b| a.rel.cmp(&b.rel));
    (out, notes)
}
