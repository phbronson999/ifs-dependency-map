//! End-to-end checks against the synthetic Core + Cust trees in /fixtures.
//! Each problem is planted on purpose; the assertions say what the tool must
//! find and, as importantly, what it must not flag.

use ifsdep_core::*;
use std::path::PathBuf;

fn fixtures() -> Index {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
    let core = root.join("core");
    let cust = root.join("cust");
    index(&IndexOptions { core: Some(&core), cust: Some(&cust), ..Default::default() })
}

fn findings<'a>(idx: &'a Index, rule: &str) -> Vec<&'a Finding> {
    idx.findings.iter().filter(|f| f.rule == rule).collect()
}

fn unit_names(idx: &Index, f: &Finding) -> Vec<String> {
    let mut v: Vec<String> = f
        .nodes
        .iter()
        .filter(|n| n.starts_with('u'))
        .map(|n| idx.units[id_index(n)].name.clone())
        .collect();
    v.sort();
    v
}

fn object<'a>(idx: &'a Index, unit: &str, name: &str, modifier: bool) -> &'a Object {
    idx.objects
        .iter()
        .find(|o| idx.units[id_index(&o.unit)].name == unit && o.name == name && o.modifier.is_some() == modifier)
        .unwrap_or_else(|| panic!("object {unit}.{name} not found"))
}

fn object_in<'a>(idx: &'a Index, layer: Layer, unit: &str, name: &str) -> &'a Object {
    idx.objects
        .iter()
        .find(|o| idx.files[o.file].root == layer && idx.units[id_index(&o.unit)].name == unit && o.name == name)
        .unwrap_or_else(|| panic!("{layer:?} object {unit}.{name} not found"))
}

#[test]
fn indexes_everything() {
    let idx = fixtures();
    assert_eq!(idx.stats.files, 44);
    let shpord = idx.components.iter().find(|c| c.name == "SHPORD").unwrap();
    assert_eq!(shpord.description.as_deref(), Some("Shop Order"));
    assert!(idx.notes.is_empty(), "{:?}", idx.notes);
    // `//` and `/* */` comments with braces in them don't swallow the command after them.
    let reprint = object(&idx, "ShopOrder", "ReprintCommand", false);
    assert_eq!((reprint.kind.as_str(), reprint.section), ("command", Some(Section::Client)));
    assert!(idx.objects.iter().all(|o| o.name != "OldPrintCommand" && o.name != "OldGroup"));
}

#[test]
fn units_merge_core_and_cust_files() {
    let idx = fixtures();
    let u = idx.units.iter().find(|u| u.name == "ShopOrderCommon").unwrap();
    assert_eq!(u.origin, Layer::Core);
    assert!(u.has_overlay);
    assert_eq!(u.files.len(), 2);

    let pure = idx.units.iter().find(|u| u.name == "TiShopOrderExtras").unwrap();
    assert_eq!(pure.origin, Layer::Cust);
    assert!(!pure.has_overlay);
    assert_eq!(idx.files[pure.files[0]].role, FileRole::Base);
}

#[test]
fn finds_all_three_cycles_and_classifies_them() {
    let idx = fixtures();
    let cycles = findings(&idx, "CYCLE");
    assert_eq!(cycles.len(), 3);

    let core = cycles.iter().find(|f| f.title.contains("in Core")).unwrap();
    assert_eq!(unit_names(&idx, core), ["PartAvailabilityPanel", "PartPlanningPanel", "PartSupplyDemandPanel"]);
    assert!(core.suggestion.as_ref().unwrap().snippet.is_none(), "no fix offered for Core code");

    let cust_loop = cycles
        .iter()
        .find(|f| unit_names(&idx, f) == ["ShopOrderCommon", "TiShopOrderExtras"])
        .expect("cycle that only exists with the Cust overlay applied");
    assert!(cust_loop.title.contains("Cust"));
    // The break point is the include in the Cust overlay, not the Core file.
    let loc = cust_loop.location.as_ref().unwrap();
    assert_eq!(idx.files[loc.file].rel_path, "shpord/model/shpord/ShopOrderCommon-Cust.fragment");
    assert!(cust_loop.suggestion.as_ref().unwrap().snippet.as_ref().unwrap().contains("- include fragment TiShopOrderExtras;"));

    assert!(cycles.iter().any(|f| unit_names(&idx, f) == ["TiQualityChecks", "TiQualityLov"]));
}

