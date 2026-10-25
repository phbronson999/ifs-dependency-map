// Shapes of the JSON written by ifsdep-core (crates/ifsdep-core/src/json.rs).

type Layer = "Core" | "Cust";
type Severity = "error" | "warning" | "info";
type UnitKind = "client" | "projection" | "fragment" | "plsvc" | "package";
type EdgeKind =
  | "include" | "uses_projection" | "implicit_include" | "implements" | "calls"
  | "reference" | "array" | "enumeration" | "entityset_for" | "binds" | "overrides"
  | "calls_operation" | "implemented_by" | "invokes" | "reads";

interface SourceFile {
  id: number;
  rel_path: string;
  abs_path: string;
  root: Layer;
  kind: string;
  component: string;
  declared_layer: string | null;
  unit: string | null;
  role: "base" | "overlay" | "config";
  lines: number;
}

interface Unit {
  id: string;
  kind: UnitKind;
  name: string;
  component: string;
  origin: Layer;
  files: number[];
  has_overlay: boolean;
}

interface Obj {
  id: string;
  unit: string;
  file: number;
  kind: string;
  name: string;
  line: number;
  modifier: string | null;
  section: "client" | "projection" | null;
  target: string | null;
  dynamic: string | null;
  base: string | null;
  base_origin: Layer | null;
}

/** A table or view named in `from = "…"` or SQL; name only until .views is indexed. */
interface Relation { id: string; name: string; kind: "table" | "view" }

interface Edge {
  id: number;
  from: string;
  to: string;
  kind: EdgeKind;
  file: number;
  line: number;
  layer: Layer;
  dynamic: string | null;
  count: number;
  detail: string | null;
}

interface Connection { target: string; kind: string; layer: Layer; file: number; line: number }

interface Component {
  name: string;
  has_core: boolean;
  has_cust: boolean;
  deploy_files: number[];
  connections: Connection[];
}

interface Finding {
  id: string;
  severity: Severity;
  rule: string;
  title: string;
  message: string;
  nodes: string[];
  edges: number[];
  location: { file: number; line: number } | null;
  suggestion: { summary: string; snippet: string | null; target: string | null } | null;
}

interface GraphIndex {
  version: number;
  core_root: string | null;
  cust_root: string | null;
  files: SourceFile[];
  units: Unit[];
  objects: Obj[];
  relations: Relation[];
  edges: Edge[];
  components: Component[];
  findings: Finding[];
  stats: { files: number; by_kind: Record<string, number>; units: number; objects: number; edges: number; millis: number };
  notes: string[];
}

/** `u12` → 12, `o40` → 40, `r3` → 3 */
function idIndex(id: string): number {
  return Number(id.slice(1));
}
