//! `.views` and `.storage` files: the views and tables they define, and what each
//! view reads. Statements end at the first `;` outside comments and strings.

use crate::{plsql, sqlscope};

#[derive(Debug, Clone)]
pub struct ViewDef {
    /// Upper-cased, as Oracle stores it.
    pub name: String,
    pub line: usize,
    /// `Override` / `Overtake` when the view layers over a base.
    pub modifier: Option<String>,
    /// Relations named in `FROM` / `JOIN`, upper-cased, with their lines.
    pub reads: Vec<(String, usize)>,
    /// `Xxx_API.Method(...)` calls in the view: (`XXX_API`, `Method`, line).
    pub calls: Vec<(String, String, usize)>,
    /// The columns its select list produces; None when it has `*` or an unnamed expression.
    pub columns: Option<Vec<String>>,
}

#[derive(Debug, Default)]
pub struct ParsedDb {
    pub layer: Option<String>,
    pub views: Vec<ViewDef>,
    pub tables: Vec<TableDef>,
}

/// `TABLE x IS (...)`, also `TEMPORARY TABLE` and `INDEX ORGANIZED TABLE`.
#[derive(Debug, Clone, PartialEq)]
pub struct TableDef {
    /// Upper-cased.
    pub name: String,
    pub line: usize,
    pub temporary: bool,
    /// Column names, upper-cased, where the definition lists them.
    pub columns: Vec<String>,
}

/// Words that may come before `TABLE` in a .storage table definition.
const TABLE_PREFIXES: &[&str] = &["TEMPORARY", "GLOBAL", "INDEX", "ORGANIZED"];

pub fn parse(text: &str) -> ParsedDb {
    let mut out = ParsedDb::default();
    let cleaned = plsql::blank_comments_and_strings(text);
    // (view index, first line, last line) for each VIEW statement.
    let mut spans: Vec<(usize, usize, usize)> = Vec::new();
    let mut open: Option<(usize, usize)> = None;
    let mut pending: Option<String> = None;

    for (i, line) in cleaned.lines().enumerate() {
        let line_no = i + 1;
        let t = line.trim();
        if let Some((vi, first)) = open {
            if t.contains(';') {
                spans.push((vi, first, line_no));
                open = None;
            }
            continue;
        }
        if t.is_empty() {
            continue;
        }
        if let Some(a) = t.strip_prefix('@') {
            let word = a.split_whitespace().next().unwrap_or("");
            if word == "Override" || word == "Overtake" {
                pending = Some(word.to_string());
            }
            continue;
        }
        // `TABLE x IS(` is written without the space too.
        let spaced = t.replace('(', " ( ");
        let words: Vec<&str> = spaced.split_whitespace().collect();
        let first = words.first().map(|w| w.to_ascii_uppercase()).unwrap_or_default();
        let second = words.get(1).copied().unwrap_or("").trim_end_matches(';');
        // `[TEMPORARY | INDEX ORGANIZED] TABLE name IS`
        let table_at = words.iter().position(|w| w.eq_ignore_ascii_case("TABLE"));
        if let Some(at) = table_at.filter(|&at| words[..at].iter().all(|w| TABLE_PREFIXES.contains(&w.to_ascii_uppercase().as_str()))) {
            if let Some(name) = words.get(at + 1).filter(|_| words.get(at + 2).is_some_and(|w| w.eq_ignore_ascii_case("IS"))) {
                let temporary = words[..at].iter().any(|w| w.eq_ignore_ascii_case("TEMPORARY"));
                out.tables.push(TableDef { name: name.to_ascii_uppercase(), line: line_no, temporary, columns: Vec::new() });
            }
            pending = None;
            continue;
        }
        match first.as_str() {
            // `layer Core;` alone on its line, not a column called layer (`layer VARCHAR2(100) NULL,`).
            "LAYER" if words.len() == 2 && t.ends_with(';') && out.layer.is_none() => out.layer = Some(second.to_string()),
            "VIEW" if !second.is_empty() => {
                out.views.push(ViewDef {
                    name: second.to_ascii_uppercase(),
                    line: line_no,
                    modifier: pending.take(),
                    reads: Vec::new(),
                    calls: Vec::new(),
                    columns: None,
                });
                let vi = out.views.len() - 1;
                if t.contains(';') {
                    spans.push((vi, line_no, line_no));
                } else {
                    open = Some((vi, line_no));
                }
            }
            _ => {}
        }
        pending = None;
    }
    if let Some((vi, first)) = open {
        spans.push((vi, first, usize::MAX));
    }
    // Columns of `TABLE x IS ( col type ..., col type ... )`: the first name of each item.
    let clines: Vec<&str> = cleaned.lines().collect();
    for t in &mut out.tables {
        let rest = clines[t.line - 1..].join("\n");
        let Some(open) = rest.find('(') else { continue };
        let (mut depth, mut item_start) = (0, true);
        for (k, c) in rest[open..].char_indices() {
            match c {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                ',' if depth == 1 => item_start = true,
                _ if item_start && depth == 1 && is_name_char(c) => {
                    let s = &rest[open + k..];
                    let name: String = s.chars().take_while(|&c| is_name_char(c)).collect();
                    t.columns.push(name.to_ascii_uppercase());
                    item_start = false;
                }
                _ => {}
            }
        }
    }
    // A view's columns: what its select list produces.
    let tlines: Vec<&str> = text.lines().collect();
    for &(vi, first, last) in &spans {
        let end = last.min(tlines.len());
        out.views[vi].columns = sqlscope::select_columns(&sqlscope::prepare(&tlines[first - 1..end].join("\n")));
    }
    for t in created_tables(text) {
        if !out.tables.iter().any(|d| d.name == t.name) {
            out.tables.push(t);
        }
    }

    let in_view = |line: usize| spans.iter().find(|&&(_, a, b)| a <= line && line <= b).map(|&(vi, _, _)| vi);
    for (rel, line) in plsql::relations(&cleaned) {
        if let Some(vi) = in_view(line) {
            if !out.views[vi].reads.iter().any(|(r, _)| *r == rel) {
                out.views[vi].reads.push((rel, line));
            }
        }
    }
    for (i, line) in cleaned.lines().enumerate() {
        let Some(vi) = in_view(i + 1) else { continue };
        for (pkg, method) in plsql::calls_in(line) {
            out.views[vi].calls.push((pkg, method, i + 1));
        }
    }
    out
}

