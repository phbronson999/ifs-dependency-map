//! Query blocks in SQL (text from `prepare`, comments and literals blanked): each SELECT (a
//! subquery or a UNION branch is a block of its own) with the relations its FROM names and
//! the column names it uses without an alias prefix. Oracle resolves an unprefixed name in
//! the innermost block that has a relation with that column, and fails to compile if two do.

#[derive(Debug, Clone, PartialEq)]
pub struct FromItem {
    /// Upper-cased relation name; empty for a subquery or `TABLE(...)` in the FROM.
    pub name: String,
    pub alias: Option<String>,
    pub line: usize,
}

#[derive(Debug, Default, Clone)]
pub struct Block {
    pub select_line: usize,
    pub from: Vec<FromItem>,
    /// Unprefixed names, upper-cased, with their lines. Includes some that are not columns
    /// (functions without arguments, PL/SQL names); callers match them against column names.
    pub refs: Vec<(String, usize)>,
}

/// Blanks comments and the contents of string literals, keeping the quotes, so that
/// `'A' type_db` still reads as a literal followed by an alias. Newlines are kept.
pub fn prepare(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        let (c, next) = (chars[i], chars.get(i + 1).copied());
        if c == '-' && next == Some('-') {
            while i < chars.len() && chars[i] != '\n' {
                out.push(' ');
                i += 1;
            }
        } else if c == '/' && next == Some('*') {
            out.push_str("  ");
            i += 2;
            while i < chars.len() && !(chars[i] == '*' && chars.get(i + 1) == Some(&'/')) {
                out.push(if chars[i] == '\n' { '\n' } else { ' ' });
                i += 1;
            }
            out.push_str("  ");
            i += 2;
        } else if c == '\'' {
            out.push('\'');
            i += 1;
            while i < chars.len() {
                if chars[i] == '\'' && chars.get(i + 1) == Some(&'\'') {
                    out.push_str("  "); // '' inside a literal
                    i += 2;
                } else if chars[i] == '\'' {
                    out.push('\'');
                    i += 1;
                    break;
                } else {
                    out.push(if chars[i] == '\n' { '\n' } else { ' ' });
                    i += 1;
                }
            }
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}

/// Literals that can end a select-list expression, so a name after them is an alias.
const LITERALS: &[&str] = &["NULL", "SYSDATE", "SYSTIMESTAMP", "USER", "ROWNUM", "LEVEL", "TRUE", "FALSE", "ROWID"];

#[derive(Debug, Clone, Copy, PartialEq)]
enum Tok<'a> {
    Name(&'a str),
    Punct(char),
}

fn tokens(cleaned: &str) -> Vec<(Tok<'_>, usize)> {
    let mut out = Vec::new();
    for (i, line) in cleaned.lines().enumerate() {
        let mut j = 0;
        while j < line.len() {
            let c = line[j..].chars().next().unwrap();
            if is_name_char(c) {
                let end = line[j..].find(|c: char| !is_name_char(c)).map_or(line.len(), |e| j + e);
                out.push((Tok::Name(&line[j..end]), i + 1));
                j = end;
            } else {
                if !c.is_whitespace() {
                    out.push((Tok::Punct(c), i + 1));
                }
                j += c.len_utf8();
            }
        }
    }
    out
}

fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '$' || c == '#'
}

