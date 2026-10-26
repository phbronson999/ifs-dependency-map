//! `ifsdep --core <dir> --cust <dir> [--prev-core <dir>] [--json out.json] [--strict]`
//!
//! Indexes the folders, prints the findings, and optionally writes the full
//! graph as JSON (the same data the desktop UI loads). With `--strict` the exit
//! code is 1 when there are error-level findings, for use in CI.

use ifsdep_core::{index, IndexOptions, Severity};
use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut core: Option<PathBuf> = None;
    let mut cust: Option<PathBuf> = None;
    let mut prev_core: Option<PathBuf> = None;
    let mut json: Option<PathBuf> = None;
    let mut strict = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--core" => core = args.next().map(PathBuf::from),
            "--cust" => cust = args.next().map(PathBuf::from),
            "--prev-core" => prev_core = args.next().map(PathBuf::from),
            "--json" => json = args.next().map(PathBuf::from),
            "--strict" => strict = true,
            "-h" | "--help" => {
                println!("usage: ifsdep --core <dir> [--cust <dir>] [--prev-core <dir>] [--json <out.json>] [--strict]");
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("unknown argument: {other}");
                return ExitCode::from(2);
            }
        }
    }
    if core.is_none() && cust.is_none() {
        eprintln!("usage: ifsdep --core <dir> [--cust <dir>] [--prev-core <dir>] [--json <out.json>] [--strict]");
        return ExitCode::from(2);
    }

    let idx = index(&IndexOptions { core: core.as_deref(), cust: cust.as_deref(), prev_core: prev_core.as_deref(), ..Default::default() });
    println!(
        "{} files, {} units, {} objects, {} edges in {} ms",
        idx.stats.files, idx.stats.units, idx.stats.objects, idx.stats.edges, idx.stats.millis
    );
    for n in &idx.notes {
        println!("note: {n}");
    }
    println!();
    for f in &idx.findings {
        let sev = match f.severity {
            Severity::Error => "ERROR",
            Severity::Warning => "WARN ",
            Severity::Info => "info ",
        };
        let loc = f
            .location
            .as_ref()
            .map(|l| format!("{:?}/{}:{}", idx.files[l.file].root, idx.files[l.file].rel_path, l.line))
            .unwrap_or_default();
        println!("{sev} {:<22} {}", f.rule, f.title);
        println!("      {}", f.message);
        if !loc.is_empty() {
            println!("      at {loc}");
        }
    }
    let errors = idx.findings.iter().filter(|f| f.severity == Severity::Error).count();
    println!("\n{} findings ({errors} errors)", idx.findings.len());

    if let Some(path) = json {
        if let Err(e) = std::fs::write(&path, ifsdep_core::json::to_string(&idx)) {
            eprintln!("could not write {}: {e}", path.display());
            return ExitCode::FAILURE;
        }
        println!("wrote {}", path.display());
    }
    if strict && errors > 0 {
        ExitCode::from(1)
    } else {
        ExitCode::SUCCESS
    }
}
