//! Upgrade comparison: given the previous Core (a full older checkout, or the Impact
//! Analyzer's `Base` folder with just the files it flagged), says what changed under each
//! Cust customization:
//!
//! - BASE_CHANGED: the Core declaration an `@Override` / `@Overtake` layers over changed.
//!   The finding carries the diff of just that declaration. A changed signature is an
//!   error (the override no longer matches), a changed body under an `@Overtake` a warning
//!   (your copy replaces IFS's new code), under an `@Override` info.
//! - ENTITY_CHANGED: the Core `.entity` a `-Cust.entity` layers over gained, lost or changed
//!   attributes. A new Core attribute with the same name as a Cust one is a warning.
//!
//! Previous files are read by the same relative path as the current Core file.

use crate::db;
use crate::entity;
use crate::model::*;
use crate::plsql;
use crate::sqlscope;
use std::collections::HashMap;
use std::path::Path;

/// How many lines of context each hunk keeps around a change.
const CONTEXT: usize = 2;
/// Bigger declarations are reported as changed without a line diff.
const MAX_DIFF_CELLS: usize = 4_000_000;

pub fn run(idx: &Index, prev: &Path, out: &mut Vec<Finding>, notes: &mut Vec<String>) {
    let mut cache: HashMap<String, Option<String>> = HashMap::new();
    let mut read_prev = |rel: &str| -> Option<String> {
        cache
            .entry(rel.to_string())
            .or_insert_with(|| std::fs::read(prev.join(rel)).ok().map(|b| String::from_utf8_lossy(&b).into_owned()))
            .clone()
    };
    let mut now_cache: HashMap<usize, String> = HashMap::new();
    let mut read_now = |fi: usize| -> String {
        now_cache
            .entry(fi)
            .or_insert_with(|| std::fs::read(&idx.files[fi].abs_path).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default())
            .clone()
    };
    let (mut compared, mut no_prev, mut changed) = (0usize, 0usize, 0usize);

    // Declarations: each Cust override and the Core base it layers over.
    for e in idx.edges.iter().filter(|e| e.kind == EdgeKind::Overrides) {
        let (Some(ov), Some(base)) = (object(idx, &e.from), object(idx, &e.to)) else { continue };
        if idx.files[ov.file].root != Layer::Cust || idx.files[base.file].root != Layer::Core {
            continue;
        }
        let base_file = &idx.files[base.file];
        let Some(old_text) = read_prev(&base_file.rel_path) else {
            no_prev += 1;
            continue;
        };
        let now_text = read_now(base.file);
        // The same occurrence in both versions (overloads, and uses of the name in a page).
        let nth = occurrence(&now_text, base_file.kind, &base.kind, &base.name, base.line);
        let Some(now) = decl_span(&now_text, base_file.kind, &base.kind, &base.name, nth) else { continue };
        compared += 1;
        let Some(old) = decl_span(&old_text, base_file.kind, &base.kind, &base.name, nth) else {
            // In the current Core but not the previous one: new in this version.
            continue;
        };
        let (old_lines, now_lines) = (lines(&old_text, old), lines(&now_text, now));
        if normalized(&old_lines) == normalized(&now_lines) {
            continue;
        }
        changed += 1;
        let (old_sig, now_sig) = (signature(&old_lines, base_file.kind), signature(&now_lines, base_file.kind));
        let overtake = ov.modifier.as_deref() == Some("Overtake");
        // When the signature changed, compare yours with both: you may have updated it already.
        let ov_text = read_now(ov.file);
        let ov_nth = occurrence(&ov_text, idx.files[ov.file].kind, &ov.kind, &ov.name, ov.line);
        let ov_sig = decl_span(&ov_text, idx.files[ov.file].kind, &ov.kind, &ov.name, ov_nth).map(|s| signature(&lines(&ov_text, s), base_file.kind));
        let (severity, what) = if old_sig != now_sig {
            if ov_sig.as_deref() == Some(now_sig.as_str()) {
                (Severity::Info, "its signature changed, and your declaration already has the new one")
            } else if ov_sig.as_deref() == Some(old_sig.as_str()) {
                (Severity::Error, "its signature changed and your declaration still has the old one, so it no longer matches")
            } else {
                (Severity::Warning, "its signature changed, and yours matches neither the old nor the new one; check it")
            }
        } else if overtake {
            (Severity::Warning, "your @Overtake replaces it with your copy, so IFS's change is lost unless you bring it over")
        } else {
            (Severity::Info, "your @Override still layers over it; check the change doesn't conflict with what you add")
        };
        let base_name = base_file.rel_path.rsplit('/').next().unwrap_or("");
        let ov_name = idx.files[ov.file].rel_path.rsplit('/').next().unwrap_or("");
        let mut diff = diff(&old_lines, &now_lines, old.0, now.0);
        let (added, removed) = count(&diff);
        let ov_span = decl_span(&ov_text, idx.files[ov.file].kind, &ov.kind, &ov.name, ov_nth);
        let ov_lines = ov_span.map_or(Vec::new(), |s| lines(&ov_text, s));
        // An @Overtake is a copy of the base: line by line, which of IFS's changes does it have?
        let (mut severity, mut what, mut yours) = (severity, what.to_string(), String::new());
        if overtake && old_sig == now_sig {
            let c = against_copy(&mut diff, &ov_lines);
            yours = c.summary();
            if c.up_to_date() {
                severity = Severity::Info;
                what = "your @Overtake copy already has IFS's change".into();
            }
        }
        let mut f = finding(
            severity,
            "BASE_CHANGED",
            format!("{} {} in {base_name} changed under your @{}", base.kind, base.name, ov.modifier.as_deref().unwrap_or("Override")),
            format!(
                "IFS changed {} {} in {base_name} since the previous Core ({removed} line{} removed, {added} added); {what}.{yours} \
                 Your @{} is in {ov_name}:{}.",
                base.kind,
                base.name,
                if removed == 1 { "" } else { "s" },
                ov.modifier.as_deref().unwrap_or("Override"),
                ov.line
            ),
        );
        f.location = Some(Location { file: ov.file, line: ov.line });
        f.nodes = vec![ov.id.clone(), base.id.clone()];
        f.diff = Some(Diff {
            old_path: prev.join(&base_file.rel_path).display().to_string(),
            old_line: old.0 + 1,
            new_file: base.file,
            new_line: now.0 + 1,
            lines: diff,
            cust_file: Some(ov.file),
            cust_line: ov_span.map_or(ov.line, |s| s.0 + 1),
            cust_code: excerpt(&ov_lines),
        });
        out.push(f);
    }

    // Views: a Cust @Override / @Overtake VIEW over a Core view.
    for r in &idx.relations {
        let Some(cust) = r.defs.iter().find(|d| idx.files[d.file].root == Layer::Cust && d.how.starts_with("view (@")) else { continue };
        let Some(core) = r.defs.iter().find(|d| idx.files[d.file].root == Layer::Core && d.how.starts_with("view")) else { continue };
        let core_file = &idx.files[core.file];
        let Some(old_text) = read_prev(&core_file.rel_path) else {
            no_prev += 1;
            continue;
        };
        let now_text = read_now(core.file);
        let (Some(now), Some(old)) = (view_span(&now_text, &r.name), view_span(&old_text, &r.name)) else { continue };
        compared += 1;
        let (old_lines, now_lines) = (lines(&old_text, old), lines(&now_text, now));
        if normalized(&old_lines) == normalized(&now_lines) {
            continue;
        }
        changed += 1;
        let overtake = cust.how.contains("Overtake");
        let modifier = if overtake { "Overtake" } else { "Override" };
        let mut diff = diff(&old_lines, &now_lines, old.0, now.0);
        let (added, removed) = count(&diff);
        let cust_text = read_now(cust.file);
        let cust_span = view_span(&cust_text, &r.name);
        let cust_lines = cust_span.map_or(Vec::new(), |s| lines(&cust_text, s));
        let (mut severity, mut what, mut yours) = if overtake {
            (Severity::Warning, "your @Overtake replaces it with your copy, so IFS's change is lost unless you bring it over".to_string(), String::new())
        } else {
            (Severity::Info, "check your override still fits".to_string(), String::new())
        };
        if overtake {
            let c = against_copy(&mut diff, &cust_lines);
            yours = c.summary();
            if c.up_to_date() {
                severity = Severity::Info;
                what = "your @Overtake copy already has IFS's change".into();
            }
        }
        let mut f = finding(
            severity,
            "BASE_CHANGED",
            format!("view {} changed under your @{modifier}", r.name),
            format!(
                "IFS changed view {} in {} since the previous Core ({removed} line{} removed, {added} added); {what}.{yours} Your @{modifier} VIEW is in {}:{}.",
                r.name,
                core_file.rel_path.rsplit('/').next().unwrap_or(""),
                if removed == 1 { "" } else { "s" },
                idx.files[cust.file].rel_path.rsplit('/').next().unwrap_or(""),
                cust.line
            ),
        );
        f.location = Some(Location { file: cust.file, line: cust.line });
        f.nodes = vec![r.id.clone()];
        f.diff = Some(Diff {
            old_path: prev.join(&core_file.rel_path).display().to_string(),
            old_line: old.0 + 1,
            new_file: core.file,
            new_line: now.0 + 1,
            lines: diff,
            cust_file: Some(cust.file),
            cust_line: cust_span.map_or(cust.line, |s| s.0 + 1),
            cust_code: excerpt(&cust_lines),
        });
        out.push(f);
    }

    // Entities: a -Cust.entity over a Core .entity whose attributes changed.
    let mut core_entity: HashMap<String, usize> = HashMap::new();
    for (fi, f) in idx.files.iter().enumerate() {
        if f.kind == FileKind::Entity && f.root == Layer::Core {
            core_entity.insert(stem(&f.rel_path), fi);
        }
    }
    for (cfi, cf) in idx.files.iter().enumerate() {
        if cf.kind != FileKind::Entity || cf.root != Layer::Cust {
            continue;
        }
        let Some(&fi) = core_entity.get(&stem(&cf.rel_path)) else { continue };
        let Some(old_text) = read_prev(&idx.files[fi].rel_path) else {
            no_prev += 1;
            continue;
        };
        compared += 1;
        let now_text = read_now(fi);
        let (old, now, cust) = (entity::parse(&old_text), entity::parse(&now_text), entity::parse(&read_now(cfi)));
        let key = |a: &entity::Attribute| a.name.to_ascii_lowercase();
        let old_attrs: HashMap<String, &entity::Attribute> = old.attributes.iter().map(|a| (key(a), a)).collect();
        let now_attrs: HashMap<String, &entity::Attribute> = now.attributes.iter().map(|a| (key(a), a)).collect();
        let old_lines: Vec<&str> = old_text.lines().collect();
        let now_lines: Vec<&str> = now_text.lines().collect();
        let mut lines_out: Vec<DiffLine> = Vec::new();
        let (mut added, mut removed, mut modified) = (Vec::new(), Vec::new(), Vec::new());
        for a in &now.attributes {
            match old_attrs.get(&key(a)) {
                None => {
                    added.push(a.name.clone());
                    lines_out.push(DiffLine { op: "+".into(), text: describe(a), old: None, new: Some(a.line), in_cust: None });
                }
                Some(o) if normalized(&attribute_block(&old_lines, o.line)) != normalized(&attribute_block(&now_lines, a.line)) => {
                    modified.push(a.name.clone());
                    lines_out.push(DiffLine { op: "~".into(), text: format!("{} (properties changed)", describe(a)), old: Some(o.line), new: Some(a.line), in_cust: None });
                }
                _ => {}
            }
        }
        for a in &old.attributes {
            if !now_attrs.contains_key(&key(a)) {
                removed.push(a.name.clone());
                lines_out.push(DiffLine { op: "-".into(), text: describe(a), old: Some(a.line), new: None, in_cust: None });
            }
        }
        if lines_out.is_empty() {
            continue;
        }
        changed += 1;
        // A Core attribute that is new in this version and also declared in the Cust entity.
        let clash: Vec<&entity::Attribute> = cust.attributes.iter().filter(|c| added.iter().any(|a| a.eq_ignore_ascii_case(&c.name))).collect();
        let list = |v: &[String]| if v.is_empty() { "none".to_string() } else { v.join(", ") };
        let mut f = finding(
            if clash.is_empty() { Severity::Info } else { Severity::Warning },
            "ENTITY_CHANGED",
            format!("{} changed under your -Cust.entity", stem(&idx.files[fi].rel_path)),
            format!(
                "IFS changed the attributes of {} since the previous Core. Added: {}. Removed: {}. Changed: {}.{}",
                idx.files[fi].rel_path.rsplit('/').next().unwrap_or(""),
                list(&added),
                list(&removed),
                list(&modified),
                if clash.is_empty() {
                    String::new()
                } else {
                    format!(
                        " Your {} also declares {}, which Core now has too.",
                        cf.rel_path.rsplit('/').next().unwrap_or(""),
                        clash.iter().map(|c| c.name.as_str()).collect::<Vec<_>>().join(", ")
                    )
                }
            ),
        );
        f.location = Some(Location { file: cfi, line: clash.first().map_or(1, |c| c.line) });
        f.diff = Some(Diff {
            old_path: prev.join(&idx.files[fi].rel_path).display().to_string(),
            old_line: 1,
            new_file: fi,
            new_line: 1,
            lines: lines_out,
            cust_file: Some(cfi),
            cust_line: 1,
            cust_code: cust.attributes.iter().map(describe).collect(),
        });
        out.push(f);
    }

    // Column conflicts the upgrade brings: the query was fine before because IFS only now
    // reads the relation with the Cust column there (or the query itself is new).
    for f in out.iter_mut().filter(|f| f.rule == "COLUMN_AMBIGUOUS") {
        let (Some(loc), [r, o, ..]) = (f.location.clone(), f.nodes.as_slice()) else { continue };
        if !r.starts_with('r') || !o.starts_with('r') {
            continue;
        }
        let (rel, other) = (idx.relations[id_index(r)].name.clone(), idx.relations[id_index(o)].name.clone());
        let col = f.title.split('`').nth(1).unwrap_or("").to_ascii_uppercase();
        let file = &idx.files[loc.file];
        // Without the old file nothing is known: the previous Core may be a partial copy.
        let Some(old_text) = read_prev(&file.rel_path) else { continue };
        let now_text = read_now(loc.file);
        let reads = |b: &sqlscope::Block, name: &str| b.from.iter().any(|i| i.name == name);
        let Some(now) = query_span(&now_text, |b| reads(b, &rel) && reads(b, &other) && b.refs.iter().any(|(c, l)| *c == col && *l == loc.line)) else {
            continue;
        };
        let old_reads_rel = query_span(&old_text, |b| reads(b, &other) && reads(b, &rel) && b.refs.iter().any(|(c, _)| *c == col)).is_some();
        if old_reads_rel {
            continue; // The same query, reading both, was there before the upgrade.
        }
        let old = query_span(&old_text, |b| reads(b, &other) && b.refs.iter().any(|(c, _)| *c == col));
        let now_lines = lines(&now_text, now);
        let (old_lines, old_start) = old.map_or((Vec::new(), 0), |s| (lines(&old_text, s), s.0));
        let cust = f.evidence.first().and_then(|t| t.steps.first()).map(|s| (s.file, s.line, s.text.clone()));
        f.upgrade = true;
        f.message.push_str(&match old {
            Some(_) => format!(" This is new with the upgrade: in the previous Core this query didn't read {rel}; IFS added it."),
            None => " This is new with the upgrade: the query isn't in the previous Core.".to_string(),
        });
        f.diff = Some(Diff {
            old_path: prev.join(&file.rel_path).display().to_string(),
            old_line: old_start + 1,
            new_file: loc.file,
            new_line: now.0 + 1,
            lines: diff(&old_lines, &now_lines, old_start, now.0),
            cust_file: cust.as_ref().map(|c| c.0),
            cust_line: cust.as_ref().map_or(1, |c| c.1),
            cust_code: cust.map(|c| vec![c.2]).unwrap_or_default(),
        });
    }
    notes.push(format!(
        "Upgrade comparison with {}: {compared} Core base{} compared, {changed} changed, {no_prev} with no previous copy.",
        prev.display(),
        if compared == 1 { "" } else { "s" }
    ));
}

