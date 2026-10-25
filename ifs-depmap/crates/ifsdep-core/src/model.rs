use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Layer {
    Core,
    Cust,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum FileKind {
    Client,
    Projection,
    Fragment,
    Plsvc,
    Plsql,
    DeployIni,
}

impl FileKind {
    pub fn from_path(p: &Path) -> Option<Self> {
        let name = p.file_name()?.to_str()?.to_ascii_lowercase();
        if name == "deploy.ini" {
            return Some(Self::DeployIni);
        }
        match p.extension()?.to_str()?.to_ascii_lowercase().as_str() {
            "client" => Some(Self::Client),
            "projection" => Some(Self::Projection),
            "fragment" => Some(Self::Fragment),
            "plsvc" => Some(Self::Plsvc),
            "plsql" => Some(Self::Plsql),
            _ => None,
        }
    }

    pub fn is_marble(self) -> bool {
        matches!(self, Self::Client | Self::Projection | Self::Fragment)
    }
}

/// A logical unit groups the Core file and any `-Cust` files that share a name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum UnitKind {
    Client,
    Projection,
    Fragment,
    Plsvc,
    Package,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileRole {
    /// Defines the unit (Core file, or the single file of a pure Cust unit).
    Base,
    /// A `-Cust` file layered over a Core unit of the same name.
    Overlay,
    /// deploy.ini and other non-unit files.
    Config,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Section {
    Client,
    Projection,
}

#[derive(Debug, Clone)]
pub struct SourceFile {
    pub id: usize,
    pub rel_path: String,
    pub abs_path: String,
    pub root: Layer,
    pub kind: FileKind,
    pub component: String,
    pub declared_layer: Option<String>,
    pub unit: Option<String>,
    pub role: FileRole,
    pub lines: usize,
}

#[derive(Debug, Clone)]
pub struct Unit {
    pub id: String,
    pub kind: UnitKind,
    pub name: String,
    pub component: String,
    /// Where the base definition lives: Core, or a pure Cust file.
    pub origin: Layer,
    pub files: Vec<usize>,
    pub has_overlay: bool,
}

#[derive(Debug, Clone)]
pub struct Object {
    pub id: String,
    pub unit: String,
    pub file: usize,
    pub kind: String,
    pub name: String,
    pub line: usize,
    /// `Override` / `Overtake` when the declaration layers over a base.
    pub modifier: Option<String>,
    pub section: Option<Section>,
    /// `for` / `using` target named in the declaration.
    pub target: Option<String>,
    pub dynamic: Option<String>,
    /// Resolved base object for overrides.
    pub base: Option<String>,
    pub base_origin: Option<Layer>,
    /// No base in any file, but it is generated from the model (IFS's "Base" layer):
    /// any Core override, or a Cust override of a template method like `Insert___`.
    pub base_generated: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EdgeKind {
    /// `include fragment X;`
    Include,
    /// Client → its projection.
    UsesProjection,
    /// Projection → fragment that reaches it through a client's include.
    ImplicitInclude,
    /// Projection → its .plsvc.
    Implements,
    /// PL/SQL package or plsvc → `Xxx_API` package.
    Calls,
    /// Object-level: `reference R(..) to Entity(..)`.
    Reference,
    /// Object-level: `array A(..) to Entity(..)`.
    Array,
    /// Object-level: attribute typed `Enumeration(X)`.
    Enumeration,
    /// Object-level: `entityset S for Entity`.
    EntitysetFor,
    /// Object-level: client construct `for` / `using` a projection object.
    Binds,
    /// Object-level: `@Override` / `@Overtake` → resolved base.
    Overrides,
    /// Object-level: client command `call X(...)` → projection action/function.
    CallsOperation,
    /// Object-level: projection action/function → its `.plsvc` method (`DoIt` → `Do_It___`).
    ImplementedBy,
    /// Object-level: method (or `fetch = "Xxx_API.M()"`) → PL/SQL method it calls.
    Invokes,
    /// Object-level: entity/query `from = "view"`, or SQL in a method → table/view.
    Reads,
}

impl EdgeKind {
    pub fn is_unit_level(self) -> bool {
        matches!(
            self,
            Self::Include | Self::UsesProjection | Self::ImplicitInclude | Self::Implements | Self::Calls
        )
    }
}

#[derive(Debug, Clone)]
pub struct Edge {
    pub id: usize,
    pub from: String,
    pub to: String,
    pub kind: EdgeKind,
    pub file: usize,
    pub line: usize,
    pub layer: Layer,
    /// Component named by `@DynamicComponentDependency` or a `$IF Component_X_SYS.INSTALLED` guard.
    pub dynamic: Option<String>,
    pub count: usize,
    pub detail: Option<String>,
}

/// A database table or view named in `from = "…"` or in SQL. Name only until
/// `.views` / `.storage` are indexed.
#[derive(Debug, Clone)]
pub struct Relation {
    pub id: String,
    pub name: String,
    /// `table` for `*_TAB`, otherwise `view`.
    pub kind: String,
}

#[derive(Debug, Clone)]
pub struct Connection {
    pub target: String,
    pub kind: String,
    pub layer: Layer,
    pub file: usize,
    pub line: usize,
}

#[derive(Debug, Clone)]
pub struct Component {
    pub name: String,
    pub has_core: bool,
    pub has_cust: bool,
    pub deploy_files: Vec<usize>,
    pub connections: Vec<Connection>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Error,
    Warning,
    Info,
}

#[derive(Debug, Clone)]
pub struct Location {
    pub file: usize,
    pub line: usize,
}

#[derive(Debug, Clone)]
pub struct Suggestion {
    pub summary: String,
    pub snippet: Option<String>,
    /// The file the change belongs in (may not exist yet).
    pub target: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Finding {
    pub id: String,
    pub severity: Severity,
    pub rule: String,
    pub title: String,
    pub message: String,
    pub nodes: Vec<String>,
    pub edges: Vec<usize>,
    pub location: Option<Location>,
    pub suggestion: Option<Suggestion>,
}

#[derive(Debug, Clone, Default)]
pub struct Stats {
    pub files: usize,
    pub by_kind: BTreeMap<String, usize>,
    pub units: usize,
    pub objects: usize,
    pub edges: usize,
    pub millis: u128,
}

#[derive(Debug, Clone, Default)]
pub struct Index {
    pub version: u32,
    pub core_root: Option<String>,
    pub cust_root: Option<String>,
    pub files: Vec<SourceFile>,
    pub units: Vec<Unit>,
    pub objects: Vec<Object>,
    pub relations: Vec<Relation>,
    pub edges: Vec<Edge>,
    pub components: Vec<Component>,
    pub findings: Vec<Finding>,
    pub stats: Stats,
    pub notes: Vec<String>,
}

/// `u12` → 12, `o40` → 40, `r3` → 3.
pub fn id_index(id: &str) -> usize {
    id[1..].parse().unwrap_or(usize::MAX)
}
