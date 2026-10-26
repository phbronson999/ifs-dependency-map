//! DATATYPE_MISMATCH: a derived attribute whose declared `DATATYPE` / `LENGTH` doesn't match
//! where its value comes from.
//!
//! A derived attribute gets its value one of three ways:
//! - `DB_SQL_IMPLEMENTATION`: an expression, usually `Xxx_API.Fn(...)`;
//! - an association's `USE_DEFINITION`: a column of the referenced entity (`COLUMN_NAME`,
//!   exposed as `COLUMN_ALIAS` when set);
//! - the LU's hand-written view in its `.views` file: the select-list item of that name.
//!
//! The origin is followed through the declarations: a function's `RETURN` type, the
//! `%TYPE` of the variable it returns, a table's column type, an entity attribute, a view's
//! select-list expression. Where the chain can't be followed the attribute is skipped:
//! nothing is reported from a guess.

use crate::db;
use crate::entity::{Attribute, ParsedEntity};
use crate::model::*;
use crate::plsql;
use crate::sqlscope::FromItem;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Text,
    Number,
    Date,
    Boolean,
}

impl Kind {
    fn word(self) -> &'static str {
        match self {
            Kind::Text => "text",
            Kind::Number => "a number",
            Kind::Date => "a date",
            Kind::Boolean => "a PL/SQL BOOLEAN",
        }
    }
    fn datatype(self) -> &'static str {
        match self {
            Kind::Text => "TEXT",
            Kind::Number => "NUMBER",
            Kind::Date => "DATE",
            Kind::Boolean => "BOOLEAN",
        }
    }
}

/// What a value really is, and the declarations that say so, nearest first.
#[derive(Clone, Debug)]
struct Origin {
    kind: Kind,
    length: Option<u32>,
    steps: Vec<(usize, usize, String)>,
}

/// The parsed files the rule needs.
pub struct Inputs<'a> {
    pub entities: Vec<(usize, &'a ParsedEntity)>,
    pub views: Vec<(usize, &'a db::ViewDef)>,
    pub tables: Vec<(usize, &'a db::TableDef)>,
}

/// An entity `DATATYPE` as a kind. BOOLEAN attributes are stored as text flags and are left
/// out, like anything unknown.
fn attr_kind(datatype: Option<&str>) -> Option<Kind> {
    match datatype? {
        "TEXT" | "LOOKUP" | "ENUMERATION" => Some(Kind::Text),
        "NUMBER" | "INTEGER" => Some(Kind::Number),
        "DATE" | "TIMESTAMP" => Some(Kind::Date),
        _ => None,
    }
}

/// An Oracle / PL/SQL type (`VARCHAR2(20)`, `NUMBER`, `DATE`) as a kind and length.
fn sql_kind(t: &str) -> Option<(Kind, Option<u32>)> {
    let t = t.trim().to_ascii_uppercase();
    let word: String = t.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '_').collect();
    let length = t[word.len()..].trim_start().strip_prefix('(').and_then(|r| {
        let digits: String = r.trim_start().chars().take_while(|c| c.is_ascii_digit()).collect();
        digits.parse().ok()
    });
    let kind = match word.as_str() {
        "VARCHAR2" | "VARCHAR" | "CHAR" | "NVARCHAR2" | "NCHAR" | "CLOB" | "LONG" | "STRING" => Kind::Text,
        "NUMBER" | "INTEGER" | "INT" | "PLS_INTEGER" | "BINARY_INTEGER" | "FLOAT" | "DECIMAL" | "BINARY_DOUBLE" | "BINARY_FLOAT"
        | "NATURAL" | "POSITIVE" | "SIMPLE_INTEGER" | "SMALLINT" => Kind::Number,
        "DATE" | "TIMESTAMP" => Kind::Date,
        "BOOLEAN" => Kind::Boolean,
        _ => return None,
    };
    Some((kind, if kind == Kind::Text { length } else { None }))
}

fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '$' || c == '#'
}

/// `ab(cd)ef` → whether the parenthesis opened at `open` closes at the end of `s`.
fn closes_at_end(s: &str, open: usize) -> bool {
    let mut depth = 0;
    for (i, c) in s.char_indices().skip_while(|&(i, _)| i < open) {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return i == s.len() - 1;
                }
            }
            _ => {}
        }
    }
    false
}