#[test]
fn core_alone_has_only_the_core_cycle() {
    let core = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/core");
    let idx = index(&IndexOptions { core: Some(&core), ..Default::default() });
    let cycles = findings(&idx, "CYCLE");
    assert_eq!(cycles.len(), 1);
    assert!(cycles[0].title.contains("in Core"));
}

#[test]
fn duplicate_definition_across_fragments() {
    let idx = fixtures();
    let d = findings(&idx, "DUPLICATE_DEFINITION");
    assert_eq!(d.len(), 1);
    assert!(d[0].message.contains("ShopOrderNoteStructure"));
    assert!(d[0].message.contains("ShopOrderHandling"));
    // `command Ok for X` style names scoped by target must not be flagged; neither must @Override.
}

#[test]
fn overlay_redeclaration_needs_override() {
    let idx = fixtures();
    let m = findings(&idx, "MISSING_OVERRIDE");
    assert_eq!(m.len(), 1);
    assert!(m[0].title.contains("GetDefaultSite"));
}

#[test]
fn overrides_resolve_to_core_or_pure_cust_bases() {
    let idx = fixtures();

    let core_based = object(&idx, "ShopOrderHandling", "ShopOrder", true);
    assert_eq!(core_based.base_origin, Some(Layer::Core));

    // The pure Cust fragment generates the base; the projection overrides that, not Core.
    let cust_based = object(&idx, "TiTracebackHandling", "TiTracebackLine", true);
    assert_eq!(cust_based.base_origin, Some(Layer::Cust));
    let base = &idx.objects[id_index(cust_based.base.as_ref().unwrap())];
    assert_eq!(idx.units[id_index(&base.unit)].name, "TiTraceback");

    let plsql = object(&idx, "Shop_Order_API", "Check_Insert___", true);
    assert_eq!(plsql.base_origin, Some(Layer::Core));

    let no_base: Vec<_> = findings(&idx, "OVERRIDE_NO_BASE").iter().map(|f| (f.severity, f.title.clone())).collect();
    assert_eq!(no_base.len(), 3, "{no_base:?}");
    assert!(no_base.iter().any(|(s, t)| *s == Severity::Warning && t.contains("Check_Update_Ti___")));
    // A state event's base is generated from the .entity state machine, which isn't parsed yet.
    assert!(no_base.iter().any(|(s, t)| *s == Severity::Warning && t.contains("Close__")));
    assert!(no_base.iter().any(|(s, t)| *s == Severity::Info && t.contains("ShopOrderOperation")));
}

#[test]
fn overrides_of_generated_methods_have_a_base() {
    let idx = fixtures();

    // Core overrides layer over the base generated from the model, which no file declares.
    let core_insert = object_in(&idx, Layer::Core, "Shop_Order_API", "Insert___");
    assert!(core_insert.base.is_none());
    assert!(core_insert.base_generated);
    assert!(object(&idx, "Shop_Order_API", "Get_Objstate", true).base_generated);

    // The Cust Insert___ layers over the Core override, even though that is an override too.
    let cust_insert = object_in(&idx, Layer::Cust, "Shop_Order_API", "Insert___");
    assert_eq!(cust_insert.base.as_deref(), Some(core_insert.id.as_str()));
    assert_eq!(cust_insert.base_origin, Some(Layer::Core));

    // A Core client override must not take the Cust command with the same name as its base.
    let unissue = object_in(&idx, Layer::Core, "ShopOrder", "UnissueCommand");
    assert!(unissue.base.is_none());
    assert!(unissue.base_generated);

    // Cust: a template method in an overlay, and anything in a pure Cust package, is generated.
    // An unknown name in an overlay is not.
    assert!(object(&idx, "Shop_Order_API", "Check_Delete___", true).base_generated);
    assert!(object(&idx, "Ti_Quality_Checks_API", "Check_Insert___", true).base_generated);
    assert!(object(&idx, "Ti_Quality_Checks_API", "Get_Check_Type", true).base_generated);
    assert!(!object(&idx, "Shop_Order_API", "Check_Update_Ti___", true).base_generated);

    for f in findings(&idx, "OVERRIDE_NO_BASE") {
        for name in ["Insert___", "Get_Objstate", "UnissueCommand", "Check_Delete___", "Get_Check_Type"] {
            assert!(!f.title.contains(name), "{}", f.title);
        }
    }
}

