//! `.views` and `.storage` files: the views and tables they define, and what each
//! view reads. Statements end at the first `;` outside comments and strings.

use crate::plsql;

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
        let words: Vec<&str> = t.split_whitespace().collect();
        let first = words.first().map(|w| w.to_ascii_uppercase()).unwrap_or_default();
        let second = words.get(1).copied().unwrap_or("").trim_end_matches(';');
        // `[TEMPORARY | INDEX ORGANIZED] TABLE name IS`
        let table_at = words.iter().position(|w| w.eq_ignore_ascii_case("TABLE"));
        if let Some(at) = table_at.filter(|&at| words[..at].iter().all(|w| TABLE_PREFIXES.contains(&w.to_ascii_uppercase().as_str()))) {
            if let Some(name) = words.get(at + 1).filter(|_| words.get(at + 2).is_some_and(|w| w.eq_ignore_ascii_case("IS"))) {
                let temporary = words[..at].iter().any(|w| w.eq_ignore_ascii_case("TEMPORARY"));
                out.tables.push(TableDef { name: name.to_ascii_uppercase(), line: line_no, temporary });
            }
            pending = None;
            continue;
        }
        match first.as_str() {
            "LAYER" => out.layer = Some(second.to_string()),
            "VIEW" if !second.is_empty() => {
                out.views.push(ViewDef {
                    name: second.to_ascii_uppercase(),
                    line: line_no,
                    modifier: pending.take(),
                    reads: Vec::new(),
                    calls: Vec::new(),
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
    }
}