/// Tables a PL/SQL block creates, named in a string rather than declared:
/// - `table_name_ VARCHAR2(30) := 'X_TAB';` in a block that goes on to create a table,
/// - `Create_Temporary_Table('X_TMP_TAB', ...)`: a create-table call whose first argument is the name,
/// - `'CREATE [GLOBAL TEMPORARY] TABLE x (...)'` passed to EXECUTE IMMEDIATE.
fn created_tables(text: &str) -> Vec<TableDef> {
    let upper = blank_comments(text).to_ascii_uppercase();
    let lines: Vec<&str> = upper.lines().collect();
    let mut out: Vec<TableDef> = Vec::new();
    let mut add = |name: String, line: usize, temporary: bool, columns: Vec<String>| {
        if !name.is_empty() && !out.iter().any(|t| t.name == name) {
            out.push(TableDef { name, line, temporary, columns });
        }
    };
    for (i, l) in lines.iter().enumerate() {
        // table_name_ [VARCHAR2(30)] := 'X';
        if let Some(at) = word_at(l, "TABLE_NAME") {
            let rest = &l[at..];
            let var_end = rest.find(|c: char| !is_name_char(c)).unwrap_or(rest.len());
            if let Some(assign) = rest[var_end..].find(":=") {
                let between = &rest[var_end..var_end + assign];
                if between.chars().all(|c| c.is_ascii_alphanumeric() || c.is_whitespace() || c == '(' || c == ')') {
                    if let Some(name) = quoted_name(&rest[var_end + assign + 2..]) {
                        // The block, up to its END;, must create a table.
                        let end = (i..lines.len()).find(|&j| lines[j].trim_start().starts_with("END;")).unwrap_or(lines.len() - 1);
                        let block = lines[i..=end].join(" ");
                        if let Some(temporary) = creates_table(&block) {
                            // Set_Table_Column(columns_, 'ORDER_NO', ...) names the columns.
                            let columns = block
                                .split("SET_TABLE_COLUMN")
                                .skip(1)
                                .filter_map(|s| s.trim_start().strip_prefix('(').and_then(|s| s.split_once(',')).and_then(|(_, s)| quoted_name(s)))
                                .collect();
                            add(name, i + 1, temporary, columns);
                        }
                    }
                }
            }
        }
        // Create_Temporary_Table('X', ...), Create_Table('X', ...)
        let mut from = 0;
        while let Some(p) = l[from..].find("CREATE_").map(|p| from + p) {
            from = p + 7;
            if p > 0 && is_name_char(l[..p].chars().last().unwrap()) {
                continue;
            }
            let id_end = l[p..].find(|c: char| !is_name_char(c)).map_or(l.len(), |e| p + e);
            let id = &l[p..id_end];
            if id.ends_with("TABLE") {
                if let Some(name) = l[id_end..].trim_start().strip_prefix('(').and_then(quoted_name) {
                    add(name, i + 1, id.contains("TEMPORARY"), Vec::new());
                }
            }
        }
        // 'CREATE [GLOBAL TEMPORARY] TABLE x ...'
        if let Some(p) = l.find("'CREATE ") {
            let words: Vec<&str> = l[p + 8..].split_whitespace().collect();
            if let Some(at) = words.iter().position(|w| *w == "TABLE").filter(|&at| words[..at].iter().all(|w| *w == "GLOBAL" || *w == "TEMPORARY")) {
                let name: String = words.get(at + 1).map_or(String::new(), |w| w.chars().take_while(|&c| is_name_char(c)).collect());
                add(name, i + 1, at > 0, Vec::new());
            }
        }
    }
    out
}

