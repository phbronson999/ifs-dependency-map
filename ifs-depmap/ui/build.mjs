// Builds the UI with nothing but Node and the TypeScript compiler.
//
//   node ui/build.mjs                      → ui/dist/index.html + app.js + styles.css (what Tauri loads)
//   node ui/build.mjs --demo graph.json    → also ui/dist/demo.html: one self-contained page with the
//                                            graph embedded (open it in any browser, no app needed)

import { execFileSync } from "node:child_process";
import { mkdirSync, readFileSync, writeFileSync, existsSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const dist = join(here, "dist");
mkdirSync(dist, { recursive: true });

// 1. Compile. Prefer the local devDependency, fall back to a global tsc.
const localTsc = join(here, "..", "node_modules", "typescript", "bin", "tsc");
if (existsSync(localTsc)) execFileSync(process.execPath, [localTsc, "-p", join(here, "tsconfig.json")], { stdio: "inherit" });
else execFileSync("tsc", ["-p", join(here, "tsconfig.json")], { stdio: "inherit", shell: process.platform === "win32" });

// The sources are plain scripts (no imports), joined in dependency order.
const js = ["types", "layout", "graphs", "app"]
  .map((f) => readFileSync(join(here, ".build", `${f}.js`), "utf8"))
  .join("\n");
const css = readFileSync(join(here, "src", "styles.css"), "utf8");
const shell = readFileSync(join(here, "src", "shell.html"), "utf8");
const fonts =
  '<link rel="preconnect" href="https://fonts.googleapis.com">\n' +
  '<link rel="stylesheet" href="https://fonts.googleapis.com/css2?family=IBM+Plex+Mono:wght@400;600&family=IBM+Plex+Sans+Condensed:wght@600&family=IBM+Plex+Sans:wght@400;500;600&display=swap">';

// 2. The app page.
writeFileSync(join(dist, "app.js"), js);
writeFileSync(join(dist, "styles.css"), css);
writeFileSync(
  join(dist, "index.html"),
  `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover">
<title>IFS Dependency Map</title>
${fonts}
<link rel="stylesheet" href="styles.css">
</head>
<body>
${shell}
<script src="app.js"></script>
</body>
</html>
`,
);
console.log("built ui/dist/index.html");

// 3. Optional single-file demo with an embedded graph.
const demoArg = process.argv.indexOf("--demo");
if (demoArg > 0) {
  const graph = JSON.parse(readFileSync(process.argv[demoArg + 1], "utf8"));
  // Show paths relative to the sample folders instead of the machine that built it.
  for (const f of graph.files) f.abs_path = `fixtures/${f.root.toLowerCase()}/${f.rel_path}`;
  graph.core_root = graph.core_root ? "fixtures/core" : null;
  graph.cust_root = graph.cust_root ? "fixtures/cust" : null;
  const data = JSON.stringify(graph).replace(/</g, "\\u003c");
  writeFileSync(
    join(dist, "demo.html"),
    `<title>IFS Dependency Map</title>
<meta name="claude:viewport-fit-sides" content="cover">
${fonts}
<style>
${css}
</style>
${shell}
<script type="application/json" id="demo-data">${data}</script>
<script>
${js}
</script>
`,
  );
  console.log("built ui/dist/demo.html");
}