/// First and last line (0-based) of the statement holding the first query block that
/// `pick` accepts: from its SELECT to the `;` after its FROM list.
fn query_span(text: &str, pick: impl Fn(&sqlscope::Block) -> bool) -> Option<(usize, usize)> {
    let prepared = sqlscope::prepare(text);
    let b = sqlscope::blocks(&prepared).into_iter().find(|b| pick(b))?;
    let from_end = b.from.iter().map(|f| f.line).max().unwrap_or(b.select_line);
    let ls: Vec<&str> = prepared.lines().collect();
    let end = (from_end - 1..ls.len()).find(|&i| ls[i].contains(';')).unwrap_or(ls.len() - 1);
    Some((b.select_line - 1, end))
}

/// Longest customization shown in full; longer ones are cut with a note.
const MAX_CUST_LINES: usize = 300;

fn excerpt(lines: &[&str]) -> Vec<String> {
    let mut out: Vec<String> = lines.iter().take(MAX_CUST_LINES).map(|l| l.trim_end().to_string()).collect();
    if lines.len() > MAX_CUST_LINES {
        out.push(format!("… {} more lines", lines.len() - MAX_CUST_LINES));
    }
    out
}

/// How an `@Overtake` copy compares with IFS's change, line by line.
struct CopyCheck {
    added: usize,
    have_added: usize,
    removed: usize,
    still_removed: usize,
}

