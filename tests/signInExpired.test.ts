import assert from "node:assert/strict";
import test from "node:test";
import { isSignInExpiredError } from "../src/lib/signInExpired.ts";

test("a rejected refresh token asks for a new sign-in", () => {
  const message = "Sign-in expired: Token refresh failed: 401 Unauthorized - {}";
  assert.equal(isSignInExpiredError(message), true);
  assert.equal(isSignInExpiredError(new Error(message)), true);
});

test("other switch errors do not ask for a new sign-in", () => {
  for (const error of [
    "Token refresh failed: 500 Internal Server Error - {}",
    "Cannot switch accounts while 1 Codex process is running",
    new Error("Account not found: x"),
    null,
    undefined,
  ]) {
    assert.equal(isSignInExpiredError(error), false);
  }
});
