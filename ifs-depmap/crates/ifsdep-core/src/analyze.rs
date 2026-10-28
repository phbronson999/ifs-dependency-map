//! Rules that turn the graph into findings.
//!
//! - CYCLE: circular `include fragment` chains, marked when they only appear
//!   once the Cust layer is applied.
//! - DUPLICATE_DEFINITION: the same construct reaches one client/projection
//!   from two different units.
//! - MISSING_OVERRIDE: a Cust overlay redeclares a Core construct without `@Override`.
//! - OVERRIDE_NO_BASE: `@Override`/`@Overtake` with nothing to layer over.
//! - CONNECTION_MISSING / DYNAMIC_NOT_GUARDED: cross-component dependencies
//!   checked against deploy.ini `[Connections]`.
//! - LAYER_MISMATCH: a file in the wrong folder for its declared layer.
//! - COLUMN_AMBIGUOUS: a column Cust adds to a view or table makes an unprefixed column
//!   name in a query ambiguous, because another relation in the same FROM has it too.

use crate::build::kind_group;
use crate::graph::tarjan_scc;
use crate::model::*;
use crate::sqlscope;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};

/// Where a column is defined, for evidence.
#[derive(Debug, Clone)]
pub struct ColumnSource {
    pub file: usize,
    pub line: usize,
    /// e.g. "attribute State of CustomerInfo".
    pub what: String,
}

/// What the column rule needs to know about columns. Built from the parsed files and not
/// part of the index.
#[derive(Debug, Default)]
pub struct Columns {
    /// Relation → column → where it is defined, in either layer.
    pub known: HashMap<String, HashMap<String, ColumnSource>>,
    /// Relation → the columns Cust adds that Core's definition of it doesn't have.
    pub added: HashMap<String, Vec<(String, ColumnSource)>>,
}

pub fn run(idx: &Index, cols: &Columns) -> Vec<Finding> {
    let mut out = Vec::new();
    let mut src = Source::default();
    cycles(idx, &mut out);
    duplicates(idx, &mut src, &mut out);
    overrides(idx, &mut out);
    connections(idx, &mut out);
    layers(idx, &mut out);
    ambiguous_columns(idx, cols, &mut src, &mut out);
    out
}

