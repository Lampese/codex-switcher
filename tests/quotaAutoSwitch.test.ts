import assert from "node:assert/strict";
import test from "node:test";
import {
  checkQuotaAutoSwitch,
  quotaState,
  readAutoSwitchEnabled,
  runAutomaticDesktopSwitch,
} from "../src/lib/quotaAutoSwitch.ts";
import type { RecoveryProgress, RecoveryState } from "../src/lib/sessionRecovery.ts";
import type { AccountInfo, CodexProcessInfo, UsageInfo } from "../src/types/index.ts";

const account = (id: string, api = false) => ({ id, auth_mode: api ? "api_key" : "chat_g_p_t" } as AccountInfo);
const usage = (primary: number | null, secondary: number | null = 0, extra = {}) => ({
  primary_used_percent: primary, secondary_used_percent: secondary, error: null, ...extra,
} as UsageInfo);

test("only confirmed exhausted quota triggers; unknown values and credits remain safe", () => {
  assert.equal(readAutoSwitchEnabled(), false);
  for (const value of [usage(100), usage(0, 100), usage(101)]) assert.equal(quotaState(value), "exhausted");
  for (const value of [usage(99.9), usage(0), usage(100, 100, { has_credits: true }), usage(100, 100, { unlimited_credits: true })]) assert.equal(quotaState(value), "available");
  for (const value of [usage(null, null), usage(NaN), usage(-1), usage(0, null, { secondary_window_minutes: 100 }), usage(100, 0, { error: "offline" })]) assert.equal(quotaState(value), "unknown");
});

function fixture(options: {
  quotas?: Record<string, UsageInfo>;
  running?: boolean;
  processCount?: number;
  desktopCount?: number;
  activeCount?: number;
  unsupported?: boolean;
  changed?: boolean;
  reset?: boolean;
  cancel?: boolean;
  fail?: boolean;
} = {}) {
  const calls: string[] = [];
  const switched: string[] = [];
  let activeReads = 0;
  let sourceReads = 0;
  const invoke = async <T>(command: string, args?: Record<string, unknown>): Promise<T> => {
    calls.push(`${command}:${args?.accountId ?? ""}`);
    let value: unknown;
    if (command === "get_active_account_info") value = account(options.changed && activeReads++ > 0 ? "other" : "source");
    else if (command === "list_accounts") value = [account("before"), account("source"), account("api", true), account("bad"), account("next")];
    else if (command === "get_usage") {
      const id = args!.accountId as string;
      if (id === "bad") throw new Error("Expired credentials");
      value = options.quotas?.[id] ?? (id === "source" ? usage(options.reset && sourceReads++ > 0 ? 0 : 100) : usage(0));
    } else if (command === "check_codex_processes") {
      const count = options.processCount ?? (options.running ? 1 : 0);
      value = { count, can_switch: count === 0 };
    } else if (command === "get_codex_reopen_info") {
      value = {
        supported: !options.unsupported,
        desktop_count: options.desktopCount ?? (options.unsupported ? 0 : 1),
        ...(options.activeCount !== undefined ? { active_count: options.activeCount } : {}),
      };
    } else throw new Error(command);
    return value as T;
  };
  const run = () => checkQuotaAutoSwitch({ invoke, cancelled: () => !!options.cancel, switchAccount: async (target, source, reopen) => {
    if (options.fail) throw new Error("Close failed");
    switched.push(`${source}->${target.id}:${reopen}`);
  } });
  return { run, calls, switched };
}

test("checks fresh quota, skips API keys and failed requests, selects next saved account", async () => {
  const f = fixture({ running: true });
  assert.equal((await f.run()).switched, true);
  assert.deepEqual(f.switched, ["source->next:true"]);
  assert.equal(f.calls.filter(c => c === "get_usage:source").length, 2);
  assert.ok(!f.calls.includes("get_usage:api"));
});

test("wraps saved order and switches without reopening when Codex is closed", async () => {
  const f = fixture({ quotas: { next: usage(0, 100) } });
  assert.equal((await f.run()).switched, true);
  assert.deepEqual(f.switched, ["source->before:false"]);
  assert.ok(!f.calls.includes("get_codex_reopen_info:"));
});

test("no capacity, unknown quota, source changes, resets, disabling and unsupported desktops never switch", async () => {
  for (const options of [
    { quotas: { next: usage(100), before: usage(null, null) } },
    { quotas: { source: usage(null, null) } },
    { quotas: { source: usage(99.9) } },
    { changed: true }, { reset: true }, { cancel: true }, { running: true, unsupported: true },
  ]) {
    const f = fixture(options);
    assert.equal((await f.run()).switched, false);
    assert.deepEqual(f.switched, []);
  }
});

