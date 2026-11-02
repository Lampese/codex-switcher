import assert from "node:assert/strict";
import test from "node:test";
import { finishForceClose, parseDesktopReopenPreference } from "../src/lib/desktopReopen.ts";

test("unknown preferences default to asking; both remembered choices are preserved", () => {
  for (const value of [null, "", "true", "invalid"]) {
    assert.equal(parseDesktopReopenPreference(value), "ask");
  }
  assert.equal(parseDesktopReopenPreference("always"), "always");
  assert.equal(parseDesktopReopenPreference("never"), "never");
});

test("waits for account switching before reopening the captured desktop", async () => {
  const calls: string[] = [];
  let completeSwitch!: () => void;
  const pendingSwitch = new Promise<void>((resolve) => { completeSwitch = resolve; });
  const result = finishForceClose({ canSwitch: true, reopenToken: "captured" }, async () => {
    calls.push("switch");
    await pendingSwitch;
    calls.push("switched");
  }, async (token) => { calls.push(`reopen:${token}`); });
  assert.deepEqual(calls, ["switch"]);
  completeSwitch();
  await result;
  assert.deepEqual(calls, ["switch", "switched", "reopen:captured"]);
});

test("a failed switch leaves the desktop closed", async () => {
  let reopened = false;
  await assert.rejects(finishForceClose({ canSwitch: true, reopenToken: "captured" }, async () => {
    throw new Error("switch failed");
  }, async () => { reopened = true; }), /switch failed/);
  assert.equal(reopened, false);
});

test("remaining processes prevent both switching and reopening", async () => {
  const calls: string[] = [];
  await finishForceClose({ canSwitch: false, reopenToken: "captured" }, async () => { calls.push("switch"); }, async () => { calls.push("reopen"); });
  assert.deepEqual(calls, []);
});

test("missing desktop identity or opting out retains close-and-switch behavior", async () => {
  const calls: string[] = [];
  await finishForceClose({ canSwitch: true, reopenToken: null }, async () => { calls.push("switch"); }, async () => { calls.push("reopen"); });
  assert.deepEqual(calls, ["switch"]);
});

test("standalone force close can reopen without switching accounts", async () => {
  const calls: string[] = [];
  await finishForceClose({ canSwitch: true, reopenToken: "captured" }, null, async (token) => { calls.push(token); });
  assert.deepEqual(calls, ["captured"]);
});

test("reopen failure is reported after switching and never retries the account change", async () => {
  const calls: string[] = [];
  await assert.rejects(finishForceClose({ canSwitch: true, reopenToken: "captured" }, async () => {
    calls.push("switched");
  }, async () => {
    calls.push("reopen failed");
    throw new Error("Could not launch desktop");
  }), /Could not launch desktop/);
  assert.deepEqual(calls, ["switched", "reopen failed"]);
});
