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
  const remainingPercent = Math.max(0, 100 - usedPercent);
  const colorClass =
    remainingPercent <= 10
      ? "theme-progress-fill--danger"
      : remainingPercent <= 30
        ? "theme-progress-fill--warn"
        : "theme-progress-fill--good";

  const windowLabel = formatWindowDuration(windowMinutes);
  const resetLabel = formatResetTime(resetsAt);
  const exactResetLabel = formatExactResetTime(resetsAt);

  return (
    <div className="space-y-1">
      <div className="flex justify-between text-xs text-gray-500 dark:text-slate-400">
        <span>
          {label} {windowLabel && `(${windowLabel})`}
        </span>
        <span>
          {remainingPercent.toFixed(0)}% left
          {resetLabel && ` - resets ${resetLabel}`}
          {resetLabel && exactResetLabel && ` (${exactResetLabel})`}
        </span>
      </div>
      <div className="theme-progress-track h-1.5 overflow-hidden rounded-full">
        <div
          className={`h-full transition-all duration-300 ${colorClass}`}
          style={{ width: `${Math.min(remainingPercent, 100)}%` }}
        />
      </div>
    </div>
  );
}

export function UsageBar({ usage, loading }: UsageBarProps) {
  if (loading && !usage) {
    return (
      <div className="space-y-2">
        <div className="animate-pulse text-xs italic text-gray-400 dark:text-slate-500">
          Fetching usage...
        </div>
        <div className="theme-progress-track h-1.5 animate-pulse overflow-hidden rounded-full">
          <div className="h-full w-2/3 theme-progress-fill--good opacity-50" />
        </div>
        <div className="theme-progress-track h-1.5 animate-pulse overflow-hidden rounded-full">
          <div className="h-full w-1/2 theme-progress-fill--good opacity-40" />
        </div>
      </div>
    );
  }

  if (!usage) {
    return (
      <div className="py-1 text-xs italic text-gray-400 dark:text-slate-500">
        Fetching usage...
      </div>
    );
  }

  if (usage.error) {
    return (
      <div className="py-1 text-xs italic text-gray-400 dark:text-slate-500">
        {usage.error}
      </div>
    );
  }

  const hasPrimary =
    usage.primary_used_percent !== null && usage.primary_used_percent !== undefined;
  const hasSecondary =
    usage.secondary_used_percent !== null &&
    usage.secondary_used_percent !== undefined;

  if (!hasPrimary && !hasSecondary) {
    return (
      <div className="py-1 text-xs italic text-gray-400 dark:text-slate-500">
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
        <div className="text-xs text-gray-500 dark:text-slate-400">
          Credits: {usage.credits_balance}
        </div>
      )}
    </div>
  );
}
