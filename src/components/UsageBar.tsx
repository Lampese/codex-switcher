import type { UsageInfo } from "../types";

interface UsageBarProps {
  usage?: UsageInfo;
  loading?: boolean;
}

function formatResetTime(resetAt: number | null | undefined): string {
  if (!resetAt) return "";
  const diff = resetAt - Math.floor(Date.now() / 1000);
  if (diff <= 0) return "now";
  if (diff < 60) return `${diff}s`;

  const totalMinutes = Math.floor(diff / 60);
  if (totalMinutes < 60) return `${totalMinutes}m`;

  const totalHours = Math.floor(totalMinutes / 60);
  const minutes = totalMinutes % 60;
  if (totalHours < 24) return `${totalHours}h ${minutes}m`;

  const days = Math.floor(totalHours / 24);
  const hours = totalHours % 24;
  return `${days}d ${hours}h ${minutes}m`;
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
  stale = false,
}: {
  label: string;
  usedPercent: number;
  windowMinutes?: number | null;
  resetsAt?: number | null;
  stale?: boolean;
}) {
  const remainingPercent = Math.max(0, 100 - usedPercent);
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
      <div className="flex justify-between text-xs text-gray-500">
        <span>
          {label} {windowLabel && `(${windowLabel})`}
        </span>
        <span>
          {remainingPercent.toFixed(0)}% left
          {resetLabel && ` - resets ${resetLabel}`}
          {resetLabel && exactResetLabel && ` (${exactResetLabel})`}
        </span>
      </div>
      <div className="h-1.5 overflow-hidden rounded-full bg-gray-100">
        <div
          className="relative h-full overflow-hidden transition-all duration-300"
          style={{ width: `${Math.min(remainingPercent, 100)}%` }}
        >
          <div
            className={`h-full transition-all duration-300 ${colorClass} ${
              stale ? "usage-stale-fill" : ""
            }`}
          />
          {stale && <div className="usage-stale-sheen absolute inset-0" aria-hidden="true" />}
        </div>
      </div>
    </div>
  );
}

export function UsageBar({ usage, loading }: UsageBarProps) {
  if (loading && !usage) {
    return (
      <div className="space-y-2">
        <div className="animate-pulse text-xs italic text-gray-400">Fetching usage...</div>
        <div className="h-1.5 animate-pulse overflow-hidden rounded-full bg-gray-100">
          <div className="h-full w-2/3 bg-gray-200" />
        </div>
        <div className="h-1.5 animate-pulse overflow-hidden rounded-full bg-gray-100">
          <div className="h-full w-1/2 bg-gray-200" />
        </div>
      </div>
    );
  }

  if (!usage) {
    return <div className="py-1 text-xs italic text-gray-400">Fetching usage...</div>;
  }

  if (usage.error) {
    return <div className="py-1 text-xs italic text-gray-400">{usage.error}</div>;
  }

  const hasPrimary =
    usage.primary_used_percent !== null && usage.primary_used_percent !== undefined;
  const hasSecondary =
    usage.secondary_used_percent !== null && usage.secondary_used_percent !== undefined;

  if (!hasPrimary && !hasSecondary) {
    return <div className="py-1 text-xs italic text-gray-400">No rate limit data</div>;
  }

  const showRefreshingState = loading && !usage.error;

  return (
    <div className="space-y-2">
      {showRefreshingState && (
        <div className="text-xs italic text-gray-400">Refreshing usage...</div>
      )}
      {hasPrimary && (
        <RateLimitBar
          label="5h Limit"
          usedPercent={usage.primary_used_percent!}
          windowMinutes={usage.primary_window_minutes}
          resetsAt={usage.primary_resets_at}
          stale={showRefreshingState}
        />
      )}
      {hasSecondary && (
        <RateLimitBar
          label="Weekly Limit"
          usedPercent={usage.secondary_used_percent!}
          windowMinutes={usage.secondary_window_minutes}
          resetsAt={usage.secondary_resets_at}
          stale={showRefreshingState}
        />
      )}
      {usage.credits_balance && (
        <div className="text-xs text-gray-500">Credits: {usage.credits_balance}</div>
      )}
    </div>
  );
}
