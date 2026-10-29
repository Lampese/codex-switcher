import assert from "node:assert/strict";
import test from "node:test";
import { completeOAuthWithoutBlockingOnUsage } from "../src/lib/oauthCompletion.ts";

test("successful OAuth completion does not wait for a stuck usage request", async () => {
  const account = { id: "newly-authenticated" };
  const calls: string[] = [];
  const result = await completeOAuthWithoutBlockingOnUsage(
    async () => { calls.push("save"); return account; },
    async () => { calls.push("reload"); },
    async (saved) => {
      assert.equal(saved, account);
      calls.push("usage");
      return new Promise<void>(() => { /* simulates a stuck API request */ });
    },
    () => assert.fail("usage should not fail"),
  );

  assert.equal(result, account);
  assert.deepEqual(calls, ["save", "reload", "usage"]);
});

test("a failed OAuth completion is still reported and skips reload/usage", async () => {
  const oauthError = new Error("failed to save tokens");
  let reloaded = false;
  let refreshed = false;
  await assert.rejects(
    completeOAuthWithoutBlockingOnUsage(
      async () => { throw oauthError; },
      async () => { reloaded = true; },
      async () => { refreshed = true; },
      () => assert.fail("usage should not run"),
    ),
    (error) => error === oauthError,
  );
  assert.equal(reloaded, false);
  assert.equal(refreshed, false);
});

test("background usage failure cannot turn a successful login into failure", async () => {
  const error = new Error("usage service unavailable");
  const logged: unknown[] = [];
  const account = await completeOAuthWithoutBlockingOnUsage(
    async () => ({ id: "renewed" }),
    async () => undefined,
    async () => { throw error; },
    (err) => { logged.push(err); },
  );
  await new Promise<void>((resolve) => setImmediate(resolve));
  assert.equal(account.id, "renewed");
  assert.deepEqual(logged, [error]);
});