impl CopyCheck {
    /// Your copy has every line IFS added and none it removed.
    fn up_to_date(&self) -> bool {
        self.have_added == self.added && self.still_removed == 0
    }
    fn summary(&self) -> String {
        let mut parts = Vec::new();
        if self.added > 0 {
            parts.push(format!("has {} of the {} line{} IFS added", self.have_added, self.added, if self.added == 1 { "" } else { "s" }));
        }
        if self.removed > 0 {
            parts.push(format!("still has {} of the {} IFS removed", self.still_removed, self.removed));
        }
        if parts.is_empty() { String::new() } else { format!(" Your copy {}.", parts.join(" and ")) }
    }
}

/// Marks each added or removed Core line by whether your copy has it (ignoring layout and
/// comments). Lines without a word (blank, comment-only, `}`) aren't counted.
fn against_copy(diff: &mut [DiffLine], copy: &[&str]) -> CopyCheck {
    let yours: std::collections::HashSet<String> = copy.iter().map(|l| normalized(&[l])).filter(|l| !l.is_empty()).collect();
    let mut c = CopyCheck { added: 0, have_added: 0, removed: 0, still_removed: 0 };
    for l in diff.iter_mut().filter(|l| l.op == "+" || l.op == "-") {
        let n = normalized(&[l.text.as_str()]);
        // `}` or `);` matches any copy: only lines with a word in them count.
        if !n.chars().any(|c| c.is_alphanumeric()) {
            continue;
        }
        let has = yours.contains(&n);
        l.in_cust = Some(has);
        if l.op == "+" {
            c.added += 1;
            c.have_added += has as usize;
        } else {
            c.removed += 1;
            c.still_removed += has as usize;
        }
    }
    c
}

