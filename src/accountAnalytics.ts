import type { AccountWithUsage, UsageInfo } from "./types";

export type ReserveBand =
  | "unknown"
  | "depleted"
  | "critical"
  | "watch"
  | "healthy"
  | "ready";

function normalizeRemainingPercent(usedPercent: number | null | undefined): number | null {
  if (usedPercent === null || usedPercent === undefined || Number.isNaN(usedPercent)) {
    return null;
  }

  return Math.max(0, Math.min(100, 100 - usedPercent));
}

export function getUsageRemainingPercent(usage?: UsageInfo): number | null {
  if (!usage || usage.error) {
    return null;
  }

  const remainingValues = [
    normalizeRemainingPercent(usage.primary_used_percent),
    normalizeRemainingPercent(usage.secondary_used_percent),
  ].filter((value): value is number => value !== null);

  if (remainingValues.length === 0) {
    return null;
  }

  return Math.min(...remainingValues);
}

export function getUsageNextResetAt(usage?: UsageInfo): number | null {
  if (!usage || usage.error) {
    return null;
  }

  const resetTimes = [usage.primary_resets_at, usage.secondary_resets_at].filter(
    (value): value is number => value !== null && value > 0
  );

  if (resetTimes.length === 0) {
    return null;
  }

  return Math.min(...resetTimes);
}

export function getRemainingPercentBand(remainingPercent: number | null): ReserveBand {
  if (remainingPercent === null) {
    return "unknown";
  }

  if (remainingPercent <= 0) {
    return "depleted";
  }

  if (remainingPercent < 25) {
    return "critical";
  }

  if (remainingPercent < 50) {
    return "watch";
  }

  if (remainingPercent < 70) {
    return "healthy";
  }

  return "ready";
}

export function getAccountRemainingPercent(account: Pick<AccountWithUsage, "usage">): number | null {
  return getUsageRemainingPercent(account.usage);
}

export function getAccountNextResetAt(account: Pick<AccountWithUsage, "usage">): number | null {
  return getUsageNextResetAt(account.usage);
}

export function getAccountReserveBand(account: Pick<AccountWithUsage, "usage">): ReserveBand {
  return getRemainingPercentBand(getAccountRemainingPercent(account));
}
