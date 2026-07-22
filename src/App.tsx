import { useState, useEffect, useCallback, useMemo, useRef } from "react";
import { useTranslation } from "react-i18next";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useAccounts } from "./hooks/useAccounts";
import { useForceCloseCodexProcesses } from "./hooks/useForceCloseCodexProcesses";
import { AccountCard, AddAccountModal, UpdateChecker } from "./components";
import type { AccountWithUsage, CodexProcessInfo, DockDisplayMode, UsageInfo } from "./types";
import {
  exportFullBackupFile,
  importFullBackupFile,
  isTauriRuntime,
  invokeBackend,
} from "./lib/platform";
import {
  applyTheme,
  readStoredTheme,
  THEME_CHANGED_EVENT,
  THEME_STORAGE_KEY,
  type ThemeMode,
} from "./lib/theme";
import {
  AUTO_WARMUP_ACCOUNTS_STORAGE_KEY,
  AUTO_WARMUP_ALL_CHANGED_EVENT,
  AUTO_WARMUP_LEDGER_STORAGE_KEY,
  TIMED_WARMUP_LEDGER_STORAGE_KEY,
  normalizeTimedWarmupTimes,
  readAutoWarmupAllEnabled,
  readTimedWarmupEnabled,
  readTimedWarmupTimes,
  writeAutoWarmupAllEnabled,
  writeTimedWarmupEnabled,
  writeTimedWarmupTimes,
} from "./lib/autoWarmup";
import {
  getAutoWarmupWindowKey,
  getAutoWarmupWindowKind,
  getDueAutoWarmupWindow,
  type AutoWarmupWindow,
  type AutoWarmupWindowKind,
} from "./lib/autoWarmupPolicy";
import "./App.css";
import {
  changeAppLanguage,
  getLanguagePreference,
  subscribeLanguagePreference,
  supportedLanguages,
  SYSTEM_LANGUAGE,
  type AppLanguage,
} from "./i18n";

const AUTO_WARMUP_CHECK_INTERVAL_MS = 30 * 1000;
const AUTO_WARMUP_RETRY_BACKOFF_MS = 60 * 1000;
const LIMIT_FULL_THRESHOLD = 99.5;
const SWITCH_ACCOUNT_BLOCKED_EVENT = "switch-account-blocked";
const CLOSE_BEHAVIOR_REQUESTED_EVENT = "close-behavior-requested";
interface SwitchAccountBlockedPayload {
  accountId?: string;
  error?: string;
}
interface CloseBehaviorRequestedPayload {
  requestId?: number;
}
type AutoWarmupLedger = Record<
  string,
  {
    lastSuccessfulWarmupAt?: number;
    lastAutoWindowKey?: string;
    lastAutoWindowKind?: AutoWarmupWindowKind;
  }
>;
const appWindow = getCurrentWindow();
const isMacOs =
  typeof navigator !== "undefined" &&
  /(Mac|iPhone|iPod|iPad)/i.test(navigator.userAgent);

function readStoredStringArray(key: string): string[] {
  if (typeof window === "undefined") return [];
  try {
    const parsed = JSON.parse(window.localStorage.getItem(key) ?? "[]");
    return Array.isArray(parsed) ? parsed.filter((item) => typeof item === "string") : [];
  } catch {
    return [];
  }
}

function readStoredAutoWarmupLedger(): AutoWarmupLedger {
  if (typeof window === "undefined") return {};
  try {
    const parsed = JSON.parse(window.localStorage.getItem(AUTO_WARMUP_LEDGER_STORAGE_KEY) ?? "{}");
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return {};

    const entries: Array<[string, AutoWarmupLedger[string]]> = [];
    for (const [accountId, value] of Object.entries(parsed)) {
      if (!value || typeof value !== "object" || Array.isArray(value)) continue;

      const entry: AutoWarmupLedger[string] = {};
      if (
        "lastSuccessfulWarmupAt" in value &&
        typeof value.lastSuccessfulWarmupAt === "number"
      ) {
        entry.lastSuccessfulWarmupAt = value.lastSuccessfulWarmupAt;
      }
      if ("lastAutoWindowKey" in value && typeof value.lastAutoWindowKey === "string") {
        entry.lastAutoWindowKey = value.lastAutoWindowKey;
      }
      if (
        "lastAutoWindowKind" in value &&
        (value.lastAutoWindowKind === "session" || value.lastAutoWindowKind === "weekly")
      ) {
        entry.lastAutoWindowKind = value.lastAutoWindowKind;
      }

      if (Object.keys(entry).length > 0) entries.push([accountId, entry]);
    }
    return Object.fromEntries(entries);
  } catch {
    return {};
  }
}

function readStoredTimedWarmupLedger(): Record<string, string> {
  if (typeof window === "undefined") return {};
  try {
    const parsed = JSON.parse(window.localStorage.getItem(TIMED_WARMUP_LEDGER_STORAGE_KEY) ?? "{}");
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return {};
    return Object.fromEntries(
      Object.entries(parsed).filter(
        (entry): entry is [string, string] =>
          typeof entry[0] === "string" && typeof entry[1] === "string"
      )
    );
  } catch {
    return {};
  }
}

function isLimitFull(usedPercent: number | null | undefined): boolean {
  return usedPercent !== null && usedPercent !== undefined && usedPercent >= LIMIT_FULL_THRESHOLD;
}

function getPreferredUsedPercent(usage: UsageInfo | undefined): number | null | undefined {
  return usage?.primary_used_percent ?? usage?.secondary_used_percent;
}

function getPreferredResetsAt(usage: UsageInfo | undefined): number | null | undefined {
  return usage?.primary_resets_at ?? usage?.secondary_resets_at;
}

function getTimedWarmupTargets(accounts: AccountWithUsage[]): AccountWithUsage[] {
  return accounts.filter(
    (account) =>
      account.usage &&
      !account.usageLoading &&
      !account.usage.error &&
      !isLimitFull(account.usage.secondary_used_percent)
  );
}

