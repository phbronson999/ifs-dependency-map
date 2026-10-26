// Turns the index into what each zoom level draws. Components roll up their
// units' edges; Files shows logical units (a Core file plus its -Cust
// overlay); Objects shows declarations inside chosen units.

type Level = "components" | "units" | "objects";

interface GNode {
  id: string;           // component name, unit id or object id
  label: string;
  sub: string;
  glyph: string;        // short kind marker
  origin: Layer | "Mixed" | "Db";
  overlay: boolean;
  sev: Severity | null; // worst finding touching it
  count: number;        // findings touching it
  tier?: number;        // Objects / Components view: band (index into the graph's tiers)
  override?: boolean;   // Objects view: sits on the band's override row
  upper?: boolean;      // Objects view: sits on the band's upper row (views above the tables they read)
  group?: string;       // Objects view: the file group it is drawn in (see Graph.groups)
}

/** A labelled box around the nodes of one file (or, for tables and views, one LU). */
interface GroupInfo { label: string; sub: string; origin: Layer | "Mixed" | "Db" }

interface Tier { label: string; hint: string }

/** Objects view bands, top (what the user sees) to bottom (what runs on the server). */
const OBJECT_TIERS: Tier[] = [
  { label: "Client UI", hint: "pages, lists, groups, dialogs, commands, selectors" },
  { label: "Entry points", hint: "entitysets" },
  { label: "Data model", hint: "entities, queries, virtuals, structures, enumerations" },
  { label: "Operations", hint: "projection actions and functions" },
  { label: "Service", hint: ".plsvc methods that implement the operations" },
  { label: "PL/SQL", hint: "procedures and functions in Xxx_API packages" },
  { label: "Database", hint: "tables and views read or written (from = …, SQL)" },
];
const DB_TIER = 6;

/** Components view bands: how much of each component is Cust's. */
const COMPONENT_TIERS: Tier[] = [
  { label: "Cust components", hint: "only in the Cust folder" },
  { label: "Customized", hint: "Core components with Cust files" },
  { label: "Used by Cust", hint: "Core only, but Cust code links to them" },
  { label: "Core", hint: "Core only, not touched by Cust" },
];

/** Files view bands, one per file type, in the order a request flows through them. */
const UNIT_TIERS: Tier[] = [
  { label: "Clients", hint: ".client pages" },
  { label: "Projections", hint: ".projection services the clients use" },
  { label: "Fragments", hint: ".fragment files that clients and projections include" },
  { label: "Service", hint: ".plsvc implementations of projections" },
  { label: "PL/SQL", hint: ".plsql packages (Xxx_API)" },
];
const UNIT_TIER: Record<UnitKind, number> = { client: 0, projection: 1, fragment: 2, plsvc: 3, package: 4 };

const UNIT_EXT: Record<UnitKind, string> = {
  client: ".client", projection: ".projection", fragment: ".fragment", plsvc: ".plsvc", package: ".plsql",
};

function objectTier(o: Obj, unitKind: UnitKind): number {
  if (o.section === null) return unitKind === "plsvc" ? 4 : 5;
  if (o.section === "client") return 0;
  if (o.kind === "entityset") return 1;
  if (o.kind === "action" || o.kind === "function") return 3;
  return 2;
}

interface GEdge {
  key: string;
  from: string;
  to: string;
  kind: EdgeKind | "rollup";
  layer: Layer;
  count: number;
  label: string;
  sources: number[];    // Edge ids this line stands for
}

interface Graph {
  nodes: GNode[];
  edges: GEdge[];
  truncated: number;
  tiers?: Tier[];                   // banded views: what each GNode.tier means
  groups?: Map<string, GroupInfo>;  // Objects view: the boxes nodes are grouped in
  hidden?: number;                  // left out because Cust doesn't touch them
}

