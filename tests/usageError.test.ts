import assert from "node:assert/strict";
import test from "node:test";
import {
  formatUsageError,
  isBackgroundAuthBusy,
  needsRelogin,
} from "../src/lib/usageError.ts";

test("a busy background OAuth lock is not an expired login", () => {
  const error = "Background OAuth refresh busy; retry next poll";
  assert.equal(isBackgroundAuthBusy(error), true);
  assert.equal(needsRelogin(error), false);
  assert.equal(formatUsageError(error).title, "Authentication check postponed");
});

test("generic access 401 and transient failures do not prompt re-login", () => {
  for (const error of [
    "API error: 401 Unauthorized",
    "Could not refresh the saved Codex Switcher session (401 Unauthorized)",
    "429 Too Many Requests",
    "Failed to send token refresh request: timed out",
  ]) {
    assert.equal(needsRelogin(error), false, error);
    assert.equal(formatUsageError(error).title, "Authentication check unsuccessful");
  }
});

test("explicit terminal credential failures request re-login", () => {
  for (const error of [
    "refresh_token_reused",
    "refresh_token_invalidated",
    "invalid_grant",
    "Codex Switcher's saved session is out of date",
    "Missing refresh token for account",
    "Token refresh did not return a fresh id_token",
  ]) {
    assert.equal(needsRelogin(error), true, error);
    assert.equal(formatUsageError(error).title, "Switcher sign-in needs renewal");
  }
});

test("bad usage format and unrelated errors keep distinct messages", () => {
  assert.equal(formatUsageError("Failed to parse usage response").title, "Usage format changed");
  const forbidden = formatUsageError("API error: 403 Forbidden");
  assert.equal(forbidden.title, "Usage unavailable");
  assert.equal(forbidden.message, "API error: 403 Forbidden");
});