/// Words that are never column references or aliases.
const KEYWORDS: &[&str] = &[
    "SELECT", "DISTINCT", "UNIQUE", "ALL", "FROM", "WHERE", "AND", "OR", "NOT", "NULL", "IS", "IN", "EXISTS", "BETWEEN",
    "LIKE", "ESCAPE", "CASE", "WHEN", "THEN", "ELSE", "END", "AS", "ON", "JOIN", "LEFT", "RIGHT", "INNER", "OUTER",
    "FULL", "CROSS", "NATURAL", "USING", "GROUP", "BY", "ORDER", "HAVING", "UNION", "MINUS", "INTERSECT", "EXCEPT",
    "ASC", "DESC", "NULLS", "FIRST", "LAST", "CONNECT", "PRIOR", "START", "WITH", "LEVEL", "ROWNUM", "ROWID", "SYSDATE",
    "SYSTIMESTAMP", "USER", "INTO", "BULK", "COLLECT", "FOR", "UPDATE", "OF", "NOWAIT", "WAIT", "SKIP", "LOCKED",
    "OVER", "PARTITION", "ROWS", "RANGE", "UNBOUNDED", "PRECEDING", "FOLLOWING", "CURRENT", "ROW", "FETCH", "NEXT",
    "ONLY", "OFFSET", "TRUE", "FALSE", "DATE", "TIMESTAMP", "INTERVAL", "LATERAL", "TABLE", "THE", "PIVOT", "UNPIVOT",
    "DUAL", "SOME", "ANY", "SIBLINGS", "KEEP", "DENSE_RANK", "WITHIN", "NOCYCLE", "MEMBER", "SUBMULTISET", "MULTISET",
];

/// Ends a FROM list when it appears at the block's own depth.
const FROM_END: &[&str] = &["WHERE", "GROUP", "ORDER", "HAVING", "CONNECT", "START", "FOR", "FETCH", "OFFSET", "PIVOT", "UNPIVOT"];

#[derive(Debug, Clone, Copy, PartialEq)]
enum State {
    List,  // the select list
    Into,  // SELECT ... INTO vars
    From,  // the FROM list
    Cond,  // ON, WHERE and everything after
}

struct Open {
    depth: i32,
    state: State,
    expect_relation: bool,
    /// The last FROM item can still take an alias.
    alias_slot: bool,
    block: Block,
    /// Select-list aliases: an unprefixed ORDER BY name may be one of these.
    aliases: Vec<String>,
}

