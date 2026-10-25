// UI controller. Runs in the Tauri window (reads folders through Rust
// commands) or in a plain browser (embedded sample data or a graph.json
// written by `ifsdep --json`).

const tauri: any = (window as any).__TAURI__;
const invoke: ((cmd: string, args?: Record<string, unknown>) => Promise<any>) | undefined = tauri?.core?.invoke;

const ALL_UNIT_KINDS: UnitKind[] = ["client", "projection", "fragment", "plsvc", "package"];
const EDGE_LABEL: Record<string, string> = {
  include: "include", uses_projection: "projection", implicit_include: "implicit include", implements: "plsvc",
  calls: "calls", reference: "reference", array: "array", enumeration: "enumeration", entityset_for: "entityset",
  binds: "binds", overrides: "override", calls_operation: "command call", implemented_by: "plsvc impl",
  invokes: "calls", reads: "reads",
};

type Depth = "direct" | "chain";

/** Width settings: how many canvas widths a band may span before it wraps. */
const BAND_WIDTHS = {
  normal: { label: "Normal", factor: 1.6 },
  wide: { label: "Wide", factor: 2.6 },
  wider: { label: "Wider", factor: 4 },
  widest: { label: "Widest", factor: 6.5 },
};

const ui = {
  model: null as Model | null,
  level: "components" as Level,
  components: new Set<string>(),
  focus: new Set<string>(),
  objUnits: new Set<string>(),
  neighbors: true,
  unitKinds: new Set<string>(ALL_UNIT_KINDS),
  edgeKinds: new Set<string>(["include", "uses_projection", "implements", "calls", ...OBJECT_EDGE_KINDS]),
  sev: new Set<Severity>(["error", "warning", "info"]),
  /** Show only the upgrade comparison's findings (what changed under each customization). */
  upgradeOnly: false,
  /** Show findings that involve only Core files. Off by default: Core can't be changed and compiles as delivered. */
  showCore: false,
  /** Selecting something either filters the graph down to it and its links, or highlights them in place. */
  selectMode: "filter" as "filter" | "highlight",
  /** How far from a selected node to go, per level: one link, or the whole chain up and down. */
  depthByLevel: { components: "direct", units: "direct", objects: "direct" } as Record<Level, Depth>,
  /** The current level's depth. */
  get depth(): Depth { return this.depthByLevel[this.level]; },
  /** Draw only what Cust touches (declared in Cust files, or overridden / used by Cust code). */
  custOnly: false,
  /** How wide a band (or the layered graph) gets before it wraps, as a multiple of the canvas width. */
  width: "normal" as keyof typeof BAND_WIDTHS,
  /** Group boxes and band extents from the last banded layout. */
  bandLayout: null as null | { groups: GroupBox[]; bands: BandBox[] },
  /** The full graph's view, kept while filtered so leaving the filter returns to it. */
  savedView: null as null | { key: string; view: { x: number; y: number; k: number } },
  /** Identifies the current filtered view ("" when not filtered). */
  filterKey: "",
  selected: null as null | { type: "finding" | "node"; id: string },
  view: { x: 0, y: 0, k: 1 },
  graph: null as Graph | null,
  placed: null as Map<string, Placed> | null,
  /** prev: the previous Core (an older checkout or the Impact Analyzer's Base folder), optional. */
  roots: { core: null as string | null, cust: null as string | null, prev: null as string | null },
  busy: false,
  loadedFile: "",
};

const $ = (id: string) => document.getElementById(id) as HTMLElement;
const esc = (s: unknown) =>
  String(s ?? "").replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]!);
const svgNS = "http://www.w3.org/2000/svg";

// ---------------------------------------------------------------- panels

const panels = { left: true, right: true };

function applyPanels() {
  const app = $("app");
  app.dataset.left = panels.left ? "open" : "closed";
  app.dataset.right = panels.right ? "open" : "closed";
  $("toggle-left").setAttribute("aria-pressed", String(panels.left));
  $("toggle-right").setAttribute("aria-pressed", String(panels.right));
}

function togglePanel(side: "left" | "right") {
  panels[side] = !panels[side];
  applyPanels();
  try { localStorage.setItem("ifsdepmap.panels", JSON.stringify(panels)); } catch { /* storage unavailable */ }
  requestAnimationFrame(() => fitView());
}

// ------------------------------------------------------------------ boot

function boot() {
  try {
    const saved = JSON.parse(localStorage.getItem("ifsdepmap.panels") ?? "null");
    if (saved) { panels.left = saved.left !== false; panels.right = saved.right !== false; }
    ui.showCore = localStorage.getItem("ifsdepmap.showCore") === "true";
    if (localStorage.getItem("ifsdepmap.selectMode") === "highlight") ui.selectMode = "highlight";
    // One depth per level; the older single setting seeds all three.
    const byLevel = JSON.parse(localStorage.getItem("ifsdepmap.depthByLevel") ?? "null");
    const old: Depth = localStorage.getItem("ifsdepmap.depth") === "chain" ? "chain" : "direct";
    for (const l of ["components", "units", "objects"] as Level[]) {
      ui.depthByLevel[l] = byLevel?.[l] === "chain" || byLevel?.[l] === "direct" ? byLevel[l] : old;
    }
    ui.custOnly = localStorage.getItem("ifsdepmap.custOnly") === "true";
    const w = localStorage.getItem("ifsdepmap.width");
    if (w && w in BAND_WIDTHS) ui.width = w as keyof typeof BAND_WIDTHS;
  } catch { /* storage unavailable */ }
  applyPanels();
  bindEvents();
  if (invoke) {
    try {
      const saved = JSON.parse(localStorage.getItem("ifsdepmap.roots") ?? "null");
      if (saved) ui.roots = { core: saved.core ?? null, cust: saved.cust ?? null, prev: saved.prev ?? null };
    } catch { /* storage unavailable */ }
    renderRoots();
    if (ui.roots.core || ui.roots.cust) reindex();
    else renderEmpty();
  } else {
    renderRoots();
    const demo = document.getElementById("demo-data");
    if (demo?.textContent?.trim()) setIndex(JSON.parse(demo.textContent));
    else renderEmpty();
  }
}

function applyScope() {
  ui.model?.indexFindings((f) => ui.showCore || f.involves_cust);
}

function setIndex(idx: GraphIndex) {
  ui.model = new Model(idx);
  applyScope();
  ui.level = "components";
  ui.components.clear();
  ui.focus.clear();
  ui.objUnits.clear();
  ui.selected = null;
  $("stats").textContent =
    `${idx.stats.files.toLocaleString()} files · ${idx.stats.units.toLocaleString()} units · ${idx.stats.edges.toLocaleString()} links · indexed in ${idx.stats.millis} ms`;
  renderRoots();
  renderAll(true);
}

async function reindex() {
  if (!invoke || ui.busy) return;
  ui.busy = true;
  $("stats").textContent = "Indexing…";
  renderRoots();
  try {
    const json: string = await invoke("index_folders", { core: ui.roots.core, cust: ui.roots.cust, prevCore: ui.roots.prev });
    setIndex(JSON.parse(json));
  } catch (e) {
    $("stats").textContent = "";
    toast(`Indexing failed: ${e}`);
  } finally {
    ui.busy = false;
    renderRoots();
  }
}

const ROOT_TITLE = { core: "Core", cust: "Cust", prev: "previous Core (an older checkout, or the Impact Analyzer's Base folder)" };

async function chooseRoot(which: "core" | "cust" | "prev") {
  if (!invoke) return;
  const path: string | null = await invoke("pick_folder", { title: `Choose the ${ROOT_TITLE[which]} folder` });
  if (!path) return;
  ui.roots[which] = path;
  try { localStorage.setItem("ifsdepmap.roots", JSON.stringify(ui.roots)); } catch { /* ignore */ }
  renderRoots();
}

// ---------------------------------------------------------------- render

function renderAll(fit = false) {
  renderFindings();
  renderToolbar();
  renderGraph(fit);
  renderDetails();
}

function renderRoots() {
  const el = $("roots");
  if (invoke) {
    const chip = (which: "core" | "cust" | "prev", tag: string, empty: string) =>
      `<span class="root-chip"><span class="tag ${which}">${tag}</span><span class="path" title="${esc(ui.roots[which] ?? "")}">${esc(ui.roots[which] ?? empty)}</span><button class="btn small" data-root="${which}">Choose</button>${
        which === "prev" && ui.roots.prev ? `<button class="btn small" data-clear-root="prev" title="Stop comparing with a previous Core" aria-label="Clear the previous Core">×</button>` : ""
      }</span>`;
    el.innerHTML = `${chip("core", "core", "not set")}${chip("cust", "cust", "not set")}${chip("prev", "previous core", "optional, for upgrades")}<button class="btn small primary" id="reindex" ${ui.busy || !(ui.roots.core || ui.roots.cust) ? "disabled" : ""}>${ui.busy ? "Indexing…" : "Index"}</button>`;
  } else {
    const idx = ui.model?.idx;
    const label = idx ? (document.getElementById("demo-data") && !ui.loadedFile ? "Sample Core + Cust test files" : ui.loadedFile) : "No data loaded";
    el.innerHTML = `<span class="root-chip"><span class="tag core">data</span><span class="path">${esc(label)}</span></span><label class="btn small" for="load-json">Load graph.json</label>`;
  }
}

