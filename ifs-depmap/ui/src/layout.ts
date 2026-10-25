// Layered (Sugiyama-style) layout, top to bottom, with no dependencies:
// 1. reverse back edges found by DFS so the graph is acyclic,
// 2. longest-path layering,
// 3. barycenter sweeps to reduce crossings,
// 4. rows wrap when a layer gets wider than `maxRowWidth`.

interface LayoutNode { id: string; w: number; h: number }
interface LayoutEdge { from: string; to: string }
interface Placed { x: number; y: number; w: number; h: number; layer: number }

function layoutLayered(
  nodes: LayoutNode[],
  edges: LayoutEdge[],
  opts: { nodeGap?: number; layerGap?: number; maxRowWidth?: number; fixedLayers?: Map<string, number> } = {},
): Map<string, Placed> {
  const nodeGap = opts.nodeGap ?? 28;
  const layerGap = opts.layerGap ?? 70;
  const maxRowWidth = opts.maxRowWidth ?? 1500;
  const n = nodes.length;
  const ix = new Map<string, number>();
  nodes.forEach((nd, i) => ix.set(nd.id, i));

  const out: number[][] = Array.from({ length: n }, () => []);
  for (const e of edges) {
    const a = ix.get(e.from), b = ix.get(e.to);
    if (a === undefined || b === undefined || a === b) continue;
    out[a].push(b);
  }

  // 1. DFS (iterative) to find back edges; reverse them.
  const state = new Uint8Array(n); // 0 new, 1 on stack, 2 done
  const dag: number[][] = Array.from({ length: n }, () => []);
  const dfsOrder: number[] = [];
  for (let r = 0; r < n; r++) {
    if (state[r]) continue;
    const stack: [number, number][] = [[r, 0]];
    state[r] = 1;
    dfsOrder.push(r);
    while (stack.length) {
      const top = stack[stack.length - 1];
      const [v, p] = top;
      if (p < out[v].length) {
        top[1]++;
        const w = out[v][p];
        if (state[w] === 1) {
          dag[w].push(v); // back edge: reverse
        } else {
          dag[v].push(w);
          if (state[w] === 0) {
            state[w] = 1;
            dfsOrder.push(w);
            stack.push([w, 0]);
          }
        }
      } else {
        state[v] = 2;
        stack.pop();
      }
    }
  }

  // 2. Longest-path layering (Kahn order).
  const indeg = new Int32Array(n);
  for (let v = 0; v < n; v++) for (const w of dag[v]) indeg[w]++;
  const layer = new Int32Array(n);
  const queue: number[] = [];
  for (let v = 0; v < n; v++) if (indeg[v] === 0) queue.push(v);
  for (let qi = 0; qi < queue.length; qi++) {
    const v = queue[qi];
    for (const w of dag[v]) {
      layer[w] = Math.max(layer[w], layer[v] + 1);
      if (--indeg[w] === 0) queue.push(w);
    }
  }
  // Pull sources down next to their first child so they don't all crowd layer 0.
  const hasParent = new Uint8Array(n);
  for (let v = 0; v < n; v++) for (const w of dag[v]) hasParent[w] = 1;
  for (let v = 0; v < n; v++) {
    if (dag[v].length && !hasParent[v]) {
      const minChild = Math.min(...dag[v].map((w) => layer[w]));
      if (minChild > 1) layer[v] = minChild - 1;
    }
  }

  // Caller-assigned rows (e.g. semantic bands) replace the computed layering;
  // empty rows are squeezed out.
  if (opts.fixedLayers) {
    const used = [...new Set(nodes.map((nd) => opts.fixedLayers!.get(nd.id) ?? 0))].sort((a, b) => a - b);
    const rank = new Map(used.map((v, i) => [v, i]));
    nodes.forEach((nd, i) => (layer[i] = rank.get(opts.fixedLayers!.get(nd.id) ?? 0)!));
  }

  const maxLayer = n ? Math.max(...Array.from(layer)) : 0;
  const layers: number[][] = Array.from({ length: maxLayer + 1 }, () => []);
  for (const v of dfsOrder) layers[layer[v]].push(v);

  // 3. Barycenter ordering.
  const pos = new Float64Array(n);
  const preds: number[][] = Array.from({ length: n }, () => []);
  for (let v = 0; v < n; v++) for (const w of dag[v]) preds[w].push(v);
  if (opts.fixedLayers) {
    // With fixed rows, order by neighbors in any other row, in both directions.
    for (let v = 0; v < n; v++) {
      preds[v] = preds[v].filter((u) => layer[u] !== layer[v]);
      dag[v] = dag[v].filter((w) => layer[w] !== layer[v]);
    }
  }
  const setPos = () => layers.forEach((l) => l.forEach((v, i) => (pos[v] = l.length > 1 ? i / (l.length - 1) : 0.5)));
  setPos();
  for (let sweep = 0; sweep < 8; sweep++) {
    const down = sweep % 2 === 0;
    const range = down ? layers.map((_, i) => i) : layers.map((_, i) => layers.length - 1 - i);
    for (const li of range) {
      const l = layers[li];
      const bary = new Map<number, number>();
      for (const v of l) {
        const nb = down ? preds[v] : dag[v];
        bary.set(v, nb.length ? nb.reduce((s, u) => s + pos[u], 0) / nb.length : pos[v]);
      }
      l.sort((a, b) => (bary.get(a)! - bary.get(b)!) || a - b);
      l.forEach((v, i) => (pos[v] = l.length > 1 ? i / (l.length - 1) : 0.5));
    }
  }

  // 4. Coordinates, wrapping very wide layers into extra rows.
  const placed = new Map<string, Placed>();
  let y = 0;
  layers.forEach((l, li) => {
    const rows: number[][] = [[]];
    let rowW = 0;
    for (const v of l) {
      const w = nodes[v].w + nodeGap;
      if (rowW + w > maxRowWidth && rows[rows.length - 1].length) {
        rows.push([]);
        rowW = 0;
      }
      rows[rows.length - 1].push(v);
      rowW += w;
    }
    for (const row of rows) {
      const width = row.reduce((s, v) => s + nodes[v].w, 0) + nodeGap * (row.length - 1);
      const h = Math.max(...row.map((v) => nodes[v].h));
      let x = -width / 2;
      for (const v of row) {
        placed.set(nodes[v].id, { x: x + nodes[v].w / 2, y: y + h / 2, w: nodes[v].w, h: nodes[v].h, layer: li });
        x += nodes[v].w + nodeGap;
      }
      y += h + layerGap * (rows.length > 1 ? 0.55 : 1);
    }
    if (rows.length > 1) y += layerGap * 0.45;
  });
  return placed;
}

