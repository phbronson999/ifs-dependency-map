//! Lightweight scanner for `.plsql` and `.plsvc` files.
//!
//! Finds top-level PROCEDURE/FUNCTION declarations (with layer annotations),
//! and `Xxx_API.Method` calls together with any `@DynamicComponentDependency`
//! or `$IF Component_X_SYS.INSTALLED $THEN` guard around them. Comments and
//! string literals are blanked out first so they can't produce false calls.

#[derive(Debug, Clone)]
pub struct Method {
    pub kind: &'static str,
    pub name: String,
    pub line: usize,
    pub modifier: Option<String>,
    pub dynamic: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Call {
    /// Upper-cased package name, e.g. `INVENTORY_PART_API`.
    pub package: String,
    pub method: String,
    pub line: usize,
    pub in_method: Option<usize>,
    /// Component of the nearest enclosing `$IF Component_X_SYS.INSTALLED` guard.
    pub guard: Option<String>,
}

/// A table or view read or written in SQL (`FROM x`, `JOIN x`, `UPDATE x`, `INSERT INTO x`).
#[derive(Debug, Clone)]
pub struct Read {
    pub relation: String,
    pub line: usize,
    pub in_method: Option<usize>,
}

#[derive(Debug, Default)]
pub struct ParsedPlsql {
    pub component: Option<String>,
    pub layer: Option<String>,
    pub methods: Vec<Method>,
    pub calls: Vec<Call>,
    pub reads: Vec<Read>,
}

fn is_ident(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '$' || c == '#'
}

/// Reads an identifier starting at byte `i`; returns (ident, end).
fn ident_at(line: &str, i: usize) -> (&str, usize) {
    let end = line[i..].find(|c: char| !is_ident(c)).map(|e| i + e).unwrap_or(line.len());
    (&line[i..end], end)
}

fn skip_ws(line: &str, i: usize) -> usize {
    line[i..].find(|c: char| !c.is_whitespace()).map(|e| i + e).unwrap_or(line.len())
}

/// `PROCEDURE Name` / `FUNCTION Name` starting in column 0.
fn decl_at_col0(line: &str) -> Option<(&'static str, &str)> {
    let first = line.split(|c: char| c.is_whitespace()).next()?;
    let kind = if first.eq_ignore_ascii_case("procedure") {
        "procedure"
    } else if first.eq_ignore_ascii_case("function") {
        "function"
    } else {
        return None;
    };
    let start = skip_ws(line, first.len());
    if start == first.len() {
        return None; // needs whitespace before the name
    }
    let (name, _) = ident_at(line, start);
    if name.is_empty() || !name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') {
        return None;
    }
    Some((kind, name))
}

/// Every `Xxx_API.Method` in the line → (PACKAGE upper-cased, Method).
pub fn calls_in(line: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < line.len() {
        let c = line[i..].chars().next().unwrap();
        if !c.is_ascii_alphabetic() || (i > 0 && line[..i].chars().next_back().is_some_and(is_ident)) {
            i += c.len_utf8();
            continue;
        }
        let (word, end) = ident_at(line, i);
        i = end.max(i + 1);
        if word.len() <= 4 || !word[word.len() - 4..].eq_ignore_ascii_case("_api") {
            continue;
        }
        let dot = skip_ws(line, end);
        if !line[dot..].starts_with('.') {
            continue;
        }
        let m = skip_ws(line, dot + 1);
        let (method, mend) = ident_at(line, m);
        if !method.is_empty() && method.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') {
            out.push((word.to_ascii_uppercase(), method.to_string()));
            i = mend;
        }
    }
    out
}

