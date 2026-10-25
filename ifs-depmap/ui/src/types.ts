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
  /** No base in any file; it is generated from the model. */
  base_generated: boolean;
}

/** A table or view named in `from = "…"` or SQL, or defined in .views / .storage / .cre / .entity. */
interface Relation {
  id: string;
  name: string;
  kind: "table" | "view";
  /** Where it is defined: a .views / .storage statement, or the .entity it is generated from. */
  defs: { file: number; line: number; how: string }[];
}

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
  /** deploy.ini `[ComponentName] Description=`. */
  description: string | null;
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
  /** How the finding was reached, as chains of source lines. */
  evidence: Trail[];
  /** A Cust file is involved; false means the finding is about Core alone. */
  involves_cust: boolean;
  /** Upgrade findings: the Core declaration before (previous Core) and now. */
  diff: Diff | null;
}

interface Diff {
  /** The previous Core file, absolute; it isn't one of the index's files. */
  old_path: string;
  old_line: number;
  new_file: number;
  new_line: number;
  lines: DiffLine[];
  /** Your customization: where it is and its code. */
  cust_file: number | null;
  cust_line: number;
  cust_code: string[];
}

/**
 * op: " " unchanged, "-" only before, "+" only now, "~" changed (entity attributes), "@" a gap.
 * in_cust (under an @Overtake): your copy has this line ("+") / still has it ("-").
 */
interface DiffLine { op: string; text: string; old: number | null; new: number | null; in_cust: boolean | null }

/** One source line in a chain of evidence; `note` marks a step that rests on an assumption. */
interface Step { file: number; line: number; text: string; note: string | null }
interface Trail { title: string; steps: Step[] }

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
