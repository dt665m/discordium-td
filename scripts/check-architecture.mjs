#!/usr/bin/env node

import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { readFileSync, readdirSync, existsSync, statSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const metadata = JSON.parse(execFileSync("cargo", ["metadata", "--format-version", "1", "--no-deps"], {
  cwd: root, encoding: "utf8",
}));
const packages = new Map(metadata.packages.map(pkg => [pkg.name, pkg]));
const engineRoot = path.join(root, "engine") + path.sep;
const gameRoot = path.join(root, "games") + path.sep;

// Require capability entry points, while leaving helper-file organization flexible.
const pluginGroups = [
  ["engine/core", ["system", "abilities", "combat", "physics", "spawn", "progression", "graphics"]],
  ["engine/client", ["camera", "graphics", "network_tools"]],
  ["games/dreamwake/client", ["camera", "graphics", "input", "audio", "ui", "network", "diagnostics"]],
  ["engine/server", ["server"]],
  ["games/dreamwake/server", ["server"]],
];
for (const [crate, capabilities] of pluginGroups) {
  for (const capability of capabilities) {
    const relative = `${crate}/src/plugins/${capability}/mod.rs`;
    const filename = path.join(root, relative);
    assert(existsSync(filename) && statSync(filename).isFile(),
      `Missing capability plugin entry point: ${relative}`);
  }
}

// Follow all workspace dependencies, including build and test dependencies.
// An indirect engine -> helper -> game dependency also breaks the boundary.
function checkDependencies(pkg, chain = [], visited = new Set()) {
  if (visited.has(pkg.name)) return;
  visited.add(pkg.name);
  const next = [...chain, pkg.name];
  assert(!pkg.manifest_path.startsWith(gameRoot), `Engine depends on a game: ${next.join(" -> ")}`);
  for (const dep of pkg.dependencies) {
    const local = packages.get(dep.name);
    if (local) checkDependencies(local, next, visited);
  }
}

for (const pkg of packages.values()) {
  if (pkg.manifest_path.startsWith(engineRoot)) checkDependencies(pkg);
}

function checkSources(directory) {
  for (const entry of readdirSync(directory, { withFileTypes: true })) {
    const filename = path.join(directory, entry.name);
    if (entry.isDirectory()) {
      if (!["target", "node_modules", "dist"].includes(entry.name)) checkSources(filename);
    } else if (entry.name.endsWith(".rs") || entry.name === "Cargo.toml") {
      const source = readFileSync(filename, "utf8");
      assert(!/dreamwake|dream_net|game_sim::|game_shared::|discordium|\bTD_/i.test(source),
        `Game-specific naming in engine source: ${path.relative(root, filename)}`);
    }
  }
}
checkSources(path.join(root, "engine"));

for (const legacy of ["game_sim", "game_shared", "game_client", "game_server", "game_dream_net"]) {
  assert(!packages.has(legacy), `Retired package remains in the workspace: ${legacy}`);
}
console.log("Architecture checks passed: capability entry points exist; engine dependencies and source naming are game-neutral.");
