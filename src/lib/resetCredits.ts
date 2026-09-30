import type { AccountResetCredit, AccountResetCredits } from "../types";

interface ResetCreditDateTimeFormatOptions {
  compact?: boolean;
  locale?: string;
  timeZone?: string;
}

function expiryTimestamp(credit: AccountResetCredit): number | null {
  if (!credit.expires_at) return null;
  const timestamp = new Date(credit.expires_at).getTime();
  return Number.isNaN(timestamp) ? null : timestamp;
}

export function getAvailableResetCredits(
  resetCredits: AccountResetCredits | null,
  now = Date.now(),
): AccountResetCredit[] {
  if (!resetCredits) return [];

  return resetCredits.credits
    .map((credit, index) => ({ credit, index, timestamp: expiryTimestamp(credit) }))
    .filter(({ credit, timestamp }) => {
      if (credit.status.toLowerCase() !== "available") return false;
      return timestamp === null || timestamp > now;
    })
    .sort((a, b) => {
      if (a.timestamp === null && b.timestamp === null) return a.index - b.index;
      if (a.timestamp === null) return 1;
      if (b.timestamp === null) return -1;
      return a.timestamp - b.timestamp || a.index - b.index;
    })
    .map(({ credit }) => credit);
}

export function formatResetCreditDateTime(
  expiresAt: string | null,
  options: ResetCreditDateTimeFormatOptions = {},
): string {
  if (!expiresAt) return "No expiry";

  const expiry = new Date(expiresAt);
  if (Number.isNaN(expiry.getTime())) return "Expiry unavailable";

  return new Intl.DateTimeFormat(options.locale, {
    month: "short",
    day: "numeric",
    ...(options.compact ? {} : { year: "numeric" }),
    hour: "numeric",
    minute: "2-digit",
    ...(options.timeZone ? { timeZone: options.timeZone } : {}),
  }).format(expiry);
}

export function getResetCreditsTone(
  resetCredits: AccountResetCredits | null,
  warningDays = 3,
  now = Date.now(),
): {
  container: string;
  badge: string;
  text: string;
} {
  const fallback = {
    container: "border-sky-200 bg-sky-50/70 dark:border-sky-800 dark:bg-sky-950/30",
    badge: "border-sky-200 bg-sky-100 text-sky-700 dark:border-sky-700 dark:bg-sky-900/50 dark:text-sky-300",
    text: "text-sky-700/80 dark:text-sky-300/80",
  };

  if (!resetCredits?.next_expires_at) return fallback;

  const expiry = new Date(resetCredits.next_expires_at);
  if (Number.isNaN(expiry.getTime())) return fallback;

  const remainingMs = expiry.getTime() - now;
  const dayMs = 24 * 60 * 60 * 1000;

  if (remainingMs <= warningDays * dayMs) {
    return {
      container: "border-red-200 bg-red-50/70 dark:border-red-800 dark:bg-red-950/30",
      badge: "border-red-200 bg-red-100 text-red-700 dark:border-red-700 dark:bg-red-900/50 dark:text-red-300",
      text: "text-red-700/80 dark:text-red-300/80",
    };
  }

  if (remainingMs <= (warningDays + 7) * dayMs) {
    return {
      container: "border-amber-200 bg-amber-50/70 dark:border-amber-800 dark:bg-amber-950/30",
      badge: "border-amber-200 bg-amber-100 text-amber-700 dark:border-amber-700 dark:bg-amber-900/50 dark:text-amber-300",
      text: "text-amber-700/80 dark:text-amber-300/80",
    };
  }

  return fallback;
}

