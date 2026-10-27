//! One PL/SQL method's calls and table reads and writes in the order they run, inside the
//! IF / CASE / exception branches and the loops they sit in. Conditional compilation
//! (`$IF Component_X_SYS.INSTALLED $THEN … $END`) isn't a branch here: its lines are skipped
//! and the code inside them is traced as if they weren't there. The Trace view asks for
//! one method at a time, so nothing here goes into the index.
//!
//! Within a statement, a call runs after the calls in its arguments: `A(B(x))` gives B, then A.
//! A fork keeps every branch; one with no ELSE (or a block whose handlers don't run) gets an
//! extra branch that isn't written in the code, the way past it. Forks and loops with no
//! calls or table use in them are left out.

use crate::plsql::{blank_comments, blank_comments_and_strings, is_ident, with_names, FROM_LIST_END};
use std::collections::HashMap;

/// One thing a method does, in run order.
#[derive(Debug, Clone, Default)]
pub struct TraceStep {
    /// `call`, `error` (`Error_SYS.X`, `RAISE`, `Raise_Application_Error`), `read`, `insert`,
    /// `update`, `delete`, `merge`, `cursor`, `fork` or `loop`.
    pub kind: &'static str,
    pub line: usize,
    /// Calls: the package, upper-cased (`INVENTORY_PART_API`). None for a call within the
    /// same package, and for `super`.
    pub package: Option<String>,
    /// The method, the table or view, the cursor, or a fork's kind (`if`, `case`, `exception`).
    pub name: String,
    /// The source line without comments; a fork's first condition; a loop's header.
    pub text: String,
    /// What a cursor's query or a loop runs.
    pub steps: Vec<TraceStep>,
    /// A fork's branches.
    pub arms: Vec<TraceArm>,
}

#[derive(Debug, Clone, Default)]
pub struct TraceArm {
    /// `IF x THEN`, `ELSIF y THEN`, `ELSE`, `WHEN NO_DATA_FOUND THEN`, …
    pub label: String,
    pub line: usize,
    /// Not written in the code: the way past a fork with no ELSE, or a block with no error.
    pub implied: bool,
    pub steps: Vec<TraceStep>,
}

#[derive(Debug, Clone, Default)]
pub struct MethodTrace {
    pub name: String,
    pub line: usize,
    pub end_line: usize,
    pub steps: Vec<TraceStep>,
}

struct Tok {
    /// Upper-cased; punctuation is the character itself.
    up: String,
    text: String,
    word: bool,
    line: usize,
    start: usize,
    end: usize,
    col0: bool,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    Root,
    Block,
    If,
    Case,
    Loop,
    Cursor,
}

struct Frame {
    kind: Kind,
    line: usize,
    /// The method's own BEGIN … END.
    main: bool,
    /// Cursor name, loop header or CASE selector.
    head: String,
    steps: Vec<TraceStep>,
    arms: Vec<TraceArm>,
    /// Reading a condition (IF / ELSIF / WHEN … THEN) that started here: (char, line).
    cond: Option<(usize, usize)>,
    /// What a later condition calls: it runs only when the branches before weren't taken.
    cond_steps: Vec<TraceStep>,
    /// A block past its EXCEPTION keyword.
    exception: bool,
    has_else: bool,
    /// Where a CASE starts, for its selector.
    head_start: usize,
}

impl Frame {
    fn new(kind: Kind, line: usize) -> Self {
        Frame {
            kind,
            line,
            main: false,
            head: String::new(),
            steps: Vec::new(),
            arms: Vec::new(),
            cond: None,
            cond_steps: Vec::new(),
            exception: false,
            has_else: false,
            head_start: 0,
        }
    }

    fn branches(&self) -> bool {
        matches!(self.kind, Kind::If | Kind::Case) || (self.kind == Kind::Block && self.exception)
    }

    fn current(&mut self) -> &mut Vec<TraceStep> {
        if self.branches() && !self.arms.is_empty() {
            &mut self.arms.last_mut().unwrap().steps
        } else {
            &mut self.steps
        }
    }
}

/// Where a step found now belongs. The first condition of an IF (and a CASE selector) runs
/// before the fork, so its calls go to the frame around it.
fn sink(fr: &mut [Frame]) -> &mut Vec<TraceStep> {
    let n = fr.len();
    let (head, last) = fr.split_at_mut(n - 1);
    let top = &mut last[0];
    let reading = top.cond.is_some() || (top.kind == Kind::Case && top.arms.is_empty());
    if reading {
        if top.arms.is_empty() && matches!(top.kind, Kind::If | Kind::Case) && !head.is_empty() {
            return sink(head);
        }
        return &mut top.cond_steps;
    }
    top.current()
}

