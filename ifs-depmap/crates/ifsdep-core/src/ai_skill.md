---
name: ifs-codebase
description: Use whenever you read, change, review or debug IFS Cloud code in this workspace (.plsql, .plsvc, .entity, .views, .storage, .client, .projection, .fragment, deploy.ini, and -Cust files) - to find where a package, method, entity, projection, table or view is defined in Core and Cust, what calls it, what a change would affect, what a PL/SQL method does step by step, and which problems are already known. Faster and more accurate than grep, because it resolves IFS layering, overrides and generated methods.
---

# IFS codebase: ask the dependency index before grepping

This workspace has an index built by IFS Dependency Map (`ifsdep-ai init`, {{BUILT}}).
It knows how the Core and Cust layers fit together, which grep can't tell you.

Folders it covers:
{{ROOTS}}

Run the queries with:

```
"{{IFSDEP}}" <command> <name> --index "{{INDEX}}"
```

(In PowerShell, put `&` in front of the quoted path.) Every answer is plain text with absolute
`path:line` locations you can open directly.

## Commands

| Question | Command |
|---|---|
| Where is X defined, in which layer, and what overrides it? | `find <name>` |
| What links to X (calls, includes, binds, reads)? | `callers <name> [--depth N]` |
| What does X link to? | `callees <name> [--depth N]` |
| What would changing X affect? | `impact <name> [--depth N]` |
| All files of a unit across Core and Cust, what Cust adds or overrides | `unit <name>` |
| What does this PL/SQL method do, in order, by branch, and where does each call go? | `trace <Package_API.Method>` |
| Known problems (the tool's rules) | `findings [--file <part of path>] [--unit <name>] [--rule <RULE>] [--cust]` |
| Is the index still current? | `status` |

`<name>` is `Unit.Object` (`Customer_Order_API.New__`, `CustomerOrderHandling.ReleaseCommand`),
or a single name: a unit (`Customer_Order_API`, `ShopOrderHandling`), an object (`New__`),
or a table or view (`CUSTOMER_ORDER_TAB`). Case doesn't matter. If nothing matches exactly,
the answer lists names that contain it. `--limit N` raises the line cap.

## Use these, not grep, for how code connects

Grep is about as fast, but it finds text: a name search mixes calls to the method you mean
with same-named methods in other packages, comments and the definition. It also can't see
methods IFS generates from `.entity` files (`Get_<Attr>`, `Get_<Attr>_Db`). These queries
resolve each name to one declaration, through Core and Cust layers, overrides, includes,
client → projection → `.plsvc`, and generated methods.

Use grep, or read the file, for:
- text inside code: message strings, comments, column names in SQL;
- `.enumeration`, `.utility`, `.upg` and most of `.entity`, which the index doesn't read;
- state machine events: a client's `call Release()` on an entity's state event has no link yet,
  so don't conclude from `callers` that nothing triggers an event;
- anything changed since the index was written (`status`).

## How to work

1. Before changing a method, run `find` and `trace` on it. Then run `callers` to find every path
   that reaches it, and `impact` to see what else to check. A check placed in one caller misses
   the others.
2. To follow a client command to the database, run `callees <Client>.<Command> --depth 3`.
3. Before adding a Cust override, run `find` on the base: it may already be overridden.
4. When a file is under discussion, `findings --file <name>` shows the problems already found there.
5. The index is a snapshot. If `status` says files changed, line numbers may be off, so read the file itself.
   The user refreshes the index by running `ifsdep-ai init` again. Don't rebuild it on your own.
6. The index says where things are. Read the actual file before you make claims about the code
   or change it. If an answer and the file disagree, the file is right; tell the user, since
   that's an index error worth reporting.

## IFS rules the index follows

- The layer comes from the folder a file is in: Core or Cust. **Core is never edited.** Changes
  go in Cust, as `-Cust` files.
- `X.fragment` in Core plus `X-Cust.fragment` in Cust make up one unit. A `-Cust` file with no Core match is a pure Cust unit.
- `@Override` / `@Overtake` resolve to the base in the same unit first, then in what the unit
  includes, then anywhere. A call into an overridden method lands on the override, which then
  reaches the base.
- Core names resolve against Core only: Core never depends on Cust.
- Package names split at every capital: `JobKPI` becomes `Job_K_P_I_API` and `JOB_K_P_I_TAB`.
- IFS generates `Get()`, `Get_<Attr>()` and `Get_<Attr>_Db()` from each `.entity`. They are in no
  source file, and `find` points at the entity instead.
- A projection action `DoIt` is taken to be implemented by the `.plsvc` method `Do_It___`.
  This is assumed, not yet checked, so confirm it in the `.plsvc` file.
- deploy.ini `[Connections]` lists the components a component depends on. Components reached
  through a chain of STATIC connections are always installed with it, so they count too. A dependency guarded
  with `@DynamicComponentDependency` or `$IF Component_X_SYS.INSTALLED` exists only when that
  component is installed.
