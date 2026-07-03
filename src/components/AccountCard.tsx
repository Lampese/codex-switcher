import { useCallback, useState, useRef, useEffect } from "react";
import { useTranslation } from "react-i18next";
import type { TFunction } from "i18next";
import type { AccountResetCredits, AccountUsageStats as AccountUsageStatsInfo, AccountWithUsage } from "../types";
import { invokeBackend } from "../lib/platform";
import { AccountUsageStats } from "./AccountUsageStats";
import { UsageBar } from "./UsageBar";

const RESET_CREDITS_REFRESH_INTERVAL_MS = 6 * 60 * 60 * 1000;

interface AccountCardProps {
  account: AccountWithUsage;
  onSwitch: () => void;
  onWarmup: () => Promise<void>;
  onDelete: () => void;
  onRefresh: () => Promise<unknown>;
  onRename: (newName: string) => Promise<void>;
  switching?: boolean;
  switchDisabled?: boolean;
  warmingUp?: boolean;
  masked?: boolean;
  onToggleMask?: () => void;
  autoWarmupEnabled?: boolean;
  autoWarmupManagedByAll?: boolean;
  autoWarmupLabel?: string;
  onToggleAutoWarmup?: () => void;
}

function formatLastRefresh(date: Date | null, t: TFunction, locale: string): string {
  if (!date) return t("accountCard.never");
  const now = new Date();
  const diff = Math.floor((now.getTime() - date.getTime()) / 1000);
  if (diff < 5) return t("accountCard.justNow");
  if (diff < 60) return t("accountCard.secondsAgo", { count: diff });
  if (diff < 3600) return t("accountCard.minutesAgo", { count: Math.floor(diff / 60) });
  if (diff < 86400) return t("accountCard.hoursAgo", { count: Math.floor(diff / 3600) });
  return date.toLocaleDateString(locale);
}

function getSubscriptionStatus(timestamp: string | null | undefined, t: TFunction, locale: string): {
  label: string;
  className: string;
} {
  if (!timestamp) {
    return {
      label: t("accountCard.expiryUnavailable"),
      className: "text-gray-400 dark:text-gray-500",
    };
  }

  const expiryDate = new Date(timestamp);
  const formattedDate = new Intl.DateTimeFormat(locale, {
    month: "short",
    day: "numeric",
    year: "numeric",
  }).format(expiryDate);

  const remainingMs = expiryDate.getTime() - Date.now();
  if (remainingMs <= 0) {
    return {
      label: t("accountCard.expired", { date: formattedDate }),
      className: "text-red-500 dark:text-red-400",
    };
  }

  if (remainingMs <= 3 * 24 * 60 * 60 * 1000) {
    return {
      label: t("accountCard.until", { date: formattedDate }),
      className: "text-red-500 dark:text-red-400",
    };
  }

  if (remainingMs <= 7 * 24 * 60 * 60 * 1000) {
    return {
      label: t("accountCard.until", { date: formattedDate }),
      className: "text-amber-500 dark:text-amber-400",
    };
  }

  return {
    label: t("accountCard.until", { date: formattedDate }),
    className: "text-gray-400 dark:text-gray-500",
  };
}

function formatResetCreditsCount(resetCredits: AccountResetCredits | null, t: TFunction): string | null {
  if (!resetCredits) return null;
  const count = resetCredits.available_count;
  if (count <= 0) return null;
  return t("accountCard.resetCount", { count });
}

function formatResetCreditsExpiry(
  resetCredits: AccountResetCredits | null,
  t: TFunction,
  locale: string,
  compact = false,
): string | null {
  if (!resetCredits?.next_expires_at) return null;

  const expiry = new Date(resetCredits.next_expires_at);
  if (Number.isNaN(expiry.getTime())) return null;

  const formattedDate = new Intl.DateTimeFormat(locale, {
    month: "short",
    day: "numeric",
    ...(compact ? {} : { year: "numeric" }),
  }).format(expiry);

  return compact
    ? t("accountCard.closest", { date: formattedDate })
    : t("accountCard.closestExpires", { date: formattedDate });
}

