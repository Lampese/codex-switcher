import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const packageJson = JSON.parse(readFileSync(new URL("../package.json", import.meta.url), "utf8"));
const lockfile = readFileSync(new URL("../pnpm-lock.yaml", import.meta.url), "utf8");

function assertCompatibleCli(version: string) {
  const match = /^(?:\^|~)?2\.(\d+)\.(\d+)$/.exec(version);
  assert.ok(match && Number(match[1]) >= 10, `Incompatible Tauri CLI: ${version}`);
}

test("the declared and locked CLI support the runtime's bundle marker", () => {
  const specifier = packageJson.devDependencies["@tauri-apps/cli"];
  assertCompatibleCli(specifier);
  const importer = /      '@tauri-apps\/cli':\n        specifier: (.+)\n        version: (.+)\n/.exec(lockfile);
  assert.ok(importer, "Missing root CLI lockfile entry");
  assert.equal(importer[1], specifier);
  assertCompatibleCli(importer[2]);
});

test("all locked CLI binaries and optional references use the root CLI version", () => {
  const rootVersion = /      '@tauri-apps\/cli':\n        specifier: .+\n        version: (.+)\n/.exec(lockfile)?.[1];
  assert.ok(rootVersion);
  const packages = [...lockfile.matchAll(/^  '@tauri-apps\/cli(?:-[^@'\n]+)?@([^'\n]+)':/gm)];
  const references = [...lockfile.matchAll(/^      '@tauri-apps\/cli-[^'\n]+': ([^\n]+)$/gm)];
  assert.ok(packages.length > 0, "Missing CLI packages/snapshots");
  assert.ok(references.length > 0, "Missing platform binary dependencies");
  for (const [, version] of [...packages, ...references]) {
    assert.equal(version, rootVersion);
    assertCompatibleCli(version);
  }
});
