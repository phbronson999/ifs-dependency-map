// The Trace view: one PL/SQL method's calls, table reads and writes, and raised errors in
// the order they run, read top to bottom. A branch point (IF / CASE / exception handler)
// lists its branches; picking one shows what's in it. Either way the trace goes on with what
// comes after it, as the code does. A call opens into the method it calls, in place, or as a
// trace of its own in a column to the right ("Trace from it"), linked to the call. Steps
// come from the app shell's `trace_method` (crates/ifsdep-core/src/trace.rs), one method at
// a time, and calls are matched to objects through the index's `invokes` links.

type TraceKind = "call" | "read" | "write" | "error" | "flow";

const TRACE_KINDS: { key: TraceKind; label: string; hint: string }[] = [
  { key: "call", label: "calls", hint: "Calls to PL/SQL methods" },
  { key: "read", label: "reads", hint: "Tables and views read: SELECT, and the cursors opened" },
  { key: "write", label: "writes", hint: "INSERT, UPDATE, DELETE and MERGE" },
  { key: "error", label: "errors", hint: "Errors raised: Error_SYS, RAISE, Raise_Application_Error" },
  { key: "flow", label: "returns", hint: "RETURN, and EXIT / CONTINUE in loops" },
];
const WRITE_KINDS = new Set(["insert", "update", "delete", "merge"]);

/** A step's box width for each Width setting. */
const TRACE_WIDTH: Record<string, number> = { normal: 400, wide: 540, wider: 700, widest: 900 };
const T_INDENT = 30, T_ROW = 46, T_H = 36, T_ARM_H = 28;

const tr = {
  /** The traced method's object id (the first column). */
  root: null as string | null,
  /** Traces started from a call ("Trace from it"), each in its own column: the method, the
   * call's row key, and the column's number (the first column is 0). */
  side: [] as { id: string; from: string; col: number }[],
  nextCol: 1,
  /** How far each column has been dragged from where the layout puts it, by column. */
  offsets: new Map<number, { dx: number; dy: number }>(),
  /** Loaded traces by object id; a string says why one couldn't be loaded. */
  cache: new Map<string, MethodTrace | string>(),
  loading: new Set<string>(),
  /** The branch picked at each fork, by the fork's path. */
  choice: new Map<string, number>(),
  /** Calls opened into the method they call, by path. */
  open: new Set<string>(),
  kinds: new Set<TraceKind>(["call", "read", "write", "error", "flow"]),
  /** The rows last drawn, by key. */
  rows: new Map<string, TRow>(),
  /** Package name, upper-cased → the package's unit id. */
  packages: null as Map<string, string> | null,
  /** Base object id → the @Override objects over it. */
  overrides: null as Map<string, string[]> | null,
  /** Table / view name, upper-cased → relation id. */
  relations: null as Map<string, string> | null,
  resolved: new WeakMap<TraceStep, string | null>(),
};

/** One line of the trace. */
interface TRow {
  /** `t:<path>` for steps (selectable), `arm:<fork path>:<i>` for branches. Paths start with the column. */
  key: string;
  /** The column it's drawn in. */
  col: number;
  /** The method whose code the step is in. */
  owner: string;
  /** method, call, error, read, insert, update, delete, merge, cursor, fork, arm, loop, note */
  kind: string;
  depth: number;
  label: string;
  sub: string;
  glyph: string;
  file: number;
  line: number;
  text: string;
  /** How many loops it sits in. */
  loops: number;
  /** Run order; 0 for rows that aren't steps. */
  num: number;
  /** The method a call goes to, the table a read uses, or the traced method. */
  obj: string | null;
  origin: string;
  path: string;
  expandable: boolean;
  expanded: boolean;
  chosen: boolean;
  implied: boolean;
  /** After a RETURN on the branches picked: doesn't run on this path. */
  unreached: boolean;
  /** Calls: runs the Cust @Override of what the code names. */
  override: boolean;
  fork?: TraceStep;
}

/** A procedure or function in a .plsql or .plsvc file. */
function isTraceable(o: Obj) {
  return o.section === null && (o.kind === "procedure" || o.kind === "function");
}

