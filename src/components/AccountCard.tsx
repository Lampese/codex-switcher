import { useEffect, useRef, useState } from "react";
import {
  getAccountNextResetAt,
  getAccountRemainingPercent,
  getRemainingPercentBand,
} from "../accountAnalytics";
import type { AccountWithUsage } from "../types";
import { UsageBar } from "./UsageBar";

interface AccountCardProps {
  account: AccountWithUsage;
  onSwitch: () => void;
  onWarmup: () => Promise<void>;
  onDelete: () => void;
  onRefresh: () => Promise<void>;
  onRename: (newName: string) => Promise<void>;
  switching?: boolean;
  switchDisabled?: boolean;
  warmingUp?: boolean;
  masked?: boolean;
  onToggleMask?: () => void;
  embedded?: boolean;
}

function formatLastRefresh(date: Date | null): string {
  if (!date) return "Never";
  const now = new Date();
  const diff = Math.floor((now.getTime() - date.getTime()) / 1000);
  if (diff < 5) return "Just now";
  if (diff < 60) return `${diff}s ago`;
  if (diff < 3600) return `${Math.floor(diff / 60)}m ago`;
  if (diff < 86400) return `${Math.floor(diff / 3600)}h ago`;
  return date.toLocaleDateString();
}

function formatTimeUntil(resetAt: number | null): string | null {
  if (!resetAt) return null;

  const diff = resetAt - Math.floor(Date.now() / 1000);
  if (diff <= 0) return "reset now";
  if (diff < 3600) return `resets in ${Math.ceil(diff / 60)}m`;

  if (diff < 86400) {
    const hours = Math.floor(diff / 3600);
    const minutes = Math.floor((diff % 3600) / 60);
    return `resets in ${minutes > 0 ? `${hours}h ${minutes}m` : `${hours}h`}`;
  }

  const days = Math.floor(diff / 86400);
  const hours = Math.floor((diff % 86400) / 3600);
  return `resets in ${hours > 0 ? `${days}d ${hours}h` : `${days}d`}`;
}

function getReserveState(remainingPercent: number | null) {
  const band = getRemainingPercentBand(remainingPercent);

  if (band === "unknown") {
    return {
      label: "Unknown",
      chipClass: "border-slate-200 bg-slate-100 text-slate-700 dark:border-slate-700 dark:bg-slate-800 dark:text-slate-200",
      borderClass: "border-slate-200 hover:border-slate-300 dark:border-slate-700 dark:hover:border-slate-600",
      accentClass: "bg-slate-300",
      glowClass: "bg-slate-200/70",
      metaClass: "border-slate-200 bg-slate-50 text-slate-600 dark:border-slate-700 dark:bg-slate-800/80 dark:text-slate-300",
    };
  }

  if (band === "depleted") {
    return {
      label: "Depleted",
      chipClass: "border-slate-200 bg-slate-100 text-slate-700 dark:border-slate-700 dark:bg-slate-800 dark:text-slate-200",
      borderClass: "border-slate-200 hover:border-slate-300 dark:border-slate-700 dark:hover:border-slate-600",
      accentClass: "bg-slate-400",
      glowClass: "bg-slate-300/70",
      metaClass: "border-slate-200 bg-slate-50 text-slate-600 dark:border-slate-700 dark:bg-slate-800/80 dark:text-slate-300",
    };
  }

  if (band === "critical") {
    return {
      label: "Critical",
      chipClass: "border-rose-200 bg-rose-50 text-rose-700 dark:border-rose-900/70 dark:bg-rose-500/12 dark:text-rose-300",
      borderClass: "border-rose-200 hover:border-rose-300 dark:border-rose-900/70 dark:hover:border-rose-800/80",
      accentClass: "bg-rose-500",
      glowClass: "bg-rose-300/70",
      metaClass: "border-rose-100 bg-rose-50/70 text-rose-700 dark:border-rose-900/60 dark:bg-rose-500/10 dark:text-rose-300",
    };
  }

  if (band === "watch") {
    return {
      label: "Watch",
      chipClass: "border-amber-200 bg-amber-50 text-amber-700 dark:border-amber-900/70 dark:bg-amber-500/12 dark:text-amber-300",
      borderClass: "border-amber-200 hover:border-amber-300 dark:border-amber-900/70 dark:hover:border-amber-800/80",
      accentClass: "bg-amber-500",
      glowClass: "bg-amber-300/70",
      metaClass: "border-amber-100 bg-amber-50/70 text-amber-700 dark:border-amber-900/60 dark:bg-amber-500/10 dark:text-amber-300",
    };
  }

  if (band === "healthy") {
    return {
      label: "Healthy",
      chipClass: "border-sky-200 bg-sky-50 text-sky-700 dark:border-sky-900/70 dark:bg-sky-500/12 dark:text-sky-300",
      borderClass: "border-sky-200 hover:border-sky-300 dark:border-sky-900/70 dark:hover:border-sky-800/80",
      accentClass: "bg-sky-500",
      glowClass: "bg-sky-300/70",
      metaClass: "border-sky-100 bg-sky-50/70 text-sky-700 dark:border-sky-900/60 dark:bg-sky-500/10 dark:text-sky-300",
    };
  }

  return {
    label: "Ready",
    chipClass: "border-emerald-200 bg-emerald-50 text-emerald-700 dark:border-emerald-900/70 dark:bg-emerald-500/12 dark:text-emerald-300",
    borderClass: "border-emerald-200 hover:border-emerald-300 dark:border-emerald-900/70 dark:hover:border-emerald-800/80",
    accentClass: "bg-emerald-500",
    glowClass: "bg-emerald-300/70",
    metaClass: "border-emerald-100 bg-emerald-50/70 text-emerald-700 dark:border-emerald-900/60 dark:bg-emerald-500/10 dark:text-emerald-300",
  };
}

