import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { normalizeBackupPassphrase, isPassphraseRequiredError } from "../src/lib/backupPassphrase.ts";

test("backup passphrases reject blank input and preserve meaningful input", () => {
  assert.equal(normalizeBackupPassphrase("   "), null);
  assert.equal(normalizeBackupPassphrase("  correct horse  "), "correct horse");
});

test("backup passphrase collection uses a masked React input instead of a browser prompt", () => {
  const appSource = readFileSync(new URL("../src/App.tsx", import.meta.url), "utf8");
  const platformSource = readFileSync(new URL("../src/lib/platform.ts", import.meta.url), "utf8");
  assert.match(appSource, /id="backup-passphrase"[\s\S]*type="password"/);
  assert.doesNotMatch(platformSource, /window\.prompt/);
});

test("passphrase-required backend errors remain detectable for import retry", () => {
  assert.equal(isPassphraseRequiredError(new Error("A passphrase is required for this backup")), true);
  assert.equal(isPassphraseRequiredError(new Error("wrong passphrase")), false);
});