function renderEmpty() {
  $("finding-list").innerHTML = `<li class="empty">No index yet.</li>`;
  $("details").innerHTML = `<div class="d"><h3>Point it at your folders</h3><p>${
    invoke
      ? "Choose the Core folder and the Cust folder at the top, then press Index. Both are optional, so you can look at Core alone."
      : "Run <span class=\"mono\">ifsdep --core &lt;dir&gt; --cust &lt;dir&gt; --json graph.json</span> and load the file with “Load graph.json”."
  }</p></div>`;
}

/** Findings the upgrade causes (only with a previous Core). */
const isUpgrade = (f: Finding) => f.upgrade;

function renderFindings() {
  const m = ui.model;
  if (!m) return;
  const counts: Record<Severity, number> = { error: 0, warning: 0, info: 0 };
  for (const f of m.findings) counts[f.severity]++;
  const names: Record<Severity, string> = { error: "Errors", warning: "Warnings", info: "Info" };
  const coreOnly = m.idx.findings.filter((f) => !f.involves_cust).length;
  const upgrades = m.findings.filter(isUpgrade).length;
  $("sev-filters").innerHTML =
    (["error", "warning", "info"] as Severity[])
      .map((s) => `<button class="sev-chip" data-sev="${s}" aria-pressed="${ui.sev.has(s)}"><span class="dot ${s}"></span>${names[s]} <b>${counts[s]}</b></button>`)
      .join("") +
    `<button class="sev-chip scope" data-scope aria-pressed="${ui.showCore}" title="Findings that involve only Core files. Core can't be changed and compiles as delivered, so these are hidden unless you turn them on."><span class="pill core">Core</span>only <b>${coreOnly.toLocaleString()}</b></button>` +
    (upgrades
      ? `<button class="sev-chip scope" data-upgrade aria-pressed="${ui.upgradeOnly}" title="Only what the upgrade causes: what IFS changed under your customizations since the previous Core, and conflicts that IFS's changes bring (like a column your Cust layer adds becoming ambiguous in a query IFS changed)"><span class="pill prev">Upgrade</span>only <b>${upgrades.toLocaleString()}</b></button>`
      : "");
  const list = m.findings.filter((f) => ui.sev.has(f.severity) && (!ui.upgradeOnly || !upgrades || isUpgrade(f)));
  const banner = ui.upgradeOnly && upgrades
    ? `<li class="list-note">Caused by the upgrade: what IFS changed under your customizations since the previous Core, and conflicts its changes bring. Other findings are hidden.</li>`
    : "";
  $("finding-list").innerHTML = banner + (list.length
    ? list
        .map((f) => {
          const loc = f.location ? m.file(f.location.file) : null;
          const where = loc ? `${loc.root} · ${loc.rel_path.split("/").pop()}:${f.location!.line}` : "";
          const sel = ui.selected?.type === "finding" && ui.selected.id === f.id ? " selected" : "";
          return `<li><button class="finding-item${sel}" data-f="${f.id}"><span class="stripe ${f.severity}"></span><span class="t">${esc(f.title)}</span><span class="rule">${esc(f.rule)}</span><span class="where">${esc(where)}</span></button></li>`;
        })
        .join("")
    : `<li class="empty">${
        !m.idx.findings.length ? "No problems found."
        : !m.findings.length ? "No findings involve Cust files. Findings about Core alone are hidden."
        : "Nothing matches the filter."
      }</li>`);
}

function renderToolbar() {
  const m = ui.model;
  for (const b of $("levels").querySelectorAll("button")) b.setAttribute("aria-pressed", String(b.dataset.level === ui.level));
  for (const b of $("selmode").querySelectorAll<HTMLElement>("button")) {
    b.setAttribute("aria-pressed", String(b.dataset.selmode ? b.dataset.selmode === ui.selectMode : b.dataset.depth === ui.depth));
  }
  for (const b of $("show").querySelectorAll<HTMLElement>("button")) b.setAttribute("aria-pressed", String((b.dataset.show === "cust") === ui.custOnly));
  const crumbs: string[] = [`<button data-crumb="components">All components</button>`];
  if (m && ui.level !== "components") {
    if (ui.components.size) {
      crumbs.push(`<span>›</span>`, ...[...ui.components].slice(0, 3).map((c) => `<button data-crumb="comp" data-comp="${esc(c)}">${esc(c)}</button>`));
      if (ui.components.size > 3) crumbs.push(`<span>+${ui.components.size - 3}</span>`);
    } else if (ui.focus.size) {
      crumbs.push(`<span>›</span><span>${ui.focus.size === 1 ? esc(m.unit([...ui.focus][0]).name) : `${ui.focus.size} files`} and neighbors</span>`);
    } else if (ui.level === "units") {
      crumbs.push(`<span>›</span><span>all files</span>`);
    }
    if (ui.level === "objects" && ui.objUnits.size) {
      const u = m.unit([...ui.objUnits][0]);
      crumbs.push(`<span>›</span><button data-crumb="comp" data-comp="${esc(u.component)}">${esc(u.component)}</button><span>›</span><span>${esc(ui.objUnits.size === 1 ? u.name : `${ui.objUnits.size} files`)}</span>`);
    }
  }
  $("crumbs").innerHTML = crumbs.join("");

  const chip = (group: string, key: string, label: string, on: boolean) =>
    `<button class="chip" data-chip="${group}" data-key="${key}" aria-pressed="${on}">${esc(label)}</button>`;
  let filters = "";
  if (ui.level === "units") {
    filters += `<div class="chipset"><span class="lbl">Kinds</span>${ALL_UNIT_KINDS.map((k) => chip("unit", k, UNIT_GLYPH[k].toLowerCase(), ui.unitKinds.has(k))).join("")}</div>`;
    filters += `<div class="chipset"><span class="lbl">Links</span>${UNIT_EDGE_KINDS.map((k) => chip("edge", k, EDGE_LABEL[k], ui.edgeKinds.has(k))).join("")}${chip("neighbors", "1", "neighbors", ui.neighbors)}</div>`;
  } else if (ui.level === "objects") {
    filters += `<div class="chipset"><span class="lbl">Links</span>${OBJECT_EDGE_KINDS.map((k) => chip("edge", k, EDGE_LABEL[k], ui.edgeKinds.has(k))).join("")}</div>`;
  }
  filters += `<div class="chipset" title="How wide a band gets before it wraps onto another line"><span class="lbl">Width</span>${
    Object.entries(BAND_WIDTHS).map(([k, w]) => chip("width", k, w.label, ui.width === k)).join("")
  }</div>`;
  // Objects are drawn by the file that declares them; files and components by where the unit comes from.
  filters += ui.level === "objects"
    ? `<div class="legend"><span><i class="core"></i>declared in Core file</span><span><i class="cust"></i>declared in Cust file</span><span><i class="db"></i>table / view</span></div>`
    : `<div class="legend"><span><i class="core"></i>Core</span><span><i class="overlay"></i>Core + Cust overlay</span><span><i class="cust"></i>pure Cust</span>${
        ui.level === "components" ? `<span>Select a component to see its links</span>` : ""
      }</div>`;
  $("filters").innerHTML = filters;
}

function nodeWidth(n: GNode) {
  const w = Math.max(n.label.length * 7.3, n.sub.length * 6.3, n.glyph.length * 6.6) + 30;
  return Math.round(Math.min(Math.max(w, 116), 330));
}

function buildGraph(): Graph {
  const m = ui.model!;
  const keep = ui.custOnly ? (id: string) => m.touched.has(id) : undefined;
  if (ui.level === "components") return componentsGraph(m, keep);
  if (ui.level === "units") {
    return unitsGraph(m, {
      components: ui.components, focus: ui.focus, neighbors: ui.neighbors, unitKinds: ui.unitKinds, edgeKinds: ui.edgeKinds,
    }, keep);
  }
  return objectsGraph(m, ui.objUnits, ui.edgeKinds, keep);
}

