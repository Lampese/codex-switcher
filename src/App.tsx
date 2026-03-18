import { useState, useEffect, useCallback, useMemo, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open, save } from "@tauri-apps/plugin-dialog";
import { useAccounts } from "./hooks/useAccounts";
import { AccountCard, AddAccountModal, UpdateChecker } from "./components";
import type { CodexProcessInfo } from "./types";
import "./App.css";

type Theme = "light" | "dark";
type OtherAccountsSort =
  | "deadline_asc"
  | "deadline_desc"
  | "remaining_desc"
  | "remaining_asc";

const THEME_STORAGE_KEY = "codex-switcher-theme";
const OTHER_ACCOUNTS_SORT_OPTIONS: Array<{ value: OtherAccountsSort; label: string }> = [
  { value: "deadline_asc", label: "Reset: earliest to latest" },
  { value: "deadline_desc", label: "Reset: latest to earliest" },
  { value: "remaining_desc", label: "% remaining: highest to lowest" },
  { value: "remaining_asc", label: "% remaining: lowest to highest" },
];

const shellClass = "theme-shell min-h-screen transition-colors";
const panelClass = "theme-panel";
const softButtonClass =
  "theme-button-secondary whitespace-nowrap rounded-lg px-4 py-2 text-sm font-medium disabled:opacity-50";
const primaryButtonClass =
  "theme-button-primary whitespace-nowrap rounded-lg px-4 py-2 text-sm font-medium disabled:opacity-50";
const warningButtonClass =
  "theme-button-warning whitespace-nowrap rounded-lg px-4 py-2 text-sm font-medium disabled:opacity-50";
const menuItemClass =
  "theme-menu-item w-full rounded-xl px-3 py-2 text-left text-sm disabled:opacity-50";

