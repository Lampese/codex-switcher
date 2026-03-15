import type { UsageInfo } from "../types";

interface UsageBarProps {
  usage?: UsageInfo;
  loading?: boolean;
}

function formatResetTime(resetAt: number | null | undefined): string {
  if (!resetAt) return "";
  const now = Math.floor(Date.now() / 1000);
  const diff = resetAt - now;
  if (diff <= 0) return "现在";
  if (diff < 60) return `${diff} 秒`;
  if (diff < 3600) return `${Math.floor(diff / 60)} 分钟`;
  return `${Math.floor(diff / 3600)} 小时 ${Math.floor((diff % 3600) / 60)} 分钟`;
}

function formatExactResetTime(resetAt: number | null | undefined): string {
  if (!resetAt) return "";

  const date = new Date(resetAt * 1000);
  return new Intl.DateTimeFormat("zh-CN", {
    month: "long",
    day: "numeric",
    hour: "numeric",
    minute: "2-digit",
    hour12: false,
  }).format(date);
}

function formatWindowDuration(minutes: number | null | undefined): string {
  if (!minutes) return "";
  if (minutes < 60) return `${minutes} 分钟`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours} 小时`;
  return `${Math.floor(hours / 24)} 天`;
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
      <div className="flex justify-between text-xs text-gray-500">
        <span>{label} {windowLabel && `(${windowLabel})`}</span>
        <span>
          剩余 {remainingPercent.toFixed(0)}%
          {resetLabel && ` • ${resetLabel}后重置`}
          {resetLabel && exactResetLabel && ` (${exactResetLabel})`}
        </span>
      </div>
      <div className="h-1.5 bg-gray-100 rounded-full overflow-hidden">
        <div
          className={`h-full transition-all duration-300 ${colorClass}`}
          style={{ width: `${Math.min(remainingPercent, 100)}%` }}
        ></div>
      </div>
    </div>
  );
}

export function UsageBar({ usage, loading }: UsageBarProps) {
  if (loading) {
    return (
      <div className="space-y-2">
        <div className="h-1.5 bg-gray-100 rounded-full overflow-hidden animate-pulse">
          <div className="h-full w-2/3 bg-gray-200"></div>
        </div>
        <div className="h-1.5 bg-gray-100 rounded-full overflow-hidden animate-pulse">
          <div className="h-full w-1/2 bg-gray-200"></div>
        </div>
      </div>
    );
  }

  if (!usage || usage.error) {
    return (
      <div className="text-xs text-gray-400 italic py-1">
        {usage?.error || "暂无配额数据"}
      </div>
    );
  }

  const hasPrimary = usage.primary_used_percent !== null && usage.primary_used_percent !== undefined;
  const hasSecondary = usage.secondary_used_percent !== null && usage.secondary_used_percent !== undefined;

  if (!hasPrimary && !hasSecondary) {
    return (
      <div className="text-xs text-gray-400 italic py-1">
        暂无限额信息
      </div>
    );
  }

  return (
    <div className="space-y-2">
      {hasPrimary && (
        <RateLimitBar
          label="5 小时配额"
          usedPercent={usage.primary_used_percent!}
          windowMinutes={usage.primary_window_minutes}
          resetsAt={usage.primary_resets_at}
        />
      )}
      {hasSecondary && (
        <RateLimitBar
          label="周配额"
          usedPercent={usage.secondary_used_percent!}
          windowMinutes={usage.secondary_window_minutes}
          resetsAt={usage.secondary_resets_at}
        />
      )}
      {usage.credits_balance && (
        <div className="text-xs text-gray-500">
          额度余额：{usage.credits_balance}
        </div>
      )}
    </div>
  );
}
