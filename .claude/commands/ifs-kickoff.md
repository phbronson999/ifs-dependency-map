---
description: First session for IFS Dependency Map, covering the build, indexing the real IFS folders and checking parser accuracy
---

Read `ifs-depmap/CLAUDE.md` and the root `README.md` first. Work from
`ifs-depmap`. This project was scaffolded without ever being run against real IFS
code, and the Tauri shell has never been compiled. Your job this session is to get it
building and then find out how well the parsers match the real files. Don't change the
parsers yet. Never write to anything under `project-parent-25r1`.

1. **Build.** Run `cargo test`, then `npm install` and `npm run tauri dev`. Fix whatever
   stops src-tauri from compiling or launching, keeping changes inside src-tauri unless
   you tell me why. Tell me what you changed.

2. **Index the real folders** with the CLI (release build), writing the JSON to `target/`:
   - Core: `C:\devstudio\project-parent-25r1\core-files\25.1.16\checkout`
   - Cust: `C:\devstudio\project-parent-25r1\RU-releaseUpdate\workspace`

   Report file counts by type, units, objects, edges, time taken, any `notes`, and
   findings grouped by rule with counts. Then run Cust alone and Core alone for comparison.

3. **Check accuracy.** For each rule, open 3–5 findings in the actual files and say whether
   each is a true problem or a false positive, with file:line evidence. Look hardest at the
   biggest sources of UNRESOLVED_INCLUDE and OVERRIDE_NO_BASE, since those usually point at
   a parser or naming assumption that doesn't hold.

4. **Check the assumptions** listed under "Status" in CLAUDE.md against real files and say
   which hold and which don't. The Cust folder also has a `config` component and the custom
   component `tietex`: check how they fit the model.

5. **Look at unparsed types.** Open one real Core `.entity` and its `-Cust.entity` (both
   XML), plus one `.views` file. Describe where the LU name, attributes,
   associations/references, state machine and view definitions live, and propose how to
   parse each with std only.

End with a prioritised list of parser fixes. For each: what's wrong, the evidence, the
fix, and the synthetic fixture you'd add to test it. Wait for my go-ahead before changing
parser code.

$ARGUMENTS