/// Unqualified calls within the same package: `Check_Insert___(rec_);` or `Do_It;`.
/// Anything followed by `(` or `;` that isn't part of `Pkg.Method` is a candidate;
/// the builder keeps only names that are methods of the same package.
fn local_calls_in(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < line.len() {
        let c = line[i..].chars().next().unwrap();
        if !(c.is_ascii_alphabetic()) || (i > 0 && line[..i].chars().next_back().is_some_and(|p| is_ident(p) || p == '.')) {
            i += c.len_utf8();
            continue;
        }
        let (word, end) = ident_at(line, i);
        i = end.max(i + 1);
        let next = line[end..].trim_start();
        // Variables end in a single `_` (`rec_`); private methods end in `__` / `___`.
        let variable = word.ends_with('_') && !word.ends_with("__");
        if word.contains('_') && !variable && (next.starts_with('(') || next.starts_with(';')) {
            let before = line[..end - word.len()].trim_end().to_ascii_uppercase();
            if !before.ends_with("END") {
                out.push(word.to_string());
            }
        }
    }
    out
}

/// `Component_Proj_SYS.INSTALLED` → `PROJ`.
fn guard_component(line: &str) -> Option<String> {
    let lower = line.to_ascii_lowercase();
    let mut from = 0;
    while let Some(p) = lower[from..].find("component_") {
        let start = from + p + "component_".len();
        let (word, end) = ident_at(&lower, start);
        from = start;
        let Some(comp) = word.strip_suffix("_sys") else { continue };
        let dot = skip_ws(&lower, end);
        if lower[dot..].starts_with('.') && lower[skip_ws(&lower, dot + 1)..].starts_with("installed") {
            return Some(comp.to_ascii_uppercase());
        }
    }
    None
}

pub fn parse(text: &str) -> ParsedPlsql {
    let mut out = ParsedPlsql::default();

    // Header: `--  Component:  SHPORD` comments and `layer Cust;` / `component X;` lines.
    for line in text.lines().take(80) {
        let t = line.trim();
        if let Some(c) = t.strip_prefix("--") {
            let c = c.trim();
            if c.len() > 10 && c[..10].eq_ignore_ascii_case("component:") {
                out.component = c[10..].split_whitespace().next().map(|s| s.to_ascii_uppercase());
            }
        } else {
            let lower = t.to_ascii_lowercase();
            if let Some(v) = lower.strip_prefix("layer ") {
                out.layer = Some(capitalize(v.trim_end_matches(';').trim()));
            } else if let Some(v) = lower.strip_prefix("component ") {
                out.component = Some(v.trim_end_matches(';').trim().to_ascii_uppercase());
            }
        }
    }

    let cleaned = blank_comments_and_strings(text);
    let mut pending_modifier: Option<String> = None;
    let mut pending_dynamic: Option<String> = None;
    let mut guards: Vec<Option<String>> = Vec::new();
    let mut current: Option<usize> = None;

    for (i, line) in cleaned.lines().enumerate() {
        let line_no = i + 1;
        let t = line.trim();
        if let Some(a) = t.strip_prefix('@') {
            let mut parts = a.split_whitespace();
            match parts.next().unwrap_or("") {
                "Override" | "Overtake" => pending_modifier = Some(a.split_whitespace().next().unwrap().to_string()),
                "DynamicComponentDependency" => {
                    pending_dynamic = parts.next().map(|s| s.trim_end_matches(';').to_ascii_uppercase())
                }
                _ => {}
            }
            continue;
        }
        let tl = t.to_ascii_lowercase();
        if tl.starts_with("$if") {
            guards.push(guard_component(t));
        } else if tl.starts_with("$else") || tl.starts_with("$elsif") {
            if let Some(top) = guards.last_mut() {
                *top = None;
            }
        }
        if let Some((kind, name)) = decl_at_col0(line) {
            out.methods.push(Method {
                kind,
                name: name.to_string(),
                line: line_no,
                modifier: pending_modifier.take(),
                dynamic: pending_dynamic.take(),
            });
            current = Some(out.methods.len() - 1);
            continue;
        }
        for name in local_calls_in(line) {
            out.calls.push(Call {
                package: String::new(),
                method: name,
                line: line_no,
                in_method: current,
                guard: guards.iter().rev().find_map(|g| g.clone()),
            });
        }
        for (package, method) in calls_in(line) {
            out.calls.push(Call {
                package,
                method,
                line: line_no,
                in_method: current,
                guard: guards.iter().rev().find_map(|g| g.clone()),
            });
        }
        if tl.starts_with("$end") {
            guards.pop();
        }
    }
    out.reads = relations_in(&cleaned, &out.methods);
    out
}