function getResetCreditsTone(resetCredits: AccountResetCredits | null): {
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

  const remainingMs = expiry.getTime() - Date.now();
  const dayMs = 24 * 60 * 60 * 1000;

  if (remainingMs <= 3 * dayMs) {
    return {
      container: "border-red-200 bg-red-50/70 dark:border-red-800 dark:bg-red-950/30",
      badge: "border-red-200 bg-red-100 text-red-700 dark:border-red-700 dark:bg-red-900/50 dark:text-red-300",
      text: "text-red-700/80 dark:text-red-300/80",
    };
  }

  if (remainingMs <= 10 * dayMs) {
    return {
      container: "border-amber-200 bg-amber-50/70 dark:border-amber-800 dark:bg-amber-950/30",
      badge: "border-amber-200 bg-amber-100 text-amber-700 dark:border-amber-700 dark:bg-amber-900/50 dark:text-amber-300",
      text: "text-amber-700/80 dark:text-amber-300/80",
    };
  }

  return fallback;
}

function BlurredText({ children, blur }: { children: React.ReactNode; blur: boolean }) {
  return (
    <span
      className={`transition-all duration-200 select-none ${blur ? "blur-sm" : ""}`}
      style={blur ? { userSelect: "none" } : undefined}
    >
      {children}
    </span>
  );
}