function BlurredText({ children, blur }: { children: React.ReactNode; blur: boolean }) {
  return (
    <span
      className={`select-none transition-all duration-200 ${blur ? "blur-sm" : ""}`}
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
  embedded = false,
}: AccountCardProps) {
  const [isRefreshing, setIsRefreshing] = useState(false);
  const [lastRefresh, setLastRefresh] = useState<Date | null>(
    account.usage && !account.usage.error ? new Date() : null
  );
  const [isEditing, setIsEditing] = useState(false);
  const [editName, setEditName] = useState(account.name);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (isEditing && inputRef.current) {
      inputRef.current.focus();
      inputRef.current.select();
    }
  }, [isEditing]);

  useEffect(() => {
    if (account.usage && !account.usage.error && !account.usageLoading) {
      setLastRefresh(new Date());
    }
  }, [account.usage, account.usageLoading]);

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
      void handleRename();
    } else if (e.key === "Escape") {
      setEditName(account.name);
      setIsEditing(false);
    }
  };

  const planDisplay = account.plan_type
    ? account.plan_type.charAt(0).toUpperCase() + account.plan_type.slice(1)
    : account.auth_mode === "api_key"
      ? "API Key"
      : "Unknown";

  const planColors: Record<string, string> = {
    pro: "bg-indigo-50 text-indigo-700 border-indigo-200 dark:bg-indigo-500/10 dark:text-indigo-300 dark:border-indigo-900/70",
    plus: "bg-emerald-50 text-emerald-700 border-emerald-200 dark:bg-emerald-500/10 dark:text-emerald-300 dark:border-emerald-900/70",
    team: "bg-blue-50 text-blue-700 border-blue-200 dark:bg-blue-500/10 dark:text-blue-300 dark:border-blue-900/70",
    enterprise: "bg-amber-50 text-amber-700 border-amber-200 dark:bg-amber-500/10 dark:text-amber-300 dark:border-amber-900/70",
    free: "bg-gray-50 text-gray-600 border-gray-200 dark:bg-slate-800 dark:text-slate-200 dark:border-slate-700",
    api_key: "bg-orange-50 text-orange-700 border-orange-200 dark:bg-orange-500/10 dark:text-orange-300 dark:border-orange-900/70",
  };

  const planKey = account.plan_type?.toLowerCase() || "api_key";
  const planColorClass = planColors[planKey] || planColors.free;
  const lastUpdatedLabel = formatLastRefresh(lastRefresh);
  const remainingPercent = getAccountRemainingPercent(account);
  const nextResetAt = getAccountNextResetAt(account);
  const reserveState = getReserveState(remainingPercent);
  const resetSummary = formatTimeUntil(nextResetAt);
  const utilityButtonClass =
    "flex h-9 w-9 items-center justify-center rounded-lg text-sm transition-colors";
  const containerClass = embedded
    ? "relative"
    : `relative isolate overflow-hidden rounded-[22px] border px-4 py-3.5 shadow-[0_1px_2px_rgba(15,23,42,0.05),0_12px_24px_rgba(15,23,42,0.05)] ring-1 ring-white/70 transform-gpu [contain:paint] transition-[transform,box-shadow,border-color,background-color] duration-200 hover:-translate-y-0.5 hover:shadow-[0_1px_2px_rgba(15,23,42,0.06),0_18px_30px_rgba(15,23,42,0.08)] dark:ring-slate-900/60 dark:shadow-[0_1px_2px_rgba(2,6,23,0.45),0_16px_26px_rgba(2,6,23,0.35)] dark:hover:shadow-[0_1px_2px_rgba(2,6,23,0.5),0_20px_34px_rgba(2,6,23,0.45)] ${
        account.is_active
          ? "border-emerald-400 bg-white dark:border-emerald-700 dark:bg-slate-900"
          : `bg-[linear-gradient(180deg,rgba(255,255,255,0.98),rgba(248,250,252,0.94))] dark:bg-[linear-gradient(180deg,rgba(15,23,42,0.96),rgba(15,23,42,0.9))] ${reserveState.borderClass}`
      }`;

  return (
    <div className={containerClass}>
      {!account.is_active && (
        <>
          <div className={`absolute inset-x-0 top-0 h-1 ${reserveState.accentClass}`} />
          <div className={`pointer-events-none absolute left-4 top-0 h-16 w-28 blur-2xl ${reserveState.glowClass}`} />
        </>
      )}

      <div className="relative mb-2.5 flex items-start justify-between gap-2.5">
        <div className="min-w-0 flex-1">
          <div className="mb-0.5 flex items-center gap-2">
            {account.is_active && (
              <span className="flex h-2 w-2">
                <span className="absolute inline-flex h-2 w-2 animate-ping rounded-full bg-green-400 opacity-75"></span>
                <span className="relative inline-flex h-2 w-2 rounded-full bg-green-500"></span>
              </span>
            )}

            {isEditing ? (
              <input
                ref={inputRef}
                type="text"
                value={editName}
                onChange={(e) => setEditName(e.target.value)}
                onBlur={() => {
                  void handleRename();
                }}
                onKeyDown={handleKeyDown}
                className="w-full rounded border border-gray-300 bg-gray-100 px-2 py-0.5 font-semibold text-gray-900 focus:border-gray-500 focus:outline-none dark:border-slate-700 dark:bg-slate-800 dark:text-white dark:focus:border-slate-500"
              />
            ) : (
              <h3
                className="cursor-pointer truncate text-[1.05rem] font-semibold tracking-tight text-slate-900 hover:text-slate-700 dark:text-white dark:hover:text-slate-200"
                onClick={() => {
                  setEditName(account.name);
                  setIsEditing(true);
                }}
                title="Click to rename"
              >
                {account.name}
              </h3>
            )}
          </div>

          {account.email && (
            <p className="truncate text-[13px] text-slate-500 dark:text-slate-400">
              <BlurredText blur={masked}>{account.email}</BlurredText>
            </p>
          )}
        </div>

        <div className="flex shrink-0 items-center gap-1.5">
          {onToggleMask && (
            <button
              onClick={onToggleMask}
              className="flex h-8 w-8 items-center justify-center rounded-lg border border-slate-200 bg-white/80 text-slate-400 transition-colors hover:border-slate-300 hover:text-slate-600 dark:border-slate-700 dark:bg-slate-800/80 dark:text-slate-500 dark:hover:border-slate-600 dark:hover:text-slate-300"
              title={masked ? "Show email" : "Hide email"}
            >
              {masked ? (
                <svg className="h-4 w-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M13.875 18.825A10.05 10.05 0 0112 19c-4.478 0-8.268-2.943-9.543-7a9.97 9.97 0 011.563-3.029m5.858.908a3 3 0 114.243 4.243M9.878 9.878l4.242 4.242M9.88 9.88l-3.29-3.29m7.532 7.532l3.29 3.29M3 3l3.59 3.59m0 0A9.953 9.953 0 0112 5c4.478 0 8.268 2.943 9.543 7a10.025 10.025 0 01-4.132 5.411m0 0L21 21" />
                </svg>
              ) : (
                <svg className="h-4 w-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M15 12a3 3 0 11-6 0 3 3 0 016 0z" />
                  <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M2.458 12C3.732 7.943 7.523 5 12 5c4.478 0 8.268 2.943 9.542 7-1.274 4.057-5.064 7-9.542 7-4.477 0-8.268-2.943-9.542-7z" />
                </svg>
              )}
            </button>
          )}

          <span className={`rounded-full border px-2.5 py-1 text-[11px] font-medium ${planColorClass}`}>
            {planDisplay}
          </span>
        </div>
      </div>

      {!account.is_active && (
        <div className="mb-2.5">
          <div className="flex flex-wrap gap-1.5">
            <span className={`inline-flex items-center rounded-full border px-2.5 py-1 text-[11px] font-semibold uppercase tracking-[0.12em] ${reserveState.chipClass}`}>
              {reserveState.label}
            </span>

            {remainingPercent !== null && (
              <span className={`inline-flex items-center rounded-full border px-2.5 py-1 text-[11px] font-medium ${reserveState.metaClass}`}>
                {Math.round(remainingPercent)}% reserve
              </span>
            )}

            {resetSummary && (
              <span className="inline-flex items-center rounded-full border border-slate-200 bg-white/80 px-2.5 py-1 text-[11px] font-medium text-slate-600 dark:border-slate-700 dark:bg-slate-800/80 dark:text-slate-300">
                {resetSummary}
              </span>
            )}
          </div>

          <p className="mt-2 text-[11px] font-medium tracking-[0.01em] text-slate-400 dark:text-slate-400">
            Refreshed {lastUpdatedLabel}
          </p>
        </div>
      )}

      <div className="mb-2.5">
        <UsageBar usage={account.usage} loading={isRefreshing || account.usageLoading} />
      </div>

      {account.is_active ? (
        <div className="flex flex-col gap-3 border-t border-gray-100 pt-3 dark:border-slate-800 sm:flex-row sm:items-center sm:justify-between">
          <div className="flex flex-wrap items-center gap-2 text-xs">
            {embedded ? (
              <span className="text-gray-400 dark:text-slate-400">Updated {lastUpdatedLabel}</span>
            ) : (
              <>
                <span className="inline-flex items-center gap-1.5 rounded-full border border-emerald-200 bg-emerald-50 px-2.5 py-1 font-medium text-emerald-700 dark:border-emerald-900/70 dark:bg-emerald-500/12 dark:text-emerald-300">
                  <span className="h-1.5 w-1.5 rounded-full bg-emerald-500" />
                  Current session
                </span>
                {remainingPercent !== null && (
                  <span
                    className={`inline-flex items-center rounded-full border px-2.5 py-1 font-medium ${reserveState.chipClass}`}
                  >
                    {reserveState.label} reserve
                  </span>
                )}
                {resetSummary && <span className="text-slate-500 dark:text-slate-400">{resetSummary}</span>}
                <span className="text-gray-400 dark:text-slate-400">Updated {lastUpdatedLabel}</span>
              </>
            )}
          </div>

          <div className="flex gap-2">
            <button
              onClick={() => {
                void onWarmup();
              }}
              disabled={warmingUp}
              className={`${utilityButtonClass} ${
                warmingUp
                  ? "bg-amber-100 text-amber-500 dark:bg-amber-500/18 dark:text-amber-300"
                  : "bg-amber-50 text-amber-700 hover:bg-amber-100 dark:bg-amber-500/12 dark:text-amber-300 dark:hover:bg-amber-500/18"
              }`}
              title={warmingUp ? "Sending warm-up request..." : "Send minimal warm-up request"}
            >
              ⚡
            </button>
            <button
              onClick={handleRefresh}
              disabled={isRefreshing}
              className={`${utilityButtonClass} ${
                isRefreshing
                  ? "bg-gray-200 text-gray-400 dark:bg-slate-800 dark:text-slate-500"
                  : "bg-gray-100 text-gray-600 hover:bg-gray-200 dark:bg-slate-800 dark:text-slate-300 dark:hover:bg-slate-700"
              }`}
              title="Refresh usage"
            >
              <span className={isRefreshing ? "animate-spin inline-block" : ""}>↻</span>
            </button>
            <button
              onClick={onDelete}
              className={`${utilityButtonClass} bg-red-50 text-red-600 hover:bg-red-100 dark:bg-rose-500/12 dark:text-rose-300 dark:hover:bg-rose-500/18`}
              title="Remove account"
            >
              ✕
            </button>
          </div>
        </div>
      ) : (
        <div className="flex items-center gap-2 border-t border-slate-100 pt-2.5 dark:border-slate-800">
          <button
            onClick={onSwitch}
            disabled={switching || switchDisabled}
            className={`flex-1 rounded-xl px-4 py-2.5 text-sm font-semibold transition-all disabled:opacity-50 ${
              switchDisabled
                ? "cursor-not-allowed bg-slate-200 text-slate-400 dark:bg-slate-800 dark:text-slate-500"
                : remainingPercent !== null && remainingPercent <= 0
                  ? "bg-slate-700 text-white hover:bg-slate-800 dark:bg-slate-700 dark:text-slate-100 dark:hover:bg-slate-600"
                  : "bg-slate-900 text-white hover:bg-slate-800 dark:bg-slate-50 dark:text-slate-950 dark:hover:bg-white"
            }`}
            title={switchDisabled ? "Close all Codex processes first" : undefined}
          >
            {switching ? "Switching..." : switchDisabled ? "Codex Running" : "Switch"}
          </button>

          <div className="flex items-center gap-1 rounded-2xl border border-slate-200 bg-white/90 p-1 shadow-[0_1px_2px_rgba(15,23,42,0.04),inset_0_1px_0_rgba(255,255,255,0.7)] dark:border-slate-700 dark:bg-slate-900/90 dark:shadow-[0_1px_2px_rgba(2,6,23,0.28),inset_0_1px_0_rgba(255,255,255,0.03)]">
            <button
              onClick={() => {
                void onWarmup();
              }}
              disabled={warmingUp}
              className={`${utilityButtonClass} ${
                warmingUp
                  ? "bg-amber-100 text-amber-500 dark:bg-amber-500/18 dark:text-amber-300"
                  : "bg-amber-50 text-amber-700 hover:bg-amber-100 dark:bg-amber-500/12 dark:text-amber-300 dark:hover:bg-amber-500/18"
              }`}
              title={warmingUp ? "Sending warm-up request..." : "Send minimal warm-up request"}
            >
              ⚡
            </button>
            <button
              onClick={handleRefresh}
              disabled={isRefreshing}
              className={`${utilityButtonClass} ${
                isRefreshing
                  ? "bg-slate-200 text-slate-400 dark:bg-slate-800 dark:text-slate-500"
                  : "bg-slate-100 text-slate-600 hover:bg-slate-200 dark:bg-slate-800 dark:text-slate-300 dark:hover:bg-slate-700"
              }`}
              title="Refresh usage"
            >
              <span className={isRefreshing ? "animate-spin inline-block" : ""}>↻</span>
            </button>
            <button
              onClick={onDelete}
              className={`${utilityButtonClass} bg-rose-50 text-rose-600 hover:bg-rose-100 dark:bg-rose-500/12 dark:text-rose-300 dark:hover:bg-rose-500/18`}
              title="Remove account"
            >
              ✕
            </button>
          </div>
        </div>
      )}
    </div>
  );
}
