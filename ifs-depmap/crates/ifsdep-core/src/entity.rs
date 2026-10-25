//! `.entity` (LU model, XML). What decides the generated view and table is read: the LU
//! name, component, layer, the code generation properties that rename them, and the
//! attributes (their columns). The XML is element-only, so a tag reader that tracks the
//! element path is enough.

/// An attribute: a column of the generated view, and of the table unless it is derived.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Attribute {
    pub name: String,
    pub line: usize,
    /// `IS_DERIVED` 1: computed in the view (`DB_SQL_IMPLEMENTATION`), not stored in the table.
    pub derived: bool,
    /// `DATATYPE` ENUMERATION: the view also has `<COLUMN>_DB`.
    pub enumeration: bool,
}

#[derive(Debug, Default)]
pub struct ParsedEntity {
    pub attributes: Vec<Attribute>,
    /// LU name and its line.
    pub name: Option<(String, usize)>,
    pub component: Option<String>,
    pub layer: Option<String>,
    /// `DB_TABLE_NAME`: replaces the generated `<VIEW>_TAB`.
    pub table: Option<(String, usize)>,
    /// `DB_VIEW_NAME`: replaces the generated view name (the LU name in snake case).
    pub view: Option<(String, usize)>,
    /// `DB_IMPLEMENTATION`, e.g. `abstract` (no generated table) or `micro-cache-array`.
    pub implementation: Option<String>,
}

const CODEGEN: &str = "ENTITY/CODE_GENERATION_PROPERTIES/CODE_GENERATION_PROPERTIES/";

pub fn parse(text: &str) -> ParsedEntity {
    let mut out = ParsedEntity::default();
    let mut path: Vec<&str> = Vec::new();
    let mut line = 1;
    let mut i = 0;
    let b = text.as_bytes();
    while let Some(off) = text[i..].find('<') {
        line += text[i..i + off].matches('\n').count();
        let start = i + off;
        let Some(len) = text[start..].find('>') else { break };
        let tag = &text[start + 1..start + len];
        i = start + len + 1;
        line += tag.matches('\n').count();
        if tag.starts_with('?') || tag.starts_with('!') {
            continue; // <?xml ...?> and comments
        }
        if let Some(name) = tag.strip_prefix('/') {
            if path.last() == Some(&name.trim()) {
                path.pop();
            }
            continue;
        }
        let self_closing = tag.ends_with('/');
        let name = tag.split(|c: char| c.is_whitespace() || c == '/').next().unwrap_or("");
        if self_closing || name.is_empty() {
            continue;
        }
        path.push(name);
        if path == ["ENTITY", "ATTRIBUTES", "ATTRIBUTE"] {
            out.attributes.push(Attribute::default());
        }
        // Leaf text runs to the next tag.
        let end = text[i..].find('<').map_or(text.len(), |n| i + n);
        let value = text[i..end].trim();
        if value.is_empty() || b.get(end + 1) != Some(&b'/') {
            continue;
        }
        let value = value.to_string();
        let joined = path.join("/");
        match joined.as_str() {
            "ENTITY/NAME" => out.name = Some((value, line)),
            "ENTITY/COMPONENT" => out.component = Some(value.to_ascii_uppercase()),
            "ENTITY/LAYER" => out.layer = Some(value),
            "ENTITY/ATTRIBUTES/ATTRIBUTE/NAME" => {
                if let Some(a) = out.attributes.last_mut() {
                    a.name = value;
                    a.line = line;
                }
            }
            "ENTITY/ATTRIBUTES/ATTRIBUTE/IS_DERIVED" => {
                if let Some(a) = out.attributes.last_mut() {
                    a.derived = value == "1";
                }
            }
            "ENTITY/ATTRIBUTES/ATTRIBUTE/DATATYPE" => {
                if let Some(a) = out.attributes.last_mut() {
                    a.enumeration = value.eq_ignore_ascii_case("ENUMERATION");
                }
            }
            p => match p.strip_prefix(CODEGEN) {
                Some("DB_TABLE_NAME") => out.table = Some((value.to_ascii_uppercase(), line)),
                Some("DB_VIEW_NAME") => out.view = Some((value.to_ascii_uppercase(), line)),
                Some("DB_IMPLEMENTATION") => out.implementation = Some(value),
                _ => {}
            },
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_names_from_the_right_elements() {
        let src = r#"<?xml version="1.0" encoding="UTF-8"?>
<ENTITY xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" xmlns="urn:ifsworld-com:schemas:entity_entity">
   <CODE_GENERATION_PROPERTIES>
      <CODE_GENERATION_PROPERTIES>
         <DB_VIEW_NAME>ACC_YEAR</DB_VIEW_NAME>
         <DB_TABLE_NAME>accounting_year_tab</DB_TABLE_NAME>
      </CODE_GENERATION_PROPERTIES>
   </CODE_GENERATION_PROPERTIES>
   <DIAGRAMS>
      <DIAGRAM>
         <NAME>Main</NAME>
      </DIAGRAM>
   </DIAGRAMS>
   <NAME>AccountingYear</NAME>
   <COMPONENT>ACCRUL</COMPONENT>
   <ATTRIBUTES>
      <ATTRIBUTE>
         <CODE_GENERATION_PROPERTIES>
            <CODE_GENERATION_PROPERTIES>
               <DB_TABLE_NAME>NOT_THIS_TAB</DB_TABLE_NAME>
            </CODE_GENERATION_PROPERTIES>
         </CODE_GENERATION_PROPERTIES>
         <NAME>Company</NAME>
         <IS_MANDATORY/>
         <DATATYPE>TEXT</DATATYPE>
      </ATTRIBUTE>
      <ATTRIBUTE>
         <NAME>YearStatus</NAME>
         <IS_DERIVED>1</IS_DERIVED>
         <DATATYPE>ENUMERATION</DATATYPE>
      </ATTRIBUTE>
   </ATTRIBUTES>
</ENTITY>
"#;
        let e = parse(src);
        assert_eq!(e.name, Some(("AccountingYear".into(), 14)), "the LU name, not the diagram's");
        assert_eq!(e.component.as_deref(), Some("ACCRUL"));
        assert_eq!(e.view, Some(("ACC_YEAR".into(), 5)));
        assert_eq!(e.table, Some(("ACCOUNTING_YEAR_TAB".into(), 6)), "the entity's, not an attribute's");
        assert!(e.layer.is_none());
        let attr = |name: &str, line, derived, enumeration| Attribute { name: name.into(), line, derived, enumeration };
        assert_eq!(e.attributes, [attr("Company", 23, false, false), attr("YearStatus", 28, true, true)]);
    }
}
