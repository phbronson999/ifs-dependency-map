//! End-to-end checks against the synthetic Core + Cust trees in /fixtures.
//! Each problem is planted on purpose; the assertions say what the tool must
//! find and, as importantly, what it must not flag.

use ifsdep_core::*;
use std::path::PathBuf;

fn fixtures() -> Index {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
    let core = root.join("core");
    let cust = root.join("cust");
    index(&IndexOptions { core: Some(&core), cust: Some(&cust) })
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
    assert_eq!(idx.stats.files, 34);
    assert!(idx.notes.is_empty(), "{:?}", idx.notes);
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
    let idx = index(&IndexOptions { core: Some(&core), cust: None });
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
    // Core calls Close__, which only a Cust override declares: its base is below Core.
    assert!(findings(&idx, "CORE_DEPENDS_ON_CUST").iter().all(|f| !f.title.contains("Close__")));
}

#[test]
fn core_alone_reports_no_overrides_or_cust_dependencies() {
    // Core never overrides Cust, so with no Cust folder there is nothing to report here.
    let core = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/core");
    let idx = index(&IndexOptions { core: Some(&core), cust: None });
    assert!(findings(&idx, "OVERRIDE_NO_BASE").is_empty());
    assert!(findings(&idx, "CORE_DEPENDS_ON_CUST").is_empty());
}

#[test]
fn deploy_ini_connections() {
    let idx = fixtures();
    let missing: Vec<String> = findings(&idx, "CONNECTION_MISSING").iter().map(|f| f.title.clone()).collect();
    assert_eq!(missing.len(), 4, "{missing:?}");
    assert!(missing.iter().any(|t| t.starts_with("SHPORD → QUAMAN")));
    assert!(missing.iter().filter(|t| t.starts_with("SHPORD → TIQUAL")).count() == 2);
    assert!(missing.iter().any(|t| t.starts_with("MPCCOM → TIQUAL")));

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
    let core_cust = findings(&idx, "CORE_DEPENDS_ON_CUST");
    assert_eq!(core_cust.len(), 1);
    assert!(core_cust[0].title.contains("TiQualityLov"));
}

#[test]
fn unresolved_include() {
    let idx = fixtures();
    let u = findings(&idx, "UNRESOLVED_INCLUDE");
    assert_eq!(u.len(), 1);
    assert!(u[0].message.contains("DoesNotExistSelector"));
    assert!(findings(&idx, "UNRESOLVED_PROJECTION").is_empty());
}

#[test]
fn total_findings_are_exactly_the_planted_ones() {
    let idx = fixtures();
    assert_eq!(idx.findings.len(), 16);
}

#[test]
fn json_is_well_formed_enough_for_the_ui() {
    let idx = fixtures();
    let s = json::to_string(&idx);
    assert!(s.starts_with("{\"version\":1,"));
    assert_eq!(s.matches('{').count(), s.matches('}').count());
    assert!(s.contains("\"kind\":\"implicit_include\""));
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
    for step in ["view SHOP_ORD", "function Get_Part_Description", "function Get_Description", "table INVENTORY_PART_TAB"] {
        assert!(chain.iter().any(|c| c == step), "missing {step} in {chain:?}");
    }

    // The Core call into a method that Cust overrides is not a Core → Cust dependency.
    assert_eq!(findings(&idx, "CORE_DEPENDS_ON_CUST").len(), 1);
}