fn object<'a>(idx: &'a Index, id: &str) -> Option<&'a Object> {
    id.starts_with('o').then(|| &idx.objects[id_index(id)])
}

fn stem(rel: &str) -> String {
    let name = rel.rsplit('/').next().unwrap_or(rel);
    name.split('.').next().unwrap_or(name).trim_end_matches("-Cust").to_string()
}

fn finding(severity: Severity, rule: &str, title: String, message: String) -> Finding {
    Finding {
        id: String::new(),
        severity,
        rule: rule.into(),
        title,
        message,
        nodes: vec![],
        edges: vec![],
        location: None,
        suggestion: None,
        evidence: vec![],
        involves_cust: true,
        diff: None,
        upgrade: true,
    }
}

fn lines(text: &str, span: (usize, usize)) -> Vec<&str> {
    text.lines().skip(span.0).take(span.1 - span.0 + 1).collect()
}

/// The code without comments or layout, to tell a real change from a reformatted one.
/// String literals are kept: a changed message is a change.
fn normalized(lines: &[&str]) -> String {
    // Whitespace only matters between two words: `NOT(x)` and `NOT (x)` are the same code.
    let code = db::blank_comments(&lines.join("\n")).to_ascii_lowercase();
    let word = |c: char| c.is_alphanumeric() || c == '_' || c == '$' || c == '#';
    let mut out = String::with_capacity(code.len());
    let mut space = false;
    for c in code.chars() {
        if c.is_whitespace() {
            space = true;
            continue;
        }
        if space && out.chars().last().is_some_and(word) && word(c) {
            out.push(' ');
        }
        space = false;
        out.push(c);
    }
    out
}

