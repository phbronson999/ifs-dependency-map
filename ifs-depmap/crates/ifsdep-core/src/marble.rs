//! Line-oriented parser for `.client`, `.projection` and `.fragment` files.
//!
//! It does not try to understand every Marble construct. It tracks brace
//! depth and pulls out what the dependency graph needs: the header, includes,
//! top-level declarations (with `@Override`/`@Overtake` and
//! `@DynamicComponentDependency`), actions and functions bound inside an
//! entity's body, and `reference` / `array` / `Enumeration(X)` links inside
//! entity-like blocks.

use crate::model::{FileKind, Section};

#[derive(Debug, Clone)]
pub struct Include {
    pub name: String,
    pub line: usize,
    pub dynamic: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefKind {
    Reference,
    Array,
    Enumeration,
    /// `Xxx_API.Method` inside a string, e.g. `fetch = "Shop_Order_API.Get_X(order_no)"`.
    /// `target` is `PACKAGE_API.Method`.
    Call,
    /// `from = "view_name"` on an entity or query.
    Reads,
    /// `call ActionName(...)` in a client command.
    Operation,
}

#[derive(Debug, Clone)]
pub struct Ref {
    pub kind: RefKind,
    pub name: String,
    pub target: String,
    pub line: usize,
    pub dynamic: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Decl {
    pub kind: String,
    pub name: String,
    pub line: usize,
    pub modifier: Option<String>,
    /// The modifier comes from the `@Override entity` the action or function is bound in,
    /// not from its own declaration.
    pub inherited: bool,
    pub section: Section,
    pub target: Option<String>,
    pub dynamic: Option<String>,
    pub refs: Vec<Ref>,
}

#[derive(Debug, Default)]
pub struct ParsedMarble {
    pub header_kind: Option<String>,
    pub name: Option<String>,
    /// The line of the `client X;` / `projection X;` / `fragment X;` header.
    pub header_line: usize,
    pub component: Option<String>,
    pub layer: Option<String>,
    /// For `.client` files: `projection X;`
    pub projection: Option<(String, usize)>,
    pub includes: Vec<Include>,
    pub decls: Vec<Decl>,
}

pub const PROJECTION_KWS: &[&str] = &[
    "entityset", "entity", "query", "virtual", "structure", "enumeration", "summary", "singleton",
    "action", "function",
];
pub const CLIENT_KWS: &[&str] = &[
    "page", "list", "group", "dialog", "command", "selector", "assistant", "navigator", "card",
    "tree", "chart", "calendar", "sheet", "tabs", "singleton",
    // These hold `call`s too (about 1,800 lines in 25R1), which were dropped while unparsed.
    "workflow", "fieldset", "initcontext", "searchcontext", "ganttchart", "ganttchartitem",
    "kanbanboard", "stackedcalendar",
];

/// Declarations whose body can hold bound actions and functions.
const BOUND_TO: &[&str] = &["entity", "query", "virtual", "summary", "singleton"];

#[derive(Default)]
struct Pending {
    modifier: Option<String>,
    dynamic: Option<String>,
}

pub fn parse(text: &str, file_kind: FileKind) -> ParsedMarble {
    let blanked = blank_block_comments(text);
    let text = blanked.as_str();
    let mut out = ParsedMarble::default();
    let default_section = match file_kind {
        FileKind::Client => Some(Section::Client),
        FileKind::Projection => Some(Section::Projection),
        _ => None,
    };
    let mut marker_section: Option<Section> = None;
    let mut depth: i32 = 0;
    let mut pending = Pending::default();
    let mut current: Option<usize> = None;
    let mut last_attr: Option<String> = None;

    for (i, raw) in text.lines().enumerate() {
        let line_no = i + 1;
        if raw.trim_start().starts_with("--") {
            let upper = raw.to_ascii_uppercase();
            if upper.contains("CLIENT FRAGMENTS") {
                marker_section = Some(Section::Client);
            } else if upper.contains("PROJECTION FRAGMENTS") {
                marker_section = Some(Section::Projection);
            }
            continue;
        }
        let code = strip_comment(raw);
        let mut rest = code.trim();
        if rest.is_empty() {
            continue;
        }

        // Leading annotations, possibly on their own line.
        while let Some(stripped) = rest.strip_prefix('@') {
            let (name, after) = split_word(stripped);
            match name {
                "Override" | "Overtake" => pending.modifier = Some(name.to_string()),
                "DynamicComponentDependency" => {
                    let (comp, after2) = split_word(after);
                    pending.dynamic = Some(comp.trim_end_matches(';').to_ascii_uppercase());
                    rest = after2;
                    continue;
                }
                _ => {}
            }
            rest = after;
        }
        if rest.is_empty() {
            continue;
        }

        let depth_before = depth;
        let (opens, closes) = count_braces(rest);
        let words = tokens(rest);

        if depth_before == 0 {
            if let Some(first) = words.first() {
                let kw = first.to_ascii_lowercase();
                match kw.as_str() {
                    "projection" if file_kind == FileKind::Client => {
                        if let Some(n) = words.get(1) {
                            out.projection = Some((n.clone(), line_no));
                        }
                    }
                    "projection" | "client" | "fragment" => {
                        if out.name.is_none() {
                            out.header_kind = Some(kw.clone());
                            out.name = words.get(1).cloned();
                            out.header_line = line_no;
                        }
                    }
                    "component" => out.component = words.get(1).map(|s| s.to_ascii_uppercase()),
                    "layer" => out.layer = words.get(1).cloned(),
                    "include" => {
                        if words.get(1).is_some_and(|w| w.eq_ignore_ascii_case("fragment")) {
                            if let Some(n) = words.get(2) {
                                out.includes.push(Include {
                                    name: n.clone(),
                                    line: line_no,
                                    dynamic: pending.dynamic.take(),
                                });
                            }
                        }
                    }
                    k if PROJECTION_KWS.contains(&k) || CLIENT_KWS.contains(&k) => {
                        if let Some(name) = words.get(1) {
                            let inferred = if PROJECTION_KWS.contains(&k) && !CLIENT_KWS.contains(&k) {
                                Section::Projection
                            } else {
                                Section::Client
                            };
                            let section = marker_section.or(default_section).unwrap_or(inferred);
                            let target = words
                                .iter()
                                .position(|w| w == "for" || w == "using")
                                .and_then(|p| words.get(p + 1))
                                .cloned();
                            out.decls.push(Decl {
                                kind: kw.clone(),
                                name: name.clone(),
                                line: line_no,
                                modifier: pending.modifier.take(),
                                inherited: false,
                                section,
                                target,
                                dynamic: pending.dynamic.take(),
                                refs: Vec::new(),
                            });
                            current = Some(out.decls.len() - 1);
                        }
                    }
                    _ => {}
                }
            }
            pending = Pending::default();
        } else if let Some(ci) = current {
            let first = words.first().map(|w| w.to_ascii_lowercase()).unwrap_or_default();
            // Bound actions and functions, declared inside an entity's body. They are scoped
            // to that entity, which becomes their target. Inside an `@Override entity` they
            // are part of the override, layered over the actions bound to the base.
            let parent = &out.decls[ci];
            if depth_before == 1
                && (first == "action" || first == "function")
                && BOUND_TO.contains(&parent.kind.as_str())
                && parent.section == Section::Projection
            {
                if let Some(name) = words.get(1) {
                    let own = pending.modifier.take();
                    let decl = Decl {
                        kind: first.clone(),
                        name: name.clone(),
                        line: line_no,
                        inherited: own.is_none() && parent.modifier.is_some(),
                        modifier: own.or_else(|| parent.modifier.clone()),
                        section: Section::Projection,
                        target: Some(parent.name.clone()),
                        dynamic: pending.dynamic.take().or_else(|| parent.dynamic.clone()),
                        refs: Vec::new(),
                    };
                    out.decls.push(decl);
                }
            }
            if (first == "reference" || first == "array") && words.len() >= 2 {
                if let Some(t) = words.iter().position(|w| w == "to").and_then(|p| words.get(p + 1)) {
                    out.decls[ci].refs.push(Ref {
                        kind: if first == "array" { RefKind::Array } else { RefKind::Reference },
                        name: words[1].clone(),
                        target: t.clone(),
                        line: line_no,
                        dynamic: pending.dynamic.take(),
                    });
                }
            }
            if first == "attribute" {
                last_attr = words.get(1).cloned();
            }
            if first == "from" {
                if let Some(v) = words.get(1).filter(|v| v.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')) {
                    out.decls[ci].refs.push(Ref {
                        kind: RefKind::Reads,
                        name: "from".into(),
                        target: v.to_ascii_uppercase(),
                        line: line_no,
                        dynamic: None,
                    });
                }
            }
            for (pkg, method) in crate::plsql::calls_in(rest) {
                out.decls[ci].refs.push(Ref {
                    kind: RefKind::Call,
                    name: last_attr.clone().unwrap_or_else(|| first.clone()),
                    target: format!("{pkg}.{method}"),
                    line: line_no,
                    dynamic: pending.dynamic.clone(),
                });
            }
            // `bulkcall` runs the action once per selected record. Not `agentcall`: that's the
            // desktop agent on the user's PC (`FileExists`, `Download`), not the projection.
            if let Some(p) = words.iter().position(|w| w == "call" || w == "bulkcall") {
                if let Some(op) = words.get(p + 1).filter(|w| w.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')) {
                    out.decls[ci].refs.push(Ref {
                        kind: RefKind::Operation,
                        name: "call".into(),
                        target: op.clone(),
                        line: line_no,
                        dynamic: None,
                    });
                }
            }
            if let Some(pos) = rest.find("Enumeration(") {
                let after = &rest[pos + "Enumeration(".len()..];
                let target: String =
                    after.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
                if !target.is_empty() {
                    let attr = if first == "attribute" { words.get(1).cloned() } else { None };
                    out.decls[ci].refs.push(Ref {
                        kind: RefKind::Enumeration,
                        name: attr.unwrap_or_else(|| target.clone()),
                        target,
                        line: line_no,
                        dynamic: None,
                    });
                }
            }
            pending = Pending::default();
        } else {
            pending = Pending::default();
        }

        depth = (depth + opens - closes).max(0);
        if depth == 0 && depth_before > 0 {
            current = None;
            last_attr = None;
        }
    }
    out
}

fn split_word(s: &str) -> (&str, &str) {
    let s = s.trim_start();
    match s.find(char::is_whitespace) {
        Some(i) => (&s[..i], s[i..].trim_start()),
        None => (s, ""),
    }
}

/// Blanks `/* ... */` comments (which may span lines) outside double-quoted strings,
/// keeping newlines so line numbers stay right. A `//` or `--` line comment hides any
/// `/*` after it on that line.
fn blank_block_comments(text: &str) -> String {
    if !text.contains("/*") {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let (mut i, mut in_str, mut in_line_comment) = (0, false, false);
    while i < chars.len() {
        let (c, next) = (chars[i], chars.get(i + 1).copied());
        if c == '\n' {
            in_str = false;
            in_line_comment = false;
        } else if in_line_comment {
        } else if c == '"' {
            in_str = !in_str;
        } else if !in_str && ((c == '/' && next == Some('/')) || (c == '-' && next == Some('-'))) {
            in_line_comment = true;
        } else if !in_str && c == '/' && next == Some('*') {
            out.push_str("  ");
            i += 2;
            while i < chars.len() && !(chars[i] == '*' && chars.get(i + 1) == Some(&'/')) {
                out.push(if chars[i] == '\n' { '\n' } else { ' ' });
                i += 1;
            }
            if i < chars.len() {
                out.push_str("  ");
                i += 2;
            }
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

/// Removes a trailing `-- comment` or `// comment`, ignoring them inside double-quoted strings.
pub fn strip_comment(line: &str) -> &str {
    let bytes = line.as_bytes();
    let mut in_str = false;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => in_str = !in_str,
            b'-' if !in_str && i + 1 < bytes.len() && bytes[i + 1] == b'-' => return &line[..i],
            // `//` comments out a line or the rest of one (35,000 lines in 25R1 Core).
            b'/' if !in_str && i + 1 < bytes.len() && bytes[i + 1] == b'/' => return &line[..i],
            _ => {}
        }
        i += 1;
    }
    line
}

fn count_braces(s: &str) -> (i32, i32) {
    let (mut open, mut close, mut in_str) = (0, 0, false);
    for c in s.chars() {
        match c {
            '"' => in_str = !in_str,
            '{' if !in_str => open += 1,
            '}' if !in_str => close += 1,
            _ => {}
        }
    }
    (open, close)
}

/// Splits on whitespace and punctuation; quoted strings collapse into one token.
fn tokens(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_str = false;
    for c in s.chars() {
        if in_str {
            if c == '"' {
                in_str = false;
                out.push(std::mem::take(&mut cur));
            } else {
                cur.push(c);
            }
            continue;
        }
        match c {
            '"' => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
                in_str = true;
            }
            c if c.is_whitespace() || ";{}(),=[]".contains(c) => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAGMENT: &str = r#"
fragment AddIndirectClockingDialog;   -- name matches filename
component SHPORD;
layer Core;
description "Use this dialog -- not a comment";

include fragment UserAllowedSiteLovSelector;
@DynamicComponentDependency PROJ
include fragment ProjectLovSelector;

----------------------------- CLIENT FRAGMENTS ------------------------------
dialog AddIndirectClockingDialog for AddIndirectClockingStructure {
   label = "New Indirect Labor Clocking";
   group AddIndirectClockingGroup;
}
command Ok for AddIndirectClockingStructure {
   execute {
      call AddIndirectClocking(Company);
   }
}
--------------------------- PROJECTION FRAGMENTS ----------------------------
structure AddIndirectClockingStructure {
   attribute Contract Text {
      maxlength = 5;
   }
   attribute OrderType Enumeration(BatchBalanceOrderType);
   @DynamicComponentDependency MFGSTD
   reference ContractRef(Contract) to UserAllowedSiteLov(Contract) {
      label = "Site";
   }
}
query OrderLov {
   from = "shop_order_lov";
   attribute Descr Text {
      fetch = "Shop_Order_API.Get_Description(order_no)";
   }
}
@Override
entity TiInspectionBridgeConn using TiInspectionBridge {
   array StitchArray(SourceAframeHu) to TiStitchToFinBridge(AframeHu);
}
action AddIndirectClocking {
   parameter Company Text;
}
"#;

    #[test]
    fn parses_fragment() {
        let p = parse(FRAGMENT, FileKind::Fragment);
        assert_eq!(p.name.as_deref(), Some("AddIndirectClockingDialog"));
        assert_eq!(p.component.as_deref(), Some("SHPORD"));
        assert_eq!(p.includes.len(), 2);
        assert_eq!(p.includes[1].dynamic.as_deref(), Some("PROJ"));
        assert!(p.includes[0].dynamic.is_none());

        let kinds: Vec<_> = p.decls.iter().map(|d| (d.kind.as_str(), d.name.as_str(), d.section)).collect();
        assert_eq!(
            kinds,
            vec![
                ("dialog", "AddIndirectClockingDialog", Section::Client),
                ("command", "Ok", Section::Client),
                ("structure", "AddIndirectClockingStructure", Section::Projection),
                ("query", "OrderLov", Section::Projection),
                ("entity", "TiInspectionBridgeConn", Section::Projection),
                ("action", "AddIndirectClocking", Section::Projection),
            ]
        );
        let s = &p.decls[2];
        assert_eq!(s.refs.len(), 2);
        assert_eq!(s.refs[0].kind, RefKind::Enumeration);
        assert_eq!(s.refs[0].target, "BatchBalanceOrderType");
        assert_eq!(s.refs[1].target, "UserAllowedSiteLov");
        assert_eq!(s.refs[1].dynamic.as_deref(), Some("MFGSTD"));
        let cmd = &p.decls[1];
        assert_eq!(cmd.refs.iter().map(|r| (r.kind, r.target.as_str())).collect::<Vec<_>>(), [(RefKind::Operation, "AddIndirectClocking")]);
        let q = &p.decls[3];
        let qrefs: Vec<_> = q.refs.iter().map(|r| (r.kind, r.name.as_str(), r.target.as_str())).collect();
        assert_eq!(
            qrefs,
            [(RefKind::Reads, "from", "SHOP_ORDER_LOV"), (RefKind::Call, "Descr", "SHOP_ORDER_API.Get_Description")]
        );
        let e = &p.decls[4];
        assert_eq!(e.modifier.as_deref(), Some("Override"));
        assert_eq!(e.target.as_deref(), Some("TiInspectionBridge"));
        assert_eq!(e.refs[0].kind, RefKind::Array);
        assert!(p.decls[5].modifier.is_none());
    }

    #[test]
    fn client_projection_line_is_a_reference() {
        let p = parse("client ShopOrder;\ncomponent SHPORD;\nlayer Core;\nprojection ShopOrderHandling;\n", FileKind::Client);
        assert_eq!(p.name.as_deref(), Some("ShopOrder"));
        assert_eq!(p.projection, Some(("ShopOrderHandling".into(), 4)));
    }

    #[test]
    fn bound_actions_inside_an_entity_body() {
        let src = "projection P;\n\
                   entity Order {\n   attribute OrderNo Text;\n   action Print {\n      initialcheck implementation;\n   }\n\
                   @Override\n   function GetTotal Number;\n   reference X(Y) to Z(Y);\n}\n\
                   structure S {\n   action NotBound;\n}\n\
                   action TopLevel;\n\
                   @Overtake\nvirtual V {\n   action Run;\n}\n";
        let p = parse(src, FileKind::Projection);
        let names: Vec<(&str, &str, Option<&str>)> =
            p.decls.iter().map(|d| (d.kind.as_str(), d.name.as_str(), d.target.as_deref())).collect();
        assert_eq!(
            names,
            [
                ("entity", "Order", None),
                ("action", "Print", Some("Order")),
                ("function", "GetTotal", Some("Order")),
                ("structure", "S", None),
                ("action", "TopLevel", None),
                ("virtual", "V", None),
                ("action", "Run", Some("V")),
            ]
        );
        assert_eq!(p.decls[1].line, 4);
        assert_eq!(p.decls[2].modifier.as_deref(), Some("Override"));
        // Inside an overriding virtual the bound action is part of the override.
        assert!(p.decls[1].modifier.is_none());
        assert_eq!(p.decls[6].modifier.as_deref(), Some("Overtake"));
        // The entity keeps the reference that follows its bound actions.
        assert_eq!(p.decls[0].refs.len(), 1);
    }
    #[test]
    fn comments_with_braces_are_ignored() {
        let src = "fragment F;
component C;
layer Core;
                   command A for E {
   execute {
      navigate \"page/X/Form?$filter=a eq 1 // not a comment\";
                   //   bulkexecute {
   }
}
                   /* list Old for E {
      field X;
 */
                   list InventoryList for E {
   field Y; /* { */
}
";
        let p = parse(src, FileKind::Fragment);
        let names: Vec<(&str, usize)> = p.decls.iter().map(|d| (d.name.as_str(), d.line)).collect();
        assert_eq!(names, [("A", 4), ("InventoryList", 13)], "the list after the commented-out braces is still top level");
    }
}
