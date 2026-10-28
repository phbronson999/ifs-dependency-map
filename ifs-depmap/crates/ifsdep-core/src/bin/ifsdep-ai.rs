//! `ifsdep-ai`: IFS Dependency Map for AI coding assistants (see `ai.rs`).
//!
//! `ifsdep-ai init --core <dir> --cust <dir> [--prev-core <dir>] [--workspace <dir>]` writes the
//! AI index and the `ifs-codebase` skill into a workspace. Only this command writes them.
//! `ifsdep-ai find | callers | callees | impact | unit | trace <name>`, `ifsdep-ai findings` and
//! `ifsdep-ai status` answer questions from the AI index.

use ifsdep_core::{ai, index, IndexOptions};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const USAGE: &str = "usage:
  ifsdep-ai init --core <dir> [--cust <dir>] [--prev-core <dir>] [--workspace <dir>]
  ifsdep-ai find|callers|callees|impact|unit|trace <name> [--depth N] [--limit N] [--index <file>]
  ifsdep-ai findings [--file <part of path>] [--unit <name>] [--rule <RULE>] [--cust] [--limit N] [--index <file>]
  ifsdep-ai status [--index <file>]

init writes ifsdep-ai.idx and .claude/skills/ifs-codebase/SKILL.md into the workspace
(the current folder unless --workspace is given), for Claude Code sessions started there.";

const QUERIES: &[&str] = &["find", "callers", "callees", "impact", "unit", "trace", "findings", "status"];

/// `a` lies in or under `b` (Windows paths: any case).
fn is_under(a: &Path, b: &Path) -> bool {
    let low = |p: &Path| p.display().to_string().replace('/', "\\").trim_end_matches('\\').to_ascii_lowercase();
    let (a, b) = (low(a), low(b));
    a == b || a.starts_with(&format!("{b}\\"))
}

fn init(args: Vec<String>) -> ExitCode {
    let (mut core, mut cust, mut prev_core, mut workspace) = (None, None, None, None);
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        let v = it.next().map(PathBuf::from);
        match a.as_str() {
            "--core" => core = v,
            "--cust" => cust = v,
            "--prev-core" => prev_core = v,
            "--workspace" => workspace = v,
            _ => {
                eprintln!("unknown argument: {a}\n{USAGE}");
                return ExitCode::from(2);
            }
        }
    }
    if core.is_none() && cust.is_none() {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    }
    let abs = |p: Option<PathBuf>| p.map(|p| std::path::absolute(&p).unwrap_or(p));
    let (core, cust, prev_core) = (abs(core), abs(cust), abs(prev_core));
    let workspace = abs(workspace).unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    // The IFS folders are read-only input: check before spending half a minute indexing.
    for root in [&core, &cust, &prev_core].into_iter().flatten() {
        if is_under(&workspace, root) {
            eprintln!("the workspace {} is inside the IFS folder {}, which is never written to; pick the folder above it with --workspace", workspace.display(), root.display());
            return ExitCode::from(2);
        }
    }

    eprintln!("indexing (about half a minute on a full Core)…");
    let idx = index(&IndexOptions { core: core.as_deref(), cust: cust.as_deref(), prev_core: prev_core.as_deref(), ..Default::default() });
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("ifsdep-ai"));
    let w = match ai::init(&idx, prev_core.as_deref(), &workspace, &exe) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };

    println!(
        "{} files, {} units, {} objects, {} links, {} findings in {} ms",
        idx.stats.files,
        idx.stats.units,
        idx.stats.objects,
        idx.stats.edges,
        idx.findings.len(),
        idx.stats.millis
    );
    println!("wrote {} ({:.1} MB)", w.index.display(), w.index_bytes as f64 / 1e6);
    println!("wrote {}", w.skill.display());
    println!("Claude Code sessions started in {} now pick up the ifs-codebase skill.", workspace.display());
    println!("Run this again to refresh both after the code changes.");
    ExitCode::SUCCESS
}

fn query(cmd: &str, args: Vec<String>) -> ExitCode {
    let mut name: Option<String> = None;
    let (mut file, mut unit, mut rule, mut index_path) = (None, None, None, None);
    let mut cust = false;
    let mut opts = ai::Opts { depth: if cmd == "impact" { 6 } else { 1 }, limit: if cmd == "findings" { 30 } else { 150 } };
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--depth" | "--limit" => {
                let Some(n) = it.next().and_then(|v| v.parse::<usize>().ok()).filter(|&n| n > 0) else {
                    eprintln!("{a} needs a number");
                    return ExitCode::from(2);
                };
                if a == "--depth" {
                    opts.depth = n;
                } else {
                    opts.limit = n;
                }
            }
            "--file" => file = it.next(),
            "--unit" => unit = it.next(),
            "--rule" => rule = it.next(),
            "--index" => index_path = it.next().map(PathBuf::from),
            "--cust" => cust = true,
            s if !s.starts_with("--") && name.is_none() => name = Some(a),
            _ => {
                eprintln!("unknown argument: {a}\n{USAGE}");
                return ExitCode::from(2);
            }
        }
    }
    let needs_name = !matches!(cmd, "findings" | "status");
    if needs_name && name.is_none() {
        eprintln!("ifsdep-ai {cmd} needs a name, e.g. Customer_Order_API.Get_Customer_Po_No\n{USAGE}");
        return ExitCode::from(2);
    }
    let Some(path) = ai::locate(index_path.as_deref()) else {
        eprintln!("no {} here or in any folder above; build one with `ifsdep-ai init --core <dir> --cust <dir>`, or pass --index", ai::FILE_NAME);
        return ExitCode::from(2);
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("could not read {}: {e}", path.display());
            return ExitCode::FAILURE;
        }
    };
    let ix = match ai::parse(&text) {
        Ok(ix) => ix,
        Err(e) => {
            eprintln!("{}: {e}", path.display());
            return ExitCode::FAILURE;
        }
    };
    drop(text);
    let name = name.unwrap_or_default();
    let result = match cmd {
        "find" => ix.find(&name, &opts),
        "callers" => ix.links(&name, true, &opts),
        "callees" => ix.links(&name, false, &opts),
        "impact" => ix.impact(&name, &opts),
        "unit" => ix.unit(&name, &opts),
        "trace" => ix.trace(&name, &opts),
        "findings" => ix.findings_list(file.as_deref(), unit.as_deref(), rule.as_deref(), cust, &opts),
        _ => Ok(ix.status()),
    };
    match result {
        Ok(s) => {
            print!("{s}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprint!("{e}");
            if !e.ends_with('\n') {
                eprintln!();
            }
            ExitCode::FAILURE
        }
    }
}

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let cmd = args.next().unwrap_or_default();
    match cmd.as_str() {
        "init" => init(args.collect()),
        c if QUERIES.contains(&c) => query(c, args.collect()),
        "-h" | "--help" | "help" => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
    }
}