/// A Cust column on relation R, used without an alias prefix in a query block whose FROM
/// also has a relation with that column (or R twice). Core compiles as delivered, so the
/// name was unambiguous before Cust added the column; now Oracle can't resolve it.
fn ambiguous_columns(idx: &Index, cols: &Columns, src: &mut Source, out: &mut Vec<Finding>) {
    if cols.added.is_empty() {
        return;
    }
    // Only SQL files that read one of those relations are read again.
    let mut files: BTreeSet<usize> = BTreeSet::new();
    for e in &idx.edges {
        if e.kind == EdgeKind::Reads
            && e.to.starts_with('r')
            && cols.added.contains_key(&idx.relations[id_index(&e.to)].name)
            && matches!(idx.files[e.file].kind, FileKind::Plsql | FileKind::Plsvc | FileKind::Views)
        {
            files.insert(e.file);
        }
    }
    let rel_id: HashMap<&str, &str> = idx.relations.iter().map(|r| (r.name.as_str(), r.id.as_str())).collect();
    let file_name = |fi: usize| idx.files[fi].rel_path.rsplit('/').next().unwrap_or("").to_string();
    for fi in files {
        let Ok(bytes) = std::fs::read(&idx.files[fi].abs_path) else { continue };
        let text = sqlscope::prepare(&String::from_utf8_lossy(&bytes));
        let mut seen: HashSet<(usize, String)> = HashSet::new();
        for b in sqlscope::blocks(&text) {
            for (k, item) in b.from.iter().enumerate() {
                let Some(added) = cols.added.get(&item.name) else { continue };
                for (col, csrc) in added {
                    let Some(&(_, line)) = b.refs.iter().find(|(r, _)| r == col) else { continue };
                    let other = b.from.iter().enumerate().find(|&(j, o)| {
                        j != k && (o.name == item.name || cols.known.get(&o.name).is_some_and(|m| m.contains_key(col)))
                    });
                    let Some((_, other)) = other else { continue };
                    if !seen.insert((line, col.clone())) {
                        continue;
                    }
                    let lower = col.to_ascii_lowercase();
                    let cust_file = file_name(csrc.file);
                    let mut f = finding(
                        Severity::Error,
                        "COLUMN_AMBIGUOUS",
                        format!("`{lower}` becomes ambiguous in {}", file_name(fi)),
                        format!(
                            "{cust_file} adds the {} to {}, so it gets a {col} column. {}:{line} uses `{lower}` without an alias \
                             prefix in a query that also reads {}, which has {col} too. Oracle can't tell which is meant \
                             (ORA-00918: column ambiguously defined), so this code no longer compiles.",
                            csrc.what, item.name, idx.files[fi].rel_path, other.name
                        ),
                    );
                    f.location = Some(Location { file: fi, line });
                    for name in [&item.name, &other.name] {
                        if let Some(id) = rel_id.get(name.as_str()) {
                            if !f.nodes.iter().any(|n| n == id) {
                                f.nodes.push(id.to_string());
                            }
                        }
                    }
                    // The method the query is in.
                    if let Some(o) = idx.objects.iter().filter(|o| o.file == fi && o.line <= line).max_by_key(|o| o.line) {
                        f.nodes.push(o.id.clone());
                    }
                    let alias = |o: &sqlscope::FromItem| o.alias.as_ref().map_or(String::new(), |a| format!(" {}", a.to_ascii_lowercase()));
                    let mut query = vec![src.step(idx, fi, line, Some(format!("`{lower}` has no alias prefix, so Oracle looks for it in every relation of the FROM")))];
                    for (o, what) in [(item, "reads"), (other, "also reads")] {
                        let note = format!("{what} {}{}", o.name, alias(o));
                        match query.iter_mut().find(|s| s.line == o.line) {
                            // Both on one FROM line: one step that names both.
                            Some(s) => s.note = Some(format!("{} and {}{}", s.note.take().unwrap_or_default(), o.name, alias(o))),
                            None => query.push(src.step(idx, fi, o.line, Some(note))),
                        }
                    }
                    query.sort_by_key(|s| s.line);
                    let mut evidence = vec![
                        Trail {
                            title: format!("Cust adds {col} to {}", item.name),
                            steps: vec![src.step(idx, csrc.file, csrc.line, Some(format!("the {} becomes column {col} of {}", csrc.what, item.name)))],
                        },
                        Trail { title: "The query".into(), steps: query },
                    ];
                    if other.name == item.name {
                        evidence.push(Trail {
                            title: format!("{} is read twice", item.name),
                            steps: vec![src.step(idx, fi, other.line, Some("both copies now have the column".into()))],
                        });
                    } else if let Some(o) = cols.known.get(&other.name).and_then(|m| m.get(col)) {
                        evidence.push(Trail {
                            title: format!("{} already has {col}", other.name),
                            steps: vec![src.step(idx, o.file, o.line, Some(format!("the {}", o.what)))],
                        });
                    }
                    f.evidence = evidence;
                    f.suggestion = Some(Suggestion {
                        summary: "Give the Cust attribute a name Core doesn't use, e.g. with your prefix. Core code can't be changed, so the Cust side has to move.".into(),
                        snippet: csrc.what.strip_prefix("attribute ").and_then(|w| w.split(' ').next()).map(|a| format!("- <NAME>{a}</NAME>\n+ <NAME>Ti{a}</NAME>")),
                        target: Some(idx.files[csrc.file].rel_path.clone()),
                    });
                    out.push(f);
                }
            }
        }
    }
}

/// Reads the source lines that evidence cites, loading each file once.
#[derive(Default)]
struct Source {
    files: HashMap<usize, Vec<String>>,
}