/** Which nodes and edges to emphasise for the current selection. */
function highlight(g: Graph) {
  const m = ui.model!;
  const nodeIds = new Set(g.nodes.map((n) => n.id));
  const hotNodes = new Set<string>(), hotEdges = new Set<string>(), selEdges = new Set<string>();
  const lit = new Set<string>();
  let selNode: string | null = null;
  if (ui.selected?.type === "finding") {
    const f = m.idx.findings.find((x) => x.id === ui.selected!.id);
    if (f) {
      for (const n of f.nodes) {
        const mapped = ui.level === "components" ? m.componentOf(n) : ui.level === "units" ? m.unitOf(n) : n;
        if (mapped && nodeIds.has(mapped)) hotNodes.add(mapped);
      }
      const fe = new Set(f.edges);
      for (const e of g.edges) {
        if (e.sources.some((s) => fe.has(s))) {
          hotEdges.add(e.key);
          lit.add(e.from).add(e.to);
        }
      }
      hotNodes.forEach((n) => lit.add(n));
    }
  } else if (ui.selected?.type === "node" && nodeIds.has(ui.selected.id)) {
    selNode = ui.selected.id;
    lit.add(selNode);
    if (ui.depth === "chain") {
      // Downstream along outgoing links and upstream along incoming ones, each all the way,
      // without turning around (which would spread into unrelated branches).
      const out = new Map<string, GEdge[]>(), inc = new Map<string, GEdge[]>();
      for (const e of g.edges) { push(out, e.from, e); push(inc, e.to, e); }
      const walk = (links: Map<string, GEdge[]>, next: (e: GEdge) => string) => {
        const queue = [selNode!], seen = new Set(queue);
        for (let i = 0; i < queue.length; i++) {
          for (const e of links.get(queue[i]) ?? []) {
            selEdges.add(e.key);
            const n = next(e);
            lit.add(n);
            if (!seen.has(n)) { seen.add(n); queue.push(n); }
          }
        }
      };
      walk(out, (e) => e.to);
      walk(inc, (e) => e.from);
    } else {
      for (const e of g.edges) {
        if (e.from === selNode || e.to === selNode) {
          selEdges.add(e.key);
          lit.add(e.from).add(e.to);
        }
      }
    }
  }
  return { hotNodes, hotEdges, selEdges, lit, selNode, dim: lit.size > 0 };
}

function edgePath(s: Placed, t: Placed, self: boolean) {
  if (self) {
    const x = s.x + s.w / 2, y = s.y;
    return { d: `M${x},${y - 10} C${x + 48},${y - 34} ${x + 48},${y + 34} ${x},${y + 10}`, mx: x + 38, my: y };
  }
  let x1 = s.x, y1: number, x2 = t.x, y2: number, c1: number, c2: number;
  if (t.y > s.y + 1) {
    y1 = s.y + s.h / 2; y2 = t.y - t.h / 2;
    const dy = (y2 - y1) / 2;
    c1 = y1 + dy; c2 = y2 - dy;
  } else if (t.y < s.y - 1) {
    y1 = s.y - s.h / 2; y2 = t.y + t.h / 2;
    const dy = (y1 - y2) / 2;
    c1 = y1 - dy; c2 = y2 + dy;
    x1 += s.w * 0.22; x2 += t.w * 0.22; // keep upward (back) links apart from downward ones
  } else {
    y1 = s.y - s.h / 2; y2 = t.y - t.h / 2;
    c1 = y1 - 46; c2 = y2 - 46;
  }
  const d = `M${x1},${y1} C${x1},${c1} ${x2},${c2} ${x2},${y2}`;
  return { d, mx: (x1 + 3 * x1 + 3 * x2 + x2) / 8, my: (y1 + 3 * c1 + 3 * c2 + y2) / 8 };
}

function renderGraph(fit = false) {
  const m = ui.model;
  const vp = $("viewport");
  if (!m) { vp.innerHTML = ""; return; }
  const full = buildGraph();
  ui.graph = full;
  const h = highlight(full);
  // Filter mode: draw only the selection and what links to it.
  const filtering = ui.selectMode === "filter" && h.dim;
  const g: Graph = filtering
    ? {
        ...full,
        nodes: full.nodes.filter((n) => h.lit.has(n.id)),
        edges: full.edges.filter((e) => h.hotEdges.has(e.key) || h.selEdges.has(e.key)),
        truncated: 0,
      }
    : full;
  const filterKey = filtering ? `${graphKey()}#${ui.selected!.type}:${ui.selected!.id}` : "";
  const prevPlaced = ui.placed, prevKey = ui.filterKey;
  // Keep the full view to return to, unless this render also changes the graph (then fit on the way back).
  if (filterKey && !prevKey) ui.savedView = fit ? null : { key: graphKey(), view: { ...ui.view } };
  // Every level is banded: components by how much is Cust's (an alphabetical grid), files by
  // file type in a box per component, objects from client to database in a box per file.
  // Filtering keeps the same bands, with only the related items in them.
  const banded = !!g.tiers;
  const maxRowWidth = Math.max(900, ($("canvas").clientWidth || 900) * BAND_WIDTHS[ui.width].factor);
  const tile = ui.level === "components";
  // The components grid stays alphabetical and draws only the selection's links (all of them
  // at once is a tangle); filtered, the links are all the selection's.
  const litEdgesOnly = tile;
  let placed: Map<string, Placed>;
  if (banded) {
    const res = layoutBands(
      g.nodes.map((n) => ({
        id: n.id, w: tile ? 236 : nodeWidth(n), h: 52, band: n.tier ?? 0,
        row: n.override || n.upper ? 0 : 1, group: n.group ?? `t${n.tier ?? 0}`,
      })),
      tile ? [] : g.edges.map((e) => ({ from: e.from, to: e.to })),
      // A component's box in the Files view may span the whole band; a file's box in the
      // Objects view wraps sooner so several files share a line.
      { maxRowWidth, maxGroupWidth: ui.level === "objects" ? maxRowWidth * 0.62 : maxRowWidth, header: tile ? 0 : 22 },
    );
    placed = res.placed;
    ui.bandLayout = { groups: res.groups, bands: res.bands };
  } else {
    placed = layoutLayered(
      g.nodes.map((n) => ({ id: n.id, w: nodeWidth(n), h: 52 })),
      g.edges.map((e) => ({ from: e.from, to: e.to })),
      { maxRowWidth },
    );
    ui.bandLayout = null;
  }
  ui.placed = placed;

  // Where to look. Entering or moving within the filter keeps the selected node where it was
  // on screen (so a double tap still lands on it), then fits the related items if some are
  // off screen. Leaving the filter returns to the view from before it.
  let viewSet = false;
  if (filterKey !== prevKey && !fit) {
    if (!filterKey) {
      if (ui.savedView?.key === graphKey()) { ui.view = { ...ui.savedView.view }; applyView(); viewSet = true; }
      else fit = true;
      ui.savedView = null;
    } else {
      const id = ui.selected!.id;
      const before = prevPlaced?.get(id), after = placed.get(id);
      if (before && after) {
        const v = ui.view;
        ui.view = { k: v.k, x: v.x + (before.x - after.x) * v.k, y: v.y + (before.y - after.y) * v.k };
        applyView();
        viewSet = true;
        window.setTimeout(() => { if (ui.filterKey === filterKey && !allVisible()) fitView(undefined, true); }, 420);
      } else fit = true;
    }
  }
  ui.filterKey = filterKey;

  const notice = $("notice");
  notice.style.top = `${(document.querySelector<HTMLElement>(".graph-toolbar")?.offsetHeight ?? 34) + 16}px`;
  if (filtering) {
    notice.hidden = false;
    const what = ui.selected!.type === "finding"
      ? "this finding"
      : full.nodes.find((n) => n.id === ui.selected!.id)?.label ?? "the selection";
    notice.className = "notice info";
    notice.textContent = `Showing ${g.nodes.length} item${g.nodes.length === 1 ? "" : "s"} ${ui.depth === "chain" && ui.selected!.type === "node" ? "in the chain through" : "linked to"} ${what}. Select it again or press Esc to see everything.`;
  } else if (full.truncated) {
    notice.hidden = false;
    notice.className = "notice";
    notice.textContent = `Showing ${full.nodes.length} of ${full.nodes.length + full.truncated}. Narrow it down with a component, a kind filter, or search.`;
  } else if (!g.nodes.length) {
    notice.className = "notice";
    notice.hidden = false;
    notice.textContent =
      ui.custOnly && full.hidden ? `Cust doesn't touch any of the ${full.hidden} items here. Switch Show to All code to see them.`
      : ui.level === "objects" ? "Pick a file first: double-tap one in the Files view, or search for it."
      : "Nothing to show with these filters.";
  } else notice.hidden = true;

  let edgesSvg = "";
  for (const e of g.edges) {
    const s = placed.get(e.from), t = placed.get(e.to);
    if (!s || !t) continue;
    const hot = h.hotEdges.has(e.key), sel = h.selEdges.has(e.key);
    if (litEdgesOnly && !hot && !sel) continue;
    const { d, mx, my } = edgePath(s, t, e.from === e.to);
    const marker = hot ? "m-hot" : sel ? "m-sel" : e.layer === "Cust" ? "m-Cust" : "m-default";
    const cls = `edge kind-${e.kind} layer-${e.layer}${hot ? " is-hot is-lit" : ""}${sel ? " is-sel is-lit" : ""}`;
    edgesSvg += `<g class="${cls}" data-edge="${esc(e.key)}"><path class="line" d="${d}" marker-end="url(#${marker})"/>${
      e.label ? `<text x="${mx + 6}" y="${my}">${esc(e.label)}</text>` : ""
    }</g>`;
  }
  let nodesSvg = "";
  for (const n of g.nodes) {
    const p = placed.get(n.id)!;
    const x = p.x - p.w / 2, y = p.y - p.h / 2;
    const label = n.label.length > 42 ? n.label.slice(0, 41) + "…" : n.label;
    const subMax = Math.floor((p.w - 22) / 6.3);
    const sub = n.sub.length > subMax ? n.sub.slice(0, subMax - 1) + "…" : n.sub;
    const cls = `node origin-${n.origin}${h.hotNodes.has(n.id) ? " is-hot" : ""}${h.selNode === n.id ? " is-sel" : ""}${h.lit.has(n.id) ? " is-lit" : ""}`;
    nodesSvg += `<g class="${cls}" data-node="${esc(n.id)}" transform="translate(${x},${y})">
<title>${esc(n.label)}${n.sub ? `\n${esc(n.sub)}` : ""}</title>
<rect class="box" width="${p.w}" height="${p.h}" rx="6"/>${n.overlay ? `<rect class="tab" x="${p.w - 6}" y="5" width="3" height="${p.h - 10}" rx="1.5"/>` : ""}
<text class="glyph" x="10" y="15">${esc(n.glyph)}</text>
<text x="10" y="32">${esc(label)}</text>
<text class="sub" x="10" y="45">${esc(sub)}</text>${
      n.count ? `<g class="badge ${n.sev}" transform="translate(${p.w - 2},2)"><circle r="9"/><text text-anchor="middle" y="3.5">${n.count > 99 ? "99" : n.count}</text></g>` : ""
    }</g>`;
  }
  vp.innerHTML = `${banded ? bandsSvg(g, placed, ui.bandLayout!) : ""}<g class="edges">${edgesSvg}</g><g class="nodes">${nodesSvg}</g>`;
  $("canvas").classList.toggle("dimmed", h.dim && !filtering);
  renderJump(banded ? g : null);
  if (fit) fitView();
  else if (!viewSet) applyView();
}

