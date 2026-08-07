import assert from "node:assert/strict";
import test from "node:test";
import { switchAndReopenCodex } from "../src/lib/runningSwitch.ts";

test("reopens Codex only after the account switch completes", async () => {
  const calls: string[] = [];

  const result = await switchAndReopenCodex({
    switchAccount: async () => {
      calls.push("switch");
    },
    reopenCodex: async () => {
      calls.push("reopen");
    },
  });

  assert.deepEqual(result, { status: "success" });
  assert.deepEqual(calls, ["switch", "reopen"]);
});

test("does not reopen Codex when the account switch fails", async () => {
  const calls: string[] = [];
  const switchError = new Error("switch failed");

  const result = await switchAndReopenCodex({
    switchAccount: async () => {
      calls.push("switch");
      throw switchError;
    },
    reopenCodex: async () => {
      calls.push("reopen");
    },
  });

  assert.deepEqual(result, { status: "switch_failed", error: switchError });
  assert.deepEqual(calls, ["switch"]);
});

test("reports a partial success when reopening Codex fails", async () => {
  const calls: string[] = [];
  const reopenError = new Error("reopen failed");

  const result = await switchAndReopenCodex({
    switchAccount: async () => {
      calls.push("switch");
    },
    reopenCodex: async () => {
      calls.push("reopen");
      throw reopenError;
    },
  });

  assert.deepEqual(result, { status: "reopen_failed", error: reopenError });
  assert.deepEqual(calls, ["switch", "reopen"]);
});
