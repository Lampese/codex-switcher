import assert from "node:assert/strict";
import test from "node:test";
import {
  formatResetCreditDateTime,
  getAvailableResetCredits,
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
  assert.equal(
    formatResetCreditDateTime("2026-07-28T08:30:00Z", {
      locale: "zh-CN",
      timeZone: "UTC",
    }),
    "2026年7月28日 8:30",
  );
});

test("missing and malformed expiry values remain visible", () => {
  assert.equal(formatResetCreditDateTime(null), "No expiry");
  assert.equal(formatResetCreditDateTime("not-a-date"), "Expiry unavailable");
});
