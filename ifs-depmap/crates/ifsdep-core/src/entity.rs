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
    /// `DATATYPE` as written: TEXT, NUMBER, DATE, TIMESTAMP, ENUMERATION, LOOKUP, BOOLEAN...
    pub datatype: Option<String>,
    /// `ENUMERATION_NAME`: the enumeration an ENUMERATION or LOOKUP attribute takes its values from.
    pub enumeration_name: Option<String>,
    /// `LENGTH` (text attributes).
    pub length: Option<u32>,
    /// `IS_PUBLIC` 1: a field of the package's generated `Public_Rec`, filled by `Get()`,
    /// with its own generated `Get_<Attr>()`, unless `exclude_get`.
    pub public: bool,
    /// `EXCLUDE_GET_METHOD` true: no `Public_Rec` field and no `Get_<Attr>()`.
    pub exclude_get: bool,
    /// `IS_PRIMARY_KEY` 1: a key, and so a parameter of the generated getters.
    pub key: bool,
    /// `DB_SQL_IMPLEMENTATION`: the expression a derived attribute is computed with.
    pub implementation: Option<String>,
}

const ATTRIBUTE_CODEGEN: &str = "ENTITY/ATTRIBUTES/ATTRIBUTE/CODE_GENERATION_PROPERTIES/CODE_GENERATION_PROPERTIES/";

/// `&apos;` and the other XML entities, in element text.
fn unescape(s: &str) -> String {
    s.replace("&apos;", "'").replace("&quot;", "\"").replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&")
}

/// An association (reference) to another entity. Its USE_DEFINITIONs bring columns of that
/// entity in as derived attributes of this one.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Association {
    pub name: String,
    pub line: usize,
    pub to_entity: String,
    /// The attributes it joins on.
    pub on: Vec<String>,
    pub uses: Vec<UseDefinition>,
}

/// `<USE_DEFINITION>`: the referenced entity's `COLUMN_NAME`, exposed here as `COLUMN_ALIAS`
/// when set (otherwise under its own name).
#[derive(Debug, Default, Clone, PartialEq)]
pub struct UseDefinition {
    pub column: String,
    pub alias: Option<String>,
    pub line: usize,
}

impl UseDefinition {
    /// The derived attribute this defines.
    pub fn attribute(&self) -> &str {
        self.alias.as_deref().unwrap_or(&self.column)
    }
}

