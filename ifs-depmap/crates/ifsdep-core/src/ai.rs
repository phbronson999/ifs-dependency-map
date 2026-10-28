//! The AI index: a compact text copy of the index, written only when asked
//! (`ifsdep-ai init`), and the queries an AI coding assistant runs against it
//! (`ifsdep-ai find | callers | callees | impact | unit | findings | trace | status`).
//! Answers are short plain text with absolute `path:line`s, sized to fit in a prompt;
//! the full JSON is far too big to read.
//!
//! One record per line, tab-separated, with `\`, tab and newline escaped. Ids are the
//! index's own (`u12` is `units[12]`, `o40` is `objects[40]`, `r3` is `relations[3]`).

use crate::model::*;
use crate::stamp::{self, Stamp};
use crate::trace::{self, TraceStep};
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::path::Path;

pub const HEADER: &str = "ifsdep ai index 1";
/// The file name `ifsdep-ai init` writes and the queries look for.
pub const FILE_NAME: &str = "ifsdep-ai.idx";

// ------------------------------------------------------------------ writing

fn esc(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\r' => {}
            c => out.push(c),
        }
    }
}

fn unesc(s: &str) -> String {
    if !s.contains('\\') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next() {
            Some('t') => out.push('\t'),
            Some('n') => out.push('\n'),
            Some(c) => out.push(c),
            None => {}
        }
    }
    out
}

fn rec(out: &mut String, fields: &[&str]) {
    for (i, f) in fields.iter().enumerate() {
        if i > 0 {
            out.push('\t');
        }
        esc(f, out);
    }
    out.push('\n');
}

fn flag(b: bool) -> &'static str {
    if b {
        "1"
    } else {
        ""
    }
}

/// The AI index for `idx`. `prev_core` is the previous Core folder it was indexed with, if
/// any; `stamps` the files to check later for changes (`status`); `built` milliseconds since 1970.
pub fn write(idx: &Index, prev_core: Option<&str>, stamps: &[Stamp], built: u64) -> String {
    let mut out = String::with_capacity(64 << 20);
    out.push_str(HEADER);
    out.push('\n');
    rec(&mut out, &["built", &built.to_string()]);
    for (name, root) in [("Core", idx.core_root.as_deref()), ("Cust", idx.cust_root.as_deref()), ("Prev", prev_core)] {
        if let Some(r) = root {
            rec(&mut out, &["root", name, r]);
        }
    }
    for f in &idx.files {
        rec(&mut out, &["F", f.root.as_str(), f.kind.as_str(), &f.component, &f.rel_path, &f.abs_path]);
    }
    for u in &idx.units {
        let files: Vec<String> = u.files.iter().map(usize::to_string).collect();
        rec(&mut out, &["U", u.kind.as_str(), &u.name, &u.component, u.origin.as_str(), flag(u.has_overlay), &files.join(",")]);
    }
    for o in &idx.objects {
        rec(
            &mut out,
            &[
                "O",
                &o.unit,
                &o.file.to_string(),
                &o.kind,
                &o.name,
                &o.line.to_string(),
                o.modifier.as_deref().unwrap_or(""),
                o.base.as_deref().unwrap_or(""),
                flag(o.generated),
            ],
        );
    }
    for r in &idx.relations {
        let defs: Vec<String> = r.defs.iter().map(|d| format!("{}:{}:{}", d.file, d.line, d.how)).collect();
        rec(&mut out, &["R", &r.name, &r.kind, &defs.join("|")]);
    }
    for e in &idx.edges {
        rec(
            &mut out,
            &[
                "E",
                &e.from,
                &e.to,
                e.kind.as_str(),
                &e.file.to_string(),
                &e.line.to_string(),
                e.layer.as_str(),
                e.dynamic.as_deref().unwrap_or(""),
                e.detail.as_deref().unwrap_or(""),
                &e.count.to_string(),
            ],
        );
    }
    for f in &idx.findings {
        let (file, line) = f.location.as_ref().map_or((String::new(), String::new()), |l| (l.file.to_string(), l.line.to_string()));
        let s = f.suggestion.as_ref();
        rec(
            &mut out,
            &[
                "X",
                f.severity.as_str(),
                &f.rule,
                &file,
                &line,
                flag(f.involves_cust),
                flag(f.upgrade),
                &f.nodes.join(","),
                &f.title,
                &f.message,
                s.map_or("", |s| s.summary.as_str()),
                s.and_then(|s| s.target.as_deref()).unwrap_or(""),
                s.and_then(|s| s.snippet.as_deref()).unwrap_or(""),
            ],
        );
    }
    for s in stamps {
        rec(&mut out, &["S", &s.size.to_string(), &s.modified.to_string(), &s.path]);
    }
    out
}

/// What `init` wrote.
pub struct Written {
    pub index: std::path::PathBuf,
    pub index_bytes: usize,
    pub skill: std::path::PathBuf,
}