const SEV_RANK: Record<Severity, number> = { error: 0, warning: 1, info: 2 };
const UNIT_GLYPH: Record<UnitKind, string> = {
  client: "CLIENT", projection: "PROJ", fragment: "FRAG", plsvc: "PLSVC", package: "PLSQL",
};
const UNIT_EDGE_KINDS: EdgeKind[] = ["include", "uses_projection", "implicit_include", "implements", "calls"];
const OBJECT_EDGE_KINDS: EdgeKind[] = [
  "binds", "entityset_for", "reference", "array", "enumeration", "overrides",
  "calls_operation", "implemented_by", "invokes", "reads",
];
const MAX_NODES = 320;

/** Lookups computed once per index. */
class Model {
  idx: GraphIndex;
  findingsByNode = new Map<string, Finding[]>();
  findingsByComponent = new Map<string, Finding[]>();
  edgesByUnit = new Map<string, Edge[]>();
  objectsByUnit = new Map<string, Obj[]>();
  edgesByObject = new Map<string, Edge[]>();
  /**
   * What Cust touches: anything declared or defined in a Cust file, and anything a link
   * written in a Cust file points at (what Cust overrides, includes, calls or reads).
   * Holds unit, object and relation ids, and component names.
   */
  touched = new Set<string>();

  constructor(idx: GraphIndex) {
    this.idx = idx;
    for (const o of idx.objects) push(this.objectsByUnit, o.unit, o);
    for (const e of idx.edges) {
      if (e.from[0] === "u") {
        push(this.edgesByUnit, e.from, e);
        if (e.to !== e.from) push(this.edgesByUnit, e.to, e);
      } else {
        push(this.edgesByObject, e.from, e);
        push(this.edgesByObject, e.to, e);
      }
    }
    const t = this.touched;
    for (const o of idx.objects) if (idx.files[o.file].root === "Cust") t.add(o.id);
    for (const r of idx.relations) if (r.defs.some((d) => idx.files[d.file].root === "Cust")) t.add(r.id);
    for (const e of idx.edges) if (e.layer === "Cust") t.add(e.from).add(e.to);
    for (const u of idx.units) if (u.origin === "Cust" || u.has_overlay) t.add(u.id);
    for (const o of idx.objects) if (t.has(o.id)) t.add(o.unit);
    for (const u of idx.units) if (t.has(u.id)) t.add(u.component);
    for (const c of idx.components) if (c.has_cust) t.add(c.name);
    this.indexFindings(() => true);
  }

  /** The findings that pass the current filter; everything that shows findings reads these. */
  findings: Finding[] = [];

  indexFindings(keep: (f: Finding) => boolean) {
    this.findings = this.idx.findings.filter(keep);
    this.findingsByNode.clear();
    this.findingsByComponent.clear();
    for (const f of this.findings) {
      const comps = new Set<string>();
      for (const n of f.nodes) {
        push(this.findingsByNode, n, f);
        comps.add(this.componentOf(n));
      }
      for (const c of comps) push(this.findingsByComponent, c, f);
    }
  }

  unit(id: string): Unit { return this.idx.units[idIndex(id)]; }
  relation(id: string): Relation { return this.idx.relations[idIndex(id)]; }
  object(id: string): Obj { return this.idx.objects[idIndex(id)]; }
  file(i: number): SourceFile { return this.idx.files[i]; }
  componentOf(nodeId: string): string {
    if (nodeId[0] === "u") return this.unit(nodeId).component;
    if (nodeId[0] === "o") return this.unit(this.object(nodeId).unit).component;
    if (nodeId[0] === "r") return "DATABASE";
    return nodeId;
  }
  unitOf(nodeId: string): string | null {
    if (nodeId[0] === "u") return nodeId;
    if (nodeId[0] === "o") return this.object(nodeId).unit;
    return null;
  }
  worst(list: Finding[] | undefined): Severity | null {
    if (!list || !list.length) return null;
    return list.reduce<Severity>((w, f) => (SEV_RANK[f.severity] < SEV_RANK[w] ? f.severity : w), "info");
  }
  /** Units with a finding on the unit or on one of its objects. */
  unitFindings(id: string): Finding[] {
    const set = new Set(this.findingsByNode.get(id) ?? []);
    for (const o of this.objectsByUnit.get(id) ?? []) for (const f of this.findingsByNode.get(o.id) ?? []) set.add(f);
    return [...set];
  }
}