#[test]
fn core_alone_reports_no_overrides_without_a_base() {
    // Core never overrides Cust, so with no Cust folder there is nothing to report here.
    let core = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/core");
    let idx = index(&IndexOptions { core: Some(&core), ..Default::default() });
    assert!(findings(&idx, "OVERRIDE_NO_BASE").is_empty());
}

#[test]
fn core_code_only_links_to_core() {
    let idx = fixtures();
    // Core never depends on Cust. The one exception is a call landing on a Cust override
    // of a Core method, which is the same method layered over at runtime.
    for e in idx.edges.iter().filter(|e| idx.files[e.file].root == Layer::Core && !e.to.starts_with('r')) {
        let ok = if e.kind.is_unit_level() {
            idx.units[id_index(&e.to)].origin == Layer::Core
        } else {
            let o = &idx.objects[id_index(&e.to)];
            idx.files[o.file].root == Layer::Core || o.modifier.is_some()
        };
        assert!(ok, "Core {:?} edge from {} line {} reaches Cust {}", e.kind, idx.files[e.file].rel_path, e.line, e.to);
    }

    // The client's call lands on the action bound to ShopOrder in its own projection, which
    // the parser now reads from inside the entity body; not on the unrelated Cust action.
    let cmd = object(&idx, "ShopOrder", "PrintCommand", false);
    let targets: Vec<&Object> = idx
        .edges
        .iter()
        .filter(|e| e.from == cmd.id && e.kind == EdgeKind::CallsOperation)
        .map(|e| &idx.objects[id_index(&e.to)])
        .collect();
    assert_eq!(targets.len(), 1);
    assert_eq!(idx.units[id_index(&targets[0].unit)].name, "ShopOrderHandling");
    assert_eq!(targets[0].target.as_deref(), Some("ShopOrder"));

    // Two entities each bind a PrintOrder action; that is not a duplicate definition.
    assert!(findings(&idx, "DUPLICATE_DEFINITION").iter().all(|f| !f.message.contains("PrintOrder")));

    // A fragment's reference resolves through the projection that includes the fragment.
    let line = object(&idx, "OrderLinesFragment", "OrderLine", false);
    assert!(
        idx.edges.iter().any(|e| e.from == line.id
            && e.kind == EdgeKind::Reference
            && idx.units[id_index(&idx.objects[id_index(&e.to)].unit)].name == "UserAllowedSiteLovSelector"),
        "OrderLine.ContractRef should reach UserAllowedSiteLov"
    );

    // An action bound inside an `@Override entity` overrides the one bound to the base.
    let recalc = object(&idx, "TiTracebackHandling", "Recalculate", true);
    let base = &idx.objects[id_index(recalc.base.as_ref().expect("bound action override has a base"))];
    assert_eq!(idx.units[id_index(&base.unit)].name, "TiTraceback");
    assert!(findings(&idx, "DUPLICATE_DEFINITION").iter().all(|f| !f.message.contains("Recalculate")));
    assert!(findings(&idx, "CORE_DEPENDS_ON_CUST").is_empty(), "the rule was removed");
}

#[test]
fn deploy_ini_connections() {
    let idx = fixtures();
    let missing: Vec<String> = findings(&idx, "CONNECTION_MISSING").iter().map(|f| f.title.clone()).collect();
    assert_eq!(missing.len(), 3, "{missing:?}");
    assert!(missing.iter().any(|t| t.starts_with("SHPORD → QUAMAN")));
    assert!(missing.iter().filter(|t| t.starts_with("SHPORD → TIQUAL")).count() == 2);

    // Guarded uses of the DYNAMIC Proj connection are fine; the unguarded Cust call is not.
    let dynamic = findings(&idx, "DYNAMIC_NOT_GUARDED");
    assert_eq!(dynamic.len(), 1);
    let loc = dynamic[0].location.as_ref().unwrap();
    assert_eq!(idx.files[loc.file].root, Layer::Cust);
    assert!(dynamic[0].suggestion.as_ref().unwrap().snippet.as_ref().unwrap().contains("Component_Proj_SYS.INSTALLED"));
}

#[test]
fn layer_rules() {
    let idx = fixtures();
    let mismatch = findings(&idx, "LAYER_MISMATCH");
    assert_eq!(mismatch.len(), 1);
    assert!(mismatch[0].message.contains("TiPartNotes"));
}

