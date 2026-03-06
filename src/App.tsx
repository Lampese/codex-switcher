import { useState, useEffect, useCallback, useMemo, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open, save } from "@tauri-apps/plugin-dialog";
import { useAccounts } from "./hooks/useAccounts";
import {
  AccountCard,
  AddAccountModal,
  MissedScheduledWarmupModal,
  ScheduledWarmupsModal,
} from "./components";
import type {
  AppSettings,
  CodexProcessInfo,
  ExportSecurityMode,
  ScheduledWarmupEvent,
  ScheduledWarmupSettings,
  ScheduledWarmupStatus,
  WarmupSummary,
} from "./types";
import "./App.css";

const SECURITY_OPTIONS: Array<{
  mode: ExportSecurityMode;
  title: string;
  description: string;
  badge?: string;
}> = [
  {
    mode: "keychain",
    title: "OS Keychain",
    description:
      "Best for this device. Full backups use a secret stored in your operating system keychain.",
    badge: "Recommended",
  },
  {
    mode: "passphrase",
    title: "Passphrase",
    description:
      "Portable encrypted backups. You will enter the passphrase when exporting and importing.",
  },
  {
    mode: "less_secure",
    title: "Less Secure",
    description:
      "Keeps the current built-in fallback secret for compatibility, but it is weaker than the other options.",
  },
];

function formatScheduledTime(localTime: string | null | undefined) {
  if (!localTime) return null;
  const [hours, minutes] = localTime.split(":").map(Number);
  if (Number.isNaN(hours) || Number.isNaN(minutes)) return localTime;

  const value = new Date();
  value.setHours(hours, minutes, 0, 0);
  return new Intl.DateTimeFormat(undefined, {
    hour: "numeric",
    minute: "2-digit",
  }).format(value);
}

function formatNextRun(nextRunLocalIso: string | null | undefined) {
  if (!nextRunLocalIso) return null;
  const value = new Date(nextRunLocalIso);
  if (Number.isNaN(value.getTime())) return null;

  return new Intl.DateTimeFormat(undefined, {
    weekday: "short",
    hour: "numeric",
    minute: "2-digit",
  }).format(value);
}