/** What the graph currently shows, apart from the selection. */
function graphKey() {
  return [ui.level, ui.custOnly, [...ui.components].join(","), [...ui.focus].join(","), [...ui.objUnits].join(",")].join("|");
}

/** The Objects view's bands that are on screen, as buttons that zoom to each (and to its override row). */
function renderJump(g: Graph | null) {
  const el = $("jump");
  const tiers = new Map<number, { all: number; over: number }>();
  for (const n of g?.nodes ?? []) {
    const t = tiers.get(n.tier!) ?? { all: 0, over: 0 };
    t.all++;
    if (n.override) t.over++;
    tiers.set(n.tier!, t);
  }
  el.hidden = tiers.size < 2 || !g?.tiers;
  if (el.hidden) { el.innerHTML = ""; return; }
  const info = g!.tiers!;
  el.innerHTML = `<span class="lbl">Jump to</span>${[...tiers.entries()]
    .sort((a, b) => a[0] - b[0])
    .map(([tier, t]) =>
      `<button class="chip" data-jump="${tier}" title="${esc(info[tier].hint)}">${esc(info[tier].label)} <b>${t.all}</b></button>${
        t.over && t.over < t.all ? `<button class="chip sub" data-jump="${tier}o" title="The overrides in this band, on the row above their bases">@Override <b>${t.over}</b></button>` : ""
      }`)
    .join("")}`;
}

/** Zooms to one band ("3") or to the override row of a band ("3o"). */
function jumpTo(key: string) {
  const g = ui.graph;
  if (!g) return;
  const tier = parseInt(key, 10), overOnly = key.endsWith("o");
  const ids = new Set(g.nodes.filter((n) => n.tier === tier && (!overOnly || n.override) && ui.placed?.has(n.id)).map((n) => n.id));
  if (ids.size) fitView(ids, true);
}

function deselect() {
  if (!ui.selected) return;
  ui.selected = null;
  renderFindings();
  renderGraph();
  renderDetails();
}

/**
 * Labelled horizontal bands behind a banded view, one per tier present, and inside them a
 * labelled box per file group (Objects view), split between its override row and its base row.
 */
function bandsSvg(g: Graph, placed: Map<string, Placed>, layout: { groups: GroupBox[]; bands: BandBox[] }) {
  if (!g.nodes.length || !g.tiers) return "";
  let x0 = Infinity, x1 = -Infinity;
  for (const p of placed.values()) { x0 = Math.min(x0, p.x - p.w / 2); x1 = Math.max(x1, p.x + p.w / 2); }
  for (const b of layout.groups) { x0 = Math.min(x0, b.x); x1 = Math.max(x1, b.x + b.w); }
  x0 -= 28; x1 += 28;
  let out = "";
  layout.bands.forEach((band, i) => {
    const info = g.tiers![band.band];
    const top = band.top - 36, bottom = band.bottom + 14;
    out += `<g class="band${i % 2 ? " alt" : ""}"><rect x="${x0}" y="${top}" width="${x1 - x0}" height="${bottom - top}"/>
<line x1="${x0}" x2="${x1}" y1="${top}" y2="${top}"/>
<text class="bl" data-band="${band.band}" x="${x0 + 12}" y="${top + 17}"><title>Zoom to ${esc(info.label)}</title>${esc(info.label.toUpperCase())}</text>
<text class="bh" x="${x0 + 14 + info.label.length * 8.2}" y="${top + 17}">${esc(info.hint)}</text></g>`;
  });
  for (const b of layout.groups) {
    const info = g.groups?.get(b.group);
    if (!info) continue;
    // Label, then the component / file types after it, cut to the box.
    const room = Math.floor((b.w - 20) / 6.6);
    const label = info.label.length > room ? info.label.slice(0, room - 1) + "…" : info.label;
    const subRoom = room - label.length - 2;
    const sub = subRoom > 4 ? (info.sub.length > subRoom ? info.sub.slice(0, subRoom - 1) + "…" : info.sub) : "";
    out += `<g class="grp origin-${info.origin}"><title>${esc(info.label)}${info.sub ? ` · ${esc(info.sub)}` : ""}</title>
<rect x="${b.x}" y="${b.y}" width="${b.w}" height="${b.h}" rx="8"/>
<text class="gl" x="${b.x + 10}" y="${b.y + 15}">${esc(label)}<tspan class="gs" dx="8">${esc(sub)}</tspan></text>${
      b.split !== null
        ? `<line class="split" x1="${b.x + 8}" x2="${b.x + b.w - 8}" y1="${b.split}" y2="${b.split}"/><text class="sl" data-band="${b.band}o" x="${b.x + b.w - 8}" y="${b.split - 3}" text-anchor="end"><title>Zoom to the overrides in ${esc(g.tiers[b.band].label)}</title>@Override ↓ base</text>`
        : ""
    }</g>`;
  }
  return `<g class="bands">${out}</g>`;
}

/** The part of the canvas not covered by the toolbar above and the filter bar below. */
function clearArea() {
  const svg = $("canvas") as unknown as SVGSVGElement;
  const W = svg.clientWidth || 800, H = svg.clientHeight || 600;
  const top = (document.querySelector<HTMLElement>(".graph-toolbar")?.offsetHeight ?? 40) + 18;
  const bottom = ($("filters").offsetHeight || 40) + 20;
  return { W, H, top, availH: Math.max(120, H - top - bottom) };
}

/** Fits everything drawn, or just `ids`, into view; `animate` glides there instead of jumping. */
function fitView(ids?: Set<string>, animate = false) {
  const placed = ui.placed;
  const { W, H, top, availH } = clearArea();
  if (!placed || !placed.size) { ui.view = { x: W / 2, y: H / 2, k: 1 }; applyView(); return; }
  let x0 = Infinity, y0 = Infinity, x1 = -Infinity, y1 = -Infinity;
  for (const [id, p] of placed) {
    if (ids && !ids.has(id)) continue;
    x0 = Math.min(x0, p.x - p.w / 2 - 30); x1 = Math.max(x1, p.x + p.w / 2 + 40);
    y0 = Math.min(y0, p.y - p.h / 2 - 50); y1 = Math.max(y1, p.y + p.h / 2);
  }
  if (x0 === Infinity) return;
  const k = Math.min((W - 32) / (x1 - x0), availH / (y1 - y0), 1.3);
  const target = { k, x: W / 2 - ((x0 + x1) / 2) * k, y: top + availH / 2 - ((y0 + y1) / 2) * k };
  if (animate) animateView(target);
  else { ui.view = target; applyView(); }
}

