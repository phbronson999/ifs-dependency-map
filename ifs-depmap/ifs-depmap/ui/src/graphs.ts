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
  tier?: number;        // Objects view: semantic band (index into OBJECT_TIERS)
  override?: boolean;   // Objects view: sits on the band's override row
}

/** Objects view bands, top (what the user sees) to bottom (what runs on the server). */
const OBJECT_TIERS = [
  { label: "Client UI", hint: "pages, lists, groups, dialogs, commands, selectors" },
  { label: "Entry points", hint: "entitysets" },
  { label: "Data model", hint: "entities, queries, virtuals, structures, enumerations" },
  { label: "Operations", hint: "projection actions and functions" },
  { label: "Service", hint: ".plsvc methods that implement the operations" },
  { label: "PL/SQL", hint: "procedures and functions in Xxx_API packages" },
  { label: "Database", hint: "tables and views read or written (from = …, SQL)" },
];
const DB_TIER = 6;

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

interface Graph { nodes: GNode[]; edges: GEdge[]; truncated: number }

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
    for (const f of idx.findings) {
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

function componentsGraph(m: Model): Graph {
  const units = m.idx.units;
  const nodes: GNode[] = m.idx.components
    .filter((c) => units.some((u) => u.component === c.name))
    .map((c) => {
      const us = units.filter((u) => u.component === c.name);
      const fs = m.findingsByComponent.get(c.name);
      return {
        id: c.name,
        label: c.name,
        sub: `${us.length} unit${us.length === 1 ? "" : "s"} · ${c.has_core && c.has_cust ? "Core + Cust" : c.has_core ? "Core" : "Cust"}`,
        glyph: "COMP",
        origin: c.has_core && c.has_cust ? "Mixed" : c.has_core ? "Core" : "Cust",
        overlay: c.has_core && c.has_cust,
        sev: m.worst(fs),
        count: fs?.length ?? 0,
      } as GNode;
    });
  const agg = new Map<string, GEdge>();
  for (const e of m.idx.edges) {
    if (!["include", "uses_projection", "calls"].includes(e.kind)) continue;
    const a = m.unit(e.from).component, b = m.unit(e.to).component;
    if (a === b) continue;
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
  return { nodes, edges, truncated: 0 };
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
  let ids = [...chosen];
  let truncated = 0;
  if (ids.length > MAX_NODES) {
    truncated = ids.length - MAX_NODES;
    ids = ids.slice(0, MAX_NODES);
  }
  const keep = new Set(ids);
  const nodes = ids.map((id) => unitNode(m, m.unit(id)));
  const agg = new Map<string, GEdge>();
  for (const e of m.idx.edges) {
    if (!opts.edgeKinds.has(e.kind) || !keep.has(e.from) || !keep.has(e.to)) continue;
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
  return { nodes, edges, truncated };
}

/**
 * Objects of the chosen units, then everything they depend on, followed down
 * as far as the links go (nearest first, so a cap drops the deepest), plus one
 * step up to show what uses the chosen objects.
 */
function objectsGraph(m: Model, units: Set<string>, edgeKinds: Set<string>): Graph {
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
  let ids = order;
  let truncated = 0;
  if (ids.length > MAX_NODES) {
    truncated = ids.length - MAX_NODES;
    ids = ids.slice(0, MAX_NODES);
  }
  const keep = new Set(ids);
  const nodes: GNode[] = ids.map((id) => {
    if (id[0] === "r") {
      const r = m.relation(id);
      return {
        id, label: r.name, sub: "not indexed yet", glyph: r.kind.toUpperCase(), origin: "Db", overlay: false,
        sev: null, count: 0, tier: DB_TIER, override: false,
      };
    }
    const o = m.object(id);
    const u = m.unit(o.unit);
    const f = m.file(o.file);
    const fs = m.findingsByNode.get(id);
    return {
      id,
      label: o.name,
      sub: `${u.name}${o.modifier ? ` · @${o.modifier}` : ""}`,
      glyph: o.kind.toUpperCase().slice(0, 9),
      origin: f.root,
      overlay: false,
      sev: m.worst(fs),
      count: fs?.length ?? 0,
      tier: objectTier(o, u.kind),
      override: !!o.modifier,
    };
  });
  const edges: GEdge[] = [];
  for (const e of m.idx.edges) {
    if (e.from[0] !== "o" || !edgeKinds.has(e.kind) || !keep.has(e.from) || !keep.has(e.to)) continue;
    const label =
      e.kind === "overrides" ? `@${e.detail ?? "Override"}`
      : e.kind === "invokes" && e.count > 1 ? `${e.count}×`
      : e.kind === "reads" && e.detail === "from" ? "from"
      : "";
    edges.push({ key: `${e.id}`, from: e.from, to: e.to, kind: e.kind, layer: e.layer, count: e.count, label, sources: [e.id] });
  }
  return { nodes, edges, truncated };
}