/// The text between two character positions, on one line with single spaces.
fn snippet(src: &[char], a: usize, b: usize) -> String {
    let s: String = src[a.min(src.len())..b.min(src.len())].iter().collect();
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if s.chars().count() > 240 {
        s.chars().take(239).collect::<String>() + "…"
    } else {
        s
    }
}

fn tokenize(code: &[char], from: usize, first_line: usize) -> Vec<Tok> {
    let mut out = Vec::new();
    let (mut i, mut line, mut line_start) = (from, first_line, from);
    while i < code.len() {
        let c = code[i];
        if c == '\n' {
            line += 1;
            i += 1;
            line_start = i;
            continue;
        }
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        let start = i;
        if is_ident(c) {
            while i < code.len() && is_ident(code[i]) {
                i += 1;
            }
        } else {
            i += 1;
        }
        let text: String = code[start..i].iter().collect();
        out.push(Tok {
            up: text.to_ascii_uppercase(),
            word: is_ident(c),
            text,
            line,
            start,
            end: i,
            col0: start == line_start,
        });
    }
    out
}

/// `DB_COMPANY_OWNED`: a package constant, not a call.
fn is_constant(name: &str) -> bool {
    !name.chars().any(|c| c.is_ascii_lowercase())
}

/// `Public_Rec`, `Some_Arr`: record and collection types named through a package.
fn is_type(name: &str) -> bool {
    let up = name.to_ascii_uppercase();
    up.ends_with("_REC") || up.ends_with("_ARR") || up.ends_with("_TAB") || up.ends_with("_TYPE")
}