/// Writes the AI index and the `ifs-codebase` skill into `workspace` (the folder Claude Code
/// is started in): `<workspace>/ifsdep-ai.idx` and `<workspace>/.claude/skills/ifs-codebase/SKILL.md`.
/// `ifsdep` is the CLI the skill tells Claude to run. Refuses a workspace inside an IFS folder,
/// since those are read-only input.
pub fn init(idx: &Index, prev_core: Option<&Path>, workspace: &Path, ifsdep: &Path) -> Result<Written, String> {
    let low = |p: &Path| p.display().to_string().replace('/', "\\").trim_end_matches('\\').to_ascii_lowercase();
    let ws = low(workspace);
    let roots: Vec<(&str, String)> = [("Core", idx.core_root.clone()), ("Cust", idx.cust_root.clone()), ("Previous Core", prev_core.map(|p| p.display().to_string()))]
        .into_iter()
        .filter_map(|(n, r)| r.map(|r| (n, r)))
        .collect();
    for (_, r) in &roots {
        let r = low(Path::new(r));
        if ws == r || ws.starts_with(&format!("{r}\\")) {
            return Err(format!("{} is inside the IFS folder {r}, which is never written to; pick a folder outside it", workspace.display()));
        }
    }
    let mut stamps = idx.stamps.clone();
    if let Some(p) = prev_core {
        stamps.extend(stamp::stamps(&[p]));
    }
    let built = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
    let text = write(idx, prev_core.map(|p| p.display().to_string()).as_deref(), &stamps, built);
    let index = workspace.join(FILE_NAME);
    std::fs::write(&index, &text).map_err(|e| format!("could not write {}: {e}", index.display()))?;
    let skill_path = workspace.join(".claude").join("skills").join("ifs-codebase").join("SKILL.md");
    let roots: Vec<(&str, &str)> = roots.iter().map(|(n, r)| (*n, r.as_str())).collect();
    let body = skill(&ifsdep.display().to_string(), &index.display().to_string(), &roots, built);
    skill_path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|_| std::fs::write(&skill_path, body))
        .map_err(|e| format!("could not write {}: {e}", skill_path.display()))?;
    Ok(Written { index, index_bytes: text.len(), skill: skill_path })
}

/// The skill `ifsdep-ai init` installs, with this machine's paths filled in.
pub fn skill(ifsdep: &str, index: &str, roots: &[(&str, &str)], built: u64) -> String {
    let roots: Vec<String> = roots.iter().map(|(name, r)| format!("- {name}: `{r}`")).collect();
    include_str!("ai_skill.md")
        .replace("{{IFSDEP}}", ifsdep)
        .replace("{{INDEX}}", index)
        .replace("{{ROOTS}}", &roots.join("\n"))
        .replace("{{BUILT}}", &date(built))
}

/// `2026-10-27 14:05 UTC` from milliseconds since 1970.
pub fn date(ms: u64) -> String {
    let secs = ms / 1000;
    let days = (secs / 86_400) as i64;
    // Civil date from days since 1970 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y}-{m:02}-{d:02} {:02}:{:02} UTC", secs % 86_400 / 3600, secs % 3600 / 60)
}

// ------------------------------------------------------------------ reading

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Node {
    Unit(usize),
    Object(usize),
    Relation(usize),
}

fn node(id: &str) -> Option<Node> {
    let n = id.get(1..)?.parse().ok()?;
    match id.as_bytes().first()? {
        b'u' => Some(Node::Unit(n)),
        b'o' => Some(Node::Object(n)),
        b'r' => Some(Node::Relation(n)),
        _ => None,
    }
}

fn layer(s: &str) -> Layer {
    if s == "Cust" {
        Layer::Cust
    } else {
        Layer::Core
    }
}

pub struct AiFile {
    pub layer: Layer,
    pub kind: String,
    pub component: String,
    pub rel: String,
    pub abs: String,
}

pub struct AiUnit {
    pub kind: String,
    pub name: String,
    pub component: String,
    pub origin: Layer,
    pub overlay: bool,
    pub files: Vec<usize>,
}

pub struct AiObject {
    pub unit: usize,
    pub file: usize,
    pub kind: String,
    pub name: String,
    pub line: usize,
    pub modifier: String,
    pub base: Option<usize>,
    pub generated: bool,
}

pub struct AiRelation {
    pub name: String,
    pub kind: String,
    /// (file, line, how)
    pub defs: Vec<(usize, usize, String)>,
}

pub struct AiEdge {
    pub from: Node,
    pub to: Node,
    pub kind: String,
    pub file: usize,
    pub line: usize,
    pub layer: Layer,
    pub dynamic: String,
    pub detail: String,
    pub count: usize,
}

pub struct AiFinding {
    pub severity: String,
    pub rule: String,
    pub location: Option<(usize, usize)>,
    pub involves_cust: bool,
    pub upgrade: bool,
    pub nodes: Vec<Node>,
    pub title: String,
    pub message: String,
    pub fix: String,
    pub fix_target: String,
    pub snippet: String,
}

#[derive(Default)]
pub struct AiIndex {
    pub built: u64,
    /// ("Core" | "Cust" | "Prev", folder)
    pub roots: Vec<(String, String)>,
    pub files: Vec<AiFile>,
    pub units: Vec<AiUnit>,
    pub objects: Vec<AiObject>,
    pub relations: Vec<AiRelation>,
    pub edges: Vec<AiEdge>,
    pub findings: Vec<AiFinding>,
    pub stamps: Vec<Stamp>,
    into: HashMap<Node, Vec<usize>>,
    out_of: HashMap<Node, Vec<usize>>,
    unit_objects: Vec<Vec<usize>>,
}

