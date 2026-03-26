import assert from "node:assert/strict";
import test from "node:test";

import {
  areOtherAccountsLoading,
  buildOtherAccountsSortSignature,
  getOrderedOtherAccountIds,
} from "../src/lib/otherAccountsOrder";
import type { AccountWithUsage } from "../src/types";

function makeAccount(
  name: string,
  options: {
    id?: string;
    usageLoading?: boolean;
    primaryResetsAt?: number | null;
    primaryUsedPercent?: number | null;
    secondaryResetsAt?: number | null;
  } = {}
): AccountWithUsage {
  return {
    id: options.id ?? name,
    name,
    email: `${name}@example.com`,
    plan_type: "plus",
    auth_mode: "chat_gpt",
    is_active: false,
    created_at: "2026-03-22T00:00:00.000Z",
    last_used_at: null,
    usageLoading: options.usageLoading ?? false,
    usage: {
      account_id: options.id ?? name,
      plan_type: "plus",
      primary_used_percent: options.primaryUsedPercent ?? 0,
      primary_window_minutes: 300,
      primary_resets_at: options.primaryResetsAt ?? null,
      secondary_used_percent: 0,
      secondary_window_minutes: 10080,
      secondary_resets_at: options.secondaryResetsAt ?? null,
      has_credits: false,
      unlimited_credits: false,
      credits_balance: "0",
      error: null,
    },
  };
}

test("minute-level signature ignores second jitter in weekly and 5h reset timestamps", () => {
  const firstSample = [
    makeAccount("jeanna", {
      primaryResetsAt: 1774180558,
      primaryUsedPercent: 0,
      secondaryResetsAt: 1774580558,
    }),
    makeAccount("miller", {
      primaryResetsAt: 1774180557,
      primaryUsedPercent: 0,
      secondaryResetsAt: 1774580557,
    }),
    makeAccount("sharen", {
      primaryResetsAt: 1774180556,
      primaryUsedPercent: 0,
      secondaryResetsAt: 1774580556,
    }),
  ];
  const secondSample = [
    makeAccount("jeanna", {
      primaryResetsAt: 1774180559,
      primaryUsedPercent: 0,
      secondaryResetsAt: 1774580559,
    }),
    makeAccount("miller", {
      primaryResetsAt: 1774180558,
      primaryUsedPercent: 0,
      secondaryResetsAt: 1774580558,
    }),
    makeAccount("sharen", {
      primaryResetsAt: 1774180557,
      primaryUsedPercent: 0,
      secondaryResetsAt: 1774580557,
    }),
  ];

  assert.equal(
    buildOtherAccountsSortSignature(firstSample),
    buildOtherAccountsSortSignature(secondSample)
  );
  assert.deepEqual(
    getOrderedOtherAccountIds(firstSample, "deadline_asc"),
    getOrderedOtherAccountIds(secondSample, "deadline_asc")
  );
});

test("loading flag can freeze reordering until refresh completes", () => {
  const accounts = [
    makeAccount("jeanna", {
      usageLoading: true,
      primaryResetsAt: 1774180559,
      secondaryResetsAt: 1774580559,
    }),
    makeAccount("miller", {
      primaryResetsAt: 1774180557,
      secondaryResetsAt: 1774580557,
    }),
  ];

  assert.equal(areOtherAccountsLoading(accounts), true);
});

test("deadline sort prioritizes weekly reset before 5h reset", () => {
  const accounts = [
    makeAccount("sharen", {
      primaryResetsAt: 1774180200,
      secondaryResetsAt: 1774581200,
    }),
    makeAccount("jeanna", {
      primaryResetsAt: 1774182000,
      secondaryResetsAt: 1774580600,
    }),
    makeAccount("miller", {
      primaryResetsAt: 1774180100,
      secondaryResetsAt: 1774580900,
    }),
  ];

  assert.deepEqual(getOrderedOtherAccountIds(accounts, "deadline_asc"), [
    "jeanna",
    "miller",
    "sharen",
  ]);
});

test("deadline sort uses 5h reset when weekly reset matches", () => {
  const accounts = [
    makeAccount("sharen", {
      primaryResetsAt: 1774181200,
      secondaryResetsAt: 1774580600,
    }),
    makeAccount("jeanna", {
      primaryResetsAt: 1774180300,
      secondaryResetsAt: 1774580600,
    }),
    makeAccount("miller", {
      primaryResetsAt: 1774180900,
      secondaryResetsAt: 1774580600,
    }),
  ];

  assert.deepEqual(getOrderedOtherAccountIds(accounts, "deadline_asc"), [
    "jeanna",
    "miller",
    "sharen",
  ]);
});

test("deadline sort falls back to account name when both reset windows match", () => {
  const tiedAccounts = [
    makeAccount("sharen", {
      primaryResetsAt: 1774180556,
      secondaryResetsAt: 1774580556,
    }),
    makeAccount("jeanna", {
      primaryResetsAt: 1774180558,
      secondaryResetsAt: 1774580558,
    }),
    makeAccount("miller", {
      primaryResetsAt: 1774180557,
      secondaryResetsAt: 1774580557,
    }),
  ];

  assert.deepEqual(getOrderedOtherAccountIds(tiedAccounts, "deadline_asc"), [
    "jeanna",
    "miller",
    "sharen",
  ]);
});
