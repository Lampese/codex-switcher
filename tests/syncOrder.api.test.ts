import assert from "node:assert/strict";
import test from "node:test";

import { getOrderedOtherAccountIds } from "../src/lib/otherAccountsOrder";
import type { AccountInfo, AccountWithUsage, UsageInfo } from "../src/types";

const backendBaseUrl = process.env.CODEX_SWITCHER_BACKEND_URL ?? "http://127.0.0.1:3211";

async function invokeCommand<T>(command: string, payload: Record<string, unknown> = {}): Promise<T> {
  const response = await fetch(`${backendBaseUrl}/api/invoke/${command}`, {
    method: "POST",
    headers: {
      "content-type": "application/json",
    },
    body: JSON.stringify(payload),
  });

  if (!response.ok) {
    const body = await response.text();
    throw new Error(`${command} failed with ${response.status}: ${body}`);
  }

  return (await response.json()) as T;
}

function summarizeOrder(accounts: AccountWithUsage[], ids: string[]): string {
  const accountMap = new Map(accounts.map((account) => [account.id, account]));

  return ids
    .map((id) => {
      const account = accountMap.get(id);
      if (!account) {
        return `${id}:missing`;
      }

      const weeklyResetMinute =
        account.usage?.secondary_resets_at === null || account.usage?.secondary_resets_at === undefined
          ? "inf"
          : Math.floor(account.usage.secondary_resets_at / 60).toString();
      const primaryResetMinute =
        account.usage?.primary_resets_at === null || account.usage?.primary_resets_at === undefined
          ? "inf"
          : Math.floor(account.usage.primary_resets_at / 60).toString();

      return `${account.name}:weekly=${weeklyResetMinute}:primary=${primaryResetMinute}`;
    })
    .join(" | ");
}

function getResetMinute(resetAt: number | null | undefined): string {
  return resetAt === null || resetAt === undefined ? "inf" : Math.floor(resetAt / 60).toString();
}

function summarizeEffectiveSortKeysByName(accounts: AccountWithUsage[]): string {
  const weeklyResetCounts = new Map<string, number>();

  for (const account of accounts) {
    const weeklyResetMinute = getResetMinute(account.usage?.secondary_resets_at);
    weeklyResetCounts.set(weeklyResetMinute, (weeklyResetCounts.get(weeklyResetMinute) ?? 0) + 1);
  }

  return [...accounts]
    .sort((a, b) => a.name.localeCompare(b.name))
    .map((account) => {
      const weeklyResetMinute = getResetMinute(account.usage?.secondary_resets_at);
      const primaryResetMinute = getResetMinute(account.usage?.primary_resets_at);
      const needsPrimaryTieBreaker = (weeklyResetCounts.get(weeklyResetMinute) ?? 0) > 1;

      return needsPrimaryTieBreaker
        ? `${account.name}:weekly=${weeklyResetMinute}:primary=${primaryResetMinute}`
        : `${account.name}:weekly=${weeklyResetMinute}`;
    })
    .join(" | ");
}

test(
  "repeated sync cycles keep other-account order stable for visible sort values",
  { timeout: 120000 },
  async (t) => {
    const health = await fetch(`${backendBaseUrl}/api/health`).catch(() => null);
    if (!health?.ok) {
      t.skip(`Backend is not reachable at ${backendBaseUrl}`);
      return;
    }

    const sampledOrders: string[][] = [];
    const sampledSummaries: string[] = [];
    const sampledFingerprints: string[] = [];

    for (let round = 0; round < 5; round += 1) {
      await invokeCommand("sync_live_auth");
      const accounts = await invokeCommand<AccountInfo[]>("list_accounts");
      const otherAccounts = accounts.filter((account) => !account.is_active);

      assert.ok(otherAccounts.length >= 2, "Need at least two non-active accounts to verify order");

      const accountsWithUsage: AccountWithUsage[] = await Promise.all(
        otherAccounts.map(async (account) => {
          const usage = await invokeCommand<UsageInfo>("get_usage", { account_id: account.id });
          return {
          ...account,
          usage,
          usageLoading: false,
          };
        })
      );

      const orderedIds = getOrderedOtherAccountIds(accountsWithUsage, "deadline_asc");
      sampledOrders.push(orderedIds);
      sampledSummaries.push(`round ${round + 1}: ${summarizeOrder(accountsWithUsage, orderedIds)}`);
      sampledFingerprints.push(summarizeEffectiveSortKeysByName(accountsWithUsage));
    }

    const baselineOrder = sampledOrders[0];
    const baselineFingerprint = sampledFingerprints[0];
    let comparedRounds = 1;

    for (let round = 1; round < sampledOrders.length; round += 1) {
      if (sampledFingerprints[round] !== baselineFingerprint) {
        continue;
      }

      comparedRounds += 1;
      assert.deepEqual(sampledOrders[round], baselineOrder, sampledSummaries.join("\n"));
    }

    assert.ok(comparedRounds >= 2, sampledSummaries.join("\n"));
  }
);