function push<K, V>(m: Map<K, V[]>, k: K, v: V) {
  const l = m.get(k);
  if (l) l.push(v);
  else m.set(k, [v]);
}

/**
 * Components in bands by how much of each is Cust's, alphabetical within a band, with
 * their deploy.ini description. `keep` drops components (the Cust filter).
 */
function componentsGraph(m: Model, keep: (id: string) => boolean = () => true): Graph {
  const unitCount = new Map<string, number>(), custUnits = new Map<string, number>();
  for (const u of m.idx.units) {
    unitCount.set(u.component, (unitCount.get(u.component) ?? 0) + 1);
    if (u.origin === "Cust" || u.has_overlay) custUnits.set(u.component, (custUnits.get(u.component) ?? 0) + 1);
  }
  const all = m.idx.components.filter((c) => unitCount.has(c.name)).sort((a, b) => a.name.localeCompare(b.name));
  const shown = all.filter((c) => keep(c.name));
  const nodes: GNode[] = shown.map((c) => {
    const n = unitCount.get(c.name)!, cu = custUnits.get(c.name) ?? 0;
    const fs = m.findingsByComponent.get(c.name);
    const tier = !c.has_core ? 0 : c.has_cust ? 1 : m.touched.has(c.name) ? 2 : 3;
    return {
      id: c.name,
      label: c.name,
      sub: `${c.description ? `${c.description} · ` : ""}${n} unit${n === 1 ? "" : "s"}${cu && c.has_core ? ` · ${cu} with Cust` : ""}`,
      glyph: "COMP",
      origin: c.has_core && c.has_cust ? "Mixed" : c.has_core ? "Core" : "Cust",
      overlay: c.has_core && c.has_cust,
      sev: m.worst(fs),
      count: fs?.length ?? 0,
      tier,
      group: `tier${tier}`,
    } as GNode;
  });
  const ids = new Set(nodes.map((n) => n.id));
  const agg = new Map<string, GEdge>();
  for (const e of m.idx.edges) {
    if (!["include", "uses_projection", "calls"].includes(e.kind)) continue;
    const a = m.unit(e.from).component, b = m.unit(e.to).component;
    if (a === b || !ids.has(a) || !ids.has(b)) continue;
    const key = `${a}>${b}`;
    let g = agg.get(key);
    if (!g) {
      g = { key, from: a, to: b, kind: "rollup", layer: "Cust", count: 0, label: "", sources: [] };
      agg.set(key, g);
    }
    g.count += e.count;
    g.sources.push(e.id);
    if (e.layer === "Core") g.layer = "Core";
  }
  const edges = [...agg.values()];
  for (const g of edges) g.label = g.count > 1 ? String(g.count) : "";
  return { nodes, edges, truncated: 0, tiers: COMPONENT_TIERS, hidden: all.length - shown.length };
}

function unitNode(m: Model, u: Unit): GNode {
  const fs = m.unitFindings(u.id);
  const files = u.files.map((f) => m.file(f));
  return {
    id: u.id,
    label: u.name,
    sub: `${u.component}${u.has_overlay ? " · +Cust" : u.origin === "Cust" ? " · pure Cust" : ""}${files.length > 1 ? ` · ${files.length} files` : ""}`,
    glyph: UNIT_GLYPH[u.kind],
    origin: u.origin,
    overlay: u.has_overlay,
    sev: m.worst(fs),
    count: fs.length,
  };
}

