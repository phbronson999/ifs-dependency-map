# IFS Dependency Map

Desktop tool that indexes an IFS Cloud **Core** folder and a **Cust** folder into one
dependency graph, shows it at three levels (components → files → objects), and flags
problems: circular fragment includes, duplicate definitions, missing @Override,
deploy.ini connection gaps, unguarded DYNAMIC dependencies, layer mistakes.
Background on the IFS file types: https://phbronson999.github.io/ifs-dev-docs/

## Real IFS folders on this machine (READ-ONLY)
- Core: `C:\devstudio\project-parent-25r1\core-files\25.1.16\checkout`
  (one folder per component, ~300 components)
- Cust: `C:\devstudio\project-parent-25r1\RU-releaseUpdate\workspace`
  (~650 `-Cust` files over ~25 components, plus the custom component `tietex` and `config`;
  contains .entity, .plsql, .fragment, .client, .projection, .plsvc, .views, .storage,
  .utility, .enumeration, .ial, .cdb, deploy.ini)
- `customer-baseline/` and `project-parent-25r1/workspace/` hold almost nothing; ignore them
  unless the user says otherwise.

## Layout
- `crates/ifsdep-core/` Rust indexer + rules + `ifsdep` CLI. **std only, no crates.**
  - `marble.rs` .client/.projection/.fragment · `plsql.rs` .plsql/.plsvc · `deployini.rs`
  - `build.rs` units, name resolution, edges · `analyze.rs` rules · `json.rs` output
- `fixtures/core`, `fixtures/cust` synthetic sample folders; every rule has a planted case
- `crates/ifsdep-core/tests/fixtures.rs` end-to-end tests against the fixtures
- `ui/` plain TypeScript, no runtime deps (own SVG renderer + layered layout); `node ui/build.mjs`
- `src-tauri/` Tauri 2 shell: `pick_folder`, `index_folders`, `open_in_editor` (`code -g`)

## Commands
- `cargo test` (49 tests, must stay green)
- `cargo run --release --bin ifsdep -- --core <dir> --cust <dir> --json out.json [--strict]`
- `npm install`, then `npm run tauri dev` / `npm run demo` (builds ui/dist/demo.html)

## Model (decided with the user, don't change without asking)
- Layer comes from the folder a file is in, never from the filename.
- A unit = logical name. `X.fragment` (Core) + `X-Cust.fragment` (Cust) = one unit with an overlay.
  A `-Cust` file with no Core match is a pure Cust unit and is the base for overrides of its constructs.
- Core is never edited, so Core never depends on Cust: names in Core files resolve against Core
  only (decided 2026-10-25; the CORE_DEPENDS_ON_CUST rule was removed for this reason).
- `@Override`/`@Overtake` resolve to whichever base exists (Core or pure Cust): same unit first,
  then what the unit includes, then anywhere. With no base in any file the base is generated
  from the model (any Core override, a pure Cust package, or a template method like `Insert___`).
- Other names (calls, references, bindings) resolve in the same unit, then what it includes,
  then what a client/projection it ends up in includes. Never anywhere.
- Calls into an overridden method land on the override, then follow Overrides to the base.
- Objects view follows links downward to tables/views, and views to the tables they read.
- The tool is read-only: it suggests fixes but never writes to IFS files.

## Status and assumptions to verify against real code
- Built and tested in a sandbox without network: core and UI are tested, the Tauri shell
  has NEVER been compiled. Icons in src-tauri/icons are placeholders.
- Assumed: a -Cust file's header uses the base name (`fragment ShopOrderCommon;`).
- Assumed: [Connections] from a component's Core and Cust deploy.ini are combined.
- Assumed: projection action `DoIt` is implemented by `.plsvc` method `Do_It___`.
- Component = header `component X;`, else the first folder under the root.
- `.views` / `.storage` are parsed for definitions and reads; `.entity` only for the view and
  table names; `.cre` / `.cdb` only for tables their PL/SQL blocks create. Not parsed yet:
  the rest of `.entity`, `.enumeration`, `.utility`, `.upg`.
- IFS names split at every capital (`JobKPI` → `Job_K_P_I_API`, `JOB_K_P_I_TAB`); checked on
  every Core entity with an acronym (2026-10-25). A generated table is `snake(LU)_TAB` even
  when `DB_VIEW_NAME` renames the view.
- Generated methods (checked 2026-10-26 on 25R1 Core calls): from each `.entity`, IFS generates
  `Get()` (Public_Rec: one field per `IS_PUBLIC` attribute without `EXCLUDE_GET_METHOD`),
  `Get_<Attr>()` and `Get_<Attr>_Db()` (enumerations). A getter coexists with a hand-written method
  of the same name as an overload (it takes the LU's keys), e.g. `Customer_Order_API.Get_Customer_Po_No`.
- A derived attribute gets its value from `DB_SQL_IMPLEMENTATION`, an association `USE_DEFINITION`
  (`COLUMN_NAME`, exposed as `COLUMN_ALIAS`), or the LU's hand-written view in `.views` (the user,
  2026-10-26). Hand-written views name their columns by alias.
- Unverified: whether an `abstract` LU gets a generated table. The rule says no; three
  abstract LUs (CustOrdRentalIvcItem, RmAccUsage, RmAccFilterType) have tables that code reads
  and only a .upg creates.

## Rules for working here
- NEVER write, move or delete anything under `C:\devstudio\project-parent-25r1` or `C:\devstudio\project-parent-23r2`. Read only.
- Never copy real IFS or customer source into `fixtures/` or anywhere in this repo. Write
  minimal synthetic snippets that reproduce the syntax instead.
- Every parser fix gets a fixture case and a test. Keep the core crate dependency-free and
  the UI free of runtime dependencies; ask before adding any.
- Put generated output (graph.json etc.) in `target/` or `ui/.build/`, both git-ignored.