/// Scans SQL for the object after FROM / JOIN / UPDATE / INSERT INTO / MERGE INTO.
/// IFS variables end in `_` (`newrec_`), so names ending in `_` are skipped, which
/// also filters `EXTRACT(YEAR FROM date_)` and similar.
/// Relations read or written in already-cleaned SQL, upper-cased, with their lines.
pub fn relations(cleaned: &str) -> Vec<(String, usize)> {
    relations_in(cleaned, &[]).into_iter().map(|r| (r.relation, r.line)).collect()
}

/// Words that end a `FROM a x, b y` list.
const FROM_LIST_END: &[&str] = &[
    "WHERE", "GROUP", "ORDER", "HAVING", "CONNECT", "START", "UNION", "MINUS", "INTERSECT", "FETCH", "FOR",
    "INNER", "LEFT", "RIGHT", "FULL", "CROSS", "NATURAL", "ON", "USING", "SET", "VALUES", "RETURNING", "WHEN",
    "THEN", "ELSE", "END", "BEGIN", "LOOP", "INTO", "SELECT", "PIVOT", "UNPIVOT", "WITH",
];

fn relations_in(cleaned: &str, methods: &[Method]) -> Vec<Read> {
    let mut out = Vec::new();
    let mut prev = String::new();
    let mut expect = false;
    let mut after_into = false; // `INSERT INTO t (cols)`: a `(` after the name is a column list
    // Open `FROM a x, b y, ...` lists, by parenthesis depth (a subquery in the list opens its
    // own). A comma at the innermost list's depth starts the next relation.
    let mut from_lists: Vec<i32> = Vec::new();
    let mut depth: i32 = 0;
    for (i, line) in cleaned.lines().enumerate() {
        let line_no = i + 1;
        let mut j = 0;
        while j < line.len() {
            let c = line[j..].chars().next().unwrap();
            if !is_ident(c) {
                match c {
                    '(' => {
                        expect = false; // subquery or function call, not a relation name
                        depth += 1;
                    }
                    ')' => {
                        depth -= 1;
                        while from_lists.last().is_some_and(|&d| d > depth) {
                            from_lists.pop();
                        }
                    }
                    ',' if from_lists.last() == Some(&depth) => expect = true,
                    ';' => {
                        from_lists.clear();
                        expect = false;
                        depth = 0;
                    }
                    _ => {}
                }
                j += c.len_utf8();
                continue;
            }
            let (word, end) = ident_at(line, j);
            j = end;
            let upper = word.to_ascii_uppercase();
            if expect {
                expect = false;
                let skip = ["SELECT", "NOWAIT", "OF", "SKIP", "WAIT", "DUAL", "TABLE", "LATERAL", "THE"];
                let next = line[end..].trim_start();
                if !upper.ends_with('_')
                    && !skip.contains(&upper.as_str())
                    && (after_into || !next.starts_with('('))
                    && !next.starts_with('.')
                {
                    let in_method = methods.iter().rposition(|m| m.line <= line_no);
                    out.push(Read { relation: upper.clone(), line: line_no, in_method });
                }
            }
            after_into = false;
            if FROM_LIST_END.contains(&upper.as_str()) && from_lists.last() == Some(&depth) {
                from_lists.pop();
            }
            match upper.as_str() {
                "FROM" => {
                    expect = true;
                    if from_lists.last() != Some(&depth) {
                        from_lists.push(depth);
                    }
                }
                "JOIN" => {
                    expect = true;
                    if from_lists.last() == Some(&depth) {
                        from_lists.pop();
                    }
                }
                "UPDATE" if prev != "FOR" => expect = true,
                "INTO" if prev == "INSERT" || prev == "MERGE" => {
                    expect = true;
                    after_into = true;
                }
                _ => {}
            }
            prev = upper;
        }
    }
    out
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_ascii_uppercase().to_string() + c.as_str(),
        None => String::new(),
    }
}