/** Forgets everything from the last index (the files may have changed). */
function traceReset() {
  tr.root = null;
  tr.side = [];
  tr.offsets.clear();
  tr.cache.clear();
  tr.choice.clear();
  tr.open.clear();
  tr.packages = tr.overrides = tr.relations = null;
  tr.resolved = new WeakMap();
}

function startTrace(id: string) {
  if (tr.root !== id) {
    tr.root = id;
    tr.side = [];
    tr.offsets.clear();
    tr.choice.clear();
    tr.open.clear();
  }
  ui.level = "trace";
  ui.selected = null;
  setPane("graph");
  renderAll(true);
}

function traceLookups(m: Model) {
  if (tr.packages && tr.overrides && tr.relations) return;
  tr.packages = new Map();
  for (const u of m.idx.units) if (u.kind === "package") tr.packages.set(u.name.toUpperCase(), u.id);
  tr.overrides = new Map();
  for (const o of m.idx.objects) if (o.base && o.modifier) push(tr.overrides, o.base, o.id);
  tr.relations = new Map(m.idx.relations.map((r) => [r.name.toUpperCase(), r.id]));
}

/**
 * The object a call goes to. The index's links from the calling method come first (the
 * indexer already picked between overloads), then any method of that name in the package.
 * A method with a Cust @Override runs the override; `super` runs the base.
 */
function traceTarget(m: Model, from: Obj, s: TraceStep): { id: string; override: boolean } | null {
  traceLookups(m);
  let id = tr.resolved.get(s);
  if (id === undefined) {
    id = null;
    const name = s.name.toLowerCase();
    if (s.package === null && name === "super") id = from.base;
    else {
      const unit = s.package === null ? from.unit : tr.packages!.get(s.package) ?? null;
      const fits = (o: Obj) => o.unit === unit && o.section === null && o.name.toLowerCase() === name && o.id !== from.id;
      for (const e of m.edgesByObject.get(from.id) ?? []) {
        if (e.kind === "invokes" && e.from === from.id && e.to[0] === "o" && fits(m.object(e.to))) { id = e.to; break; }
      }
      if (!id && unit) id = (m.objectsByUnit.get(unit) ?? []).find(fits)?.id ?? null;
    }
    tr.resolved.set(s, id);
  }
  if (!id) return null;
  const over = s.name.toLowerCase() === "super" ? undefined : tr.overrides!.get(id)?.find((o) => o !== from.id);
  return over ? { id: over, override: true } : { id, override: false };
}

/** Loads a method's trace from the app shell (once), then redraws. */
function loadTrace(m: Model, id: string) {
  if (tr.cache.has(id) || tr.loading.has(id)) return;
  const o = m.object(id);
  if (o.generated) {
    // Generated from the .entity: no source to read. What the index links it to stands in.
    const steps: TraceStep[] = [];
    for (const e of m.edgesByObject.get(id) ?? []) {
      if (e.from !== id) continue;
      if (e.kind === "invokes" && e.to[0] === "o") {
        const t = m.object(e.to);
        steps.push({ kind: "call", line: e.line, package: m.unit(t.unit).name.toUpperCase(), name: t.name, text: e.detail ? `for ${e.detail}` : "", steps: [], arms: [], file: e.file });
      } else if (e.kind === "reads" && e.to[0] === "r") {
        steps.push({ kind: "read", line: e.line, package: null, name: m.relation(e.to).name, text: "", steps: [], arms: [], file: e.file });
      }
    }
    tr.cache.set(id, { name: o.name, line: o.line, end_line: o.line, steps });
    return;
  }
  if (!invoke) {
    tr.cache.set(id, "Trace reads the source files, so it works in the desktop app, not in this page.");
    return;
  }
  tr.loading.add(id);
  invoke("trace_method", { path: m.file(o.file).abs_path, line: o.line })
    .then((json: string) => tr.cache.set(id, JSON.parse(json)))
    .catch((e: unknown) => tr.cache.set(id, String(e)))
    .finally(() => {
      tr.loading.delete(id);
      if (ui.level === "trace") { renderGraph(); renderDetails(); }
    });
}

function traceKindOf(s: TraceStep): TraceKind | null {
  if (s.kind === "call" || s.kind === "error" || s.kind === "read") return s.kind;
  if (s.kind === "return" || s.kind === "exit") return "flow";
  if (WRITE_KINDS.has(s.kind)) return "write";
  return null;
}