function unitsGraph(
  m: Model,
  opts: { components: Set<string>; focus: Set<string>; neighbors: boolean; unitKinds: Set<string>; edgeKinds: Set<string> },
  keep: (id: string) => boolean = () => true,
): Graph {
  const chosen = new Set<string>();
  for (const u of m.idx.units) {
    if (!opts.unitKinds.has(u.kind)) continue;
    if (opts.components.has(u.component) || opts.focus.has(u.id)) chosen.add(u.id);
  }
  if (!opts.components.size && !opts.focus.size) for (const u of m.idx.units) if (opts.unitKinds.has(u.kind)) chosen.add(u.id);
  if (opts.neighbors) {
    for (const id of [...chosen]) {
      for (const e of m.edgesByUnit.get(id) ?? []) {
        if (!opts.edgeKinds.has(e.kind)) continue;
        const other = e.from === id ? e.to : e.from;
        if (opts.unitKinds.has(m.unit(other).kind)) chosen.add(other);
      }
    }
  }
  let ids = [...chosen].filter(keep);
  const hidden = chosen.size - ids.length;
  let truncated = 0;
  if (ids.length > MAX_NODES) {
    truncated = ids.length - MAX_NODES;
    ids = ids.slice(0, MAX_NODES);
  }
  const shown = new Set(ids);
  // Banded by file type; within a band, one box per component.
  const groups = new Map<string, GroupInfo>();
  const nodes = ids.map((id) => {
    const u = m.unit(id);
    const group = `comp:${u.component}`;
    if (!groups.has(group)) {
      const c = m.idx.components.find((x) => x.name === u.component);
      groups.set(group, {
        label: u.component,
        sub: c?.description ?? "",
        origin: c && c.has_core && c.has_cust ? "Mixed" : c && !c.has_core ? "Cust" : "Core",
      });
    }
    return { ...unitNode(m, u), tier: UNIT_TIER[u.kind], group };
  });
  const agg = new Map<string, GEdge>();
  for (const e of m.idx.edges) {
    if (!opts.edgeKinds.has(e.kind) || !shown.has(e.from) || !shown.has(e.to)) continue;
    const key = `${e.from}>${e.to}>${e.kind}`;
    let g = agg.get(key);
    if (!g) {
      g = { key, from: e.from, to: e.to, kind: e.kind, layer: "Cust", count: 0, label: "", sources: [] };
      agg.set(key, g);
    }
    g.count += e.count;
    g.sources.push(e.id);
    if (e.layer === "Core") g.layer = "Core";
  }
  const edges = [...agg.values()];
  for (const g of edges) g.label = g.kind === "calls" && g.count > 1 ? `${g.count} calls` : "";
  return { nodes, edges, truncated, hidden, tiers: UNIT_TIERS, groups };
}

/**
 * Objects of the chosen units, then everything they depend on, followed down
 * as far as the links go (nearest first, so a cap drops the deepest), plus one
 * step up to show what uses the chosen objects.
 */
/** Where a table or view comes from, in a few words: the file that defines or generates it. */
function relationSource(m: Model, r: Relation): string {
  const d = r.defs[0];
  if (!d) return "not defined in the indexed files";
  const file = m.file(d.file).rel_path.split("/").pop();
  const more = r.defs.length > 1 ? ` +${r.defs.length - 1}` : "";
  return d.how.startsWith("generated") ? `generated from ${file}${more}` : `${file}${more}`;
}

/** The LU a table or view belongs to: the stem of the file that defines it (ShopOrder.entity, ShopOrder.views → ShopOrder). */
function relationGroup(m: Model, r: Relation): string {
  const d = r.defs.find((x) => !x.how.startsWith("generated")) ?? r.defs[0];
  if (!d) return "";
  return m.file(d.file).rel_path.split("/").pop()!.replace(/\.[^.]*$/, "").replace(/-Cust$/, "");
}

