import assert from "node:assert/strict";
import test from "node:test";
import { formatUsageError, needsRelogin } from "../src/lib/usageError.ts";

test("transient access failures do not suggest an invalid saved login", () => {
  for (const error of [
    "API error: 401 Unauthorized",
    "Failed to send token refresh request: timed out",
  ]) {
    assert.equal(needsRelogin(error), false, error);
    assert.equal(formatUsageError(error).title, "Usage check unsuccessful");
  }
});

test("429 usage limits are not authentication failures", () => {
  const error = "API error: 429 Too Many Requests";
  assert.equal(needsRelogin(error), false);
  assert.equal(formatUsageError(error).title, "Rate limited, try again later");
});

test("upstream terminal refresh failures require removing and re-adding the account", () => {
  for (const error of [
    'Token refresh failed: 401 Unauthorized - {"error":{"code":"refresh_token_reused"}}',
    'Token refresh failed: 401 Unauthorized - {"error":{"code":"refresh_token_expired"}}',
    'Token refresh failed: 400 Bad Request - {"error":"invalid_grant"}',
    "Missing refresh token for account",
    "Token refresh did not return a fresh id_token",
  ]) {
    assert.equal(needsRelogin(error), true, error);
    const shown = formatUsageError(error);
    assert.equal(shown.title, "Switcher sign-in needs renewal");
    assert.match(shown.message, /Remove this account and add it again/);
  }
});

test("unrecognized refresh rejections do not falsely claim revocation", () => {
  const error = "Token refresh failed: 401 Unauthorized - ";
  assert.equal(needsRelogin(error), false);
  assert.equal(formatUsageError(error).title, "Usage check unsuccessful");
});

test("bad usage format and unrelated errors keep distinct messages", () => {
  assert.equal(formatUsageError("Failed to parse usage response").title, "Usage format changed");
  const forbidden = formatUsageError("API error: 403 Forbidden");
  assert.equal(forbidden.title, "Usage unavailable");
  assert.equal(forbidden.message, "API error: 403 Forbidden");
});