impl Source {
    fn step(&mut self, idx: &Index, file: usize, line: usize, note: Option<String>) -> Step {
        let lines = self.files.entry(file).or_insert_with(|| {
            std::fs::read(&idx.files[file].abs_path)
                .map(|b| String::from_utf8_lossy(&b).lines().map(|l| l.trim().to_string()).collect())
                .unwrap_or_default()
        });
        let text = line.checked_sub(1).and_then(|i| lines.get(i)).cloned().unwrap_or_default();
        Step { file, line, text, note }
    }
}

fn finding(severity: Severity, rule: &str, title: impl Into<String>, message: impl Into<String>) -> Finding {
    Finding {
        id: String::new(),
        severity,
        rule: rule.into(),
        title: title.into(),
        message: message.into(),
        nodes: vec![],
        edges: vec![],
        location: None,
        suggestion: None,
        evidence: vec![],
        diff: None,
        upgrade: false,
        involves_cust: false,
    }
}

fn unit<'a>(idx: &'a Index, id: &str) -> &'a Unit {
    &idx.units[id_index(id)]
}

fn cap(s: &str) -> String {
    let lower = s.to_ascii_lowercase();
    let mut c = lower.chars();
    match c.next() {
        Some(f) => f.to_ascii_uppercase().to_string() + c.as_str(),
        None => String::new(),
    }
}

// ---------------------------------------------------------------- cycles

fn cycles(idx: &Index, out: &mut Vec<Finding>) {
    let n = idx.units.len();
    let include_edges: Vec<&Edge> = idx.edges.iter().filter(|e| e.kind == EdgeKind::Include).collect();

    let build = |core_only: bool| {
        let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];
        for e in &include_edges {
            if !core_only || e.layer == Layer::Core {
                adj[id_index(&e.from)].push(id_index(&e.to));
            }
        }
        adj
    };
    let is_cyclic = |adj: &[Vec<usize>], scc: &[usize]| scc.len() > 1 || adj[scc[0]].contains(&scc[0]);

    let core_g = build(true);
    let mut core_scc_of = vec![usize::MAX; n];
    for (i, scc) in tarjan_scc(&core_g).iter().enumerate() {
        if is_cyclic(&core_g, scc) {
            for &node in scc {
                core_scc_of[node] = i;
            }
        }
    }

    let g = build(false);
    for scc in tarjan_scc(&g) {
        if !is_cyclic(&g, &scc) {
            continue;
        }
        let members: HashSet<usize> = scc.iter().copied().collect();
        let first_core = core_scc_of[scc[0]];
        let core_only = first_core != usize::MAX && members.iter().all(|&m| core_scc_of[m] == first_core);
        let prefer = if core_only { Layer::Core } else { Layer::Cust };

        // Shortest loop through the alphabetically first member.
        let start = *members.iter().min_by_key(|&&m| idx.units[m].name.to_ascii_lowercase()).unwrap();
        let mut adj: BTreeMap<usize, BTreeSet<usize>> = BTreeMap::new();
        for e in &include_edges {
            let (a, b) = (id_index(&e.from), id_index(&e.to));
            if members.contains(&a) && members.contains(&b) {
                adj.entry(a).or_default().insert(b);
            }
        }
        let path = shortest_cycle(start, &adj);
        let hop_edges: Vec<&Edge> = path
            .windows(2)
            .map(|w| {
                let mut cands: Vec<&&Edge> = include_edges
                    .iter()
                    .filter(|e| id_index(&e.from) == w[0] && id_index(&e.to) == w[1])
                    .collect();
                cands.sort_by_key(|e| (e.layer != prefer, e.file, e.line));
                *cands[0]
            })
            .collect();
        let break_edge = if core_only {
            *hop_edges.last().unwrap()
        } else {
            hop_edges.iter().copied().find(|e| e.layer == Layer::Cust).unwrap_or(hop_edges[0])
        };

        let names: Vec<&str> = path.iter().map(|&u| idx.units[u].name.as_str()).collect();
        let mut message = names.join(" → ");
        if members.len() > path.len() - 1 {
            message.push_str(&format!(
                ". {} fragments are tangled in this loop group; the path above is the shortest loop.",
                members.len()
            ));
        }
        let (title, summary, snippet) = if core_only {
            (
                "Circular fragment include (in Core)".to_string(),
                "This loop exists in Core itself, so it isn't yours to change. Avoid Cust includes that depend on the order these fragments load in.".to_string(),
                None,
            )
        } else {
            let from = unit(idx, &break_edge.from);
            let to = unit(idx, &break_edge.to);
            let bf = &idx.files[break_edge.file];
            (
                "Circular include (appears with Cust layer)".to_string(),
                format!(
                    "Break the loop at {} → {}: move what {} uses from {} into a new shared fragment that both include, then remove this include.",
                    from.name, to.name, from.name, to.name
                ),
                Some(format!(
                    "-- {path}:{line}\n- include fragment {to};\n\n-- new file: {to}Shared-Cust.fragment\nfragment {to}Shared;\ncomponent {comp};\nlayer Cust;\n-- move the constructs both {from} and {to} need here,\n-- then add `include fragment {to}Shared;` to each of them",
                    path = bf.rel_path,
                    line = break_edge.line,
                    to = to.name,
                    from = from.name,
                    comp = bf.component,
                )),
            )
        };
        let mut f = finding(Severity::Error, "CYCLE", title, message);
        let mut nodes: Vec<usize> = members.iter().copied().collect();
        nodes.sort();
        f.nodes = nodes.iter().map(|&u| idx.units[u].id.clone()).collect();
        f.edges = hop_edges.iter().map(|e| e.id).collect();
        f.location = Some(Location { file: break_edge.file, line: break_edge.line });
        f.suggestion = Some(Suggestion {
            summary,
            snippet,
            target: Some(idx.files[break_edge.file].rel_path.clone()),
        });
        out.push(f);
    }
}