fn num(s: Option<&str>) -> usize {
    s.and_then(|s| s.parse().ok()).unwrap_or(0)
}

/// Reads what `write` wrote.
pub fn parse(text: &str) -> Result<AiIndex, String> {
    let mut lines = text.lines();
    if lines.next() != Some(HEADER) {
        return Err(format!("not an AI index written by this version of ifsdep-ai (expected `{HEADER}`); run `ifsdep-ai init` again"));
    }
    let mut ix = AiIndex::default();
    for l in lines {
        let mut p = l.split('\t');
        let tag = p.next().unwrap_or("");
        let mut s = || p.next().map(unesc).unwrap_or_default();
        match tag {
            "built" => ix.built = s().parse().unwrap_or(0),
            "root" => {
                let name = s();
                ix.roots.push((name, s()));
            }
            "F" => ix.files.push(AiFile { layer: layer(&s()), kind: s(), component: s(), rel: s(), abs: s() }),
            "U" => ix.units.push(AiUnit {
                kind: s(),
                name: s(),
                component: s(),
                origin: layer(&s()),
                overlay: s() == "1",
                files: s().split(',').filter_map(|f| f.parse().ok()).collect(),
            }),
            "O" => ix.objects.push(AiObject {
                unit: id_index(&s()),
                file: num(Some(&s())),
                kind: s(),
                name: s(),
                line: num(Some(&s())),
                modifier: s(),
                base: match node(&s()) {
                    Some(Node::Object(o)) => Some(o),
                    _ => None,
                },
                generated: s() == "1",
            }),
            "R" => ix.relations.push(AiRelation {
                name: s(),
                kind: s(),
                defs: s()
                    .split('|')
                    .filter_map(|d| {
                        let mut q = d.splitn(3, ':');
                        Some((q.next()?.parse().ok()?, q.next()?.parse().ok()?, q.next()?.to_string()))
                    })
                    .collect(),
            }),
            "E" => {
                let (Some(from), Some(to)) = (node(&s()), node(&s())) else { continue };
                ix.edges.push(AiEdge {
                    from,
                    to,
                    kind: s(),
                    file: num(Some(&s())),
                    line: num(Some(&s())),
                    layer: layer(&s()),
                    dynamic: s(),
                    detail: s(),
                    count: num(Some(&s())).max(1),
                });
            }
            "X" => {
                let severity = s();
                let rule = s();
                let (file, line) = (s(), s());
                ix.findings.push(AiFinding {
                    severity,
                    rule,
                    location: file.parse().ok().map(|f| (f, line.parse().unwrap_or(0))),
                    involves_cust: s() == "1",
                    upgrade: s() == "1",
                    nodes: s().split(',').filter_map(node).collect(),
                    title: s(),
                    message: s(),
                    fix: s(),
                    fix_target: s(),
                    snippet: s(),
                });
            }
            "S" => {
                let size = s().parse().unwrap_or(0);
                let modified = s().parse().unwrap_or(0);
                ix.stamps.push(Stamp { path: s(), size, modified });
            }
            _ => {}
        }
    }
    for (i, e) in ix.edges.iter().enumerate() {
        ix.into.entry(e.to).or_default().push(i);
        ix.out_of.entry(e.from).or_default().push(i);
    }
    ix.unit_objects = vec![Vec::new(); ix.units.len()];
    for (i, o) in ix.objects.iter().enumerate() {
        if let Some(v) = ix.unit_objects.get_mut(o.unit) {
            v.push(i);
        }
    }
    Ok(ix)
}

/// Where to find the AI index: `explicit`, else `IFSDEP_INDEX`, else `ifsdep-ai.idx` in the
/// current folder or the nearest folder above it that has one.
pub fn locate(explicit: Option<&Path>) -> Option<std::path::PathBuf> {
    if let Some(p) = explicit {
        return Some(p.to_path_buf());
    }
    if let Some(p) = std::env::var_os("IFSDEP_INDEX") {
        return Some(p.into());
    }
    let cwd = std::env::current_dir().ok()?;
    cwd.ancestors().map(|d| d.join(FILE_NAME)).find(|p| p.is_file())
}

// ------------------------------------------------------------------ queries

pub struct Opts {
    pub depth: usize,
    /// Most lines (or findings) to print before stopping.
    pub limit: usize,
}

impl AiIndex {
    fn place(&self, file: usize, line: usize) -> String {
        match self.files.get(file) {
            Some(f) => format!("[{}] {}:{}", f.layer.as_str(), f.abs, line),
            None => "?".into(),
        }
    }

    pub fn label(&self, n: Node) -> String {
        match n {
            Node::Unit(u) => format!("{} ({})", self.units[u].name, self.units[u].kind),
            Node::Object(o) => {
                let ob = &self.objects[o];
                let m = if ob.modifier.is_empty() { String::new() } else { format!(" @{}", ob.modifier) };
                format!("{}.{} ({}{m})", self.units[ob.unit].name, ob.name, ob.kind)
            }
            Node::Relation(r) => format!("{} ({})", self.relations[r].name, self.relations[r].kind),
        }
    }