/** True when every drawn node is inside the clear area of the canvas. */
function allVisible() {
  const { W, top, availH } = clearArea();
  const v = ui.view;
  for (const p of ui.placed?.values() ?? []) {
    const x = p.x * v.k + v.x, y = p.y * v.k + v.y, hw = (p.w / 2) * v.k, hh = (p.h / 2) * v.k;
    if (x - hw < 0 || x + hw > W || y - hh < top || y + hh > top + availH) return false;
  }
  return true;
}

let viewAnimation = 0;
/** Glides from the current view to `target` (about a quarter of a second). */
function animateView(target: { x: number; y: number; k: number }) {
  const from = { ...ui.view };
  const run = ++viewAnimation;
  const t0 = performance.now();
  const step = (now: number) => {
    if (run !== viewAnimation) return;
    const t = Math.min(1, (now - t0) / 260);
    const e = 1 - Math.pow(1 - t, 3);
    ui.view = { x: from.x + (target.x - from.x) * e, y: from.y + (target.y - from.y) * e, k: from.k + (target.k - from.k) * e };
    applyView();
    if (t < 1) requestAnimationFrame(step);
  };
  requestAnimationFrame(step);
}

function applyView() {
  const v = ui.view;
  $("viewport").setAttribute("transform", `translate(${v.x},${v.y}) scale(${v.k})`);
}

function zoomAt(factor: number, px: number, py: number) {
  viewAnimation++;
  const v = ui.view;
  const k = Math.min(3, Math.max(0.08, v.k * factor));
  v.x = px - (px - v.x) * (k / v.k);
  v.y = py - (py - v.y) * (k / v.k);
  v.k = k;
  applyView();
}

// --------------------------------------------------------------- details

function fileLine(fi: number, line: number | null, label?: string) {
  const m = ui.model!;
  const f = m.file(fi);
  const pill = f.root === "Core" ? `<span class="pill core">Core</span>` : `<span class="pill cust">Cust</span>`;
  return `<div class="fileline">${pill}<span class="p">${esc(label ?? f.rel_path)}${line ? `:${line}` : ""}</span><button class="btn small" data-action="open" data-file="${fi}" data-line="${line ?? 1}">${invoke ? "Open" : "Copy"}</button></div>`;
}

function snippetHtml(s: string) {
  return s
    .split("\n")
    .map((l) => {
      const cls = l.startsWith("+") ? "add" : l.startsWith("- ") ? "del" : l.trimStart().startsWith("--") ? "cmt" : "";
      return cls ? `<span class="${cls}">${esc(l)}</span>` : `${esc(l)}\n`;
    })
    .join("");
}

function nodeLink(id: string) {
  const m = ui.model!;
  if (id[0] === "u") {
    const u = m.unit(id);
    return `<button data-action="goto" data-node="${id}"><span class="k">${UNIT_GLYPH[u.kind].toLowerCase()}</span><span class="n">${esc(u.name)}</span></button>`;
  }
  if (id[0] === "o") {
    const o = m.object(id);
    return `<button data-action="goto" data-node="${id}"><span class="k">${esc(o.kind)}</span><span class="n">${esc(o.name)} <span class="mono" style="color:var(--muted)">in ${esc(m.unit(o.unit).name)}</span></span></button>`;
  }
  if (id[0] === "r") {
    const r = m.relation(id);
    return `<button data-action="goto" data-node="${id}"><span class="k">${r.kind}</span><span class="n">${esc(r.name)}</span></button>`;
  }
  return `<button data-action="goto" data-node="${esc(id)}"><span class="k">component</span><span class="n">${esc(id)}</span></button>`;
}

function findingLinks(list: Finding[]) {
  if (!list.length) return "";
  return `<section><h4>Findings</h4><div class="linklist">${list
    .map((f) => `<button data-action="finding" data-f="${f.id}"><span class="k"><span class="dot ${f.severity}" style="display:inline-block"></span> ${esc(f.rule)}</span><span class="n">${esc(f.title)}</span></button>`)
    .join("")}</div></section>`;
}

function renderDetails() {
  const m = ui.model;
  const el = $("details");
  if (!m) return;
  const sel = ui.selected;
  if (!sel) { el.innerHTML = overviewHtml(m); $("details-head").textContent = "Overview"; return; }
  if (sel.type === "finding") {
    const f = m.idx.findings.find((x) => x.id === sel.id);
    el.innerHTML = f ? findingHtml(m, f) : "";
    $("details-head").textContent = "Finding";
    return;
  }
  const id = sel.id;
  $("details-head").textContent = id[0] === "u" ? "File" : id[0] === "o" ? "Object" : id[0] === "r" ? "Table / view" : "Component";
  el.innerHTML =
    id[0] === "u" ? unitHtml(m, id) : id[0] === "o" ? objectHtml(m, id) : id[0] === "r" ? relationHtml(m, id) : componentHtml(m, id);
}

function overviewHtml(m: Model) {
  const s = m.idx.stats;
  const kinds = Object.entries(s.by_kind).map(([k, v]) => `<dt>.${esc(k === "deployini" ? "ini" : k)}</dt><dd>${v.toLocaleString()}</dd>`).join("");
  const errors = m.findings.filter((f) => f.severity === "error").length;
  return `<div class="d">
<h3>${errors ? `${errors} thing${errors === 1 ? "" : "s"} to fix` : "No blocking problems"}</h3>
<p>Each box is a component. Lines are includes, projection use and PL/SQL calls that cross components; the number is how many. Double-tap a component to see its files, and a file to see what it declares.</p>
<section><h4>Indexed</h4><dl class="kv">${kinds}<dt>units</dt><dd>${s.units.toLocaleString()}</dd><dt>objects</dt><dd>${s.objects.toLocaleString()}</dd></dl></section>
${m.idx.core_root || m.idx.cust_root ? `<section><h4>Folders</h4><dl class="kv"><dt>Core</dt><dd class="mono">${esc(m.idx.core_root ?? "—")}</dd><dt>Cust</dt><dd class="mono">${esc(m.idx.cust_root ?? "—")}</dd></dl></section>` : ""}
${m.idx.notes.length ? `<section><h4>Notes</h4>${m.idx.notes.slice(0, 20).map((n) => `<p class="mono">${esc(n)}</p>`).join("")}</section>` : ""}
</div>`;
}

/** The chains of source lines behind a finding, in order, each line openable. */
function evidenceHtml(f: Finding) {
  if (!f.evidence.length) return "";
  const m = ui.model!;
  const step = (s: Step) => {
    const file = m.file(s.file);
    const pill = file.root === "Core" ? `<span class="pill core">Core</span>` : `<span class="pill cust">Cust</span>`;
    const name = `${file.component.toLowerCase()} · ${file.rel_path.split("/").pop()}:${s.line}`;
    return `<li class="step${s.note ? " assumed" : ""}">
<div class="step-head">${pill}<span class="p" title="${esc(file.rel_path)}">${esc(name)}</span><button class="btn small" data-action="open" data-file="${s.file}" data-line="${s.line}">${invoke ? "Open" : "Copy"}</button></div>
<code>${esc(s.text)}</code>${s.note ? `<p class="note">${esc(s.note)}</p>` : ""}</li>`;
  };
  const trails = f.evidence
    .map((t) => `<div class="trail"><div class="trail-title">${esc(t.title)}</div><ol>${t.steps.map(step).join("")}</ol></div>`)
    .join("");
  return `<section><h4>How this was found</h4>${trails}</section>`;
}

/** Upgrade findings: the Core declaration before and now, as a diff with both line numbers. */
function diffHtml(f: Finding) {
  const d = f.diff;
  if (!d) return "";
  const m = ui.model!;
  const now = m.file(d.new_file);
  // Under an @Overtake, a column says whether your copy has each changed line.
  const yours = d.lines.some((l) => l.in_cust !== null);
  const mark = (l: DiffLine) =>
    l.in_cust === null ? ""
    : l.op === "+" ? (l.in_cust ? `<span class="yes" title="Your copy has this line">✓ has</span>` : `<span class="no" title="Your copy doesn't have this line IFS added">✗ missing</span>`)
    : (l.in_cust ? `<span class="no" title="Your copy still has this line IFS removed">✗ still there</span>` : `<span class="yes" title="Your copy doesn't have this line either">✓ gone</span>`);
  const rows = d.lines
    .map((l) => {
      if (l.op === "@") return `<tr class="gap"><td colspan="${yours ? 4 : 3}">${esc(l.text)}</td></tr>`;
      const cls = l.op === "+" ? "add" : l.op === "-" ? "del" : l.op === "~" ? "mod" : "ctx";
      return `<tr class="${cls}"><td class="n">${l.old ?? ""}</td><td class="n">${l.new ?? ""}</td>${yours ? `<td class="y">${mark(l)}</td>` : ""}<td class="t"><span class="op">${l.op === " " ? "" : esc(l.op)}</span>${esc(l.text)}</td></tr>`;
    })
    .join("");
  const name = (p: string) => p.split(/[\\/]/).pop();
  const cust = d.cust_file !== null ? m.file(d.cust_file) : null;
  return `<section><h4>What changed in Core</h4>
<div class="fileline"><span class="pill prev">Before</span><span class="p" title="${esc(d.old_path)}">${esc(name(d.old_path))}:${d.old_line}</span><button class="btn small" data-action="open-path" data-path="${esc(d.old_path)}" data-line="${d.old_line}">${invoke ? "Open" : "Copy"}</button></div>
<div class="fileline"><span class="pill core">Now</span><span class="p" title="${esc(now.rel_path)}">${esc(name(now.rel_path))}:${d.new_line}</span><button class="btn small" data-action="open" data-file="${d.new_file}" data-line="${d.new_line}">${invoke ? "Open" : "Copy"}</button></div>
<div class="diff"><table><thead><tr><th>before</th><th>now</th>${yours ? "<th>your copy</th>" : ""}<th></th></tr></thead><tbody>${rows}</tbody></table></div></section>${
    cust
      ? `<section><h4>Your customization</h4>${fileLine(d.cust_file!, d.cust_line, cust.rel_path.split("/").pop())}<pre class="cust-code">${d.cust_code.map(esc).join("\n")}</pre></section>`
      : ""
  }`;
}