/** Whether a step (or anything in it) is drawn with the current kinds. */
function traceVisible(m: Model, from: Obj, s: TraceStep): boolean {
  if (s.kind === "fork") return s.arms.some((a) => a.steps.some((x) => traceVisible(m, from, x)));
  if (s.kind === "loop" || s.kind === "cursor") return s.steps.some((x) => traceVisible(m, from, x));
  const k = traceKindOf(s);
  if (!k || !tr.kinds.has(k)) return false;
  // A same-package name that isn't one of the package's methods is a built-in or a type.
  return !(s.kind === "call" && s.package === null && !traceTarget(m, from, s));
}

/** "2 calls, 1 read" for a branch, not counting what the calls do. */
function armSummary(m: Model, from: Obj, a: TraceArm) {
  const n = { call: 0, read: 0, write: 0, error: 0, flow: 0 };
  const count = (steps: TraceStep[]) => {
    for (const s of steps) {
      if (!traceVisible(m, from, s)) continue;
      const k = traceKindOf(s);
      if (k) n[k]++;
      for (const x of s.arms) count(x.steps);
      count(s.steps);
    }
  };
  count(a.steps);
  const parts = TRACE_KINDS.filter((k) => n[k.key]).map((k) => `${n[k.key]} ${n[k.key] === 1 ? k.label.replace(/s$/, "") : k.label}`);
  return parts.length ? parts.join(", ") : a.implied ? "nothing runs" : "nothing to trace";
}

const titlePackage = (p: string) => p.toLowerCase().replace(/(^|_)([a-z])/g, (_, a: string, b: string) => a + b.toUpperCase()).replace(/_Api$/, "_API");
const fileName = (m: Model, fi: number) => m.file(fi).rel_path.split("/").pop()!;