function getInitialTheme(): Theme {
  if (typeof window === "undefined") return "light";

  try {
    const savedTheme = window.localStorage.getItem(THEME_STORAGE_KEY);
    if (savedTheme === "light" || savedTheme === "dark") {
      return savedTheme;
    }
  } catch (error) {
    console.error("Failed to read theme preference:", error);
  }

  return window.matchMedia?.("(prefers-color-scheme: dark)")?.matches
    ? "dark"
    : "light";
}

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
  } = useAccounts();

  const [theme, setTheme] = useState<Theme>(() => getInitialTheme());
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
  const [otherAccountsSort, setOtherAccountsSort] = useState<OtherAccountsSort>("deadline_asc");
  const [isActionsMenuOpen, setIsActionsMenuOpen] = useState(false);
  const [isSortMenuOpen, setIsSortMenuOpen] = useState(false);
  const actionsMenuRef = useRef<HTMLDivElement | null>(null);
  const sortMenuRef = useRef<HTMLDivElement | null>(null);

  const toggleMask = (accountId: string) => {
    setMaskedAccounts((prev) => {
      const next = new Set(prev);
      next.has(accountId) ? next.delete(accountId) : next.add(accountId);
      void saveMaskedAccountIds(Array.from(next));
      return next;
    });
  };

  const allMasked =
    accounts.length > 0 && accounts.every((account) => maskedAccounts.has(account.id));

  const toggleMaskAll = () => {
    setMaskedAccounts((prev) => {
      const shouldMaskAll = !accounts.every((account) => prev.has(account.id));
      const next = shouldMaskAll
        ? new Set(accounts.map((account) => account.id))
        : new Set<string>();
      void saveMaskedAccountIds(Array.from(next));
      return next;
    });
  };

  const checkProcesses = useCallback(async () => {
    try {
      const info = await invoke<CodexProcessInfo>("check_codex_processes");
      setProcessInfo(info);
      return info;
    } catch (err) {
      console.error("Failed to check processes:", err);
      return null;
    }
  }, []);

  useEffect(() => {
    const root = document.documentElement;
    root.classList.toggle("dark", theme === "dark");
    try {
      window.localStorage.setItem(THEME_STORAGE_KEY, theme);
    } catch (error) {
      console.error("Failed to save theme preference:", error);
    }
  }, [theme]);

  useEffect(() => {
    void checkProcesses();
    const interval = setInterval(() => void checkProcesses(), 3000);
    return () => clearInterval(interval);
  }, [checkProcesses]);

  useEffect(() => {
    loadMaskedAccountIds().then((ids) => {
      if (ids.length > 0) setMaskedAccounts(new Set(ids));
    });
  }, [loadMaskedAccountIds]);

  useEffect(() => {
    if (!isActionsMenuOpen && !isSortMenuOpen) return;

    const handleClickOutside = (event: MouseEvent) => {
      const target = event.target as Node;

      if (actionsMenuRef.current && !actionsMenuRef.current.contains(target)) {
        setIsActionsMenuOpen(false);
      }

      if (sortMenuRef.current && !sortMenuRef.current.contains(target)) {
        setIsSortMenuOpen(false);
      }
    };

    const handleEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        setIsActionsMenuOpen(false);
        setIsSortMenuOpen(false);
      }
    };

    document.addEventListener("mousedown", handleClickOutside);
    document.addEventListener("keydown", handleEscape);
    return () => {
      document.removeEventListener("mousedown", handleClickOutside);
      document.removeEventListener("keydown", handleEscape);
    };
  }, [isActionsMenuOpen, isSortMenuOpen]);

  const handleSwitch = async (accountId: string) => {
    const info = await checkProcesses();
    if (info && !info.can_switch) return;

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
      setConfigModalError(err instanceof Error ? err.message : String(err));
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
      setConfigModalError(err instanceof Error ? err.message : String(err));
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
        filters: [{ name: "Codex Switcher Full Backup", extensions: ["cswf"] }],
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
        filters: [{ name: "Codex Switcher Full Backup", extensions: ["cswf"] }],
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

  const activeAccount = accounts.find((account) => account.is_active);
  const otherAccounts = accounts.filter((account) => !account.is_active);
  const hasRunningProcesses = processInfo && processInfo.count > 0;

  const sortedOtherAccounts = useMemo(() => {
    const getResetDeadline = (resetAt: number | null | undefined) =>
      resetAt ?? Number.POSITIVE_INFINITY;
    const getRemainingPercent = (usedPercent: number | null | undefined) =>
      usedPercent === null || usedPercent === undefined
        ? Number.NEGATIVE_INFINITY
        : Math.max(0, 100 - usedPercent);

    return [...otherAccounts].sort((a, b) => {
      if (
        otherAccountsSort === "deadline_asc" ||
        otherAccountsSort === "deadline_desc"
      ) {
        const deadlineDiff =
          getResetDeadline(a.usage?.primary_resets_at) -
          getResetDeadline(b.usage?.primary_resets_at);
        if (deadlineDiff !== 0) {
          return otherAccountsSort === "deadline_asc"
            ? deadlineDiff
            : -deadlineDiff;
        }

        const remainingDiff =
          getRemainingPercent(b.usage?.primary_used_percent) -
          getRemainingPercent(a.usage?.primary_used_percent);
        if (remainingDiff !== 0) return remainingDiff;
        return a.name.localeCompare(b.name);
      }

      const remainingDiff =
        getRemainingPercent(b.usage?.primary_used_percent) -
        getRemainingPercent(a.usage?.primary_used_percent);
      if (otherAccountsSort === "remaining_desc" && remainingDiff !== 0) {
        return remainingDiff;
      }
      if (otherAccountsSort === "remaining_asc" && remainingDiff !== 0) {
        return -remainingDiff;
      }

      const deadlineDiff =
        getResetDeadline(a.usage?.primary_resets_at) -
        getResetDeadline(b.usage?.primary_resets_at);
      if (deadlineDiff !== 0) return deadlineDiff;
      return a.name.localeCompare(b.name);
    });
  }, [otherAccounts, otherAccountsSort]);

  const selectedSortLabel =
    OTHER_ACCOUNTS_SORT_OPTIONS.find((option) => option.value === otherAccountsSort)?.label ??
    OTHER_ACCOUNTS_SORT_OPTIONS[0].label;

  return (
    <div className={shellClass}>
      <header
        className="sticky top-0 z-40 border-b backdrop-blur"
        style={{
          borderColor: "var(--theme-border-subtle)",
          backgroundColor:
            theme === "dark" ? "rgba(21, 26, 34, 0.9)" : "rgba(255, 255, 255, 0.9)",
        }}
      >
        <div className="mx-auto max-w-5xl px-6 py-4">
          <div className="grid grid-cols-1 gap-3 md:grid-cols-[minmax(0,1fr)_max-content] md:items-center md:gap-4">
            <div className="flex min-w-0 items-center gap-3">
              <div
                className="flex h-10 w-10 items-center justify-center rounded-xl text-lg font-bold"
                style={{
                  backgroundColor: "var(--theme-primary-bg)",
                  color: "var(--theme-primary-text)",
                }}
              >
                C
              </div>
              <div className="min-w-0">
                <div className="flex flex-wrap items-center gap-2">
                  <h1 className="text-xl font-bold tracking-tight text-slate-900 dark:text-[var(--theme-text-primary)]">
                    Codex Switcher
                  </h1>
                  {processInfo && (
                    <span
                      className={`theme-status-chip inline-flex items-center gap-1 rounded-md px-2 py-0.5 text-xs ${
                        hasRunningProcesses
                          ? "theme-status-chip--warning"
                          : "theme-status-chip--success"
                      }`}
                    >
                      <span
                        className="h-1.5 w-1.5 rounded-full"
                        style={{
                          backgroundColor: hasRunningProcesses
                            ? "var(--theme-warning-text)"
                            : "var(--theme-success-text)",
                        }}
                      />
                      <span>
                        {hasRunningProcesses
                          ? `${processInfo.count} Codex running`
                          : "0 Codex running"}
                      </span>
                    </span>
                  )}
                </div>
                <p className="text-xs text-slate-500 dark:text-slate-400">
                  Multi-account manager for Codex CLI
                </p>
              </div>
            </div>

            <div className="flex flex-wrap items-center gap-2 md:ml-4 md:w-max md:flex-nowrap md:justify-end">
              <button
                onClick={toggleMaskAll}
                className={softButtonClass}
                title={
                  allMasked
                    ? "Show all account names and emails"
                    : "Hide all account names and emails"
                }
              >
                {allMasked ? "Show All" : "Hide All"}
              </button>
              <button
                onClick={handleRefresh}
                disabled={isRefreshing}
                className={softButtonClass}
              >
                {isRefreshing ? "Refreshing..." : "Refresh All"}
              </button>
              <button
                onClick={handleWarmupAll}
                disabled={isWarmingAll || accounts.length === 0}
                className={warningButtonClass}
                title="Send minimal traffic using all accounts"
              >
                {isWarmingAll ? "Warming..." : "Warm-up All"}
              </button>
              <div className="relative" ref={actionsMenuRef}>
                <button
                  onClick={() => {
                    setIsSortMenuOpen(false);
                    setIsActionsMenuOpen((prev) => !prev);
                  }}
                  className={primaryButtonClass}
                >
                  Account
                </button>
                {isActionsMenuOpen && (
                  <div
                    className={`absolute right-0 z-50 mt-2 w-72 rounded-2xl p-2 ${panelClass}`}
                    style={{ backgroundColor: "var(--theme-surface-elevated)" }}
                  >
                    <button
                      onClick={() => {
                        setIsActionsMenuOpen(false);
                        setIsAddModalOpen(true);
                      }}
                      className={menuItemClass}
                    >
                      + Add Account
                    </button>
                    <div
                      className="my-2 h-px"
                      style={{ backgroundColor: "var(--theme-border-subtle)" }}
                    />
                    <button
                      type="button"
                      role="switch"
                      aria-checked={theme === "dark"}
                      onClick={() =>
                        setTheme((current) => (current === "dark" ? "light" : "dark"))
                      }
                      className="theme-menu-item flex w-full items-center justify-between rounded-xl px-3 py-2 text-left"
                    >
                      <span>
                        <span className="block text-sm font-medium text-slate-800 dark:text-[var(--theme-text-primary)]">
                          Dark theme
                        </span>
                        <span className="block text-xs text-slate-500 dark:text-[var(--theme-text-secondary)]">
                          Toggle the interface palette
                        </span>
                      </span>
                      <span
                        className="relative inline-flex h-6 w-11 items-center rounded-full border transition-colors"
                        style={{
                          backgroundColor:
                            theme === "dark"
                              ? "var(--theme-primary-bg)"
                              : "var(--theme-secondary-border)",
                          borderColor:
                            theme === "dark"
                              ? "var(--theme-primary-bg)"
                              : "var(--theme-border-strong)",
                        }}
                      >
                        <span
                          className={`inline-block h-5 w-5 rounded-full shadow-sm transition-transform ${
                            theme === "dark"
                              ? "translate-x-5"
                              : "translate-x-1"
                          }`}
                          style={{
                            backgroundColor:
                              theme === "dark"
                                ? "var(--theme-text-on-light)"
                                : "var(--theme-surface)",
                          }}
                        />
                      </span>
                    </button>
                    <div
                      className="my-2 h-px"
                      style={{ backgroundColor: "var(--theme-border-subtle)" }}
                    />
                    <button
                      onClick={() => {
                        setIsActionsMenuOpen(false);
                        void handleExportSlimText();
                      }}
                      disabled={isExportingSlim}
                      className={menuItemClass}
                    >
                      {isExportingSlim ? "Exporting..." : "Export Slim Text"}
                    </button>
                    <button
                      onClick={() => {
                        setIsActionsMenuOpen(false);
                        openImportSlimTextModal();
                      }}
                      disabled={isImportingSlim}
                      className={menuItemClass}
                    >
                      {isImportingSlim ? "Importing..." : "Import Slim Text"}
                    </button>
                    <button
                      onClick={() => {
                        setIsActionsMenuOpen(false);
                        void handleExportFullFile();
                      }}
                      disabled={isExportingFull}
                      className={menuItemClass}
                    >
                      {isExportingFull
                        ? "Exporting..."
                        : "Export Full Encrypted File"}
                    </button>
                    <button
                      onClick={() => {
                        setIsActionsMenuOpen(false);
                        void handleImportFullFile();
                      }}
                      disabled={isImportingFull}
                      className={menuItemClass}
                    >
                      {isImportingFull
                        ? "Importing..."
                        : "Import Full Encrypted File"}
                    </button>
                  </div>
                )}
              </div>
            </div>
          </div>
        </div>
      </header>

      <main className="mx-auto max-w-5xl px-6 py-8">
        {loading && accounts.length === 0 ? (
          <div className="flex flex-col items-center justify-center py-20">
            <div className="mb-4 h-10 w-10 animate-spin rounded-full border-2 border-slate-900 border-t-transparent dark:border-slate-100" />
            <p className="text-slate-500 dark:text-slate-400">Loading accounts...</p>
          </div>
        ) : error ? (
          <div className="py-20 text-center">
            <div className="mb-2 text-red-600 dark:text-red-400">
              Failed to load accounts
            </div>
            <p className="text-sm text-slate-500 dark:text-slate-400">{error}</p>
          </div>
        ) : accounts.length === 0 ? (
          <div className="py-20 text-center">
            <div
              className="mx-auto mb-4 flex h-16 w-16 items-center justify-center rounded-2xl text-2xl"
              style={{
                backgroundColor: "var(--theme-surface-elevated)",
                color: "var(--theme-text-secondary)",
              }}
            >
              A
            </div>
            <h2 className="mb-2 text-xl font-semibold text-slate-900 dark:text-slate-50">
              No accounts yet
            </h2>
            <p className="mb-6 text-slate-500 dark:text-slate-400">
              Add your first Codex account to get started
            </p>
            <button onClick={() => setIsAddModalOpen(true)} className={primaryButtonClass}>
              Add Account
            </button>
          </div>
        ) : (
          <div className="space-y-8">
            {activeAccount && (
              <section>
                <h2 className="mb-4 text-sm font-medium uppercase tracking-wider text-slate-500 dark:text-slate-400">
                  Active Account
                </h2>
                <AccountCard
                  account={activeAccount}
                  onSwitch={() => {}}
                  onWarmup={() => handleWarmupAccount(activeAccount.id, activeAccount.name)}
                  onDelete={() => handleDelete(activeAccount.id)}
                  onRefresh={() => refreshSingleUsage(activeAccount.id)}
                  onRename={(newName) => renameAccount(activeAccount.id, newName)}
                  switching={switchingId === activeAccount.id}
                  switchDisabled={hasRunningProcesses ?? false}
                  warmingUp={isWarmingAll || warmingUpId === activeAccount.id}
                  masked={maskedAccounts.has(activeAccount.id)}
                  onToggleMask={() => toggleMask(activeAccount.id)}
                />
              </section>
            )}
            {otherAccounts.length > 0 && (
              <section>
                <div className="mb-4 flex items-center justify-between gap-3">
                  <h2 className="text-sm font-medium uppercase tracking-wider text-slate-500 dark:text-slate-400">
                    Other Accounts ({otherAccounts.length})
                  </h2>
                  <div className="flex items-center gap-2">
                    <label
                      htmlFor="other-accounts-sort"
                      className="text-xs text-slate-500 dark:text-slate-400"
                    >
                      Sort
                    </label>
                    <div className="relative" ref={sortMenuRef}>
                      <button
                        id="other-accounts-sort"
                        type="button"
                        aria-haspopup="listbox"
                        aria-expanded={isSortMenuOpen}
                        onClick={() => {
                          setIsActionsMenuOpen(false);
                          setIsSortMenuOpen((prev) => !prev);
                        }}
                        className="theme-select-button flex min-w-[16rem] items-center justify-between rounded-xl px-3 py-2 text-left text-xs font-medium sm:text-sm"
                      >
                        <span>{selectedSortLabel}</span>
                        <svg
                          className={`h-4 w-4 transition-transform ${isSortMenuOpen ? "rotate-180" : ""}`}
                          viewBox="0 0 20 20"
                          fill="none"
                          stroke="currentColor"
                          strokeWidth="2"
                        >
                          <path d="M6 8l4 4 4-4" strokeLinecap="round" strokeLinejoin="round" />
                        </svg>
                      </button>
                      {isSortMenuOpen && (
                        <div
                          role="listbox"
                          aria-labelledby="other-accounts-sort"
                          className="theme-select-menu absolute right-0 z-30 mt-2 w-72 rounded-2xl p-1"
                        >
                          {OTHER_ACCOUNTS_SORT_OPTIONS.map((option) => {
                            const selected = option.value === otherAccountsSort;
                            return (
                              <button
                                key={option.value}
                                type="button"
                                role="option"
                                aria-selected={selected}
                                onClick={() => {
                                  setOtherAccountsSort(option.value);
                                  setIsSortMenuOpen(false);
                                }}
                                className={`theme-select-option flex w-full items-center justify-between rounded-xl px-3 py-2 text-left text-sm ${
                                  selected ? "theme-select-option--active" : ""
                                }`}
                              >
                                <span>{option.label}</span>
                                {selected && (
                                  <svg
                                    className="h-4 w-4"
                                    viewBox="0 0 20 20"
                                    fill="none"
                                    stroke="currentColor"
                                    strokeWidth="2"
                                  >
                                    <path
                                      d="M5 10.5l3 3 7-7"
                                      strokeLinecap="round"
                                      strokeLinejoin="round"
                                    />
                                  </svg>
                                )}
                              </button>
                            );
                          })}
                        </div>
                      )}
                    </div>
                  </div>
                </div>
                <div className="grid grid-cols-1 gap-4 md:grid-cols-2">
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
      {refreshSuccess && (
        <div className="theme-toast-success fixed bottom-6 left-1/2 -translate-x-1/2 rounded-lg px-4 py-3 text-sm shadow-lg">
          Usage refreshed successfully
        </div>
      )}
      {warmupToast && (
        <div
          className={`fixed bottom-20 left-1/2 -translate-x-1/2 rounded-lg px-4 py-3 text-sm shadow-lg ${
            warmupToast.isError
              ? "theme-toast-danger"
              : "theme-toast-warning"
          }`}
        >
          {warmupToast.message}
        </div>
      )}
      {deleteConfirmId && (
        <div className="theme-toast-danger fixed bottom-6 left-1/2 -translate-x-1/2 rounded-lg px-4 py-3 text-sm shadow-lg">
          Click delete again to confirm removal
        </div>
      )}

      <AddAccountModal
        isOpen={isAddModalOpen}
        onClose={() => setIsAddModalOpen(false)}
        onImportFile={importFromFile}
        onStartOAuth={startOAuthLogin}
        onCompleteOAuth={completeOAuthLogin}
        onCancelOAuth={cancelOAuthLogin}
      />

      {isConfigModalOpen && (
        <div className="theme-scrim fixed inset-0 z-50 flex items-center justify-center px-4">
          <div
            className={`w-full max-w-2xl rounded-2xl shadow-xl shadow-slate-900/15 dark:shadow-slate-950/60 ${panelClass}`}
            style={{ backgroundColor: "var(--theme-surface)" }}
          >
            <div
              className="flex items-center justify-between border-b p-5"
              style={{ borderColor: "var(--theme-border-subtle)" }}
            >
              <h2 className="text-lg font-semibold text-slate-900 dark:text-slate-50">
                {configModalMode === "slim_export" ? "Export Slim Text" : "Import Slim Text"}
              </h2>
              <button
                onClick={() => setIsConfigModalOpen(false)}
                className="text-slate-400 transition-colors hover:text-slate-600 dark:text-slate-500 dark:hover:text-slate-300"
              >
                Close
              </button>
            </div>
            <div className="space-y-4 p-5">
              {configModalMode === "slim_import" ? (
                <p className="theme-toast-warning rounded-lg px-3 py-2 text-sm">
                  Existing accounts are kept. Only missing accounts are imported.
                </p>
              ) : (
                <p className="text-sm text-slate-500 dark:text-slate-400">
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
                className="theme-field h-48 w-full rounded-lg px-4 py-3 font-mono text-sm"
              />
              {configModalError && (
                <div className="theme-toast-danger rounded-lg p-3 text-sm">
                  {configModalError}
                </div>
              )}
            </div>
            <div
              className="flex gap-3 border-t p-5"
              style={{ borderColor: "var(--theme-border-subtle)" }}
            >
              <button
                onClick={() => setIsConfigModalOpen(false)}
                className={softButtonClass}
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
                  className={primaryButtonClass}
                >
                  {configCopied ? "Copied" : "Copy String"}
                </button>
              ) : (
                <button
                  onClick={handleImportSlimText}
                  disabled={isImportingSlim}
                  className={primaryButtonClass}
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