/// The declaration's header: up to the PL/SQL `IS` / `AS`, or a Marble declaration's first line.
fn signature(lines: &[&str], kind: FileKind) -> String {
    let n = normalized(lines);
    if matches!(kind, FileKind::Plsql | FileKind::Plsvc) {
        // Up to the first whole word IS or AS (normalized text may read `rec)is`).
        let b = n.as_bytes();
        let word = |i: usize| b.get(i).is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_' || *c == b'$' || *c == b'#');
        let end = (0..b.len().saturating_sub(1))
            .find(|&i| (&n[i..i + 2] == "is" || &n[i..i + 2] == "as") && (i == 0 || !word(i - 1)) && !word(i + 2))
            .unwrap_or(n.len());
        n[..end].to_string()
    } else {
        normalized(&lines[..1.min(lines.len())])
    }
}

/// Lines (0-based) that start `kind name`: declarations, and in Marble also uses of the
/// name inside other constructs (`list OrdersList { ... }` in a page).
fn decl_starts(text: &str, file_kind: FileKind, kind: &str, name: &str) -> Vec<usize> {
    let plsql = matches!(file_kind, FileKind::Plsql | FileKind::Plsvc);
    text.lines()
        .enumerate()
        .filter(|(_, l)| strip_word(if plsql { l } else { l.trim_start() }, kind).is_some_and(|rest| names(rest.trim_start(), name)))
        .map(|(i, _)| i)
        .collect()
}