/** One column's rows, top to bottom, and the loops around runs of them. */
function traceRows(m: Model, col: number, rootId: string) {
  const rows: TRow[] = [];
  const loops: { from: number; to: number; depth: number }[] = [];
  let num = 0;
  // Rows added now come after a RETURN on the picked branches (set per step in `walk`).
  let unreached = false;
  const root = m.object(rootId);
  const row = (r: Partial<TRow>): TRow => {
    const full: TRow = {
      key: "", col, owner: rootId, kind: "", depth: 0, label: "", sub: "", glyph: "", file: root.file, line: 0, text: "", loops: 0, num: 0,
      obj: null, origin: "Core", path: "", expandable: false, expanded: false, chosen: false, implied: false, unreached, override: false, ...r,
    };
    rows.push(full);
    return full;
  };
  row({
    key: `t:${col}`, kind: "method", label: `${m.unit(root.unit).name}.${root.name}`, glyph: root.kind.toUpperCase(),
    sub: fileName(m, root.file), file: root.file, line: root.line, obj: root.id, origin: m.file(root.file).root, path: `${col}`,
  });

  /**
   * `ret.done`: a RETURN on the picked branches of this method has been passed. `dead`: the
   * call into this method is itself after one.
   */
  interface Ctx { depth: number; path: string; from: Obj; loops: number; chain: Set<string>; dead: boolean; ret: { done: boolean } }

  /** A method's steps; true when the trace stops in them (a method still being read). */
  const body = (id: string, c: Omit<Ctx, "from" | "ret">): boolean => {
    const t = tr.cache.get(id);
    if (t === undefined || typeof t === "string") {
      if (t === undefined) loadTrace(m, id);
      const now = tr.cache.get(id);
      if (now !== undefined && typeof now !== "string") return walk(now.steps, { ...c, from: m.object(id), ret: { done: false } });
      row({ key: `t:${c.path}~`, kind: "note", depth: c.depth, owner: id, label: typeof now === "string" ? now : "Reading the method…", glyph: "", file: m.object(id).file, line: m.object(id).line });
      return now === undefined;
    }
    return walk(t.steps, { ...c, from: m.object(id), ret: { done: false } });
  };

  const walk = (steps: TraceStep[], c: Ctx): boolean => {
    for (let i = 0; i < steps.length; i++) {
      const s = steps[i], p = `${c.path}/${i}`;
      if (!traceVisible(m, c.from, s)) continue;
      unreached = c.dead || c.ret.done;
      const base: Partial<TRow> = { key: `t:${p}`, owner: c.from.id, depth: c.depth, loops: c.loops, file: s.file ?? c.from.file, line: s.line, text: s.text, path: p, kind: s.kind };
      if (s.kind === "call") {
        const target = traceTarget(m, c.from, s);
        const t = target ? m.object(target.id) : null;
        const recursive = !!target && c.chain.has(target.id);
        const expandable = !!t && !recursive && (t.generated || !!invoke);
        const expanded = expandable && tr.open.has(p);
        const pkg = t ? m.unit(t.unit).name : s.package ? titlePackage(s.package) : "";
        const where = t
          ? (target!.override ? `runs your @Override in ${fileName(m, t.file)}` : t.generated ? `generated from ${fileName(m, t.file)}` : `in ${fileName(m, t.file)}`)
          : s.package && tr.packages!.has(s.package) ? `no ${s.name} in ${pkg}'s files` : "package not in the indexed files";
        row({
          ...base, num: ++num, glyph: "CALL", obj: target?.id ?? null, origin: t ? m.file(t.file).root : "None",
          // A call without a package is to the current one; it's shown with its package all the same.
          label: s.name.toLowerCase() === "super" ? `super → ${t ? `${pkg}.${t.name}` : "base method"}` : pkg ? `${pkg}.${s.name}` : s.name,
          sub: recursive ? `${where} · calls itself again` : where, expandable, expanded, override: !!target?.override,
        });
        if (expanded && body(target!.id, { depth: c.depth + 1, path: `${p}>`, loops: c.loops, chain: new Set(c.chain).add(target!.id), dead: unreached })) return true;
      } else if (s.kind === "return" || s.kind === "exit") {
        const what = s.name === "RETURN" ? "leaves the method" : s.name === "EXIT" ? "leaves the loop" : "goes to the loop's next pass";
        row({ ...base, num: ++num, glyph: s.name, label: s.text, sub: s.text.includes(" WHEN ") ? `${what}, when the condition holds` : what, origin: "Flow" });
        if (s.name === "RETURN") c.ret.done = true;
      } else if (s.kind === "error") {
        const label = s.package ? `Error_SYS.${s.name}` : s.name.toUpperCase() === "RAISE_APPLICATION_ERROR" ? "Raise_Application_Error" : s.name === "RAISE" ? "RAISE (the error again)" : `RAISE ${s.name}`;
        row({ ...base, num: ++num, glyph: "ERROR", label, sub: "raises an error", origin: "Error" });
      } else if (s.kind === "read" || WRITE_KINDS.has(s.kind)) {
        const rel = tr.relations!.get(s.name.toUpperCase()) ?? null;
        row({ ...base, num: ++num, glyph: s.kind.toUpperCase(), label: s.name, sub: rel ? m.relation(rel).kind : "table or view", obj: rel, origin: "Db" });
      } else if (s.kind === "cursor") {
        row({ ...base, num: ++num, glyph: "CURSOR", label: s.name, sub: "opens its query", origin: "Db" });
        if (walk(s.steps, { ...c, depth: c.depth + 1, path: p })) return true;
      } else if (s.kind === "loop") {
        const start = rows.length;
        row({ ...base, glyph: "↻ LOOP", label: s.text, sub: "what's inside runs once per pass", origin: "Loop" });
        const stopped = walk(s.steps, { ...c, depth: c.depth + 1, path: p, loops: c.loops + 1 });
        loops.push({ from: start, to: rows.length - 1, depth: c.depth });
        if (stopped) return true;
      } else if (s.kind === "fork") {
        const ch = tr.choice.get(p);
        row({
          ...base, glyph: `◆ ${s.name.toUpperCase()}`, label: s.text, origin: "Fork", fork: s,
          sub: ch === undefined ? `${s.arms.length} branches: pick one to see inside it` : "click the picked branch to choose again",
        });
        if (ch === undefined) {
          // Not picked: list the branches and go on after them, as the code does after END IF.
          s.arms.forEach((a, ai) => row({
            key: `arm:${p}:${ai}`, owner: c.from.id, kind: "arm", depth: c.depth + 1, loops: c.loops, label: a.label, sub: armSummary(m, c.from, a),
            file: c.from.file, line: a.line, implied: a.implied, path: p, origin: "Fork", text: a.label,
          }));
          continue;
        }
        const a = s.arms[ch];
        row({
          key: `arm:${p}:${ch}`, owner: c.from.id, kind: "arm", depth: c.depth + 1, loops: c.loops, label: `✓ ${a.label}`, sub: armSummary(m, c.from, a),
          file: c.from.file, line: a.line, implied: a.implied, chosen: true, path: p, origin: "Fork", text: a.label,
        });
        if (walk(a.steps, { ...c, depth: c.depth + 2, path: `${p}:${ch}` })) return true;
      }
    }
    return false;
  };

  const stopped = body(root.id, { depth: 1, path: `${col}`, loops: 0, chain: new Set([root.id]), dead: false });
  unreached = false;
  const t = tr.cache.get(root.id);
  if (!stopped && t && typeof t !== "string") {
    row({ key: `t:${col}$`, kind: "note", depth: 1, label: `End of ${root.name}`, glyph: "END", file: root.file, line: t.end_line, sub: "", origin: "Note" });
  }
  return { rows, loops };
}