function objectsGraph(m: Model, units: Set<string>, edgeKinds: Set<string>, keep: (id: string) => boolean = () => true): Graph {
  const seeds: string[] = [];
  for (const u of units) for (const o of m.objectsByUnit.get(u) ?? []) seeds.push(o.id);
  const order: string[] = [];
  const seen = new Set<string>();
  const queue = [...seeds];
  for (const s of seeds) seen.add(s);
  for (let qi = 0; qi < queue.length; qi++) {
    const id = queue[qi];
    order.push(id);
    for (const e of m.edgesByObject.get(id) ?? []) {
      if (e.from === id && edgeKinds.has(e.kind) && !seen.has(e.to)) {
        seen.add(e.to);
        queue.push(e.to);
      }
    }
  }
  for (const s of seeds) {
    for (const e of m.edgesByObject.get(s) ?? []) {
      if (e.to === s && edgeKinds.has(e.kind) && !seen.has(e.from)) {
        seen.add(e.from);
        order.push(e.from);
      }
    }
  }
  let ids = order.filter(keep);
  const hidden = order.length - ids.length;
  let truncated = 0;
  if (ids.length > MAX_NODES) {
    truncated = ids.length - MAX_NODES;
    ids = ids.slice(0, MAX_NODES);
  }
  const shown = new Set(ids);
  const groups = new Map<string, GroupInfo>();
  const nodes: GNode[] = ids.map((id) => {
    if (id[0] === "r") {
      const r = m.relation(id);
      const stem = relationGroup(m, r);
      const group = `db:${stem}`;
      if (!groups.has(group)) {
        groups.set(group, { label: stem || "not defined in the indexed files", sub: "", origin: "Db" });
      }
      return {
        id, label: r.name, sub: relationSource(m, r), glyph: r.kind.toUpperCase(), origin: "Db", overlay: false,
        sev: null, count: 0, tier: DB_TIER, override: false, upper: r.kind === "view", group,
      };
    }
    const o = m.object(id);
    const u = m.unit(o.unit);
    const f = m.file(o.file);
    const fs = m.findingsByNode.get(id);
    if (!groups.has(u.id)) {
      groups.set(u.id, {
        label: `${u.name}${UNIT_EXT[u.kind]}`,
        sub: `${u.component} · ${u.has_overlay ? "Core + Cust" : u.origin === "Cust" ? "pure Cust" : "Core"}`,
        origin: u.has_overlay ? "Mixed" : u.origin,
      });
    }
    return {
      id,
      label: o.name,
      sub: `${u.name}${o.modifier ? ` · @${o.modifier}` : ""}${o.generated ? ` · generated from ${f.rel_path.split("/").pop()}` : ""}`,
      glyph: o.kind.toUpperCase().slice(0, 9),
      origin: f.root,
      overlay: false,
      sev: m.worst(fs),
      count: fs?.length ?? 0,
      tier: objectTier(o, u.kind),
      override: !!o.modifier,
      group: u.id,
    };
  });
  // Name the file types a table/view group's definitions come from (ShopOrder · .entity .views).
  for (const n of nodes) {
    if (n.id[0] !== "r" || !n.group) continue;
    const g = groups.get(n.group)!;
    for (const d of m.relation(n.id).defs) {
      const ext = m.file(d.file).rel_path.match(/\.[^./]*$/)?.[0] ?? "";
      if (ext && !g.sub.split(" ").includes(ext)) g.sub = (g.sub ? `${g.sub} ` : "") + ext;
    }
  }
  const edges: GEdge[] = [];
  for (const e of m.idx.edges) {
    if (e.from[0] === "u" || !edgeKinds.has(e.kind) || !shown.has(e.from) || !shown.has(e.to)) continue;
    const label =
      e.kind === "overrides" ? `@${e.detail ?? "Override"}`
      : e.kind === "invokes" && e.count > 1 ? `${e.count}×`
      : e.kind === "reads" && (e.detail === "from" || e.detail === "generated") ? e.detail
      : "";
    edges.push({ key: `${e.id}`, from: e.from, to: e.to, kind: e.kind, layer: e.layer, count: e.count, label, sources: [e.id] });
  }
  return { nodes, edges, truncated, tiers: OBJECT_TIERS, groups, hidden };
}