export function AccountCard({
  account,
  onSwitch,
  onWarmup,
  onDelete,
  onRefresh,
  onRename,
  switching,
  switchDisabled,
  warmingUp,
  masked = false,
  onToggleMask,
  autoWarmupEnabled = false,
  autoWarmupManagedByAll = false,
  autoWarmupLabel,
  onToggleAutoWarmup,
}: AccountCardProps) {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? "en-US";
  const [isRefreshing, setIsRefreshing] = useState(false);
  const [lastRefresh, setLastRefresh] = useState<Date | null>(
    account.usage && !account.usage.error ? new Date() : null
  );
  const [isEditing, setIsEditing] = useState(false);
  const [editName, setEditName] = useState(account.name);
  const [resetCredits, setResetCredits] = useState<AccountResetCredits | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const resetRequestSeq = useRef(0);

  useEffect(() => {
    if (isEditing && inputRef.current) {
      inputRef.current.focus();
      inputRef.current.select();
    }
  }, [isEditing]);

  const handleRefresh = async () => {
    setIsRefreshing(true);
    try {
      await onRefresh();
      setLastRefresh(new Date());
    } finally {
      setIsRefreshing(false);
    }
  };

  const handleRename = async () => {
    const trimmed = editName.trim();
    if (trimmed && trimmed !== account.name) {
      try {
        await onRename(trimmed);
      } catch {
        setEditName(account.name);
      }
    } else {
      setEditName(account.name);
    }
    setIsEditing(false);
  };

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === "Enter") {
      handleRename();
    } else if (e.key === "Escape") {
      setEditName(account.name);
      setIsEditing(false);
    }
  };

  const planDisplay = account.plan_type
    ? account.plan_type.charAt(0).toUpperCase() + account.plan_type.slice(1)
    : account.auth_mode === "api_key"
      ? t("accountCard.apiKey")
      : t("common.unknown");

  const planColors: Record<string, string> = {
    pro: "bg-indigo-50 text-indigo-700 border-indigo-200 dark:bg-indigo-900/30 dark:text-indigo-300 dark:border-indigo-700",
    plus: "bg-emerald-50 text-emerald-700 border-emerald-200 dark:bg-emerald-900/30 dark:text-emerald-300 dark:border-emerald-700",
    team: "bg-blue-50 text-blue-700 border-blue-200 dark:bg-blue-900/30 dark:text-blue-300 dark:border-blue-700",
    enterprise: "bg-amber-50 text-amber-700 border-amber-200 dark:bg-amber-900/30 dark:text-amber-300 dark:border-amber-700",
    free: "bg-gray-50 text-gray-600 border-gray-200 dark:bg-gray-800 dark:text-gray-300 dark:border-gray-700",
    api_key: "bg-orange-50 text-orange-700 border-orange-200 dark:bg-orange-900/30 dark:text-orange-300 dark:border-orange-700",
  };

  const planKey = account.plan_type?.toLowerCase() || "api_key";
  const planColorClass = planColors[planKey] || planColors.free;
  const showSubscriptionStatus = account.auth_mode === "chat_g_p_t";
  const subscriptionStatus = getSubscriptionStatus(account.subscription_expires_at, t, locale);
  const resetCreditsCount = formatResetCreditsCount(resetCredits, t);
  const compactResetCredits = !account.is_active;
  const resetCreditsExpiry = formatResetCreditsExpiry(resetCredits, t, locale, compactResetCredits);
  const resetCreditsTone = getResetCreditsTone(resetCredits);

  const loadResetCredits = useCallback(async () => {
    const requestId = ++resetRequestSeq.current;

    if (account.auth_mode !== "chat_g_p_t") {
      setResetCredits(null);
      return;
    }

    try {
      const stats = await invokeBackend<AccountUsageStatsInfo>("get_account_usage_stats", {
        accountId: account.id,
      });
      if (requestId !== resetRequestSeq.current) return;
      setResetCredits(stats.account_id === account.id ? stats.reset_credits : null);
    } catch {
      if (requestId !== resetRequestSeq.current) return;
      setResetCredits(null);
    }
  }, [account.auth_mode, account.id]);

  const handleStatsLoaded = useCallback(
    (stats: AccountUsageStatsInfo | null) => {
      setResetCredits(stats?.account_id === account.id ? stats.reset_credits : null);
    },
    [account.id]
  );

  useEffect(() => {
    setResetCredits(null);

    void loadResetCredits();
    const timer = window.setInterval(() => {
      void loadResetCredits();
    }, RESET_CREDITS_REFRESH_INTERVAL_MS);

    return () => {
      resetRequestSeq.current += 1;
      window.clearInterval(timer);
    };
  }, [loadResetCredits]);


  return (
    <div
      className={`relative rounded-xl border p-5 transition-all duration-200 ${
        account.is_active
          ? "bg-white dark:bg-gray-900 border-emerald-400 shadow-sm"
          : "bg-white dark:bg-gray-900 border-gray-200 dark:border-gray-700 hover:border-gray-300 dark:hover:border-gray-600"
      }`}
    >
      {/* Header */}
      <div className="flex items-start justify-between mb-3">
        <div className="flex-1 min-w-0">
          <div className="flex items-center gap-2 mb-1">
            {account.is_active && (
              <span className="flex h-2 w-2">
                <span className="animate-ping absolute inline-flex h-2 w-2 rounded-full bg-green-400 opacity-75"></span>
                <span className="relative inline-flex rounded-full h-2 w-2 bg-green-500"></span>
              </span>
            )}
            {isEditing ? (
              <input
                ref={inputRef}
                type="text"
                value={editName}
                onChange={(e) => setEditName(e.target.value)}
                onBlur={handleRename}
                onKeyDown={handleKeyDown}
                className="font-semibold text-gray-900 dark:text-gray-100 bg-gray-100 dark:bg-gray-800 px-2 py-0.5 rounded border border-gray-300 dark:border-gray-700 focus:outline-none focus:border-gray-500 dark:focus:border-gray-500 w-full"
              />
            ) : (
              <h3
                className="font-semibold text-gray-900 dark:text-gray-100 truncate cursor-pointer hover:text-gray-600 dark:hover:text-gray-300"
                onClick={() => {
                  if (masked) return;
                  setEditName(account.name);
                  setIsEditing(true);
                }}
                title={masked ? undefined : t("accountCard.rename")}
              >
                <BlurredText blur={masked}>{account.name}</BlurredText>
              </h3>
            )}
          </div>
          {account.email && (
            <p className="text-sm text-gray-500 dark:text-gray-400 truncate">
              <BlurredText blur={masked}>{account.email}</BlurredText>
            </p>
          )}
        </div>

        <div className="flex max-w-[60%] flex-wrap items-center justify-end gap-2">
          {/* Eye toggle */}
          {onToggleMask && (
            <button
              onClick={onToggleMask}
              className="p-1 text-gray-400 dark:text-gray-500 hover:text-gray-600 dark:hover:text-gray-300 transition-colors"
              title={masked ? t("accountCard.showInfo") : t("accountCard.hideInfo")}
            >
              {masked ? (
                <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M13.875 18.825A10.05 10.05 0 0112 19c-4.478 0-8.268-2.943-9.543-7a9.97 9.97 0 011.563-3.029m5.858.908a3 3 0 114.243 4.243M9.878 9.878l4.242 4.242M9.88 9.88l-3.29-3.29m7.532 7.532l3.29 3.29M3 3l3.59 3.59m0 0A9.953 9.953 0 0112 5c4.478 0 8.268 2.943 9.543 7a10.025 10.025 0 01-4.132 5.411m0 0L21 21" />
                </svg>
              ) : (
                <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M15 12a3 3 0 11-6 0 3 3 0 016 0z" />
                  <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M2.458 12C3.732 7.943 7.523 5 12 5c4.478 0 8.268 2.943 9.542 7-1.274 4.057-5.064 7-9.542 7-4.477 0-8.268-2.943-9.542-7z" />
                </svg>
              )}
            </button>
          )}
          {/* Plan badge */}
          <span
            className={`px-2.5 py-1 text-xs font-medium rounded-full border ${planColorClass}`}
          >
            {planDisplay}
          </span>
          {resetCreditsCount && compactResetCredits && (
            <div
              className={`flex min-w-0 max-w-full items-center gap-1.5 rounded-full border px-2 py-1 text-[11px] leading-none ${resetCreditsTone.container} ${resetCreditsTone.text}`}
              title={[resetCreditsCount, resetCreditsExpiry].filter(Boolean).join(" · ")}
            >
              <span className="shrink-0 whitespace-nowrap font-semibold">
                {resetCreditsCount}
              </span>
              {resetCreditsExpiry && (
                <span className="truncate">
                  · {resetCreditsExpiry}
                </span>
              )}
            </div>
          )}
          {resetCreditsCount && !compactResetCredits && (
            <div
              className={`flex max-w-full items-center gap-2 rounded-lg border px-2 py-1.5 text-xs ${resetCreditsTone.container}`}
            >
              <span className={`whitespace-nowrap rounded-full border px-2.5 py-0.5 font-medium ${resetCreditsTone.badge}`}>
                {resetCreditsCount}
              </span>
              {resetCreditsExpiry && (
                <span className={`truncate ${resetCreditsTone.text}`}>
                  {resetCreditsExpiry}
                </span>
              )}
            </div>
          )}
        </div>
      </div>

      {/* Usage */}
      <div className="mb-3">
        <UsageBar usage={account.usage} loading={isRefreshing || account.usageLoading} />
      </div>

      {/* Last refresh time */}
      <div className="flex flex-wrap items-center justify-between gap-2 text-xs mb-3">
        <div className="text-gray-400 dark:text-gray-500">
          {t("accountCard.lastUpdated", { time: formatLastRefresh(lastRefresh, t, locale) })}
        </div>
        {showSubscriptionStatus && (
          <div className={`text-right ${subscriptionStatus.className}`}>
            {subscriptionStatus.label}
          </div>
        )}
      </div>

      <AccountUsageStats
        accountId={account.id}
        enabled={account.auth_mode === "chat_g_p_t"}
        defaultOpen={account.is_active}
        onStatsLoaded={handleStatsLoaded}
      />

      {/* Actions */}
      <div className="flex gap-2 mt-3">
        {account.is_active ? (
          <button
            disabled
            className="flex-1 px-4 py-2 text-sm font-medium rounded-lg bg-gray-100 dark:bg-gray-800 text-gray-500 dark:text-gray-400 border border-gray-200 dark:border-gray-700 cursor-default"
          >
            ✓ {t("accountCard.active")}
          </button>
        ) : (
          <button
            onClick={onSwitch}
            disabled={switching || switchDisabled}
            className={`flex-1 px-4 py-2 text-sm font-medium rounded-lg transition-colors disabled:opacity-50 ${
              switchDisabled
                ? "bg-gray-200 dark:bg-gray-800 text-gray-400 dark:text-gray-500 cursor-not-allowed"
                : "bg-gray-900 hover:bg-gray-800 dark:bg-gray-100 dark:hover:bg-gray-200 text-white dark:text-gray-900"
            }`}
            title={switchDisabled ? t("accountCard.closeProcesses") : undefined}
          >
            {switching ? t("accountCard.switching") : switchDisabled ? t("accountCard.codexRunning") : t("accountCard.switch")}
          </button>
        )}
        <button
          onClick={() => {
            void onWarmup();
          }}
          disabled={warmingUp}
          className={`px-3 py-2 text-sm rounded-lg transition-colors ${
            warmingUp
              ? "bg-amber-100 dark:bg-amber-900/30 text-amber-500 dark:text-amber-300"
              : "bg-amber-50 dark:bg-amber-900/20 hover:bg-amber-100 dark:hover:bg-amber-900/40 text-amber-700 dark:text-amber-300"
          }`}
          title={warmingUp ? t("accountCard.warmupSending") : t("accountCard.warmupSend")}
        >
          ⚡
        </button>
        {onToggleAutoWarmup && (
          <button
            onClick={onToggleAutoWarmup}
            disabled={autoWarmupManagedByAll}
            className={`px-3 py-2 text-xs font-medium rounded-lg transition-colors whitespace-nowrap ${
              autoWarmupEnabled
                ? "bg-emerald-50 dark:bg-emerald-900/20 text-emerald-700 dark:text-emerald-300"
                : "bg-gray-100 dark:bg-gray-800 hover:bg-gray-200 dark:hover:bg-gray-700 text-gray-600 dark:text-gray-300"
            } disabled:opacity-60`}
            title={
              autoWarmupManagedByAll
                ? t("accountCard.autoAll")
                : autoWarmupEnabled
                  ? t("accountCard.autoDisable")
                : t("accountCard.autoEnable")
            }
          >
            {autoWarmupLabel ?? (autoWarmupEnabled ? t("warmup.autoOn") : t("warmup.autoOff"))}
          </button>
        )}
        <button
          onClick={handleRefresh}
          disabled={isRefreshing}
          className={`px-3 py-2 text-sm rounded-lg transition-colors ${
            isRefreshing
              ? "bg-gray-200 dark:bg-gray-800 text-gray-400 dark:text-gray-500"
              : "bg-gray-100 dark:bg-gray-800 hover:bg-gray-200 dark:hover:bg-gray-700 text-gray-600 dark:text-gray-300"
          }`}
          title={t("accountCard.refreshUsage")}
        >
          <span className={isRefreshing ? "animate-spin inline-block" : ""}>↻</span>
        </button>
        <button
          onClick={onDelete}
          className="px-3 py-2 text-sm rounded-lg bg-red-50 dark:bg-red-900/20 hover:bg-red-100 dark:hover:bg-red-900/40 text-red-600 dark:text-red-300 transition-colors"
          title={t("accountCard.remove")}
        >
          ✕
        </button>
      </div>
    </div>
  );
}