/// Which of those starts the declaration on `line` (1-based) is: the same occurrence is
/// then looked up in the other version.
fn occurrence(text: &str, file_kind: FileKind, kind: &str, name: &str, line: usize) -> usize {
    decl_starts(text, file_kind, kind, name).iter().filter(|&&s| s + 1 < line).count()
}

/// First and last line (0-based) of the nth occurrence of `kind name`.
fn decl_span(text: &str, file_kind: FileKind, kind: &str, name: &str, nth: usize) -> Option<(usize, usize)> {
    let ls: Vec<&str> = text.lines().collect();
    let plsql = matches!(file_kind, FileKind::Plsql | FileKind::Plsvc);
    let start = *decl_starts(text, file_kind, kind, name).get(nth)?;
    if plsql {
        let end_tag = format!("END {name}");
        let end = (start..ls.len())
            .find(|&i| strip_ci(ls[i].trim_start(), &end_tag).is_some())
            .or_else(|| (start + 1..ls.len()).find(|&i| strip_word(ls[i], "PROCEDURE").or(strip_word(ls[i], "FUNCTION")).is_some()).map(|i| i - 1))
            .unwrap_or(ls.len() - 1);
        return Some((start, end));
    }
    // Marble: a braced body, or up to the `;` when there is none.
    let cleaned = plsql::blank_comments_and_strings(&ls[start..].join("\n"));
    let mut depth = 0;
    for (k, l) in cleaned.lines().enumerate() {
        for c in l.chars() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some((start, start + k));
                    }
                }
                ';' if depth == 0 => return Some((start, start + k)),
                _ => {}
            }
        }
    }
    Some((start, ls.len() - 1))
}