#[test]
fn unresolved_include() {
    let idx = fixtures();
    let u = findings(&idx, "UNRESOLVED_INCLUDE");
    assert_eq!(u.len(), 2);
    assert!(u.iter().any(|f| f.message.contains("DoesNotExistSelector")));
    // TiPartNotes sits in the Core folder, and Core can't see the pure Cust TiQualityLov.
    assert!(u.iter().any(|f| f.message.contains("TiPartNotes") && f.message.contains("TiQualityLov")));
    assert!(findings(&idx, "UNRESOLVED_PROJECTION").is_empty());
}

#[test]
fn total_findings_are_exactly_the_planted_ones() {
    let idx = fixtures();
    assert_eq!(idx.findings.len(), 16);
}

#[test]
fn a_cust_column_that_makes_a_core_query_ambiguous() {
    let idx = fixtures();
    let text = |s: &Step| format!("{}:{} {}", idx.files[s.file].rel_path.rsplit('/').next().unwrap(), s.line, s.note.as_deref().unwrap_or(""));
    let f = findings(&idx, "COLUMN_AMBIGUOUS");
    // Only the unprefixed `priority` in the two-relation cursor; not the prefixed one, not the
    // single-relation one, not the alias priority_site.
    assert_eq!(f.len(), 1, "{:?}", f.iter().map(|f| &f.message).collect::<Vec<_>>());
    let f = f[0];
    assert_eq!(f.severity, Severity::Error);
    assert!(f.involves_cust);
    let loc = f.location.as_ref().unwrap();
    assert_eq!((idx.files[loc.file].rel_path.rsplit('/').next().unwrap(), loc.line), ("ShopOrderHandling.plsvc", 39));
    let trails: Vec<Vec<String>> = f.evidence.iter().map(|t| t.steps.iter().map(text).collect()).collect();
    assert_eq!(
        trails,
        [
            vec!["ShopOrder-Cust.entity:8 the attribute Priority of ShopOrder becomes column PRIORITY of SHOP_ORD".to_string()],
            vec![
                "ShopOrderHandling.plsvc:39 `priority` has no alias prefix, so Oracle looks for it in every relation of the FROM".to_string(),
                "ShopOrderHandling.plsvc:40 reads SHOP_ORD so and SHOP_ORDER_LINE sol".to_string(),
            ],
            vec!["ShopOrderLine.entity:16 the attribute Priority of ShopOrderLine".to_string()],
        ]
    );
    assert_eq!(f.suggestion.as_ref().unwrap().snippet.as_deref(), Some("- <NAME>Priority</NAME>\n+ <NAME>TiPriority</NAME>"));
    // The finding points at both relations and the method.
    let names: Vec<String> = f.nodes.iter().map(|n| if n.starts_with('r') { idx.relations[id_index(n)].name.clone() } else { idx.objects[id_index(n)].name.clone() }).collect();
    assert_eq!(names, ["SHOP_ORD", "SHOP_ORDER_LINE", "Get_Top_Priority___"]);
}

#[test]
fn json_is_well_formed_enough_for_the_ui() {
    let idx = fixtures();
    let s = json::to_string(&idx);
    assert!(s.starts_with("{\"version\":1,"));
    // Braces and brackets balance outside strings (evidence quotes source lines with braces).
    let (mut depth, mut in_str, mut escaped) = (0i64, false, false);
    for c in s.chars() {
        match (in_str, c) {
            (true, _) if escaped => escaped = false,
            (true, '\\') => escaped = true,
            (true, '"') | (false, '"') => in_str = !in_str,
            (false, '{' | '[') => depth += 1,
            (false, '}' | ']') => depth -= 1,
            _ => {}
        }
        assert!(depth >= 0);
    }
    assert_eq!((depth, in_str), (0, false));
    assert!(s.contains("\"kind\":\"implicit_include\""));
}

fn relation<'a>(idx: &'a Index, name: &str) -> &'a Relation {
    idx.relations.iter().find(|r| r.name == name).unwrap_or_else(|| panic!("relation {name} not found"))
}