function findingHtml(m: Model, f: Finding) {
  const loc = f.location ? fileLine(f.location.file, f.location.line) : "";
  const involved = [...new Set(f.nodes)].map(nodeLink).join("");
  const sug = f.suggestion;
  return `<div class="d">
<div class="eyebrow"><span class="pill ${f.severity}">${f.severity}</span><span>${esc(f.rule)}</span></div>
<h3>${esc(f.title)}</h3>
<p>${esc(f.message)}</p>
${loc ? `<section><h4>Where</h4>${loc}</section>` : ""}
${diffHtml(f)}
${evidenceHtml(f)}
${sug ? `<section><h4>Suggested fix</h4><p class="callout">${esc(sug.summary)}</p>${sug.snippet ? `<pre class="snippet">${snippetHtml(sug.snippet)}</pre>` : ""}${sug.target ? `<p class="mono" style="color:var(--muted)">in ${esc(sug.target)}</p>` : ""}</section>` : ""}
<section><h4>Involved</h4><div class="linklist">${involved}</div></section>
</div>`;
}

function componentHtml(m: Model, name: string) {
  const c = m.idx.components.find((x) => x.name === name);
  if (!c) return "";
  const units = m.idx.units.filter((u) => u.component === name);
  const byKind = ALL_UNIT_KINDS.map((k) => [k, units.filter((u) => u.kind === k).length] as const).filter(([, n]) => n);
  const conns = c.connections.length
    ? `<div style="overflow-x:auto"><table class="conn"><thead><tr><th>Component</th><th>Type</th><th>Declared in</th></tr></thead><tbody>${c.connections
        .map((x) => `<tr><td class="mono">${esc(x.target)}</td><td>${esc(x.kind)}</td><td><span class="pill ${x.layer === "Core" ? "core" : "cust"}">${x.layer}</span></td></tr>`)
        .join("")}</tbody></table></div>`
    : `<p style="color:var(--muted)">${c.deploy_files.length ? "No [Connections] entries." : "No deploy.ini found for this component."}</p>`;
  return `<div class="d">
<div class="eyebrow"><span>component</span>${c.has_core ? `<span class="pill core">Core</span>` : ""}${c.has_cust ? `<span class="pill cust">Cust</span>` : ""}</div>
<h3>${esc(name)}</h3>
<dl class="kv">${byKind.map(([k, n]) => `<dt>${UNIT_GLYPH[k].toLowerCase()}</dt><dd>${n}</dd>`).join("")}</dl>
<div><button class="btn" data-action="drill" data-node="${esc(name)}">Show its files</button></div>
<section><h4>deploy.ini connections</h4>${conns}</section>
${c.deploy_files.length ? `<section><h4>deploy.ini</h4><div class="filelist">${c.deploy_files.map((f) => fileLine(f, 1)).join("")}</div></section>` : ""}
${findingLinks(m.findingsByComponent.get(name) ?? [])}
</div>`;
}

function unitHtml(m: Model, id: string) {
  const u = m.unit(id);
  const origin = u.origin === "Core" ? (u.has_overlay ? "Core with a Cust overlay" : "Core") : "pure Cust (no Core file)";
  const files = u.files.map((fi) => {
    const f = m.file(fi);
    return fileLine(fi, null, `${f.rel_path}${f.role === "overlay" ? "  (overlay)" : ""}`);
  }).join("");
  const edges = m.edgesByUnit.get(id) ?? [];
  const group = (dir: "out" | "in") => {
    const rows = edges
      .filter((e) => (dir === "out" ? e.from === id : e.to === id))
      .map((e) => {
        const other = dir === "out" ? e.to : e.from;
        const ou = m.unit(other);
        const via = e.layer === "Cust" ? " · Cust" : "";
        return `<button data-action="goto" data-node="${other}"><span class="k">${esc(EDGE_LABEL[e.kind])}${via}</span><span class="n">${esc(ou.name)}${e.kind === "calls" ? ` <span class="mono" style="color:var(--muted)">${esc(e.detail ?? "")}</span>` : ""}</span></button>`;
      });
    return rows.length ? `<section><h4>${dir === "out" ? "Depends on" : "Used by"} (${rows.length})</h4><div class="linklist">${rows.join("")}</div></section>` : "";
  };
  const objs = m.objectsByUnit.get(id) ?? [];
  return `<div class="d">
<div class="eyebrow"><span>${UNIT_GLYPH[u.kind].toLowerCase()}</span><span>${esc(u.component)}</span><span class="pill ${u.origin === "Core" ? "core" : "cust"}">${esc(origin)}</span></div>
<h3>${esc(u.name)}</h3>
<section><h4>Files</h4><div class="filelist">${files}</div></section>
${objs.length ? `<div><button class="btn" data-action="drill" data-node="${id}">Show its ${objs.length} declarations</button></div>` : ""}
${group("out")}${group("in")}
${findingLinks(m.unitFindings(id))}
</div>`;
}

function objectHtml(m: Model, id: string) {
  const o = m.object(id);
  const base = o.base ? m.object(o.base) : null;
  const edges = m.edgesByObject.get(id) ?? [];
  const rows = edges.map((e) => {
    const other = e.from === id ? e.to : e.from;
    const dir = e.from === id ? "→" : "←";
    const where = other[0] === "r"
      ? `<span class="n">${esc(m.relation(other).name)} <span class="mono" style="color:var(--muted)">${m.relation(other).kind}</span></span>`
      : `<span class="n">${esc(m.object(other).name)} <span class="mono" style="color:var(--muted)">in ${esc(m.unit(m.object(other).unit).name)}</span></span>`;
    return `<button data-action="goto" data-node="${other}"><span class="k">${dir} ${esc(EDGE_LABEL[e.kind])}</span>${where}</button>`;
  });
  return `<div class="d">
<div class="eyebrow"><span>${esc(o.kind)}</span>${o.modifier ? `<span class="pill cust">@${esc(o.modifier)}</span>` : ""}${o.section ? `<span>${esc(o.section)} section</span>` : ""}</div>
<h3>${esc(o.name)}</h3>
<dl class="kv"><dt>in</dt><dd>${nodeLink(o.unit)}</dd>${o.target ? `<dt>for</dt><dd class="mono">${esc(o.target)}</dd>` : ""}${o.dynamic ? `<dt>optional</dt><dd class="mono">@DynamicComponentDependency ${esc(o.dynamic)}</dd>` : ""}</dl>
<section><h4>Declared at</h4>${fileLine(o.file, o.line)}</section>
${o.modifier ? `<section><h4>Overrides</h4>${base ? `<p>Base comes from <b>${m.file(base.file).root === "Cust" ? "a pure Cust file" : "Core"}</b> (${esc(m.unit(base.unit).name)}).</p>${fileLine(base.file, base.line)}` : o.base_generated ? `<p>Base is generated from the model, so it isn't in any file.</p>` : `<p>No base found in the indexed files.</p>`}</section>` : ""}
${rows.length ? `<section><h4>Links</h4><div class="linklist">${rows.join("")}</div></section>` : ""}
${findingLinks(m.findingsByNode.get(id) ?? [])}
</div>`;
}

