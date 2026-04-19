import type { UsageInfo } from "../types";
import { useI18n } from "../i18n";

interface UsageBarProps {
  usage?: UsageInfo;
  loading?: boolean;
}

function formatResetTime(
  resetAt: number | null | undefined,
  t: (key: string, params?: Record<string, string | number>) => string
): string {
  if (!resetAt) return "";
  const now = Math.floor(Date.now() / 1000);
  const diff = resetAt - now;
  if (diff <= 0) return t("usage.time.now");
  if (diff < 60) return t("usage.time.seconds", { count: diff });
  if (diff < 3600) {
    return t("usage.time.minutes", { count: Math.floor(diff / 60) });
  }
  return t("usage.time.hoursMinutes", {
    hours: Math.floor(diff / 3600),
    minutes: Math.floor((diff % 3600) / 60),
  });
}

function formatExactResetTime(
  resetAt: number | null | undefined,
  locale: string
): string {
  if (!resetAt) return "";

  const date = new Date(resetAt * 1000);
  return new Intl.DateTimeFormat(locale, {
    month: "long",
    day: "numeric",
    hour: "numeric",
    minute: "2-digit",
  }).format(date);
}

function formatWindowDuration(
  minutes: number | null | undefined,
  t: (key: string, params?: Record<string, string | number>) => string
): string {
  if (!minutes) return "";
  if (minutes < 60) return t("usage.window.minutes", { count: minutes });
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return t("usage.window.hours", { count: hours });
  return t("usage.window.days", { count: Math.floor(hours / 24) });
}

function RateLimitBar({
  label,
  usedPercent,
  windowMinutes,
  resetsAt,
  locale,
  t,
}: {
  label: string;
  usedPercent: number;
  windowMinutes?: number | null;
  resetsAt?: number | null;
  locale: string;
  t: (key: string, params?: Record<string, string | number>) => string;
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

  const windowLabel = formatWindowDuration(windowMinutes, t);
  const resetLabel = formatResetTime(resetsAt, t);
  const exactResetLabel = formatExactResetTime(resetsAt, locale);

  return (
    <div className="space-y-1">
      <div className="flex justify-between text-xs text-gray-500 dark:text-gray-400">
        <span>{label} {windowLabel && `(${windowLabel})`}</span>
        <span>
          {t("usage.remaining", { percent: remainingPercent.toFixed(0) })}
          {resetLabel && ` • ${t("usage.resetsIn", { value: resetLabel })}`}
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
  const { t, locale } = useI18n();

  if (loading && !usage) {
    return (
      <div className="space-y-2">
        <div className="text-xs text-gray-400 dark:text-gray-500 italic animate-pulse">
          {t("usage.fetching")}
        </div>
        <div className="h-1.5 bg-gray-100 dark:bg-gray-800 rounded-full overflow-hidden animate-pulse">
          <div className="h-full w-2/3 bg-gray-200 dark:bg-gray-700"></div>
        </div>
        <div className="h-1.5 bg-gray-100 dark:bg-gray-800 rounded-full overflow-hidden animate-pulse">
          <div className="h-full w-1/2 bg-gray-200 dark:bg-gray-700"></div>
        </div>
      </div>
    );
  }

  if (!usage) {
    return (
      <div className="text-xs text-gray-400 dark:text-gray-500 italic py-1 animate-pulse">
        {t("usage.fetching")}
      </div>
    );
  }

  if (usage.error) {
    return (
      <div className="text-xs text-gray-400 dark:text-gray-500 italic py-1">
        {usage.error}
      </div>
    );
  }

  const hasPrimary = usage.primary_used_percent !== null && usage.primary_used_percent !== undefined;
  const hasSecondary = usage.secondary_used_percent !== null && usage.secondary_used_percent !== undefined;

  if (!hasPrimary && !hasSecondary) {
    return (
      <div className="text-xs text-gray-400 dark:text-gray-500 italic py-1">
        {t("usage.noData")}
      </div>
    );
  }

  return (
    <div className="space-y-2">
      {hasPrimary && (
        <RateLimitBar
          label={t("usage.primaryLabel")}
          usedPercent={usage.primary_used_percent!}
          windowMinutes={usage.primary_window_minutes}
          resetsAt={usage.primary_resets_at}
          locale={locale}
          t={t}
        />
      )}
      {hasSecondary && (
        <RateLimitBar
          label={t("usage.secondaryLabel")}
          usedPercent={usage.secondary_used_percent!}
          windowMinutes={usage.secondary_window_minutes}
          resetsAt={usage.secondary_resets_at}
          locale={locale}
          t={t}
        />
      )}
      {usage.credits_balance && (
        <div className="text-xs text-gray-500 dark:text-gray-400">
          {t("usage.credits", { balance: usage.credits_balance })}
        </div>
      )}
    </div>
  );
}
