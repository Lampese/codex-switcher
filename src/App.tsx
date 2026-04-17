import { useState, useEffect, useCallback, useMemo, useRef } from "react";
import { useAccounts } from "./hooks/useAccounts";
import { useAutoSwitch } from "./hooks/useAutoSwitch";
import { AccountCard, AddAccountModal, AutoSwitchSettings, UpdateChecker } from "./components";
import type { CodexProcessInfo, SwitchState } from "./types";
import {
  exportFullBackupFile,
  importFullBackupFile,
  invokeBackend,
} from "./lib/platform";
import "./App.css";

function App() {
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
    loadSwitchState,
    applyQueuedAccount,
    clearQueuedSwitch,
    queueAccountForNextSession,
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
  const [switchState, setSwitchState] = useState<SwitchState | null>(null);
  const [deleteConfirmId, setDeleteConfirmId] = useState<string | null>(null);
  const [processInfo, setProcessInfo] = useState<CodexProcessInfo | null>(null);
  const [isRefreshing, setIsRefreshing] = useState(false);
  const previousHadRunningProcessesRef = useRef(false);
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
  const [isAutoSwitchModalOpen, setIsAutoSwitchModalOpen] = useState(false);
  const actionsMenuRef = useRef<HTMLDivElement | null>(null);

  const {
    config: autoSwitchConfig,
    isRunning: isAutoSwitchRunning,
    events: autoSwitchEvents,
    loading: autoSwitchLoading,
    error: autoSwitchError,
    saveConfig: saveAutoSwitchConfig,
    start: startAutoSwitch,
    stop: stopAutoSwitch,
    clearEvents: clearAutoSwitchEvents,
  } = useAutoSwitch();

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
      setProcessInfo(info);

      const hasRunning = info.count > 0;
      const hadRunning = previousHadRunningProcessesRef.current;
      previousHadRunningProcessesRef.current = hasRunning;

      if (hadRunning && !hasRunning) {
        const result = await applyQueuedAccount();
        setSwitchState(result.state);
        if (result.outcome === "applied_queued" && result.account_id) {
          const accountList = await loadAccounts(true);
          const applied = accountList.find((account) => account.id === result.account_id);
          showWarmupToast(
            `Applied queued account: ${applied?.name ?? "Next account"}`
          );
        }
      }
    } catch (err) {
      console.error("Failed to check processes:", err);
    }
  }, [applyQueuedAccount, loadAccounts]);

  // Check processes on mount and periodically
  useEffect(() => {
    checkProcesses();
    const interval = setInterval(checkProcesses, 3000); // Check every 3 seconds
    return () => clearInterval(interval);
  }, [checkProcesses]);

  useEffect(() => {
    loadSwitchState()
      .then((state) => setSwitchState(state))
      .catch((err) => console.error("Failed to load switch state:", err));
  }, [loadSwitchState]);

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

  const handleSwitch = async (accountId: string) => {
    try {
      setSwitchingId(accountId);
      const result = await switchAccount(accountId);
      setSwitchState(result.state);

      if (result.outcome === "queued_for_next_session") {
        const queued = accounts.find((account) => account.id === accountId);
        showWarmupToast(
          `Queued ${queued?.name ?? "account"} for the next session`
        );
      } else if (result.outcome === "switched_now") {
        const switched = accounts.find((account) => account.id === accountId);
        showWarmupToast(`Switched to ${switched?.name ?? "account"}`);
      }
    } catch (err) {
      console.error("Failed to switch account:", err);
      showWarmupToast(`Switch failed: ${formatWarmupError(err)}`, true);
    } finally {
      setSwitchingId(null);
      await checkProcesses();
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

  const handleApplyQueued = async () => {
    try {
      const result = await applyQueuedAccount();
      setSwitchState(result.state);
      if (result.outcome === "applied_queued" && result.account_id) {
        const accountList = await loadAccounts(true);
        const applied = accountList.find((account) => account.id === result.account_id);
        showWarmupToast(`Applied queued account: ${applied?.name ?? "account"}`);
      } else {
        showWarmupToast("No queued account was applied");
      }
    } catch (err) {
      console.error("Failed to apply queued account:", err);
      showWarmupToast(`Apply failed: ${formatWarmupError(err)}`, true);
    }
  };

  const handleClearQueued = async () => {
    try {
      const result = await clearQueuedSwitch();
      setSwitchState(result.state);
      showWarmupToast("Cleared queued next-session account");
    } catch (err) {
      console.error("Failed to clear queued switch:", err);
      showWarmupToast(`Clear failed: ${formatWarmupError(err)}`, true);
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
      const exported = await exportFullBackupFile();
      if (!exported) return;
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
      const summary = await importFullBackupFile();
      if (!summary) return;
      const accountList = await loadAccounts();
      await refreshUsage(accountList);
      const maskedIds = await loadMaskedAccountIds();
      setMaskedAccounts(new Set(maskedIds));
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
  const queuedAccount = switchState?.queued_account_id
    ? accounts.find((account) => account.id === switchState.queued_account_id)
    : undefined;

  const sortedOtherAccounts = useMemo(() => {
    const getResetDeadline = (resetAt: number | null | undefined) =>
      resetAt ?? Number.POSITIVE_INFINITY;

    const getRemainingPercent = (usedPercent: number | null | undefined) => {
      if (usedPercent === null || usedPercent === undefined) {
        return Number.NEGATIVE_INFINITY;
      }
      return Math.max(0, 100 - usedPercent);
    };

    return [...otherAccounts].sort((a, b) => {
      if (otherAccountsSort === "deadline_asc" || otherAccountsSort === "deadline_desc") {
        const deadlineDiff =
          getResetDeadline(a.usage?.primary_resets_at) -
          getResetDeadline(b.usage?.primary_resets_at);
        if (deadlineDiff !== 0) {
          return otherAccountsSort === "deadline_asc" ? deadlineDiff : -deadlineDiff;
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

  const evaluateAutoQueueCandidate = useCallback(() => {
    if (!autoSwitchConfig?.enabled || !activeAccount?.usage) return null;
    const activeUsage = activeAccount.usage.primary_used_percent;
    if (activeUsage == null || activeUsage < autoSwitchConfig.threshold_percent) return null;

    const excluded = new Set(autoSwitchConfig.excluded_account_ids);
    const priority = new Map(autoSwitchConfig.priority_order.map((id, index) => [id, index]));

    const candidates = otherAccounts.filter((account) => {
      if (excluded.has(account.id)) return false;
      if (maskedAccounts.has(account.id)) return false;
      if (!account.usage || account.usage.error) return false;
      if (account.usage.primary_used_percent == null) return false;
      if (account.usage.primary_used_percent >= autoSwitchConfig.threshold_percent) return false;
      if (
        autoSwitchConfig.respect_weekly_limit &&
        account.usage.secondary_used_percent != null &&
        account.usage.secondary_used_percent >= autoSwitchConfig.threshold_percent
      ) {
        return false;
      }
      return true;
    });

    if (candidates.length === 0) return null;

    return [...candidates].sort((a, b) => {
      const priorityDiff = (priority.get(a.id) ?? Number.MAX_SAFE_INTEGER) - (priority.get(b.id) ?? Number.MAX_SAFE_INTEGER);
      if (priorityDiff !== 0) return priorityDiff;

      const remainingA = 100 - (a.usage?.primary_used_percent ?? 100);
      const remainingB = 100 - (b.usage?.primary_used_percent ?? 100);
      if (remainingB !== remainingA) return remainingB - remainingA;

      const resetA = a.usage?.primary_resets_at ?? Number.POSITIVE_INFINITY;
      const resetB = b.usage?.primary_resets_at ?? Number.POSITIVE_INFINITY;
      if (resetA !== resetB) return resetA - resetB;

      return a.name.localeCompare(b.name);
    })[0];
  }, [activeAccount, autoSwitchConfig, maskedAccounts, otherAccounts]);

  useEffect(() => {
    const candidate = evaluateAutoQueueCandidate();
    if (!candidate) return;
    if (switchState?.queued_account_id === candidate.id) return;

    void queueAccountForNextSession(candidate.id, "threshold")
      .then((result) => {
        setSwitchState(result.state);
        showWarmupToast(`Auto-queued ${candidate.name} for the next session`);
      })
      .catch((err) => {
        console.error("Failed to auto-queue account:", err);
      });
  }, [evaluateAutoQueueCandidate, queueAccountForNextSession, switchState?.queued_account_id]);

  return (
    <div className="min-h-screen bg-gray-50">
      {/* Header */}
      <header className="sticky top-0 z-40 bg-white border-b border-gray-200">
        <div className="max-w-5xl mx-auto px-6 py-4">
          <div className="grid grid-cols-1 gap-3 md:grid-cols-[minmax(0,1fr)_max-content] md:items-center md:gap-4">
            <div className="flex items-center gap-3 min-w-0 flex-1">
              <div className="h-10 w-10 rounded-xl bg-gray-900 flex items-center justify-center text-white font-bold text-lg">
                C
              </div>
              <div className="min-w-0">
                <div className="flex items-center gap-2 flex-wrap">
                  <h1 className="text-xl font-bold text-gray-900 tracking-tight">
                    Codex Switcher
                  </h1>
                  {processInfo && (
                    <span
                      className={`inline-flex items-center gap-1 px-2 py-0.5 rounded-md text-xs border ${hasRunningProcesses
                          ? "bg-amber-50 text-amber-700 border-amber-200"
                          : "bg-green-50 text-green-700 border-green-200"
                        }`}
                    >
                      <span
                        className={`inline-block w-1.5 h-1.5 rounded-full ${hasRunningProcesses ? "bg-amber-500" : "bg-green-500"
                          }`}
                      ></span>
                      <span>
                        {hasRunningProcesses
                          ? `${processInfo.count} Codex running`
                          : "0 Codex running"}
                      </span>
                    </span>
                  )}
                </div>
                <p className="text-xs text-gray-500">
                  Multi-account manager for Codex CLI · auto-switch applies on the next session only
                </p>
              </div>
            </div>

            <div className="flex flex-wrap items-center gap-2 shrink-0 md:ml-4 md:w-max md:flex-nowrap md:justify-end">
              <button
                onClick={toggleMaskAll}
                className="h-10 px-4 py-2 text-sm font-medium rounded-lg bg-gray-100 hover:bg-gray-200 text-gray-700 transition-colors shrink-0 whitespace-nowrap"
                title={allMasked ? "Show all account names and emails" : "Hide all account names and emails"}
              >
                <span className="flex items-center gap-2">
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
                  {allMasked ? "Show All" : "Hide All"}
                </span>
              </button>
              <button
                onClick={handleRefresh}
                disabled={isRefreshing}
                className="h-10 px-4 py-2 text-sm font-medium rounded-lg bg-gray-100 hover:bg-gray-200 text-gray-700 transition-colors disabled:opacity-50 shrink-0 whitespace-nowrap"
              >
                {isRefreshing ? "↻ Refreshing..." : "↻ Refresh All"}
              </button>
              <button
                onClick={handleWarmupAll}
                disabled={isWarmingAll || accounts.length === 0}
                className="h-10 px-4 py-2 text-sm font-medium rounded-lg bg-gray-100 hover:bg-gray-200 text-gray-700 transition-colors disabled:opacity-50 shrink-0 whitespace-nowrap"
                title="Send minimal traffic using all accounts"
              >
                {isWarmingAll ? (
                  <span className="flex items-center gap-2">
                    <span className="animate-pulse">⚡</span> Warming...
                  </span>
                ) : (
                  <span className="flex items-center gap-2">
                    <span>⚡</span> Warm-up All
                  </span>
                )}
              </button>
              <button
                onClick={() => setIsAutoSwitchModalOpen(true)}
                className={`h-10 px-4 py-2 text-sm font-medium rounded-lg transition-colors shrink-0 whitespace-nowrap ${
                  isAutoSwitchRunning
                    ? "bg-green-100 text-green-700 hover:bg-green-200"
                    : "bg-gray-100 text-gray-700 hover:bg-gray-200"
                }`}
                title="Auto-switch settings"
              >
                <span className="flex items-center gap-2">
                  <span>{isAutoSwitchRunning ? "🔄" : "⚙️"}</span>
                  {isAutoSwitchRunning ? "Auto" : "Auto-Switch"}
                </span>
              </button>

              <div className="relative" ref={actionsMenuRef}>
                <button
                  onClick={() => setIsActionsMenuOpen((prev) => !prev)}
                  className="h-10 px-4 py-2 text-sm font-medium rounded-lg bg-gray-900 hover:bg-gray-800 text-white transition-colors shrink-0 whitespace-nowrap"
                >
                  Account ▾
                </button>
                {isActionsMenuOpen && (
                  <div className="absolute right-0 mt-2 w-56 rounded-xl border border-gray-200 bg-white shadow-xl p-2 z-50">
                    <button
                      onClick={() => {
                        setIsActionsMenuOpen(false);
                        setIsAddModalOpen(true);
                      }}
                      className="w-full text-left px-3 py-2 text-sm rounded-lg hover:bg-gray-100 text-gray-700"
                    >
                      + Add Account
                    </button>
                    <button
                      onClick={() => {
                        setIsActionsMenuOpen(false);
                        void handleExportSlimText();
                      }}
                      disabled={isExportingSlim}
                      className="w-full text-left px-3 py-2 text-sm rounded-lg hover:bg-gray-100 text-gray-700 disabled:opacity-50"
                    >
                      {isExportingSlim ? "Exporting..." : "Export Slim Text"}
                    </button>
                    <button
                      onClick={() => {
                        setIsActionsMenuOpen(false);
                        openImportSlimTextModal();
                      }}
                      disabled={isImportingSlim}
                      className="w-full text-left px-3 py-2 text-sm rounded-lg hover:bg-gray-100 text-gray-700 disabled:opacity-50"
                    >
                      {isImportingSlim ? "Importing..." : "Import Slim Text"}
                    </button>
                    <button
                      onClick={() => {
                        setIsActionsMenuOpen(false);
                        void handleExportFullFile();
                      }}
                      disabled={isExportingFull}
                      className="w-full text-left px-3 py-2 text-sm rounded-lg hover:bg-gray-100 text-gray-700 disabled:opacity-50"
                    >
                      {isExportingFull ? "Exporting..." : "Export Full Encrypted File"}
                    </button>
                    <button
                      onClick={() => {
                        setIsActionsMenuOpen(false);
                        void handleImportFullFile();
                      }}
                      disabled={isImportingFull}
                      className="w-full text-left px-3 py-2 text-sm rounded-lg hover:bg-gray-100 text-gray-700 disabled:opacity-50"
                    >
                      {isImportingFull ? "Importing..." : "Import Full Encrypted File"}
                    </button>
                  </div>
                )}
              </div>
            </div>
          </div>
        </div>
      </header>

      {/* Main Content */}
      <main className="max-w-5xl mx-auto px-6 py-8">
        <div className="space-y-4 mb-6">
          <div className="rounded-2xl border border-gray-200 dark:border-gray-800 bg-white dark:bg-gray-900 p-4">
            <div className="flex flex-col gap-3 md:flex-row md:items-center md:justify-between">
              <div>
                <p className="text-sm font-semibold text-gray-900 dark:text-gray-100">
                  Auto-switch (next session only)
                </p>
                <p className="text-sm text-gray-600 dark:text-gray-300 mt-1">
                  Running Codex sessions are never interrupted. When you switch while Codex is active,
                  the next account is queued and applied after the current session ends.
                </p>
              </div>
              <div className="text-xs text-gray-500 dark:text-gray-400">
                Policy: {switchState?.switch_policy ?? "next_session_only"}
              </div>
            </div>
          </div>

          {queuedAccount && (
            <div className="rounded-2xl border border-sky-200 dark:border-sky-800 bg-sky-50 dark:bg-sky-950/30 p-4">
              <div className="flex flex-col gap-3 md:flex-row md:items-center md:justify-between">
                <div>
                  <p className="text-sm font-semibold text-sky-900 dark:text-sky-200">
                    Next account queued: {queuedAccount.name}
                  </p>
                  <p className="text-sm text-sky-700 dark:text-sky-300 mt-1">
                    {hasRunningProcesses
                      ? "Will be applied automatically after the current Codex session ends."
                      : "Ready to apply now. No Codex process is currently running."}
                  </p>
                </div>
                <div className="flex gap-2">
                  {!hasRunningProcesses && (
                    <button
                      onClick={handleApplyQueued}
                      className="px-4 py-2 text-sm font-medium rounded-lg bg-sky-600 hover:bg-sky-700 text-white transition-colors"
                    >
                      Apply now
                    </button>
                  )}
                  <button
                    onClick={handleClearQueued}
                    className="px-4 py-2 text-sm font-medium rounded-lg bg-white dark:bg-gray-900 border border-sky-200 dark:border-sky-700 text-sky-700 dark:text-sky-200 hover:bg-sky-100 dark:hover:bg-sky-900/40 transition-colors"
                  >
                    Clear queue
                  </button>
                </div>
              </div>
            </div>
          )}

          {hasRunningProcesses && !queuedAccount && (
            <div className="rounded-2xl border border-amber-200 dark:border-amber-800 bg-amber-50 dark:bg-amber-950/30 p-4 text-sm text-amber-800 dark:text-amber-200">
              Codex is running. Immediate account switching is disabled; selecting another account will queue it for the next session.
            </div>
          )}
        </div>

        {loading && accounts.length === 0 ? (
          <div className="flex flex-col items-center justify-center py-20">
            <div className="animate-spin h-10 w-10 border-2 border-gray-900 border-t-transparent rounded-full mb-4"></div>
            <p className="text-gray-500">Loading accounts...</p>
          </div>
        ) : error ? (
          <div className="text-center py-20">
            <div className="text-red-600 mb-2">Failed to load accounts</div>
            <p className="text-sm text-gray-500">{error}</p>
          </div>
        ) : accounts.length === 0 ? (
          <div className="text-center py-20">
            <div className="h-16 w-16 rounded-2xl bg-gray-100 flex items-center justify-center mx-auto mb-4">
              <span className="text-3xl">👤</span>
            </div>
            <h2 className="text-xl font-semibold text-gray-900 mb-2">
              No accounts yet
            </h2>
            <p className="text-gray-500 mb-6">
              Add your first Codex account to get started
            </p>
            <button
              onClick={() => setIsAddModalOpen(true)}
              className="px-6 py-3 text-sm font-medium rounded-lg bg-gray-900 hover:bg-gray-800 text-white transition-colors"
            >
              Add Account
            </button>
          </div>
        ) : (
          <div className="space-y-8">
            {/* Active Account */}
            {activeAccount && (
              <section>
                <h2 className="text-sm font-medium text-gray-500 uppercase tracking-wider mb-4">
                  Active Account
                </h2>
                <AccountCard
                  account={activeAccount}
                  onSwitch={() => { }}
                  onWarmup={() =>
                    handleWarmupAccount(activeAccount.id, activeAccount.name)
                  }
                  onDelete={() => handleDelete(activeAccount.id)}
                  onRefresh={() => refreshSingleUsage(activeAccount.id)}
                  onRename={(newName) => renameAccount(activeAccount.id, newName)}
                  switching={switchingId === activeAccount.id}
                  switchDisabled={false}
                  warmingUp={isWarmingAll || warmingUpId === activeAccount.id}
                  masked={maskedAccounts.has(activeAccount.id)}
                  onToggleMask={() => toggleMask(activeAccount.id)}
                  switchLabel="Active now"
                />
              </section>
            )}

            {/* Other Accounts */}
            {otherAccounts.length > 0 && (
              <section>
                <div className="flex items-center justify-between gap-3 mb-4">
                  <h2 className="text-sm font-medium text-gray-500 uppercase tracking-wider">
                    Other Accounts ({otherAccounts.length})
                  </h2>
                  <div className="flex items-center gap-2">
                    <label htmlFor="other-accounts-sort" className="text-xs text-gray-500">
                      Sort
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
                          )
                        }
                        className="appearance-none font-sans text-xs sm:text-sm font-medium pl-3 pr-9 py-2 rounded-xl border border-gray-300 bg-gradient-to-b from-white to-gray-50 text-gray-700 shadow-sm hover:border-gray-400 hover:shadow focus:outline-none focus:ring-2 focus:ring-gray-300 focus:border-gray-400 transition-all"
                      >
                        <option value="deadline_asc">Reset: earliest to latest</option>
                        <option value="deadline_desc">Reset: latest to earliest</option>
                        <option value="remaining_desc">
                          % remaining: highest to lowest
                        </option>
                        <option value="remaining_asc">
                          % remaining: lowest to highest
                        </option>
                      </select>
                      <span className="pointer-events-none absolute inset-y-0 right-3 flex items-center text-gray-500">
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
      <UpdateChecker />

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
                      onRefresh={() => refreshSingleUsage(account.id)}
                      onRename={(newName) => renameAccount(account.id, newName)}
                      switching={switchingId === account.id}
                      switchDisabled={false}
                      queued={switchState?.queued_account_id === account.id}
                      switchLabel={
                        switchState?.queued_account_id === account.id
                          ? "Queued Next"
                          : hasRunningProcesses
                            ? "Queue Next"
                            : "Switch Now"
                      }
                      switchTitle={
                        switchState?.queued_account_id === account.id
                          ? "This account will be applied when the current Codex session ends."
                          : hasRunningProcesses
                            ? "Codex is running. This will queue the account for the next session."
                            : "Apply this account immediately because no Codex process is running."
                      }
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

      {/* Refresh Success Toast */}
      {refreshSuccess && (
        <div className="fixed bottom-6 left-1/2 -translate-x-1/2 px-4 py-3 bg-green-600 text-white rounded-lg shadow-lg text-sm flex items-center gap-2">
          <span>✓</span> Usage refreshed successfully
        </div>
      )}

      {/* Warm-up Toast */}
      {warmupToast && (
        <div
          className={`fixed bottom-20 left-1/2 -translate-x-1/2 px-4 py-3 rounded-lg shadow-lg text-sm ${
            warmupToast.isError
              ? "bg-red-600 text-white"
              : "bg-amber-100 text-amber-900 border border-amber-300"
          }`}
        >
          {warmupToast.message}
        </div>
      )}

      {/* Delete Confirmation Toast */}
      {deleteConfirmId && (
        <div className="fixed bottom-6 left-1/2 -translate-x-1/2 px-4 py-3 bg-red-600 text-white rounded-lg shadow-lg text-sm">
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
          <div className="bg-white border border-gray-200 rounded-2xl w-full max-w-2xl mx-4 shadow-xl">
            <div className="flex items-center justify-between p-5 border-b border-gray-100">
              <h2 className="text-lg font-semibold text-gray-900">
                {configModalMode === "slim_export" ? "Export Slim Text" : "Import Slim Text"}
              </h2>
              <button
                onClick={() => setIsConfigModalOpen(false)}
                className="text-gray-400 hover:text-gray-600 transition-colors"
              >
                ✕
              </button>
            </div>
            <div className="p-5 space-y-4">
              {configModalMode === "slim_import" ? (
                <p className="text-sm text-amber-700 bg-amber-50 border border-amber-200 rounded-lg px-3 py-2">
                  Existing accounts are kept. Only missing accounts are imported.
                </p>
              ) : (
                <p className="text-sm text-gray-500">
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
                className="w-full h-48 px-4 py-3 bg-gray-50 border border-gray-200 rounded-lg text-sm text-gray-800 placeholder-gray-400 focus:outline-none focus:border-gray-400 focus:ring-1 focus:ring-gray-400 font-mono"
              />
              {configModalError && (
                <div className="p-3 bg-red-50 border border-red-200 rounded-lg text-red-600 text-sm">
                  {configModalError}
                </div>
              )}
            </div>
            <div className="flex gap-3 p-5 border-t border-gray-100">
              <button
                onClick={() => setIsConfigModalOpen(false)}
                className="px-4 py-2.5 text-sm font-medium rounded-lg bg-gray-100 hover:bg-gray-200 text-gray-700 transition-colors"
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
                  className="px-4 py-2.5 text-sm font-medium rounded-lg bg-gray-900 hover:bg-gray-800 text-white transition-colors disabled:opacity-50"
                >
                  {configCopied ? "Copied" : "Copy String"}
                </button>
              ) : (
                <button
                  onClick={handleImportSlimText}
                  disabled={isImportingSlim}
                  className="px-4 py-2.5 text-sm font-medium rounded-lg bg-gray-900 hover:bg-gray-800 text-white transition-colors disabled:opacity-50"
                >
                  {isImportingSlim ? "Importing..." : "Import Missing Accounts"}
                </button>
              )}
            </div>
          </div>
        </div>
      )}

      {/* Auto-Switch Settings Modal */}
      {isAutoSwitchModalOpen && (
        <div className="fixed inset-0 bg-black/40 flex items-center justify-center z-50">
          <div className="bg-white border border-gray-200 rounded-2xl w-full max-w-md mx-4 shadow-xl">
            <div className="flex items-center justify-between p-5 border-b border-gray-100">
              <h2 className="text-lg font-semibold text-gray-900">
                Auto-Switch Settings
              </h2>
              <button
                onClick={() => setIsAutoSwitchModalOpen(false)}
                className="text-gray-400 hover:text-gray-600 transition-colors"
              >
                ✕
              </button>
            </div>
            <div className="p-5">
              <AutoSwitchSettings
                config={autoSwitchConfig}
                isRunning={isAutoSwitchRunning}
                events={autoSwitchEvents}
                accounts={accounts}
                loading={autoSwitchLoading}
                error={autoSwitchError}
                onConfigChange={saveAutoSwitchConfig}
                onStart={startAutoSwitch}
                onStop={stopAutoSwitch}
                onClearEvents={clearAutoSwitchEvents}
              />
            </div>
          </div>
        </div>
      )}

    </div>
  );
}

export default App;