test("mixed desktop and terminal CLI sessions pause automatic switching", async () => {
  for (const options of [
    { running: true, processCount: 2, desktopCount: 1 },
    { running: true, processCount: 1, desktopCount: 1, activeCount: 2 },
  ]) {
    const f = fixture(options);
    const outcome = await f.run();
    assert.equal(outcome.switched, false);
    assert.match(outcome.message, /cannot be reopened automatically/);
    assert.deepEqual(f.switched, []);
  }

  const allDesktops = fixture({ running: true, processCount: 2, desktopCount: 2, activeCount: 2 });
  assert.equal((await allDesktops.run()).switched, true);
  assert.deepEqual(allDesktops.switched, ["source->next:true"]);
});

test("switch failures propagate to pause automatic retries rather than report success", async () => {
  const f = fixture({ fail: true });
  await assert.rejects(f.run(), /Close failed/);
  assert.deepEqual(f.switched, []);
});

test("automatic desktop switch preserves conversation recovery when enabled and cancels on close rejection", async () => {
  const events: string[] = [];
  const states: RecoveryState[] = [];
  await runAutomaticDesktopSwitch({
    accountId: "next",
    sourceId: "source",
    forceClose: false,
    autoResumeSessions: true,
    isAutoSwitchEnabled: () => true,
    invoke: async <T>(command: string, args?: Record<string, unknown>): Promise<T> => {
      events.push(command);
      if (command === "get_active_account_info") return account("source") as T;
      if (command === "prepare_codex_session_recovery") return 2 as T;
      if (command === "reopen_closed_codex_desktop") {
        assert.equal(args?.token, "reopen-1");
        return undefined as T;
      }
      if (command === "resume_codex_session_recovery") {
        const send = args?.onProgress as (progress: RecoveryProgress) => void;
        send({ completed: 1, total: 2, current_session_id: "s1" });
        return { resumed: ["s1", "s2"], skipped: [], failed: [], timed_out: false } as T;
      }
      throw new Error(command);
    },
    closeProcesses: async (reopen, force, requireAllDesktops) => {
      events.push(`close:${reopen}:${force}:${requireAllDesktops}`);
      return { processInfo: { count: 0, background_count: 0, can_switch: true, pids: [] } as CodexProcessInfo, reopenToken: "reopen-1" };
    },
    switchAccount: async (targetId, expectedSourceId) => {
      events.push(`switch:${expectedSourceId}->${targetId}`);
    },
    onRecoveryStateChange: (state) => { states.push(state); },
    showToast: () => {},
    formatError: String,
    createProgressChannel: async (onMessage) => onMessage,
  });
  assert.deepEqual(events, [
    "get_active_account_info",
    "prepare_codex_session_recovery",
    "close:true:false:true",
    "get_active_account_info",
    "switch:source->next",
    "reopen_closed_codex_desktop",
    "resume_codex_session_recovery",
  ]);
  assert.deepEqual(states, [
    { status: "running", progress: { completed: 0, total: 2, current_session_id: null } },
    { status: "running", progress: { completed: 1, total: 2, current_session_id: "s1" } },
    { status: "complete", report: { resumed: ["s1", "s2"], skipped: [], failed: [], timed_out: false } },
  ]);

  const rejectedEvents: string[] = [];
  await assert.rejects(
    runAutomaticDesktopSwitch({
      accountId: "next",
      sourceId: "source",
      forceClose: false,
      autoResumeSessions: true,
      isAutoSwitchEnabled: () => true,
      invoke: async <T>(command: string): Promise<T> => {
        rejectedEvents.push(command);
        if (command === "get_active_account_info") return account("source") as T;
        if (command === "prepare_codex_session_recovery") return 1 as T;
        if (command === "cancel_codex_session_recovery") return undefined as T;
        throw new Error(command);
      },
      closeProcesses: async (_reopen, _force, requireAllDesktops) => {
        assert.equal(requireAllDesktops, true);
        throw new Error("Running Codex processes include sessions that cannot be reopened automatically");
      },
      switchAccount: async () => { rejectedEvents.push("switch"); },
      onRecoveryStateChange: () => { rejectedEvents.push("state"); },
      showToast: () => {},
      formatError: String,
    }),
    /cannot be reopened automatically/,
  );
  assert.deepEqual(rejectedEvents, [
    "get_active_account_info",
    "prepare_codex_session_recovery",
    "cancel_codex_session_recovery",
  ]);
});
