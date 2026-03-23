import { useState, useEffect, useCallback, useMemo, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open, save } from "@tauri-apps/plugin-dialog";
import {
  getAccountNextResetAt,
  getAccountRemainingPercent,
  getAccountReserveBand,
} from "./accountAnalytics";
import { useAccounts } from "./hooks/useAccounts";
import { AccountCard, AccountsInsights, AddAccountModal, UpdateChecker } from "./components";
import type { CodexProcessInfo } from "./types";
import "./App.css";

type ThemePreference = "system" | "light" | "dark";

const THEME_STORAGE_KEY = "codex-switcher-theme";

function App() {
  const {
    accounts,
    loading,
    error,
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
    exportAccountsFullEncryptedFile,
    importAccountsFullEncryptedFile,
    startOAuthLogin,
    completeOAuthLogin,
    cancelOAuthLogin,
    loadMaskedAccountIds,
    saveMaskedAccountIds,
    loadOpencodeSyncEnabled,
    saveOpencodeSyncEnabled,
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
  const [isRefreshing, setIsRefreshing] = useState(false);
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
  const [maskedAccounts, setMaskedAccounts] = useState<Set<string>>(new Set());
  const [otherAccountsSort, setOtherAccountsSort] = useState<
    "deadline_asc" | "deadline_desc" | "remaining_desc" | "remaining_asc"
  >("deadline_asc");
  const [isActionsMenuOpen, setIsActionsMenuOpen] = useState(false);
  const actionsMenuRef = useRef<HTMLDivElement | null>(null);

  const [opencodeSyncEnabled, setOpencodeSyncEnabled] = useState(true);
  const [switchSuccessToast, setSwitchSuccessToast] = useState<{
    message: string;
    show: boolean;
  }>({ message: "", show: false });
  const [themePreference, setThemePreference] = useState<ThemePreference>("system");
  const [resolvedTheme, setResolvedTheme] = useState<"light" | "dark">("light");

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
      const info = await invoke<CodexProcessInfo>("check_codex_processes");
      setProcessInfo(info);
    } catch (err) {
      console.error("Failed to check processes:", err);
    }
  }, []);

  // Check processes on mount and periodically
  useEffect(() => {
    checkProcesses();
    const interval = setInterval(checkProcesses, 3000); // Check every 3 seconds
    return () => clearInterval(interval);
  }, [checkProcesses]);

  // Load masked accounts and OpenCode settings from storage on mount
  useEffect(() => {
    loadMaskedAccountIds().then((ids) => {
      if (ids.length > 0) {
        setMaskedAccounts(new Set(ids));
      }
    });
    loadOpencodeSyncEnabled().then((enabled) => {
      setOpencodeSyncEnabled(enabled);
    });
  }, [loadMaskedAccountIds, loadOpencodeSyncEnabled]);

  useEffect(() => {
    if (typeof window === "undefined") return;

    const saved = window.localStorage.getItem(THEME_STORAGE_KEY);
    if (saved === "light" || saved === "dark" || saved === "system") {
      setThemePreference(saved);
    }
  }, []);

  useEffect(() => {
    if (typeof window === "undefined") return;

    const mediaQuery = window.matchMedia("(prefers-color-scheme: dark)");

    const applyTheme = () => {
      const nextTheme =
        themePreference === "system"
          ? mediaQuery.matches
            ? "dark"
            : "light"
          : themePreference;

      setResolvedTheme(nextTheme);
      document.documentElement.classList.toggle("dark", nextTheme === "dark");
      document.documentElement.style.colorScheme = nextTheme;
    };

    applyTheme();
    mediaQuery.addEventListener("change", applyTheme);

    return () => {
      mediaQuery.removeEventListener("change", applyTheme);
    };
  }, [themePreference]);

  const handleThemeChange = (nextTheme: ThemePreference) => {
    setThemePreference(nextTheme);
    window.localStorage.setItem(THEME_STORAGE_KEY, nextTheme);
  };

  const toggleOpencodeSync = async () => {
    const next = !opencodeSyncEnabled;
    setOpencodeSyncEnabled(next);
    await saveOpencodeSyncEnabled(next);
  };

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

  const handleSwitch = async (accountId: string) => {
    // Check processes before switching
    await checkProcesses();
    if (processInfo && !processInfo.can_switch) {
      return;
    }

    try {
      setSwitchingId(accountId);
      const result = await switchAccount(accountId);

      const message = result.opencode_synced
        ? "✓ Synced to Codex CLI + OpenCode"
        : "✓ Synced to Codex CLI";

      setSwitchSuccessToast({ message, show: true });
      setTimeout(() => setSwitchSuccessToast({ message: "", show: false }), 3000);
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
      await refreshUsage();
      setRefreshSuccess(true);
      setTimeout(() => setRefreshSuccess(false), 2000);
    } finally {
      setIsRefreshing(false);
    }
  };

  const showWarmupToast = (message: string, isError = false) => {
    setWarmupToast({ message, isError });
    setTimeout(() => setWarmupToast(null), 2500);
  };

  const formatWarmupError = (err: unknown) => {
    if (!err) return "Unknown error";
    if (err instanceof Error && err.message) return err.message;
    if (typeof err === "string") return err;
    try {
      return JSON.stringify(err);
    } catch {
      return "Unknown error";
    }
  };

  const handleWarmupAccount = async (accountId: string, accountName: string) => {
    try {
      setWarmingUpId(accountId);
      await warmupAccount(accountId);
      showWarmupToast(`Warm-up sent for ${accountName}`);
    } catch (err) {
      console.error("Failed to warm up account:", err);
      showWarmupToast(
        `Warm-up failed for ${accountName}: ${formatWarmupError(err)}`,
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
        showWarmupToast("No accounts available for warm-up", true);
        return;
      }

      if (summary.failed_account_ids.length === 0) {
        showWarmupToast(
          `Warm-up sent for all ${summary.warmed_accounts} account${
            summary.warmed_accounts === 1 ? "" : "s"
          }`
        );
      } else {
        showWarmupToast(
          `Warmed ${summary.warmed_accounts}/${summary.total_accounts}. Failed: ${summary.failed_account_ids.length}`,
          true
        );
      }
    } catch (err) {
      console.error("Failed to warm up all accounts:", err);
      showWarmupToast(`Warm-up all failed: ${formatWarmupError(err)}`, true);
    } finally {
      setIsWarmingAll(false);
    }
  };

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
      showWarmupToast(`Slim text exported (${accounts.length} accounts).`);
    } catch (err) {
      console.error("Failed to export slim text:", err);
      const message = err instanceof Error ? err.message : String(err);
      setConfigModalError(message);
      showWarmupToast("Slim export failed", true);
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
      setConfigModalError("Please paste the slim text string first.");
      return;
    }

    try {
      setIsImportingSlim(true);
      setConfigModalError(null);
      const summary = await importAccountsSlimText(configPayload);
      setMaskedAccounts(new Set());
      setIsConfigModalOpen(false);
      showWarmupToast(
        `Imported ${summary.imported_count}, skipped ${summary.skipped_count} (total ${summary.total_in_payload})`
      );
    } catch (err) {
      console.error("Failed to import slim text:", err);
      const message = err instanceof Error ? err.message : String(err);
      setConfigModalError(message);
      showWarmupToast("Slim import failed", true);
    } finally {
      setIsImportingSlim(false);
    }
  };

  const handleExportFullFile = async () => {
    try {
      setIsExportingFull(true);
      const selected = await save({
        title: "Export Full Encrypted Account Config",
        defaultPath: "codex-switcher-full.cswf",
        filters: [
          {
            name: "Codex Switcher Full Backup",
            extensions: ["cswf"],
          },
        ],
      });

      if (!selected) return;

      await exportAccountsFullEncryptedFile(selected);
      showWarmupToast("Full encrypted file exported.");
    } catch (err) {
      console.error("Failed to export full encrypted file:", err);
      showWarmupToast("Full export failed", true);
    } finally {
      setIsExportingFull(false);
    }
  };

  const handleImportFullFile = async () => {
    try {
      setIsImportingFull(true);
      const selected = await open({
        multiple: false,
        title: "Import Full Encrypted Account Config",
        filters: [
          {
            name: "Codex Switcher Full Backup",
            extensions: ["cswf"],
          },
        ],
      });

      if (!selected || Array.isArray(selected)) return;

      const summary = await importAccountsFullEncryptedFile(selected);
      setMaskedAccounts(new Set());
      showWarmupToast(
        `Imported ${summary.imported_count}, skipped ${summary.skipped_count} (total ${summary.total_in_payload})`
      );
    } catch (err) {
      console.error("Failed to import full encrypted file:", err);
      showWarmupToast("Full import failed", true);
    } finally {
      setIsImportingFull(false);
    }
  };

  const activeAccount = accounts.find((a) => a.is_active);
  const otherAccounts = accounts.filter((a) => !a.is_active);
  const hasRunningProcesses = processInfo && processInfo.count > 0;

  const sortedOtherAccounts = useMemo(() => {
    const getResetDeadline = (account: (typeof otherAccounts)[number]) =>
      getAccountNextResetAt(account) ?? Number.POSITIVE_INFINITY;

    const getRemainingPercent = (account: (typeof otherAccounts)[number]) =>
      getAccountRemainingPercent(account) ?? Number.NEGATIVE_INFINITY;

    return [...otherAccounts].sort((a, b) => {
      if (otherAccountsSort === "deadline_asc" || otherAccountsSort === "deadline_desc") {
        const deadlineDiff = getResetDeadline(a) - getResetDeadline(b);
        if (deadlineDiff !== 0) {
          return otherAccountsSort === "deadline_asc" ? deadlineDiff : -deadlineDiff;
        }
        const remainingDiff = getRemainingPercent(b) - getRemainingPercent(a);
        if (remainingDiff !== 0) return remainingDiff;
        return a.name.localeCompare(b.name);
      }

      const remainingDiff = getRemainingPercent(b) - getRemainingPercent(a);
      if (otherAccountsSort === "remaining_desc" && remainingDiff !== 0) {
        return remainingDiff;
      }
      if (otherAccountsSort === "remaining_asc" && remainingDiff !== 0) {
        return -remainingDiff;
      }
      const deadlineDiff = getResetDeadline(a) - getResetDeadline(b);
      if (deadlineDiff !== 0) return deadlineDiff;
      return a.name.localeCompare(b.name);
    });
  }, [otherAccounts, otherAccountsSort]);

  const toolbarIconButtonClass =
    "group inline-flex h-9 w-9 items-center justify-center rounded-xl border border-slate-200 bg-white/92 text-slate-600 shadow-[0_1px_2px_rgba(15,23,42,0.05),0_10px_20px_rgba(15,23,42,0.04)] transition-all hover:-translate-y-0.5 hover:border-slate-300 hover:bg-white hover:text-slate-900 disabled:cursor-not-allowed disabled:opacity-50 dark:border-slate-700 dark:bg-slate-900/92 dark:text-slate-300 dark:hover:border-slate-600 dark:hover:bg-slate-900 dark:hover:text-white";

  const primaryToolbarButtonClass =
    "group inline-flex h-9 items-center justify-center gap-2 rounded-xl bg-slate-900 px-3.5 text-sm font-semibold text-white shadow-[0_1px_2px_rgba(15,23,42,0.18),0_14px_24px_rgba(15,23,42,0.2)] transition-all hover:-translate-y-0.5 hover:bg-slate-800 dark:bg-white dark:text-slate-900 dark:hover:bg-slate-100";

  const activeReserveBand = activeAccount ? getAccountReserveBand(activeAccount) : null;
  const activeReserveState =
    activeReserveBand === null
      ? null
      : {
          unknown: {
            label: "Unknown reserve",
            className: "border-slate-200 bg-slate-100 text-slate-700 dark:border-slate-700 dark:bg-slate-800 dark:text-slate-200",
          },
          depleted: {
            label: "Depleted reserve",
            className: "border-slate-200 bg-slate-100 text-slate-700 dark:border-slate-700 dark:bg-slate-800 dark:text-slate-200",
          },
          critical: {
            label: "Critical reserve",
            className: "border-rose-200 bg-rose-50 text-rose-700 dark:border-rose-900/70 dark:bg-rose-500/12 dark:text-rose-300",
          },
          watch: {
            label: "Watch reserve",
            className: "border-amber-200 bg-amber-50 text-amber-700 dark:border-amber-900/70 dark:bg-amber-500/12 dark:text-amber-300",
          },
          healthy: {
            label: "Healthy reserve",
            className: "border-sky-200 bg-sky-50 text-sky-700 dark:border-sky-900/70 dark:bg-sky-500/12 dark:text-sky-300",
          },
          ready: {
            label: "Ready reserve",
            className: "border-emerald-200 bg-emerald-50 text-emerald-700 dark:border-emerald-900/70 dark:bg-emerald-500/12 dark:text-emerald-300",
          },
        }[activeReserveBand];

  return (
    <div className="min-h-screen bg-gray-50 text-slate-900 transition-colors dark:bg-slate-950 dark:text-slate-100">
      {/* Header */}
      <header className="sticky top-0 z-40 border-b border-slate-200/70 bg-slate-50/92 backdrop-blur-xl dark:border-slate-800/80 dark:bg-slate-950/92">
        <div className="max-w-6xl mx-auto px-4 py-2.5 sm:px-6 sm:py-3">
          <div className="relative rounded-[24px] border border-white/80 bg-[linear-gradient(180deg,rgba(255,255,255,0.98),rgba(248,250,252,0.94))] shadow-[0_1px_2px_rgba(15,23,42,0.06),0_18px_40px_rgba(15,23,42,0.08)] dark:border-slate-800/80 dark:bg-[linear-gradient(180deg,rgba(15,23,42,0.96),rgba(15,23,42,0.9))] dark:shadow-[0_1px_2px_rgba(2,6,23,0.6),0_22px_48px_rgba(2,6,23,0.5)]">
            <div className="pointer-events-none absolute inset-0 overflow-hidden rounded-[24px]">
              <div className="absolute inset-0 bg-[radial-gradient(circle_at_top_left,_rgba(16,185,129,0.12),_transparent_34%),radial-gradient(circle_at_bottom_right,_rgba(59,130,246,0.08),_transparent_28%)] dark:bg-[radial-gradient(circle_at_top_left,_rgba(16,185,129,0.18),_transparent_34%),radial-gradient(circle_at_bottom_right,_rgba(59,130,246,0.16),_transparent_28%)]" />
            </div>

            <div className="relative flex flex-col gap-2.5 p-3 sm:flex-row sm:items-center sm:justify-between sm:p-3.5">
              <div className="flex min-w-0 items-center gap-3">
                <div className="relative shrink-0">
                  <div className="absolute inset-0 rounded-xl bg-emerald-400/20 blur-lg" />
                  <div className="relative flex h-11 w-11 items-center justify-center rounded-xl bg-[linear-gradient(145deg,#0f172a,#1e293b)] text-white shadow-[0_12px_28px_rgba(15,23,42,0.28)] ring-1 ring-white/20 sm:h-12 sm:w-12 sm:rounded-[18px]">
                    <span className="text-lg font-semibold tracking-tight">C</span>
                  </div>
                </div>

                <div className="min-w-0">
                  <div className="flex flex-wrap items-center gap-2.5">
                    <h1 className="text-[1.7rem] font-semibold tracking-tight text-slate-900 dark:text-white">
                      Codex Switcher
                    </h1>
                    {processInfo && (
                      <span
                        className={`inline-flex items-center gap-2 rounded-full border px-2.5 py-1 text-xs font-medium shadow-sm ${
                          hasRunningProcesses
                            ? "border-amber-200 bg-amber-50 text-amber-700 dark:border-amber-900/70 dark:bg-amber-500/12 dark:text-amber-300"
                            : "border-emerald-200 bg-emerald-50 text-emerald-700 dark:border-emerald-900/70 dark:bg-emerald-500/12 dark:text-emerald-300"
                        }`}
                      >
                        <span
                          className={`inline-block h-2 w-2 rounded-full ${
                            hasRunningProcesses ? "bg-amber-500" : "bg-emerald-500"
                          }`}
                        />
                        <span>
                          {hasRunningProcesses
                            ? `${processInfo.count} Codex running`
                            : "0 Codex running"}
                        </span>
                      </span>
                    )}
                  </div>

                  <p className="hidden text-xs text-slate-500 dark:text-slate-400 md:block">
                    Multi-account manager for Codex CLI
                  </p>
                </div>
              </div>

              <div className="flex items-center justify-between gap-2 sm:justify-end">
                <div className="flex items-center gap-2 rounded-2xl border border-white/80 bg-white/72 p-1 shadow-[inset_0_1px_0_rgba(255,255,255,0.8),0_10px_22px_rgba(15,23,42,0.05)] backdrop-blur dark:border-slate-800 dark:bg-slate-950/70 dark:shadow-[inset_0_1px_0_rgba(255,255,255,0.03),0_10px_22px_rgba(2,6,23,0.35)]">
                  <button
                    onClick={toggleMaskAll}
                    className={toolbarIconButtonClass}
                    aria-label={allMasked ? "Show all account emails" : "Hide all account emails"}
                    title={allMasked ? "Show all account emails" : "Hide all account emails"}
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
                    <span className="sr-only">{allMasked ? "Show all account emails" : "Hide all account emails"}</span>
                  </button>

                  <button
                    onClick={handleRefresh}
                    disabled={isRefreshing}
                    className={toolbarIconButtonClass}
                    aria-label={isRefreshing ? "Refreshing all account usage" : "Refresh all account usage"}
                    title={isRefreshing ? "Refreshing all account usage" : "Refresh all account usage"}
                  >
                    <span className={isRefreshing ? "animate-spin inline-block" : ""}>↻</span>
                    <span className="sr-only">{isRefreshing ? "Refreshing..." : "Refresh All"}</span>
                  </button>

                  <button
                    onClick={handleWarmupAll}
                    disabled={isWarmingAll || accounts.length === 0}
                    className={toolbarIconButtonClass}
                    aria-label={isWarmingAll ? "Warming up all accounts" : "Warm up all accounts"}
                    title="Send minimal traffic using all accounts"
                  >
                    <span className={`${isWarmingAll ? "animate-pulse" : ""} text-amber-600`}>⚡</span>
                    <span className="sr-only">{isWarmingAll ? "Warming..." : "Warm-up All"}</span>
                  </button>
                </div>

                <div className="relative" ref={actionsMenuRef}>
                  <button
                    onClick={() => setIsActionsMenuOpen((prev) => !prev)}
                    className={primaryToolbarButtonClass}
                  >
                    <span className="flex h-7 w-7 items-center justify-center rounded-xl bg-white/12 text-white/90 dark:bg-slate-200 dark:text-slate-700">
                      <svg className="w-4 h-4" viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="1.8">
                        <path d="M4 6h12M4 10h12M4 14h8" strokeLinecap="round" strokeLinejoin="round" />
                      </svg>
                    </span>
                    <span>Account</span>
                    <svg className="h-4 w-4 text-white/80 dark:text-slate-500" viewBox="0 0 20 20" fill="none" stroke="currentColor" strokeWidth="2">
                      <path d="M6 8l4 4 4-4" strokeLinecap="round" strokeLinejoin="round" />
                    </svg>
                  </button>

                  {isActionsMenuOpen && (
                    <div className="absolute right-0 top-full z-50 mt-2 w-64 max-w-[calc(100vw-2rem)] rounded-2xl border border-slate-200 bg-white/96 p-2.5 shadow-[0_18px_50px_rgba(15,23,42,0.18)] backdrop-blur dark:border-slate-800 dark:bg-slate-900/96 dark:shadow-[0_20px_55px_rgba(2,6,23,0.55)]">
                      <button
                        onClick={() => {
                          setIsActionsMenuOpen(false);
                          setIsAddModalOpen(true);
                        }}
                        className="w-full rounded-xl px-3 py-2.5 text-left text-sm font-medium text-slate-700 transition-colors hover:bg-slate-100 dark:text-slate-100 dark:hover:bg-slate-800"
                      >
                        + Add Account
                      </button>

                      <div className="my-2 border-t border-slate-100 dark:border-slate-800"></div>

                      <div className="rounded-xl px-3 py-2.5">
                        <div className="mb-2 flex items-center justify-between text-sm">
                          <span className="font-medium text-slate-700 dark:text-slate-100">Theme</span>
                          <span className="text-xs text-slate-400 dark:text-slate-500">
                            {resolvedTheme === "dark" ? "Dark" : "Light"}
                          </span>
                        </div>
                        <div className="grid grid-cols-3 gap-1 rounded-xl border border-slate-200 bg-slate-50 p-1 dark:border-slate-700 dark:bg-slate-950/70">
                          {(["system", "light", "dark"] as ThemePreference[]).map((theme) => {
                            const active = themePreference === theme;

                            return (
                              <button
                                key={theme}
                                onClick={() => handleThemeChange(theme)}
                                className={`rounded-lg px-2 py-1.5 text-xs font-medium transition-colors ${
                                  active
                                    ? "bg-white text-slate-900 shadow-sm dark:bg-slate-800 dark:text-white"
                                    : "text-slate-500 hover:text-slate-700 dark:text-slate-400 dark:hover:text-slate-200"
                                }`}
                              >
                                {theme === "system"
                                  ? "System"
                                  : theme.charAt(0).toUpperCase() + theme.slice(1)}
                              </button>
                            );
                          })}
                        </div>
                      </div>

                      <label className="flex w-full cursor-pointer items-center justify-between rounded-xl px-3 py-2.5 text-sm transition-colors hover:bg-slate-100 dark:hover:bg-slate-800">
                        <span className="font-medium text-slate-700 dark:text-slate-100">Sync to OpenCode</span>
                        <div className={`relative h-5 w-9 rounded-full transition-colors ${opencodeSyncEnabled ? "bg-emerald-500" : "bg-slate-300"}`}>
                          <div className={`absolute top-0.5 left-0.5 h-4 w-4 rounded-full bg-white transition-transform ${opencodeSyncEnabled ? "translate-x-4" : "translate-x-0"}`}></div>
                        </div>
                        <input
                          type="checkbox"
                          className="hidden"
                          checked={opencodeSyncEnabled}
                          onChange={toggleOpencodeSync}
                        />
                      </label>

                      <div className="my-2 border-t border-slate-100 dark:border-slate-800"></div>

                      <button
                        onClick={() => {
                          setIsActionsMenuOpen(false);
                          void handleExportSlimText();
                        }}
                        disabled={isExportingSlim}
                        className="w-full rounded-xl px-3 py-2.5 text-left text-sm text-slate-700 transition-colors hover:bg-slate-100 disabled:opacity-50 dark:text-slate-100 dark:hover:bg-slate-800"
                      >
                        {isExportingSlim ? "Exporting..." : "Export Slim Text"}
                      </button>
                      <button
                        onClick={() => {
                          setIsActionsMenuOpen(false);
                          openImportSlimTextModal();
                        }}
                        disabled={isImportingSlim}
                        className="w-full rounded-xl px-3 py-2.5 text-left text-sm text-slate-700 transition-colors hover:bg-slate-100 disabled:opacity-50 dark:text-slate-100 dark:hover:bg-slate-800"
                      >
                        {isImportingSlim ? "Importing..." : "Import Slim Text"}
                      </button>
                      <button
                        onClick={() => {
                          setIsActionsMenuOpen(false);
                          void handleExportFullFile();
                        }}
                        disabled={isExportingFull}
                        className="w-full rounded-xl px-3 py-2.5 text-left text-sm text-slate-700 transition-colors hover:bg-slate-100 disabled:opacity-50 dark:text-slate-100 dark:hover:bg-slate-800"
                      >
                        {isExportingFull ? "Exporting..." : "Export Full Encrypted File"}
                      </button>
                      <button
                        onClick={() => {
                          setIsActionsMenuOpen(false);
                          void handleImportFullFile();
                        }}
                        disabled={isImportingFull}
                        className="w-full rounded-xl px-3 py-2.5 text-left text-sm text-slate-700 transition-colors hover:bg-slate-100 disabled:opacity-50 dark:text-slate-100 dark:hover:bg-slate-800"
                      >
                        {isImportingFull ? "Importing..." : "Import Full Encrypted File"}
                      </button>
                    </div>
                  )}
                </div>
              </div>
            </div>
          </div>
        </div>
      </header>

      {/* Main Content */}
      <main className="max-w-6xl mx-auto px-4 py-6 sm:px-6 sm:py-8">
        {loading && accounts.length === 0 ? (
          <div className="flex flex-col items-center justify-center py-20">
            <div className="animate-spin h-10 w-10 border-2 border-gray-900 border-t-transparent rounded-full mb-4 dark:border-slate-200 dark:border-t-transparent"></div>
            <p className="text-gray-500 dark:text-slate-400">Loading accounts...</p>
          </div>
        ) : error ? (
          <div className="text-center py-20">
            <div className="text-red-600 mb-2">Failed to load accounts</div>
            <p className="text-sm text-gray-500 dark:text-slate-400">{error}</p>
          </div>
        ) : accounts.length === 0 ? (
          <div className="text-center py-20">
            <div className="h-16 w-16 rounded-2xl bg-gray-100 dark:bg-slate-900 dark:ring-1 dark:ring-slate-800 flex items-center justify-center mx-auto mb-4">
              <span className="text-3xl">👤</span>
            </div>
            <h2 className="text-xl font-semibold text-gray-900 dark:text-white mb-2">
              No accounts yet
            </h2>
            <p className="text-gray-500 dark:text-slate-400 mb-6">
              Add your first Codex account to get started
            </p>
            <button
              onClick={() => setIsAddModalOpen(true)}
              className="px-6 py-3 text-sm font-medium rounded-lg bg-gray-900 hover:bg-gray-800 text-white transition-colors dark:bg-white dark:text-slate-900 dark:hover:bg-slate-100"
            >
              Add Account
            </button>
          </div>
        ) : (
          <div className="space-y-7">
            {(activeAccount || otherAccounts.length > 0) && (
              <section>
                <div className="relative overflow-hidden rounded-[30px] border border-slate-200 bg-[linear-gradient(180deg,rgba(255,255,255,0.98),rgba(248,250,252,0.95))] shadow-[0_1px_2px_rgba(15,23,42,0.08),0_18px_38px_rgba(15,23,42,0.06)] dark:border-slate-800 dark:bg-[linear-gradient(180deg,rgba(15,23,42,0.96),rgba(15,23,42,0.92))] dark:shadow-[0_1px_2px_rgba(2,6,23,0.55),0_20px_40px_rgba(2,6,23,0.45)]">
                  <div className="pointer-events-none absolute inset-0 bg-[radial-gradient(circle_at_top_left,_rgba(16,185,129,0.08),_transparent_26%),radial-gradient(circle_at_bottom_right,_rgba(59,130,246,0.07),_transparent_24%)] dark:bg-[radial-gradient(circle_at_top_left,_rgba(16,185,129,0.14),_transparent_28%),radial-gradient(circle_at_bottom_right,_rgba(59,130,246,0.12),_transparent_24%)]" />

                  <div className="relative flex flex-wrap items-center justify-between gap-3 border-b border-slate-100/90 bg-white/72 px-4 py-3.5 backdrop-blur sm:px-5 dark:border-slate-800 dark:bg-slate-900/55">
                    <div className="flex flex-wrap items-center gap-2">
                        <h2 className="text-sm font-medium uppercase tracking-[0.22em] text-slate-500 dark:text-slate-300">
                          Account Overview
                        </h2>
                      {activeAccount && (
                        <span className="inline-flex items-center rounded-full border border-emerald-100 bg-emerald-50 px-2.5 py-1 text-[11px] font-medium text-emerald-700 shadow-sm dark:border-emerald-900/70 dark:bg-emerald-500/12 dark:text-emerald-300">
                          Live now
                        </span>
                      )}
                      {activeReserveState && (
                        <span
                          className={`inline-flex items-center rounded-full border px-2.5 py-1 text-[11px] font-medium shadow-sm ${activeReserveState.className}`}
                        >
                          {activeReserveState.label}
                        </span>
                      )}
                      {otherAccounts.length > 0 && (
                        <span className="inline-flex items-center rounded-full border border-slate-200 bg-white px-2.5 py-1 text-[11px] font-medium text-slate-600 shadow-sm dark:border-slate-700 dark:bg-slate-900 dark:text-slate-300">
                          {accounts.length} total / {otherAccounts.length} standby
                        </span>
                      )}
                    </div>
                  </div>

                  <div className="relative p-4 sm:p-5">
                    {otherAccounts.length > 0 && (
                      <div className="mb-4">
                        <AccountsInsights accounts={otherAccounts} embedded />
                      </div>
                    )}

                    {activeAccount && (
                      <div className="min-w-0">
                        <AccountCard
                          account={activeAccount}
                          onSwitch={() => {}}
                          onWarmup={() =>
                            handleWarmupAccount(activeAccount.id, activeAccount.name)
                          }
                          onDelete={() => handleDelete(activeAccount.id)}
                          onRefresh={() => refreshSingleUsage(activeAccount.id)}
                          onRename={(newName) => renameAccount(activeAccount.id, newName)}
                          switching={switchingId === activeAccount.id}
                          switchDisabled={hasRunningProcesses ?? false}
                          warmingUp={isWarmingAll || warmingUpId === activeAccount.id}
                          masked={maskedAccounts.has(activeAccount.id)}
                          onToggleMask={() => toggleMask(activeAccount.id)}
                          embedded
                        />
                      </div>
                    )}
                  </div>
                </div>
              </section>
            )}

            {/* Other Accounts */}
            {otherAccounts.length > 0 && (
              <section>
                <div className="mb-4 space-y-4">
                  <div className="flex flex-col gap-3 min-[700px]:flex-row min-[700px]:items-center min-[700px]:justify-between">
                    <div className="min-w-0">
                      <div className="flex flex-wrap items-center gap-2">
                        <h2 className="text-sm font-medium uppercase tracking-[0.22em] text-gray-500 dark:text-slate-300">
                          Other Accounts
                        </h2>
                        <span className="inline-flex items-center rounded-full border border-slate-200 bg-white px-2.5 py-1 text-[11px] font-medium text-slate-600 shadow-sm dark:border-slate-700 dark:bg-slate-900 dark:text-slate-300">
                          {otherAccounts.length} standby
                        </span>
                      </div>
                      <p className="mt-1 text-xs text-slate-400 dark:text-slate-500">
                        Bench cards ranked by reserve strength and reset timing.
                      </p>
                    </div>
                    <div className="flex w-full items-center gap-2 min-[700px]:w-auto min-[700px]:justify-end">
                      <label
                        htmlFor="other-accounts-sort"
                        className="text-[11px] font-semibold uppercase tracking-[0.18em] text-gray-400 dark:text-slate-500"
                      >
                        Sort
                      </label>
                      <div className="relative min-w-0 flex-1 min-[700px]:w-[240px] min-[700px]:flex-none">
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
                            )
                          }
                          className="w-full appearance-none font-sans text-xs sm:text-sm font-medium pl-3 pr-9 py-2 rounded-2xl border border-slate-200 bg-white text-slate-700 shadow-[0_1px_2px_rgba(15,23,42,0.08),0_8px_24px_rgba(15,23,42,0.04)] hover:border-slate-300 focus:outline-none focus:ring-2 focus:ring-slate-200 focus:border-slate-300 transition-all dark:border-slate-700 dark:bg-slate-900 dark:text-slate-100 dark:hover:border-slate-600 dark:focus:ring-slate-800 dark:focus:border-slate-600"
                        >
                          <option value="deadline_asc">Soonest reset</option>
                          <option value="deadline_desc">Latest reset</option>
                          <option value="remaining_desc">Best reserve</option>
                          <option value="remaining_asc">Lowest reserve</option>
                        </select>
                        <span className="pointer-events-none absolute inset-y-0 right-3 flex items-center text-gray-500 dark:text-slate-400">
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
                </div>
                <div className="grid grid-cols-1 gap-4 md:grid-cols-2 xl:grid-cols-3">
                  {sortedOtherAccounts.map((account) => (
                    <AccountCard
                      key={account.id}
                      account={account}
                      onSwitch={() => handleSwitch(account.id)}
                      onWarmup={() => handleWarmupAccount(account.id, account.name)}
                      onDelete={() => handleDelete(account.id)}
                      onRefresh={() => refreshSingleUsage(account.id)}
                      onRename={(newName) => renameAccount(account.id, newName)}
                      switching={switchingId === account.id}
                      switchDisabled={hasRunningProcesses ?? false}
                      warmingUp={isWarmingAll || warmingUpId === account.id}
                      masked={maskedAccounts.has(account.id)}
                      onToggleMask={() => toggleMask(account.id)}
                    />
                  ))}
                </div>
              </section>
            )}
          </div>
        )}
      </main>

      {/* Switch Success Toast */}
      {switchSuccessToast.show && (
        <div className="fixed bottom-6 left-1/2 z-50 flex -translate-x-1/2 items-center gap-2 rounded-lg border border-emerald-500/30 bg-emerald-600 px-4 py-3 text-sm text-white shadow-lg dark:border-emerald-500/20 dark:bg-emerald-500/90">
          <span>{switchSuccessToast.message}</span>
        </div>
      )}

      {/* Refresh Success Toast */}
      {refreshSuccess && (
        <div className="fixed bottom-6 left-1/2 flex -translate-x-1/2 items-center gap-2 rounded-lg border border-emerald-500/30 bg-emerald-600 px-4 py-3 text-sm text-white shadow-lg dark:border-emerald-500/20 dark:bg-emerald-500/90">
          <span>✓</span> Usage refreshed successfully
        </div>
      )}

      {/* Warm-up Toast */}
      {warmupToast && (
        <div
          className={`fixed bottom-20 left-1/2 -translate-x-1/2 px-4 py-3 rounded-lg shadow-lg text-sm ${
            warmupToast.isError
              ? "border border-red-500/20 bg-red-600 text-white"
              : "border border-amber-300 bg-amber-100 text-amber-900 dark:border-amber-900/70 dark:bg-amber-500/12 dark:text-amber-200"
          }`}
        >
          {warmupToast.message}
        </div>
      )}

      {/* Delete Confirmation Toast */}
      {deleteConfirmId && (
        <div className="fixed bottom-6 left-1/2 -translate-x-1/2 rounded-lg border border-red-500/20 bg-red-600 px-4 py-3 text-sm text-white shadow-lg">
          Click delete again to confirm removal
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
          <div className="bg-white border border-gray-200 rounded-2xl w-full max-w-2xl mx-4 shadow-xl dark:bg-slate-900 dark:border-slate-800">
            <div className="flex items-center justify-between p-5 border-b border-gray-100 dark:border-slate-800">
              <h2 className="text-lg font-semibold text-gray-900 dark:text-white">
                {configModalMode === "slim_export" ? "Export Slim Text" : "Import Slim Text"}
              </h2>
              <button
                onClick={() => setIsConfigModalOpen(false)}
                className="text-gray-400 hover:text-gray-600 transition-colors dark:text-slate-500 dark:hover:text-slate-300"
              >
                ✕
              </button>
            </div>
            <div className="p-5 space-y-4">
              {configModalMode === "slim_import" ? (
                <p className="text-sm text-amber-700 bg-amber-50 border border-amber-200 rounded-lg px-3 py-2 dark:bg-amber-950/40 dark:border-amber-800 dark:text-amber-300">
                  Existing accounts are kept. Only missing accounts are imported.
                </p>
              ) : (
                <p className="text-sm text-gray-500 dark:text-slate-400">
                  This slim string contains account secrets. Keep it private.
                </p>
              )}
              <textarea
                value={configPayload}
                onChange={(e) => setConfigPayload(e.target.value)}
                readOnly={configModalMode === "slim_export"}
                placeholder={
                  configModalMode === "slim_export"
                    ? isExportingSlim
                      ? "Generating..."
                      : "Export string will appear here"
                    : "Paste config string here"
                }
                className="w-full h-48 px-4 py-3 bg-gray-50 border border-gray-200 rounded-lg text-sm text-gray-800 placeholder-gray-400 focus:outline-none focus:border-gray-400 focus:ring-1 focus:ring-gray-400 font-mono dark:bg-slate-950 dark:border-slate-800 dark:text-slate-100 dark:placeholder-slate-500 dark:focus:border-slate-700 dark:focus:ring-slate-700"
              />
              {configModalError && (
                <div className="p-3 bg-red-50 border border-red-200 rounded-lg text-red-600 text-sm dark:bg-red-950/40 dark:border-red-900 dark:text-red-300">
                  {configModalError}
                </div>
              )}
            </div>
            <div className="flex gap-3 p-5 border-t border-gray-100 dark:border-slate-800">
              <button
                onClick={() => setIsConfigModalOpen(false)}
                className="px-4 py-2.5 text-sm font-medium rounded-lg bg-gray-100 hover:bg-gray-200 text-gray-700 transition-colors dark:bg-slate-800 dark:hover:bg-slate-700 dark:text-slate-200"
              >
                Close
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
                      setConfigModalError("Clipboard unavailable. Please copy manually.");
                    }
                  }}
                  disabled={!configPayload || isExportingSlim}
                  className="px-4 py-2.5 text-sm font-medium rounded-lg bg-gray-900 hover:bg-gray-800 text-white transition-colors disabled:opacity-50 dark:bg-white dark:text-slate-900 dark:hover:bg-slate-100"
                >
                  {configCopied ? "Copied" : "Copy String"}
                </button>
              ) : (
                <button
                  onClick={handleImportSlimText}
                  disabled={isImportingSlim}
                  className="px-4 py-2.5 text-sm font-medium rounded-lg bg-gray-900 hover:bg-gray-800 text-white transition-colors disabled:opacity-50 dark:bg-white dark:text-slate-900 dark:hover:bg-slate-100"
                >
                  {isImportingSlim ? "Importing..." : "Import Missing Accounts"}
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