/// Replaces comment and string-literal characters with spaces, keeping newlines
/// so line numbers stay correct.
pub fn blank_comments_and_strings(text: &str) -> String {
    #[derive(PartialEq)]
    enum S {
        Code,
        Line,
        Block,
        Str,
    }
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut s = S::Code;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        match s {
            S::Code => {
                if c == '-' && next == Some('-') {
                    s = S::Line;
                    out.push_str("  ");
                    i += 2;
                    continue;
                } else if c == '/' && next == Some('*') {
                    s = S::Block;
                    out.push_str("  ");
                    i += 2;
                    continue;
                } else if c == '\'' {
                    s = S::Str;
                    out.push(' ');
                } else {
                    out.push(c);
                }
            }
            S::Line => {
                if c == '\n' {
                    s = S::Code;
                    out.push('\n');
                } else {
                    out.push(' ');
                }
            }
            S::Block => {
                if c == '*' && next == Some('/') {
                    s = S::Code;
                    out.push_str("  ");
                    i += 2;
                    continue;
                }
                out.push(if c == '\n' { '\n' } else { ' ' });
            }
            S::Str => {
                if c == '\'' && next == Some('\'') {
                    out.push_str("  ");
                    i += 2;
                    continue;
                } else if c == '\'' {
                    s = S::Code;
                    out.push(' ');
                } else {
                    out.push(if c == '\n' { '\n' } else { ' ' });
                }
            }
        }
        i += 1;
    }
    out
}

/// `ShopOrder` → `Shop_Order_API`, `ShopOrdUtil` → `Shop_Ord_Util_API`.
pub fn package_name(stem: &str) -> String {
    format!("{}_API", snake_name(stem))
}

