import assert from "node:assert/strict";
import test from "node:test";
import {
  formatResetCreditDateTime,
  getAvailableResetCredits,
  getResetCreditsDisplayCount,
  queueResetCreditsCheck,
} from "../src/lib/resetCredits.ts";
import type { AccountResetCredit, AccountResetCredits } from "../src/types/index.ts";

function credit(
  id: string,
  status: string,
  expiresAt: string | null,
): AccountResetCredit {
  return {
    id,
    reset_type: "codex_rate_limits",
    status,
    granted_at: null,
    expires_at: expiresAt,
    redeem_started_at: null,
    redeemed_at: null,
    title: null,
    description: null,
  };
}

test("available reset credits exclude redeemed and expired rows and sort by expiry", () => {
  const resetCredits: AccountResetCredits = {
    available_count: 4,
    next_expires_at: "2026-07-28T08:30:00Z",
    credits: [
      credit("no-expiry", "available", null),
      credit("later", "available", "2026-07-30T08:30:00Z"),
      credit("redeemed", "redeemed", "2026-07-28T08:30:00Z"),
      credit("expired", "available", "2026-07-26T08:30:00Z"),
      credit("earlier", "available", "2026-07-28T08:30:00Z"),
    ],
  };

  const available = getAvailableResetCredits(
    resetCredits,
    new Date("2026-07-27T08:30:00Z").getTime(),
  );

  assert.deepEqual(available.map(({ id }) => id), ["earlier", "later", "no-expiry"]);
});

test("reset expiry includes both date and local time", () => {
  assert.equal(
    formatResetCreditDateTime("2026-07-28T08:30:00Z", {
      locale: "en-US",
      timeZone: "UTC",
    }),
    "Jul 28, 2026, 8:30 AM",
  );
  assert.equal(
    formatResetCreditDateTime("2026-07-28T08:30:00Z", {
      compact: true,
      locale: "en-US",
      timeZone: "UTC",
    }),
    "Jul 28, 8:30 AM",
  );
});

test("missing and malformed expiry values remain visible", () => {
  assert.equal(formatResetCreditDateTime(null), "No expiry");
  assert.equal(formatResetCreditDateTime("not-a-date"), "Expiry unavailable");
});

test("shows reset count even if API omits expiry rows", () => {
  const resets: AccountResetCredits = {
    available_count: 2,
    next_expires_at: "2026-12-01T00:00:00Z",
    credits: [],
  };
  assert.equal(getAvailableResetCredits(resets).length, 0);
  assert.equal(getResetCreditsDisplayCount(resets), 2);
});

test("keeps reported count without double counting known resets", () => {
  const now = new Date("2026-11-03T10:00:00Z").getTime();
  const resets: AccountResetCredits = {
    available_count: 2,
    next_expires_at: null,
    credits: [credit("known", "available", "2026-12-01T10:00:00Z")],
  };
  assert.equal(getResetCreditsDisplayCount(resets, now), 2);
  assert.equal(getResetCreditsDisplayCount({ ...resets, available_count: 0 }, now), 1);
  assert.equal(getResetCreditsDisplayCount(null, now), 0);
});

test("queues reset checks across accounts, including after a failure", async () => {
  const started: string[] = [];
  let releaseFirst: (() => void) | undefined;
  const gate = new Promise<void>((resolve) => { releaseFirst = resolve; });
  const first = queueResetCreditsCheck(async () => {
    started.push("first");
    await gate;
    throw new Error("service unavailable");
  });
  const second = queueResetCreditsCheck(async () => {
    started.push("second");
    return "ok";
  });
  await Promise.resolve();
  assert.deepEqual(started, ["first"]);
  releaseFirst?.();
  await assert.rejects(first, /service unavailable/);
  assert.equal(await second, "ok");
  assert.deepEqual(started, ["first", "second"]);
});

test("retryable busy lock is bounded but ordinary failures are not retried", async () => {
  let busy = 0;
  const result = await queueResetCreditsCheck(async () => {
    busy++;
    if (busy <= 2) throw new Error("Background OAuth refresh busy; retry next poll");
    return "ok";
  });
  assert.equal(result, "ok");
  assert.equal(busy, 3);
  let calls = 0;
  await assert.rejects(queueResetCreditsCheck(async () => {
    calls++;
    throw new Error("401 Unauthorized");
  }), /401/);
  assert.equal(calls, 1);
});