pub fn blocks(cleaned: &str) -> Vec<Block> {
    let toks = tokens(cleaned);
    let mut out = Vec::new();
    let mut stack: Vec<Open> = Vec::new();
    let mut depth = 0i32;
    let mut sibling: Option<(i32, Vec<String>)> = None;
    let up = |t: &str| t.to_ascii_uppercase();
    let close = |stack: &mut Vec<Open>, out: &mut Vec<Block>| {
        let o = stack.pop().unwrap();
        let mut b = o.block;
        b.refs.retain(|(r, _)| !o.aliases.contains(r));
        out.push(b);
    };
    for i in 0..toks.len() {
        let (tok, line) = toks[i];
        let prev = i.checked_sub(1).map(|k| toks[k].0);
        let next = toks.get(i + 1).map(|t| t.0);
        match tok {
            Tok::Punct('(') => depth += 1,
            Tok::Punct(')') => {
                depth -= 1;
                while stack.last().is_some_and(|o| o.depth > depth) {
                    close(&mut stack, &mut out);
                }
            }
            Tok::Punct(';') => {
                while !stack.is_empty() {
                    close(&mut stack, &mut out);
                }
                depth = 0;
            }
            Tok::Punct(',') => {
                if let Some(o) = stack.last_mut() {
                    if o.depth == depth && o.state == State::From {
                        o.expect_relation = true;
                        o.alias_slot = false;
                    }
                }
            }
            Tok::Punct(_) => {}
            Tok::Name(t) => {
                let u = up(t);
                if u == "SELECT" {
                    // A subquery directly in a FROM list is an item with unknown columns.
                    if let Some(o) = stack.last_mut() {
                        if o.depth + 1 == depth && o.state == State::From && o.expect_relation {
                            o.block.from.push(FromItem { name: String::new(), alias: None, line });
                            o.expect_relation = false;
                            o.alias_slot = true;
                        }
                    }
                    // After a set operator, a closing ORDER BY may name the first branch's aliases.
                    let aliases = match sibling.take() {
                        Some((d, a)) if d == depth => a,
                        _ => Vec::new(),
                    };
                    stack.push(Open {
                        depth,
                        state: State::List,
                        expect_relation: false,
                        alias_slot: false,
                        block: Block { select_line: line, ..Default::default() },
                        aliases,
                    });
                    continue;
                }
                let Some(o) = stack.last_mut() else { continue };
                let own = o.depth == depth;
                if own && matches!(u.as_str(), "UNION" | "MINUS" | "INTERSECT" | "EXCEPT") {
                    sibling = Some((depth, o.aliases.clone()));
                    close(&mut stack, &mut out);
                    continue;
                }
                if own {
                    match (o.state, u.as_str()) {
                        (State::List, "INTO") => { o.state = State::Into; continue; }
                        (State::List | State::Into, "FROM") => {
                            o.state = State::From;
                            o.expect_relation = true;
                            continue;
                        }
                        (State::From, "JOIN") => { o.expect_relation = true; o.alias_slot = false; continue; }
                        (State::From, "ON" | "USING") => { o.state = State::Cond; o.alias_slot = false; continue; }
                        (State::Cond, "JOIN") => { o.state = State::From; o.expect_relation = true; continue; }
                        (State::From, w) if FROM_END.contains(&w) => { o.state = State::Cond; o.alias_slot = false; continue; }
                        _ => {}
                    }
                }
                match o.state {
                    State::Into => {}
                    State::From if own => {
                        if o.expect_relation {
                            if KEYWORDS.contains(&u.as_str()) && u != "TABLE" && u != "LATERAL" {
                                continue; // LEFT / OUTER before JOIN, etc.
                            }
                            // `&AO.customer_info` or `owner.name`: the name is the last part.
                            if next == Some(Tok::Punct('.')) {
                                continue;
                            }
                            let name = if u == "TABLE" || u == "LATERAL" || next == Some(Tok::Punct('(')) { String::new() } else { u };
                            o.block.from.push(FromItem { name, alias: None, line });
                            o.expect_relation = false;
                            o.alias_slot = true;
                        } else if o.alias_slot && !KEYWORDS.contains(&u.as_str()) {
                            if let Some(f) = o.block.from.last_mut() {
                                f.alias = Some(u);
                            }
                            o.alias_slot = false;
                        }
                    }
                    State::From => {}
                    State::List | State::Cond => {
                        let qualified = prev == Some(Tok::Punct('.')) || next == Some(Tok::Punct('.'));
                        let call = next == Some(Tok::Punct('('));
                        let bind = matches!(prev, Some(Tok::Punct(':' | '&')));
                        if qualified || call || bind || KEYWORDS.contains(&u.as_str()) || u.ends_with('_')
                            || u.starts_with(|c: char| c.is_ascii_digit() || c == '$')
                        {
                            continue;
                        }
                        // `expr alias` / `expr AS alias` in the select list (at the block's own depth).
                        if o.state == State::List && own {
                            let after_expr = match prev {
                                Some(Tok::Name(p)) => {
                                    let pu = up(p);
                                    pu == "AS" || pu == "END" || LITERALS.contains(&pu.as_str()) || !KEYWORDS.contains(&pu.as_str())
                                }
                                Some(Tok::Punct(')' | '\'')) => true,
                                _ => false,
                            };
                            if after_expr && matches!(next, None | Some(Tok::Punct(',')) | Some(Tok::Name(_))) {
                                o.aliases.push(u);
                                continue;
                            }
                        }
                        o.block.refs.push((u, line));
                    }
                }
            }
        }
    }
    while !stack.is_empty() {
        close(&mut stack, &mut out);
    }
    out
}