function clip(s: string, n: number) {
  return s.length > n ? s.slice(0, Math.max(1, n - 1)) + "…" : s;
}

/** Draws the trace into the canvas (Trace view): the first column, then one per "Trace from it". */
function renderTrace(fit: boolean) {
  const m = ui.model!;
  const vp = $("viewport");
  const notice = $("notice");
  renderJump(null);
  $("canvas").classList.remove("dimmed");
  ui.bandLayout = null;
  if (!tr.root) {
    vp.innerHTML = "";
    ui.placed = new Map();
    notice.hidden = false;
    notice.className = "notice";
    notice.textContent = "Select a procedure or function in the Objects view, then click Trace.";
    return;
  }
  notice.hidden = true;
  const W = TRACE_WIDTH[ui.width] ?? TRACE_WIDTH.normal;
  const height = (r: TRow) => (r.kind === "arm" || r.kind === "note" ? T_ARM_H : T_H);
  const placed = new Map<string, Placed>();
  tr.rows = new Map();
  const sel = ui.selected?.type === "node" ? ui.selected.id : null;
  const room = Math.floor((W - 24) / 7.4);
  let out = "", links = "";
  // Columns by level: the first is level 0, and one started from a call in level n is in
  // level n + 1. A level shares one x; its columns stack top to bottom, each level with the
  // call it came from or below the one above. `auto` is where rows go before any dragging:
  // the layout follows it, so moving one column doesn't shift the others.
  const auto = new Map<string, Placed>();
  const level = new Map<number, number>([[0, 0]]);
  for (const s of tr.side) level.set(s.col, (level.get(traceColOf(s.from)) ?? 0) + 1);
  const byLevel = new Map<number, { id: string; from: string | null; col: number }[]>();
  for (const c of [{ id: tr.root, from: null as string | null, col: 0 }, ...tr.side]) push(byLevel, level.get(c.col) ?? 0, c);
  let levelX = 0;
  for (let lv = 0; byLevel.has(lv); lv++) {
    const cols = byLevel.get(lv)!.map((c) => ({ ...c, src: c.from ? auto.get(c.from) : undefined }));
    cols.sort((a, b) => (a.src?.y ?? Infinity) - (b.src?.y ?? Infinity));
    let bottom = -Infinity, right = 0;
    for (const c of cols) {
      const { rows, loops } = traceRows(m, c.col, c.id);
      const x0 = levelX, y0 = Math.max(c.src ? c.src.y - c.src.h / 2 : 0, bottom);
      bottom = y0 + rows.length * T_ROW + 30;
      const off = tr.offsets.get(c.col) ?? { dx: 0, dy: 0 };
      const ax = x0 + off.dx, ay = y0 + off.dy;
      rows.forEach((r, i) => {
        const h = height(r), x = r.depth * T_INDENT + W / 2, y = i * T_ROW + h / 2;
        auto.set(r.key, { x: x0 + x, y: y0 + y, w: W, h, layer: 0 });
        placed.set(r.key, { x: ax + x, y: ay + y, w: W, h, layer: 0 });
        tr.rows.set(r.key, r);
        right = Math.max(right, r.depth * T_INDENT + W);
      });
      // The dashed line from the call (where it is drawn now, dragged or not).
      const src = c.from ? placed.get(c.from) : undefined;
      if (src) {
        const sx = src.x + src.w / 2, sy = src.y, tx = ax - 2, ty = ay + T_H / 2;
        links += `<path d="M${sx},${sy} C${sx + 60},${sy} ${tx - 60},${ty} ${tx},${ty}" marker-end="url(#m-sel)"/>`;
      }

      // Loops: a shaded box behind the loop and everything in it.
      let loopSvg = "";
      for (const l of loops) {
        let maxDepth = l.depth;
        for (let i = l.from; i <= l.to; i++) maxDepth = Math.max(maxDepth, rows[i].depth);
        const x = l.depth * T_INDENT - 10, y = l.from * T_ROW - 6;
        const w = W + (maxDepth - l.depth) * T_INDENT + 20, h = (l.to - l.from) * T_ROW + height(rows[l.to]) + 12;
        loopSvg += `<g class="tloop"><rect x="${x}" y="${y}" width="${w}" height="${h}" rx="10"/><text x="${x + w - 10}" y="${y + h - 6}" text-anchor="end">↻ repeats</text></g>`;
      }

      // Connectors: each row hangs off the row it belongs to (one level up).
      let lines = "";
      const lastAt: number[] = [];
      rows.forEach((r, i) => {
        lastAt[r.depth] = i;
        lastAt.length = r.depth + 1;
        if (r.depth === 0) return;
        const j = lastAt[r.depth - 1];
        if (j === undefined) return;
        const pr = rows[j];
        const px = pr.depth * T_INDENT + 14, py = j * T_ROW + height(pr);
        const cy = i * T_ROW + height(r) / 2;
        lines += `<path d="M${px},${py} V${cy} H${r.depth * T_INDENT}"/>`;
      });

      let rowsSvg = "";
      rows.forEach((r, i) => {
        const h = height(r);
        const x = r.depth * T_INDENT, y = i * T_ROW;
        let cls = `node trow k-${r.kind} origin-${r.origin}${r.key === sel ? " is-sel" : ""}${r.chosen ? " is-chosen" : ""}${r.implied ? " is-implied" : ""}${r.unreached ? " is-unreached" : ""}`;
        // A button at the right: + / − opens a call in place; × closes a column started from a call.
        const closable = r.kind === "method" && c.col > 0;
      if (r.kind === "method") cls += " is-handle";
        const right = r.expandable || closable ? 34 : 10;
        const loopMark = r.loops ? `↻ in ${r.loops > 1 ? `${r.loops} loops` : "a loop"}` : "";
        let inner: string;
        if (r.kind === "arm" || r.kind === "note") {
          const subRoom = Math.floor((W - 24) / 6.4) - r.label.length - 3;
          inner = `<text x="10" y="18">${esc(clip(r.label, room))}</text>${
            subRoom > 6 ? `<text class="sub" x="${W - 10}" y="18" text-anchor="end">${esc(clip(r.sub, subRoom))}</text>` : ""
          }`;
        } else {
          const topRoom = Math.floor((W - right - 20) / 6.3) - r.glyph.length - loopMark.length - 4;
          const button = r.expandable
            ? `<g class="texp" data-node="exp:${esc(r.path)}" transform="translate(${W - 18},${h / 2})"><title>${r.expanded ? "Close" : "Open"} what this calls</title><circle r="9"/><text y="4" text-anchor="middle">${r.expanded ? "−" : "+"}</text></g>`
            : closable
            ? `<g class="texp" data-node="close:${c.col}" transform="translate(${W - 18},${h / 2})"><title>Close this trace</title><circle r="9"/><text y="4" text-anchor="middle">×</text></g>`
            : "";
          inner = `<text class="glyph" x="10" y="13">${esc(r.glyph)}<tspan class="tsub" dx="8">${esc(clip(r.sub, Math.max(0, topRoom)))}</tspan></text>
  <text x="10" y="29">${esc(clip(r.label, room - (right > 10 ? 4 : 0)))}</text>${
            loopMark ? `<text class="loopmark" x="${W - right}" y="13" text-anchor="end">${loopMark}</text>` : ""
          }${button}`;
        }
        rowsSvg += `<g class="${cls}" data-node="${esc(r.key)}" transform="translate(${x},${y})">
  <title>${r.unreached ? "Not reached on the branches picked: a RETURN comes first\n" : ""}${esc(r.label)}${r.sub ? `\n${esc(r.sub)}` : ""}${r.text && r.text !== r.label ? `\n${esc(r.text)}` : ""}</title>
  <rect class="box" width="${W}" height="${h}" rx="${r.kind === "arm" ? 14 : 6}"/>${r.num ? `<text class="num" x="-8" y="${h / 2 + 4}" text-anchor="end">${r.num}</text>` : ""}${inner}</g>`;
      });
      out += `<g class="tcol" transform="translate(${ax},${ay})"><g class="tloops">${loopSvg}</g><g class="tlines">${lines}</g><g class="nodes">${rowsSvg}</g></g>`;
    }
    levelX += right + 110;
  }
  ui.placed = placed;
  vp.innerHTML = `<g class="tlinks">${links}</g>${out}`;
  if (fit) traceFit();
  else applyView();
}