/// `GetDefaultSite` → `Get_Default_Site` (IFS PascalCase → Snake_Case).
pub fn snake_name(stem: &str) -> String {
    let chars: Vec<char> = stem.chars().collect();
    let mut out = String::new();
    for (i, &c) in chars.iter().enumerate() {
        if i > 0 && c.is_ascii_uppercase() {
            let prev = chars[i - 1];
            let next_lower = chars.get(i + 1).is_some_and(|n| n.is_ascii_lowercase());
            if prev.is_ascii_lowercase() || prev.is_ascii_digit() || (prev.is_ascii_uppercase() && next_lower) {
                out.push('_');
            }
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_table_and_view_reads() {
        let src = "PROCEDURE Release__ (order_no_ IN VARCHAR2)\nIS\n   CURSOR c IS\n      SELECT contract FROM shop_order_tab s\n      JOIN user_allowed_site_pub u ON u.contract = s.contract\n      FOR UPDATE NOWAIT;\nBEGIN\n   UPDATE shop_order_tab SET rowstate = 'Released';\n   INSERT INTO shop_order_hist_tab (order_no) VALUES (order_no_);\n   SELECT COUNT(*) INTO n_ FROM (SELECT 1 FROM dual);\n   x_ := EXTRACT(YEAR FROM date_);\nEND Release__;\n";
        let p = parse(src);
        let r: Vec<_> = p.reads.iter().map(|r| r.relation.as_str()).collect();
        assert_eq!(r, ["SHOP_ORDER_TAB", "USER_ALLOWED_SITE_PUB", "SHOP_ORDER_TAB", "SHOP_ORDER_HIST_TAB"]);
        assert!(p.reads.iter().all(|r| r.in_method == Some(0)));
    }

    #[test]
    fn comma_separated_from_lists() {
        let src = "SELECT a.x, NVL(b.y, 0), Some_API.Get(a.z, b.w)\n\
                   FROM   shop_ord_tab a, shop_order_operation_tab b,\n       (SELECT 1 c FROM dual) d, work_center_tab w\n\
                   WHERE  a.order_no = b.order_no AND x IN (1, 2);\n\
                   SELECT q FROM part_tab p WHERE p.a = 1 ORDER BY 1, 2;\n";
        let r = relations(&blank_comments_and_strings(src));
        let names: Vec<&str> = r.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["SHOP_ORD_TAB", "SHOP_ORDER_OPERATION_TAB", "WORK_CENTER_TAB", "PART_TAB"]);
        assert_eq!(r[2].1, 3);
    }

    #[test]
    fn package_names() {
        assert_eq!(package_name("ShopOrder"), "Shop_Order_API");
        assert_eq!(package_name("ShopOrdUtil"), "Shop_Ord_Util_API");
        assert_eq!(package_name("UserAllowedSite"), "User_Allowed_Site_API");
    }

    #[test]
    fn finds_methods_calls_and_guards() {
        let src = r#"
-----------------------------------------------------------------------------
--  Logical unit: ShopOrder
--  Component:    SHPORD
-----------------------------------------------------------------------------
layer Cust;

@Override
PROCEDURE Check_Insert___ (
   newrec_ IN OUT shop_order_tab%ROWTYPE )
IS
   -- Fake_API.Not_A_Call(x) in a comment
   msg_ VARCHAR2(100) := 'Fake_API.Not_A_Call too';
BEGIN
   super(newrec_);
   User_Allowed_Site_API.Exist(newrec_.contract);
   $IF Component_Proj_SYS.INSTALLED $THEN
      Project_API.Exist(newrec_.project_id);
   $ELSE
      Inventory_Part_API.Get_Description(newrec_.contract, newrec_.part_no);
   $END
END Check_Insert___;

@DynamicComponentDependency PROJ
FUNCTION Get_Project_Name (
   project_id_ IN VARCHAR2 ) RETURN VARCHAR2
IS
BEGIN
   RETURN Project_API.Get_Name(project_id_);
END Get_Project_Name;
"#;
        let p = parse(src);
        assert_eq!(p.component.as_deref(), Some("SHPORD"));
        assert_eq!(p.layer.as_deref(), Some("Cust"));
        assert_eq!(p.methods.len(), 2);
        assert_eq!(p.methods[0].modifier.as_deref(), Some("Override"));
        assert_eq!(p.methods[1].dynamic.as_deref(), Some("PROJ"));
        let calls: Vec<_> = p.calls.iter().filter(|c| !c.package.is_empty()).map(|c| (c.package.as_str(), c.guard.as_deref())).collect();
        assert_eq!(
            calls,
            vec![
                ("USER_ALLOWED_SITE_API", None),
                ("PROJECT_API", Some("PROJ")),
                ("INVENTORY_PART_API", None),
                ("PROJECT_API", None),
            ]
        );
        assert_eq!(p.calls.last().unwrap().in_method, Some(1));
        // `super(newrec_)` has no underscore; `END Check_Insert___;` is not a call.
        assert!(p.calls.iter().all(|c| !c.package.is_empty() || c.method != "Check_Insert___"));
        let local = parse("PROCEDURE A__ IS\nBEGIN\n   Check_Insert___(rec_);\n   x_ := Get_Name(id_);\n   Do_It;\nEND A__;\n");
        let names: Vec<_> = local.calls.iter().map(|c| c.method.as_str()).collect();
        assert_eq!(names, ["Check_Insert___", "Get_Name", "Do_It"]);
    }
}
