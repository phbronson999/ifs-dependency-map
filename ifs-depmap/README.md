# IFS Dependency Map

Indexes an IFS Cloud **Core** folder and a **Cust** folder into one dependency
graph, shows it at three zoom levels (components → files → objects), and flags
the things that get in the way, like circular fragment includes.

It is read-only. It never writes to the folders it indexes. Each finding links to
the file and line, shows an example of the fix and, under *How this was found*,
the source lines that led to it, with any step that rests on an assumption marked.
Findings that involve only Core files are hidden by default (Core can't be
changed and compiles as delivered); the *Core only* chip shows them.

## What it checks

| Rule | Severity | What it means |
|---|---|---|
| `CYCLE` | error | `include fragment` chains that loop. Marked *appears with Cust layer* when the loop only exists once Cust overlays are applied; Core-only loops are reported but no fix is suggested. |
| `DUPLICATE_DEFINITION` | error | The same construct (structure, entity, function…) reaches one client/projection from two different units. |
| `MISSING_OVERRIDE` | error | A `-Cust` overlay redeclares a Core construct without `@Override`. |
| `CONNECTION_MISSING` | warning | A cross-component include or `Xxx_API` call with no `[Connections]` entry in deploy.ini. |
| `DYNAMIC_NOT_GUARDED` | warning | A DYNAMIC connection used without `@DynamicComponentDependency` or a `$IF Component_X_SYS.INSTALLED` guard. |
| `LAYER_MISMATCH` | warning | `layer Cust;` in the Core folder, or `layer Core;` in the Cust folder. |
| `OVERRIDE_NO_BASE` | warning / info | `@Override`/`@Overtake` with nothing to layer over (info for projection entities, whose base may be an LU `.entity`). |
| `UNRESOLVED_INCLUDE` / `UNRESOLVED_PROJECTION` | warning | Name not found in either folder. |

## How Core and Cust are modelled

- **Layer comes from the folder.** Everything under the Core folder is Core;
  everything under the Cust folder is Cust.
- **Unit** = a logical name. `ShopOrderCommon.fragment` (Core) and
  `ShopOrderCommon-Cust.fragment` (Cust) are one unit with an *overlay*.
- A `-Cust` file with no Core counterpart is a **pure Cust** unit; its
  generated base is the base for any `@Override` of its constructs.
- **Core never depends on Cust.** Core is IFS's delivered code, so names in Core
  files only resolve against Core.
- `@Override` resolves to whichever base exists, Core or pure Cust, preferring
  the same unit, then what the unit includes. When no file declares the base, it
  is generated from the model (`.entity` / `.projection`), as for every Core
  override of `Insert___`, `CRUD_Create___` or a getter.
- Other names (calls, references, bindings) resolve in the same unit, then what
  it includes, then what the client or projection it ends up in includes.
- deploy.ini `[Connections]` from the Core and Cust copies of a component are
  combined.

## Object links (Objects view)

The Objects view follows these links downward as far as they go:

| Link | Found from |
|---|---|
| client construct → projection object | `for` / `using` |
| command → action/function | `call ReleaseOrder(...)` |
| entityset → entity | `entityset X for Y` |
| entity → entity / enumeration | `reference`, `array`, `Enumeration(X)` |
| action/function → .plsvc method | IFS naming: `ReleaseOrder` → `Release_Order___` |
| method → method | `Xxx_API.Method(...)`, same-package `Method___(...)`, and `fetch = "Xxx_API.M(...)"` |
| override → base | `@Override` / `@Overtake` |
| entity/query/method → table or view | `from = "view"`, `FROM` / `JOIN` / `UPDATE` / `INSERT INTO` in SQL |

A call into a method that Cust overrides lands on the override, then follows
`@Override` to the Core base. Tables and views are names only until `.views` and
`.storage` are indexed.

## Layout

```
crates/ifsdep-core/   Rust indexer + rules, std only (no crates)
  src/marble.rs       .client / .projection / .fragment parser
  src/plsql.rs        .plsql / .plsvc scanner (methods, Xxx_API calls, guards)
  src/deployini.rs    deploy.ini
  src/build.rs        units, name resolution, edges
  src/analyze.rs      the rules above
  src/bin/ifsdep.rs   CLI
  tests/fixtures.rs   end-to-end tests
fixtures/core, fixtures/cust   sample folders with every problem planted
ui/                   TypeScript UI, no runtime dependencies (own SVG renderer + layered layout)
src-tauri/            Tauri 2 shell: folder picker, indexing, open in VS Code
```

## Running

```sh
cargo test                                         # 27 tests
cargo run --bin ifsdep -- --core <dir> --cust <dir> [--json graph.json] [--strict]

npm install                                        # typescript + tauri cli
npm run tauri dev                                  # desktop app
npm run demo                                       # ui/dist/demo.html with the sample data embedded
```

The desktop app remembers the two folders. "Open" on any file runs
`code -g path:line`, so the VS Code `code` command must be on your PATH.

## Not done yet

- `.entity` / `.enumeration` (XML) parsing. Needs a real sample file.
- `.views` and `.storage`.
- `lov X with ReferenceYSelector` links.
- Persisted index with incremental rescans (indexing is fast enough to run in full for now: about 0.2 s for 6,000 files).