function relationHtml(m: Model, id: string) {
  const r = m.relation(id);
  const links = m.edgesByObject.get(id) ?? [];
  const users = links.filter((e) => e.to === id);
  const reads = links.filter((e) => e.from === id);
  const how: Record<string, string> = {
    "generated view": "Generated from the entity (the view every LU gets)",
    "generated table": "Generated from the entity (the LU's table)",
    view: "Written by hand in a .views file",
    table: "Defined in a .storage file",
    "temporary table": "A temporary table, defined in a .storage file",
  };
  const defs = r.defs.length
    ? r.defs.map((d) => `<p>${esc(how[d.how] ?? d.how)}</p>${fileLine(d.file, d.line)}`).join("")
    : `<p style="color:var(--muted)">Not defined in the indexed files: probably owned by a component outside these folders, or by the framework.</p>`;
  const link = (other: string, e: Edge) =>
    nodeLink(other).replace("</span></button>", `${e.count > 1 ? ` <span class="mono" style="color:var(--muted)">· ${e.count}×</span>` : ""}</span></button>`);
  return `<div class="d">
<div class="eyebrow"><span>${r.kind}</span><span class="pill plain">database</span></div>
<h3>${esc(r.name)}</h3>
<section><h4>Defined</h4>${defs}</section>
${reads.length ? `<section><h4>Reads and calls (${reads.length})</h4><div class="linklist">${reads.map((e) => link(e.to, e)).join("")}</div></section>` : ""}
<section><h4>Read or written by (${users.length})</h4><div class="linklist">${users.map((e) => link(e.from, e)).join("")}</div></section>
</div>`;
}

// ------------------------------------------------------------- actions

function selectFinding(id: string) {
  const m = ui.model!;
  const f = m.idx.findings.find((x) => x.id === id);
  if (!f) return;
  // Selecting the same finding again returns to the normal view.
  if (ui.selected?.type === "finding" && ui.selected.id === id) { deselect(); return; }
  ui.selected = { type: "finding", id };
  const units = new Set<string>();
  for (const n of f.nodes) { const u = m.unitOf(n); if (u) units.add(u); }
  if (units.size) {
    ui.level = "units";
    ui.components.clear();
    ui.focus = units;
    ui.neighbors = true;
  }
  setPane("details");
  renderAll(true);
}

/** Files whose declarations read a table or view, directly or through views that read it. */
function relationUserUnits(m: Model, id: string): Set<string> {
  const units = new Set<string>();
  const queue = [id], seen = new Set(queue);
  for (let i = 0; i < queue.length && units.size < 12; i++) {
    for (const e of m.edgesByObject.get(queue[i]) ?? []) {
      if (e.to !== queue[i]) continue;
      if (e.from[0] === "o") units.add(m.object(e.from).unit);
      else if (e.from[0] === "r" && !seen.has(e.from)) { seen.add(e.from); queue.push(e.from); }
    }
  }
  return units;
}

function gotoNode(id: string) {
  const m = ui.model!;
  if (id[0] === "u") {
    if (!(ui.level === "units" && ui.graph?.nodes.some((n) => n.id === id))) {
      ui.level = "units";
      ui.components.clear();
      ui.focus = new Set([id]);
    }
  } else if (id[0] === "o") {
    const unit = m.object(id).unit;
    if (!(ui.level === "objects" && ui.graph?.nodes.some((n) => n.id === id))) {
      ui.level = "objects";
      ui.objUnits = new Set([unit]);
    }
  } else if (id[0] === "r") {
    if (!(ui.level === "objects" && ui.graph?.nodes.some((n) => n.id === id))) {
      ui.level = "objects";
      ui.objUnits = relationUserUnits(m, id);
    }
  } else {
    ui.level = "components";
  }
  ui.selected = { type: "node", id };
  renderAll(true);
}

function drill(id: string) {
  if (id[0] === "u") {
    ui.level = "objects";
    ui.objUnits = new Set([id]);
  } else if (id[0] !== "o" && id[0] !== "r") {
    ui.level = "units";
    ui.components = new Set([id]);
    ui.focus.clear();
  } else return;
  ui.selected = { type: "node", id };
  setPane("graph");
  renderAll(true);
}

function setLevel(level: Level) {
  const m = ui.model;
  if (!m) return;
  const sel = ui.selected?.type === "node" ? ui.selected.id : null;
  if (level === "units" && !ui.components.size && !ui.focus.size) {
    if (sel && sel[0] !== "u" && sel[0] !== "o") ui.components = new Set([sel]);
    else if (sel) ui.focus = new Set([m.unitOf(sel)!]);
  }
  if (level === "objects" && !ui.objUnits.size) {
    const u = sel && sel[0] !== "r" ? m.unitOf(sel) : ui.focus.size === 1 ? [...ui.focus][0] : null;
    if (u) ui.objUnits = new Set([u]);
  }
  ui.level = level;
  renderAll(true);
}

async function openFile(fi: number, line: number) {
  await openPath(ui.model!.file(fi).abs_path, line);
}

/** Opens any file at a line in VS Code (in the browser build, copies the command). */
async function openPath(path: string, line: number) {
  if (invoke) {
    try { await invoke("open_in_editor", { path, line }); }
    catch (e) { toast(`Couldn't open VS Code: ${e}`); }
    return;
  }
  const cmd = `code -g "${path}:${line}"`;
  try {
    await navigator.clipboard.writeText(cmd);
    toast("Copied the VS Code command");
  } catch {
    toast(cmd);
  }
}

let toastTimer = 0;
function toast(msg: string) {
  const t = $("toast");
  t.textContent = msg;
  t.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = window.setTimeout(() => (t.hidden = true), 3200);
}

function setPane(p: "findings" | "graph" | "details") {
  $("app").dataset.pane = p;
  for (const b of $("tabbar").querySelectorAll("button")) b.setAttribute("aria-pressed", String(b.dataset.pane === p));
  if (p === "graph") requestAnimationFrame(() => fitView());
}

// --------------------------------------------------------------- search

function searchMatches(q: string) {
  const m = ui.model;
  if (!m || q.length < 2) return [];
  const ql = q.toLowerCase();
  const hits: { id: string; kind: string; name: string; sub: string; score: number }[] = [];
  const score = (name: string) => {
    const n = name.toLowerCase();
    const i = n.indexOf(ql);
    return i < 0 ? -1 : (i === 0 ? 0 : 1) * 1000 + n.length;
  };
  for (const c of m.idx.components) { const s = score(c.name); if (s >= 0) hits.push({ id: c.name, kind: "component", name: c.name, sub: "", score: s }); }
  for (const u of m.idx.units) { const s = score(u.name); if (s >= 0) hits.push({ id: u.id, kind: UNIT_GLYPH[u.kind].toLowerCase(), name: u.name, sub: u.component, score: s }); }
  for (const o of m.idx.objects) { const s = score(o.name); if (s >= 0) hits.push({ id: o.id, kind: o.kind, name: o.name, sub: m.unit(o.unit).name, score: s + 500 }); }
  for (const r of m.idx.relations) { const s = score(r.name); if (s >= 0) hits.push({ id: r.id, kind: r.kind, name: r.name, sub: "database", score: s + 700 }); }
  return hits.sort((a, b) => a.score - b.score).slice(0, 14);
}

function renderSearch() {
  const input = $("search") as HTMLInputElement;
  const box = $("search-results");
  const hits = searchMatches(input.value.trim());
  box.hidden = !hits.length;
  box.innerHTML = hits
    .map((h, i) => `<button data-node="${esc(h.id)}" class="${i === 0 ? "active" : ""}"><span class="k">${esc(h.kind)}</span><span>${esc(h.name)}${h.sub ? ` <span class="mono" style="color:var(--muted)">${esc(h.sub)}</span>` : ""}</span></button>`)
    .join("");
}

// --------------------------------------------------------------- events

let lastTap = { id: "", t: 0 };

