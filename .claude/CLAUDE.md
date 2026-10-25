# C:\devstudio workspace

- `ifs-depmap\ifs-depmap\` is the IFS Dependency Map tool (Rust + TypeScript + Tauri).
  Its own CLAUDE.md has the project context: read it before working on the tool, and run
  cargo / npm commands from that folder.
- `project-parent-25r1\` is the real IFS Cloud 25R1 source (Developer Studio project).
  It is READ-ONLY input for the tool: never write, move or delete anything in it.
  - Core layer: `project-parent-25r1\core-files\25.1.16\checkout`
  - Cust layer: `project-parent-25r1\RU-releaseUpdate\workspace`
- `*.zip` files here are the original downloads; leave them alone.
- `/ifs-kickoff` runs the first-session checklist (build, index the real folders, check accuracy).