    /// Where a node is defined.
    fn node_place(&self, n: Node) -> String {
        match n {
            Node::Unit(u) => self.units[u].files.first().map_or("?".into(), |&f| self.place(f, 1)),
            Node::Object(o) => {
                let ob = &self.objects[o];
                let g = if ob.generated { "  (generated from the .entity, in no source file)" } else { "" };
                format!("{}{g}", self.place(ob.file, ob.line))
            }
            Node::Relation(r) => match self.relations[r].defs.first() {
                Some((f, l, how)) => format!("{}  ({how})", self.place(*f, *l)),
                None => "(not defined in the indexed folders)".into(),
            },
        }
    }

    /// `Unit.Object`, or a unit, object, table or view name; any case.
    pub fn resolve(&self, q: &str) -> Vec<Node> {
        let q = q.trim();
        let units_named = |name: &str| -> Vec<usize> { (0..self.units.len()).filter(|&u| self.units[u].name.eq_ignore_ascii_case(name)).collect() };
        if let Some((u, m)) = q.rsplit_once('.') {
            return units_named(u)
                .into_iter()
                .flat_map(|u| self.unit_objects[u].iter().copied())
                .filter(|&o| self.objects[o].name.eq_ignore_ascii_case(m))
                .map(Node::Object)
                .collect();
        }
        let mut v: Vec<Node> = units_named(q).into_iter().map(Node::Unit).collect();
        v.extend((0..self.relations.len()).filter(|&r| self.relations[r].name.eq_ignore_ascii_case(q)).map(Node::Relation));
        v.extend((0..self.objects.len()).filter(|&o| self.objects[o].name.eq_ignore_ascii_case(q)).map(Node::Object));
        v
    }

    /// Names that contain the query, for when nothing matches exactly. Underscores and case
    /// don't count, so `CustomerOrder` finds `Customer_Order_API` and the `CustomerOrder*` units.
    fn suggest(&self, q: &str) -> String {
        let q = q.trim().to_string();
        let norm = |s: &str| s.replace('_', "").to_ascii_lowercase();
        let (uq, oq) = match q.rsplit_once('.') {
            Some((u, o)) => (norm(u), norm(o)),
            None => (norm(&q), norm(&q)),
        };
        let both = !q.contains('.');
        let mut names: Vec<String> = Vec::new();
        let mut seen = HashSet::new();
        let mut add = |s: String, names: &mut Vec<String>| {
            if seen.insert(s.to_ascii_lowercase()) {
                names.push(s);
            }
        };
        if both {
            for u in &self.units {
                if norm(&u.name).contains(&uq) {
                    add(u.name.clone(), &mut names);
                }
            }
            for r in &self.relations {
                if norm(&r.name).contains(&uq) {
                    add(r.name.clone(), &mut names);
                }
            }
        }
        for o in &self.objects {
            if norm(&o.name).contains(&oq) && (both || norm(&self.units[o.unit].name).contains(&uq)) {
                add(format!("{}.{}", self.units[o.unit].name, o.name), &mut names);
            }
        }
        names.sort_by_key(|n| (n.len(), n.clone()));
        let more = names.len().saturating_sub(15);
        names.truncate(15);
        if names.is_empty() {
            return format!("nothing named `{}` or containing it in the index", q);
        }
        let mut s = format!("nothing is named exactly `{q}`. Names containing it:\n");
        for n in &names {
            let _ = writeln!(s, "  {n}");
        }
        if more > 0 {
            let _ = writeln!(s, "  … and {more} more");
        }
        s
    }

    fn resolved(&self, q: &str) -> Result<Vec<Node>, String> {
        let v = self.resolve(q);
        if v.is_empty() {
            Err(self.suggest(q))
        } else {
            Ok(v)
        }
    }

    /// `find`: where each match is defined, its unit and what it overrides or is overridden by.
    pub fn find(&self, q: &str, o: &Opts) -> Result<String, String> {
        let nodes = self.resolved(q)?;
        let mut s = format!("{} match{} for `{}`\n", nodes.len(), if nodes.len() == 1 { "" } else { "es" }, q.trim());
        for (i, &n) in nodes.iter().enumerate() {
            if i >= o.limit {
                let _ = writeln!(s, "… and {} more (use Unit.Name to narrow, or --limit)", nodes.len() - i);
                break;
            }
            let _ = writeln!(s, "\n{}\n  at {}", self.label(n), self.node_place(n));
            match n {
                Node::Unit(u) => self.unit_files(u, &mut s),
                Node::Object(ob) => {
                    let obj = &self.objects[ob];
                    let u = &self.units[obj.unit];
                    let _ = writeln!(s, "  in {} ({}, component {}){}", u.name, u.kind, u.component, if u.overlay { ", has a Cust overlay" } else { "" });
                    if let Some(b) = obj.base {
                        let _ = writeln!(s, "  overrides {} at {}", self.label(Node::Object(b)), self.node_place(Node::Object(b)));
                    } else if !obj.modifier.is_empty() {
                        let _ = writeln!(s, "  overrides a base generated from the model (in no source file)");
                    }
                    for &e in self.into.get(&n).into_iter().flatten() {
                        let e = &self.edges[e];
                        if e.kind == "overrides" {
                            let _ = writeln!(s, "  overridden by {} at {}", self.label(e.from), self.node_place(e.from));
                        }
                    }
                }
                Node::Relation(r) => {
                    for (f, l, how) in self.relations[r].defs.iter().skip(1) {
                        let _ = writeln!(s, "  also {}  ({how})", self.place(*f, *l));
                    }
                }
            }
            let ins = self.into.get(&n).map_or(0, Vec::len);
            let outs = self.out_of.get(&n).map_or(0, Vec::len);
            let _ = writeln!(s, "  {ins} incoming and {outs} outgoing links (see callers / callees)");
        }
        Ok(s)
    }

