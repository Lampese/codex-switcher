import type { UsageInfo } from "../types";

interface UsageBarProps {
  usage?: UsageInfo;
  loading?: boolean;
}

function formatResetTime(resetAt: number | null | undefined): string {
  if (!resetAt) return "";
  const now = Math.floor(Date.now() / 1000);
  const diff = resetAt - now;
  if (diff <= 0) return "now";
  if (diff < 60) return `${diff}s`;
  if (diff < 3600) return `${Math.floor(diff / 60)}m`;
  return `${Math.floor(diff / 3600)}h ${Math.floor((diff % 3600) / 60)}m`;
}

function formatExactResetTime(resetAt: number | null | undefined): string {
  if (!resetAt) return "";

  const date = new Date(resetAt * 1000);
  const month = new Intl.DateTimeFormat(undefined, { month: "long" }).format(date);
  const day = date.getDate();
  const minutes = String(date.getMinutes()).padStart(2, "0");
  const period = date.getHours() >= 12 ? "PM" : "AM";
  const hour12 = date.getHours() % 12 || 12;

  return `${month} ${day}, ${hour12}:${minutes} ${period}`;
}

function formatWindowDuration(minutes: number | null | undefined): string {
  if (!minutes) return "";
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h`;
  return `${Math.floor(hours / 24)}d`;
}

function formatUsageError(error: string): { title: string; message: string } {
  if (
    error.includes("refresh_token_invalidated") ||
    error.includes("saved session is out of date")
  ) {
    return {
      title: "Session expired",
      message: "Re-authenticate this account.",
    };
  }

  if (error.includes("refresh_token_reused") || error.includes("outdated refresh token")) {
    return {
      title: "Session expired",
      message: "Re-authenticate this account.",
    };
  }

  // Older builds may still surface a raw backend JSON payload. Never dump that
  // into the account card; keep the UI concise while logs retain the details.
  if (error.includes("Token refresh failed:") && error.includes('"error"')) {
    return {
      title: "Couldn’t refresh Switcher session",
      message:
        "The saved Switcher credentials could not be refreshed. Your Codex session may still be valid. Retry usage before re-authenticating.",
    };
  }

  return {
    title: "Usage unavailable",
    message: error,
  };
}

function RateLimitBar({
  label,
  usedPercent,
  windowMinutes,
  resetsAt,
}: {
  label: string;
  usedPercent: number;
  windowMinutes?: number | null;
  resetsAt?: number | null;
}) {
  // Calculate remaining percentage
  const remainingPercent = Math.max(0, 100 - usedPercent);

  // Color based on remaining (green = plenty left, red = almost none left)
  const colorClass =
    remainingPercent <= 10
      ? "bg-red-500"
      : remainingPercent <= 30
        ? "bg-amber-500"
        : "bg-emerald-500";

  const windowLabel = formatWindowDuration(windowMinutes);
  const resetLabel = formatResetTime(resetsAt);
  const exactResetLabel = formatExactResetTime(resetsAt);

  return (
    <div className="space-y-1">
      <div className="flex justify-between text-xs text-gray-500 dark:text-gray-400">
        <span>{windowLabel ? `${windowLabel} limit` : label}</span>
        <span>
          {remainingPercent.toFixed(0)}% left
          {resetLabel && ` • resets ${resetLabel}`}
          {resetLabel && exactResetLabel && ` (${exactResetLabel})`}
        </span>
      </div>
      <div className="h-1.5 bg-gray-100 dark:bg-gray-800 rounded-full overflow-hidden">
        <div
          className={`h-full transition-all duration-300 ${colorClass}`}
          style={{ width: `${Math.min(remainingPercent, 100)}%` }}
        ></div>
      </div>
    </div>
  );
}

export function UsageBar({ usage, loading }: UsageBarProps) {
  if (loading && !usage) {
    return (
      <div className="space-y-2">
        <div className="text-xs text-gray-400 dark:text-gray-500 italic animate-pulse">
          Fetching usage...
        </div>
        <div className="h-1.5 bg-gray-100 dark:bg-gray-800 rounded-full overflow-hidden animate-pulse">
          <div className="h-full w-2/3 bg-gray-200 dark:bg-gray-700"></div>
        </div>
      </div>
    );
  }

  if (!usage) {
    return (
      <div className="text-xs text-gray-400 dark:text-gray-500 italic py-1 animate-pulse">
        Fetching usage...
      </div>
    );
  }

  if (usage.error) {
    const friendlyError = formatUsageError(usage.error);
    return (
      <div className="rounded-lg border border-amber-200 bg-amber-50/70 px-3 py-2 dark:border-amber-800/70 dark:bg-amber-950/20">
        <div className="flex items-start gap-2">
          <span
            className="mt-0.5 inline-flex h-4 w-4 shrink-0 items-center justify-center rounded-full bg-amber-100 text-[10px] font-bold text-amber-700 dark:bg-amber-900/60 dark:text-amber-300"
            aria-hidden="true"
          >
            !
          </span>
          <div className="min-w-0">
            <div className="text-xs font-medium text-amber-800 dark:text-amber-200">
              {friendlyError.title}
            </div>
            <div className="mt-0.5 text-xs leading-relaxed text-amber-700 dark:text-amber-300/90">
              {friendlyError.message}
            </div>
          </div>
        </div>
      </div>
    );
  }

  const hasPrimary = usage.primary_used_percent !== null && usage.primary_used_percent !== undefined;
  const hasSecondary = usage.secondary_used_percent !== null && usage.secondary_used_percent !== undefined;

  if (!hasPrimary && !hasSecondary) {
    return (
      <div className="text-xs text-gray-400 dark:text-gray-500 italic py-1">
        No rate limit data
      </div>
    );
  }

  return (
    <div className="space-y-2">
      {hasPrimary && (
        <RateLimitBar
          label="5h Limit"
          usedPercent={usage.primary_used_percent!}
          windowMinutes={usage.primary_window_minutes}
          resetsAt={usage.primary_resets_at}
        />
      )}
      {hasSecondary && (
        <RateLimitBar
          label="Weekly Limit"
          usedPercent={usage.secondary_used_percent!}
          windowMinutes={usage.secondary_window_minutes}
          resetsAt={usage.secondary_resets_at}
        />
      )}
      {usage.credits_balance && (
        <div className="text-xs text-gray-500 dark:text-gray-400">
          Credits: {usage.credits_balance}
        </div>
      )}
    </div>
  );
}