/// Splits on commas at depth 0.
fn top_args(s: &str) -> Vec<&str> {
    let (mut depth, mut start, mut out, mut in_str) = (0, 0, Vec::new(), false);
    for (i, c) in s.char_indices() {
        match c {
            '\'' => in_str = !in_str,
            '(' if !in_str => depth += 1,
            ')' if !in_str => depth -= 1,
            ',' if !in_str && depth == 0 => {
                out.push(s[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(s[start..].trim());
    out
}

/// Splits on any of `ops` outside parentheses and literals (a leading sign is not a split).
fn split_top<'s>(s: &'s str, ops: &[char]) -> Vec<&'s str> {
    let (mut depth, mut in_str, mut start, mut out) = (0, false, 0, Vec::new());
    for (i, c) in s.char_indices() {
        match c {
            '\'' => in_str = !in_str,
            '(' if !in_str => depth += 1,
            ')' if !in_str => depth -= 1,
            _ if !in_str && depth == 0 && ops.contains(&c) && !s[start..i].trim().is_empty() => {
                out.push(s[start..i].trim());
                start = i + c.len_utf8();
            }
            _ => {}
        }
    }
    out.push(s[start..].trim());
    out
}

/// Whether `op` occurs in `s` outside parentheses and literals.
fn has_top(s: &str, op: &str) -> bool {
    let (mut depth, mut in_str) = (0, false);
    for (i, c) in s.char_indices() {
        match c {
            '\'' => in_str = !in_str,
            '(' if !in_str => depth += 1,
            ')' if !in_str => depth -= 1,
            _ if !in_str && depth == 0 && s[i..].starts_with(op) => return true,
            _ => {}
        }
    }
    false
}

struct Resolver<'a> {
    idx: &'a Index,
    /// LU (upper) → its entity files, Core first.
    lus: HashMap<String, Vec<(usize, &'a ParsedEntity)>>,
    /// Generated view or table name → (LU, is the table).
    generated: HashMap<String, (String, bool)>,
    /// View name → definitions, Cust first (a Cust @Override is what runs).
    views: HashMap<String, Vec<(usize, &'a db::ViewDef)>>,
    tables: HashMap<String, (usize, &'a db::TableDef)>,
    /// Package (upper) → unit index.
    packages: HashMap<String, usize>,
    /// (unit, method upper) → objects.
    methods: HashMap<(usize, String), Vec<usize>>,
    lines: RefCell<HashMap<usize, Vec<String>>>,
    /// Guards against cycles (a view reading itself, mutually recursive functions).
    visiting: RefCell<HashSet<String>>,
    /// How deep the current expression is nested.
    depth: RefCell<usize>,
}

impl<'a> Resolver<'a> {
    fn new(idx: &'a Index, inputs: &Inputs<'a>) -> Self {
        let mut lus: HashMap<String, Vec<(usize, &ParsedEntity)>> = HashMap::new();
        for &(f, e) in &inputs.entities {
            if let Some((lu, _)) = &e.name {
                lus.entry(lu.to_ascii_uppercase()).or_default().push((f, e));
            }
        }
        for files in lus.values_mut() {
            files.sort_by_key(|&(f, _)| idx.files[f].root != Layer::Core);
        }
        let mut generated = HashMap::new();
        for (key, files) in &lus {
            let lu = files[0].1.name.as_ref().unwrap().0.clone();
            let snake = plsql::snake_name(&lu).to_ascii_uppercase();
            // A Cust rename wins, as for the relations in the graph.
            let pick = |view: bool| {
                files.iter().rev().find_map(|(_, e)| if view { e.view.clone() } else { e.table.clone() }).map(|(n, _)| n)
            };
            generated.insert(pick(true).unwrap_or_else(|| snake.clone()), (key.clone(), false));
            generated.insert(pick(false).unwrap_or_else(|| format!("{snake}_TAB")), (key.clone(), true));
        }
        let mut views: HashMap<String, Vec<(usize, &db::ViewDef)>> = HashMap::new();
        for &(f, v) in &inputs.views {
            views.entry(v.name.clone()).or_default().push((f, v));
        }
        for defs in views.values_mut() {
            defs.sort_by_key(|&(f, _)| idx.files[f].root != Layer::Cust);
        }
        let tables = inputs.tables.iter().map(|&(f, t)| (t.name.clone(), (f, t))).collect();
        let mut packages = HashMap::new();
        for (ui, u) in idx.units.iter().enumerate() {
            if u.kind == UnitKind::Package {
                packages.insert(u.name.to_ascii_uppercase(), ui);
            }
        }
        let mut methods: HashMap<(usize, String), Vec<usize>> = HashMap::new();
        for (oi, o) in idx.objects.iter().enumerate() {
            if o.section.is_none() {
                methods.entry((id_index(&o.unit), o.name.to_ascii_uppercase())).or_default().push(oi);
            }
        }
        Resolver { idx, lus, generated, views, tables, packages, methods, lines: RefCell::default(), visiting: RefCell::default(), depth: RefCell::default() }
    }

    fn line(&self, file: usize, line: usize) -> String {
        let mut cache = self.lines.borrow_mut();
        let lines = cache.entry(file).or_insert_with(|| {
            std::fs::read(&self.idx.files[file].abs_path)
                .map(|b| String::from_utf8_lossy(&b).lines().map(|l| l.to_string()).collect())
                .unwrap_or_default()
        });
        line.checked_sub(1).and_then(|i| lines.get(i)).cloned().unwrap_or_default()
    }

    fn file_lines(&self, file: usize) -> Vec<String> {
        self.line(file, 1);
        self.lines.borrow()[&file].clone()
    }

    /// Runs `f` unless `key` is already being resolved further up.
    fn guarded<T>(&self, key: String, f: impl FnOnce() -> Option<T>) -> Option<T> {
        if !self.visiting.borrow_mut().insert(key.clone()) {
            return None;
        }
        let out = f();
        self.visiting.borrow_mut().remove(&key);
        out
    }

    fn attributes(&self, lu: &str) -> impl Iterator<Item = (usize, &'a Attribute)> + '_ {
        self.lus.get(&lu.to_ascii_uppercase()).into_iter().flatten().flat_map(|&(f, e)| e.attributes.iter().map(move |a| (f, a)))
    }

    /// A declared attribute's type.
    fn declared(&self, f: usize, a: &Attribute, lu: &str) -> Option<Origin> {
        let kind = attr_kind(a.datatype.as_deref())?;
        let len = a.length.map_or(String::new(), |l| format!("({l})"));
        Some(Origin {
            kind,
            length: if kind == Kind::Text { a.length } else { None },
            steps: vec![(f, a.line, format!("attribute {} of {lu}: {}{len}", a.name, a.datatype.as_deref().unwrap_or("")))],
        })
    }

    /// The type a column of a table or view really holds.
    fn column(&self, rel: &str, col: &str) -> Option<Origin> {
        let (rel, col) = (rel.to_ascii_uppercase(), col.to_ascii_uppercase());
        self.guarded(format!("col:{rel}.{col}"), || {
            if let Some((f, t)) = self.tables.get(&rel) {
                if let Some((_, ty)) = t.types.iter().find(|(c, _)| *c == col) {
                    let (kind, length) = sql_kind(ty)?;
                    return Some(Origin { kind, length, steps: vec![(*f, t.line, format!("column {col} of table {rel}: {ty}"))] });
                }
            }
            if let Some(defs) = self.views.get(&rel) {
                let (f, v) = defs[0];
                if let Some(item) = v.items.iter().find(|i| i.name.as_deref() == Some(col.as_str())) {
                    let mut o = self.expr(&item.expr, &v.from, None)?;
                    o.steps.insert(0, (f, item.line, format!("column {col} of view {rel}")));
                    return Some(o);
                }
            }
            if let Some((lu, is_table)) = self.generated.get(&rel) {
                let lu_name = self.lus[lu][0].1.name.as_ref().unwrap().0.clone();
                for (f, a) in self.attributes(lu) {
                    let c = plsql::snake_name(&a.name).to_ascii_uppercase();
                    if c == col {
                        // In the generated view a derived attribute is its expression.
                        if !is_table && a.derived {
                            if let Some(mut o) = self.derived(f, a, &lu_name) {
                                o.steps.insert(0, (f, a.line, format!("column {col} of view {rel} is the derived attribute {}", a.name)));
                                return Some(o);
                            }
                        }
                        return self.declared(f, a, &lu_name);
                    }
                    if a.enumeration && col == format!("{c}_DB") {
                        return Some(Origin { kind: Kind::Text, length: None, steps: vec![(f, a.line, format!("{col}: the database value of {}", a.name))] });
                    }
                }
            }
            None
        })
    }

    /// Where a derived attribute's value comes from.
    fn derived(&self, f: usize, a: &Attribute, lu: &str) -> Option<Origin> {
        if let Some(code) = &a.implementation {
            let mut o = self.expr(code, &[], Some(lu))?;
            o.steps.insert(0, (f, a.line, format!("DB_SQL_IMPLEMENTATION {}", code.split_whitespace().collect::<Vec<_>>().join(" "))));
            return Some(o);
        }
        // An association's USE_DEFINITION: a column of the referenced entity.
        for &(ef, e) in self.lus.get(&lu.to_ascii_uppercase()).into_iter().flatten() {
            for assoc in &e.associations {
                for u in &assoc.uses {
                    if !u.attribute().eq_ignore_ascii_case(&a.name) {
                        continue;
                    }
                    let target = self.attributes(&assoc.to_entity).find(|(_, t)| t.name.eq_ignore_ascii_case(&u.column))?;
                    let mut o = self.declared(target.0, target.1, &assoc.to_entity)?;
                    o.steps.insert(0, (ef, u.line, format!("USE_DEFINITION {} of {} (to {})", u.column, assoc.name, assoc.to_entity)));
                    return Some(o);
                }
            }
        }
        // The LU's hand-written view: the item of that name, in a view of the LU's .views file.
        let col = plsql::snake_name(&a.name).to_ascii_uppercase();
        for defs in self.views.values() {
            for &(vf, v) in defs {
                let stem = self.idx.files[vf].rel_path.rsplit('/').next().unwrap_or("").split('.').next().unwrap_or("").trim_end_matches("-Cust");
                if !stem.eq_ignore_ascii_case(lu) {
                    continue;
                }
                if let Some(item) = v.items.iter().find(|i| i.name.as_deref() == Some(col.as_str())) {
                    let mut o = self.expr(&item.expr, &v.from, None)?;
                    o.steps.insert(0, (vf, item.line, format!("column {col} of the hand-written view {}", v.name)));
                    return Some(o);
                }
            }
        }
        None
    }

    /// The type an SQL expression produces. `from` resolves `alias.col`; `own_lu` resolves a
    /// bare column in a DB_SQL_IMPLEMENTATION (the LU's own table).
    fn expr(&self, e: &str, from: &[FromItem], own_lu: Option<&str>) -> Option<Origin> {
        // A hard limit on nesting, whatever the expression: deeper chains aren't followed.
        if *self.depth.borrow() > 40 {
            return None;
        }
        *self.depth.borrow_mut() += 1;
        let out = self.expr_inner(e, from, own_lu);
        *self.depth.borrow_mut() -= 1;
        out
    }

    fn expr_inner(&self, e: &str, from: &[FromItem], own_lu: Option<&str>) -> Option<Origin> {
        let mut e = e.trim();
        while e.starts_with('(') && closes_at_end(e, 0) {
            e = e[1..e.len() - 1].trim();
        }
        let simple = |kind| Some(Origin { kind, length: None, steps: Vec::new() });
        if e.starts_with('\'') {
            return Some(Origin { kind: Kind::Text, length: Some(e.len().saturating_sub(2) as u32), steps: Vec::new() });
        }
        if e.starts_with(|c: char| c.is_ascii_digit()) && e.chars().all(|c| c.is_ascii_digit() || c == '.') {
            return simple(Kind::Number);
        }
        if e.eq_ignore_ascii_case("NULL") {
            return None;
        }
        if e.eq_ignore_ascii_case("SYSDATE") || e.eq_ignore_ascii_case("SYSTIMESTAMP") {
            return simple(Kind::Date);
        }
        // A call covering the whole expression: `Pkg_API.Fn(...)` or `NVL(...)`.
        if let Some(open) = e.find('(').filter(|&o| closes_at_end(e, o)) {
            let head = e[..open].trim();
            let args = top_args(&e[open + 1..e.len() - 1]);
            if let Some((pkg, method)) = head.split_once('.') {
                if pkg.len() > 4 && pkg[pkg.len() - 4..].eq_ignore_ascii_case("_API") {
                    let m = method.to_ascii_uppercase();
                    if m == "DECODE" || m == "GET_CLIENT_VALUE" || m == "ENCODE" || m == "GET_DB_VALUE" {
                        return simple(Kind::Text);
                    }
                    let argc = if args.len() == 1 && args[0].is_empty() { 0 } else { args.len() };
                    return self.function(pkg, method, Some(argc));
                }
                return None;
            }
            return match head.to_ascii_uppercase().as_str() {
                "NVL" | "NVL2" | "COALESCE" | "MAX" | "MIN" | "GREATEST" | "LEAST" | "TRUNC" | "ROUND" if !args.is_empty() => {
                    let first = if head.eq_ignore_ascii_case("NVL2") { args.get(1).copied()? } else { args[0] };
                    self.expr(first, from, own_lu)
                }
                "TO_CHAR" | "UPPER" | "LOWER" | "SUBSTR" | "LTRIM" | "RTRIM" | "TRIM" | "REPLACE" | "INITCAP" | "CONCAT" | "LPAD" | "RPAD" => simple(Kind::Text),
                "TO_NUMBER" | "COUNT" | "SUM" | "ABS" | "LENGTH" | "INSTR" | "AVG" | "MOD" | "SIGN" | "CEIL" | "FLOOR" => simple(Kind::Number),
                "TO_DATE" | "ADD_MONTHS" | "LAST_DAY" | "TO_TIMESTAMP" => simple(Kind::Date),
                _ => None,
            };
        }
        if has_top(e, "||") {
            return simple(Kind::Text);
        }
        // Arithmetic. A date plus or minus days is a date; two dates subtracted, or anything
        // else on numbers, is a number.
        if !e.contains('\'') && (has_top(e, "+") || has_top(e, "-")) {
            let operands = split_top(e, &['+', '-']);
            if operands.len() < 2 {
                return simple(Kind::Number); // a signed number, `-x`
            }
            let dates = operands.iter().filter(|op| self.expr(op, from, own_lu).is_some_and(|o| o.kind == Kind::Date)).count();
            return simple(if dates == 1 { Kind::Date } else { Kind::Number });
        }
        if !e.contains('\'') && (has_top(e, "*") || has_top(e, "/")) {
            return simple(Kind::Number);
        }
        // `a.col` or `col`.
        if e.chars().all(|c| is_name_char(c) || c == '.') {
            if let Some((alias, col)) = e.split_once('.') {
                let rel = from.iter().find(|f| f.alias.as_deref().is_some_and(|a| a.eq_ignore_ascii_case(alias)) || f.name.eq_ignore_ascii_case(alias))?;
                return self.column(&rel.name, col);
            }
            if from.len() == 1 && !from[0].name.is_empty() {
                return self.column(&from[0].name, e);
            }
            if let Some(lu) = own_lu {
                let table = self.generated.iter().find(|(_, (l, t))| *t && l.eq_ignore_ascii_case(lu)).map(|(n, _)| n.clone())?;
                return self.column(&table, e);
            }
        }
        None
    }

    /// The type a PL/SQL function returns: its RETURN clause, refined through the `%TYPE`
    /// of the variable or record field it returns.
    fn function(&self, pkg: &str, method: &str, argc: Option<usize>) -> Option<Origin> {
        let key = format!("fn:{}.{}/{argc:?}", pkg.to_ascii_uppercase(), method.to_ascii_uppercase());
        self.guarded(key, || {
            let &u = self.packages.get(&pkg.to_ascii_uppercase())?;
            let candidates = self.methods.get(&(u, method.to_ascii_uppercase()))?;
            // Overloads: the one taking as many parameters as the call passes.
            let oi = candidates
                .iter()
                .copied()
                .find(|&c| argc.is_some() && self.arity(c) == argc)
                .or_else(|| candidates.first().copied())?;
            let o = &self.idx.objects[oi];
            if o.generated {
                let lu = self.lus.iter().find(|(_, files)| files.iter().any(|&(f, _)| f == o.file)).map(|(k, _)| k.clone())?;
                let lu_name = self.lus[&lu][0].1.name.as_ref().unwrap().0.clone();
                let upper = o.name.to_ascii_uppercase();
                if let Some(attr) = upper.strip_prefix("GET_") {
                    let (f, a) = self.attributes(&lu).find(|(_, a)| plsql::snake_name(&a.name).eq_ignore_ascii_case(attr))?;
                    let mut d = self.declared(f, a, &lu_name)?;
                    d.steps.insert(0, (o.file, o.line, format!("{pkg}.{} is generated from attribute {}", o.name, a.name)));
                    return Some(d);
                }
                return None;
            }
            let lines = self.file_lines(o.file);
            let start = o.line.saturating_sub(1);
            let end_tag = format!("END {}", o.name).to_ascii_uppercase();
            let end = (start..lines.len()).find(|&i| lines[i].trim_start().to_ascii_uppercase().starts_with(&end_tag)).unwrap_or(lines.len().saturating_sub(1));
            let body: Vec<(usize, String)> =
                (start..=end).map(|i| (i + 1, plsql::blank_comments_and_strings(&lines[i]).to_string())).collect();
            // Header: up to the line where IS / AS opens the body.
            let is_line = body.iter().position(|(_, l)| {
                let u = l.to_ascii_uppercase();
                u.split(|c: char| !is_name_char(c)).any(|w| w == "IS" || w == "AS")
            })?;
            let header: String = body[..=is_line].iter().map(|(_, l)| l.as_str()).collect::<Vec<_>>().join(" ");
            let hu = header.to_ascii_uppercase();
            let r = hu.rfind("RETURN")?;
            let rest = &header[r + 6..];
            let cut = rest.to_ascii_uppercase().find(|c: char| !is_name_char(c) && c != '.' && c != '%' && c != '(' && c != ')' && !c.is_whitespace()).unwrap_or(rest.len());
            let mut ret_type = rest[..cut].trim().to_string();
            for kw in [" IS", " AS"] {
                if ret_type.to_ascii_uppercase().ends_with(kw) {
                    ret_type.truncate(ret_type.len() - kw.len());
                }
            }
            let ret_type = ret_type.trim().to_string();
            let ret_line = body.iter().find(|(_, l)| l.to_ascii_uppercase().contains("RETURN")).map_or(o.line, |(n, _)| *n);
            let mut declared = self.type_text(&ret_type, o.file, ret_line);
            if let Some(d) = &mut declared {
                d.steps.insert(0, (o.file, ret_line, format!("{pkg}.{method} returns {ret_type}")));
            }
            // Refine a bare VARCHAR2 (or NUMBER) through what is actually returned.
            if declared.as_ref().is_none_or(|d| d.length.is_none() || !ret_type.contains('%')) {
                let begin = body.iter().position(|(_, l)| l.trim().eq_ignore_ascii_case("BEGIN")).unwrap_or(is_line);
                let mut returned: Vec<(usize, String)> = Vec::new();
                for (n, l) in &body[begin..] {
                    let t = l.trim();
                    if t.len() > 7 && t[..7].eq_ignore_ascii_case("RETURN ") {
                        let ex = t[7..].trim_end_matches(';').trim().trim_start_matches('(').trim_end_matches(')').trim().to_string();
                        if !ex.eq_ignore_ascii_case("NULL") && !returned.iter().any(|(_, r)| r.eq_ignore_ascii_case(&ex)) {
                            returned.push((*n, ex));
                        }
                    }
                }
                if let [(n, ex)] = returned.as_slice() {
                    if let Some(mut refined) = self.returned(ex, &lines, start, begin + start, o.file) {
                        refined.steps.insert(0, (o.file, *n, format!("returns {ex}")));
                        if let Some(d) = &declared {
                            refined.steps.insert(0, d.steps[0].clone());
                        }
                        if declared.as_ref().is_none_or(|d| d.kind == refined.kind) {
                            return Some(refined);
                        }
                    }
                }
            }
            declared
        })
    }

    /// How many parameters a method takes: the LU's keys for a generated getter, the
    /// declared ones for a hand-written method.
    fn arity(&self, oi: usize) -> Option<usize> {
        let o = &self.idx.objects[oi];
        if o.generated {
            let lu = self.lus.values().find(|files| files.iter().any(|&(f, _)| f == o.file))?;
            return Some(lu.iter().flat_map(|(_, e)| &e.attributes).filter(|a| a.key).count());
        }
        let lines = self.file_lines(o.file);
        let mut header = String::new();
        for l in lines.iter().skip(o.line.saturating_sub(1)).take(60) {
            let clean = plsql::blank_comments_and_strings(l);
            header.push_str(&clean);
            header.push(' ');
            let u = clean.to_ascii_uppercase();
            if u.split(|c: char| !is_name_char(c)).any(|w| w == "IS" || w == "AS") {
                break;
            }
        }
        let after_name = header.to_ascii_uppercase().find(&o.name.to_ascii_uppercase()).map(|p| p + o.name.len())?;
        let rest = header[after_name..].trim_start();
        if !rest.starts_with('(') {
            return Some(0);
        }
        let close = (0..rest.len()).find(|&i| closes_at_end(&rest[..=i], 0))?;
        let inside = rest[1..close].trim();
        Some(if inside.is_empty() { 0 } else { top_args(inside).len() })
    }

    /// The type of what a function returns: a variable `x_` or a record field `rec_.field`,
    /// looked up in the function's declarations, then the package body's.
    fn returned(&self, ex: &str, lines: &[String], fn_start: usize, fn_begin: usize, file: usize) -> Option<Origin> {
        let (var, field) = match ex.split_once('.') {
            Some((v, f)) => (v, Some(f)),
            None => (ex, None),
        };
        if !var.chars().all(is_name_char) || field.is_some_and(|f| !f.chars().all(is_name_char)) {
            return None;
        }
        let decl = |i: usize| -> Option<String> {
            let t = lines[i].trim();
            let rest = t.get(..var.len()).filter(|h| h.eq_ignore_ascii_case(var)).map(|_| &t[var.len()..])?;
            if !rest.starts_with(char::is_whitespace) {
                return None;
            }
            let ty = rest.trim().split(":=").next()?.trim().trim_end_matches(';').trim();
            (!ty.is_empty()).then(|| ty.to_string())
        };
        // In the function's declare section, then anywhere in the file (package-level).
        let (line, ty) = (fn_start..fn_begin.min(lines.len()))
            .chain(0..lines.len())
            .find_map(|i| decl(i).map(|t| (i + 1, t)))?;
        let upper = ty.to_ascii_uppercase();
        let mut o = match field {
            None => self.type_text(&ty, file, line)?,
            Some(f) => {
                if let Some(rel) = upper.strip_suffix("%ROWTYPE") {
                    self.column(rel.trim(), f)?
                } else if let Some(pkg) = upper.strip_suffix(".PUBLIC_REC") {
                    let (lu, _) = self.lus.iter().find(|(k, _)| plsql::package_name(&self.lus[*k][0].1.name.as_ref().unwrap().0).eq_ignore_ascii_case(pkg))?;
                    let lu_name = self.lus[lu][0].1.name.as_ref().unwrap().0.clone();
                    let (af, a) = self.attributes(lu).find(|(_, a)| plsql::snake_name(&a.name).eq_ignore_ascii_case(f))?;
                    self.declared(af, a, &lu_name)?
                } else {
                    return None;
                }
            }
        };
        o.steps.insert(0, (file, line, format!("{var} is declared {ty}")));
        Some(o)
    }

    /// A type as written in PL/SQL: `VARCHAR2(20)`, `NUMBER`, `x_tab.col%TYPE`.
    fn type_text(&self, ty: &str, file: usize, line: usize) -> Option<Origin> {
        let upper = ty.trim().to_ascii_uppercase();
        if let Some(r) = upper.strip_suffix("%TYPE") {
            let parts: Vec<&str> = r.trim().split('.').collect();
            return match parts.as_slice() {
                [rel, col] => self.column(rel, col),
                _ => None,
            };
        }
        let (kind, length) = sql_kind(&upper)?;
        Some(Origin { kind, length, steps: vec![(file, line, ty.to_string())] })
    }
}

pub fn run(idx: &Index, inputs: Inputs) -> Vec<Finding> {
    let r = Resolver::new(idx, &inputs);
    let mut out = Vec::new();

    // Callers of generated methods, for the impact of a public attribute.
    let mut callers: HashMap<&str, Vec<&Edge>> = HashMap::new();
    for e in &idx.edges {
        if e.kind == EdgeKind::Invokes {
            callers.entry(e.to.as_str()).or_default().push(e);
        }
    }

    let mut lus: Vec<&String> = r.lus.keys().collect();
    lus.sort();
    for key in lus {
        let files = &r.lus[key];
        let lu = files[0].1.name.as_ref().unwrap().0.clone();
        for &(f, e) in files {
            for a in &e.attributes {
                if !a.derived || a.name.is_empty() {
                    continue;
                }
                let Some(declared) = attr_kind(a.datatype.as_deref()) else { continue };
                let Some(origin) = r.derived(f, a, &lu) else { continue };
                let length_short = declared == Kind::Text
                    && origin.kind == Kind::Text
                    && matches!((a.length, origin.length), (Some(d), Some(o)) if d < o);
                if origin.kind == declared && !length_short {
                    continue;
                }
                out.push(finding(idx, &r, &callers, f, a, &lu, declared, &origin, length_short));
            }
        }
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn finding(
    idx: &Index,
    r: &Resolver,
    callers: &HashMap<&str, Vec<&Edge>>,
    f: usize,
    a: &Attribute,
    lu: &str,
    declared: Kind,
    origin: &Origin,
    length_short: bool,
) -> Finding {
    let len = |l: Option<u32>| l.map_or(String::new(), |l| format!(" ({l})"));
    let origin_desc = format!("{}{}", origin.kind.word(), len(origin.length));
    let decl_desc = format!("{}{}", a.datatype.as_deref().unwrap_or(""), len(a.length));
    let (severity, title, what) = if length_short {
        (
            Severity::Warning,
            format!("{lu}.{} is {decl_desc} but its value can be longer", a.name),
            format!("is declared {decl_desc}, but its value is {origin_desc}, so longer values don't fit"),
        )
    } else {
        (
            Severity::Error,
            format!("{lu}.{} is {} but its value is {}", a.name, declared.datatype(), origin.kind.word()),
            format!(
                "is declared {decl_desc}, but its value is {origin_desc}. Wherever it is read as {} (its view column, the record `Get()` returns), a value that doesn't convert fails with \"numeric or value error\" (ORA-06502)",
                declared.datatype()
            ),
        )
    };
    // The generated methods that fill it.
    let pkg = plsql::package_name(lu);
    let gen = |name: &str| {
        idx.objects.iter().find(|o| o.generated && o.name.eq_ignore_ascii_case(name) && idx.units[id_index(&o.unit)].name.eq_ignore_ascii_case(&pkg))
    };
    let in_get = a.public && !a.exclude_get;
    let get = if in_get { gen("Get") } else { None };
    let getter = if in_get { gen(&format!("Get_{}", plsql::snake_name(&a.name))) } else { None };
    let get_calls = get.map_or(0, |g| callers.get(g.id.as_str()).map_or(0, |v| v.len()));
    let mut message = format!("{} {what}.", a.name);
    if get.is_some() {
        message.push_str(&format!(
            " It is public, so it is a field of {pkg}.Public_Rec and every {pkg}.Get() fills it: {get_calls} place{} call {}.Get().",
            if get_calls == 1 { "" } else { "s" },
            pkg
        ));
    }
    let mut f_out = Finding {
        id: String::new(),
        severity,
        rule: "DATATYPE_MISMATCH".into(),
        title,
        message,
        nodes: [get, getter].into_iter().flatten().map(|o| o.id.clone()).collect(),
        edges: vec![],
        location: Some(Location { file: f, line: a.line }),
        suggestion: None,
        evidence: vec![],
        involves_cust: false,
        diff: None,
        upgrade: false,
    };
    let step = |file: usize, line: usize, note: &str| Step { file, line, text: r.line(file, line).trim().to_string(), note: Some(note.to_string()) };
    f_out.evidence.push(Trail { title: format!("Declared {decl_desc}"), steps: vec![step(f, a.line, &format!("attribute {} of {lu}", a.name))] });
    f_out.evidence.push(Trail {
        title: format!("Where its value comes from: {origin_desc}"),
        steps: origin.steps.iter().map(|(sf, sl, note)| step(*sf, *sl, note)).collect(),
    });
    // A way the value is read: from a view, through calls, to the generated Get().
    if let Some(g) = get {
        if let Some(path) = path_from_view(callers, &g.id) {
            f_out.evidence.push(Trail {
                title: format!("One of the {get_calls} paths into {pkg}.Get()"),
                steps: path.iter().map(|e| step(e.file, e.line, &describe(idx, e))).collect(),
            });
        }
    }
    let fix_type = if length_short { None } else { Some(origin.kind.datatype()) };
    let mut snippet = String::new();
    if let Some(t) = fix_type {
        snippet.push_str(&format!("- <DATATYPE>{}</DATATYPE>\n+ <DATATYPE>{t}</DATATYPE>", a.datatype.as_deref().unwrap_or("")));
    }
    if let Some(l) = origin.length.filter(|_| origin.kind == Kind::Text) {
        if !snippet.is_empty() {
            snippet.push('\n');
        }
        match a.length {
            Some(old) => snippet.push_str(&format!("- <LENGTH>{old}</LENGTH>\n+ <LENGTH>{l}</LENGTH>")),
            None => snippet.push_str(&format!("+ <LENGTH>{l}</LENGTH>")),
        }
    }
    f_out.suggestion = Some(Suggestion {
        summary: format!(
            "Declare {} as {origin_desc}, the type its value has. Projection attributes declared over it need the same type.",
            a.name
        ),
        snippet: (!snippet.is_empty()).then_some(snippet),
        target: Some(idx.files[f].rel_path.clone()),
    });
    f_out
}

fn describe(idx: &Index, e: &Edge) -> String {
    let name = |id: &str| -> String {
        if let Some(rest) = id.strip_prefix('r') {
            return format!("view {}", idx.relations[rest.parse::<usize>().unwrap_or(0)].name);
        }
        let o = &idx.objects[id_index(id)];
        format!("{}.{}", idx.units[id_index(&o.unit)].name, o.name)
    };
    format!("{} calls {}", name(&e.from), name(&e.to))
}

/// The shortest chain of calls from a view to `target` (walking callers backwards), if any.
fn path_from_view<'a>(callers: &HashMap<&str, Vec<&'a Edge>>, target: &str) -> Option<Vec<&'a Edge>> {
    let mut prev: HashMap<String, &Edge> = HashMap::new();
    let mut queue = VecDeque::from([target.to_string()]);
    let mut seen = HashSet::from([target.to_string()]);
    while let Some(x) = queue.pop_front() {
        if x.starts_with('r') {
            let mut path = Vec::new();
            let mut cur = x;
            while let Some(e) = prev.get(&cur) {
                path.push(*e);
                cur = e.to.clone();
            }
            return Some(path);
        }
        if seen.len() > 20_000 {
            return None;
        }
        for e in callers.get(x.as_str()).into_iter().flatten() {
            if seen.insert(e.from.clone()) {
                prev.insert(e.from.clone(), e);
                queue.push_back(e.from.clone());
            }
        }
    }
    None
}