/** Opens the method a call goes to as a trace of its own, in a column to the right. */
function traceSide(rowKey: string) {
  const r = tr.rows.get(rowKey);
  if (!r?.obj) return;
  let side = tr.side.find((s) => s.from === rowKey);
  if (!side) {
    side = { id: r.obj, from: rowKey, col: tr.nextCol++ };
    tr.side.push(side);
  }
  ui.selected = { type: "node", id: `t:${side.col}` };
  renderGraph();
  renderDetails();
  const p = ui.placed?.get(`t:${side.col}`);
  if (p && !allVisible()) fitView(new Set([`t:${side.col}`, rowKey]), true);
}

/** The column a row key (`t:3/2/1`) is in. */
function traceColOf(key: string) {
  return Number(key.slice(2).match(/^\d+/)?.[0]);
}

/** Closes a column, and the columns started from calls in it. */
function traceCloseSide(col: number) {
  const gone = new Set([col]);
  for (const s of tr.side) if (gone.has(traceColOf(s.from))) gone.add(s.col);
  tr.side = tr.side.filter((s) => !gone.has(s.col));
  for (const c of gone) tr.offsets.delete(c);
  const sel = ui.selected?.type === "node" ? ui.selected.id : "";
  if (sel.startsWith("t:") && gone.has(traceColOf(sel))) ui.selected = null;
  renderGraph();
  renderDetails();
}