/// Traces the method declared on `line` (1-based) of a `.plsql` / `.plsvc` file.
pub fn trace_method(text: &str, line: usize) -> Option<MethodTrace> {
    let code: Vec<char> = blank_comments_and_strings(text).chars().collect();
    let src: Vec<char> = blank_comments(text).chars().collect();
    let mut from = 0;
    for _ in 1..line {
        from = code[from..].iter().position(|&c| c == '\n').map(|p| from + p + 1)?;
    }
    let toks = tokenize(&code, from, line);
    let first = toks.first()?;
    if !(first.up == "PROCEDURE" || first.up == "FUNCTION") {
        return None;
    }
    let name = toks.get(1).filter(|t| t.word)?.text.clone();

    // The method's own BEGIN: the first one in column 0 before the next declaration (IFS
    // writes it there), else the first one.
    let mut main_begin = None;
    for (i, t) in toks.iter().enumerate().skip(1) {
        if t.col0 && (t.up == "PROCEDURE" || t.up == "FUNCTION") {
            break;
        }
        if t.up == "BEGIN" && t.col0 {
            main_begin = Some(i);
            break;
        }
    }
    let main_begin = main_begin.or_else(|| toks.iter().position(|t| t.up == "BEGIN"))?;

    // Line text for steps, from the comment-free source (strings kept).
    let src_text: String = src.iter().collect();
    let lines: Vec<&str> = src_text.lines().collect();
    let line_text = |n: usize| -> String { lines.get(n - 1).map(|l| l.trim().to_string()).unwrap_or_default() };
    let method_code: String = code[from..].iter().collect();
    let local = with_names(&method_code);

    let mut fr: Vec<Frame> = vec![Frame::new(Kind::Root, line)];
    let mut cursors: HashMap<String, Vec<TraceStep>> = HashMap::new();
    // Calls waiting for their argument list to close: (package, method, line, depth).
    let mut pending: Vec<(Option<String>, String, usize, i32)> = Vec::new();
    let mut depth: i32 = 0;
    let mut stmt = 0usize;
    let mut expect: Option<&'static str> = None;
    let mut after_into = false;
    let mut from_lists: Vec<i32> = Vec::new();
    let mut loop_head: Option<usize> = None;
    let mut in_merge = false;
    let mut stmt_start = true;
    let mut skip_body = 0i32; // inside a local subprogram's body, before the method's BEGIN
    let mut skip_error = false; // inside `$ERROR … $END`
    let mut skip_to = 0usize;
    let mut end_line = line;

    let call_step = |package: Option<String>, method: String, line: usize| TraceStep {
        kind: "call",
        line,
        package,
        name: method,
        text: line_text(line),
        ..Default::default()
    };

    let mut i = 0;
    'scan: while i < toks.len() {
        let t = &toks[i];
        let prev = if i > 0 { toks[i - 1].up.as_str() } else { "" };
        let next = toks.get(i + 1);
        let next_up = next.map(|n| n.up.as_str()).unwrap_or("");
        let was_start = std::mem::replace(&mut stmt_start, false);
        if i < skip_to {
            i += 1;
            continue;
        }
        if skip_error {
            skip_error = t.up != "$END";
            i += 1;
            continue;
        }
        if i < main_begin && skip_body > 0 {
            if t.up == "BEGIN" || (t.up == "CASE" && prev != "END") {
                skip_body += 1;
            } else if t.up == "END" && !matches!(next_up, "IF" | "LOOP" | "CASE") {
                skip_body -= 1;
            }
            i += 1;
            continue;
        }

        if !t.word {
            match t.up.as_str() {
                "(" => {
                    expect = None;
                    depth += 1;
                }
                ")" => {
                    depth -= 1;
                    while from_lists.last().is_some_and(|&d| d > depth) {
                        from_lists.pop();
                    }
                    while pending.last().is_some_and(|p| p.3 >= depth) {
                        let (p, m, l, _) = pending.pop().unwrap();
                        sink(&mut fr).push(call_step(p, m, l));
                    }
                }
                "," if from_lists.last() == Some(&depth) => expect = Some("read"),
                ";" => {
                    while let Some((p, m, l, _)) = pending.pop() {
                        sink(&mut fr).push(call_step(p, m, l));
                    }
                    depth = 0;
                    from_lists.clear();
                    expect = None;
                    stmt += 1;
                    in_merge = false;
                    loop_head = None;
                    stmt_start = true;
                    if let Some(f) = fr.pop_if(|f| f.kind == Kind::Cursor) {
                        cursors.insert(f.head, f.steps);
                    }
                }
                ">" => stmt_start = prev == ">", // after a `<<label>>`
                _ => {}
            }
            i += 1;
            continue;
        }

        let up = t.up.as_str();

        // Tables and views, as in `plsql::relations`.
        let mut relation = false;
        if let Some(verb) = expect.take() {
            relation = true;
            const SKIP: [&str; 10] = ["SELECT", "NOWAIT", "OF", "SKIP", "WAIT", "DUAL", "TABLE", "LATERAL", "THE", "FROM"];
            if !up.ends_with('_')
                && !SKIP.contains(&up)
                && (after_into || next_up != "(")
                && next_up != "."
                && !local.contains(&(stmt, t.up.clone()))
            {
                sink(&mut fr).push(TraceStep {
                    kind: verb,
                    line: t.line,
                    name: t.up.clone(),
                    text: line_text(t.line),
                    ..Default::default()
                });
            }
        }
        after_into = false;
        if FROM_LIST_END.contains(&up) && from_lists.last() == Some(&depth) {
            from_lists.pop();
        }
        match up {
            "FROM" => {
                expect = Some(if prev == "DELETE" { "delete" } else { "read" });
                if from_lists.last() != Some(&depth) {
                    from_lists.push(depth);
                }
            }
            "JOIN" => {
                expect = Some("read");
                if from_lists.last() == Some(&depth) {
                    from_lists.pop();
                }
            }
            "UPDATE" if prev != "FOR" => expect = Some("update"),
            "DELETE" => expect = Some("delete"),
            "INTO" if prev == "INSERT" || prev == "MERGE" => {
                expect = Some(if prev == "INSERT" { "insert" } else { "merge" });
                after_into = true;
            }
            _ => {}
        }

        // Branches, blocks and loops.
        let top = fr.len() - 1;
        match up {
            "IF" if prev != "END" => {
                let mut f = Frame::new(Kind::If, t.line);
                f.cond = Some((t.start, t.line));
                fr.push(f);
            }
            "ELSIF" if fr[top].kind == Kind::If => fr[top].cond = Some((t.start, t.line)),
            "THEN" if fr[top].cond.is_some() => {
                let (a, l) = fr[top].cond.take().unwrap();
                let label = snippet(&src, a, t.end);
                let steps = std::mem::take(&mut fr[top].cond_steps);
                fr[top].arms.push(TraceArm { label, line: l, implied: false, steps });
                stmt_start = true;
            }
            "ELSE" if matches!(fr[top].kind, Kind::If | Kind::Case) => {
                fr[top].has_else = true;
                fr[top].arms.push(TraceArm { label: t.text.clone(), line: t.line, implied: false, steps: Vec::new() });
                stmt_start = true;
            }
            "WHEN" if !matches!(prev, "EXIT" | "CONTINUE") && !in_merge => {
                if fr[top].kind == Kind::Case || (fr[top].kind == Kind::Block && fr[top].exception) {
                    if fr[top].kind == Kind::Case && fr[top].arms.is_empty() && fr[top].cond.is_none() {
                        let a = fr[top].head_start;
                        fr[top].head = snippet(&src, a, t.start);
                    }
                    fr[top].cond = Some((t.start, t.line));
                }
            }
            "CASE" if prev != "END" => {
                let mut f = Frame::new(Kind::Case, t.line);
                f.head_start = t.start;
                fr.push(f);
            }
            "BEGIN" => {
                if i < main_begin {
                    skip_body = 1;
                } else {
                    let mut f = Frame::new(Kind::Block, t.line);
                    f.main = i == main_begin;
                    fr.push(f);
                    stmt_start = true;
                }
            }
            "EXCEPTION" if fr[top].kind == Kind::Block && next_up != ";" => {
                fr[top].exception = true;
                stmt_start = true;
            }
            "LOOP" if prev != "END" => {
                let mut f = Frame::new(Kind::Loop, t.line);
                f.head = snippet(&src, loop_head.unwrap_or(t.start), t.end);
                fr.push(f);
                stmt_start = true;
            }
            "FOR" | "WHILE" if was_start => loop_head = Some(t.start),
            "MERGE" => in_merge = true,
            "$ERROR" => skip_error = true,
            "$THEN" | "$ELSE" | "$END" => stmt_start = true,
            "CURSOR" if i < main_begin => {
                if let Some(n) = next.filter(|n| n.word) {
                    let mut f = Frame::new(Kind::Cursor, t.line);
                    f.head = n.up.clone();
                    fr.push(f);
                    skip_to = i + 2;
                }
            }
            "END" => {
                let until = match next_up {
                    "IF" => Some(Kind::If),
                    "LOOP" => Some(Kind::Loop),
                    "CASE" => Some(Kind::Case),
                    _ => None,
                };
                let at = match until {
                    Some(k) => fr.iter().rposition(|f| f.kind == k),
                    None => fr.iter().rposition(|f| matches!(f.kind, Kind::Block | Kind::Case)),
                };
                if let Some(at) = at.filter(|&a| a > 0) {
                    let main = fr[at].main;
                    while fr.len() > at {
                        close(&mut fr, &mut cursors);
                    }
                    if main {
                        end_line = t.line;
                        break 'scan;
                    }
                }
                if until.is_some() {
                    skip_to = i + 2; // the IF / LOOP / CASE after END
                }
            }
            _ => {}
        }

        // Cursors opened by name.
        let opens = (up == "OPEN" || (up == "IN" && i >= 2 && toks[i - 2].up == "FOR")) && next.is_some_and(|n| cursors.contains_key(&n.up));
        if opens {
            let n = next.unwrap();
            let steps = cursors[&n.up].clone();
            sink(&mut fr).push(TraceStep {
                kind: "cursor",
                line: n.line,
                name: n.text.clone(),
                text: line_text(n.line),
                steps,
                ..Default::default()
            });
            skip_to = i + 2;
        }

        // Errors raised. `Error_SYS.Record_General(...)` and the rest stop the method.
        if up == "RAISE" && was_start {
            let what = next.filter(|n| n.word).map(|n| n.text.clone()).unwrap_or_else(|| "RAISE".into());
            sink(&mut fr).push(TraceStep { kind: "error", line: t.line, name: what, text: line_text(t.line), ..Default::default() });
        } else if up == "ERROR_SYS" && next_up == "." && toks.get(i + 2).is_some_and(|m| m.word) && prev != "." {
            let m = &toks[i + 2];
            sink(&mut fr).push(TraceStep {
                kind: "error",
                line: m.line,
                package: Some("ERROR_SYS".into()),
                name: m.text.clone(),
                text: line_text(m.line),
                ..Default::default()
            });
            skip_to = i + 3;
        } else if up == "RAISE_APPLICATION_ERROR" && next_up == "(" {
            sink(&mut fr).push(TraceStep { kind: "error", line: t.line, name: t.text.clone(), text: line_text(t.line), ..Default::default() });
        }
        // Calls.
        else if prev != "." && !opens {
            let qualified = up.len() > 4
                && up.ends_with("_API")
                && next_up == "."
                && toks.get(i + 2).is_some_and(|m| m.word && m.text.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_'));
            if qualified {
                let m = &toks[i + 2];
                let after = toks.get(i + 3).map(|a| a.up.as_str()).unwrap_or("");
                if after == "(" {
                    pending.push((Some(t.up.clone()), m.text.clone(), m.line, depth));
                } else if !is_constant(&m.text) && !is_type(&m.text) && after != "%" {
                    sink(&mut fr).push(call_step(Some(t.up.clone()), m.text.clone(), m.line));
                }
                skip_to = i + 3;
            } else {
                // Same-package calls: `Check_Insert___(rec_);`, `Do_It;`, `super(...)`. Variables end
                // in a single `_`; the UI keeps only names that are methods of the package.
                let variable = up.ends_with('_') && !up.ends_with("__");
                let callable = next_up == "(" || next_up == ";";
                let local_name = (up.contains('_') && !variable) || up == "SUPER";
                let named_otherwise = relation || matches!(prev, "END" | "PROCEDURE" | "FUNCTION" | "CURSOR" | "RAISE");
                if callable && local_name && !named_otherwise && !cursors.contains_key(up) {
                    if next_up == "(" {
                        pending.push((None, t.text.clone(), t.line, depth));
                    } else {
                        sink(&mut fr).push(call_step(None, t.text.clone(), t.line));
                    }
                }
            }
        }
        i += 1;
    }
    while fr.len() > 1 {
        close(&mut fr, &mut cursors);
    }
    let root = fr.pop().unwrap();
    Some(MethodTrace { name, line, end_line, steps: root.steps })
}