    fn unit_files(&self, u: usize, s: &mut String) {
        for &f in &self.units[u].files {
            let fl = &self.files[f];
            let _ = writeln!(s, "  file [{}] {} ({})", fl.layer.as_str(), fl.abs, fl.kind);
        }
    }

    fn edge_line(&self, e: &AiEdge, other: Node, incoming: bool) -> String {
        let arrow = if incoming { "<-" } else { "->" };
        let mut extra = String::new();
        if !e.detail.is_empty() && e.kind != "overrides" {
            let _ = write!(extra, " `{}`", e.detail);
        }
        if e.count > 1 {
            let _ = write!(extra, " ×{}", e.count);
        }
        if !e.dynamic.is_empty() {
            let _ = write!(extra, " (only when {} is installed)", e.dynamic);
        }
        format!("{arrow} {} {}{extra}  at {}", e.kind, self.label(other), self.place(e.file, e.line))
    }

    /// `callers` (incoming) or `callees` (outgoing), as a tree `depth` levels deep.
    pub fn links(&self, q: &str, incoming: bool, o: &Opts) -> Result<String, String> {
        let nodes = self.resolved(q)?;
        let mut s = String::new();
        let _ = writeln!(s, "{} of `{}`:", if incoming { "Callers (everything that links to it)" } else { "Callees (everything it links to)" }, q.trim());
        for &n in &nodes {
            let _ = writeln!(s, "  {}  {}", self.label(n), self.node_place(n));
        }
        s.push('\n');
        let mut seen: HashSet<Node> = nodes.iter().copied().collect();
        let mut printed = 0usize;
        let mut cut = false;
        for &n in &nodes {
            self.links_tree(n, incoming, 1, o, &mut seen, &mut printed, &mut cut, &mut s);
        }
        if printed == 0 {
            s.push_str("(none)\n");
        }
        if cut {
            let _ = writeln!(s, "… stopped at {} lines (use --limit, or a lower --depth)", o.limit);
        }
        Ok(s)
    }

    #[allow(clippy::too_many_arguments)]
    fn links_tree(&self, n: Node, incoming: bool, level: usize, o: &Opts, seen: &mut HashSet<Node>, printed: &mut usize, cut: &mut bool, s: &mut String) {
        let map = if incoming { &self.into } else { &self.out_of };
        let mut es: Vec<&AiEdge> = map.get(&n).into_iter().flatten().map(|&e| &self.edges[e]).collect();
        let other = |e: &AiEdge| if incoming { e.from } else { e.to };
        // Cust first: that's the code that can change.
        es.sort_by_key(|e| (e.layer != Layer::Cust, self.label(other(e)), e.file, e.line));
        for e in es {
            if *printed >= o.limit {
                *cut = true;
                return;
            }
            let m = other(e);
            let again = !seen.insert(m);
            let _ = writeln!(s, "{}{}{}", "   ".repeat(level - 1), self.edge_line(e, m, incoming), if again && level < o.depth { "  (expanded elsewhere)" } else { "" });
            *printed += 1;
            if !again && level < o.depth {
                self.links_tree(m, incoming, level + 1, o, seen, printed, cut, s);
            }
        }
    }

