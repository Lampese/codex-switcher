import assert from "node:assert/strict";
import test from "node:test";
import {
  formatResetCreditDateTime,
  getAvailableResetCredits,
  getResetCreditsTone,
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

test("reset credit tone highlights red according to configured warning days", () => {
  const now = new Date("2026-09-19T12:00:00Z").getTime();
  const dayMs = 24 * 60 * 60 * 1000;

  // Credit expires in 4 days
  const credits: AccountResetCredits = {
    available_count: 1,
    next_expires_at: new Date(now + 4 * dayMs).toISOString(),
    credits: [],
  };

  // With default 3 days, 4 days is amber
  const toneDefault = getResetCreditsTone(credits, 3, now);
  assert.match(toneDefault.container, /border-amber/);

  // With 5 days threshold, 4 days becomes red (urgent)
  const toneUrgent = getResetCreditsTone(credits, 5, now);
  assert.match(toneUrgent.container, /border-red/);
});