/// `VIEW name IS ... ;`
fn view_span(text: &str, name: &str) -> Option<(usize, usize)> {
    let ls: Vec<&str> = text.lines().collect();
    let start = ls.iter().position(|l| {
        strip_word(l.trim_start(), "VIEW").is_some_and(|r| names(r.trim_start(), name))
    })?;
    let cleaned = plsql::blank_comments_and_strings(&ls[start..].join("\n"));
    let end = cleaned.lines().position(|l| l.contains(';')).map_or(ls.len() - 1, |k| start + k);
    Some((start, end))
}

/// `prefix` at the start of `s`, ignoring ASCII case: the rest. Never slices inside a
/// multi-byte character.
fn strip_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    s.get(..prefix.len()).filter(|head| head.eq_ignore_ascii_case(prefix)).map(|_| &s[prefix.len()..])
}

/// `word` at the start of `s` followed by whitespace: the rest.
fn strip_word<'a>(s: &'a str, word: &str) -> Option<&'a str> {
    strip_ci(s, word).filter(|rest| rest.starts_with(char::is_whitespace))
}

/// Whether `s` starts with the whole name `name` (not a longer one).
fn names(s: &str, name: &str) -> bool {
    strip_ci(s, name).is_some_and(|rest| rest.chars().next().is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_')))
}

/// The lines of the `<ATTRIBUTE>` element whose `<NAME>` is on `name_line` (1-based).
fn attribute_block<'a>(ls: &[&'a str], name_line: usize) -> Vec<&'a str> {
    let at = name_line.saturating_sub(1).min(ls.len().saturating_sub(1));
    let start = (0..=at).rev().find(|&i| ls[i].contains("<ATTRIBUTE>")).unwrap_or(at);
    let end = (at..ls.len()).find(|&i| ls[i].contains("</ATTRIBUTE>")).unwrap_or(at);
    ls[start..=end].to_vec()
}

fn describe(a: &entity::Attribute) -> String {
    format!("attribute {}{}", a.name, if a.derived { " (derived)" } else { "" })
}

fn count(d: &[DiffLine]) -> (usize, usize) {
    (d.iter().filter(|l| l.op == "+").count(), d.iter().filter(|l| l.op == "-").count())
}

