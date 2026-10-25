use crate::analyze;
use crate::deployini::{self, DeployIni};
use crate::marble::{self, ParsedMarble, RefKind};
use crate::model::*;
use crate::plsql::{self, ParsedPlsql};
use crate::scan;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::rc::Rc;
use std::time::Instant;

pub struct IndexOptions<'a> {
    pub core: Option<&'a Path>,
    pub cust: Option<&'a Path>,
}

enum Parsed {
    Marble(ParsedMarble),
    Plsql(ParsedPlsql),
    Deploy(DeployIni),
}

const ENTITY_LIKE: &[&str] = &["entity", "query", "virtual", "structure", "summary", "singleton"];

/// Template methods that IFS generates from the .entity / .projection model, so an
/// `@Override` of them has a base even when no file declares one. Taken from the names
/// Core overrides with no declared base in hundreds of packages (25R1).
const GENERATED_METHODS: &[&str] = &[
    "Check_Insert___", "Insert___", "Check_Common___", "Prepare_Insert___", "Check_Update___",
    "Update___", "Check_Delete___", "Delete___", "Check_Exist___", "Unpack___", "New__", "Modify__",
    "Remove__", "Exist", "Get", "Raise_Record_Not_Exist___", "Raise_Record_Exist___",
    "Raise_Record_Access_Blocked___", "Raise_Constraint_Violated___", "Finite_State_Set___",
    "Finite_State_Machine___", "Finite_State_Add_To_Attr___", "CRUD_Create___", "CRUD_Update___",
    "CRUD_Delete___", "CRUD_Default___", "CRUD_Default_Copy___", "CRUD_Upload___",
];

/// Groups declarations that would clash if defined twice.
pub fn kind_group(kind: &str) -> &str {
    if ENTITY_LIKE.contains(&kind) {
        "entity"
    } else if kind == "procedure" || kind == "function" {
        // PL/SQL and projection functions share the word; PL/SQL objects have no section,
        // so the caller combines this with the section to tell them apart.
        "method"
    } else {
        kind
    }
}

fn strip_cust(stem: &str) -> &str {
    stem.strip_suffix("-Cust").or_else(|| stem.strip_suffix("-cust")).unwrap_or(stem)
}