    /// `impact`: everything that reaches the name through any link, grouped by unit.
    pub fn impact(&self, q: &str, o: &Opts) -> Result<String, String> {
        let mut start = self.resolved(q)?;
        // A unit stands for its objects too.
        for n in start.clone() {
            if let Node::Unit(u) = n {
                start.extend(self.unit_objects[u].iter().map(|&o| Node::Object(o)));
            }
        }
        let starts: HashSet<Node> = start.iter().copied().collect();
        // node → (hops, edge that reached it)
        let mut reached: HashMap<Node, (usize, usize)> = HashMap::new();
        let mut frontier = start;
        for hop in 1..=o.depth {
            let mut next = Vec::new();
            for n in &frontier {
                for &e in self.into.get(n).into_iter().flatten() {
                    let m = self.edges[e].from;
                    if !starts.contains(&m) && !reached.contains_key(&m) {
                        reached.insert(m, (hop, e));
                        next.push(m);
                    }
                }
            }
            if next.is_empty() {
                break;
            }
            frontier = next;
        }
        // Group by unit: (layer, nearest hop, objects, nearest node, its edge)
        let unit_of = |n: Node| match n {
            Node::Unit(u) => Some(u),
            Node::Object(o) => Some(self.objects[o].unit),
            Node::Relation(_) => None,
        };
        let mut by_unit: HashMap<usize, (usize, usize, Node, usize)> = HashMap::new();
        for (&n, &(hop, e)) in &reached {
            let Some(u) = unit_of(n) else { continue };
            let g = by_unit.entry(u).or_insert((hop, 0, n, e));
            if matches!(n, Node::Object(_)) {
                g.1 += 1;
            }
            if hop < g.0 {
                (g.0, g.2, g.3) = (hop, n, e);
            }
        }
        let mut units: Vec<(usize, (usize, usize, Node, usize))> = by_unit.into_iter().collect();
        units.sort_by_key(|(u, g)| (self.units[*u].origin != Layer::Cust && !self.units[*u].overlay, g.0, self.units[*u].name.clone()));
        let cust = units.iter().filter(|(u, _)| self.units[*u].origin == Layer::Cust || self.units[*u].overlay).count();
        let mut s = format!(
            "Changing `{}` reaches {} units ({} with Cust code) within {} hops; nearest first, Cust first.\n\
             Each line: unit, hops away, how many of its objects are reached, and the nearest link.\n\n",
            q.trim(),
            units.len(),
            cust,
            o.depth
        );
        for (i, (u, (hop, objs, n, e))) in units.iter().enumerate() {
            if i >= o.limit {
                let _ = writeln!(s, "… and {} more units (use --limit)", units.len() - i);
                break;
            }
            let un = &self.units[*u];
            let tag = if un.origin == Layer::Cust { "Cust" } else if un.overlay { "Core+Cust" } else { "Core" };
            let e = &self.edges[*e];
            let _ = writeln!(s, "[{tag}] {} ({}, {}) {hop} hop{}, {objs} objects", un.name, un.kind, un.component, if *hop == 1 { "" } else { "s" });
            let _ = writeln!(s, "    {}", self.edge_line(e, *n, true).trim_start_matches("<- "));
        }
        Ok(s)
    }

    /// `unit`: a unit's files in both layers, what Cust adds or overrides, its unit-level
    /// links and its findings.
    pub fn unit(&self, q: &str, o: &Opts) -> Result<String, String> {
        let name = q.trim();
        let us: Vec<usize> = (0..self.units.len()).filter(|&u| self.units[u].name.eq_ignore_ascii_case(name)).collect();
        if us.is_empty() {
            return Err(self.suggest(name));
        }
        let mut s = String::new();
        for u in us {
            let un = &self.units[u];
            let _ = writeln!(
                s,
                "{} ({}), component {}, base in {}{}",
                un.name,
                un.kind,
                un.component,
                un.origin.as_str(),
                if un.overlay { ", with a Cust overlay" } else { "" }
            );
            self.unit_files(u, &mut s);
            let objs = &self.unit_objects[u];
            let mut kinds: HashMap<&str, usize> = HashMap::new();
            for &ob in objs {
                *kinds.entry(self.objects[ob].kind.as_str()).or_default() += 1;
            }
            let mut kinds: Vec<_> = kinds.into_iter().collect();
            kinds.sort();
            let list: Vec<String> = kinds.iter().map(|(k, n)| format!("{n} {k}")).collect();
            let _ = writeln!(s, "  {} objects: {}", objs.len(), list.join(", "));
            let cust: Vec<usize> = objs.iter().copied().filter(|&ob| self.files[self.objects[ob].file].layer == Layer::Cust).collect();
            if !cust.is_empty() && un.origin == Layer::Core {
                let _ = writeln!(s, "  In Cust ({}):", cust.len());
                for (i, &ob) in cust.iter().enumerate() {
                    if i >= o.limit {
                        let _ = writeln!(s, "    … and {} more", cust.len() - i);
                        break;
                    }
                    let obj = &self.objects[ob];
                    let what = if obj.modifier.is_empty() { "new".to_string() } else { format!("@{}", obj.modifier) };
                    let _ = writeln!(s, "    {what} {} {} :{}", obj.kind, obj.name, obj.line);
                }
            }
            for (title, incoming) in [("Uses", false), ("Used by", true)] {
                let map = if incoming { &self.into } else { &self.out_of };
                let es: Vec<&AiEdge> = map.get(&Node::Unit(u)).into_iter().flatten().map(|&e| &self.edges[e]).collect();
                if es.is_empty() {
                    continue;
                }
                // One line per (kind, other unit).
                let mut grouped: HashMap<(String, Node), (usize, &AiEdge)> = HashMap::new();
                for e in &es {
                    let other = if incoming { e.from } else { e.to };
                    let g = grouped.entry((e.kind.clone(), other)).or_insert((0, e));
                    g.0 += e.count;
                }
                let mut g: Vec<_> = grouped.into_iter().collect();
                g.sort_by_key(|((k, n), _)| (k.clone(), self.label(*n)));
                let _ = writeln!(s, "  {title} ({}):", g.len());
                for (i, ((k, n), (count, e))) in g.iter().enumerate() {
                    if i >= o.limit {
                        let _ = writeln!(s, "    … and {} more", g.len() - i);
                        break;
                    }
                    let _ = writeln!(s, "    {k} {} ×{count}  first at {}", self.label(*n), self.place(e.file, e.line));
                }
            }
            let mine: HashSet<Node> = std::iter::once(Node::Unit(u)).chain(objs.iter().map(|&o| Node::Object(o))).collect();
            let fs: Vec<&AiFinding> = self.findings.iter().filter(|f| f.nodes.iter().any(|n| mine.contains(n))).collect();
            if !fs.is_empty() {
                let _ = writeln!(s, "  Findings ({}):", fs.len());
                for f in fs.iter().take(o.limit) {
                    let _ = writeln!(s, "    {} {} {}", f.severity, f.rule, f.title);
                }
                if fs.len() > o.limit {
                    let _ = writeln!(s, "    … and {} more (ifsdep-ai findings --unit {})", fs.len() - o.limit, un.name);
                }
            }
            s.push('\n');
        }
        Ok(s)
    }

