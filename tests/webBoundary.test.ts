import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

test("web client does not embed a server authentication secret", async () => {
  const source = await readFile(new URL("../src/lib/platform.ts", import.meta.url), "utf8");

  assert.doesNotMatch(source, /VITE_CODEX_SWITCHER_WEB_SECRET/);
  assert.match(source, /sessionStorage\.getItem\(WEB_SECRET_STORAGE_KEY\)/);
});