/** Back to the first method alone: no branches picked, no calls opened, no other columns. */
function traceStartOver() {
  tr.side = [];
  tr.offsets.clear();
  tr.choice.clear();
  tr.open.clear();
  ui.selected = null;
  renderGraph(true);
  renderDetails();
}

/** Shows the top of the trace at a readable size (Fit shows all of it). */
function traceFit() {
  const { W, top } = clearArea();
  let x1 = 0;
  for (const p of ui.placed?.values() ?? []) x1 = Math.max(x1, p.x + p.w / 2);
  const k = Math.min(1.1, (W - 60) / Math.max(1, x1 + 40));
  ui.view = { k, x: 40 * k + 16, y: top + 8 };
  applyView();
}

/** Handles a tap in the Trace view; false leaves it to the normal selection. */
function traceTap(key: string, double: boolean): boolean {
  if (key.startsWith("arm:")) {
    const at = key.lastIndexOf(":");
    const path = key.slice(4, at), arm = Number(key.slice(at + 1));
    if (tr.choice.get(path) === arm) tr.choice.delete(path);
    else tr.choice.set(path, arm);
    ui.selected = { type: "node", id: `t:${path}` };
    renderGraph();
    renderDetails();
    return true;
  }
  if (key.startsWith("exp:")) {
    traceToggle(key.slice(4));
    return true;
  }
  if (key.startsWith("close:")) {
    traceCloseSide(Number(key.slice(6)));
    return true;
  }
  if (double) {
    const r = tr.rows.get(key);
    if (r?.expandable) { traceToggle(r.path); return true; }
  }
  return false;
}