function bindEvents() {
  document.addEventListener("click", (ev) => {
    const t = ev.target as HTMLElement;
    const panelBtn = t.closest<HTMLElement>("[data-panel]");
    if (panelBtn) { togglePanel(panelBtn.dataset.panel as "left" | "right"); return; }
    const root = t.closest<HTMLElement>("[data-root]");
    if (root) { chooseRoot(root.dataset.root as "core" | "cust" | "prev"); return; }
    if (t.closest("[data-clear-root]")) {
      ui.roots.prev = null;
      try { localStorage.setItem("ifsdepmap.roots", JSON.stringify(ui.roots)); } catch { /* ignore */ }
      renderRoots();
      return;
    }
    if (t.closest("#reindex")) { reindex(); return; }
    const fi = t.closest<HTMLElement>("[data-f]");
    if (fi && !t.closest("[data-action]")) { selectFinding(fi.dataset.f!); return; }
    const sev = t.closest<HTMLElement>("[data-sev]");
    if (sev) {
      const s = sev.dataset.sev as Severity;
      ui.sev.has(s) ? ui.sev.delete(s) : ui.sev.add(s);
      renderFindings();
      return;
    }
    const mode = t.closest<HTMLElement>("[data-selmode]");
    if (mode) {
      ui.selectMode = mode.dataset.selmode as "filter" | "highlight";
      try { localStorage.setItem("ifsdepmap.selectMode", ui.selectMode); } catch { /* storage unavailable */ }
      renderToolbar();
      renderGraph();
      return;
    }
    const depth = t.closest<HTMLElement>("[data-depth]");
    if (depth) {
      ui.depthByLevel[ui.level] = depth.dataset.depth as Depth;
      try { localStorage.setItem("ifsdepmap.depthByLevel", JSON.stringify(ui.depthByLevel)); } catch { /* storage unavailable */ }
      renderToolbar();
      renderGraph();
      return;
    }
    const show = t.closest<HTMLElement>("[data-show]");
    if (show) {
      ui.custOnly = show.dataset.show === "cust";
      try { localStorage.setItem("ifsdepmap.custOnly", String(ui.custOnly)); } catch { /* storage unavailable */ }
      renderToolbar();
      renderGraph(true);
      return;
    }
    const jump = t.closest<HTMLElement>("[data-jump]");
    if (jump) { jumpTo(jump.dataset.jump!); return; }
    if (t.closest("[data-upgrade]")) {
      ui.upgradeOnly = !ui.upgradeOnly;
      renderFindings();
      return;
    }
    if (t.closest("[data-scope]")) {
      ui.showCore = !ui.showCore;
      try { localStorage.setItem("ifsdepmap.showCore", String(ui.showCore)); } catch { /* storage unavailable */ }
      applyScope();
      renderAll();
      return;
    }
    const lvl = t.closest<HTMLElement>("[data-level]");
    if (lvl) { setLevel(lvl.dataset.level as Level); return; }
    const crumb = t.closest<HTMLElement>("[data-crumb]");
    if (crumb) {
      if (crumb.dataset.crumb === "components") { ui.level = "components"; ui.components.clear(); ui.focus.clear(); }
      else { ui.level = "units"; ui.components = new Set([crumb.dataset.comp!]); ui.focus.clear(); }
      renderAll(true);
      return;
    }
    const chip = t.closest<HTMLElement>("[data-chip]");
    if (chip) {
      const key = chip.dataset.key!;
      if (chip.dataset.chip === "unit") ui.unitKinds.has(key) ? ui.unitKinds.delete(key) : ui.unitKinds.add(key);
      else if (chip.dataset.chip === "edge") ui.edgeKinds.has(key) ? ui.edgeKinds.delete(key) : ui.edgeKinds.add(key);
      else if (chip.dataset.chip === "width") {
        ui.width = key as keyof typeof BAND_WIDTHS;
        try { localStorage.setItem("ifsdepmap.width", ui.width); } catch { /* storage unavailable */ }
      } else ui.neighbors = !ui.neighbors;
      renderToolbar();
      renderGraph(true);
      return;
    }
    const act = t.closest<HTMLElement>("[data-action]");
    if (act) {
      const a = act.dataset.action;
      if (a === "open") openFile(Number(act.dataset.file), Number(act.dataset.line));
      else if (a === "open-path") openPath(act.dataset.path!, Number(act.dataset.line));
      else if (a === "goto") { gotoNode(act.dataset.node!); setPane("graph"); }
      else if (a === "drill") drill(act.dataset.node!);
      else if (a === "finding") selectFinding(act.dataset.f!);
      return;
    }
    const res = t.closest<HTMLElement>("#search-results [data-node]");
    if (res) {
      ($("search") as HTMLInputElement).value = "";
      $("search-results").hidden = true;
      gotoNode(res.dataset.node!);
      setPane("graph");
      return;
    }
    const tab = t.closest<HTMLElement>("#tabbar [data-pane]");
    if (tab) { setPane(tab.dataset.pane as "findings" | "graph" | "details"); return; }
    const z = t.closest<HTMLElement>("[data-zoom]");
    if (z) {
      const svg = $("canvas");
      if (z.dataset.zoom === "fit") fitView(undefined, true);
      else zoomAt(z.dataset.zoom === "in" ? 1.25 : 0.8, svg.clientWidth / 2, svg.clientHeight / 2);
      return;
    }
    if (!t.closest(".search")) $("search-results").hidden = true;
  });

  // VS Code shortcuts: Ctrl/Cmd+B toggles the left panel, Ctrl/Cmd+Alt+B the right one.
  document.addEventListener("keydown", (e) => {
    // Esc outside a text field clears the selection (and so leaves the filtered view).
    if (e.key === "Escape" && !(e.target as Element).closest?.("input, textarea") && ui.selected) {
      deselect();
      return;
    }
    if (!(e.ctrlKey || e.metaKey) || e.shiftKey || e.code !== "KeyB") return;
    e.preventDefault();
    togglePanel(e.altKey ? "right" : "left");
  });

  const search = $("search") as HTMLInputElement;
  search.addEventListener("input", renderSearch);
  search.addEventListener("keydown", (e) => {
    if (e.key === "Enter") {
      const first = $("search-results").querySelector<HTMLElement>("[data-node]");
      if (first) first.click();
    } else if (e.key === "Escape") {
      search.value = "";
      $("search-results").hidden = true;
    }
  });

  const fileInput = $("load-json") as HTMLInputElement | null;
  fileInput?.addEventListener("change", () => {
    const file = fileInput.files?.[0];
    if (!file) return;
    const r = new FileReader();
    r.onload = () => {
      try {
        ui.loadedFile = file.name;
        setIndex(JSON.parse(String(r.result)));
        renderRoots();
      } catch (e) {
        toast(`That file isn't a graph.json from ifsdep: ${e}`);
      }
    };
    r.readAsText(file);
  });

  // Pan, pinch and tap on the canvas.
  const svg = $("canvas");
  const pointers = new Map<number, { x: number; y: number }>();
  let start: { x: number; y: number; vx: number; vy: number; node: string | null; band: string | null; moved: boolean } | null = null;
  let pinch: { d: number; k: number } | null = null;
  const local = (e: PointerEvent) => { const r = svg.getBoundingClientRect(); return { x: e.clientX - r.left, y: e.clientY - r.top }; };
  svg.addEventListener("pointerdown", (e) => {
    svg.setPointerCapture(e.pointerId);
    pointers.set(e.pointerId, local(e));
    if (pointers.size === 1) {
      const node = (e.target as Element).closest?.("[data-node]")?.getAttribute("data-node") ?? null;
      const band = (e.target as Element).closest?.("[data-band]")?.getAttribute("data-band") ?? null;
      viewAnimation++; // a drag or tap takes over from a glide in progress
      start = { ...local(e), vx: ui.view.x, vy: ui.view.y, node, band, moved: false };
    } else if (pointers.size === 2) {
      const [a, b] = [...pointers.values()];
      pinch = { d: Math.hypot(a.x - b.x, a.y - b.y), k: ui.view.k };
      if (start) start.moved = true;
    }
  });
  svg.addEventListener("pointermove", (e) => {
    if (!pointers.has(e.pointerId)) return;
    pointers.set(e.pointerId, local(e));
    if (pointers.size === 2 && pinch) {
      const [a, b] = [...pointers.values()];
      const d = Math.hypot(a.x - b.x, a.y - b.y);
      zoomAt((pinch.k * (d / pinch.d)) / ui.view.k, (a.x + b.x) / 2, (a.y + b.y) / 2);
    } else if (start) {
      const p = local(e);
      if (Math.abs(p.x - start.x) + Math.abs(p.y - start.y) > 4) start.moved = true;
      if (start.moved) {
        svg.classList.add("panning");
        ui.view.x = start.vx + p.x - start.x;
        ui.view.y = start.vy + p.y - start.y;
        applyView();
      }
    }
  });
  const end = (e: PointerEvent) => {
    pointers.delete(e.pointerId);
    if (pointers.size < 2) pinch = null;
    if (pointers.size) return;
    svg.classList.remove("panning");
    if (start && !start.moved) {
      const now = Date.now();
      if (start.node) {
        if (lastTap.id === start.node && now - lastTap.t < 380) {
          drill(start.node);
          lastTap = { id: "", t: 0 };
        } else if (ui.selected?.type === "node" && ui.selected.id === start.node) {
          // Selecting the same item again returns to the normal view.
          lastTap = { id: start.node, t: now };
          deselect();
        } else {
          ui.selected = { type: "node", id: start.node };
          lastTap = { id: start.node, t: now };
          renderFindings();
          renderGraph();
          renderDetails();
        }
      } else if (start.band) {
        jumpTo(start.band);
      } else if (ui.selected) {
        deselect();
      }
    }
    start = null;
  };
  svg.addEventListener("pointerup", end);
  svg.addEventListener("pointercancel", end);
  svg.addEventListener("wheel", (e) => {
    e.preventDefault();
    const p = svg.getBoundingClientRect();
    zoomAt(Math.exp(-e.deltaY * 0.0016), e.clientX - p.left, e.clientY - p.top);
  }, { passive: false });
  let resizeTimer = 0;
  window.addEventListener("resize", () => { clearTimeout(resizeTimer); resizeTimer = window.setTimeout(() => fitView(), 150); });
}

document.addEventListener("DOMContentLoaded", boot);