/// What a relation reads (other relations) or calls (methods), by name.
fn reads_of(idx: &Index, name: &str) -> Vec<String> {
    let id = &relation(idx, name).id;
    let mut v: Vec<String> = idx
        .edges
        .iter()
        .filter(|e| &e.from == id)
        .map(|e| if e.to.starts_with('r') { idx.relations[id_index(&e.to)].name.clone() } else { idx.objects[id_index(&e.to)].name.clone() })
        .collect();
    v.sort();
    v.dedup();
    v
}

#[test]
fn views_lead_to_their_tables() {
    let idx = fixtures();
    let defs = |name: &str| -> Vec<String> {
        relation(&idx, name).defs.iter().map(|d| format!("{} {}:{}", d.how, idx.files[d.file].rel_path.rsplit('/').next().unwrap(), d.line)).collect()
    };

    // ShopOrder.entity renames both: its view SHOP_ORD reads SHOP_ORDER_TAB, not SHOP_ORD_TAB.
    assert_eq!(defs("SHOP_ORD"), ["generated view ShopOrder.entity:5"]);
    assert_eq!(relation(&idx, "SHOP_ORD").kind, "view");
    assert_eq!(reads_of(&idx, "SHOP_ORD"), ["SHOP_ORDER_TAB"]);
    assert_eq!(defs("SHOP_ORDER_TAB"), ["generated table ShopOrder.entity:6"]);
    assert!(idx.relations.iter().all(|r| r.name != "SHOP_ORD_TAB"));

    // ShopOrderLine.entity uses the defaults ("make-company-abstract" is not "abstract").
    assert_eq!(defs("SHOP_ORDER_LINE"), ["generated view ShopOrderLine.entity:8"]);
    assert_eq!(reads_of(&idx, "SHOP_ORDER_LINE"), ["SHOP_ORDER_LINE_TAB"]);

    // The hand-written view reads both relations in its FROM list and calls a package; the Cust
    // override adds the customer's notes table.
    assert_eq!(defs("SHOP_ORDER_OVERVIEW"), ["view ShopOrder.views:14", "view (@Override) ShopOrder-Cust.views:5"]);
    assert_eq!(reads_of(&idx, "SHOP_ORDER_OVERVIEW"), ["Get_Part_Description", "SHOP_ORD", "SHOP_ORDER_LINE", "TI_SHOP_ORDER_NOTE_TAB"]);

    // A table defined in .storage, and one generated for a pure Cust LU.
    assert_eq!(defs("SHOP_ORDER_HIST_TAB"), ["table ShopOrder.storage:5"]);
    // Tables created by PL/SQL blocks in .storage, .cre and .cdb files, named in strings. The
    // .cdb block that only alters SHOP_ORDER_TAB doesn't define it.
    assert_eq!(defs("SHOP_ORDER_QUEUE_TAB"), ["temporary table ShopOrder.storage:16"]);
    assert_eq!(defs("SHOP_ORDER_SNAP_TAB"), ["temporary table ShopOrderSnapshot.cre:6"]);
    assert_eq!(defs("SHOP_ORDER_SU_LOG_TAB"), ["table 25R1-SU01.cdb:3"]);
    assert_eq!(defs("SHOP_ORDER_TAB"), ["generated table ShopOrder.entity:6"]);
    assert_eq!(defs("TI_QUALITY_CHECKS_TAB"), ["generated table TiQualityChecks-Cust.entity:3"]);

    // ShopOrderKPI renames only its view; the table keeps the LU's name, and every capital of
    // the acronym starts a word. The function GetLateKPI finds Get_Late_K_P_I___, whose WITH
    // name late_orders is not a relation.
    assert_eq!(reads_of(&idx, "SHOP_ORD_KPI"), ["SHOP_ORDER_K_P_I_TAB"]);
    assert_eq!(defs("SHOP_ORDER_K_P_I_TAB"), ["generated table ShopOrderKPI.entity:8"]);
    let f = object(&idx, "ShopOrderHandling", "GetLateKPI", false);
    let chain = reach(&idx, &f.id);
    assert!(chain.iter().any(|c| c == "function Get_Late_K_P_I___"), "{chain:?}");
    assert!(chain.iter().any(|c| c == "table SHOP_ORDER_K_P_I_TAB"), "{chain:?}");
    assert!(idx.relations.iter().all(|r| r.name != "LATE_ORDERS"));
}