#[derive(Debug, Default, Clone)]
pub struct ParsedEntity {
    pub attributes: Vec<Attribute>,
    pub associations: Vec<Association>,
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
        if path == ["ENTITY", "ASSOCIATIONS", "ASSOCIATION"] {
            out.associations.push(Association { line, ..Default::default() });
        }
        if path == ["ENTITY", "ASSOCIATIONS", "ASSOCIATION", "USE_DEFINITIONS", "USE_DEFINITION"] {
            if let Some(a) = out.associations.last_mut() {
                a.uses.push(UseDefinition { line, ..Default::default() });
            }
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
                    a.datatype = Some(value.to_ascii_uppercase());
                }
            }
            "ENTITY/ATTRIBUTES/ATTRIBUTE/ENUMERATION_NAME" => {
                if let Some(a) = out.attributes.last_mut() {
                    a.enumeration_name = Some(value);
                }
            }
            "ENTITY/ASSOCIATIONS/ASSOCIATION/NAME" => {
                if let Some(a) = out.associations.last_mut() {
                    a.name = value;
                }
            }
            "ENTITY/ASSOCIATIONS/ASSOCIATION/TO_ENTITY" => {
                if let Some(a) = out.associations.last_mut() {
                    a.to_entity = value;
                }
            }
            "ENTITY/ASSOCIATIONS/ASSOCIATION/ATTRIBUTES/ASSOCIATION_ATTRIBUTE/NAME" => {
                if let Some(a) = out.associations.last_mut() {
                    a.on.push(value);
                }
            }
            "ENTITY/ASSOCIATIONS/ASSOCIATION/USE_DEFINITIONS/USE_DEFINITION/COLUMN_NAME" => {
                if let Some(u) = out.associations.last_mut().and_then(|a| a.uses.last_mut()) {
                    u.column = value;
                    u.line = line;
                }
            }
            "ENTITY/ASSOCIATIONS/ASSOCIATION/USE_DEFINITIONS/USE_DEFINITION/COLUMN_ALIAS" => {
                if let Some(u) = out.associations.last_mut().and_then(|a| a.uses.last_mut()) {
                    u.alias = Some(value);
                }
            }
            "ENTITY/ATTRIBUTES/ATTRIBUTE/LENGTH" => {
                if let Some(a) = out.attributes.last_mut() {
                    a.length = value.parse().ok();
                }
            }
            "ENTITY/ATTRIBUTES/ATTRIBUTE/IS_PRIMARY_KEY" => {
                if let Some(a) = out.attributes.last_mut() {
                    a.key = value == "1";
                }
            }
            "ENTITY/ATTRIBUTES/ATTRIBUTE/IS_PUBLIC" => {
                if let Some(a) = out.attributes.last_mut() {
                    a.public = value == "1";
                }
            }
            p if p.starts_with(ATTRIBUTE_CODEGEN) => {
                if let Some(a) = out.attributes.last_mut() {
                    match &p[ATTRIBUTE_CODEGEN.len()..] {
                        "DB_SQL_IMPLEMENTATION" => a.implementation = Some(unescape(&value)),
                        "EXCLUDE_GET_METHOD" => a.exclude_get = value.eq_ignore_ascii_case("true"),
                        _ => {}
                    }
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
      <ATTRIBUTE>
         <CODE_GENERATION_PROPERTIES>
            <CODE_GENERATION_PROPERTIES>
               <DB_SQL_IMPLEMENTATION>Supplier_API.Get_Primary_No(company, &apos;*&apos;)</DB_SQL_IMPLEMENTATION>
               <EXCLUDE_GET_METHOD>true</EXCLUDE_GET_METHOD>
            </CODE_GENERATION_PROPERTIES>
         </CODE_GENERATION_PROPERTIES>
         <NAME>PrimarySupplier</NAME>
         <IS_PUBLIC>1</IS_PUBLIC>
         <IS_DERIVED>1</IS_DERIVED>
         <DATATYPE>TEXT</DATATYPE>
         <LENGTH>20</LENGTH>
      </ATTRIBUTE>
   </ATTRIBUTES>
   <ASSOCIATIONS>
      <ASSOCIATION>
         <NAME>ConsCompanyRef</NAME>
         <TO_ENTITY>CompanyFinance</TO_ENTITY>
         <ATTRIBUTES>
            <ASSOCIATION_ATTRIBUTE>
               <NAME>Company</NAME>
            </ASSOCIATION_ATTRIBUTE>
         </ATTRIBUTES>
         <USE_DEFINITIONS>
            <USE_DEFINITION>
               <COLUMN_NAME>MasterCompany</COLUMN_NAME>
            </USE_DEFINITION>
            <USE_DEFINITION>
               <COLUMN_NAME>Name</COLUMN_NAME>
               <COLUMN_ALIAS>MasterCompanyName</COLUMN_ALIAS>
            </USE_DEFINITION>
         </USE_DEFINITIONS>
      </ASSOCIATION>
   </ASSOCIATIONS>
</ENTITY>
"#;
        let e = parse(src);
        assert_eq!(e.name, Some(("AccountingYear".into(), 14)), "the LU name, not the diagram's");
        assert_eq!(e.component.as_deref(), Some("ACCRUL"));
        assert_eq!(e.view, Some(("ACC_YEAR".into(), 5)));
        assert_eq!(e.table, Some(("ACCOUNTING_YEAR_TAB".into(), 6)), "the entity's, not an attribute's");
        assert!(e.layer.is_none());
        let summary: Vec<(&str, usize, bool, bool)> = e.attributes.iter().map(|a| (a.name.as_str(), a.line, a.derived, a.enumeration)).collect();
        assert_eq!(summary, [("Company", 23, false, false), ("YearStatus", 28, true, true), ("PrimarySupplier", 39, true, false)]);
        let p = &e.attributes[2];
        assert_eq!((p.datatype.as_deref(), p.length, p.public, p.exclude_get), (Some("TEXT"), Some(20), true, true));
        assert_eq!(p.implementation.as_deref(), Some("Supplier_API.Get_Primary_No(company, '*')"), "XML entities decoded");
        assert!(!e.attributes[0].public && e.attributes[0].implementation.is_none() && !e.attributes[0].exclude_get);
        let a = &e.associations[0];
        assert_eq!((a.name.as_str(), a.to_entity.as_str(), a.on.clone()), ("ConsCompanyRef", "CompanyFinance", vec!["Company".to_string()]));
        let uses: Vec<(&str, &str)> = a.uses.iter().map(|u| (u.column.as_str(), u.attribute())).collect();
        assert_eq!(uses, [("MasterCompany", "MasterCompany"), ("Name", "MasterCompanyName")]);
        assert!(a.uses[1].line > a.uses[0].line);
    }
}