/// Returns `[start, …, start]`, or `[start, start]` for a self-loop.
fn shortest_cycle(start: usize, adj: &BTreeMap<usize, BTreeSet<usize>>) -> Vec<usize> {
    let mut parent: HashMap<usize, usize> = HashMap::new();
    let mut queue = VecDeque::new();
    if adj.get(&start).is_some_and(|s| s.contains(&start)) {
        return vec![start, start];
    }
    for &v in adj.get(&start).into_iter().flatten() {
        if let std::collections::hash_map::Entry::Vacant(e) = parent.entry(v) {
            e.insert(start);
            queue.push_back(v);
        }
    }
    while let Some(u) = queue.pop_front() {
        for &v in adj.get(&u).into_iter().flatten() {
            if v == start {
                let mut path = vec![start, u];
                let mut cur = u;
                while let Some(&p) = parent.get(&cur) {
                    if p == start {
                        break;
                    }
                    path.push(p);
                    cur = p;
                }
                path.push(start);
                // path is start, u, …, start reversed in the middle
                let mut mid: Vec<usize> = path[1..path.len() - 1].to_vec();
                mid.reverse();
                let mut res = vec![start];
                res.extend(mid);
                res.push(start);
                return res;
            }
            if let std::collections::hash_map::Entry::Vacant(e) = parent.entry(v) {
                e.insert(u);
                queue.push_back(v);
            }
        }
    }
    vec![start, start]
}

// ------------------------------------------------------------ duplicates

fn dup_key(o: &Object) -> String {
    let section = match o.section {
        Some(Section::Client) => "client",
        Some(Section::Projection) => "projection",
        None => "plsql",
    };
    let group = kind_group(&o.kind);
    let bound = o.target.is_some() && (o.kind == "action" || o.kind == "function");
    if o.kind == "command" || bound {
        // Commands are scoped by their `for` target (many dialogs have `command Ok`),
        // and bound actions by the entity they are declared in.
        format!("{section}:{group}:{}:{}", o.name.to_ascii_uppercase(), o.target.clone().unwrap_or_default().to_ascii_uppercase())
    } else {
        format!("{section}:{group}:{}", o.name.to_ascii_uppercase())
    }
}