#[test]
fn findings_show_how_they_were_reached() {
    let idx = fixtures();
    let text = |s: &Step| format!("{}:{} {}", idx.files[s.file].rel_path.rsplit('/').next().unwrap(), s.line, s.text);

    // The duplicate structure: one trail per definition, from the projection, line by line.
    let dup = &findings(&idx, "DUPLICATE_DEFINITION")[0];
    let trails: Vec<Vec<String>> = dup.evidence.iter().map(|t| t.steps.iter().map(text).collect()).collect();
    assert_eq!(
        trails,
        [
            vec![
                "ShopOrderHandling.projection:12 include fragment ShopOrderCommon;",
                "ShopOrderCommon.fragment:32 structure ShopOrderNoteStructure {",
            ],
            vec![
                "ShopOrderHandling.projection:12 include fragment ShopOrderCommon;",
                "ShopOrderCommon-Cust.fragment:6 include fragment TiShopOrderExtras;",
                "TiShopOrderExtras-Cust.fragment:21 structure ShopOrderNoteStructure {",
            ],
        ]
    );
    assert!(dup.involves_cust);

    // MISSING_OVERRIDE cites both declarations.
    let m = &findings(&idx, "MISSING_OVERRIDE")[0];
    assert_eq!(m.evidence[0].steps.len(), 2);
    assert!(m.evidence[0].steps.iter().all(|s| s.text.contains("GetDefaultSite")));

    // The Core-only cycle involves no Cust file; the one that appears with Cust does.
    let cycles = findings(&idx, "CYCLE");
    assert!(!cycles.iter().find(|f| f.title.contains("in Core")).unwrap().involves_cust);
    assert!(cycles.iter().filter(|f| f.involves_cust).count() == 2);
}

/// Follows outgoing object-level links from one object, depth first, and returns
/// each reached node as `kind name` (objects) or `table|view NAME` (relations).
fn reach(idx: &Index, start: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![start.to_string()];
    let mut seen = std::collections::HashSet::new();
    while let Some(id) = stack.pop() {
        if !seen.insert(id.clone()) {
            continue;
        }
        out.push(if id.starts_with('r') {
            let r = &idx.relations[id_index(&id)];
            format!("{} {}", r.kind, r.name)
        } else {
            let o = &idx.objects[id_index(&id)];
            format!("{} {}", o.kind, o.name)
        });
        for e in idx.edges.iter().filter(|e| e.from == id) {
            stack.push(e.to.clone());
        }
    }
    out
}

#[test]
fn objects_link_all_the_way_down_to_tables() {
    let idx = fixtures();
    let cmd = object(&idx, "ShopOrder", "ReleaseCommand", false);
    let chain = reach(&idx, &cmd.id);
    for step in [
        "action ReleaseOrder",          // client command → projection action
        "procedure Release_Order___",   // → .plsvc implementation
        "procedure Release__",          // → Shop_Order_API
        "table SHOP_ORDER_TAB",         // → SQL in Release__
        "procedure Check_Insert___",    // same-package call, lands on the Cust override…
        "function Get_Name",            // …which calls Project_API (Cust code)
        "table PROJECT_TAB",
        "procedure Exist",              // …and through @Override to the Core base
    ] {
        assert!(chain.iter().any(|c| c == step), "missing {step} in {chain:?}");
    }

    // fetch = "Shop_Order_API.Get_Part_Description(...)" and from = "shop_ord".
    let entity = object(&idx, "ShopOrderHandling", "ShopOrder", false);
    let chain = reach(&idx, &entity.id);
    for step in ["view SHOP_ORD", "table SHOP_ORDER_TAB", "function Get_Part_Description", "function Get_Description", "table INVENTORY_PART_TAB"] {
        assert!(chain.iter().any(|c| c == step), "missing {step} in {chain:?}");
    }
}