/// Pops the top frame and adds what it found to the frame around it.
fn close(fr: &mut Vec<Frame>, cursors: &mut HashMap<String, Vec<TraceStep>>) {
    let mut f = fr.pop().unwrap();
    // A condition still being read (unbalanced code): keep its calls.
    let leftover = std::mem::take(&mut f.cond_steps);
    match f.kind {
        Kind::Root => unreachable!(),
        Kind::Cursor => {
            cursors.insert(f.head, f.steps);
        }
        Kind::Block => {
            let out = sink(fr);
            out.extend(f.steps);
            out.extend(leftover);
            if f.exception && f.arms.iter().any(|a| !a.steps.is_empty()) {
                let text = "EXCEPTION".to_string();
                f.arms.insert(0, TraceArm { label: "No error".into(), line: f.line, implied: true, steps: Vec::new() });
                out.push(TraceStep { kind: "fork", line: f.arms[1].line, name: "exception".into(), text, arms: f.arms, ..Default::default() });
            }
        }
        Kind::Loop => {
            if !f.steps.is_empty() {
                sink(fr).push(TraceStep { kind: "loop", line: f.line, name: "loop".into(), text: f.head, steps: f.steps, ..Default::default() });
            }
        }
        Kind::If | Kind::Case => {
            let out = sink(fr);
            out.extend(leftover);
            if f.arms.iter().any(|a| !a.steps.is_empty()) {
                let (name, past) = if f.kind == Kind::If { ("if", "No ELSE: nothing runs") } else { ("case", "No WHEN matches") };
                if !f.has_else {
                    f.arms.push(TraceArm { label: past.into(), line: f.line, implied: true, steps: Vec::new() });
                }
                let text = if f.kind == Kind::Case && !f.head.is_empty() { f.head.clone() } else { f.arms[0].label.clone() };
                out.push(TraceStep { kind: "fork", line: f.line, name: name.into(), text, arms: f.arms, ..Default::default() });
            }
        }
    }
}