fn duplicates(idx: &Index, src: &mut Source, out: &mut Vec<Finding>) {
    let mut by_unit: HashMap<&str, Vec<usize>> = HashMap::new();
    // Generated methods aren't declarations anyone wrote: a generated getter next to a
    // hand-written method of the same name is an overload, not a redeclaration.
    for (oi, o) in idx.objects.iter().enumerate().filter(|(_, o)| !o.generated) {
        by_unit.entry(o.unit.as_str()).or_default().push(oi);
    }

    // a) Same unit, two files, no @Override → the overlay forgot the annotation.
    for u in &idx.units {
        let mut seen: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for &oi in by_unit.get(u.id.as_str()).into_iter().flatten() {
            let o = &idx.objects[oi];
            if o.modifier.is_none() {
                seen.entry(dup_key(o)).or_default().push(oi);
            }
        }
        for objs in seen.values() {
            let files: BTreeSet<usize> = objs.iter().map(|&o| idx.objects[o].file).collect();
            if files.len() < 2 {
                continue;
            }
            let base = objs.iter().copied().find(|&o| idx.files[idx.objects[o].file].root == Layer::Core).unwrap_or(objs[0]);
            for &o in objs {
                if o == base {
                    continue;
                }
                let ob = &idx.objects[o];
                let bf = &idx.files[idx.objects[base].file];
                let of = &idx.files[ob.file];
                let mut f = finding(
                    Severity::Error,
                    "MISSING_OVERRIDE",
                    format!("{} {} redeclared without @Override", ob.kind, ob.name),
                    format!(
                        "{} declares {} {} again, but it already exists in {}.",
                        of.rel_path, ob.kind, ob.name, bf.rel_path
                    ),
                );
                f.nodes = vec![u.id.clone(), ob.id.clone(), idx.objects[base].id.clone()];
                f.location = Some(Location { file: ob.file, line: ob.line });
                let bo = &idx.objects[base];
                f.evidence = vec![Trail {
                    title: format!("Both files make up {}, and neither declaration is an override", u.name),
                    steps: vec![src.step(idx, bo.file, bo.line, None), src.step(idx, ob.file, ob.line, None)],
                }];
                f.suggestion = Some(Suggestion {
                    summary: "Mark the Cust declaration as an override so it layers over the base instead of clashing with it.".into(),
                    snippet: Some(format!(
                        "-- {}:{}\n+ @Override\n  {} {} …",
                        of.rel_path,
                        ob.line,
                        if ob.section.is_none() { ob.kind.to_uppercase() } else { ob.kind.clone() },
                        ob.name
                    )),
                    target: Some(of.rel_path.clone()),
                });
                out.push(f);
            }
        }
    }

    // b) Two different units feed the same definition into one client/projection.
    let mut include_adj: HashMap<&str, Vec<(&str, usize)>> = HashMap::new();
    for e in &idx.edges {
        if matches!(e.kind, EdgeKind::Include | EdgeKind::ImplicitInclude) {
            include_adj.entry(e.from.as_str()).or_default().push((e.to.as_str(), e.id));
        }
    }
    let mut reported: BTreeMap<(Vec<String>, String), (Vec<String>, Vec<usize>)> = BTreeMap::new();
    for root in idx.units.iter().filter(|u| matches!(u.kind, UnitKind::Client | UnitKind::Projection)) {
        let section = if root.kind == UnitKind::Client { Section::Client } else { Section::Projection };
        let mut reach: Vec<&str> = Vec::new();
        let mut seen: HashSet<&str> = HashSet::new();
        let mut stack = vec![root.id.as_str()];
        while let Some(u) = stack.pop() {
            if seen.insert(u) {
                reach.push(u);
                stack.extend(include_adj.get(u).into_iter().flatten().map(|&(to, _)| to));
            }
        }
        let mut defs: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for u in &reach {
            for &oi in by_unit.get(u).into_iter().flatten() {
                let o = &idx.objects[oi];
                if o.modifier.is_none() && o.section == Some(section) {
                    defs.entry(dup_key(o)).or_default().push(oi);
                }
            }
        }
        for objs in defs.values() {
            let units: BTreeSet<&str> = objs.iter().map(|&o| idx.objects[o].unit.as_str()).collect();
            if units.len() < 2 {
                continue;
            }
            let key = (units.iter().map(|s| s.to_string()).collect::<Vec<_>>(), dup_key(&idx.objects[objs[0]]));
            let entry = reported.entry(key).or_insert_with(|| (Vec::new(), objs.clone()));
            entry.0.push(root.id.clone());
        }
    }
    for ((units, _), (roots, objs)) in reported {
        let o0 = &idx.objects[objs[0]];
        let where_: Vec<String> = objs
            .iter()
            .map(|&o| {
                let ob = &idx.objects[o];
                format!("{} ({}:{})", unit(idx, &ob.unit).name, idx.files[ob.file].rel_path, ob.line)
            })
            .collect();
        let root_names: Vec<&str> = roots.iter().map(|r| unit(idx, r).name.as_str()).collect();
        let mut f = finding(
            Severity::Error,
            "DUPLICATE_DEFINITION",
            format!("{} {} defined twice", o0.kind, o0.name),
            format!(
                "{} {} is defined in {} and both reach {}. A {} takes in everything its fragments define, so the name would be there twice.",
                o0.kind,
                o0.name,
                where_.join(" and "),
                root_names.join(", "),
                if unit(idx, &roots[0]).kind == UnitKind::Client { "client" } else { "projection" }
            ),
        );
        let root = unit(idx, &roots[0]);
        let parents = include_parents(&include_adj, root.id.as_str());
        f.evidence = objs
            .iter()
            .map(|&o| {
                let ob = &idx.objects[o];
                let mut chain = Vec::new();
                let mut at = ob.unit.as_str();
                while let Some(&(prev, e)) = parents.get(at) {
                    chain.push(e);
                    at = prev;
                }
                chain.reverse();
                let mut steps: Vec<Step> = chain
                    .iter()
                    .map(|&e| {
                        let ed = &idx.edges[e];
                        let note = (ed.kind == EdgeKind::ImplicitInclude).then(|| {
                            let client = idx.files[ed.file].unit.as_deref().map(|u| unit(idx, u).name.as_str()).unwrap_or("?");
                            format!(
                                "Assumed: a fragment the client {client} includes also reaches its projection {}, which doesn't include it itself.",
                                root.name
                            )
                        });
                        src.step(idx, ed.file, ed.line, note)
                    })
                    .collect();
                steps.push(src.step(idx, ob.file, ob.line, None));
                Trail { title: format!("{} → definition in {}", root.name, unit(idx, &ob.unit).name), steps }
            })
            .collect();
        let cust = objs.iter().copied().find(|&o| idx.files[idx.objects[o].file].root == Layer::Cust).unwrap_or(objs[0]);
        let cf = &idx.files[idx.objects[cust].file];
        f.nodes = units.clone();
        f.nodes.extend(objs.iter().map(|&o| idx.objects[o].id.clone()));
        f.nodes.extend(roots.iter().cloned());
        f.location = Some(Location { file: idx.objects[cust].file, line: idx.objects[cust].line });
        f.suggestion = Some(Suggestion {
            summary: format!(
                "Keep one definition. Either drop the copy in {} and include the fragment that already defines it, or rename the Cust one (Ti prefix) if it's really a different thing.",
                unit(idx, &idx.objects[cust].unit).name
            ),
            snippet: None,
            target: Some(cf.rel_path.clone()),
        });
        out.push(f);
    }
}

