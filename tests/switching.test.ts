import test from "node:test";
import assert from "node:assert/strict";

import {
  getSwitchConfirmationMessage,
  getSwitchErrorMessage,
  hasRunningCodexProcesses,
} from "../src/utils/switching.ts";

test("hasRunningCodexProcesses returns true when foreground or background codex is present", () => {
  assert.equal(
    hasRunningCodexProcesses({ count: 6, background_count: 0, can_switch: false, pids: [1] }),
    true
  );
  assert.equal(
    hasRunningCodexProcesses({ count: 0, background_count: 2, can_switch: false, pids: [] }),
    true
  );
  assert.equal(
    hasRunningCodexProcesses({ count: 0, background_count: 0, can_switch: true, pids: [] }),
    false
  );
});

test("getSwitchConfirmationMessage includes both foreground and background counts", () => {
  assert.equal(
    getSwitchConfirmationMessage({
      count: 6,
      background_count: 0,
      can_switch: false,
      pids: [1, 2, 3],
    }),
    "Codex is running (6 foreground, 0 background). Do you want Codex Switcher to close and reopen it gracefully before switching accounts?"
  );
});

test("getSwitchErrorMessage unwraps strings and Error-like objects", () => {
  assert.equal(getSwitchErrorMessage("plain failure"), "plain failure");
  assert.equal(getSwitchErrorMessage({ message: "object failure" }), "object failure");
  assert.equal(getSwitchErrorMessage(null), "Unknown error");
});