/// Whether upper-cased code creates a table, and if so whether a temporary one: a
/// `Create_[Temporary_]Table(` call or a `CREATE [GLOBAL TEMPORARY] TABLE` statement.
/// `Create_Index(table_name_, ...)` doesn't count.
fn creates_table(code: &str) -> Option<bool> {
    let words: Vec<&str> = code.split(|c: char| !is_name_char(c)).filter(|w| !w.is_empty()).collect();
    for (k, w) in words.iter().enumerate() {
        if w.starts_with("CREATE_") && w.ends_with("_TABLE") {
            return Some(w.contains("TEMPORARY"));
        }
        if *w == "CREATE" {
            let rest = &words[k + 1..];
            if let Some(at) = rest.iter().position(|w| *w == "TABLE").filter(|&at| rest[..at].iter().all(|w| *w == "GLOBAL" || *w == "TEMPORARY")) {
                return Some(at > 0);
            }
        }
    }
    None
}

fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '$' || c == '#'
}

/// Where `word` starts in `line` as a whole identifier prefix (not inside another name).
fn word_at(line: &str, word: &str) -> Option<usize> {
    let mut from = 0;
    while let Some(p) = line[from..].find(word).map(|p| from + p) {
        if p == 0 || !is_name_char(line[..p].chars().last().unwrap()) {
            return Some(p);
        }
        from = p + word.len();
    }
    None
}

/// `  'X_TAB' ...` → `X_TAB`, when the quoted text is a plain name.
fn quoted_name(s: &str) -> Option<String> {
    let s = s.trim_start().strip_prefix('\'')?;
    let end = s.find('\'')?;
    let name = &s[..end];
    (!name.is_empty() && name.chars().all(is_name_char)).then(|| name.to_string())
}

