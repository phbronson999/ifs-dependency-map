//! The AI index (`ifsdep-ai init` and the queries) against the synthetic fixtures:
//! it reads back what it wrote, and each query answers with the right places.

use ifsdep_core::ai::{self, AiIndex, Node, Opts};
use ifsdep_core::*;
use std::path::PathBuf;

fn fixtures() -> (Index, AiIndex) {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures");
    let core = std::path::absolute(root.join("core")).unwrap();
    let cust = std::path::absolute(root.join("cust")).unwrap();
    let idx = index(&IndexOptions { core: Some(&core), cust: Some(&cust), ..Default::default() });
    let text = ai::write(&idx, None, &idx.stamps, 0);
    let ix = ai::parse(&text).expect("parses");
    (idx, ix)
}

fn opts() -> Opts {
    Opts { depth: 1, limit: 500 }
}

#[test]
fn reads_back_what_it_wrote() {
    let (idx, ix) = fixtures();
    assert_eq!(ix.files.len(), idx.files.len());
    assert_eq!(ix.units.len(), idx.units.len());
    assert_eq!(ix.objects.len(), idx.objects.len());
    assert_eq!(ix.relations.len(), idx.relations.len());
    assert_eq!(ix.edges.len(), idx.edges.len());
    assert_eq!(ix.findings.len(), idx.findings.len());
    // Messages and snippets keep their tabs and newlines.
    for (a, b) in ix.findings.iter().zip(&idx.findings) {
        assert_eq!(a.message, b.message);
        assert_eq!(a.snippet, b.suggestion.as_ref().and_then(|s| s.snippet.clone()).unwrap_or_default());
    }
    for (a, b) in ix.objects.iter().zip(&idx.objects) {
        assert_eq!((a.name.as_str(), a.line, a.base.map(|o| format!("o{o}"))), (b.name.as_str(), b.line, b.base.clone()));
    }
    assert!(ai::parse("something else\n").is_err());
}

#[test]
fn resolves_names_in_any_case() {
    let (_, ix) = fixtures();
    let v = ix.resolve("user_allowed_site_api.get_default_site");
    assert_eq!(v.len(), 1);
    assert_eq!(ix.label(v[0]), "User_Allowed_Site_API.Get_Default_Site (function)");
    assert!(matches!(ix.resolve("User_Allowed_Site_API")[..], [Node::Unit(_)]));
    assert!(matches!(ix.resolve("user_allowed_site_pub")[..], [Node::Relation(_)]));
    // No exact match: the error lists names that contain it.
    let e = ix.find("AllowedSite", &opts()).unwrap_err();
    assert!(e.contains("User_Allowed_Site_API"), "{e}");
}

#[test]
fn find_shows_both_layers_of_an_override() {
    let (_, ix) = fixtures();
    let s = ix.find("ShopOrderHandling.ShopOrder", &opts()).unwrap();
    assert!(s.contains("2 matches"), "{s}");
    assert!(s.contains("overridden by ShopOrderHandling.ShopOrder (entity @Override)"), "{s}");
    assert!(s.contains("overrides ShopOrderHandling.ShopOrder (entity) at [Core]"), "{s}");
    assert!(s.contains("ShopOrderHandling-Cust.projection:11"), "{s}");
}

#[test]
fn callers_and_callees_follow_links() {
    let (_, ix) = fixtures();
    let s = ix.links("User_Allowed_Site_API.Get_Default_Site", true, &Opts { depth: 2, limit: 500 }).unwrap();
    assert!(s.contains("<- invokes ShopOrderHandling.Get_Default_Site___ (function)"), "{s}");
    assert!(s.contains("ShopOrderHandling.plsvc:10"), "{s}");
    // Depth 2 reaches the projection function the .plsvc method implements, Cust first.
    let cust = s.find("ShopOrderHandling-Cust.projection:17").expect(&s);
    let core = s.find("ShopOrderHandling.projection:59").expect(&s);
    assert!(cust < core, "{s}");
    let s = ix.links("User_Allowed_Site_API.Get_Default_Site", false, &opts()).unwrap();
    assert!(s.contains("-> reads USER_ALLOWED_SITE_PUB (view)"), "{s}");
}

#[test]
fn impact_reaches_cust_units() {
    let (_, ix) = fixtures();
    let s = ix.impact("USER_ALLOWED_SITE_PUB", &Opts { depth: 6, limit: 500 }).unwrap();
    assert!(s.contains("[Cust] TiTracebackHandling (projection, TIQUAL) 2 hops"), "{s}");
    assert!(s.contains("[Core] User_Allowed_Site_API (package, FNDBAS) 1 hop"), "{s}");
    // Depth 1 stops at what reads the view directly.
    let s = ix.impact("USER_ALLOWED_SITE_PUB", &Opts { depth: 1, limit: 500 }).unwrap();
    assert!(!s.contains("TiTracebackHandling"), "{s}");
}

#[test]
fn unit_lists_files_and_cust_changes() {
    let (_, ix) = fixtures();
    let s = ix.unit("shoporderhandling", &opts()).unwrap();
    assert!(s.contains("ShopOrderHandling (projection), component SHPORD, base in Core, with a Cust overlay"), "{s}");
    assert!(s.contains("@Override entity ShopOrder :11"), "{s}");
    assert!(s.contains("new function GetDefaultSite :17"), "{s}");
    assert!(s.contains("MISSING_OVERRIDE"), "{s}");
}

#[test]
fn findings_filter() {
    let (idx, ix) = fixtures();
    let cycles = idx.findings.iter().filter(|f| f.rule == "CYCLE").count();
    let s = ix.findings_list(None, None, Some("cycle"), false, &opts()).unwrap();
    assert!(s.starts_with(&format!("{cycles} findings")), "{s}");
    let s = ix.findings_list(Some("ShopOrderHandling.plsvc"), None, None, false, &opts()).unwrap();
    assert!(s.contains("COLUMN_AMBIGUOUS"), "{s}");
    assert!(s.contains("+ <NAME>TiPriority</NAME>"), "{s}");
}

#[test]
fn trace_says_where_each_call_goes() {
    let (_, ix) = fixtures();
    let s = ix.trace("ShopOrderHandling.Get_Default_Site___", &opts()).unwrap();
    assert!(s.contains("call USER_ALLOWED_SITE_API.Get_Default_Site :10  → [Core]"), "{s}");
    assert!(s.contains("UserAllowedSite.plsql:16"), "{s}");
    assert!(ix.trace("ShopOrderHandling", &opts()).is_err());
}

#[test]
fn status_up_to_date_and_skill_filled_in() {
    let (_, ix) = fixtures();
    assert!(ix.status().contains("up to date"), "{}", ix.status());
    let skill = ai::skill("C:\\tools\\ifsdep-ai.exe", "C:\\ws\\ifsdep-ai.idx", &[("Core", "C:\\core")], 0);
    assert!(!skill.contains("{{"), "{skill}");
    assert!(skill.starts_with("---\nname: ifs-codebase\n"));
    assert!(skill.contains("\"C:\\tools\\ifsdep-ai.exe\" <command> <name> --index \"C:\\ws\\ifsdep-ai.idx\""));
    assert_eq!(ai::date(0), "1970-01-01 00:00 UTC");
    assert_eq!(ai::date(1_793_145_600_000), "2026-10-28 00:00 UTC");
}
