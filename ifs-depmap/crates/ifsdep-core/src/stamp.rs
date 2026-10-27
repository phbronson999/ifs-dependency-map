//! Each indexed file's size and modified time. Saved next to an index, they tell the next
//! launch which files changed without reading any of them.

use crate::scan;
use std::collections::HashMap;
use std::path::Path;
use std::time::UNIX_EPOCH;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Stamp {
    /// Absolute path, as the indexer read it.
    pub path: String,
    pub size: u64,
    /// Milliseconds since 1970.
    pub modified: u64,
}

/// Size and modified time from a file's metadata.
pub fn of(meta: &std::fs::Metadata) -> (u64, u64) {
    let modified = meta.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_millis() as u64);
    (meta.len(), modified)
}

/// The stamps of every file the indexer would read under these roots.
pub fn stamps(roots: &[&Path]) -> Vec<Stamp> {
    let mut out = Vec::new();
    for root in roots {
        if !root.is_dir() {
            continue;
        }
        let (files, _) = scan::scan_root(root);
        out.extend(files.into_iter().map(|s| Stamp { path: s.abs.display().to_string(), size: s.size, modified: s.modified }));
    }
    out
}

/// One line per file: `size<TAB>modified<TAB>path`.
pub fn write(stamps: &[Stamp], out: &mut String) {
    for s in stamps {
        out.push_str(&format!("{}\t{}\t{}\n", s.size, s.modified, s.path));
    }
}

/// Reads what `write` wrote; lines that don't parse are skipped.
pub fn read(text: &str) -> Vec<Stamp> {
    text.lines()
        .filter_map(|l| {
            let mut p = l.splitn(3, '\t');
            let size = p.next()?.parse().ok()?;
            let modified = p.next()?.parse().ok()?;
            Some(Stamp { path: p.next()?.to_string(), size, modified })
        })
        .collect()
}

/// What differs between the stamps saved with an index and the folders now.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Changes {
    /// Same path, different size or modified time.
    pub changed: Vec<String>,
    pub added: Vec<String>,
    pub removed: Vec<String>,
}

impl Changes {
    pub fn is_empty(&self) -> bool {
        self.changed.is_empty() && self.added.is_empty() && self.removed.is_empty()
    }
}

pub fn compare(saved: &[Stamp], now: &[Stamp]) -> Changes {
    let before: HashMap<&str, &Stamp> = saved.iter().map(|s| (s.path.as_str(), s)).collect();
    let mut c = Changes::default();
    for s in now {
        match before.get(s.path.as_str()) {
            None => c.added.push(s.path.clone()),
            Some(b) if b.size != s.size || b.modified != s.modified => c.changed.push(s.path.clone()),
            Some(_) => {}
        }
    }
    let after: HashMap<&str, ()> = now.iter().map(|s| (s.path.as_str(), ())).collect();
    c.removed = saved.iter().filter(|s| !after.contains_key(s.path.as_str())).map(|s| s.path.clone()).collect();
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn written_stamps_read_back_and_compare() {
        let st = |p: &str, size, modified| Stamp { path: p.into(), size, modified };
        let saved = vec![st("C:\\a b\\x.plsql", 10, 100), st("y.views", 5, 50), st("gone.client", 1, 1)];
        let mut text = String::new();
        write(&saved, &mut text);
        assert_eq!(read(&text), saved);
        let now = vec![st("C:\\a b\\x.plsql", 10, 100), st("y.views", 6, 50), st("new.entity", 2, 2)];
        let c = compare(&saved, &now);
        assert_eq!(c, Changes { changed: vec!["y.views".into()], added: vec!["new.entity".into()], removed: vec!["gone.client".into()] });
        assert!(compare(&saved, &saved).is_empty());
    }
}
