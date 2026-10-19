import assert from "node:assert/strict";
import test from "node:test";
import { needsRelogin } from "../src/lib/accountAuth.ts";

test("re-login is offered for authentication failures, not quota or network errors", () => {
  for (const message of [
    "API error: 401 Unauthorized", "HTTP 401", "invalid_grant",
    "refresh_token_reused", "refresh_token_expired", "token_invalidated",
    "Missing refresh token for account", "Your authentication token has been invalidated",
  ]) assert.equal(needsRelogin(message), true, message);
  for (const message of [null, undefined, "", "API error: 403 Forbidden", "429 Too Many Requests",
    "usage_limit_exceeded", "Network timeout", "invalid JSON", "request ID 40123 failed"])
    assert.equal(needsRelogin(message), false, String(message));
});