/// Blanks `--` and `/* */` comments, keeping string literals (which hold the names here).
pub(crate) fn blank_comments(text: &str) -> String {
    let b = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let (mut i, mut in_str) = (0, false);
    while i < b.len() {
        let c = b[i];
        if in_str {
            in_str = c != b'\'';
        } else if c == b'\'' {
            in_str = true;
        } else if c == b'-' && b.get(i + 1) == Some(&b'-') {
            while i < b.len() && b[i] != b'\n' {
                out.push(' ');
                i += 1;
            }
            continue;
        } else if c == b'/' && b.get(i + 1) == Some(&b'*') {
            let end = text[i + 2..].find("*/").map_or(b.len(), |e| i + 2 + e + 2);
            out.extend(text[i..end].chars().map(|c| if c == '\n' { '\n' } else { ' ' }));
            i = end;
            continue;
        }
        // Multi-byte characters are copied whole.
        let len = text[i..].chars().next().map_or(1, char::len_utf8);
        out.push_str(&text[i..i + len]);
        i += len;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn views_and_tables() {
        let src = "layer Core;\n\
                   -- VIEW Not_A_View IS SELECT * FROM nope_tab;\n\
                   COLUMN Description IS\n   Flags = 'A----';\n\
                   @SecurityCheck Site.UserAuthorized(a.contract)\n\
                   VIEW Shop_Order_Pub IS\n   Prompt = 'Shop Order; public'\n\
                   SELECT a.order_no, Inventory_Part_API.Get_Description(a.contract, a.part_no) descr\n\
                   FROM   shop_order_tab a, shop_ord_line_tab b\n\
                   WHERE  a.order_no = b.order_no;\n\
                   @Override\nVIEW Shop_Order_Lov IS\nSELECT * FROM shop_order_pub;\n\
                   TABLE shop_order_hist_tab IS\n   ( order_no VARCHAR2(12) NOT NULL );\n\
                   INDEX shop_order_hist_ix1 IS shop_order_hist_tab (order_no);\n\
                   TEMPORARY TABLE shop_order_tmp IS\n   ( order_no VARCHAR2(12) NULL );\n\
                   INDEX ORGANIZED TABLE shop_order_iot_tab IS\n   ( order_no VARCHAR2(12) NOT NULL );\n";
        let p = parse(src);
        assert_eq!(p.layer.as_deref(), Some("Core"));
        let names: Vec<&str> = p.views.iter().map(|v| v.name.as_str()).collect();
        assert_eq!(names, ["SHOP_ORDER_PUB", "SHOP_ORDER_LOV"]);
        let v = &p.views[0];
        assert_eq!(v.line, 6);
        assert!(v.modifier.is_none());
        let reads: Vec<&str> = v.reads.iter().map(|(r, _)| r.as_str()).collect();
        assert_eq!(reads, ["SHOP_ORDER_TAB", "SHOP_ORD_LINE_TAB"], "both tables in a comma-separated FROM");
        assert_eq!(v.calls, [("INVENTORY_PART_API".to_string(), "Get_Description".to_string(), 8)]);
        assert_eq!(p.views[1].modifier.as_deref(), Some("Override"));
        assert_eq!(p.views[1].reads, [("SHOP_ORDER_PUB".to_string(), 13)]);
        let tables: Vec<(&str, usize, bool)> = p.tables.iter().map(|t| (t.name.as_str(), t.line, t.temporary)).collect();
        assert_eq!(tables, [("SHOP_ORDER_HIST_TAB", 14, false), ("SHOP_ORDER_TMP", 17, true), ("SHOP_ORDER_IOT_TAB", 19, false)]);
        // Columns: a view's select list (the Prompt line is not part of it), a table's definition.
        assert_eq!(v.columns, Some(vec!["ORDER_NO".to_string(), "DESCR".to_string()]));
        assert_eq!(p.views[1].columns, None, "SELECT * has no known column list");
        assert_eq!(p.tables[0].columns, ["ORDER_NO"]);
    }

    #[test]
    fn tables_created_from_plsql_blocks() {
        let src = "TEMPORARY TABLE shop_order_calc_tab IS(\n   layer VARCHAR2(12) NULL );\n\
                   BEGIN\n   DECLARE\n      table_name_ VARCHAR2(30) := 'SHOP_ORDER_QUEUE_TAB';\n\
                   BEGIN\n      Installation_SYS.Set_Table_Column(columns_, 'ORDER_NO', 'VARCHAR2(12)', 'N');\n\
                   IF NOT Installation_SYS.Table_Exist(table_name_) THEN\n         Installation_SYS.Create_Table(table_name_, columns_);\n      END IF;\n   END;\n\
                   DECLARE\n      table_name_ := 'SHOP_ORDER_OLD_TAB';\n   BEGIN\n      Installation_SYS.Remove_Table(table_name_, TRUE);\n   END;\n\
                   Create_Temporary_Table('SHOP_ORDER_RUN_TMP_TAB', 'SHOP_ORDER_TAB', 'ORDER_NO');\n\
                   EXECUTE IMMEDIATE 'CREATE GLOBAL TEMPORARY TABLE shop_order_snap_tab (order_no VARCHAR2(12))';\n\
                   stmnt_ := 'CREATE GLOBAL TEMPORARY TABLE ' || table_name_;\n\
                   -- table_name_ VARCHAR2(30) := 'COMMENTED_OUT_TAB'; Create_Table\n\
                   END;\n\
                   DECLARE\n   table_name_ VARCHAR2(30) := 'SHOP_ORDER_MV';\nBEGIN\n   Database_SYS.Create_Index(table_name_, index_name_, columns_);\nEND;\n";
        let p = parse(src);
        let tables: Vec<(&str, usize, bool)> = p.tables.iter().map(|t| (t.name.as_str(), t.line, t.temporary)).collect();
        assert_eq!(p.layer, None, "a column called layer is not a layer declaration");
        // The blocks that only remove SHOP_ORDER_OLD_TAB or index SHOP_ORDER_MV create nothing.
        assert_eq!(
            tables,
            [("SHOP_ORDER_CALC_TAB", 1, true), ("SHOP_ORDER_QUEUE_TAB", 5, false), ("SHOP_ORDER_RUN_TMP_TAB", 17, true), ("SHOP_ORDER_SNAP_TAB", 18, true)]
        );
        assert_eq!(p.tables[0].columns, ["LAYER"]);
        assert_eq!(p.tables[1].columns, ["ORDER_NO"], "from Set_Table_Column");
    }
}