/// Names of every call, read and write in a trace, depth first (for tests).
pub fn flat(steps: &[TraceStep]) -> Vec<String> {
    let mut out = Vec::new();
    for s in steps {
        match s.kind {
            "fork" => {
                for a in &s.arms {
                    out.push(format!("[{}]", a.label));
                    out.extend(flat(&a.steps));
                }
                out.push("[end]".into());
            }
            "loop" | "cursor" => {
                out.push(format!("{} {} {{", s.kind, s.name));
                out.extend(flat(&s.steps));
                out.push("}".into());
            }
            "call" => out.push(match &s.package {
                Some(p) => format!("{p}.{}", s.name),
                None => s.name.clone(),
            }),
            k => out.push(format!("{k} {}", s.name)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "\
PROCEDURE Other IS
BEGIN
   NULL;
END Other;

@Override
PROCEDURE Do_It___ (
   rec_ IN OUT Shop_Order_API.Public_Rec )
IS
   kind_ VARCHAR2(20) := Shop_Order_Type_API.DB_NORMAL;
   rec2_ Shop_Order_API.Public_Rec;
   CURSOR get_ops IS
      SELECT * FROM shop_order_operation_tab WHERE order_no = rec_.order_no;
   PROCEDURE Local_Helper IS
   BEGIN
      Helper_API.Never_Traced(1);
   END Local_Helper;
BEGIN
   super(rec_); -- the base method
   rec_.cost := Cost_API.Total(Cost_API.Default_Details(rec_.part_no), 'A');
   IF (Part_API.Is_Active(rec_.part_no) = 'TRUE') THEN
      Check_Part___(rec_);
   ELSIF (Part_API.Is_Phased_Out(rec_.part_no) = 'TRUE') THEN
      Error_SYS.Record_General(lu_name_, 'PHASED: Phased out.');
   ELSE
      UPDATE shop_order_tab SET cost = 0 WHERE order_no = rec_.order_no;
   END IF;
   IF (kind_ = 'X') THEN
      kind_ := 'Y';
   END IF;
   $IF Component_Proj_SYS.INSTALLED $THEN
      Project_API.Touch(rec_.project_id);
   $ELSE
      NULL;
   $END
   CASE rec_.state
      WHEN 'Planned' THEN
         Plan_API.Release(rec_.order_no);
      WHEN 'Closed' THEN
         RAISE closed_error;
   END CASE;
   FOR op_ IN get_ops LOOP
      INSERT INTO shop_order_log_tab (order_no) VALUES (op_.order_no);
   END LOOP;
EXCEPTION
   WHEN no_data_found THEN
      Log_API.Write('none');
END Do_It___;
";

    fn trace_lines() -> Vec<String> {
        let line = SRC.lines().position(|l| l.starts_with("PROCEDURE Do_It___")).unwrap() + 1;
        let t = trace_method(SRC, line).unwrap();
        assert_eq!(t.name, "Do_It___");
        assert_eq!(t.end_line, SRC.lines().count());
        flat(&t.steps)
    }

    #[test]
    fn steps_in_run_order_by_branch() {
        let expected = [
            // Declarations: the constant, the record type and the local procedure's body are left out.
            "super",
            // Arguments first.
            "COST_API.Default_Details",
            "COST_API.Total",
            // The first condition runs before the fork; a later one only inside its branch.
            "PART_API.Is_Active",
            "[IF (Part_API.Is_Active(rec_.part_no) = 'TRUE') THEN]",
            "Check_Part___",
            "[ELSIF (Part_API.Is_Phased_Out(rec_.part_no) = 'TRUE') THEN]",
            "PART_API.Is_Phased_Out",
            "error Record_General",
            "[ELSE]",
            "update SHOP_ORDER_TAB",
            "[end]",
            // The IF with nothing in it is left out; $IF lines are skipped, the code inside stays.
            "PROJECT_API.Touch",
            "[WHEN 'Planned' THEN]",
            "PLAN_API.Release",
            "[WHEN 'Closed' THEN]",
            "error closed_error",
            "[No WHEN matches]",
            "[end]",
            // The cursor's query runs as the loop starts.
            "cursor get_ops {",
            "read SHOP_ORDER_OPERATION_TAB",
            "}",
            "loop loop {",
            "insert SHOP_ORDER_LOG_TAB",
            "}",
            "[No error]",
            "[WHEN no_data_found THEN]",
            "LOG_API.Write",
            "[end]",
        ];
        assert_eq!(trace_lines(), expected);
    }

    #[test]
    fn a_fork_without_else_gets_the_way_past_it() {
        let src = "FUNCTION F RETURN NUMBER IS\nBEGIN\n   IF a_ THEN\n      A_API.Go;\n   END IF;\n   RETURN 1;\nEND F;\n";
        let t = trace_method(src, 1).unwrap();
        let fork = &t.steps[0];
        assert_eq!((fork.kind, fork.name.as_str(), fork.text.as_str()), ("fork", "if", "IF a_ THEN"));
        let arms: Vec<(&str, bool)> = fork.arms.iter().map(|a| (a.label.as_str(), a.implied)).collect();
        assert_eq!(arms, [("IF a_ THEN", false), ("No ELSE: nothing runs", true)]);
        assert_eq!(t.end_line, 7);
    }

    #[test]
    fn not_a_declaration() {
        assert!(trace_method("PROCEDURE P IS\nBEGIN\n   NULL;\nEND P;\n", 2).is_none());
    }
}