pub fn index(opts: &IndexOptions) -> Index {
    let start = Instant::now();
    let mut idx = Index {
        version: 1,
        core_root: opts.core.map(|p| p.display().to_string()),
        cust_root: opts.cust.map(|p| p.display().to_string()),
        ..Default::default()
    };
    let mut parsed: Vec<Parsed> = Vec::new();

    // 1. Scan and parse both roots.
    for (root, layer) in [(opts.core, Layer::Core), (opts.cust, Layer::Cust)] {
        let Some(root) = root else { continue };
        if !root.is_dir() {
            idx.notes.push(format!("{layer:?} folder not found: {}", root.display()));
            continue;
        }
        let (files, notes) = scan::scan_root(root);
        idx.notes.extend(notes);
        for s in files {
            let text = match std::fs::read(&s.abs) {
                Ok(b) => String::from_utf8_lossy(&b).into_owned(),
                Err(e) => {
                    idx.notes.push(format!("could not read {}: {e}", s.abs.display()));
                    continue;
                }
            };
            let p = match s.kind {
                FileKind::Client | FileKind::Projection | FileKind::Fragment => {
                    Parsed::Marble(marble::parse(&text, s.kind))
                }
                FileKind::Plsvc | FileKind::Plsql => Parsed::Plsql(plsql::parse(&text)),
                FileKind::DeployIni => Parsed::Deploy(deployini::parse(&text)),
            };
            let (hdr_component, hdr_layer) = match &p {
                Parsed::Marble(m) => (m.component.clone(), m.layer.clone()),
                Parsed::Plsql(p) => (p.component.clone(), p.layer.clone()),
                Parsed::Deploy(d) => (d.component.clone(), None),
            };
            let component = hdr_component
                .or_else(|| s.path_component.clone())
                .unwrap_or_else(|| "UNKNOWN".into());
            *idx.stats.by_kind.entry(format!("{:?}", s.kind).to_lowercase()).or_default() += 1;
            idx.files.push(SourceFile {
                id: idx.files.len(),
                rel_path: s.rel,
                abs_path: s.abs.display().to_string(),
                root: layer,
                kind: s.kind,
                component,
                declared_layer: hdr_layer,
                unit: None,
                role: FileRole::Config,
                lines: text.lines().count(),
            });
            parsed.push(p);
        }
    }

    // 2. Group files into logical units.
    let mut unit_map: HashMap<(UnitKind, String), usize> = HashMap::new();
    for fi in 0..idx.files.len() {
        let f = &idx.files[fi];
        let stem = Path::new(&f.rel_path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        let base_stem = strip_cust(&stem).to_string();
        let (kind, name) = match (&parsed[fi], f.kind) {
            (Parsed::Marble(m), FileKind::Client) => (UnitKind::Client, m.name.clone().unwrap_or(base_stem)),
            (Parsed::Marble(m), FileKind::Projection) => {
                (UnitKind::Projection, m.name.clone().unwrap_or(base_stem))
            }
            (Parsed::Marble(m), FileKind::Fragment) => (UnitKind::Fragment, m.name.clone().unwrap_or(base_stem)),
            (Parsed::Plsql(_), FileKind::Plsvc) => (UnitKind::Plsvc, base_stem),
            (Parsed::Plsql(_), FileKind::Plsql) => (UnitKind::Package, plsql::package_name(&base_stem)),
            _ => continue,
        };
        let component = f.component.clone();
        let root = f.root;
        let ui = *unit_map.entry((kind, name.to_ascii_uppercase())).or_insert_with(|| {
            idx.units.push(Unit {
                id: format!("u{}", idx.units.len()),
                kind,
                name: name.clone(),
                component: component.clone(),
                origin: root,
                files: Vec::new(),
                has_overlay: false,
            });
            idx.units.len() - 1
        });
        idx.units[ui].files.push(fi);
    }
    for u in &mut idx.units {
        let core_file = u.files.iter().copied().find(|&f| idx.files[f].root == Layer::Core);
        u.origin = if core_file.is_some() { Layer::Core } else { Layer::Cust };
        if let Some(cf) = core_file {
            u.component = idx.files[cf].component.clone();
        }
        for &f in &u.files {
            let file = &mut idx.files[f];
            file.unit = Some(u.id.clone());
            file.role = if u.origin == Layer::Core && file.root == Layer::Cust {
                u.has_overlay = true;
                FileRole::Overlay
            } else {
                FileRole::Base
            };
        }
    }
    let file_unit = |idx: &Index, fi: usize| -> Option<usize> { idx.files[fi].unit.as_deref().map(id_index) };

    // 3. Objects.
    let mut pending_refs: Vec<(usize, marble::Ref)> = Vec::new();
    let mut method_objects: HashMap<(usize, usize), usize> = HashMap::new();
    for fi in 0..idx.files.len() {
        let Some(ui) = file_unit(&idx, fi) else { continue };
        let unit_id = idx.units[ui].id.clone();
        match &parsed[fi] {
            Parsed::Marble(m) => {
                for d in &m.decls {
                    let oi = idx.objects.len();
                    idx.objects.push(Object {
                        id: format!("o{oi}"),
                        unit: unit_id.clone(),
                        file: fi,
                        kind: d.kind.clone(),
                        name: d.name.clone(),
                        line: d.line,
                        modifier: d.modifier.clone(),
                        section: Some(d.section),
                        target: d.target.clone(),
                        dynamic: d.dynamic.clone(),
                        base: None,
                        base_origin: None,
                        base_generated: false,
                    });
                    for r in &d.refs {
                        pending_refs.push((oi, r.clone()));
                    }
                }
            }
            Parsed::Plsql(p) => {
                for (mi, m) in p.methods.iter().enumerate() {
                    let oi = idx.objects.len();
                    method_objects.insert((fi, mi), oi);
                    idx.objects.push(Object {
                        id: format!("o{oi}"),
                        unit: unit_id.clone(),
                        file: fi,
                        kind: m.kind.to_string(),
                        name: m.name.clone(),
                        line: m.line,
                        modifier: m.modifier.clone(),
                        section: None,
                        target: None,
                        dynamic: m.dynamic.clone(),
                        base: None,
                        base_origin: None,
                        base_generated: false,
                    });
                }
            }
            Parsed::Deploy(_) => {}
        }
    }

    // 4. Unit-level edges.
    let mut edges: Vec<Edge> = Vec::new();
    let mut unresolved: Vec<Finding> = Vec::new();
    let push_edge = |edges: &mut Vec<Edge>, e: Edge| {
        let mut e = e;
        e.id = edges.len();
        edges.push(e);
    };
    let edge = |from: String, to: String, kind: EdgeKind, file: usize, line: usize, layer: Layer| Edge {
        id: 0,
        from,
        to,
        kind,
        file,
        line,
        layer,
        dynamic: None,
        count: 1,
        detail: None,
    };

    for fi in 0..idx.files.len() {
        let Some(ui) = file_unit(&idx, fi) else { continue };
        let from = idx.units[ui].id.clone();
        let layer = idx.files[fi].root;
        match &parsed[fi] {
            Parsed::Marble(m) => {
                if let Some((p, line)) = &m.projection {
                    match unit_map.get(&(UnitKind::Projection, p.to_ascii_uppercase())) {
                        Some(&t) => push_edge(
                            &mut edges,
                            edge(from.clone(), idx.units[t].id.clone(), EdgeKind::UsesProjection, fi, *line, layer),
                        ),
                        None => unresolved.push(unresolved_finding(&idx, fi, *line, "projection", p, &from)),
                    }
                }
                for inc in &m.includes {
                    match unit_map.get(&(UnitKind::Fragment, inc.name.to_ascii_uppercase())) {
                        Some(&t) => {
                            let mut e =
                                edge(from.clone(), idx.units[t].id.clone(), EdgeKind::Include, fi, inc.line, layer);
                            e.dynamic = inc.dynamic.clone();
                            push_edge(&mut edges, e);
                        }
                        None => unresolved.push(unresolved_finding(&idx, fi, inc.line, "fragment", &inc.name, &from)),
                    }
                }
            }
            Parsed::Plsql(p) => {
                // Aggregate calls per (target package, effective guard).
                let mut agg: BTreeMap<(usize, Option<String>), (usize, usize, Vec<String>)> = BTreeMap::new();
                for c in p.calls.iter().filter(|c| !c.package.is_empty()) {
                    let Some(&t) = unit_map.get(&(UnitKind::Package, c.package.clone())) else { continue };
                    if t == ui {
                        continue;
                    }
                    let guard = c
                        .guard
                        .clone()
                        .or_else(|| c.in_method.and_then(|m| p.methods[m].dynamic.clone()));
                    let entry = agg.entry((t, guard)).or_insert((0, c.line, Vec::new()));
                    entry.0 += 1;
                    if !entry.2.contains(&c.method) {
                        entry.2.push(c.method.clone());
                    }
                }
                for ((t, guard), (count, line, methods)) in agg {
                    let mut e = edge(from.clone(), idx.units[t].id.clone(), EdgeKind::Calls, fi, line, layer);
                    e.dynamic = guard;
                    e.count = count;
                    let shown: Vec<_> = methods.iter().take(8).cloned().collect();
                    e.detail = Some(if methods.len() > 8 {
                        format!("{} …(+{})", shown.join(", "), methods.len() - 8)
                    } else {
                        shown.join(", ")
                    });
                    push_edge(&mut edges, e);
                }
            }
            Parsed::Deploy(_) => {}
        }
    }

    // Client includes also feed the paired projection.
    let mut implicit_seen: HashSet<(String, String)> = HashSet::new();
    let uses: Vec<(String, String)> = edges
        .iter()
        .filter(|e| e.kind == EdgeKind::UsesProjection)
        .map(|e| (e.from.clone(), e.to.clone()))
        .collect();
    for (client, proj) in uses {
        let incs: Vec<Edge> = edges
            .iter()
            .filter(|e| e.kind == EdgeKind::Include && e.from == client)
            .cloned()
            .collect();
        for inc in incs {
            if implicit_seen.insert((proj.clone(), inc.to.clone())) {
                let mut e = edge(proj.clone(), inc.to.clone(), EdgeKind::ImplicitInclude, inc.file, inc.line, inc.layer);
                e.dynamic = inc.dynamic.clone();
                push_edge(&mut edges, e);
            }
        }
    }
    // Projection → plsvc.
    for u in &idx.units {
        if u.kind != UnitKind::Projection {
            continue;
        }
        if let Some(&s) = unit_map.get(&(UnitKind::Plsvc, u.name.to_ascii_uppercase())) {
            let svc = &idx.units[s];
            let f = svc.files[0];
            push_edge(
                &mut edges,
                edge(u.id.clone(), svc.id.clone(), EdgeKind::Implements, f, 1, idx.files[f].root),
            );
        }
    }

    // 5. Object resolution, preferring same unit, then what the unit can reach.
    let mut adjacency: HashMap<usize, Vec<usize>> = HashMap::new();
    for e in &edges {
        if matches!(
            e.kind,
            EdgeKind::Include | EdgeKind::UsesProjection | EdgeKind::ImplicitInclude | EdgeKind::Implements
        ) {
            adjacency.entry(id_index(&e.from)).or_default().push(id_index(&e.to));
        }
    }
    // Reachable units per unit, computed on demand and shared (Rc) instead of copied.
    let mut closure_cache: HashMap<usize, Rc<HashSet<usize>>> = HashMap::new();
    let mut closure = |u: usize| -> Rc<HashSet<usize>> {
        closure_cache
            .entry(u)
            .or_insert_with(|| {
                let mut seen = HashSet::new();
                let mut stack = vec![u];
                while let Some(n) = stack.pop() {
                    if seen.insert(n) {
                        if let Some(next) = adjacency.get(&n) {
                            stack.extend(next.iter().copied());
                        }
                    }
                }
                Rc::new(seen)
            })
            .clone()
    };
    let mut by_name: HashMap<String, Vec<usize>> = HashMap::new();
    for (oi, o) in idx.objects.iter().enumerate() {
        by_name.entry(o.name.to_ascii_uppercase()).or_default().push(oi);
    }
    let file_root: Vec<Layer> = idx.files.iter().map(|f| f.root).collect();
    // Candidates are plain declarations. With `core_overrides`, a Core declaration that is
    // itself an override also counts: a Cust override layers over it, not over its base.
    let resolve = |objects: &[Object],
                   closure: &mut dyn FnMut(usize) -> Rc<HashSet<usize>>,
                   name: &str,
                   accept: &dyn Fn(&Object) -> bool,
                   from_unit: usize,
                   exclude: Option<usize>,
                   core_overrides: bool|
     -> Option<usize> {
        let cands: Vec<usize> = by_name
            .get(&name.to_ascii_uppercase())?
            .iter()
            .copied()
            .filter(|&c| {
                let o = &objects[c];
                Some(c) != exclude
                    && (o.modifier.is_none() || core_overrides && file_root[o.file] == Layer::Core)
                    && accept(o)
            })
            .collect();
        if cands.len() <= 1 {
            return cands.first().copied();
        }
        let reach = closure(from_unit);
        cands
            .into_iter()
            .min_by_key(|&c| {
                let u = id_index(&objects[c].unit);
                if u == from_unit {
                    0
                } else if reach.contains(&u) {
                    1
                } else {
                    2
                }
            })
    };

    // Overrides → base.
    for oi in 0..idx.objects.len() {
        if idx.objects[oi].modifier.is_none() {
            continue;
        }
        let o = idx.objects[oi].clone();
        let from_unit = id_index(&o.unit);
        let group = kind_group(&o.kind).to_string();
        let is_plsql = o.section.is_none();
        let in_cust = idx.files[o.file].root == Layer::Cust;
        let base = resolve(
            &idx.objects,
            &mut closure,
            &o.name,
            &|c: &Object| {
                kind_group(&c.kind) == group
                    && c.section.is_none() == is_plsql
                    && (!is_plsql || c.unit == o.unit)
                    // A Core override layers over Core or generated code, never over Cust.
                    && (in_cust || file_root[c.file] == Layer::Core)
            },
            from_unit,
            Some(oi),
            in_cust,
        );
        if base.is_none() {
            // Below Core there is only generated code, and a pure Cust package can only
            // override what its own model generates.
            let pure_cust = idx.units[from_unit].origin == Layer::Cust;
            idx.objects[oi].base_generated = !in_cust
                || is_plsql && (pure_cust || GENERATED_METHODS.contains(&o.name.as_str()));
        }
        if let Some(b) = base {
            let origin = idx.files[idx.objects[b].file].root;
            idx.objects[oi].base = Some(idx.objects[b].id.clone());
            idx.objects[oi].base_origin = Some(origin);
            let mut e = edge(
                o.id.clone(),
                idx.objects[b].id.clone(),
                EdgeKind::Overrides,
                o.file,
                o.line,
                idx.files[o.file].root,
            );
            e.detail = o.modifier.clone();
            push_edge(&mut edges, e);
        }
    }

    // Database relations (name only for now).
    let mut relations: Vec<Relation> = Vec::new();
    let mut rel_map: HashMap<String, usize> = HashMap::new();
    let mut rel_id = |name: &str| -> String {
        let key = name.to_ascii_uppercase();
        let i = *rel_map.entry(key.clone()).or_insert_with(|| {
            relations.push(Relation {
                id: format!("r{}", relations.len()),
                kind: if key.ends_with("_TAB") { "table".into() } else { "view".into() },
                name: key.clone(),
            });
            relations.len() - 1
        });
        relations[i].id.clone()
    };

    // PL/SQL methods by (package or plsvc unit, NAME). An override is the effective
    // implementation, so calls land on it and follow `Overrides` down to the base.
    let mut methods: HashMap<(usize, String), Vec<usize>> = HashMap::new();
    for (oi, o) in idx.objects.iter().enumerate() {
        if o.section.is_none() {
            methods.entry((id_index(&o.unit), o.name.to_ascii_uppercase())).or_default().push(oi);
        }
    }
    let method_in = |unit: usize, name: &str| -> Option<usize> {
        let c = methods.get(&(unit, name.to_ascii_uppercase()))?;
        // Core and Cust can both override a method; the Cust one is on top.
        let is_override = |o: usize, layer: Layer| idx.objects[o].modifier.is_some() && idx.files[idx.objects[o].file].root == layer;
        c.iter()
            .copied()
            .find(|&o| is_override(o, Layer::Cust))
            .or_else(|| c.iter().copied().find(|&o| is_override(o, Layer::Core)))
            .or_else(|| c.first().copied())
    };
    let package_method = |target: &str| -> Option<usize> {
        let (pkg, m) = target.split_once('.')?;
        let &u = unit_map.get(&(UnitKind::Package, pkg.to_ascii_uppercase()))?;
        method_in(u, m)
    };

    // References, arrays, enumerations, fetch calls, `from =` and `call X()`.
    for (oi, r) in &pending_refs {
        let from_unit = id_index(&idx.objects[*oi].unit);
        let file = idx.objects[*oi].file;
        let layer = idx.files[file].root;
        let from_id = idx.objects[*oi].id.clone();
        let (to, kind) = match r.kind {
            RefKind::Enumeration => (
                resolve(&idx.objects, &mut closure, &r.target, &|c: &Object| c.kind == "enumeration", from_unit, None, false)
                    .map(|t| idx.objects[t].id.clone()),
                EdgeKind::Enumeration,
            ),
            RefKind::Reference | RefKind::Array => (
                resolve(
                    &idx.objects,
                    &mut closure,
                    &r.target,
                    &|c: &Object| ENTITY_LIKE.contains(&c.kind.as_str()) && c.section == Some(Section::Projection),
                    from_unit,
                    None,
                    false,
                )
                .map(|t| idx.objects[t].id.clone()),
                if r.kind == RefKind::Array { EdgeKind::Array } else { EdgeKind::Reference },
            ),
            RefKind::Operation => (
                resolve(
                    &idx.objects,
                    &mut closure,
                    &r.target,
                    &|c: &Object| (c.kind == "action" || c.kind == "function") && c.section == Some(Section::Projection),
                    from_unit,
                    None,
                    false,
                )
                .map(|t| idx.objects[t].id.clone()),
                EdgeKind::CallsOperation,
            ),
            RefKind::Call => (package_method(&r.target).map(|t| idx.objects[t].id.clone()), EdgeKind::Invokes),
            RefKind::Reads => (Some(rel_id(&r.target)), EdgeKind::Reads),
        };
        if let Some(to) = to {
            let mut e = edge(from_id, to, kind, file, r.line, layer);
            e.dynamic = r.dynamic.clone();
            e.detail = Some(r.name.clone());
            push_edge(&mut edges, e);
        }
    }

    // Projection action/function → .plsvc method. A projection's own plsvc first;
    // for a fragment, its own plsvc, then those of projections that include it.
    let mut plsvc_for: HashMap<usize, Vec<usize>> = HashMap::new();
    for (ui, u) in idx.units.iter().enumerate() {
        if !matches!(u.kind, UnitKind::Projection | UnitKind::Fragment) {
            continue;
        }
        if let Some(&s) = unit_map.get(&(UnitKind::Plsvc, u.name.to_ascii_uppercase())) {
            plsvc_for.entry(ui).or_default().push(s);
        }
    }
    let projections_with_svc: Vec<(usize, usize)> = idx
        .units
        .iter()
        .enumerate()
        .filter(|(_, u)| u.kind == UnitKind::Projection)
        .filter_map(|(ui, u)| unit_map.get(&(UnitKind::Plsvc, u.name.to_ascii_uppercase())).map(|&s| (ui, s)))
        .collect();
    for (p, s) in &projections_with_svc {
        for f in closure(*p).iter() {
            if *f != *p && idx.units[*f].kind == UnitKind::Fragment {
                let v = plsvc_for.entry(*f).or_default();
                if !v.contains(s) {
                    v.push(*s);
                }
            }
        }
    }
    for oi in 0..idx.objects.len() {
        let o = &idx.objects[oi];
        if o.section != Some(Section::Projection) || !(o.kind == "action" || o.kind == "function") {
            continue;
        }
        let snake = plsql::snake_name(&o.name);
        let candidates = plsvc_for.get(&id_index(&o.unit)).cloned().unwrap_or_default();
        let hit = candidates
            .iter()
            .find_map(|&s| method_in(s, &format!("{snake}___")).or_else(|| method_in(s, &snake)));
        if let Some(t) = hit {
            let mut e = edge(o.id.clone(), idx.objects[t].id.clone(), EdgeKind::ImplementedBy, o.file, o.line, idx.files[o.file].root);
            e.detail = Some(idx.objects[t].name.clone());
            push_edge(&mut edges, e);
        }
    }

    // Method → method calls and method → table/view, aggregated per pair.
    for fi in 0..idx.files.len() {
        let Parsed::Plsql(p) = &parsed[fi] else { continue };
        let layer = idx.files[fi].root;
        let mut calls: BTreeMap<(usize, usize), (usize, usize)> = BTreeMap::new();
        for c in &p.calls {
            let Some(&from) = c.in_method.and_then(|m| method_objects.get(&(fi, m))) else { continue };
            let to = if c.package.is_empty() {
                method_in(id_index(idx.files[fi].unit.as_deref().unwrap_or("u0")), &c.method)
            } else {
                package_method(&format!("{}.{}", c.package, c.method))
            };
            let Some(to) = to else { continue };
            if to == from {
                continue;
            }
            let e = calls.entry((from, to)).or_insert((0, c.line));
            e.0 += 1;
        }
        for ((from, to), (count, line)) in calls {
            let mut e = edge(idx.objects[from].id.clone(), idx.objects[to].id.clone(), EdgeKind::Invokes, fi, line, layer);
            e.count = count;
            push_edge(&mut edges, e);
        }
        let mut reads: BTreeMap<(usize, String), (usize, usize)> = BTreeMap::new();
        for r in &p.reads {
            let Some(&from) = r.in_method.and_then(|m| method_objects.get(&(fi, m))) else { continue };
            let e = reads.entry((from, r.relation.clone())).or_insert((0, r.line));
            e.0 += 1;
        }
        for ((from, rel), (count, line)) in reads {
            let to = rel_id(&rel);
            let mut e = edge(idx.objects[from].id.clone(), to, EdgeKind::Reads, fi, line, layer);
            e.count = count;
            push_edge(&mut edges, e);
        }
    }

    // entityset for / client constructs bound to projection objects.
    for oi in 0..idx.objects.len() {
        let o = idx.objects[oi].clone();
        let Some(target) = &o.target else { continue };
        if o.section.is_none() || o.modifier.is_some() && o.kind == "entity" {
            continue; // `@Override entity X using Y` is a rename, not a binding
        }
        let from_unit = id_index(&o.unit);
        let (accept, kind): (Box<dyn Fn(&Object) -> bool>, EdgeKind) = if o.kind == "entityset" {
            (Box::new(|c: &Object| ENTITY_LIKE.contains(&c.kind.as_str())), EdgeKind::EntitysetFor)
        } else if o.kind == "entity" {
            (Box::new(|c: &Object| ENTITY_LIKE.contains(&c.kind.as_str())), EdgeKind::Binds)
        } else {
            (Box::new(|c: &Object| c.section == Some(Section::Projection)), EdgeKind::Binds)
        };
        if let Some(t) = resolve(&idx.objects, &mut closure, target, &*accept, from_unit, Some(oi), false) {
            push_edge(
                &mut edges,
                edge(o.id.clone(), idx.objects[t].id.clone(), kind, o.file, o.line, idx.files[o.file].root),
            );
        }
    }
    idx.edges = edges;
    idx.relations = relations;

    // 6. Components and deploy.ini connections.
    let mut comps: BTreeMap<String, Component> = BTreeMap::new();
    for (fi, f) in idx.files.iter().enumerate() {
        let c = comps.entry(f.component.clone()).or_insert_with(|| Component {
            name: f.component.clone(),
            has_core: false,
            has_cust: false,
            deploy_files: Vec::new(),
            connections: Vec::new(),
        });
        match f.root {
            Layer::Core => c.has_core = true,
            Layer::Cust => c.has_cust = true,
        }
        if let Parsed::Deploy(d) = &parsed[fi] {
            c.deploy_files.push(fi);
            for (target, kind, line) in &d.connections {
                c.connections.push(Connection {
                    target: target.clone(),
                    kind: kind.clone(),
                    layer: f.root,
                    file: fi,
                    line: *line,
                });
            }
        }
    }
    idx.components = comps.into_values().collect();

    // 7. Findings.
    let mut findings = unresolved;
    findings.extend(analyze::run(&idx));
    findings.sort_by(|a, b| a.severity.cmp(&b.severity).then(a.rule.cmp(&b.rule)).then(a.message.cmp(&b.message)));
    for (i, f) in findings.iter_mut().enumerate() {
        f.id = format!("f{i}");
    }
    idx.findings = findings;

    idx.stats.files = idx.files.len();
    idx.stats.units = idx.units.len();
    idx.stats.objects = idx.objects.len();
    idx.stats.edges = idx.edges.len();
    idx.stats.millis = start.elapsed().as_millis();
    idx
}

fn unresolved_finding(idx: &Index, file: usize, line: usize, what: &str, name: &str, from: &str) -> Finding {
    let f = &idx.files[file];
    Finding {
        id: String::new(),
        severity: Severity::Warning,
        rule: if what == "fragment" { "UNRESOLVED_INCLUDE" } else { "UNRESOLVED_PROJECTION" }.into(),
        title: format!("{} not found", if what == "fragment" { "Included fragment" } else { "Projection" }),
        message: format!(
            "{} refers to {what} {name}, which isn't in either indexed folder.",
            f.rel_path
        ),
        nodes: vec![from.to_string()],
        edges: vec![],
        location: Some(Location { file, line }),
        suggestion: Some(Suggestion {
            summary: format!(
                "Check the spelling, or make sure the component that owns {name} is inside the Core or Cust folder."
            ),
            snippet: None,
            target: None,
        }),
    }
}