/// Line diff (longest common subsequence) cut into hunks with a little context. `old0` and
/// `new0` are the 0-based file lines the slices start at, for the line numbers.
fn diff(old: &[&str], new: &[&str], old0: usize, new0: usize) -> Vec<DiffLine> {
    let (n, m) = (old.len(), new.len());
    if n * m > MAX_DIFF_CELLS {
        return vec![DiffLine { op: "@".into(), text: format!("{n} lines before, {m} after: too big to compare line by line"), old: None, new: None, in_cust: None }];
    }
    let eq = |a: &str, b: &str| a.trim_end() == b.trim_end();
    // lcs[i][j]: common length of old[i..] and new[j..].
    let mut lcs = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if eq(old[i], new[j]) { lcs[i + 1][j + 1] + 1 } else { lcs[i + 1][j].max(lcs[i][j + 1]) };
        }
    }
    let mut all: Vec<DiffLine> = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < n || j < m {
        if i < n && j < m && eq(old[i], new[j]) {
            all.push(DiffLine { op: " ".into(), text: new[j].trim_end().into(), old: Some(old0 + i + 1), new: Some(new0 + j + 1), in_cust: None });
            i += 1;
            j += 1;
        } else if j < m && (i == n || lcs[i][j + 1] > lcs[i + 1][j]) {
            all.push(DiffLine { op: "+".into(), text: new[j].trim_end().into(), old: None, new: Some(new0 + j + 1), in_cust: None });
            j += 1;
        } else {
            all.push(DiffLine { op: "-".into(), text: old[i].trim_end().into(), old: Some(old0 + i + 1), new: None, in_cust: None });
            i += 1;
        }
    }
    // Keep changes and CONTEXT lines around them; mark the gaps.
    let keep: Vec<bool> = (0..all.len())
        .map(|k| (k.saturating_sub(CONTEXT)..(k + CONTEXT + 1).min(all.len())).any(|x| all[x].op != " "))
        .collect();
    let mut out = Vec::new();
    let mut gap = false;
    for (k, l) in all.into_iter().enumerate() {
        if keep[k] {
            if gap && !out.is_empty() {
                out.push(DiffLine { op: "@".into(), text: "…".into(), old: None, new: None, in_cust: None });
            }
            gap = false;
            out.push(l);
        } else {
            gap = true;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_declarations_and_diffs_them() {
        let old = "PROCEDURE Insert___ (\n   newrec_ IN OUT rec )\nIS\nBEGIN\n   -- Bug 1\n   newrec_.line_no := seq.nextval;\nEND Insert___;\n";
        let new = "PROCEDURE Insert___ (\n   newrec_ IN OUT rec )\nIS\nBEGIN\n   IF sync_ THEN\n      newrec_.line_no := seq.nextval;\n   END IF;\nEND Insert___;\n";
        let (o, n) = (decl_span(old, FileKind::Plsql, "procedure", "Insert___", 0).unwrap(), decl_span(new, FileKind::Plsql, "procedure", "Insert___", 0).unwrap());
        assert_eq!((o, n), ((0, 6), (0, 7)));
        let (ol, nl) = (lines(old, o), lines(new, n));
        assert_eq!(signature(&ol, FileKind::Plsql), signature(&nl, FileKind::Plsql));
        let d = diff(&ol, &nl, o.0, n.0);
        let ops: String = d.iter().map(|l| l.op.as_str()).collect();
        assert_eq!(ops, "  --+++ ", "{d:?}");
        assert_eq!(d.iter().find(|l| l.op == "+").unwrap().new, Some(5));
        // Comments and layout alone are not a change.
        assert_eq!(normalized(&["x := 1; -- why", "", "y := 2;"]), normalized(&["x := 1;", "y   := 2;"]));
        assert_eq!(normalized(&["IF NOT(a = TRUE) THEN"]), normalized(&["   IF NOT (a=TRUE)  THEN"]));
        assert_ne!(normalized(&["x := 'a b';"]), normalized(&["x := 'ab';"]), "a changed literal is a change");
        assert_ne!(normalized(&["RETURN a b"]), normalized(&["RETURN ab"]));
        // Marble: a braced body, and a one-line declaration.
        let client = "list OrdersList for Order {\n   field OrderNo;\n}\ncommand Go for Order;\n";
        assert_eq!(decl_span(client, FileKind::Client, "list", "OrdersList", 0), Some((0, 2)));
        assert_eq!(decl_span(client, FileKind::Client, "command", "Go", 0), Some((3, 3)));
        assert_eq!(decl_span(client, FileKind::Client, "list", "Orders", 0), None, "a longer name is not a match");
        // An @Overtake copy that took one of IFS's two new lines and dropped the removed one.
        let copy = ["BEGIN", "   IF sync_ THEN", "  newrec_.line_no  :=  seq.nextval;", "END Insert___;"];
        let mut d2 = d.clone();
        let c = against_copy(&mut d2, &copy);
        assert_eq!((c.added, c.have_added, c.removed, c.still_removed), (3, 2, 1, 1), "{d2:?}");
        assert!(!c.up_to_date());
        assert_eq!(c.summary(), " Your copy has 2 of the 3 lines IFS added and still has 1 of the 1 IFS removed.");
        assert_eq!(d2.iter().find(|l| l.text.contains("END IF")).unwrap().in_cust, Some(false));
        // Non-ASCII text next to where a name would be is skipped, not sliced through.
        assert_eq!(decl_span("list Or‑ders for X;\nlist OrdersList;\n", FileKind::Client, "list", "OrdersList", 0), Some((1, 1)));
    }
}
