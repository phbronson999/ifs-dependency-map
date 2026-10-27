//! `ifsdep --core <dir> --cust <dir> [--prev-core <dir>] [--json out.json] [--strict]`
//!
//! Indexes the folders, prints the findings, and optionally writes the full
//! graph as JSON (the same data the desktop UI loads). With `--strict` the exit
//! code is 1 when there are error-level findings, for use in CI.
//!
//! `ifsdep --trace <file.plsql> <line>` prints what the method declared on that line calls,
//! reads and writes, in order and by branch (what the Trace view draws).

use ifsdep_core::{index, IndexOptions, Severity};
use std::path::PathBuf;
use std::process::ExitCode;

fn trace(file: &str, line: usize) -> ExitCode {
    let text = match std::fs::read_to_string(file) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("could not read {file}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let Some(t) = ifsdep_core::trace::trace_method(&text, line) else {
        eprintln!("no PROCEDURE or FUNCTION starts on line {line}");
        return ExitCode::FAILURE;
    };
    println!("{} (lines {}-{})", t.name, t.line, t.end_line);
    print_steps(&t.steps, 1);
    ExitCode::SUCCESS
}

fn print_steps(steps: &[ifsdep_core::trace::TraceStep], indent: usize) {
    let pad = "   ".repeat(indent);
    for s in steps {
        match s.kind {
            "fork" => {
                println!("{pad}<> {} :{}", s.text, s.line);
                for a in &s.arms {
                    println!("{pad}   [{}]{} :{}", a.label, if a.implied { " (implied)" } else { "" }, a.line);
                    print_steps(&a.steps, indent + 2);
                }
            }
            "loop" | "cursor" => {
                println!("{pad}{} {} :{}", s.kind, if s.kind == "loop" { &s.text } else { &s.name }, s.line);
                print_steps(&s.steps, indent + 1);
            }
            "error" => println!("{pad}error {}{} :{}", s.package.as_deref().map(|p| format!("{p}.")).unwrap_or_default(), s.name, s.line),
            "call" => println!("{pad}call {}{} :{}", s.package.as_deref().map(|p| format!("{p}.")).unwrap_or_default(), s.name, s.line),
            k => println!("{pad}{k} {} :{}", s.name, s.line),
        }
    }
}

fn main() -> ExitCode {
    let mut core: Option<PathBuf> = None;
    let mut cust: Option<PathBuf> = None;
    let mut prev_core: Option<PathBuf> = None;
    let mut json: Option<PathBuf> = None;
    let mut strict = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--trace" => {
                let (Some(file), Some(line)) = (args.next(), args.next().and_then(|l| l.parse::<usize>().ok())) else {
                    eprintln!("usage: ifsdep --trace <file.plsql> <line>");
                    return ExitCode::from(2);
                };
                return trace(&file, line);
            }
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
