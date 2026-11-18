import assert from "node:assert/strict";
import test from "node:test";
import { retryBusyUsage } from "../src/lib/retryBusyUsage.ts";

test("skipped background checks retry at most two times", async () => {
  let calls = 0;
  const delays: number[] = [];
  const result = await retryBusyUsage(async () => {
    calls++;
    if (calls <= 2) throw new Error("Background OAuth refresh busy; retry next poll");
    return "ok";
  }, async (delay) => { delays.push(delay); });
  assert.equal(result, "ok");
  assert.equal(calls, 3);
  assert.deepEqual(delays, [300, 600]);
});

test("a persistent busy state stops after three reads", async () => {
  let calls = 0;
  await assert.rejects(retryBusyUsage(async () => {
    calls++;
    throw new Error("Background OAuth refresh busy");
  }, async () => {}), /Background OAuth refresh busy/);
  assert.equal(calls, 3);
});

test("a terminal OAuth error is never retried", async () => {
  let calls = 0;
  await assert.rejects(retryBusyUsage(async () => {
    calls++;
    throw new Error("refresh_token_reused");
  }, async () => {}), /refresh_token_reused/);
  assert.equal(calls, 1);
});

test("ordinary network failures are not retried automatically", async () => {
  let calls = 0;
  await assert.rejects(retryBusyUsage(async () => {
    calls++;
    throw new Error("Network timeout");
  }, async () => {}), /Network timeout/);
  assert.equal(calls, 1);
});
