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
    // With fixed rows, order by neighbours in any other row, in both directions.
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