function App() {
  const { t } = useTranslation();
  const {
    accounts,
    loading,
    error,
    loadAccounts,
    refreshUsage,
    refreshSingleUsage,
    warmupAccount,
    warmupAllAccounts,
    switchAccount,
    deleteAccount,
    renameAccount,
    importFromFile,
    exportAccountsSlimText,
    importAccountsSlimText,
    startOAuthLogin,
    completeOAuthLogin,
    cancelOAuthLogin,
    loadMaskedAccountIds,
    saveMaskedAccountIds,
  } = useAccounts();

  const [isAddModalOpen, setIsAddModalOpen] = useState(false);
  const [isConfigModalOpen, setIsConfigModalOpen] = useState(false);
  const [configModalMode, setConfigModalMode] = useState<"slim_export" | "slim_import">(
    "slim_export"
  );
  const [configPayload, setConfigPayload] = useState("");
  const [configModalError, setConfigModalError] = useState<string | null>(null);
  const [configCopied, setConfigCopied] = useState(false);
  const [switchingId, setSwitchingId] = useState<string | null>(null);
  const [deleteConfirmId, setDeleteConfirmId] = useState<string | null>(null);
  const [processInfo, setProcessInfo] = useState<CodexProcessInfo | null>(null);
  const [pendingTraySwitchAccountId, setPendingTraySwitchAccountId] = useState<string | null>(null);
  const [isRefreshing, setIsRefreshing] = useState(false);
  const [isOpeningCodex, setIsOpeningCodex] = useState(false);
  const [isExportingSlim, setIsExportingSlim] = useState(false);
  const [isImportingSlim, setIsImportingSlim] = useState(false);
  const [isExportingFull, setIsExportingFull] = useState(false);
  const [isImportingFull, setIsImportingFull] = useState(false);
  const [isWarmingAll, setIsWarmingAll] = useState(false);
  const [warmingUpId, setWarmingUpId] = useState<string | null>(null);
  const [refreshSuccess, setRefreshSuccess] = useState(false);
  const [warmupToast, setWarmupToast] = useState<{
    message: string;
    isError: boolean;
  } | null>(null);
  const [autoWarmupAllEnabled, setAutoWarmupAllEnabled] = useState(() => {
    return readAutoWarmupAllEnabled();
  });
  const [autoWarmupAccountIds, setAutoWarmupAccountIds] = useState<Set<string>>(
    () => new Set(readStoredStringArray(AUTO_WARMUP_ACCOUNTS_STORAGE_KEY))
  );
  const [autoWarmupLedger, setAutoWarmupLedger] =
    useState<AutoWarmupLedger>(() => readStoredAutoWarmupLedger());
  const [autoWarmupRunningIds, setAutoWarmupRunningIds] = useState<Set<string>>(
    new Set()
  );
  const [timedWarmupEnabled, setTimedWarmupEnabled] = useState(() =>
    readTimedWarmupEnabled()
  );
  const [timedWarmupTimes, setTimedWarmupTimes] = useState<string[]>(() =>
    readTimedWarmupTimes()
  );
  const [isTimedWarmupOpen, setIsTimedWarmupOpen] = useState(false);
  const [timedWarmupRunning, setTimedWarmupRunning] = useState(false);
  const [timedWarmupDraft, setTimedWarmupDraft] = useState("");
  const [maskedAccounts, setMaskedAccounts] = useState<Set<string>>(new Set());
  const [otherAccountsSort, setOtherAccountsSort] = useState<
    | "deadline_asc"
    | "deadline_desc"
    | "remaining_desc"
    | "remaining_asc"
    | "subscription_asc"
    | "subscription_desc"
  >("deadline_asc");
  const [isActionsMenuOpen, setIsActionsMenuOpen] = useState(false);
  const actionsMenuRef = useRef<HTMLDivElement | null>(null);
  const timedWarmupRef = useRef<HTMLDivElement | null>(null);
  const [themeMode, setThemeMode] = useState<ThemeMode>(readStoredTheme);
  const [languagePreference, setLanguagePreference] = useState<AppLanguage>(
    getLanguagePreference
  );
  const [isWindowMaximized, setIsWindowMaximized] = useState(false);
  const [closeBehaviorPromptOpen, setCloseBehaviorPromptOpen] = useState(false);
  const [closeBehaviorDontAskAgain, setCloseBehaviorDontAskAgain] = useState(false);
  const [isCompletingCloseBehavior, setIsCompletingCloseBehavior] = useState(false);
  const accountsRef = useRef(accounts);
  const autoWarmupAccountIdsRef = useRef(autoWarmupAccountIds);
  const autoWarmupLedgerRef = useRef(autoWarmupLedger);
  const autoWarmupRunningIdsRef = useRef(autoWarmupRunningIds);
  const autoWarmupRetryAfterRef = useRef<Record<string, number>>({});
  const timedWarmupRunningRef = useRef(timedWarmupRunning);
  // Tracks the last calendar date (YYYY-MM-DD) each scheduled time fired on,
  // so each time triggers at most once per day.
  const timedWarmupLastFireRef = useRef<Record<string, string>>(readStoredTimedWarmupLedger());

  useEffect(() => {
    accountsRef.current = accounts;
  }, [accounts]);

  useEffect(() => {
    autoWarmupAccountIdsRef.current = autoWarmupAccountIds;
  }, [autoWarmupAccountIds]);

  useEffect(() => {
    autoWarmupRunningIdsRef.current = autoWarmupRunningIds;
  }, [autoWarmupRunningIds]);

  useEffect(() => {
    timedWarmupRunningRef.current = timedWarmupRunning;
  }, [timedWarmupRunning]);

  useEffect(() => {
    try {
      writeTimedWarmupEnabled(timedWarmupEnabled);
    } catch {
      // Ignore storage errors; timed warm-up still works for the current session.
    }
  }, [timedWarmupEnabled]);

  useEffect(() => {
    try {
      writeTimedWarmupTimes(timedWarmupTimes);
    } catch {
      // Ignore storage errors; timed warm-up still works for the current session.
    }
  }, [timedWarmupTimes]);

  useEffect(() => {
    if (loading || error) return;

    const validAccountIds = new Set(accounts.map((account) => account.id));

    setAutoWarmupAccountIds((prev) => {
      const next = new Set(Array.from(prev).filter((id) => validAccountIds.has(id)));
      return next.size === prev.size ? prev : next;
    });

    setAutoWarmupLedger((prev) => {
      const next = Object.fromEntries(
        Object.entries(prev).filter(([accountId]) => validAccountIds.has(accountId))
      );
      return Object.keys(next).length === Object.keys(prev).length ? prev : next;
    });

    for (const accountId of Object.keys(autoWarmupRetryAfterRef.current)) {
      if (!validAccountIds.has(accountId)) {
        delete autoWarmupRetryAfterRef.current[accountId];
      }
    }
  }, [accounts, error, loading]);

  useEffect(() => {
    autoWarmupLedgerRef.current = autoWarmupLedger;
    try {
      window.localStorage.setItem(
        AUTO_WARMUP_LEDGER_STORAGE_KEY,
        JSON.stringify(autoWarmupLedger)
      );
    } catch {
      // Ignore storage errors; auto warm-up still works for the current session.
    }
  }, [autoWarmupLedger]);

  useEffect(() => {
    try {
      writeAutoWarmupAllEnabled(autoWarmupAllEnabled);
    } catch {
      // Ignore storage errors; auto warm-up still works for the current session.
    }

    if (isTauriRuntime()) {
      void import("@tauri-apps/api/event")
        .then(({ emit }) => emit(AUTO_WARMUP_ALL_CHANGED_EVENT, autoWarmupAllEnabled))
        .catch((err) => console.error("Failed to sync tray auto warm-up:", err));
    }
  }, [autoWarmupAllEnabled]);

  useEffect(() => {
    try {
      window.localStorage.setItem(
        AUTO_WARMUP_ACCOUNTS_STORAGE_KEY,
        JSON.stringify(Array.from(autoWarmupAccountIds))
      );
    } catch {
      // Ignore storage errors; auto warm-up still works for the current session.
    }
  }, [autoWarmupAccountIds]);

  const handleTitlebarDrag = useCallback(
    (event: React.MouseEvent<HTMLDivElement>) => {
      if (!isTauriRuntime() || event.button !== 0) return;
      void appWindow.startDragging();
    },
    []
  );

  const handleTitlebarDoubleClick = useCallback(() => {
    if (!isTauriRuntime()) return;
    void appWindow.toggleMaximize();
  }, []);

  const toggleMask = (accountId: string) => {
    setMaskedAccounts((prev) => {
      const next = new Set(prev);
      if (next.has(accountId)) {
        next.delete(accountId);
      } else {
        next.add(accountId);
      }
      void saveMaskedAccountIds(Array.from(next));
      return next;
    });
  };

  const allMasked =
    accounts.length > 0 && accounts.every((account) => maskedAccounts.has(account.id));

  const toggleMaskAll = () => {
    setMaskedAccounts((prev) => {
      const shouldMaskAll = !accounts.every((account) => prev.has(account.id));
      const next = shouldMaskAll ? new Set(accounts.map((account) => account.id)) : new Set<string>();
      void saveMaskedAccountIds(Array.from(next));
      return next;
    });
  };

  const checkProcesses = useCallback(async () => {
    try {
      const info = await invokeBackend<CodexProcessInfo>("check_codex_processes");
      setProcessInfo((prev) => {
        if (
          prev &&
          prev.can_switch === info.can_switch &&
          prev.count === info.count &&
          prev.background_count === info.background_count &&
          prev.pids.length === info.pids.length &&
          prev.pids.every((pid, index) => pid === info.pids[index])
        ) {
          return prev;
        }
        return info;
      });
      return info;
    } catch (err) {
      console.error("Failed to check processes:", err);
      return null;
    }
  }, []);

  // Check processes on mount and periodically
  useEffect(() => {
    checkProcesses();
    const interval = setInterval(checkProcesses, 5000);
    return () => clearInterval(interval);
  }, [checkProcesses]);

  // Load masked accounts from storage on mount
  useEffect(() => {
    loadMaskedAccountIds().then((ids) => {
      if (ids.length > 0) {
        setMaskedAccounts(new Set(ids));
      }
    });
  }, [loadMaskedAccountIds]);

  useEffect(() => {
    if (!isActionsMenuOpen) return;

    const handleClickOutside = (event: MouseEvent) => {
      if (!actionsMenuRef.current) return;
      if (!actionsMenuRef.current.contains(event.target as Node)) {
        setIsActionsMenuOpen(false);
      }
    };

    document.addEventListener("mousedown", handleClickOutside);
    return () => document.removeEventListener("mousedown", handleClickOutside);
  }, [isActionsMenuOpen]);

  useEffect(() => {
    if (!isTimedWarmupOpen) return;

    const handleClickOutside = (event: MouseEvent) => {
      if (!timedWarmupRef.current) return;
      if (!timedWarmupRef.current.contains(event.target as Node)) {
        setIsTimedWarmupOpen(false);
      }
    };

    document.addEventListener("mousedown", handleClickOutside);
    return () => document.removeEventListener("mousedown", handleClickOutside);
  }, [isTimedWarmupOpen]);

  useEffect(() => {
    applyTheme(themeMode);
    try {
      window.localStorage.setItem(THEME_STORAGE_KEY, themeMode);
    } catch {
      // Ignore storage errors; theme still works for current session.
    }

    if (isTauriRuntime()) {
      void import("@tauri-apps/api/event")
        .then(({ emit }) => emit(THEME_CHANGED_EVENT, themeMode))
        .catch((err) => console.error("Failed to sync tray theme:", err));
    }
  }, [themeMode]);

  useEffect(
    () => subscribeLanguagePreference(setLanguagePreference),
    []
  );

  useEffect(() => {
    if (!isTauriRuntime() || isMacOs) return;

    let unlisten: (() => void) | undefined;

    const syncMaximizedState = async () => {
      try {
        setIsWindowMaximized(await appWindow.isMaximized());
      } catch (err) {
        console.error("Failed to read window state:", err);
      }
    };

    void syncMaximizedState();

    appWindow
      .onResized(() => {
        void syncMaximizedState();
      })
      .then((fn) => {
        unlisten = fn;
      })
      .catch((err) => {
        console.error("Failed to watch window resize:", err);
      });

    return () => {
      unlisten?.();
    };
  }, []);

  const handleSwitch = async (accountId: string) => {
    // Check processes before switching
    const latestProcessInfo = await checkProcesses();
    if (latestProcessInfo && !latestProcessInfo.can_switch) {
      return;
    }

    try {
      setSwitchingId(accountId);
      await switchAccount(accountId);
    } catch (err) {
      console.error("Failed to switch account:", err);
    } finally {
      setSwitchingId(null);
    }
  };

  const handleDelete = async (accountId: string) => {
    if (deleteConfirmId !== accountId) {
      setDeleteConfirmId(accountId);
      setTimeout(() => setDeleteConfirmId(null), 3000);
      return;
    }

    try {
      await deleteAccount(accountId);
      setDeleteConfirmId(null);
    } catch (err) {
      console.error("Failed to delete account:", err);
    }
  };

  const handleRefresh = async () => {
    setIsRefreshing(true);
    setRefreshSuccess(false);
    try {
      await refreshUsage(undefined, { refreshMetadata: true });
      setRefreshSuccess(true);
      setTimeout(() => setRefreshSuccess(false), 2000);
    } finally {
      setIsRefreshing(false);
    }
  };

  const showWarmupToast = useCallback((message: string, isError = false) => {
    setWarmupToast({ message, isError });
    setTimeout(() => setWarmupToast(null), 2500);
  }, []);

  const handleLanguageChange = useCallback(async (language: AppLanguage) => {
    try {
      await changeAppLanguage(language);
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      showWarmupToast(t("language.changeFailed", { error: message }), true);
    }
  }, [showWarmupToast, t]);

  const formatWarmupError = useCallback((err: unknown) => {
    if (!err) return t("common.unknownError");
    if (err instanceof Error && err.message) return err.message;
    if (typeof err === "string") return err;
    try {
      return JSON.stringify(err);
    } catch {
      return t("common.unknownError");
    }
  }, [t]);

  const markSuccessfulWarmup = useCallback(
    (accountId: string, timestamp = Date.now(), window?: AutoWarmupWindow) => {
      delete autoWarmupRetryAfterRef.current[accountId];
      setAutoWarmupLedger((prev) => ({
        ...prev,
        [accountId]: {
          lastSuccessfulWarmupAt: timestamp,
          ...(window
            ? {
                lastAutoWindowKey: getAutoWarmupWindowKey(window),
                lastAutoWindowKind: window.kind,
              }
            : {}),
        },
      }));
    },
    []
  );

  const {
    forceCloseConfirmOpen,
    setForceCloseConfirmOpen,
    isForceClosingCodex,
    forceCloseCodexProcesses,
  } = useForceCloseCodexProcesses({
    processCount: processInfo?.count ?? 0,
    checkProcesses,
    showToast: showWarmupToast,
    formatError: formatWarmupError,
  });

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let unlistenAutoWarmup: (() => void) | undefined;
    let unlistenCloseBehavior: (() => void) | undefined;

    void (async () => {
      if (!isTauriRuntime()) return;
      const { listen } = await import("@tauri-apps/api/event");
      unlisten = await listen<SwitchAccountBlockedPayload>(
        SWITCH_ACCOUNT_BLOCKED_EVENT,
        async (event) => {
          const latestProcessInfo = await checkProcesses();
          const accountId = event.payload?.accountId;

          if (accountId && latestProcessInfo && !latestProcessInfo.can_switch) {
            setPendingTraySwitchAccountId(accountId);
            setForceCloseConfirmOpen(true);
            return;
          }

          if (accountId && latestProcessInfo?.can_switch) {
            try {
              setSwitchingId(accountId);
              await switchAccount(accountId);
              setPendingTraySwitchAccountId(null);
              showWarmupToast(t("warmup.switchedFromTray"));
            } catch (err) {
              console.error("Failed to retry tray account switch:", err);
              showWarmupToast(t("warmup.switchFailed", { error: formatWarmupError(err) }), true);
            } finally {
              setSwitchingId(null);
            }
            return;
          }

          showWarmupToast(
            event.payload?.error || t("warmup.switchBlocked"),
            true
          );
        }
      );
      unlistenAutoWarmup = await listen<boolean>(
        AUTO_WARMUP_ALL_CHANGED_EVENT,
        ({ payload }) => {
          if (typeof payload === "boolean") {
            setAutoWarmupAllEnabled(payload);
          }
        }
      );
      unlistenCloseBehavior = await listen<CloseBehaviorRequestedPayload>(
        CLOSE_BEHAVIOR_REQUESTED_EVENT,
        ({ payload }) => {
          const requestId = payload?.requestId;
          if (typeof requestId === "number") {
            void invokeBackend("ack_close_behavior_prompt", { requestId });
          }
          setCloseBehaviorDontAskAgain(false);
          setCloseBehaviorPromptOpen(true);
        }
      );
    })();

    return () => {
      unlisten?.();
      unlistenAutoWarmup?.();
      unlistenCloseBehavior?.();
    };
  }, [checkProcesses, formatWarmupError, setForceCloseConfirmOpen, showWarmupToast, switchAccount, t]);

  const handleCloseBehaviorChoice = useCallback(
    async (mode: DockDisplayMode) => {
      try {
        setIsCompletingCloseBehavior(true);
        await invokeBackend("complete_close_behavior", {
          mode,
          dontAskAgain: closeBehaviorDontAskAgain,
        });
        setCloseBehaviorPromptOpen(false);
      } catch (err) {
        console.error("Failed to complete close behavior:", err);
        showWarmupToast(t("warmup.closeFailed", { error: formatWarmupError(err) }), true);
      } finally {
        setIsCompletingCloseBehavior(false);
      }
    },
    [closeBehaviorDontAskAgain, formatWarmupError, showWarmupToast, t]
  );

  const handleForceCloseConfirm = useCallback(async () => {
    const accountId = pendingTraySwitchAccountId;
    const latestProcessInfo = await forceCloseCodexProcesses();

    if (!accountId) {
      return;
    }

    if (!latestProcessInfo?.can_switch) {
      setPendingTraySwitchAccountId(null);
      return;
    }

    try {
      setSwitchingId(accountId);
      await switchAccount(accountId);
      setPendingTraySwitchAccountId(null);
      showWarmupToast(t("warmup.switchedAfterClose"));
    } catch (err) {
      console.error("Failed to switch account after force close:", err);
      setPendingTraySwitchAccountId(null);
      showWarmupToast(
        t("warmup.switchAfterCloseFailed", { error: formatWarmupError(err) }),
        true
      );
    } finally {
      setSwitchingId(null);
    }
  }, [
    forceCloseCodexProcesses,
    formatWarmupError,
    pendingTraySwitchAccountId,
    showWarmupToast,
    switchAccount,
    t,
  ]);

  const handleWarmupAccount = async (accountId: string, accountName: string) => {
    try {
      setWarmingUpId(accountId);
      await warmupAccount(accountId);
      markSuccessfulWarmup(accountId);
      showWarmupToast(t("warmup.sentFor", { name: accountName }));
    } catch (err) {
      console.error("Failed to warm up account:", err);
      showWarmupToast(
        t("warmup.failedFor", { name: accountName, error: formatWarmupError(err) }),
        true
      );
    } finally {
      setWarmingUpId(null);
    }
  };

  const handleWarmupAll = async () => {
    try {
      setIsWarmingAll(true);
      const summary = await warmupAllAccounts();
      if (summary.total_accounts === 0) {
        showWarmupToast(t("warmup.noneAvailable"), true);
        return;
      }

      const warmedAt = Date.now();
      const failedAccountIds = new Set(summary.failed_account_ids);
      accounts.forEach((account) => {
        if (!failedAccountIds.has(account.id)) {
          markSuccessfulWarmup(account.id, warmedAt);
        }
      });

      if (summary.failed_account_ids.length === 0) {
        showWarmupToast(
          t("warmup.sentForAll", { count: summary.warmed_accounts })
        );
      } else {
        showWarmupToast(
          t("warmup.summary", {
            warmed: summary.warmed_accounts,
            total: summary.total_accounts,
            failed: summary.failed_account_ids.length,
          }),
          true
        );
      }
    } catch (err) {
      console.error("Failed to warm up all accounts:", err);
      showWarmupToast(t("warmup.allFailed", { error: formatWarmupError(err) }), true);
    } finally {
      setIsWarmingAll(false);
    }
  };

  const toggleAutoWarmupAccount = (accountId: string) => {
    setAutoWarmupAccountIds((prev) => {
      const next = new Set(prev);
      if (next.has(accountId)) {
        next.delete(accountId);
      } else {
        next.add(accountId);
      }
      return next;
    });
  };

  const getDueAutoWarmupForAccount = useCallback(
    (accountId: string, usage: UsageInfo | undefined) => {
      return getDueAutoWarmupWindow(usage, autoWarmupLedgerRef.current[accountId]);
    },
    []
  );

  const getAutoWarmupLabel = useCallback(
    (
      usage: UsageInfo | undefined,
      isEnabled: boolean,
      isRunning: boolean
    ) => {
      if (isRunning) return t("warmup.warming");
      if (!isEnabled) return t("warmup.autoOff");
      if (!usage || usage.error) return t("warmup.autoOn");

      const windowKind = getAutoWarmupWindowKind(usage);
      if (windowKind === "session" && isLimitFull(usage.secondary_used_percent)) {
        return t("warmup.waitingWeekly");
      }
      if (windowKind === "session") return t("warmup.autoSession");
      if (windowKind === "weekly") return t("warmup.autoWeekly");

      return t("warmup.autoOn");
    },
    [t]
  );

  const headerAutoWarmupLabel = useMemo(() => {
    if (autoWarmupRunningIds.size > 0) return t("warmup.autoWarming");
    return autoWarmupAllEnabled || autoWarmupAccountIds.size > 0
      ? t("warmup.autoOn")
      : t("warmup.autoOff");
  }, [autoWarmupAccountIds.size, autoWarmupAllEnabled, autoWarmupRunningIds, t]);

  const timedWarmupTargetsReady = useMemo(
    () =>
      accounts.length > 0 &&
      accounts.every((account) => account.usage && !account.usageLoading),
    [accounts]
  );

  const timedWarmupTargetCount = useMemo(
    () => getTimedWarmupTargets(accounts).length,
    [accounts]
  );

  const backOffAutoWarmupRetry = useCallback((accountId: string) => {
    autoWarmupRetryAfterRef.current[accountId] =
      Date.now() + AUTO_WARMUP_RETRY_BACKOFF_MS;
  }, []);

  const runAutoWarmupForAccount = useCallback(
    async (accountId: string, accountName: string) => {
      setAutoWarmupRunningIds((prev) => new Set(prev).add(accountId));

      try {
        let freshUsage: UsageInfo;
        try {
          freshUsage = await refreshSingleUsage(accountId);
        } catch (err) {
          console.error("Auto warm-up usage refresh failed:", err);
          backOffAutoWarmupRetry(accountId);
          return;
        }

        const window = getDueAutoWarmupForAccount(accountId, freshUsage);
        if (!window) return;

        await warmupAccount(accountId);
        markSuccessfulWarmup(accountId, Date.now(), window);
        showWarmupToast(
          t(
            window.kind === "session"
              ? "warmup.autoSessionSentFor"
              : "warmup.autoWeeklySentFor",
            { name: accountName }
          )
        );
      } catch (err) {
        console.error("Auto warm-up failed:", err);
        backOffAutoWarmupRetry(accountId);
        showWarmupToast(
          t("warmup.autoFailedFor", {
            name: accountName,
            error: formatWarmupError(err),
          }),
          true
        );
      } finally {
        setAutoWarmupRunningIds((prev) => {
          const next = new Set(prev);
          next.delete(accountId);
          return next;
        });
      }
    },
    [
      backOffAutoWarmupRetry,
      formatWarmupError,
      getDueAutoWarmupForAccount,
      markSuccessfulWarmup,
      refreshSingleUsage,
      showWarmupToast,
      t,
      warmupAccount,
    ]
  );

  useEffect(() => {
    if (!autoWarmupAllEnabled && autoWarmupAccountIds.size === 0) return;

    const checkAutoWarmup = () => {
      for (const account of accountsRef.current) {
        const autoEnabled =
          autoWarmupAllEnabled || autoWarmupAccountIdsRef.current.has(account.id);
        if (!autoEnabled || autoWarmupRunningIdsRef.current.has(account.id)) continue;

        const retryAfter = autoWarmupRetryAfterRef.current[account.id];
        if (retryAfter && Date.now() < retryAfter) continue;

        if (!getDueAutoWarmupForAccount(account.id, account.usage)) continue;

        void runAutoWarmupForAccount(account.id, account.name);
      }
    };

    checkAutoWarmup();
    const interval = window.setInterval(
      checkAutoWarmup,
      AUTO_WARMUP_CHECK_INTERVAL_MS
    );

    return () => window.clearInterval(interval);
  }, [
    autoWarmupAccountIds.size,
    autoWarmupAllEnabled,
    getDueAutoWarmupForAccount,
    runAutoWarmupForAccount,
  ]);

  const runTimedWarmup = useCallback(async () => {
    const targets = getTimedWarmupTargets(accountsRef.current);
    if (targets.length === 0) return;

    setTimedWarmupRunning(true);
    try {
      const warmedAt = Date.now();
      let warmed = 0;
      let failed = 0;
      for (const account of targets) {
        try {
          await warmupAccount(account.id);
          markSuccessfulWarmup(account.id, warmedAt);
          warmed += 1;
        } catch (err) {
          console.error("Timed warm-up failed:", err);
          failed += 1;
        }
      }

      if (failed === 0) {
        showWarmupToast(t("warmup.timedSent", { count: warmed }));
      } else {
        showWarmupToast(t("warmup.timedSummary", { warmed, failed }), true);
      }
    } finally {
      setTimedWarmupRunning(false);
    }
  }, [markSuccessfulWarmup, showWarmupToast, t, warmupAccount]);

  useEffect(() => {
    if (!timedWarmupEnabled || timedWarmupTimes.length === 0) return;

    const checkTimedWarmup = () => {
      if (timedWarmupRunningRef.current) return;

      const now = new Date();
      const todayKey = `${now.getFullYear()}-${now.getMonth()}-${now.getDate()}`;
      const currentTime = `${String(now.getHours()).padStart(2, "0")}:${String(
        now.getMinutes()
      ).padStart(2, "0")}`;

      // Only fire during the scheduled minute itself; a missed time (e.g. while
      // asleep) is skipped rather than warmed late at the wrong moment.
      if (!timedWarmupTimes.includes(currentTime)) return;
      if (timedWarmupLastFireRef.current[currentTime] === todayKey) return;
      if (!timedWarmupTargetsReady || timedWarmupTargetCount === 0) return;

      // Mark before running so a slow warm-up can't double-fire on the next tick.
      timedWarmupLastFireRef.current[currentTime] = todayKey;
      try {
        window.localStorage.setItem(
          TIMED_WARMUP_LEDGER_STORAGE_KEY,
          JSON.stringify(timedWarmupLastFireRef.current)
        );
      } catch {
        // Ignore storage errors; timed warm-up still works for the current session.
      }
      void runTimedWarmup();
    };

    checkTimedWarmup();
    const interval = window.setInterval(
      checkTimedWarmup,
      AUTO_WARMUP_CHECK_INTERVAL_MS
    );

    return () => window.clearInterval(interval);
  }, [
    timedWarmupEnabled,
    timedWarmupTimes,
    timedWarmupTargetsReady,
    timedWarmupTargetCount,
    runTimedWarmup,
  ]);

  const handleAddTimedWarmupTime = useCallback(() => {
    const normalized = normalizeTimedWarmupTimes([timedWarmupDraft]);
    if (normalized.length === 0) return;
    setTimedWarmupTimes((prev) =>
      normalizeTimedWarmupTimes([...prev, normalized[0]])
    );
    setTimedWarmupDraft("");
  }, [timedWarmupDraft]);

  const handleRemoveTimedWarmupTime = useCallback((time: string) => {
    setTimedWarmupTimes((prev) => prev.filter((entry) => entry !== time));
  }, []);

  const timedWarmupLabel = useMemo(() => {
    if (timedWarmupRunning) return t("warmup.timedWarming");
    if (!timedWarmupEnabled || timedWarmupTimes.length === 0) return t("warmup.timedOff");

    const now = new Date();
    const nowMinutes = now.getHours() * 60 + now.getMinutes();
    const upcoming = timedWarmupTimes.find((time) => {
      const [hours, minutes] = time.split(":").map(Number);
      return hours * 60 + minutes > nowMinutes;
    });
    return t("warmup.timedNext", { time: upcoming ?? timedWarmupTimes[0] });
  }, [timedWarmupEnabled, timedWarmupRunning, timedWarmupTimes, t]);

  const handleExportSlimText = async () => {
    setConfigModalMode("slim_export");
    setConfigModalError(null);
    setConfigPayload("");
    setConfigCopied(false);
    setIsConfigModalOpen(true);

    try {
      setIsExportingSlim(true);
      const payload = await exportAccountsSlimText();
      setConfigPayload(payload);
      showWarmupToast(t("backup.slimExported", { count: accounts.length }));
    } catch (err) {
      console.error("Failed to export slim text:", err);
      const message = err instanceof Error ? err.message : String(err);
      setConfigModalError(message);
      showWarmupToast(t("backup.slimExportFailed"), true);
    } finally {
      setIsExportingSlim(false);
    }
  };

  const openImportSlimTextModal = () => {
    setConfigModalMode("slim_import");
    setConfigModalError(null);
    setConfigPayload("");
    setConfigCopied(false);
    setIsConfigModalOpen(true);
  };

  const handleImportSlimText = async () => {
    if (!configPayload.trim()) {
      setConfigModalError(t("backup.pasteFirst"));
      return;
    }

    try {
      setIsImportingSlim(true);
      setConfigModalError(null);
      const summary = await importAccountsSlimText(configPayload);
      setMaskedAccounts(new Set());
      setIsConfigModalOpen(false);
      showWarmupToast(
        t("backup.importSummary", {
          imported: summary.imported_count,
          skipped: summary.skipped_count,
          total: summary.total_in_payload,
        })
      );
    } catch (err) {
      console.error("Failed to import slim text:", err);
      const message = err instanceof Error ? err.message : String(err);
      setConfigModalError(message);
      showWarmupToast(t("backup.slimImportFailed"), true);
    } finally {
      setIsImportingSlim(false);
    }
  };

  const handleExportFullFile = async () => {
    try {
      setIsExportingFull(true);
      const exported = await exportFullBackupFile(t("fileDialog.exportFull"));
      if (!exported) return;
      showWarmupToast(t("backup.fullExported"));
    } catch (err) {
      console.error("Failed to export full encrypted file:", err);
      showWarmupToast(t("backup.fullExportFailed"), true);
    } finally {
      setIsExportingFull(false);
    }
  };

  const handleImportFullFile = async () => {
    try {
      setIsImportingFull(true);
      const summary = await importFullBackupFile(t("fileDialog.importFull"));
      if (!summary) return;
      const accountList = await loadAccounts();
      await refreshUsage(accountList);
      const maskedIds = await loadMaskedAccountIds();
      setMaskedAccounts(new Set(maskedIds));
      showWarmupToast(
        t("backup.importSummary", {
          imported: summary.imported_count,
          skipped: summary.skipped_count,
          total: summary.total_in_payload,
        })
      );
    } catch (err) {
      console.error("Failed to import full encrypted file:", err);
      showWarmupToast(t("backup.fullImportFailed"), true);
    } finally {
      setIsImportingFull(false);
    }
  };

  const handleOpenCodexApp = async () => {
    try {
      setIsOpeningCodex(true);
      await invokeBackend("open_codex_app");
      showWarmupToast(t("codex.opened"));
      setTimeout(() => {
        void checkProcesses();
      }, 1500);
    } catch (err) {
      console.error("Failed to open Codex app:", err);
      showWarmupToast(t("codex.openFailed", { error: formatWarmupError(err) }), true);
    } finally {
      setIsOpeningCodex(false);
    }
  };

  const activeAccount = accounts.find((a) => a.is_active);
  const otherAccounts = accounts.filter((a) => !a.is_active);
  const hasRunningProcesses = processInfo && processInfo.count > 0;
  const pendingTraySwitchAccount = useMemo(
    () => accounts.find((account) => account.id === pendingTraySwitchAccountId),
    [accounts, pendingTraySwitchAccountId]
  );
  const forceCloseConfirmLabel = pendingTraySwitchAccount
    ? t("forceClose.switch")
    : t("forceClose.processes");

  const sortedOtherAccounts = useMemo(() => {
    const getResetDeadline = (resetAt: number | null | undefined) =>
      resetAt ?? Number.POSITIVE_INFINITY;

    const getSubscriptionDeadline = (expiresAt: string | null | undefined) => {
      if (!expiresAt) return null;
      const timestamp = new Date(expiresAt).getTime();
      return Number.isNaN(timestamp) ? null : timestamp;
    };

    const compareOptionalNumber = (
      aValue: number | null,
      bValue: number | null,
      direction: "asc" | "desc"
    ) => {
      if (aValue === null && bValue === null) return 0;
      if (aValue === null) return 1;
      if (bValue === null) return -1;
      return direction === "asc" ? aValue - bValue : bValue - aValue;
    };

    const getRemainingPercent = (usedPercent: number | null | undefined) => {
      if (usedPercent === null || usedPercent === undefined) {
        return Number.NEGATIVE_INFINITY;
      }
      return Math.max(0, 100 - usedPercent);
    };

    return [...otherAccounts].sort((a, b) => {
      if (
        otherAccountsSort === "subscription_asc" ||
        otherAccountsSort === "subscription_desc"
      ) {
        const subscriptionDiff = compareOptionalNumber(
          getSubscriptionDeadline(a.subscription_expires_at),
          getSubscriptionDeadline(b.subscription_expires_at),
          otherAccountsSort === "subscription_asc" ? "asc" : "desc"
        );
        if (subscriptionDiff !== 0) return subscriptionDiff;

        const deadlineDiff =
          getResetDeadline(getPreferredResetsAt(a.usage)) -
          getResetDeadline(getPreferredResetsAt(b.usage));
        if (deadlineDiff !== 0) return deadlineDiff;

        const remainingDiff =
          getRemainingPercent(getPreferredUsedPercent(b.usage)) -
          getRemainingPercent(getPreferredUsedPercent(a.usage));
        if (remainingDiff !== 0) return remainingDiff;

        return a.name.localeCompare(b.name);
      }

      if (otherAccountsSort === "deadline_asc" || otherAccountsSort === "deadline_desc") {
        const deadlineDiff =
          getResetDeadline(getPreferredResetsAt(a.usage)) -
          getResetDeadline(getPreferredResetsAt(b.usage));
        if (deadlineDiff !== 0) {
          return otherAccountsSort === "deadline_asc" ? deadlineDiff : -deadlineDiff;
        }
        const remainingDiff =
          getRemainingPercent(getPreferredUsedPercent(b.usage)) -
          getRemainingPercent(getPreferredUsedPercent(a.usage));
        if (remainingDiff !== 0) return remainingDiff;
        return a.name.localeCompare(b.name);
      }

      const remainingDiff =
        getRemainingPercent(getPreferredUsedPercent(b.usage)) -
        getRemainingPercent(getPreferredUsedPercent(a.usage));
      if (otherAccountsSort === "remaining_desc" && remainingDiff !== 0) {
        return remainingDiff;
      }
      if (otherAccountsSort === "remaining_asc" && remainingDiff !== 0) {
        return -remainingDiff;
      }
      const deadlineDiff =
        getResetDeadline(getPreferredResetsAt(a.usage)) -
        getResetDeadline(getPreferredResetsAt(b.usage));
      if (deadlineDiff !== 0) return deadlineDiff;
      return a.name.localeCompare(b.name);
    });
  }, [otherAccounts, otherAccountsSort]);

  return (
    <div className="min-h-screen bg-gray-50 text-gray-900 dark:bg-gray-950 dark:text-gray-100">
      <header className="sticky top-0 z-40 border-b border-gray-200 bg-white dark:border-gray-800 dark:bg-gray-900">
        <div className="flex h-9 items-center bg-white px-3 dark:bg-gray-900">
          <div
            onMouseDown={handleTitlebarDrag}
            onDoubleClick={handleTitlebarDoubleClick}
            className={`h-full flex-1 select-none cursor-default ${isMacOs ? "ml-18 mr-2" : "mr-3"}`}
          />
          {!isMacOs && (
            <div className="flex items-center gap-1">
              <button
                onClick={() => {
                  void appWindow.minimize();
                }}
                className="flex h-8 w-8 items-center justify-center rounded-md text-gray-500 transition-colors hover:bg-gray-100 hover:text-gray-900 dark:text-gray-400 dark:hover:bg-gray-800 dark:hover:text-gray-100"
                title={t("window.minimize")}
              >
                <svg className="h-4 w-4" viewBox="0 0 24 24" fill="none" stroke="currentColor">
                  <path d="M5 12h14" strokeWidth="2" strokeLinecap="round" />
                </svg>
              </button>
              <button
                onClick={() => {
                  void appWindow.toggleMaximize();
                }}
                className="flex h-8 w-8 items-center justify-center rounded-md text-gray-500 transition-colors hover:bg-gray-100 hover:text-gray-900 dark:text-gray-400 dark:hover:bg-gray-800 dark:hover:text-gray-100"
                title={isWindowMaximized ? t("window.restore") : t("window.maximize")}
              >
                {isWindowMaximized ? (
                  <svg className="h-4 w-4" viewBox="0 0 24 24" fill="none" stroke="currentColor">
                    <path d="M9 9h10v10H9z" strokeWidth="2" />
                    <path d="M5 15V5h10" strokeWidth="2" strokeLinecap="round" />
                  </svg>
                ) : (
                  <svg className="h-4 w-4" viewBox="0 0 24 24" fill="none" stroke="currentColor">
                    <rect x="5" y="5" width="14" height="14" strokeWidth="2" />
                  </svg>
                )}
              </button>
              <button
                onClick={() => {
                  void appWindow.close();
                }}
                className="flex h-8 w-8 items-center justify-center rounded-md text-gray-500 transition-colors hover:bg-red-500 hover:text-white dark:text-gray-400 dark:hover:bg-red-500 dark:hover:text-white"
                title={t("window.close")}
              >
                <svg className="h-4 w-4" viewBox="0 0 24 24" fill="none" stroke="currentColor">
                  <path d="M6 6l12 12M18 6L6 18" strokeWidth="2" strokeLinecap="round" />
                </svg>
              </button>
            </div>
          )}
        </div>

        <div className="max-w-5xl mx-auto px-6 py-4">
          <div className="grid grid-cols-1 gap-3 md:grid-cols-[minmax(0,1fr)_max-content] md:items-center md:gap-4">
            <div className="flex items-center gap-3 min-w-0 flex-1">
              <div className="min-w-0">
                <div className="flex items-center gap-2 flex-wrap">
                  <h1 className="text-xl font-bold text-gray-900 dark:text-gray-100 tracking-tight">
                    Codex Switcher
                  </h1>
                  {processInfo && (
                    <div className="inline-flex items-center gap-1">
                      <span
                        className={`inline-flex items-center gap-1 px-2 py-0.5 rounded-md text-xs border ${hasRunningProcesses
                            ? "bg-amber-50 text-amber-700 border-amber-200 dark:bg-amber-900/30 dark:text-amber-300 dark:border-amber-700"
                            : "bg-green-50 text-green-700 border-green-200 dark:bg-green-900/30 dark:text-green-300 dark:border-green-700"
                          }`}
                      >
                        <span
                          className={`inline-block w-1.5 h-1.5 rounded-full ${hasRunningProcesses ? "bg-amber-500" : "bg-green-500"
                            }`}
                        ></span>
                        <span>
                          {hasRunningProcesses
                            ? t("header.codexRunning", { count: processInfo.count })
                            : t("header.codexRunning", { count: 0 })}
                        </span>
                      </span>
                      {hasRunningProcesses && (
                        <button
                          onClick={() => {
                            setPendingTraySwitchAccountId(null);
                            setForceCloseConfirmOpen(true);
                          }}
                          disabled={isForceClosingCodex}
                          className="inline-flex items-center rounded-md border border-red-200 bg-red-50 px-2 py-0.5 text-xs font-medium text-red-700 transition-colors hover:bg-red-100 disabled:opacity-50 dark:border-red-800 dark:bg-red-900/20 dark:text-red-300 dark:hover:bg-red-900/30"
                          title={t("header.forceCloseTitle")}
                        >
                          {t("header.forceClose")}
                        </button>
                      )}
                    </div>
                  )}
                  {isTauriRuntime() && processInfo && !hasRunningProcesses && (
                    <button
                      onClick={handleOpenCodexApp}
                      disabled={isOpeningCodex}
                      className="inline-flex items-center rounded-md border border-green-200 bg-green-50 px-2 py-0.5 text-xs font-medium text-green-700 transition-colors hover:bg-green-100 disabled:opacity-50 dark:border-green-800 dark:bg-green-900/20 dark:text-green-300 dark:hover:bg-green-900/30"
                      title={t("header.openCodex")}
                    >
                      {isOpeningCodex ? t("header.opening") : t("header.openCodex")}
                    </button>
                  )}
                </div>
              </div>
            </div>

            <div className="flex flex-wrap items-center gap-2 shrink-0 md:ml-4 md:w-max md:flex-nowrap md:justify-end">
              <button
                onClick={toggleMaskAll}
                className="flex h-10 w-10 items-center justify-center rounded-lg bg-gray-100 text-gray-700 transition-colors hover:bg-gray-200 dark:bg-gray-800 dark:text-gray-200 dark:hover:bg-gray-700 shrink-0"
                title={allMasked ? t("header.showAll") : t("header.hideAll")}
              >
                {allMasked ? (
                  <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                    <path
                      strokeLinecap="round"
                      strokeLinejoin="round"
                      strokeWidth={2}
                      d="M13.875 18.825A10.05 10.05 0 0112 19c-4.478 0-8.268-2.943-9.543-7a9.97 9.97 0 011.563-3.029m5.858.908a3 3 0 114.243 4.243M9.878 9.878l4.242 4.242M9.88 9.88l-3.29-3.29m7.532 7.532l3.29 3.29M3 3l3.59 3.59m0 0A9.953 9.953 0 0112 5c4.478 0 8.268 2.943 9.543 7a10.025 10.025 0 01-4.132 5.411m0 0L21 21"
                    />
                  </svg>
                ) : (
                  <svg className="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                    <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M15 12a3 3 0 11-6 0 3 3 0 016 0z" />
                    <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M2.458 12C3.732 7.943 7.523 5 12 5c4.478 0 8.268 2.943 9.542 7-1.274 4.057-5.064 7-9.542 7-4.477 0-8.268-2.943-9.542-7z" />
                  </svg>
                )}
              </button>
              <button
                onClick={handleRefresh}
                disabled={isRefreshing}
                className="flex h-10 w-10 items-center justify-center rounded-lg bg-gray-100 text-gray-700 transition-colors hover:bg-gray-200 disabled:opacity-50 dark:bg-gray-800 dark:text-gray-200 dark:hover:bg-gray-700 shrink-0"
                title={isRefreshing ? t("header.refreshingAll") : t("header.refreshAll")}
              >
                <span className={isRefreshing ? "animate-spin inline-block" : ""}>↻</span>
              </button>
              <button
                onClick={handleWarmupAll}
                disabled={isWarmingAll || accounts.length === 0}
                className="flex h-10 w-10 items-center justify-center rounded-lg bg-gray-100 text-gray-700 transition-colors hover:bg-gray-200 disabled:opacity-50 dark:bg-gray-800 dark:text-gray-200 dark:hover:bg-gray-700 shrink-0"
                title={t("header.warmupAll")}
              >
                <span className={isWarmingAll ? "animate-pulse" : ""}>⚡</span>
              </button>
              <button
                onClick={() => setAutoWarmupAllEnabled((prev) => !prev)}
                disabled={accounts.length === 0}
                className={`flex h-10 items-center justify-center rounded-lg px-3 text-xs font-semibold transition-colors disabled:opacity-50 shrink-0 whitespace-nowrap ${
                  autoWarmupAllEnabled
                    ? "bg-emerald-50 text-emerald-700 hover:bg-emerald-100 dark:bg-emerald-900/20 dark:text-emerald-300 dark:hover:bg-emerald-900/30"
                    : "bg-gray-100 text-gray-700 hover:bg-gray-200 dark:bg-gray-800 dark:text-gray-200 dark:hover:bg-gray-700"
                }`}
                title={
                  autoWarmupAllEnabled
                    ? t("header.disableAutoAll")
                    : t("header.enableAutoAll")
                }
              >
                {headerAutoWarmupLabel}
              </button>
              <div className="relative shrink-0" ref={timedWarmupRef}>
                <button
                  onClick={() => setIsTimedWarmupOpen((prev) => !prev)}
                  className={`flex h-10 items-center justify-center rounded-lg px-3 text-xs font-semibold transition-colors whitespace-nowrap ${
                    timedWarmupEnabled
                      ? "bg-emerald-50 text-emerald-700 hover:bg-emerald-100 dark:bg-emerald-900/20 dark:text-emerald-300 dark:hover:bg-emerald-900/30"
                      : "bg-gray-100 text-gray-700 hover:bg-gray-200 dark:bg-gray-800 dark:text-gray-200 dark:hover:bg-gray-700"
                  }`}
                  title={t("header.timedTitle")}
                >
                  {timedWarmupLabel} ▾
                </button>
                {isTimedWarmupOpen && (
                  <div className="absolute right-0 z-20 mt-2 w-64 rounded-lg border border-gray-200 bg-white p-3 shadow-lg dark:border-gray-700 dark:bg-gray-900">
                    <label className="flex items-center justify-between text-sm font-medium text-gray-800 dark:text-gray-100">
                      <span>{t("header.timedWarmup")}</span>
                      <input
                        type="checkbox"
                        checked={timedWarmupEnabled}
                        onChange={(e) => setTimedWarmupEnabled(e.target.checked)}
                        className="h-4 w-4 accent-emerald-600"
                      />
                    </label>
                    <div className="mt-3 space-y-1">
                      {timedWarmupTimes.length === 0 ? (
                        <p className="text-xs italic text-gray-400 dark:text-gray-500">
                          {t("header.noTimes")}
                        </p>
                      ) : (
                        timedWarmupTimes.map((time) => (
                          <div
                            key={time}
                            className="flex items-center justify-between rounded-md bg-gray-50 px-2 py-1 text-sm dark:bg-gray-800"
                          >
                            <span className="font-mono text-gray-800 dark:text-gray-100">
                              {time}
                            </span>
                            <button
                              onClick={() => handleRemoveTimedWarmupTime(time)}
                              className="text-gray-400 transition-colors hover:text-red-500"
                              title={t("header.removeTime", { time })}
                            >
                              ✕
                            </button>
                          </div>
                        ))
                      )}
                    </div>

                    <div className="mt-3 flex items-center gap-2">
                      <input
                        type="time"
                        value={timedWarmupDraft}
                        onChange={(e) => setTimedWarmupDraft(e.target.value)}
                        onKeyDown={(e) => {
                          if (e.key === "Enter") handleAddTimedWarmupTime();
                        }}
                        className="h-8 flex-1 rounded-md border border-gray-300 bg-white px-2 text-sm text-gray-800 dark:border-gray-600 dark:bg-gray-800 dark:text-gray-100"
                      />
                      <button
                        onClick={handleAddTimedWarmupTime}
                        disabled={!timedWarmupDraft}
                        className="h-8 rounded-md bg-gray-900 px-3 text-xs font-semibold text-white transition-colors hover:bg-gray-800 disabled:opacity-50 dark:bg-black dark:hover:bg-neutral-900"
                      >
                        {t("common.add")}
                      </button>
                    </div>
                  </div>
                )}
              </div>
              <button
                onClick={() => setThemeMode((prev) => (prev === "dark" ? "light" : "dark"))}
                className="flex h-10 w-10 items-center justify-center rounded-lg bg-gray-100 text-lg text-gray-700 transition-colors hover:bg-gray-200 dark:bg-gray-800 dark:text-gray-200 dark:hover:bg-gray-700 shrink-0"
                title={themeMode === "dark" ? t("header.lightMode") : t("header.darkMode")}
              >
                {themeMode === "dark" ? "☀" : "☾"}
              </button>

              <div className="relative" ref={actionsMenuRef}>
                <button
                  onClick={() => setIsActionsMenuOpen((prev) => !prev)}
                  className="h-10 px-4 py-2 text-sm font-medium rounded-lg bg-gray-900 text-white transition-colors hover:bg-gray-800 dark:bg-black dark:hover:bg-neutral-900 shrink-0 whitespace-nowrap"
                >
                  {t("header.accountMenu")} ▾
                </button>
                {isActionsMenuOpen && (
                  <div className="absolute right-0 z-50 mt-2 w-56 rounded-xl border border-gray-200 bg-white p-2 text-gray-700 shadow-xl dark:border-neutral-800 dark:bg-black dark:text-white">
                    <button
                      onClick={() => {
                        setIsActionsMenuOpen(false);
                        setIsAddModalOpen(true);
                      }}
                      className="w-full rounded-lg px-3 py-2 text-left text-sm transition-colors hover:bg-gray-100 dark:text-white dark:hover:bg-neutral-900"
                    >
                      {t("header.addAccount")}
                    </button>
                    <button
                      onClick={() => {
                        setIsActionsMenuOpen(false);
                        void handleExportSlimText();
                      }}
                      disabled={isExportingSlim}
                      className="w-full rounded-lg px-3 py-2 text-left text-sm transition-colors hover:bg-gray-100 disabled:opacity-50 dark:text-white dark:hover:bg-neutral-900"
                    >
                      {isExportingSlim ? t("backup.exporting") : t("backup.exportSlim")}
                    </button>
                    <button
                      onClick={() => {
                        setIsActionsMenuOpen(false);
                        openImportSlimTextModal();
                      }}
                      disabled={isImportingSlim}
                      className="w-full rounded-lg px-3 py-2 text-left text-sm transition-colors hover:bg-gray-100 disabled:opacity-50 dark:text-white dark:hover:bg-neutral-900"
                    >
                      {isImportingSlim ? t("backup.importing") : t("backup.importSlim")}
                    </button>
                    <button
                      onClick={() => {
                        setIsActionsMenuOpen(false);
                        void handleExportFullFile();
                      }}
                      disabled={isExportingFull}
                      className="w-full rounded-lg px-3 py-2 text-left text-sm transition-colors hover:bg-gray-100 disabled:opacity-50 dark:text-white dark:hover:bg-neutral-900"
                    >
                      {isExportingFull ? t("backup.exporting") : t("backup.exportFull")}
                    </button>
                    <button
                      onClick={() => {
                        setIsActionsMenuOpen(false);
                        void handleImportFullFile();
                      }}
                      disabled={isImportingFull}
                      className="w-full rounded-lg px-3 py-2 text-left text-sm transition-colors hover:bg-gray-100 disabled:opacity-50 dark:text-white dark:hover:bg-neutral-900"
                    >
                      {isImportingFull ? t("backup.importing") : t("backup.importFull")}
                    </button>
                    <div className="my-1 border-t border-gray-100 dark:border-neutral-800" />
                    <label className="block px-3 py-2 text-xs text-gray-500 dark:text-neutral-400">
                      {t("language.label")}
                      <select
                        value={languagePreference}
                        onChange={(event) => {
                          void handleLanguageChange(event.target.value as AppLanguage);
                        }}
                        className="mt-1.5 w-full rounded-lg border border-gray-200 bg-white px-2 py-1.5 text-sm text-gray-700 outline-none dark:border-neutral-700 dark:bg-neutral-900 dark:text-white"
                      >
                        <option value={SYSTEM_LANGUAGE}>{t("language.system")}</option>
                        {supportedLanguages.map(({ code, label }) => (
                          <option key={code} value={code}>{label}</option>
                        ))}
                      </select>
                    </label>
                  </div>
                )}
              </div>
            </div>
          </div>
        </div>
      </header>

      {/* Main Content */}
      <main className="max-w-5xl mx-auto px-6 py-8">
        {loading && accounts.length === 0 ? (
          <div className="flex flex-col items-center justify-center py-20">
            <div className="animate-spin h-10 w-10 border-2 border-gray-900 dark:border-gray-100 border-t-transparent rounded-full mb-4"></div>
            <p className="text-gray-500 dark:text-gray-400">{t("accounts.loading")}</p>
          </div>
        ) : error ? (
          <div className="text-center py-20">
            <div className="text-red-600 dark:text-red-300 mb-2">{t("accounts.loadFailed")}</div>
            <p className="text-sm text-gray-500 dark:text-gray-400">{error}</p>
          </div>
        ) : accounts.length === 0 ? (
          <div className="text-center py-20">
            <div className="h-16 w-16 rounded-2xl bg-gray-100 dark:bg-gray-800 flex items-center justify-center mx-auto mb-4">
              <span className="text-3xl">👤</span>
            </div>
            <h2 className="text-xl font-semibold text-gray-900 dark:text-gray-100 mb-2">
              {t("accounts.emptyTitle")}
            </h2>
            <p className="text-gray-500 dark:text-gray-400 mb-6">
              {t("accounts.emptyBody")}
            </p>
            <button
              onClick={() => setIsAddModalOpen(true)}
              className="px-6 py-3 text-sm font-medium rounded-lg bg-gray-900 hover:bg-gray-800 dark:bg-gray-100 dark:hover:bg-gray-200 text-white dark:text-gray-900 transition-colors"
            >
              {t("accounts.add")}
            </button>
          </div>
        ) : (
          <div className="space-y-8">
            {/* Active Account */}
            {activeAccount && (
              <section>
                <h2 className="text-sm font-medium text-gray-500 dark:text-gray-400 uppercase tracking-wider mb-4">
                  {t("accounts.activeHeading")}
                </h2>
                <AccountCard
                  account={activeAccount}
                  onSwitch={() => { }}
                  onWarmup={() =>
                    handleWarmupAccount(activeAccount.id, activeAccount.name)
                  }
                  onDelete={() => handleDelete(activeAccount.id)}
                  onRefresh={() =>
                    refreshSingleUsage(activeAccount.id, { refreshMetadata: true })
                  }
                  onRename={(newName) => renameAccount(activeAccount.id, newName)}
                  switching={switchingId === activeAccount.id}
                  switchDisabled={hasRunningProcesses ?? false}
                  warmingUp={
                    isWarmingAll ||
                    warmingUpId === activeAccount.id ||
                    autoWarmupRunningIds.has(activeAccount.id)
                  }
                  masked={maskedAccounts.has(activeAccount.id)}
                  onToggleMask={() => toggleMask(activeAccount.id)}
                  autoWarmupEnabled={
                    autoWarmupAllEnabled || autoWarmupAccountIds.has(activeAccount.id)
                  }
                  autoWarmupManagedByAll={autoWarmupAllEnabled}
                  autoWarmupLabel={getAutoWarmupLabel(
                    activeAccount.usage,
                    autoWarmupAllEnabled || autoWarmupAccountIds.has(activeAccount.id),
                    autoWarmupRunningIds.has(activeAccount.id)
                  )}
                  onToggleAutoWarmup={() => toggleAutoWarmupAccount(activeAccount.id)}
                />
              </section>
            )}

            {/* Other Accounts */}
            {otherAccounts.length > 0 && (
              <section>
                <div className="flex items-center justify-between gap-3 mb-4">
                  <h2 className="text-sm font-medium text-gray-500 dark:text-gray-400 uppercase tracking-wider">
                    {t("accounts.otherHeading", { count: otherAccounts.length })}
                  </h2>
                  <div className="flex items-center gap-2">
                    <label htmlFor="other-accounts-sort" className="text-xs text-gray-500 dark:text-gray-400">
                      {t("accounts.sort")}
                    </label>
                    <div className="relative">
                      <select
                        id="other-accounts-sort"
                        value={otherAccountsSort}
                        onChange={(e) =>
                          setOtherAccountsSort(
                            e.target.value as
                              | "deadline_asc"
                              | "deadline_desc"
                              | "remaining_desc"
                              | "remaining_asc"
                              | "subscription_asc"
                              | "subscription_desc"
                          )
                        }
                        className="appearance-none font-sans text-xs sm:text-sm font-medium pl-3 pr-9 py-2 rounded-xl border border-gray-300 dark:border-gray-700 bg-gradient-to-b from-white to-gray-50 dark:from-gray-900 dark:to-gray-800 text-gray-700 dark:text-gray-200 shadow-sm hover:border-gray-400 dark:hover:border-gray-600 hover:shadow focus:outline-none focus:ring-2 focus:ring-gray-300 dark:focus:ring-gray-600 focus:border-gray-400 dark:focus:border-gray-600 transition-all"
                      >
                        <option value="deadline_asc">{t("accounts.sortResetAsc")}</option>
                        <option value="deadline_desc">{t("accounts.sortResetDesc")}</option>
                        <option value="remaining_desc">
                          {t("accounts.sortRemainingDesc")}
                        </option>
                        <option value="remaining_asc">
                          {t("accounts.sortRemainingAsc")}
                        </option>
                        <option value="subscription_asc">
                          {t("accounts.sortExpiryAsc")}
                        </option>
                        <option value="subscription_desc">
                          {t("accounts.sortExpiryDesc")}
                        </option>
                      </select>
                      <span className="pointer-events-none absolute inset-y-0 right-3 flex items-center text-gray-500 dark:text-gray-400">
                        <svg
                          className="h-4 w-4"
                          viewBox="0 0 20 20"
                          fill="none"
                          stroke="currentColor"
                          strokeWidth="2"
                        >
                          <path d="M6 8l4 4 4-4" strokeLinecap="round" strokeLinejoin="round" />
                        </svg>
                      </span>
                    </div>
                  </div>
                </div>
                <div className="grid grid-cols-1 md:grid-cols-2 gap-4">
                  {sortedOtherAccounts.map((account) => (
                    <AccountCard
                      key={account.id}
                      account={account}
                      onSwitch={() => handleSwitch(account.id)}
                      onWarmup={() => handleWarmupAccount(account.id, account.name)}
                      onDelete={() => handleDelete(account.id)}
                      onRefresh={() =>
                        refreshSingleUsage(account.id, { refreshMetadata: true })
                      }
                      onRename={(newName) => renameAccount(account.id, newName)}
                      switching={switchingId === account.id}
                      switchDisabled={hasRunningProcesses ?? false}
                      warmingUp={
                        isWarmingAll ||
                        warmingUpId === account.id ||
                        autoWarmupRunningIds.has(account.id)
                      }
                      masked={maskedAccounts.has(account.id)}
                      onToggleMask={() => toggleMask(account.id)}
                      autoWarmupEnabled={
                        autoWarmupAllEnabled || autoWarmupAccountIds.has(account.id)
                      }
                      autoWarmupManagedByAll={autoWarmupAllEnabled}
                      autoWarmupLabel={getAutoWarmupLabel(
                        account.usage,
                        autoWarmupAllEnabled || autoWarmupAccountIds.has(account.id),
                        autoWarmupRunningIds.has(account.id)
                      )}
                      onToggleAutoWarmup={() => toggleAutoWarmupAccount(account.id)}
                    />
                  ))}
                </div>
              </section>
            )}
          </div>
        )}
      </main>

      {/* Refresh Success Toast */}
      {refreshSuccess && (
        <div className="fixed bottom-6 left-1/2 -translate-x-1/2 px-4 py-3 bg-green-600 text-white rounded-lg shadow-lg text-sm flex items-center gap-2">
          <span>✓</span> {t("accounts.refreshSuccess")}
        </div>
      )}

      {/* Warm-up Toast */}
      {warmupToast && (
        <div
          className={`fixed bottom-20 left-1/2 -translate-x-1/2 px-4 py-3 rounded-lg shadow-lg text-sm ${
            warmupToast.isError
              ? "bg-red-600 text-white"
              : "bg-amber-100 text-amber-900 border border-amber-300 dark:bg-amber-900/30 dark:text-amber-200 dark:border-amber-700"
          }`}
        >
          {warmupToast.message}
        </div>
      )}

      {/* Delete Confirmation Toast */}
      {deleteConfirmId && (
        <div className="fixed bottom-6 left-1/2 -translate-x-1/2 px-4 py-3 bg-red-600 text-white rounded-lg shadow-lg text-sm">
          {t("accounts.deleteConfirm")}
        </div>
      )}

      {forceCloseConfirmOpen && (
        <div className="fixed inset-0 bg-black/40 flex items-center justify-center z-50">
          <div className="bg-white dark:bg-gray-900 border border-gray-200 dark:border-gray-700 rounded-2xl w-full max-w-md mx-4 shadow-xl">
            <div className="p-5 border-b border-gray-100 dark:border-gray-800">
              <h2 className="text-lg font-semibold text-gray-900 dark:text-gray-100">
                {t("forceClose.title")}
              </h2>
            </div>
            <div className="p-5 space-y-3">
              <p className="text-sm text-gray-600 dark:text-gray-300">
                {t("forceClose.body", { count: processInfo?.count ?? 0 })}
              </p>
              {pendingTraySwitchAccount && (
                <p className="text-sm text-gray-600 dark:text-gray-300">
                  {t("forceClose.thenSwitch", { name: pendingTraySwitchAccount.name })}
                </p>
              )}
              <p className="text-sm text-red-600 dark:text-red-300">
                {t("forceClose.warning")}
              </p>
            </div>
            <div className="flex justify-end gap-3 p-5 border-t border-gray-100 dark:border-gray-800">
              <button
                onClick={() => {
                  setPendingTraySwitchAccountId(null);
                  setForceCloseConfirmOpen(false);
                }}
                disabled={isForceClosingCodex}
                className="px-4 py-2.5 text-sm font-medium rounded-lg bg-gray-100 hover:bg-gray-200 dark:bg-gray-800 dark:hover:bg-gray-700 text-gray-700 dark:text-gray-200 transition-colors disabled:opacity-50"
              >
                {t("common.cancel")}
              </button>
              <button
                onClick={() => {
                  void handleForceCloseConfirm();
                }}
                disabled={isForceClosingCodex}
                className="px-4 py-2.5 text-sm font-medium rounded-lg bg-red-600 hover:bg-red-700 text-white transition-colors disabled:opacity-50"
              >
                {isForceClosingCodex
                  ? t("forceClose.closing")
                  : forceCloseConfirmLabel}
              </button>
            </div>
          </div>
        </div>
      )}

      {closeBehaviorPromptOpen && (
        <div className="fixed inset-0 bg-black/40 flex items-center justify-center z-50">
          <div className="bg-white dark:bg-gray-900 border border-gray-200 dark:border-gray-700 rounded-2xl w-full max-w-md mx-4 shadow-xl">
            <div className="p-5 border-b border-gray-100 dark:border-gray-800">
              <h2 className="text-lg font-semibold text-gray-900 dark:text-gray-100">
                {t("closeBehavior.title")}
              </h2>
            </div>
            <div className="p-5 space-y-4">
              <p className="text-sm text-gray-600 dark:text-gray-300">
                {t("closeBehavior.body")}
              </p>
              <p className="text-sm text-gray-600 dark:text-gray-300">
                {t("closeBehavior.changeLater")}
              </p>
              <label className="flex items-center gap-2 text-sm text-gray-700 dark:text-gray-200">
                <input
                  type="checkbox"
                  checked={closeBehaviorDontAskAgain}
                  onChange={(event) => setCloseBehaviorDontAskAgain(event.target.checked)}
                  className="h-4 w-4 accent-gray-900 dark:accent-gray-100"
                />
                <span>{t("closeBehavior.dontAsk")}</span>
              </label>
            </div>
            <div className="flex flex-col gap-2 p-5 border-t border-gray-100 dark:border-gray-800 sm:flex-row sm:justify-end">
              <button
                onClick={() => setCloseBehaviorPromptOpen(false)}
                disabled={isCompletingCloseBehavior}
                className="px-4 py-2.5 text-sm font-medium rounded-lg bg-gray-100 hover:bg-gray-200 dark:bg-gray-800 dark:hover:bg-gray-700 text-gray-700 dark:text-gray-200 transition-colors disabled:opacity-50"
              >
                {t("common.cancel")}
              </button>
              <button
                onClick={() => void handleCloseBehaviorChoice("show_in_dock")}
                disabled={isCompletingCloseBehavior}
                className="px-4 py-2.5 text-sm font-medium rounded-lg bg-gray-100 hover:bg-gray-200 dark:bg-gray-800 dark:hover:bg-gray-700 text-gray-700 dark:text-gray-200 transition-colors disabled:opacity-50"
              >
                {t("closeBehavior.keepDock")}
              </button>
              <button
                onClick={() => void handleCloseBehaviorChoice("menu_bar_only")}
                disabled={isCompletingCloseBehavior}
                className="px-4 py-2.5 text-sm font-medium rounded-lg bg-gray-900 hover:bg-gray-800 dark:bg-gray-100 dark:hover:bg-gray-200 text-white dark:text-gray-900 transition-colors disabled:opacity-50"
              >
                {t("closeBehavior.menuBarOnly")}
              </button>
            </div>
          </div>
        </div>
      )}

      {/* Add Account Modal */}
      <AddAccountModal
        isOpen={isAddModalOpen}
        onClose={() => setIsAddModalOpen(false)}
        onImportFile={importFromFile}
        onStartOAuth={startOAuthLogin}
        onCompleteOAuth={completeOAuthLogin}
        onCancelOAuth={cancelOAuthLogin}
      />

      {/* Import/Export Config Modal */}
      {isConfigModalOpen && (
        <div className="fixed inset-0 bg-black/40 flex items-center justify-center z-50">
          <div className="bg-white dark:bg-gray-900 border border-gray-200 dark:border-gray-700 rounded-2xl w-full max-w-2xl mx-4 shadow-xl">
            <div className="flex items-center justify-between p-5 border-b border-gray-100 dark:border-gray-800">
              <h2 className="text-lg font-semibold text-gray-900 dark:text-gray-100">
                {configModalMode === "slim_export" ? t("backup.exportSlim") : t("backup.importSlim")}
              </h2>
              <button
                onClick={() => setIsConfigModalOpen(false)}
                className="text-gray-400 hover:text-gray-600 dark:hover:text-gray-300 transition-colors"
              >
                ✕
              </button>
            </div>
            <div className="p-5 space-y-4">
              {configModalMode === "slim_import" ? (
                <p className="text-sm text-amber-700 dark:text-amber-200 bg-amber-50 dark:bg-amber-900/30 border border-amber-200 dark:border-amber-700 rounded-lg px-3 py-2">
                  {t("backup.keepExisting")}
                </p>
              ) : (
                <p className="text-sm text-gray-500 dark:text-gray-400">
                  {t("backup.secretWarning")}
                </p>
              )}
              <textarea
                value={configPayload}
                onChange={(e) => setConfigPayload(e.target.value)}
                readOnly={configModalMode === "slim_export"}
                placeholder={
                  configModalMode === "slim_export"
                    ? isExportingSlim
                      ? t("backup.generating")
                      : t("backup.exportPlaceholder")
                    : t("backup.importPlaceholder")
                }
                className="w-full h-48 px-4 py-3 bg-gray-50 dark:bg-gray-800 border border-gray-200 dark:border-gray-700 rounded-lg text-sm text-gray-800 dark:text-gray-100 placeholder-gray-400 dark:placeholder-gray-500 focus:outline-none focus:border-gray-400 dark:focus:border-gray-500 focus:ring-1 focus:ring-gray-400 dark:focus:ring-gray-500 font-mono"
              />
              {configModalError && (
                <div className="p-3 bg-red-50 dark:bg-red-900/20 border border-red-200 dark:border-red-700 rounded-lg text-red-600 dark:text-red-300 text-sm">
                  {configModalError}
                </div>
              )}
            </div>
            <div className="flex gap-3 p-5 border-t border-gray-100 dark:border-gray-800">
              <button
                onClick={() => setIsConfigModalOpen(false)}
                className="px-4 py-2.5 text-sm font-medium rounded-lg bg-gray-100 hover:bg-gray-200 dark:bg-gray-800 dark:hover:bg-gray-700 text-gray-700 dark:text-gray-200 transition-colors"
              >
                {t("common.close")}
              </button>
              {configModalMode === "slim_export" ? (
                <button
                  onClick={async () => {
                    if (!configPayload) return;
                    try {
                      await navigator.clipboard.writeText(configPayload);
                      setConfigCopied(true);
                      setTimeout(() => setConfigCopied(false), 1500);
                    } catch {
                      setConfigModalError(t("backup.clipboardUnavailable"));
                    }
                  }}
                  disabled={!configPayload || isExportingSlim}
                  className="px-4 py-2.5 text-sm font-medium rounded-lg bg-gray-900 hover:bg-gray-800 dark:bg-gray-100 dark:hover:bg-gray-200 text-white dark:text-gray-900 transition-colors disabled:opacity-50"
                >
                  {configCopied ? t("common.copied") : t("backup.copyString")}
                </button>
              ) : (
                <button
                  onClick={handleImportSlimText}
                  disabled={isImportingSlim}
                  className="px-4 py-2.5 text-sm font-medium rounded-lg bg-gray-900 hover:bg-gray-800 dark:bg-gray-100 dark:hover:bg-gray-200 text-white dark:text-gray-900 transition-colors disabled:opacity-50"
                >
                  {isImportingSlim ? t("backup.importing") : t("backup.importMissing")}
                </button>
              )}
            </div>
          </div>
        </div>
      )}
      <UpdateChecker />

    </div>
  );
}

export default App;