    /// `findings`, filtered. `file`: part of the path; `unit`: a unit name; `rule`: the rule id.
    pub fn findings_list(&self, file: Option<&str>, unit: Option<&str>, rule: Option<&str>, cust_only: bool, o: &Opts) -> Result<String, String> {
        let file = file.map(|f| f.replace('\\', "/").to_ascii_lowercase());
        let unit_nodes: Option<HashSet<Node>> = unit.map(|name| {
            let mut set = HashSet::new();
            for u in (0..self.units.len()).filter(|&u| self.units[u].name.eq_ignore_ascii_case(name.trim())) {
                set.insert(Node::Unit(u));
                set.extend(self.unit_objects[u].iter().map(|&o| Node::Object(o)));
            }
            set
        });
        let mut fs: Vec<&AiFinding> = self
            .findings
            .iter()
            .filter(|f| rule.is_none_or(|r| f.rule.eq_ignore_ascii_case(r)))
            .filter(|f| !cust_only || f.involves_cust)
            .filter(|f| unit_nodes.as_ref().is_none_or(|set| f.nodes.iter().any(|n| set.contains(n))))
            .filter(|f| {
                file.as_ref().is_none_or(|q| {
                    f.location.is_some_and(|(fi, _)| {
                        self.files.get(fi).is_some_and(|fl| fl.abs.replace('\\', "/").to_ascii_lowercase().contains(q.as_str()))
                    })
                })
            })
            .collect();
        let rank = |s: &str| match s {
            "error" => 0,
            "warning" => 1,
            _ => 2,
        };
        fs.sort_by_key(|f| (rank(&f.severity), !f.involves_cust, f.rule.clone()));
        let mut s = format!("{} findings\n", fs.len());
        for (i, f) in fs.iter().enumerate() {
            if i >= o.limit {
                let _ = writeln!(s, "\n… and {} more (narrow with --file, --unit, --rule or --cust, or use --limit)", fs.len() - i);
                break;
            }
            let _ = writeln!(s, "\n{} {} {}{}", f.severity.to_uppercase(), f.rule, f.title, if f.upgrade { "  (upgrade)" } else { "" });
            if let Some((fi, l)) = f.location {
                let _ = writeln!(s, "  at {}", self.place(fi, l));
            }
            let _ = writeln!(s, "  {}", f.message);
            if !f.fix.is_empty() {
                let _ = writeln!(s, "  fix: {}{}", f.fix, if f.fix_target.is_empty() { String::new() } else { format!(" (in {})", f.fix_target) });
            }
            for l in f.snippet.lines() {
                let _ = writeln!(s, "    {l}");
            }
        }
        Ok(s)
    }

    /// `trace`: what a PL/SQL method does in run order, each call followed by where it's defined.
    /// Reads the method's source file, so it shows the code as it is now.
    pub fn trace(&self, q: &str, o: &Opts) -> Result<String, String> {
        let nodes = self.resolved(q)?;
        let methods: Vec<usize> = nodes
            .iter()
            .filter_map(|n| match n {
                Node::Object(o) => Some(*o),
                _ => None,
            })
            .filter(|&ob| {
                let obj = &self.objects[ob];
                !obj.generated && matches!(obj.kind.as_str(), "procedure" | "function") && matches!(self.files[obj.file].kind.as_str(), "plsql" | "plsvc")
            })
            .collect();
        if methods.is_empty() {
            return Err(format!("`{}` isn't a PL/SQL procedure or function in a .plsql / .plsvc file (try `ifsdep-ai find {}`)", q.trim(), q.trim()));
        }
        let mut s = String::new();
        for (i, &ob) in methods.iter().enumerate() {
            if i >= o.limit {
                let _ = writeln!(s, "… and {} more (use Unit.Name)", methods.len() - i);
                break;
            }
            let obj = &self.objects[ob];
            let file = &self.files[obj.file];
            let text = std::fs::read_to_string(&file.abs).map_err(|e| format!("could not read {}: {e}", file.abs))?;
            let Some(t) = trace::trace_method(&text, obj.line) else {
                let _ = writeln!(s, "{}: no PROCEDURE or FUNCTION on line {} any more; the file changed since `ifsdep-ai init`\n", self.label(Node::Object(ob)), obj.line);
                continue;
            };
            let _ = writeln!(s, "{}  [{}] {}:{}-{}", self.label(Node::Object(ob)), file.layer.as_str(), file.abs, t.line, t.end_line);
            let annotate = |st: &TraceStep| -> String {
                if st.kind != "call" {
                    return String::new();
                }
                let defs = self.method_defs(st.package.as_deref(), obj.unit, &st.name);
                if defs.is_empty() {
                    return String::new();
                }
                let parts: Vec<String> = defs
                    .iter()
                    .map(|&d| {
                        let dob = &self.objects[d];
                        let m = if dob.modifier.is_empty() { String::new() } else { format!(" @{}", dob.modifier) };
                        let g = if dob.generated { " (generated)" } else { "" };
                        format!("{}{m}{g}", self.place(dob.file, dob.line))
                    })
                    .collect();
                format!("  → {}", parts.join("; "))
            };
            print_steps(&t.steps, 1, &annotate, &mut s);
            s.push('\n');
        }
        Ok(s)
    }