#[test]
fn upgrade_comparison_says_what_changed_under_each_customization() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
    let (core, cust, prev) = (root.join("core"), root.join("cust"), root.join("prev-core"));
    let idx = index(&IndexOptions { core: Some(&core), cust: Some(&cust), prev_core: Some(&prev) });
    let mut got: Vec<(String, Severity, String)> = idx
        .findings
        .iter()
        .filter(|f| f.rule == "BASE_CHANGED" || f.rule == "ENTITY_CHANGED")
        .map(|f| (f.rule.clone(), f.severity, f.title.clone()))
        .collect();
    got.sort_by(|a, b| a.2.cmp(&b.2));
    assert_eq!(
        got,
        [
            ("ENTITY_CHANGED".to_string(), Severity::Info, "ShopOrder changed under your -Cust.entity".to_string()),
            ("BASE_CHANGED".to_string(), Severity::Info, "group ShopOrderGroup in ShopOrder.client changed under your @Override".to_string()),
            ("BASE_CHANGED".to_string(), Severity::Info, "procedure Check_Insert___ in ShopOrder.plsql changed under your @Override".to_string()),
            ("BASE_CHANGED".to_string(), Severity::Info, "procedure Insert___ in ShopOrder.plsql changed under your @Override".to_string()),
        ]
    );
    let by_title = |t: &str| idx.findings.iter().find(|f| f.title.starts_with(t)).unwrap();

    // Check_Insert___ gained the $IF block: the diff has just those lines, numbered in both versions.
    let f = by_title("procedure Check_Insert___");
    let d = f.diff.as_ref().unwrap();
    assert!(d.old_path.ends_with("ShopOrder.plsql") && d.old_path.contains("prev-core"));
    assert_eq!((d.old_line, d.new_line), (7, 7));
    let added: Vec<(&str, Option<usize>)> = d.lines.iter().filter(|l| l.op == "+").map(|l| (l.text.trim(), l.new)).collect();
    assert_eq!(added[0], ("$IF Component_Proj_SYS.INSTALLED $THEN", Some(12)));
    assert!(d.lines.iter().all(|l| l.op != "-"));
    assert_eq!(idx.files[f.location.as_ref().unwrap().file].rel_path.rsplit('/').next(), Some("ShopOrder-Cust.plsql"));
    // Next to IFS's change: your customization's code, from its declaration to its END.
    assert_eq!(idx.files[d.cust_file.unwrap()].rel_path.rsplit('/').next(), Some("ShopOrder-Cust.plsql"));
    assert_eq!(d.cust_line, 8);
    assert_eq!((d.cust_code.first().map(String::as_str), d.cust_code.last().map(String::as_str)), (Some("PROCEDURE Check_Insert___ ("), Some("END Check_Insert___;")));

    // Insert___ gained attr_; the Cust override already has it.
    assert!(by_title("procedure Insert___").message.contains("already has the new one"));
    // The group's declaration changed, not the page's `group ShopOrderGroup;`.
    let g = by_title("group ShopOrderGroup").diff.as_ref().unwrap();
    assert_eq!(g.new_line, 51);
    assert!(g.lines.iter().any(|l| l.op == "+" && l.text.contains("\"General\"")));
    // The entity lost an attribute.
    let e = by_title("ShopOrder changed under");
    assert!(e.message.contains("Removed: OrderStatus"), "{}", e.message);
    assert!(idx.notes.iter().any(|n| n.starts_with("Upgrade comparison with")));
    // The comparison's own findings are all caused by the upgrade.
    assert!(idx.findings.iter().filter(|f| f.rule == "BASE_CHANGED" || f.rule == "ENTITY_CHANGED").all(|f| f.upgrade));

    // The ambiguous `priority` is caused by the upgrade too: before, the cursor read only
    // SHOP_ORDER_LINE; IFS added SHOP_ORD, which has the Cust column. The diff shows that.
    let c = findings(&idx, "COLUMN_AMBIGUOUS")[0];
    assert!(c.upgrade);
    assert!(c.message.ends_with("in the previous Core this query didn't read SHOP_ORD; IFS added it."), "{}", c.message);
    let d = c.diff.as_ref().unwrap();
    assert!(d.lines.iter().any(|l| l.op == "-" && l.text.trim() == "FROM   shop_order_line sol"));
    assert!(d.lines.iter().any(|l| l.op == "+" && l.text.trim() == "FROM   shop_ord so, shop_order_line sol"));
    assert_eq!(idx.files[d.cust_file.unwrap()].rel_path.rsplit('/').next(), Some("ShopOrder-Cust.entity"));
    assert_eq!(d.cust_code, ["<NAME>Priority</NAME>"]);

    // Without a previous Core nothing is marked as caused by the upgrade.
    let plain = fixtures();
    assert!(plain.findings.iter().all(|f| f.rule != "BASE_CHANGED" && f.rule != "ENTITY_CHANGED" && !f.upgrade));
}
