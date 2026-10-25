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
- `cargo test` (24 tests, must stay green)
- `cargo run --release --bin ifsdep -- --core <dir> --cust <dir> --json out.json [--strict]`
- `npm install`, then `npm run tauri dev` / `npm run demo` (builds ui/dist/demo.html)

## Model (decided with the user, don't change without asking)
- Layer comes from the folder a file is in, never from the filename.
- A unit = logical name. `X.fragment` (Core) + `X-Cust.fragment` (Cust) = one unit with an overlay.
  A `-Cust` file with no Core match is a pure Cust unit and is the base for overrides of its constructs.
- `@Override`/`@Overtake` resolve to whichever base exists (Core or pure Cust): same unit first,
  then what the unit includes, then anywhere.
- Calls into an overridden method land on the override, then follow Overrides to the base.
- Objects view follows links downward to tables/views (names only until .views is parsed).
- The tool is read-only: it suggests fixes but never writes to IFS files.

## Status and assumptions to verify against real code
- Built and tested in a sandbox without network: core and UI are tested, the Tauri shell
  has NEVER been compiled. Icons in src-tauri/icons are placeholders.
- Assumed: a -Cust file's header uses the base name (`fragment ShopOrderCommon;`).
- Assumed: [Connections] from a component's Core and Cust deploy.ini are combined.
- Assumed: projection action `DoIt` is implemented by `.plsvc` method `Do_It___`.
- Component = header `component X;`, else the first folder under the root.
- Not parsed yet: `.entity` / `.enumeration` / `.utility` (XML), `.views`, `.storage`.

## Rules for working here
- NEVER write, move or delete anything under `C:\devstudio\project-parent-25r1`. Read only.
- Never copy real IFS or customer source into `fixtures/` or anywhere in this repo. Write
  minimal synthetic snippets that reproduce the syntax instead.
- Every parser fix gets a fixture case and a test. Keep the core crate dependency-free and
  the UI free of runtime dependencies; ask before adding any.
- Put generated output (graph.json etc.) in `target/` or `ui/.build/`, both git-ignored.
