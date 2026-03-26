import type { AccountWithUsage } from "../types";

export type OtherAccountsSort =
  | "deadline_asc"
  | "deadline_desc"
  | "remaining_desc"
  | "remaining_asc";

function getResetMinuteBucket(resetAt: number | null | undefined): number {
  if (resetAt === null || resetAt === undefined) {
    return Number.POSITIVE_INFINITY;
  }

  // The UI renders reset time at minute precision, so keep sorting at the same
  // granularity to avoid apparent random reshuffles caused by second-level jitter.
  return Math.floor(resetAt / 60);
}

function getRemainingPercentBucket(usedPercent: number | null | undefined): number {
  if (usedPercent === null || usedPercent === undefined) {
    return Number.NEGATIVE_INFINITY;
  }

  return Math.round(Math.max(0, 100 - usedPercent));
}

function compareResetBuckets(
  aResetBucket: number,
  bResetBucket: number,
  sort: "deadline_asc" | "deadline_desc"
): number {
  const resetDiff = aResetBucket - bResetBucket;
  if (resetDiff === 0) {
    return 0;
  }

  return sort === "deadline_asc" ? resetDiff : -resetDiff;
}

export function areOtherAccountsLoading(accounts: AccountWithUsage[]): boolean {
  return accounts.some((account) => account.usageLoading);
}

export function compareOtherAccounts(
  a: AccountWithUsage,
  b: AccountWithUsage,
  sort: OtherAccountsSort
): number {
  const aWeeklyResetBucket = getResetMinuteBucket(a.usage?.secondary_resets_at);
  const bWeeklyResetBucket = getResetMinuteBucket(b.usage?.secondary_resets_at);
  const aPrimaryResetBucket = getResetMinuteBucket(a.usage?.primary_resets_at);
  const bPrimaryResetBucket = getResetMinuteBucket(b.usage?.primary_resets_at);
  const aRemainingBucket = getRemainingPercentBucket(a.usage?.primary_used_percent);
  const bRemainingBucket = getRemainingPercentBucket(b.usage?.primary_used_percent);

  if (sort === "deadline_asc" || sort === "deadline_desc") {
    const weeklyResetDiff = compareResetBuckets(aWeeklyResetBucket, bWeeklyResetBucket, sort);
    if (weeklyResetDiff !== 0) {
      return weeklyResetDiff;
    }

    const primaryResetDiff = compareResetBuckets(aPrimaryResetBucket, bPrimaryResetBucket, sort);
    if (primaryResetDiff !== 0) {
      return primaryResetDiff;
    }

    return a.name.localeCompare(b.name);
  }

  const remainingDiff = bRemainingBucket - aRemainingBucket;
  if (sort === "remaining_desc" && remainingDiff !== 0) {
    return remainingDiff;
  }
  if (sort === "remaining_asc" && remainingDiff !== 0) {
    return -remainingDiff;
  }

  const weeklyResetDiff = aWeeklyResetBucket - bWeeklyResetBucket;
  if (weeklyResetDiff !== 0) {
    return weeklyResetDiff;
  }

  const primaryResetDiff = aPrimaryResetBucket - bPrimaryResetBucket;
  if (primaryResetDiff !== 0) {
    return primaryResetDiff;
  }

  return a.name.localeCompare(b.name);
}

export function buildOtherAccountsSortSignature(accounts: AccountWithUsage[]): string {
  return accounts
    .map((account) =>
      [
        account.id,
        account.name,
        account.is_active ? "1" : "0",
        account.usageLoading ? "1" : "0",
        getResetMinuteBucket(account.usage?.secondary_resets_at),
        getResetMinuteBucket(account.usage?.primary_resets_at),
        getRemainingPercentBucket(account.usage?.primary_used_percent),
      ].join(":")
    )
    .join("|");
}

export function getOrderedOtherAccountIds(
  accounts: AccountWithUsage[],
  sort: OtherAccountsSort
): string[] {
  return [...accounts].sort((a, b) => compareOtherAccounts(a, b, sort)).map((account) => account.id);
}