// Banded, grouped layout (Objects and Components views): each node sits in a fixed band and,
// within it, in a group (the file it comes from) drawn as one block, with the group's
// row-0 nodes (overrides, views) above its row-1 nodes. Groups are ordered to sit under what
// they link to, and wrap into extra lines when a band gets wider than `maxRowWidth`.

interface BandNode { id: string; w: number; h: number; band: number; row: 0 | 1; group: string }
interface GroupBox { band: number; group: string; x: number; y: number; w: number; h: number; split: number | null }
interface BandBox { band: number; top: number; bottom: number }

function layoutBands(
  nodes: BandNode[],
  edges: LayoutEdge[],
  opts: { maxRowWidth?: number; maxGroupWidth?: number; nodeGap?: number; header?: number } = {},
): { placed: Map<string, Placed>; groups: GroupBox[]; bands: BandBox[] } {
  const nodeGap = opts.nodeGap ?? 18;
  const maxRowWidth = opts.maxRowWidth ?? 1500;
  const maxGroupWidth = Math.min(opts.maxGroupWidth ?? maxRowWidth * 0.7, maxRowWidth);
  const header = opts.header ?? 20;    // space for the group's label
  const pad = 10;                      // inside a group's box
  const groupGap = 26, lineGap = 26, bandGap = 64;

  const byId = new Map(nodes.map((n) => [n.id, n]));
  const nbrs = new Map<string, string[]>();
  for (const e of edges) {
    const a = byId.get(e.from), b = byId.get(e.to);
    if (!a || !b || a === b || a.band === b.band) continue;
    (nbrs.get(a.id) ?? nbrs.set(a.id, []).get(a.id)!).push(b.id);
    (nbrs.get(b.id) ?? nbrs.set(b.id, []).get(b.id)!).push(a.id);
  }
  // band → group → nodes, in first-seen order.
  const bands = new Map<number, Map<string, BandNode[]>>();
  for (const n of nodes) {
    const groups = bands.get(n.band) ?? bands.set(n.band, new Map()).get(n.band)!;
    (groups.get(n.group) ?? groups.set(n.group, []).get(n.group)!).push(n);
  }
  const bandOrder = [...bands.keys()].sort((a, b) => a - b);
  const order = new Map<number, string[]>();   // band → group order
  for (const b of bandOrder) order.set(b, [...bands.get(b)!.keys()]);
  const xpos = new Map<string, number>();      // normalized 0..1 across the band

  /** Rows of one group, wrapped to the group width: [[row0 lines], [row1 lines]]. */
  const rowsOf = (list: BandNode[]) => {
    const out: BandNode[][] = [];
    for (const row of [0, 1] as const) {
      let line: BandNode[] = [], w = 0;
      for (const n of list.filter((x) => x.row === row)) {
        if (line.length && w + n.w > maxGroupWidth) { out.push(line); line = []; w = 0; }
        line.push(n);
        w += n.w + nodeGap;
      }
      if (line.length) out.push(line);
    }
    return out;
  };
  const groupWidth = (list: BandNode[]) =>
    Math.max(...rowsOf(list).map((l) => l.reduce((s, n) => s + n.w, 0) + nodeGap * (l.length - 1))) + pad * 2;

  const setX = (b: number) => {
    const groups = bands.get(b)!, ids = order.get(b)!;
    let total = 0;
    const spans: [string, number, number][] = [];
    for (const g of ids) { const w = groupWidth(groups.get(g)!); spans.push([g, total, w]); total += w + groupGap; }
    for (const [g, x0, w] of spans) {
      const list = groups.get(g)!;
      list.forEach((n, i) => xpos.set(n.id, (x0 + (w * (i + 0.5)) / list.length) / Math.max(total, 1)));
    }
  };
  bandOrder.forEach(setX);
  const bary = (ids: string[], fallback: number) => {
    let s = 0, c = 0;
    for (const id of ids) for (const m of nbrs.get(id) ?? []) { s += xpos.get(m) ?? 0.5; c++; }
    return c ? s / c : fallback;
  };
  for (let sweep = 0; sweep < 6; sweep++) {
    const seq = sweep % 2 === 0 ? bandOrder : [...bandOrder].reverse();
    for (const b of seq) {
      const groups = bands.get(b)!;
      const gb = new Map<string, number>();
      for (const g of order.get(b)!) {
        const list = groups.get(g)!;
        const own = list.reduce((s, n) => s + (xpos.get(n.id) ?? 0.5), 0) / list.length;
        gb.set(g, bary(list.map((n) => n.id), own));
        list.sort((p, q) => bary([p.id], xpos.get(p.id)!) - bary([q.id], xpos.get(q.id)!));
      }
      order.get(b)!.sort((p, q) => gb.get(p)! - gb.get(q)!);
      setX(b);
    }
  }

  // Coordinates.
  const placed = new Map<string, Placed>();
  const boxes: GroupBox[] = [];
  const bandBoxes: BandBox[] = [];
  let y = 0;
  bandOrder.forEach((b, bi) => {
    const groups = bands.get(b)!;
    const blocks = order.get(b)!.map((g) => {
      const rows = rowsOf(groups.get(g)!);
      const w = groupWidth(groups.get(g)!);
      const h = header + pad + rows.reduce((s, l) => s + Math.max(...l.map((n) => n.h)), 0) + nodeGap * (rows.length - 1) + pad;
      return { g, rows, w, h };
    });
    // Wrap blocks into lines.
    const lines: (typeof blocks)[] = [[]];
    let lw = 0;
    for (const bl of blocks) {
      if (lines[lines.length - 1].length && lw + bl.w > maxRowWidth) { lines.push([]); lw = 0; }
      lines[lines.length - 1].push(bl);
      lw += bl.w + groupGap;
    }
    const top = y;
    for (const line of lines) {
      const width = line.reduce((s, bl) => s + bl.w, 0) + groupGap * (line.length - 1);
      const lh = Math.max(...line.map((bl) => bl.h));
      let x = -width / 2;
      for (const bl of line) {
        let ry = y + header + pad, split: number | null = null;
        bl.rows.forEach((row, ri) => {
          const rh = Math.max(...row.map((n) => n.h));
          if (ri > 0 && row[0].row === 1 && bl.rows[ri - 1][0].row === 0) split = ry - nodeGap / 2;
          const rw = row.reduce((s, n) => s + n.w, 0) + nodeGap * (row.length - 1);
          let nx = x + (bl.w - rw) / 2;
          for (const n of row) {
            placed.set(n.id, { x: nx + n.w / 2, y: ry + rh / 2, w: n.w, h: n.h, layer: bi });
            nx += n.w + nodeGap;
          }
          ry += rh + nodeGap;
        });
        boxes.push({ band: b, group: bl.g, x, y, w: bl.w, h: bl.h, split });
        x += bl.w + groupGap;
      }
      y += lh + lineGap;
    }
    bandBoxes.push({ band: b, top, bottom: y - lineGap });
    y += bandGap - lineGap;
  });
  return { placed, groups: boxes, bands: bandBoxes };
}
