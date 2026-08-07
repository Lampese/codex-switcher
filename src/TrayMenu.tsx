import { useCallback, useEffect, useState } from "react";
import type { AccountInfo, AccountUsageStats, DockDisplayMode, UsageInfo } from "./types";
import { invokeBackend, isTauriRuntime } from "./lib/platform";
import { LanguageToggle, useI18n } from "./lib/i18n";
import {
  applyTheme,
  syncThemeFromStorage,
  THEME_CHANGED_EVENT,
  type ThemeMode,
} from "./lib/theme";
import {
  AUTO_WARMUP_ALL_CHANGED_EVENT,
  readAutoWarmupAllEnabled,
  writeAutoWarmupAllEnabled,
} from "./lib/autoWarmup";

const TRAY_REFRESH_EVENT = "tray-refresh";
const ACCOUNTS_CHANGED_EVENT = "accounts-changed";
const SWITCH_ACCOUNT_BLOCKED_EVENT = "switch-account-blocked";
// Stable backend error code from process.rs (ensure_codex_not_running).
const CODEX_RUNNING_PREFIX = "CODEX_RUNNING";

function formatError(err: unknown): string {
  if (!err) return "Unknown error";
  if (err instanceof Error && err.message) return err.message;
  if (typeof err === "string") return err;
  try {
    return JSON.stringify(err);
  } catch {
    return "Unknown error";
  }
}

// "plus" -> "Plus". Returns null when there is no usable plan label.
function formatPlan(plan: string | null): string | null {
  const trimmed = plan?.trim();
  if (!trimmed) return null;
  return trimmed.charAt(0).toUpperCase() + trimmed.slice(1);
}

// Color classes for a rate-limit window based on remaining %, matching the main app.
function remainingTone(remaining: number): { text: string; bar: string; dot: string } {
  if (remaining <= 10) {
    return { text: "text-red-500 dark:text-red-400", bar: "bg-red-500", dot: "bg-red-500" };
  }
  if (remaining <= 30) {
    return {
      text: "text-amber-500 dark:text-amber-400",
      bar: "bg-amber-500",
      dot: "bg-amber-500",
    };
  }
  return {
    text: "text-green-600 dark:text-green-400",
    bar: "bg-emerald-500",
    dot: "bg-emerald-500",
  };
}

// "time until reset" label, e.g. "4h 55m" / "4d 18h" / "now".
function formatResetAt(resetAt: number | null | undefined): string | null {
  if (!resetAt) return null;

  const diff = resetAt - Math.floor(Date.now() / 1000);
  if (diff <= 0) return "now";
  if (diff < 60) return `${diff}s`;
  if (diff < 3600) return `${Math.floor(diff / 60)}m`;
  if (diff < 86_400) {
    return `${Math.floor(diff / 3600)}h ${Math.floor((diff % 3600) / 60)}m`;
  }
  return `${Math.floor(diff / 86_400)}d ${Math.floor((diff % 86_400) / 3600)}h`;
}

function formatTokens(tokens: number | null | undefined): string {
  if (tokens === null || tokens === undefined || !Number.isFinite(tokens)) return "--";
  const abs = Math.abs(tokens);
  if (abs >= 1_000_000_000) return `${(tokens / 1_000_000_000).toFixed(1)}B`;
  if (abs >= 1_000_000) return `${(tokens / 1_000_000).toFixed(1)}M`;
  if (abs >= 1_000) return `${(tokens / 1_000).toFixed(1)}K`;
  return `${tokens}`;
}

function dayKey(offset: number): string {
  const date = new Date();
  date.setDate(date.getDate() - offset);
  const year = date.getFullYear();
  const month = `${date.getMonth() + 1}`.padStart(2, "0");
  const day = `${date.getDate()}`.padStart(2, "0");
  return `${year}-${month}-${day}`;
}

function sumDailyTokens(stats: AccountUsageStats, days: number): number {
  const keys = new Set(Array.from({ length: days }, (_, index) => dayKey(index)));
  return stats.daily.reduce((total, day) => (keys.has(day.date) ? total + day.tokens : total), 0);
}

function retainUsageForAccounts(
  usageById: Record<string, UsageInfo>,
  accounts: AccountInfo[]
): Record<string, UsageInfo> {
  return Object.fromEntries(
    accounts.flatMap((account) =>
      usageById[account.id] ? [[account.id, usageById[account.id]]] : []
    )
  );
}

