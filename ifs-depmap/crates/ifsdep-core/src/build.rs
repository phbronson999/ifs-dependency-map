use crate::analyze;
use crate::datatypes;
use crate::db;
use crate::deployini::{self, DeployIni};
use crate::entity;
use crate::marble::{self, ParsedMarble, RefKind};
use crate::model::*;
use crate::plsql::{self, ParsedPlsql};
use crate::scan;
use crate::upgrade;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::rc::Rc;
use std::time::Instant;

#[derive(Default)]
pub struct IndexOptions<'a> {
    pub core: Option<&'a Path>,
    pub cust: Option<&'a Path>,
    /// The previous Core (an older checkout, or the Impact Analyzer's `Base` folder), for
    /// the upgrade comparison. Only read where a Cust customization needs it.
    pub prev_core: Option<&'a Path>,
}

enum Parsed {
    Marble(ParsedMarble),
    Plsql(ParsedPlsql),
    Deploy(DeployIni),
    Db(db::ParsedDb),
    Entity(entity::ParsedEntity),
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
                FileKind::Views | FileKind::Storage | FileKind::Cre => Parsed::Db(db::parse(&text)),
                FileKind::Entity => Parsed::Entity(entity::parse(&text)),
            };
            let (hdr_component, hdr_layer) = match &p {
                Parsed::Marble(m) => (m.component.clone(), m.layer.clone()),
                Parsed::Plsql(p) => (p.component.clone(), p.layer.clone()),
                Parsed::Deploy(d) => (d.component.clone(), None),
                Parsed::Db(d) => (None, d.layer.clone()),
                Parsed::Entity(e) => (e.component.clone(), e.layer.clone()),
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
                        generated: false,
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
                        generated: false,
                    });
                }
            }
            Parsed::Deploy(_) | Parsed::Db(_) | Parsed::Entity(_) => {}
        }
    }

    // Generated methods: from each LU's .entity, IFS generates into its Xxx_API package
    // `Get()` (the whole Public_Rec), `Get_<Attr>()` per public attribute and `Get_<Attr>_Db()`
    // per public enumeration, except for attributes with EXCLUDE_GET_METHOD. They are in no
    // source file, so they become objects pointing at the entity; a name the package declares
    // itself is the hand-written method instead. Derived attributes they return are linked to
    // what their DB_SQL_IMPLEMENTATION calls further down.
    let mut generated_reads: Vec<(usize, usize, usize)> = Vec::new(); // (object, entity file, attribute)
    {
        let mut by_lu: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for fi in 0..idx.files.len() {
            if let Parsed::Entity(entity::ParsedEntity { name: Some((lu, _)), .. }) = &parsed[fi] {
                by_lu.entry(lu.to_ascii_uppercase()).or_default().push(fi);
            }
        }
        for files in by_lu.values_mut() {
            // Core first: its attributes come before the Cust ones.
            files.sort_by_key(|&f| idx.files[f].root != Layer::Core);
            let Parsed::Entity(base) = &parsed[files[0]] else { continue };
            let (lu, lu_line) = base.name.clone().expect("grouped by name");
            let Some(&pu) = unit_map.get(&(UnitKind::Package, plsql::package_name(&lu).to_ascii_uppercase())) else { continue };
            let unit_id = idx.units[pu].id.clone();
            let declared: HashSet<String> =
                idx.objects.iter().filter(|o| o.unit == unit_id).map(|o| o.name.to_ascii_uppercase()).collect();
            let mut made: HashSet<String> = HashSet::new();
            // A per-attribute getter exists next to a hand-written method of the same name: they
            // are overloads (the getter takes the LU's keys), e.g. Customer_Order_API's
            // Get_Customer_Po_No(order_no_) and Get_Customer_Po_No(customer_po_no_, customer_no_, ...).
            // Get() itself is replaced by a hand-written one.
            let mut add = |idx: &mut Index, name: String, file: usize, line: usize, overload: bool| -> Option<usize> {
                let upper = name.to_ascii_uppercase();
                if (!overload && declared.contains(&upper)) || !made.insert(upper) {
                    return None;
                }
                let oi = idx.objects.len();
                idx.objects.push(Object {
                    id: format!("o{oi}"),
                    unit: unit_id.clone(),
                    file,
                    kind: "function".into(),
                    name,
                    line,
                    modifier: None,
                    section: None,
                    target: None,
                    dynamic: None,
                    base: None,
                    base_origin: None,
                    base_generated: false,
                    generated: true,
                });
                Some(oi)
            };
            let get = add(&mut idx, "Get".into(), files[0], lu_line, false);
            for &f in files.iter() {
                let Parsed::Entity(e) = &parsed[f] else { continue };
                for (ai, a) in e.attributes.iter().enumerate() {
                    if a.name.is_empty() || !a.public || a.exclude_get {
                        continue;
                    }
                    let getter = add(&mut idx, format!("Get_{}", plsql::snake_name(&a.name)), f, a.line, true);
                    if a.enumeration {
                        add(&mut idx, format!("Get_{}_Db", plsql::snake_name(&a.name)), f, a.line, true);
                    }
                    if a.implementation.is_some() {
                        for o in [get, getter].into_iter().flatten() {
                            generated_reads.push((o, f, ai));
                        }
                    }
                }
            }
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
        // Core code only sees Core units; a pure Cust unit with the same name is not it.
        let lookup = |kind: UnitKind, name: String| -> Option<usize> {
            unit_map.get(&(kind, name)).copied().filter(|&t| layer == Layer::Cust || idx.units[t].origin == Layer::Core)
        };
        match &parsed[fi] {
            Parsed::Marble(m) => {
                if let Some((p, line)) = &m.projection {
                    match lookup(UnitKind::Projection, p.to_ascii_uppercase()) {
                        Some(t) => push_edge(
                            &mut edges,
                            edge(from.clone(), idx.units[t].id.clone(), EdgeKind::UsesProjection, fi, *line, layer),
                        ),
                        None => unresolved.push(unresolved_finding(&idx, fi, *line, "projection", p, &from)),
                    }
                }
                for inc in &m.includes {
                    match lookup(UnitKind::Fragment, inc.name.to_ascii_uppercase()) {
                        Some(t) => {
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
                    let Some(t) = lookup(UnitKind::Package, c.package.clone()) else { continue };
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
            Parsed::Deploy(_) | Parsed::Db(_) | Parsed::Entity(_) => {}
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
    // The clients and projections each unit ends up in. A fragment can use what the
    // projection including it brings in, without including that itself.
    let mut roots_of: HashMap<usize, Vec<usize>> = HashMap::new();
    for (ui, u) in idx.units.iter().enumerate() {
        if matches!(u.kind, UnitKind::Client | UnitKind::Projection) {
            for &r in closure(ui).iter() {
                roots_of.entry(r).or_default().push(ui);
            }
        }
    }
    let mut by_name: HashMap<String, Vec<usize>> = HashMap::new();
    for (oi, o) in idx.objects.iter().enumerate() {
        by_name.entry(o.name.to_ascii_uppercase()).or_default().push(oi);
    }
    let file_root: Vec<Layer> = idx.files.iter().map(|f| f.root).collect();
    // Finds the declaration a name in `from_unit` (a file in `from`) refers to: the same unit
    // first, then what the unit can reach, then what a client or projection the unit ends up
    // in can reach. Core code only sees Core.
    //
    // For an override (`for_override`) the base may also be anywhere, as agreed for the model,
    // and when `from` is Cust a Core declaration that is itself an override counts: the Cust
    // override layers over it, not over its base. Everything else is a plain declaration.
    let resolve = |objects: &[Object],
                   closure: &mut dyn FnMut(usize) -> Rc<HashSet<usize>>,
                   name: &str,
                   accept: &dyn Fn(&Object) -> bool,
                   from_unit: usize,
                   exclude: Option<usize>,
                   from: Layer,
                   for_override: bool|
     -> Option<usize> {
        let core_overrides = for_override && from == Layer::Cust;
        let cands: Vec<usize> = by_name
            .get(&name.to_ascii_uppercase())?
            .iter()
            .copied()
            .filter(|&c| {
                let o = &objects[c];
                Some(c) != exclude
                    && (from == Layer::Cust || file_root[o.file] == Layer::Core)
                    && (o.modifier.is_none() || core_overrides && file_root[o.file] == Layer::Core)
                    && accept(o)
            })
            .collect();
        if cands.is_empty() {
            return None;
        }
        let reach = closure(from_unit);
        let roots = roots_of.get(&from_unit).map(Vec::as_slice).unwrap_or_default();
        cands
            .into_iter()
            .filter_map(|c| {
                let u = id_index(&objects[c].unit);
                let rank = if u == from_unit {
                    0
                } else if reach.contains(&u) {
                    1
                } else if roots.iter().any(|&r| closure(r).contains(&u)) {
                    2
                } else if for_override {
                    3
                } else {
                    return None;
                };
                Some((rank, c))
            })
            .min_by_key(|&(rank, _)| rank)
            .map(|(_, c)| c)
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
                    // A bound action overrides the one bound to the same entity.
                    && (c.target == o.target || !matches!(o.kind.as_str(), "action" | "function"))
            },
            from_unit,
            Some(oi),
            idx.files[o.file].root,
            true,
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

    // Database relations: tables and views, by upper-cased name. Where each is defined is
    // filled in from .views, .storage and .entity files further down.
    let mut relations: Vec<Relation> = Vec::new();
    let mut rel_map: HashMap<String, usize> = HashMap::new();
    let mut rel_id = |name: &str| -> String {
        let key = name.to_ascii_uppercase();
        let i = *rel_map.entry(key.clone()).or_insert_with(|| {
            relations.push(Relation {
                id: format!("r{}", relations.len()),
                kind: if key.ends_with("_TAB") { "table".into() } else { "view".into() },
                name: key.clone(),
                defs: Vec::new(),
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
    // Core code only sees Core packages.
    let package_method = |target: &str, from: Layer| -> Option<usize> {
        let (pkg, m) = target.split_once('.')?;
        let &u = unit_map.get(&(UnitKind::Package, pkg.to_ascii_uppercase()))?;
        if from == Layer::Core && idx.units[u].origin != Layer::Core {
            return None;
        }
        method_in(u, m)
    };

    // A generated Get() / Get_<Attr>() evaluates each derived attribute it returns: it calls
    // what the attribute's DB_SQL_IMPLEMENTATION calls (`detail` names the attribute).
    for &(oi, f, ai) in &generated_reads {
        let Parsed::Entity(e) = &parsed[f] else { continue };
        let a = &e.attributes[ai];
        let layer = idx.files[f].root;
        let code = a.implementation.as_deref().unwrap_or("").replace(['\n', '\r'], " ");
        for (pkg, method) in plsql::calls_in(&code) {
            let Some(t) = package_method(&format!("{pkg}.{method}"), layer) else { continue };
            let mut ed = edge(idx.objects[oi].id.clone(), idx.objects[t].id.clone(), EdgeKind::Invokes, f, a.line, layer);
            ed.detail = Some(a.name.clone());
            push_edge(&mut edges, ed);
        }
    }

    // References, arrays, enumerations, fetch calls, `from =` and `call X()`.
    for (oi, r) in &pending_refs {
        let from_unit = id_index(&idx.objects[*oi].unit);
        let file = idx.objects[*oi].file;
        let layer = idx.files[file].root;
        let from_id = idx.objects[*oi].id.clone();
        let (to, kind) = match r.kind {
            RefKind::Enumeration => (
                resolve(&idx.objects, &mut closure, &r.target, &|c: &Object| c.kind == "enumeration", from_unit, None, layer, false)
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
                    layer,
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
                    &|c: &Object| {
                        // An action bound to an entity is only callable from a command for that entity.
                        let entity = idx.objects[*oi].target.as_deref();
                        (c.kind == "action" || c.kind == "function")
                            && c.section == Some(Section::Projection)
                            && c.target.as_deref().is_none_or(|t| entity.is_some_and(|e| e.eq_ignore_ascii_case(t)))
                    },
                    from_unit,
                    None,
                    layer,
                    false,
                )
                .map(|t| idx.objects[t].id.clone()),
                EdgeKind::CallsOperation,
            ),
            RefKind::Call => (package_method(&r.target, layer).map(|t| idx.objects[t].id.clone()), EdgeKind::Invokes),
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
        let candidates = plsvc_for.get(&id_index(&o.unit)).cloned().unwrap_or_default();
        let hit = [plsql::snake_name(&o.name), plsql::snake_name_grouped(&o.name)].iter().find_map(|snake| {
            candidates.iter().find_map(|&s| method_in(s, &format!("{snake}___")).or_else(|| method_in(s, snake)))
        });
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
                package_method(&format!("{}.{}", c.package, c.method), layer)
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

    // Views and tables: where each is defined, and what each view reads.
    let mut rel_defs: Vec<(String, RelationDef)> = Vec::new();
    let mut cols = analyze::Columns::default();
    // Each LU's generated view reads its generated table. A Cust entity's rename wins over
    // the Core one's; otherwise the names are the defaults (LU in snake case, plus _TAB).
    let mut entities: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for fi in 0..idx.files.len() {
        if let Parsed::Entity(entity::ParsedEntity { name: Some((lu, _)), .. }) = &parsed[fi] {
            entities.entry(lu.to_ascii_uppercase()).or_default().push(fi);
        }
    }
    let ent = |fi: usize| match &parsed[fi] {
        Parsed::Entity(e) => e,
        _ => unreachable!("only entity files are grouped"),
    };
    for files in entities.values() {
        // An abstract LU has no generated table; its view is written in a .views file. The
        // property is a comma-separated list (`micro-cache-array,make-company-abstract` is not abstract).
        let is_abstract = |f: usize| ent(f).implementation.as_deref().is_some_and(|i| i.split(',').any(|t| t.trim() == "abstract"));
        if files.iter().any(|&f| is_abstract(f)) {
            continue;
        }
        let core = files.iter().copied().find(|&f| idx.files[f].root == Layer::Core);
        let cust = files.iter().copied().find(|&f| idx.files[f].root == Layer::Cust);
        let base = core.or(cust).expect("a group has at least one file");
        let pick = |view: bool| {
            [cust, core].into_iter().flatten().find_map(|f| entity_rename(ent(f), view).clone().map(|(n, l)| (n, f, l)))
        };
        let (lu, lu_line) = ent(base).name.clone().expect("grouped by name");
        // Both defaults come from the LU: renaming the view leaves the table as LU_TAB
        // (AccountingYear's view is ACC_YEAR, its table ACCOUNTING_YEAR_TAB).
        let snake = plsql::snake_name(&lu).to_ascii_uppercase();
        let (view, vfile, vline) = pick(true).unwrap_or((snake.clone(), base, lu_line));
        let (table, tfile, tline) = pick(false).unwrap_or((format!("{snake}_TAB"), base, lu_line));
        let (vid, tid) = (rel_id(&view), rel_id(&table));
        rel_defs.push((vid.clone(), RelationDef { file: vfile, line: vline, how: "generated view".into() }));
        rel_defs.push((tid.clone(), RelationDef { file: tfile, line: tline, how: "generated table".into() }));
        let mut e = edge(vid, tid, EdgeKind::Reads, tfile, tline, idx.files[tfile].root);
        e.detail = Some("generated".into());
        push_edge(&mut edges, e);

        // Columns: every attribute is a column of the view (an enumeration also `<COL>_DB`),
        // and of the table unless it is derived. A Cust attribute that Core's entity doesn't
        // have is a column Cust adds.
        let column = |name: &str| plsql::snake_name(name).to_ascii_uppercase();
        let core_cols: HashSet<String> = core.map_or(HashSet::new(), |f| ent(f).attributes.iter().map(|a| column(&a.name)).collect());
        for &f in files {
            for a in ent(f).attributes.iter().filter(|a| !a.name.is_empty()) {
                let col = column(&a.name);
                let source = analyze::ColumnSource { file: f, line: a.line, what: format!("attribute {} of {lu}", a.name) };
                let mut on = vec![(view.clone(), col.clone())];
                if a.enumeration {
                    on.push((view.clone(), format!("{col}_DB")));
                }
                if !a.derived {
                    on.push((table.clone(), col.clone()));
                }
                let added = idx.files[f].root == Layer::Cust && !core_cols.contains(&col);
                for (rel, c) in on {
                    cols.known.entry(rel.clone()).or_default().entry(c.clone()).or_insert_with(|| source.clone());
                    if added {
                        cols.added.entry(rel).or_default().push((c, source.clone()));
                    }
                }
            }
        }
    }
    let mut core_view_cols: HashMap<String, HashSet<String>> = HashMap::new();
    for fi in (0..idx.files.len()).filter(|&fi| idx.files[fi].root == Layer::Core) {
        if let Parsed::Db(d) = &parsed[fi] {
            for v in &d.views {
                if let Some(c) = &v.columns {
                    core_view_cols.entry(v.name.clone()).or_default().extend(c.iter().cloned());
                }
            }
        }
    }
    for fi in 0..idx.files.len() {
        let Parsed::Db(d) = &parsed[fi] else { continue };
        let layer = idx.files[fi].root;
        for t in &d.tables {
            let how = if t.temporary { "temporary table" } else { "table" };
            rel_defs.push((rel_id(&t.name), RelationDef { file: fi, line: t.line, how: how.into() }));
            let known = cols.known.entry(t.name.clone()).or_default();
            for c in &t.columns {
                known.entry(c.clone()).or_insert_with(|| analyze::ColumnSource { file: fi, line: t.line, what: format!("column {c} of table {}", t.name) });
            }
        }
        for v in &d.views {
            let vid = rel_id(&v.name);
            let how = v.modifier.as_ref().map_or("view".to_string(), |m| format!("view (@{m})"));
            rel_defs.push((vid.clone(), RelationDef { file: fi, line: v.line, how }));
            // A Cust @Override of a Core view adds the columns Core's version doesn't have.
            let core_cols = core_view_cols.get(&v.name);
            for c in v.columns.iter().flatten() {
                let source = analyze::ColumnSource { file: fi, line: v.line, what: format!("column {c} of view {}", v.name) };
                cols.known.entry(v.name.clone()).or_default().entry(c.clone()).or_insert_with(|| source.clone());
                if layer == Layer::Cust && v.modifier.is_some() && core_cols.is_some_and(|cc| !cc.contains(c)) {
                    cols.added.entry(v.name.clone()).or_default().push((c.clone(), source));
                }
            }
            for (r, line) in &v.reads {
                if *r != v.name {
                    push_edge(&mut edges, edge(vid.clone(), rel_id(r), EdgeKind::Reads, fi, *line, layer));
                }
            }
            let mut calls: BTreeMap<usize, (usize, usize)> = BTreeMap::new();
            for (pkg, method, line) in &v.calls {
                if let Some(t) = package_method(&format!("{pkg}.{method}"), layer) {
                    calls.entry(t).or_insert((0, *line)).0 += 1;
                }
            }
            for (t, (count, line)) in calls {
                let mut e = edge(vid.clone(), idx.objects[t].id.clone(), EdgeKind::Invokes, fi, line, layer);
                e.count = count;
                push_edge(&mut edges, e);
            }
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
        if let Some(t) = resolve(&idx.objects, &mut closure, target, &*accept, from_unit, Some(oi), idx.files[o.file].root, false) {
            push_edge(
                &mut edges,
                edge(o.id.clone(), idx.objects[t].id.clone(), kind, o.file, o.line, idx.files[o.file].root),
            );
        }
    }
    idx.edges = edges;
    for (id, d) in rel_defs {
        let r = &mut relations[id_index(&id)];
        r.kind = if d.how.contains("view") { "view" } else { "table" }.into();
        r.defs.push(d);
    }
    idx.relations = relations;

    // 6. Components and deploy.ini connections.
    let mut comps: BTreeMap<String, Component> = BTreeMap::new();
    for (fi, f) in idx.files.iter().enumerate() {
        let c = comps.entry(f.component.clone()).or_insert_with(|| Component {
            name: f.component.clone(),
            has_core: false,
            has_cust: false,
            description: None,
            deploy_files: Vec::new(),
            connections: Vec::new(),
        });
        match f.root {
            Layer::Core => c.has_core = true,
            Layer::Cust => c.has_cust = true,
        }
        if let Parsed::Deploy(d) = &parsed[fi] {
            c.deploy_files.push(fi);
            if c.description.is_none() || f.root == Layer::Core {
                c.description = d.description.clone().or(c.description.take());
            }
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
    findings.extend(analyze::run(&idx, &cols));
    let mut inputs = datatypes::Inputs { entities: Vec::new(), views: Vec::new(), tables: Vec::new() };
    for (fi, p) in parsed.iter().enumerate() {
        match p {
            Parsed::Entity(e) => inputs.entities.push((fi, e)),
            Parsed::Db(d) => {
                inputs.views.extend(d.views.iter().map(|v| (fi, v)));
                inputs.tables.extend(d.tables.iter().map(|t| (fi, t)));
            }
            _ => {}
        }
    }
    findings.extend(datatypes::run(&idx, inputs));
    if let Some(prev) = opts.prev_core {
        let mut notes = Vec::new();
        upgrade::run(&idx, prev, &mut findings, &mut notes);
        idx.notes.extend(notes);
    }
    for f in &mut findings {
        let cust_file = |fi: usize| idx.files[fi].root == Layer::Cust;
        f.involves_cust = f.location.as_ref().is_some_and(|l| cust_file(l.file))
            || f.edges.iter().any(|&e| cust_file(idx.edges[e].file))
            || f.evidence.iter().flat_map(|t| &t.steps).any(|s| cust_file(s.file))
            || f.nodes.iter().any(|n| match n.as_bytes().first() {
                Some(b'u') => idx.units[id_index(n)].origin == Layer::Cust,
                Some(b'o') => cust_file(idx.objects[id_index(n)].file),
                _ => false,
            });
    }
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

/// The entity's own view name (`DB_VIEW_NAME`) or table name (`DB_TABLE_NAME`), if it sets one.
fn entity_rename(e: &entity::ParsedEntity, view: bool) -> &Option<(String, usize)> {
    if view {
        &e.view
    } else {
        &e.table
    }
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
        evidence: vec![],
        diff: None,
        upgrade: false,
        involves_cust: false,
    }
}
