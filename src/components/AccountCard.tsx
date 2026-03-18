import { useState, useRef, useEffect, type KeyboardEvent, type ReactNode } from "react";
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

function BlurredText({
  children,
  blur,
}: {
  children: ReactNode;
  blur: boolean;
}) {
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

  const handleKeyDown = (e: KeyboardEvent) => {
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
    pro: "theme-plan-badge",
    plus: "theme-plan-badge theme-plan-badge--success",
    team: "theme-plan-badge",
    enterprise: "theme-plan-badge theme-plan-badge--warning",
    free: "theme-plan-badge",
    api_key: "theme-plan-badge theme-plan-badge--warning",
  };

  const planKey = account.plan_type?.toLowerCase() || "api_key";
  const planColorClass = planColors[planKey] || planColors.free;

  return (
    <div
      className={`theme-card relative rounded-xl p-5 transition-all duration-200 ${
        account.is_active
          ? "theme-card--active"
          : "theme-card--inactive"
      }`}
    >
      <div className="mb-3 flex items-start justify-between">
        <div className="min-w-0 flex-1">
          <div className="mb-1 flex items-center gap-2">
            {account.is_active && (
              <span className="flex h-2 w-2">
                <span
                  className="absolute inline-flex h-2 w-2 animate-ping rounded-full opacity-50"
                  style={{ backgroundColor: "var(--theme-text-secondary)" }}
                />
                <span
                  className="relative inline-flex h-2 w-2 rounded-full"
                  style={{ backgroundColor: "var(--theme-primary-bg)" }}
                />
              </span>
            )}
            {isEditing ? (
              <input
                ref={inputRef}
                type="text"
                value={editName}
                onChange={(e) => setEditName(e.target.value)}
                onBlur={() => void handleRename()}
                onKeyDown={handleKeyDown}
                className="theme-field w-full rounded px-2 py-0.5 font-semibold"
              />
            ) : (
              <h3
                className="cursor-pointer truncate font-semibold text-gray-900 hover:text-gray-600 dark:text-slate-100 dark:hover:text-slate-300"
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
            <p className="truncate text-sm text-gray-500 dark:text-slate-400">
              <BlurredText blur={masked}>{account.email}</BlurredText>
            </p>
          )}
        </div>

        <div className="flex items-center gap-2">
          {onToggleMask && (
            <button
              onClick={onToggleMask}
              className="p-1 text-gray-400 transition-colors hover:text-gray-600 dark:text-slate-500 dark:hover:text-slate-300"
              title={masked ? "Show info" : "Hide info"}
            >
              {masked ? (
                <svg className="h-4 w-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <path
                    strokeLinecap="round"
                    strokeLinejoin="round"
                    strokeWidth={2}
                    d="M13.875 18.825A10.05 10.05 0 0112 19c-4.478 0-8.268-2.943-9.543-7a9.97 9.97 0 011.563-3.029m5.858.908a3 3 0 114.243 4.243M9.878 9.878l4.242 4.242M9.88 9.88l-3.29-3.29m7.532 7.532l3.29 3.29M3 3l3.59 3.59m0 0A9.953 9.953 0 0112 5c4.478 0 8.268 2.943 9.543 7a10.025 10.025 0 01-4.132 5.411m0 0L21 21"
                  />
                </svg>
              ) : (
                <svg className="h-4 w-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                  <path
                    strokeLinecap="round"
                    strokeLinejoin="round"
                    strokeWidth={2}
                    d="M15 12a3 3 0 11-6 0 3 3 0 016 0z"
                  />
                  <path
                    strokeLinecap="round"
                    strokeLinejoin="round"
                    strokeWidth={2}
                    d="M2.458 12C3.732 7.943 7.523 5 12 5c4.478 0 8.268 2.943 9.542 7-1.274 4.057-5.064 7-9.542 7-4.477 0-8.268-2.943-9.542-7z"
                  />
                </svg>
              )}
            </button>
          )}
          <span className={`rounded-full border px-2.5 py-1 text-xs font-medium ${planColorClass}`}>
            {planDisplay}
          </span>
        </div>
      </div>

      <div className="mb-3">
        <UsageBar usage={account.usage} loading={isRefreshing || account.usageLoading} />
      </div>

      <div className="mb-3 text-xs text-gray-400 dark:text-slate-500">
        Last updated: {formatLastRefresh(lastRefresh)}
      </div>

      <div className="flex gap-2">
        {account.is_active ? (
          <button
            disabled
            className="theme-button-disabled flex-1 cursor-default rounded-lg px-4 py-2 text-sm font-medium"
          >
            Active
          </button>
        ) : (
          <button
            onClick={onSwitch}
            disabled={switching || switchDisabled}
            className={`flex-1 rounded-lg px-4 py-2 text-sm font-medium disabled:opacity-50 ${
              switchDisabled
                ? "theme-button-disabled"
                : "theme-button-primary"
            }`}
            title={switchDisabled ? "Close all Codex processes first" : undefined}
          >
            {switching ? "Switching..." : switchDisabled ? "Codex Running" : "Switch"}
          </button>
        )}
        <button
          onClick={() => {
            void onWarmup();
          }}
          disabled={warmingUp}
          className="theme-button-warning rounded-lg px-3 py-2 text-sm"
          title={warmingUp ? "Sending warm-up request..." : "Send minimal warm-up request"}
        >
          Warm
        </button>
        <button
          onClick={handleRefresh}
          disabled={isRefreshing}
          className={`rounded-lg px-3 py-2 text-sm ${isRefreshing ? "theme-button-disabled" : "theme-button-secondary"}`}
          title="Refresh usage"
        >
          <span className={isRefreshing ? "inline-block animate-spin" : ""}>Sync</span>
        </button>
        <button
          onClick={onDelete}
          className="theme-button-danger rounded-lg px-3 py-2 text-sm"
          title="Remove account"
        >
          Delete
        </button>
      </div>
    </div>
  );
}