    /// The definitions of `package.name` (or `name` in `own_unit` with no package), Cust first.
    fn method_defs(&self, package: Option<&str>, own_unit: usize, name: &str) -> Vec<usize> {
        let units: Vec<usize> = match package {
            Some(p) => (0..self.units.len()).filter(|&u| self.units[u].name.eq_ignore_ascii_case(p)).collect(),
            None => vec![own_unit],
        };
        let mut v: Vec<usize> = units
            .into_iter()
            .flat_map(|u| self.unit_objects[u].iter().copied())
            .filter(|&o| self.objects[o].name.eq_ignore_ascii_case(name) && matches!(self.objects[o].kind.as_str(), "procedure" | "function"))
            .collect();
        v.sort_by_key(|&o| (self.files[self.objects[o].file].layer != Layer::Cust, self.objects[o].line));
        v
    }

    /// `status`: which indexed files changed, appeared or went since the AI index was written.
    pub fn status(&self) -> String {
        let roots: Vec<&Path> = self.roots.iter().map(|(_, r)| Path::new(r.as_str())).collect();
        let now = stamp::stamps(&roots);
        let c = stamp::compare(&self.stamps, &now);
        let mut s = String::new();
        for (name, root) in &self.roots {
            let _ = writeln!(s, "{name}: {root}");
        }
        let _ = writeln!(s, "{} files, {} units, {} objects, {} links, {} findings", self.files.len(), self.units.len(), self.objects.len(), self.edges.len(), self.findings.len());
        let _ = writeln!(s, "written {} ({})", date(self.built), age(self.built));
        if c.is_empty() {
            s.push_str("up to date: no file changed since\n");
            return s;
        }
        let _ = writeln!(
            s,
            "OUT OF DATE: {} changed, {} added, {} removed since; line numbers may be off. Run `ifsdep-ai init` again to refresh.",
            c.changed.len(),
            c.added.len(),
            c.removed.len()
        );
        for (what, list) in [("changed", &c.changed), ("added", &c.added), ("removed", &c.removed)] {
            for p in list.iter().take(10) {
                let _ = writeln!(s, "  {what} {p}");
            }
            if list.len() > 10 {
                let _ = writeln!(s, "  … and {} more {what}", list.len() - 10);
            }
        }
        s
    }
}

fn age(ms: u64) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
    let mins = now.saturating_sub(ms) / 60_000;
    match mins {
        0 => "just now".into(),
        m if m < 120 => format!("{m} minutes ago"),
        m if m < 48 * 60 => format!("{} hours ago", m / 60),
        m => format!("{} days ago", m / (24 * 60)),
    }
}

/// A trace as indented text; `note` adds to a step's line (e.g. where a call goes).
pub fn print_steps(steps: &[TraceStep], indent: usize, note: &dyn Fn(&TraceStep) -> String, out: &mut String) {
    let pad = "   ".repeat(indent);
    let pkg = |s: &TraceStep| s.package.as_deref().map(|p| format!("{p}.")).unwrap_or_default();
    for s in steps {
        match s.kind {
            "fork" => {
                let _ = writeln!(out, "{pad}<> {} :{}", s.text, s.line);
                for a in &s.arms {
                    let _ = writeln!(out, "{pad}   [{}]{} :{}", a.label, if a.implied { " (implied)" } else { "" }, a.line);
                    print_steps(&a.steps, indent + 2, note, out);
                }
            }
            "loop" | "cursor" => {
                let _ = writeln!(out, "{pad}{} {} :{}", s.kind, if s.kind == "loop" { &s.text } else { &s.name }, s.line);
                print_steps(&s.steps, indent + 1, note, out);
            }
            "error" | "call" => {
                let _ = writeln!(out, "{pad}{} {}{} :{}{}", s.kind, pkg(s), s.name, s.line, note(s));
            }
            k => {
                let _ = writeln!(out, "{pad}{k} {} :{}", s.name, s.line);
            }
        }
    }
}
