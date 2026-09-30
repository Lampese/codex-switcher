import assert from "node:assert/strict";
import test from "node:test";
import {
  parseCodexClosePreference,
  rememberedCodexClosePreference,
} from "../src/lib/codexClosePreference.ts";

test("close preference defaults to asking with graceful close selected", () => {
  for (const value of [null, "", "true", "always", "invalid"]) {
    assert.equal(parseCodexClosePreference(value), "ask");
  }
});

test("close preference accepts remembered close methods", () => {
  assert.equal(parseCodexClosePreference("graceful"), "graceful");
  assert.equal(parseCodexClosePreference("force"), "force");
});

test("remembering force makes force the next default", () => {
  assert.equal(rememberedCodexClosePreference(true, true), "force");
  assert.equal(rememberedCodexClosePreference(false, true), "graceful");
  assert.equal(rememberedCodexClosePreference(true, false), "ask");
});
