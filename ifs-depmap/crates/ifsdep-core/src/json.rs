//! Minimal JSON writer for the index. Field names and enum spellings match
//! what the UI reads (`ui/src/types.ts`).

use crate::model::*;
use std::collections::BTreeMap;

pub trait ToJson {
    fn write_json(&self, out: &mut String);
}

pub fn to_string<T: ToJson + ?Sized>(v: &T) -> String {
    let mut s = String::with_capacity(4096);
    v.write_json(&mut s);
    s
}

pub fn write_str(s: &str, out: &mut String) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            // Keep `</script>` from ending an inline <script> block.
            '<' => out.push_str("\\u003c"),
            c => out.push(c),
        }
    }
    out.push('"');
}

impl ToJson for str {
    fn write_json(&self, out: &mut String) {
        write_str(self, out)
    }
}
impl ToJson for String {
    fn write_json(&self, out: &mut String) {
        write_str(self, out)
    }
}
macro_rules! json_num {
    ($($t:ty),*) => {$(
        impl ToJson for $t {
            fn write_json(&self, out: &mut String) { out.push_str(&self.to_string()) }
        }
    )*};
}
json_num!(usize, u32, u128, i64);
impl ToJson for bool {
    fn write_json(&self, out: &mut String) {
        out.push_str(if *self { "true" } else { "false" })
    }
}
impl<T: ToJson> ToJson for Option<T> {
    fn write_json(&self, out: &mut String) {
        match self {
            Some(v) => v.write_json(out),
            None => out.push_str("null"),
        }
    }
}
impl<T: ToJson> ToJson for Vec<T> {
    fn write_json(&self, out: &mut String) {
        out.push('[');
        for (i, v) in self.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            v.write_json(out);
        }
        out.push(']');
    }
}
impl<T: ToJson> ToJson for BTreeMap<String, T> {
    fn write_json(&self, out: &mut String) {
        out.push('{');
        for (i, (k, v)) in self.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            write_str(k, out);
            out.push(':');
            v.write_json(out);
        }
        out.push('}');
    }
}

macro_rules! json_struct {
    ($t:ty { $($field:ident),* $(,)? }) => {
        impl ToJson for $t {
            #[allow(unused_assignments)]
            fn write_json(&self, out: &mut String) {
                out.push('{');
                let mut first = true;
                $(
                    if !first { out.push(','); }
                    first = false;
                    write_str(stringify!($field), out);
                    out.push(':');
                    self.$field.write_json(out);
                )*
                out.push('}');
            }
        }
    };
}

macro_rules! json_enum {
    ($t:ty { $($variant:ident => $name:literal),* $(,)? }) => {
        impl $t {
            pub fn as_str(&self) -> &'static str {
                match self { $(Self::$variant => $name),* }
            }
        }
        impl ToJson for $t {
            fn write_json(&self, out: &mut String) { write_str(self.as_str(), out) }
        }
    };
}

json_enum!(Layer { Core => "Core", Cust => "Cust" });
json_enum!(FileKind {
    Client => "client", Projection => "projection", Fragment => "fragment",
    Plsvc => "plsvc", Plsql => "plsql", DeployIni => "deployini",
});
json_enum!(UnitKind {
    Client => "client", Projection => "projection", Fragment => "fragment",
    Plsvc => "plsvc", Package => "package",
});
json_enum!(FileRole { Base => "base", Overlay => "overlay", Config => "config" });
json_enum!(Section { Client => "client", Projection => "projection" });
json_enum!(EdgeKind {
    Include => "include", UsesProjection => "uses_projection", ImplicitInclude => "implicit_include",
    Implements => "implements", Calls => "calls", Reference => "reference", Array => "array",
    Enumeration => "enumeration", EntitysetFor => "entityset_for", Binds => "binds",
    Overrides => "overrides", CallsOperation => "calls_operation", ImplementedBy => "implemented_by",
    Invokes => "invokes", Reads => "reads",
});
json_enum!(Severity { Error => "error", Warning => "warning", Info => "info" });

json_struct!(SourceFile { id, rel_path, abs_path, root, kind, component, declared_layer, unit, role, lines });
json_struct!(Unit { id, kind, name, component, origin, files, has_overlay });
json_struct!(Object { id, unit, file, kind, name, line, modifier, section, target, dynamic, base, base_origin, base_generated });
json_struct!(Edge { id, from, to, kind, file, line, layer, dynamic, count, detail });
json_struct!(Relation { id, name, kind });
json_struct!(Connection { target, kind, layer, file, line });
json_struct!(Component { name, has_core, has_cust, deploy_files, connections });
json_struct!(Location { file, line });
json_struct!(Suggestion { summary, snippet, target });
json_struct!(Step { file, line, text, note });
json_struct!(Trail { title, steps });
json_struct!(Finding { id, severity, rule, title, message, nodes, edges, location, suggestion, evidence, involves_cust });
json_struct!(Stats { files, by_kind, units, objects, edges, millis });
json_struct!(Index { version, core_root, cust_root, files, units, objects, relations, edges, components, findings, stats, notes });

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes() {
        let lt = format!("{}u003c", '\\');
        assert_eq!(to_string("a\"b\\c\n</script>"), format!(r#""a\"b\\c\n{lt}/script>""#));
        assert_eq!(to_string(&Some(vec![1usize, 2])), "[1,2]");
        assert_eq!(to_string(&None::<String>), "null");
        assert_eq!(to_string(&Location { file: 3, line: 7 }), r#"{"file":3,"line":7}"#);
        assert_eq!(to_string(&EdgeKind::UsesProjection), r#""uses_projection""#);
    }
}