/// For each unit reachable from `root` through includes, the unit and include link it was
/// first reached from (breadth first, so following them back gives the shortest chain).
fn include_parents<'a>(adj: &HashMap<&'a str, Vec<(&'a str, usize)>>, root: &'a str) -> HashMap<&'a str, (&'a str, usize)> {
    let mut parents = HashMap::new();
    let mut seen = HashSet::from([root]);
    let mut queue = VecDeque::from([root]);
    while let Some(u) = queue.pop_front() {
        for &(to, e) in adj.get(u).into_iter().flatten() {
            if seen.insert(to) {
                parents.insert(to, (u, e));
                queue.push_back(to);
            }
        }
    }
    parents
}

// ------------------------------------------------------------- overrides

fn overrides(idx: &Index, out: &mut Vec<Finding>) {
    for o in &idx.objects {
        let Some(m) = &o.modifier else { continue };
        if o.base.is_some() || o.base_generated {
            continue;
        }
        let file = &idx.files[o.file];
        let u = unit(idx, &o.unit);
        let mut f = if o.section.is_none() {
            let mut f = finding(
                Severity::Warning,
                "OVERRIDE_NO_BASE",
                format!("@{m} {} has nothing to override", o.name),
                format!(
                    "{} marks {} {} as @{m}, but {} has no such method in the indexed files.",
                    file.rel_path, o.kind, o.name, u.name
                ),
            );
            f.suggestion = Some(Suggestion {
                summary: "Check the method name and parameter list against the base package, or drop the annotation if this is a new method.".into(),
                snippet: None,
                target: Some(file.rel_path.clone()),
            });
            f
        } else {
            let mut f = finding(
                Severity::Info,
                "OVERRIDE_NO_BASE",
                format!("@{m} {} {}: base not indexed", o.kind, o.name),
                format!(
                    "No base {} {} was found in the indexed projection or fragment files. It may come from an LU's .entity model, which isn't indexed yet.",
                    o.kind, o.name
                ),
            );
            f.suggestion = Some(Suggestion {
                summary: "If the name is spelled right this is probably fine. Once .entity files are indexed this check will become exact.".into(),
                snippet: None,
                target: None,
            });
            f
        };
        f.nodes = vec![u.id.clone(), o.id.clone()];
        f.location = Some(Location { file: o.file, line: o.line });
        out.push(f);
    }
}

