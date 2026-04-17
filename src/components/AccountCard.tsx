import { useState, useRef, useEffect } from "react";
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
  queued?: boolean;
  switchLabel?: string;
  switchTitle?: string;
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
  queued = false,
  switchLabel,
  switchTitle,
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
      ? "API Key"
      : "Unknown";

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
            {queued && !account.is_active && (
              <span className="px-2 py-0.5 text-[11px] font-medium rounded-full border bg-sky-50 text-sky-700 border-sky-200 dark:bg-sky-900/30 dark:text-sky-300 dark:border-sky-700">
                Queued next
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
                title={masked ? undefined : "Click to rename"}
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

        <div className="flex items-center gap-2">
          {/* Eye toggle */}
          {onToggleMask && (
            <button
              onClick={onToggleMask}
              className="p-1 text-gray-400 dark:text-gray-500 hover:text-gray-600 dark:hover:text-gray-300 transition-colors"
              title={masked ? "Show info" : "Hide info"}
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
        </div>
      </div>

      {/* Usage */}
      <div className="mb-3">
        <UsageBar usage={account.usage} loading={isRefreshing || account.usageLoading} />
      </div>

      {/* Last refresh time */}
      <div className="text-xs text-gray-400 dark:text-gray-500 mb-3">
        Last updated: {formatLastRefresh(lastRefresh)}
      </div>

      {/* Actions */}
      <div className="flex gap-2">
        {account.is_active ? (
          <button
            disabled
            className="flex-1 px-4 py-2 text-sm font-medium rounded-lg bg-gray-100 dark:bg-gray-800 text-gray-500 dark:text-gray-400 border border-gray-200 dark:border-gray-700 cursor-default"
          >
            ✓ Active
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
            title={switchTitle}
          >
            {switching ? "Working..." : switchLabel ?? (switchDisabled ? "Queue Next" : "Switch Now")}
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
          title={warmingUp ? "Sending warm-up request..." : "Send minimal warm-up request"}
        >
          ⚡
        </button>
        <button
          onClick={handleRefresh}
          disabled={isRefreshing}
          className={`px-3 py-2 text-sm rounded-lg transition-colors ${
            isRefreshing
              ? "bg-gray-200 dark:bg-gray-800 text-gray-400 dark:text-gray-500"
              : "bg-gray-100 dark:bg-gray-800 hover:bg-gray-200 dark:hover:bg-gray-700 text-gray-600 dark:text-gray-300"
          }`}
          title="Refresh usage"
        >
          <span className={isRefreshing ? "animate-spin inline-block" : ""}>↻</span>
        </button>
        <button
          onClick={onDelete}
          className="px-3 py-2 text-sm rounded-lg bg-red-50 dark:bg-red-900/20 hover:bg-red-100 dark:hover:bg-red-900/40 text-red-600 dark:text-red-300 transition-colors"
          title="Remove account"
        >
          ✕
        </button>
      </div>
    </div>
  );
}