function summarizeWarmup(summary: WarmupSummary) {
  if (summary.total_accounts === 0) {
    return { message: "No scheduled accounts were available to warm", isError: true };
  }

  if (summary.failed_account_ids.length === 0) {
    return {
      message: `Warm-up sent for ${summary.warmed_accounts} scheduled account${
        summary.warmed_accounts === 1 ? "" : "s"
      }`,
      isError: false,
    };
  }

  return {
    message: `Warmed ${summary.warmed_accounts}/${summary.total_accounts}. Failed: ${summary.failed_account_ids.length}`,
    isError: true,
  };
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
    getAppSettings,
    saveExportSecurityMode,
    saveScheduledWarmupSettings,
    getScheduledWarmupStatus,
    dismissMissedScheduledWarmup,
    runScheduledWarmupNow,
  } = useAccounts();

  const [isAddModalOpen, setIsAddModalOpen] = useState(false);
  const [isScheduledWarmupsModalOpen, setIsScheduledWarmupsModalOpen] = useState(false);
  const [isMissedScheduledWarmupModalOpen, setIsMissedScheduledWarmupModalOpen] = useState(false);
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
  const [appSettings, setAppSettings] = useState<AppSettings | null>(null);
  const [isSavingSecurityMode, setIsSavingSecurityMode] = useState(false);
  const [scheduledWarmupStatus, setScheduledWarmupStatus] =
    useState<ScheduledWarmupStatus | null>(null);
  const [isRunningMissedWarmup, setIsRunningMissedWarmup] = useState(false);
  const actionsMenuRef = useRef<HTMLDivElement | null>(null);

  const toggleMask = (accountId: string) => {
    setMaskedAccounts((prev) => {
      const next = new Set(prev);
      if (next.has(accountId)) {
        next.delete(accountId);
      } else {
        next.add(accountId);
      }
      return next;
    });
  };

  const allMasked =
    accounts.length > 0 && accounts.every((account) => maskedAccounts.has(account.id));

  const toggleMaskAll = () => {
    setMaskedAccounts((prev) => {
      const shouldMaskAll = !accounts.every((account) => prev.has(account.id));
      if (shouldMaskAll) {
        return new Set(accounts.map((account) => account.id));
      }
      return new Set();
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

  useEffect(() => {
    getAppSettings()
      .then(setAppSettings)
      .catch((err) => {
        console.error("Failed to load app settings:", err);
      });
  }, [getAppSettings]);

  const loadScheduledWarmupStatus = useCallback(async () => {
    try {
      const status = await getScheduledWarmupStatus();
      setScheduledWarmupStatus(status);
      setIsMissedScheduledWarmupModalOpen(status.missed_run_today);
      return status;
    } catch (err) {
      console.error("Failed to load scheduled warmup status:", err);
      return null;
    }
  }, [getScheduledWarmupStatus]);

  useEffect(() => {
    void loadScheduledWarmupStatus();
  }, [loadScheduledWarmupStatus]);

  useEffect(() => {
    if (!loading) {
      void loadScheduledWarmupStatus();
    }
  }, [accounts, loading, loadScheduledWarmupStatus]);

  useEffect(() => {
    let unsubscribe: (() => void) | undefined;

    const register = async () => {
      unsubscribe = await listen<ScheduledWarmupEvent>(
        "scheduled-warmup-result",
        ({ payload }) => {
          const toast = summarizeWarmup(payload.summary);
          showWarmupToast(toast.message, toast.isError);
          void loadScheduledWarmupStatus();
          getAppSettings().then(setAppSettings).catch((err) => {
            console.error("Failed to refresh app settings after scheduled warmup:", err);
          });
        }
      );
    };

    void register();

    return () => {
      if (unsubscribe) {
        unsubscribe();
      }
    };
  }, [getAppSettings, loadScheduledWarmupStatus]);

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
      const latestProcessInfo = await invoke<CodexProcessInfo>("check_codex_processes").catch(
        (err) => {
          console.error("Failed to check processes before switching:", err);
          return processInfo;
        }
      );

      if (latestProcessInfo) {
        setProcessInfo(latestProcessInfo);
      }

      const hasRunningCodex =
        !!latestProcessInfo &&
        (latestProcessInfo.count > 0 || latestProcessInfo.background_count > 0);

      let restartRunningCodex = false;
      if (hasRunningCodex) {
        restartRunningCodex = window.confirm(
          `Codex is running (${latestProcessInfo.count} foreground, ${latestProcessInfo.background_count} background). Do you want Codex Switcher to close and reopen it gracefully before switching accounts?`
        );

        if (!restartRunningCodex) {
          return;
        }
      }

      await switchAccount(accountId, restartRunningCodex);
      await checkProcesses();
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

  const handleWarmupAccount = async (accountId: string, accountName: string) => {
    try {
      setWarmingUpId(accountId);
      await warmupAccount(accountId);
      showWarmupToast(`Warm-up sent for ${accountName}`);
    } catch (err) {
      console.error("Failed to warm up account:", err);
      showWarmupToast(`Warm-up failed for ${accountName}`, true);
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
      showWarmupToast("Warm-up all failed", true);
    } finally {
      setIsWarmingAll(false);
    }
  };

  const handleSaveScheduledWarmup = async (schedule: ScheduledWarmupSettings) => {
    try {
      const nextSettings = await saveScheduledWarmupSettings(schedule);
      setAppSettings(nextSettings);
      const status = await loadScheduledWarmupStatus();
      const enabledSchedule = status?.schedule;
      if (enabledSchedule?.enabled) {
        showWarmupToast(
          `Scheduled warmups saved for ${formatScheduledTime(enabledSchedule.local_time) ?? enabledSchedule.local_time}`
        );
      } else {
        showWarmupToast("Scheduled warmups saved");
      }
    } catch (err) {
      console.error("Failed to save scheduled warmup settings:", err);
      throw err;
    }
  };

  const handleSkipMissedScheduledWarmup = async () => {
    try {
      const nextSettings = await dismissMissedScheduledWarmup();
      setAppSettings(nextSettings);
      setIsMissedScheduledWarmupModalOpen(false);
      await loadScheduledWarmupStatus();
      showWarmupToast("Skipped today's missed scheduled warmup");
    } catch (err) {
      console.error("Failed to dismiss missed scheduled warmup:", err);
      showWarmupToast("Failed to skip missed scheduled warmup", true);
    }
  };

  const handleRunMissedScheduledWarmup = async () => {
    try {
      setIsRunningMissedWarmup(true);
      const summary = await runScheduledWarmupNow();
      setIsMissedScheduledWarmupModalOpen(false);
      const nextSettings = await getAppSettings();
      setAppSettings(nextSettings);
      await loadScheduledWarmupStatus();
      const toast = summarizeWarmup(summary);
      showWarmupToast(toast.message, toast.isError);
    } catch (err) {
      console.error("Failed to run missed scheduled warmup:", err);
      showWarmupToast("Failed to run missed scheduled warmup", true);
    } finally {
      setIsRunningMissedWarmup(false);
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

      let passphrase: string | undefined;
      if (appSettings?.export_security_mode === "passphrase") {
        const entered = window.prompt("Enter a passphrase for this backup file:");
        if (!entered) return;

        const confirmed = window.prompt("Re-enter the passphrase to confirm:");
        if (entered !== confirmed) {
          showWarmupToast("Passphrases did not match", true);
          return;
        }

        passphrase = entered;
      }

      await exportAccountsFullEncryptedFile(selected, passphrase);
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

      let summary;
      try {
        summary = await importAccountsFullEncryptedFile(selected);
      } catch (err) {
        const message = err instanceof Error ? err.message : String(err);
        if (!message.includes("requires the passphrase")) {
          throw err;
        }

        const passphrase = window.prompt("Enter the passphrase used for this backup:");
        if (!passphrase) return;
        summary = await importAccountsFullEncryptedFile(selected, passphrase);
      }

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
  const needsSecurityOnboarding =
    accounts.length === 0 && appSettings && !appSettings.export_security_mode;
  const scheduledWarmupTimeLabel = formatScheduledTime(
    scheduledWarmupStatus?.schedule?.local_time ?? appSettings?.scheduled_warmup?.local_time
  );
  const nextScheduledRunLabel = formatNextRun(scheduledWarmupStatus?.next_run_local_iso);

  const handleSelectSecurityMode = async (mode: ExportSecurityMode) => {
    try {
      setIsSavingSecurityMode(true);
      const nextSettings = await saveExportSecurityMode(mode);
      setAppSettings(nextSettings);
      showWarmupToast(`Backup security mode set to ${mode.replace("_", " ")}`);
    } catch (err) {
      console.error("Failed to save export security mode:", err);
      showWarmupToast("Failed to save backup security mode", true);
    } finally {
      setIsSavingSecurityMode(false);
    }
  };

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
                  Multi-account manager for Codex CLI
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
                onClick={() => setIsScheduledWarmupsModalOpen(true)}
                disabled={accounts.length === 0}
                className="h-10 px-4 py-2 text-sm font-medium rounded-lg bg-white border border-gray-200 hover:bg-gray-50 text-gray-700 transition-colors disabled:opacity-50 shrink-0 whitespace-nowrap"
                title="Configure daily scheduled warmups"
              >
                {appSettings?.scheduled_warmup?.enabled ? "🕒 Scheduled On" : "🕒 Schedule"}
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
            <section className="rounded-3xl border border-gray-200 bg-white px-6 py-5 shadow-sm">
              <div className="flex flex-col gap-3 md:flex-row md:items-center md:justify-between">
                <div>
                  <p className="text-xs font-medium uppercase tracking-[0.2em] text-gray-500">
                    Scheduled Warmups
                  </p>
                  <h2 className="mt-2 text-lg font-semibold text-gray-900">
                    {appSettings?.scheduled_warmup?.enabled
                      ? `Daily at ${scheduledWarmupTimeLabel ?? appSettings.scheduled_warmup.local_time}`
                      : "Not enabled yet"}
                  </h2>
                  <p className="mt-1 text-sm text-gray-500">
                    {appSettings?.scheduled_warmup?.enabled
                      ? nextScheduledRunLabel
                        ? `Next run: ${nextScheduledRunLabel}`
                        : "Runs while Codex Switcher is open."
                      : "Choose accounts and a local time to keep them warm automatically."}
                  </p>
                </div>
                <button
                  onClick={() => setIsScheduledWarmupsModalOpen(true)}
                  className="rounded-xl bg-gray-900 px-4 py-2.5 text-sm font-medium text-white hover:bg-gray-800 transition-colors"
                >
                  {appSettings?.scheduled_warmup?.enabled ? "Edit Schedule" : "Set Schedule"}
                </button>
              </div>
            </section>

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

      {needsSecurityOnboarding && (
        <div className="fixed inset-0 z-50 bg-black/40 flex items-center justify-center p-4">
          <div className="w-full max-w-3xl rounded-3xl bg-white shadow-2xl border border-gray-200 overflow-hidden">
            <div className="border-b border-gray-100 px-6 py-5">
              <h2 className="text-xl font-semibold text-gray-900">
                Choose your backup security mode
              </h2>
              <p className="mt-2 text-sm text-gray-500">
                New users should choose how full backup files are protected before getting started.
              </p>
            </div>
            <div className="grid gap-4 p-6 md:grid-cols-3">
              {SECURITY_OPTIONS.map((option) => (
                <button
                  key={option.mode}
                  disabled={isSavingSecurityMode}
                  onClick={() => {
                    void handleSelectSecurityMode(option.mode);
                  }}
                  className="rounded-2xl border border-gray-200 p-5 text-left hover:border-gray-400 hover:shadow-md transition-all disabled:opacity-60"
                >
                  <div className="flex items-center justify-between gap-2">
                    <h3 className="text-base font-semibold text-gray-900">{option.title}</h3>
                    {option.badge && (
                      <span className="rounded-full bg-emerald-50 border border-emerald-200 px-2 py-0.5 text-xs font-medium text-emerald-700">
                        {option.badge}
                      </span>
                    )}
                  </div>
                  <p className="mt-3 text-sm leading-6 text-gray-500">{option.description}</p>
                </button>
              ))}
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

      <ScheduledWarmupsModal
        isOpen={isScheduledWarmupsModalOpen}
        accounts={accounts}
        initialValue={appSettings?.scheduled_warmup ?? null}
        nextRunLabel={nextScheduledRunLabel}
        onClose={() => setIsScheduledWarmupsModalOpen(false)}
        onSave={handleSaveScheduledWarmup}
      />

      <MissedScheduledWarmupModal
        isOpen={isMissedScheduledWarmupModalOpen}
        timeLabel={scheduledWarmupTimeLabel}
        accountCount={scheduledWarmupStatus?.valid_account_ids.length ?? 0}
        running={isRunningMissedWarmup}
        onRunNow={handleRunMissedScheduledWarmup}
        onSkipToday={handleSkipMissedScheduledWarmup}
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

    </div>
  );
}

export default App;