// ----------------------------------------------------------- connections

fn connections(idx: &Index, out: &mut Vec<Finding>) {
    let comps: HashMap<&str, &Component> = idx.components.iter().map(|c| (c.name.as_str(), c)).collect();
    let mut seen: HashSet<(usize, String, &str)> = HashSet::new();
    // What a component reaches through STATIC connections, its own and theirs: always installed
    // with it, so not missing. (25R1 Core relies on this ~19,000 times: ORDER calls ENTERP
    // through Invoic=STATIC and lists no Enterp.) A chain through a DYNAMIC one doesn't count.
    let static_reach: HashMap<&str, HashSet<&str>> = idx
        .components
        .iter()
        .map(|start| {
            let mut seen: HashSet<&str> = HashSet::new();
            let mut stack = vec![start];
            while let Some(n) = stack.pop() {
                for c in n.connections.iter().filter(|c| c.kind == "STATIC") {
                    if let Some(t) = comps.get(c.target.as_str()) {
                        if seen.insert(t.name.as_str()) {
                            stack.push(t);
                        }
                    }
                }
            }
            (start.name.as_str(), seen)
        })
        .collect();
    let reaches = |from: &str, to: &str| static_reach.get(from).is_some_and(|s| s.contains(to));

    for e in &idx.edges {
        if !matches!(e.kind, EdgeKind::Include | EdgeKind::Calls) {
            continue;
        }
        let file = &idx.files[e.file];
        let src = file.component.as_str();
        let target_unit = unit(idx, &e.to);
        let tgt = target_unit.component.as_str();
        if src == tgt {
            continue;
        }
        let Some(comp) = comps.get(src) else { continue };
        if comp.deploy_files.is_empty() {
            continue;
        }
        let deploy_path = comp
            .deploy_files
            .iter()
            .map(|&d| &idx.files[d])
            .find(|d| d.root == file.root)
            .map(|d| d.rel_path.clone())
            .unwrap_or_else(|| format!("{}/deploy.ini ({:?} folder)", src.to_ascii_lowercase(), file.root));
        let conn = comp.connections.iter().find(|c| c.target == tgt);
        let what = match e.kind {
            EdgeKind::Include => format!("includes fragment {}", target_unit.name),
            _ => format!("calls {} ({})", target_unit.name, e.detail.clone().unwrap_or_default()),
        };
        match conn {
            None if reaches(src, tgt) => {}
            None => {
                if !seen.insert((e.file, tgt.to_string(), "missing")) {
                    continue;
                }
                let kind = if e.dynamic.is_some() { "DYNAMIC" } else { "STATIC" };
                let mut f = finding(
                    Severity::Warning,
                    "CONNECTION_MISSING",
                    format!("{src} → {tgt} not declared in deploy.ini"),
                    format!("{} {what}, but {src}'s deploy.ini has no [Connections] entry for {tgt}.", file.rel_path),
                );
                f.nodes = vec![e.from.clone(), e.to.clone()];
                f.edges = vec![e.id];
                f.location = Some(Location { file: e.file, line: e.line });
                f.suggestion = Some(Suggestion {
                    summary: format!(
                        "Add {tgt} to [Connections]. Use STATIC if {src} can't work without it, DYNAMIC if it's optional (then guard the usage too)."
                    ),
                    snippet: Some(format!("-- {deploy_path}\n[Connections]\n+ {}={kind}", cap(tgt))),
                    target: Some(deploy_path),
                });
                out.push(f);
            }
            Some(c) if c.kind == "DYNAMIC" && e.dynamic.as_deref() != Some(tgt) => {
                if !seen.insert((e.file, tgt.to_string(), "dynamic")) {
                    continue;
                }
                let snippet = if e.kind == EdgeKind::Include {
                    format!(
                        "-- {}:{}\n+ @DynamicComponentDependency {tgt}\n  include fragment {};",
                        file.rel_path, e.line, target_unit.name
                    )
                } else {
                    let first = e.detail.as_deref().unwrap_or("").split(", ").next().unwrap_or("").to_string();
                    format!(
                        "-- {}:{}\n+ $IF Component_{}_SYS.INSTALLED $THEN\n     {}.{first}(…);\n+ $END\n\n-- or annotate the whole method:\n+ @DynamicComponentDependency {tgt}",
                        file.rel_path,
                        e.line,
                        cap(tgt),
                        target_unit.name
                    )
                };
                let mut f = finding(
                    Severity::Warning,
                    "DYNAMIC_NOT_GUARDED",
                    format!("Optional component {tgt} used without a guard"),
                    format!(
                        "{} {what}. {tgt} is a DYNAMIC connection of {src}, so the usage needs @DynamicComponentDependency or a Component_{}_SYS.INSTALLED check.",
                        file.rel_path,
                        cap(tgt)
                    ),
                );
                f.nodes = vec![e.from.clone(), e.to.clone()];
                f.edges = vec![e.id];
                f.location = Some(Location { file: e.file, line: e.line });
                f.suggestion = Some(Suggestion {
                    summary: format!("Guard the usage so the build works on installs without {tgt}."),
                    snippet: Some(snippet),
                    target: Some(file.rel_path.clone()),
                });
                out.push(f);
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------- layers

fn layers(idx: &Index, out: &mut Vec<Finding>) {
    for f in &idx.files {
        let Some(decl) = &f.declared_layer else { continue };
        let decl_core = decl.eq_ignore_ascii_case("core");
        let bad = match f.root {
            Layer::Core => !decl_core,
            Layer::Cust => decl_core,
        };
        if !bad {
            continue;
        }
        let mut fi = finding(
            Severity::Warning,
            "LAYER_MISMATCH",
            format!("layer {decl} file in the {:?} folder", f.root),
            format!("{} declares `layer {decl};` but sits in the {:?} folder.", f.rel_path, f.root),
        );
        fi.nodes = f.unit.iter().cloned().collect();
        fi.location = Some(Location { file: f.id, line: 1 });
        fi.suggestion = Some(Suggestion {
            summary: "Move the file to the folder that matches its layer, or fix the layer line.".into(),
            snippet: None,
            target: Some(f.rel_path.clone()),
        });
        out.push(fi);
    }
}