/// The column names a view's query produces: each select-list item's alias, or the column
/// it names. None when the list has `*` or an unnamed expression.
pub fn select_columns(cleaned: &str) -> Option<Vec<String>> {
    let toks = tokens(cleaned);
    let start = toks.iter().position(|(t, _)| matches!(t, Tok::Name(n) if n.eq_ignore_ascii_case("SELECT")))?;
    let mut items: Vec<Vec<Tok>> = vec![Vec::new()];
    let mut depth = 0;
    for &(t, _) in &toks[start + 1..] {
        match t {
            Tok::Punct('(') => depth += 1,
            Tok::Punct(')') => depth -= 1,
            Tok::Punct(',') if depth == 0 => { items.push(Vec::new()); continue; }
            Tok::Name(n) if depth == 0 && n.eq_ignore_ascii_case("FROM") => break,
            _ => {}
        }
        items.last_mut().unwrap().push(t);
    }
    let mut out = Vec::new();
    for item in items {
        let item: Vec<Tok> = item.into_iter().filter(|t| !matches!(t, Tok::Name(n) if n.eq_ignore_ascii_case("DISTINCT") || n.eq_ignore_ascii_case("UNIQUE"))).collect();
        match item.as_slice() {
            [.., Tok::Punct('*')] => return None,
            [Tok::Name(c)] | [Tok::Name(_), Tok::Punct('.'), Tok::Name(c)] => out.push(c.to_ascii_uppercase()),
            [.., p, Tok::Name(a)] if !matches!(p, Tok::Punct('.')) => out.push(a.to_ascii_uppercase()),
            _ => return None,
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refs(b: &Block) -> Vec<&str> {
        b.refs.iter().map(|(r, _)| r.as_str()).collect()
    }

    #[test]
    fn blocks_with_their_from_lists_and_unprefixed_names() {
        let src = "CURSOR c IS\n   SELECT cia.customer_id party_id, state, NVL(city, 'x') city_name, ci.name AS cname\n\
                   FROM &AO.customer_info ci, customer_info_address cia\n\
                   WHERE cia.customer_id = ci.customer_id AND country = country_\n\
                   AND EXISTS (SELECT 1 FROM customer_tax_tab t WHERE t.customer_id = ci.customer_id AND zip_code IS NOT NULL)\n\
                   UNION ALL\n   SELECT s.supplier_id, state, NULL tax_db, 'S' type_db FROM supplier_info s JOIN supplier_info_address sa ON sa.supplier_id = s.supplier_id\n\
                   ORDER BY party_id;\n\
                   SELECT x INTO rec_ FROM dual;\n";
        let b = blocks(&prepare(src));
        assert_eq!(b.len(), 4);
        // Blocks close innermost first: the EXISTS subquery, the first branch, the second, then the next statement.
        assert_eq!(b[0].from, [FromItem { name: "CUSTOMER_TAX_TAB".into(), alias: Some("T".into()), line: 5 }]);
        assert_eq!(refs(&b[0]), ["ZIP_CODE"]);
        let names: Vec<(&str, Option<&str>)> = b[1].from.iter().map(|f| (f.name.as_str(), f.alias.as_deref())).collect();
        assert_eq!(names, [("CUSTOMER_INFO", Some("CI")), ("CUSTOMER_INFO_ADDRESS", Some("CIA"))]);
        // Aliases (party_id, city_name, cname), prefixed names, calls and variables are left out.
        assert_eq!(refs(&b[1]), ["STATE", "CITY", "COUNTRY"]);
        assert_eq!(b[1].refs[0].1, 2);
        let names: Vec<&str> = b[2].from.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["SUPPLIER_INFO", "SUPPLIER_INFO_ADDRESS"]);
        // tax_db and type_db are aliases of a NULL and a literal; party_id is the first branch's alias.
        assert_eq!(refs(&b[2]), ["STATE"]);
        assert!(b[3].from.is_empty(), "dual is not a relation with columns");
        assert_eq!(refs(&b[3]), ["X"]);
    }

    #[test]
    fn view_select_columns() {
        let cols = |s: &str| select_columns(&prepare(s));
        assert_eq!(
            cols("VIEW v IS\nSELECT a.order_no, contract, NVL(b.qty, 0) qty, a.rowversion AS objversion\nFROM a_tab a, b_tab b;"),
            Some(vec!["ORDER_NO".into(), "CONTRACT".into(), "QTY".into(), "OBJVERSION".into()])
        );
        assert_eq!(cols("VIEW v IS SELECT a.* FROM a_tab a;"), None);
        assert_eq!(cols("VIEW v IS SELECT NVL(x, 0) FROM a_tab;"), None);
    }
}