function traceToggle(path: string) {
  tr.open.has(path) ? tr.open.delete(path) : tr.open.add(path);
  renderGraph();
  renderDetails();
}

/** The right panel for a step in the trace. */
function traceDetailsHtml(m: Model, r: TRow) {
  const pills = [
    `<span>${esc(r.kind === "method" ? "traced method" : r.kind === "fork" ? "branch point" : r.kind)}</span>`,
    r.num ? `<span>step ${r.num}</span>` : "",
    r.loops ? `<span class="pill prev">↻ in ${r.loops > 1 ? `${r.loops} loops` : "a loop"}</span>` : "",
    r.override ? `<span class="pill cust">your @Override</span>` : "",
    r.unreached ? `<span class="pill prev">not reached on this path</span>` : "",
    r.col > 0 ? `<span>column ${r.col + 1}</span>` : "",
  ].join("");
  let body = "";
  if (r.kind === "method") {
    const t = tr.cache.get(r.obj!);
    body = `<section><h4>Declared at</h4>${fileLine(r.file, r.line)}</section>
<section><p>The steps run top to bottom, numbered in order. At a ◆ step, pick a branch to see what's inside it; <b>+</b> on a call opens what it calls.</p>${
      typeof t === "string" ? `<p class="callout">${esc(t)}</p>` : ""
    }<p>${nodeLink(r.obj!)}</p><div class="row-actions">${
      r.col > 0 ? `<button class="btn small" data-action="trace-close" data-col="${r.col}">Close this trace</button>` : ""
    }<button class="btn small" data-action="trace-reset">Start over</button></div></section>`;
  } else if (r.kind === "fork" && r.fork) {
    const ch = tr.choice.get(r.path);
    const from = m.object(r.owner);
    body = `<section><h4>Code</h4><pre class="snippet">${esc(r.text)}</pre>${fileLine(r.file, r.line)}</section>
<section><h4>Branches</h4><div class="linklist">${r.fork.arms.map((a, ai) =>
      `<button data-action="trace-arm" data-key="arm:${esc(r.path)}:${ai}"><span class="k">${ch === ai ? "✓ picked" : a.implied ? "not in code" : "branch"}</span><span class="n">${esc(a.label)} <span class="mono" style="color:var(--muted)">${esc(armSummary(m, from, a))}</span></span></button>`).join("")}</div></section>`;
  } else {
    body = `<section><h4>Code</h4>${r.text ? `<pre class="snippet">${esc(r.text)}</pre>` : ""}${fileLine(r.file, r.line)}</section>`;
    if (r.kind === "loop") body += `<section><p>Everything inside the shaded box runs once for each pass of this loop.</p></section>`;
    if (r.kind === "call") {
      if (r.obj) {
        const t = m.object(r.obj);
        body += `<section><h4>Goes to</h4><p>${nodeLink(r.obj)}</p>${r.override ? `<p>The code calls the Core method, but your <b>@Override</b> in Cust is what runs; its <span class="mono">super()</span> calls the Core one.</p>` : ""}${
          t.generated ? `<p>Generated from the entity, so there's no code to read. Opening it lists what the index links it to, not in run order.</p>` : ""
        }<div class="row-actions">${r.expandable ? `<button class="btn small" data-action="trace-open" data-path="${esc(r.path)}">${r.expanded ? "Close it here" : "Open it here"}</button>` : ""}${
          isTraceable(t) ? `<button class="btn small" data-action="trace-side" data-key="${esc(r.key)}" title="Opens it as a trace of its own, in a column to the right">Trace from it</button>` : ""
        }</div></section>`;
      } else body += `<section><p class="muted">${esc(r.sub[0].toUpperCase() + r.sub.slice(1))}.</p></section>`;
    }
    if (r.obj && r.obj[0] === "r") body += `<section><h4>Table / view</h4><p>${nodeLink(r.obj)}</p></section>`;
  }
  return `<div class="d"><div class="eyebrow">${pills}</div><h3>${esc(r.label)}</h3>${body}</div>`;
}