function TrayMenu() {
  const { t } = useI18n();
  const [accounts, setAccounts] = useState<AccountInfo[]>([]);
  const [loading, setLoading] = useState(true);
  const [switchingId, setSwitchingId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [usageById, setUsageById] = useState<Record<string, UsageInfo>>({});
  const [statsById, setStatsById] = useState<Record<string, AccountUsageStats>>({});
  const [refreshing, setRefreshing] = useState(false);
  const [autoWarmupAllEnabled, setAutoWarmupAllEnabled] = useState(readAutoWarmupAllEnabled);
  const [dockDisplayMode, setDockDisplayMode] = useState<DockDisplayMode | null>(null);

  // Fetch each account's rate-limit usage in parallel; rows fill in as they land.
  const loadUsage = useCallback(async (list: AccountInfo[]) => {
    const readyAccounts = list.filter((account) => account.auth_state === "ready");
    setUsageById((prev) => retainUsageForAccounts(prev, readyAccounts));
    await Promise.all(
      readyAccounts.map(async (account) => {
        try {
          const usage = await invokeBackend<UsageInfo>("get_usage", {
            accountId: account.id,
          });
          setUsageById((prev) => ({ ...prev, [account.id]: usage }));
        } catch (err) {
          setUsageById((prev) => ({
            ...prev,
            [account.id]: {
              account_id: account.id,
              plan_type: account.plan_type,
              primary_used_percent: null,
              primary_window_minutes: null,
              primary_resets_at: null,
              secondary_used_percent: null,
              secondary_window_minutes: null,
              secondary_resets_at: null,
              has_credits: null,
              unlimited_credits: null,
              credits_balance: null,
              error: formatError(err),
            },
          }));
        }
      })
    );
  }, []);

  const loadActiveStats = useCallback(async (list: AccountInfo[]) => {
    const active = list.find((account) => account.is_active);
    if (!active) return;
    if (active.auth_state === "reauth_required") {
      setStatsById((prev) => {
        const next = { ...prev };
        delete next[active.id];
        return next;
      });
      return;
    }

    try {
      const stats = await invokeBackend<AccountUsageStats>("get_account_usage_stats", {
        accountId: active.id,
      });
      setStatsById((prev) => ({ ...prev, [active.id]: stats }));
    } catch (err) {
      setStatsById((prev) => ({
        ...prev,
        [active.id]: {
          account_id: active.id,
          available: false,
          source: "Codex usage stats via ChatGPT backend",
          generated_at: null,
          stats_as_of: null,
          summary: {
            lifetime_tokens: null,
            peak_daily_tokens: null,
            longest_task_seconds: null,
            current_streak_days: null,
            longest_streak_days: null,
          },
          activity: {
            fast_mode_percent: null,
            reasoning_effort: null,
            reasoning_effort_percent: null,
            skills_explored: null,
            total_skills_used: null,
            total_threads: null,
          },
          daily: [],
          top_invocations: [],
          reset_credits: null,
          error: formatError(err),
        },
      }));
    }
  }, []);

  const loadDockDisplayMode = useCallback(async () => {
    try {
      const mode = await invokeBackend<DockDisplayMode | null>("get_dock_display_mode");
      setDockDisplayMode(mode);
    } catch {
      setDockDisplayMode(null);
    }
  }, []);

  const load = useCallback(async () => {
    try {
      void loadDockDisplayMode();
      const list = await invokeBackend<AccountInfo[]>("list_accounts");
      setAccounts(list);
      setUsageById((prev) => retainUsageForAccounts(prev, list));
      setError(null);
      void loadUsage(list); // Don't block the list render on the usage calls.
      void loadActiveStats(list);
    } catch (err) {
      setError(formatError(err));
    } finally {
      setLoading(false);
    }
  }, [loadActiveStats, loadDockDisplayMode, loadUsage]);

  // Manual refresh: re-pull accounts and actively fetch fresh usage once.
  const handleRefresh = useCallback(async () => {
    setRefreshing(true);
    try {
      const list = await invokeBackend<AccountInfo[]>("list_accounts");
      setAccounts(list);
      setUsageById((prev) => retainUsageForAccounts(prev, list));
      setError(null);
      await Promise.all([loadUsage(list), loadActiveStats(list)]);
    } catch (err) {
      setError(formatError(err));
    } finally {
      setRefreshing(false);
    }
  }, [loadActiveStats, loadUsage]);

  const handleAutoWarmupToggle = useCallback(async () => {
    const next = !autoWarmupAllEnabled;
    setAutoWarmupAllEnabled(next);
    try {
      writeAutoWarmupAllEnabled(next);
      if (isTauriRuntime()) {
        const { emit } = await import("@tauri-apps/api/event");
        await emit(AUTO_WARMUP_ALL_CHANGED_EVENT, next);
      }
    } catch (err) {
      setAutoWarmupAllEnabled(!next);
      setError(formatError(err));
    }
  }, [autoWarmupAllEnabled]);

  const handleDockDisplayMode = useCallback(
    async (mode: DockDisplayMode) => {
      const previous = dockDisplayMode;
      setDockDisplayMode(mode);
      try {
        const next = await invokeBackend<DockDisplayMode | null>("set_dock_display_mode", {
          mode,
        });
        setDockDisplayMode(next);
      } catch (err) {
        setDockDisplayMode(previous);
        setError(formatError(err));
      }
    },
    [dockDisplayMode]
  );

  // Reload when the tray is reopened or accounts change elsewhere.
  useEffect(() => {
    if (!isTauriRuntime()) return;
    let unlistenRefresh: (() => void) | undefined;
    let unlistenChanged: (() => void) | undefined;
    let unlistenTheme: (() => void) | undefined;
    let unlistenAutoWarmup: (() => void) | undefined;

    void (async () => {
      const { listen } = await import("@tauri-apps/api/event");
      unlistenRefresh = await listen(TRAY_REFRESH_EVENT, () => {
        syncThemeFromStorage();
        setAutoWarmupAllEnabled(readAutoWarmupAllEnabled());
        void load();
      });
      unlistenChanged = await listen(ACCOUNTS_CHANGED_EVENT, () => void load());
      unlistenTheme = await listen<ThemeMode>(THEME_CHANGED_EVENT, ({ payload }) => {
        if (payload === "light" || payload === "dark") {
          applyTheme(payload);
        }
      });
      unlistenAutoWarmup = await listen<boolean>(
        AUTO_WARMUP_ALL_CHANGED_EVENT,
        ({ payload }) => {
          if (typeof payload === "boolean") {
            setAutoWarmupAllEnabled(payload);
          }
        }
      );
    })();

    return () => {
      unlistenRefresh?.();
      unlistenChanged?.();
      unlistenTheme?.();
      unlistenAutoWarmup?.();
    };
  }, [load]);

  const handleSwitch = useCallback(async (account: AccountInfo) => {
    if (account.auth_state === "reauth_required") {
      setError(t("sessionExpiredHint"));
      void invokeBackend("open_main_window");
      return;
    }
    if (account.is_active) {
      void invokeBackend("hide_tray_window");
      return;
    }
    try {
      setSwitchingId(account.id);
      setError(null);
      await invokeBackend("switch_account", { accountId: account.id });
      // Notify the main window immediately so its active-account state stays in
      // sync without waiting on the backend accounts-file watcher (~1s poll).
      const { emit } = await import("@tauri-apps/api/event");
      await emit(ACCOUNTS_CHANGED_EVENT);
      void invokeBackend("hide_tray_window");
    } catch (err) {
      const message = formatError(err);
      // Codex is running: hand off to the main window's force-close flow.
      if (message.startsWith(CODEX_RUNNING_PREFIX)) {
        const { emit } = await import("@tauri-apps/api/event");
        await emit(SWITCH_ACCOUNT_BLOCKED_EVENT, {
          accountId: account.id,
          error: message,
        });
        void invokeBackend("open_main_window"); // focus main + hide tray
        return;
      }
      if (message.includes("AUTH_REAUTH_REQUIRED")) {
        setError(t("sessionExpiredHint"));
        void invokeBackend("open_main_window");
        return;
      }
      setError(message);
    } finally {
      setSwitchingId(null);
    }
  }, [t]);

  return (
    <div className="flex h-screen w-screen flex-col overflow-hidden rounded-xl border border-gray-200 bg-white text-gray-900 shadow-2xl dark:border-gray-700 dark:bg-gray-900 dark:text-gray-100">
      <div className="flex items-center gap-2 border-b border-gray-100 px-3 py-2 dark:border-gray-800">
        <div className="flex h-6 w-6 items-center justify-center rounded-md bg-black text-xs font-bold text-white">
          C
        </div>
        <span className="text-sm font-semibold">Codex Switcher</span>
        <LanguageToggle compact />
        <button
          onClick={() => void handleAutoWarmupToggle()}
          disabled={accounts.length === 0}
          title={
            autoWarmupAllEnabled
              ? t("disableAutoAll")
              : t("enableAutoAll")
          }
          className={`ml-auto rounded-md px-2 py-1 text-[11px] font-semibold transition-colors disabled:opacity-50 ${
            autoWarmupAllEnabled
              ? "bg-emerald-50 text-emerald-700 hover:bg-emerald-100 dark:bg-emerald-900/20 dark:text-emerald-300 dark:hover:bg-emerald-900/30"
              : "bg-gray-100 text-gray-700 hover:bg-gray-200 dark:bg-gray-800 dark:text-gray-200 dark:hover:bg-gray-700"
          }`}
        >
          {autoWarmupAllEnabled ? t("autoOn") : t("autoOff")}
        </button>
        <button
          onClick={() => void handleRefresh()}
          disabled={refreshing}
          title={t("refreshUsage")}
          className="flex h-6 w-6 items-center justify-center rounded-md text-gray-500 transition-colors hover:bg-gray-100 hover:text-gray-900 disabled:opacity-50 dark:text-gray-400 dark:hover:bg-gray-800 dark:hover:text-gray-100"
        >
          <span className={`text-base leading-none ${refreshing ? "inline-block animate-spin" : ""}`}>
            ↻
          </span>
        </button>
      </div>

      <div className="flex-1 overflow-y-auto p-1.5">
        {loading ? (
          <div className="px-2 py-6 text-center text-xs text-gray-500 dark:text-gray-400">
            {t("loading")}
          </div>
        ) : accounts.length === 0 ? (
          <div className="px-2 py-6 text-center text-xs text-gray-500 dark:text-gray-400">
            {t("noAccounts")}
          </div>
        ) : (
          accounts.map((account) => {
            const requiresReauth = account.auth_state === "reauth_required";
            const plan = formatPlan(account.plan_type);
            const usage = requiresReauth ? undefined : usageById[account.id];
            const stats = requiresReauth ? undefined : statsById[account.id];
            const windows =
              usage && !usage.error
                ? ([
                    {
                      label: t("session"),
                      used: usage.primary_used_percent,
                      resetAt: usage.primary_resets_at,
                    },
                    {
                      label: t("weekly"),
                      used: usage.secondary_used_percent,
                      resetAt: usage.secondary_resets_at,
                    },
                  ].filter((w) => w.used != null) as {
                    label: string;
                    used: number;
                    resetAt: number | null;
                  }[])
                : [];

            return (
              <button
                key={account.id}
                onClick={() => void handleSwitch(account)}
                disabled={switchingId !== null}
                className={`flex w-full items-start gap-2 rounded-lg px-2 py-1.5 text-left transition-colors disabled:opacity-60 ${
                  requiresReauth
                    ? "bg-red-50 dark:bg-red-950/30"
                    : account.is_active
                    ? "bg-gray-100 dark:bg-gray-800"
                    : "hover:bg-gray-100 dark:hover:bg-gray-800"
                }`}
              >
                <span className="mt-0.5 flex h-4 w-4 shrink-0 items-center justify-center">
                  {account.is_active && (
                    <svg
                      className={`h-4 w-4 ${requiresReauth ? "text-red-500" : "text-emerald-500"}`}
                      viewBox="0 0 20 20"
                      fill="currentColor"
                    >
                      <path
                        fillRule="evenodd"
                        d="M16.7 5.3a1 1 0 010 1.4l-7.5 7.5a1 1 0 01-1.4 0L3.3 9.7a1 1 0 011.4-1.4l3.3 3.3 6.8-6.8a1 1 0 011.4 0z"
                        clipRule="evenodd"
                      />
                    </svg>
                  )}
                </span>
                <span className="min-w-0 flex-1">
                  <span className="flex items-center gap-1.5">
                    <span className="min-w-0 flex-1 truncate text-sm font-medium">
                      {account.name}
                    </span>
                    {plan && (
                      <span className="shrink-0 rounded bg-gray-200 px-1.5 py-0.5 text-[10px] font-medium text-gray-700 dark:bg-gray-700 dark:text-gray-200">
                        {plan}
                      </span>
                    )}
                  </span>
                  {requiresReauth ? (
                    <span className="block truncate text-xs text-red-500 dark:text-red-400">
                      {t("sessionExpired")}
                    </span>
                  ) : windows.length > 0 ? (
                    <span className="mt-1.5 block space-y-1.5">
                      {windows.map((w) => {
                        const remaining = Math.max(0, 100 - w.used);
                        const tone = remainingTone(remaining);
                        const reset = formatResetAt(w.resetAt);
                        return (
                          <span key={w.label} className="block">
                            <span className="flex items-center gap-1">
                              <span className="text-[11px] font-medium text-gray-700 dark:text-gray-200">
                                {w.label}
                              </span>
                              <span
                                className={`h-1.5 w-1.5 rounded-full ${tone.dot}`}
                              />
                            </span>
                            <span className="mt-0.5 block h-1.5 w-full overflow-hidden rounded-full bg-gray-200 dark:bg-gray-800">
                              <span
                                className={`block h-full rounded-full ${tone.bar}`}
                                style={{ width: `${Math.min(remaining, 100)}%` }}
                              />
                            </span>
                            <span className="mt-0.5 flex justify-between text-[11px] text-gray-500 dark:text-gray-400">
                              <span className={tone.text}>
                                {remaining.toFixed(0)}% {t("left")}
                              </span>
                              {reset && (
                                <span>
                                  {reset === "now" ? t("resetsNowShort") : t("resetsIn", { value: reset })}
                                </span>
                              )}
                            </span>
                          </span>
                        );
                      })}
                    </span>
                  ) : usage?.error ? (
                    <span className="block truncate text-xs text-red-500 dark:text-red-400">
                      {t("usageUnavailableShort")}
                    </span>
                  ) : account.email ? (
                    <span className="block truncate text-xs text-gray-500 dark:text-gray-400">
                      {account.email}
                    </span>
                  ) : null}
                  {account.is_active && stats?.available && (
                    <span className="mt-2 grid grid-cols-2 gap-1.5">
                      <span className="rounded-md bg-white px-2 py-1 text-[11px] text-gray-600 shadow-sm dark:bg-gray-950 dark:text-gray-300">
                        <span className="block font-medium text-gray-900 dark:text-gray-100">
                          {formatTokens(sumDailyTokens(stats, 1))}
                        </span>
                        <span>{t("today")}</span>
                      </span>
                      <span className="rounded-md bg-white px-2 py-1 text-[11px] text-gray-600 shadow-sm dark:bg-gray-950 dark:text-gray-300">
                        <span className="block font-medium text-gray-900 dark:text-gray-100">
                          {formatTokens(sumDailyTokens(stats, 7))}
                        </span>
                        <span>{t("last7Short")}</span>
                      </span>
                    </span>
                  )}
                </span>
                {switchingId === account.id && (
                  <span className="shrink-0 text-xs text-gray-400">...</span>
                )}
              </button>
            );
          })
        )}
      </div>

      {error && (
        <div className="border-t border-gray-100 px-3 py-2 text-xs text-red-600 dark:border-gray-800 dark:text-red-400">
          {error}
        </div>
      )}

      {dockDisplayMode && (
        <div className="flex items-center gap-1 border-t border-gray-100 px-1.5 py-1.5 dark:border-gray-800">
          <span className="px-1.5 text-[11px] font-medium text-gray-500 dark:text-gray-400">
            {t("dock")}
          </span>
          <button
            onClick={() => void handleDockDisplayMode("show_in_dock")}
            className={`rounded-md px-2 py-1 text-[11px] font-semibold transition-colors ${
              dockDisplayMode === "show_in_dock"
                ? "bg-gray-900 text-white dark:bg-gray-100 dark:text-gray-900"
                : "bg-gray-100 text-gray-700 hover:bg-gray-200 dark:bg-gray-800 dark:text-gray-200 dark:hover:bg-gray-700"
            }`}
          >
            {t("show")}
          </button>
          <button
            onClick={() => void handleDockDisplayMode("menu_bar_only")}
            className={`rounded-md px-2 py-1 text-[11px] font-semibold transition-colors ${
              dockDisplayMode === "menu_bar_only"
                ? "bg-gray-900 text-white dark:bg-gray-100 dark:text-gray-900"
                : "bg-gray-100 text-gray-700 hover:bg-gray-200 dark:bg-gray-800 dark:text-gray-200 dark:hover:bg-gray-700"
            }`}
          >
            {t("menuBar")}
          </button>
        </div>
      )}

      <div className="flex items-center gap-1 border-t border-gray-100 p-1.5 dark:border-gray-800">
        <button
          onClick={() => void invokeBackend("open_main_window")}
          className="flex-1 rounded-lg px-2 py-1.5 text-left text-sm transition-colors hover:bg-gray-100 dark:hover:bg-gray-800"
        >
          {t("openSwitcher")}
        </button>
        <button
          onClick={() => void invokeBackend("quit_app")}
          className="rounded-lg px-2 py-1.5 text-sm text-gray-500 transition-colors hover:bg-gray-100 hover:text-red-600 dark:text-gray-400 dark:hover:bg-gray-800 dark:hover:text-red-400"
        >
          {t("quit")}
        </button>
      </div>
    </div>
  );
}

export default TrayMenu;
