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
