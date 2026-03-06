import { useState, useEffect, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import type {
  AccountInfo,
  UsageInfo,
  AccountWithUsage,
  WarmupSummary,
  ImportAccountsSummary,
  ExportSecurityMode,
  ScheduledWarmupSettings,
  ScheduledWarmupStatus,
  AppSettings,
} from "../types";

export function useAccounts() {
  const [accounts, setAccounts] = useState<AccountWithUsage[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const loadAccounts = useCallback(async (preserveUsage = false) => {
    try {
      setLoading(true);
      setError(null);
      const accountList = await invoke<AccountInfo[]>("list_accounts");
      
      if (preserveUsage) {
        // Preserve existing usage data when just updating account info
        setAccounts((prev) => {
          const usageMap = new Map(prev.map((a) => [a.id, a.usage]));
          return accountList.map((a) => ({
            ...a,
            usage: usageMap.get(a.id),
          }));
        });
      } else {
        setAccounts(accountList.map((a) => ({ ...a })));
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  }, []);

  const refreshUsage = useCallback(async () => {
    try {
      const usageList = await invoke<UsageInfo[]>("refresh_all_accounts_usage");
      setAccounts((prev) =>
        prev.map((account) => {
          const usage = usageList.find((u) => u.account_id === account.id);
          return { ...account, usage, usageLoading: false };
        })
      );
    } catch (err) {
      console.error("Failed to refresh usage:", err);
      throw err;
    }
  }, []);

  const refreshSingleUsage = useCallback(async (accountId: string) => {
    try {
      setAccounts((prev) =>
        prev.map((a) =>
          a.id === accountId ? { ...a, usageLoading: true } : a
        )
      );
      const usage = await invoke<UsageInfo>("get_usage", { accountId });
      setAccounts((prev) =>
        prev.map((a) =>
          a.id === accountId ? { ...a, usage, usageLoading: false } : a
        )
      );
    } catch (err) {
      console.error("Failed to refresh single usage:", err);
      setAccounts((prev) =>
        prev.map((a) =>
          a.id === accountId ? { ...a, usageLoading: false } : a
        )
      );
      throw err;
    }
  }, []);

  const warmupAccount = useCallback(async (accountId: string) => {
    try {
      await invoke("warmup_account", { accountId });
    } catch (err) {
      console.error("Failed to warm up account:", err);
      throw err;
    }
  }, []);

  const warmupAllAccounts = useCallback(async () => {
    try {
      return await invoke<WarmupSummary>("warmup_all_accounts");
    } catch (err) {
      console.error("Failed to warm up all accounts:", err);
      throw err;
    }
  }, []);

  const switchAccount = useCallback(
    async (accountId: string, restartRunningCodex?: boolean) => {
      try {
        await invoke("switch_account", { accountId, restartRunningCodex });
        await loadAccounts(true); // Preserve usage data
      } catch (err) {
        throw err;
      }
    },
    [loadAccounts]
  );

  const deleteAccount = useCallback(
    async (accountId: string) => {
      try {
        await invoke("delete_account", { accountId });
        await loadAccounts();
      } catch (err) {
        throw err;
      }
    },
    [loadAccounts]
  );

  const renameAccount = useCallback(
    async (accountId: string, newName: string) => {
      try {
        await invoke("rename_account", { accountId, newName });
        await loadAccounts(true); // Preserve usage data
      } catch (err) {
        throw err;
      }
    },
    [loadAccounts]
  );

  const importFromFile = useCallback(
    async (path: string, name: string) => {
      try {
        await invoke<AccountInfo>("add_account_from_file", { path, name });
        await loadAccounts();
        await refreshUsage();
      } catch (err) {
        throw err;
      }
    },
    [loadAccounts, refreshUsage]
  );

  const startOAuthLogin = useCallback(async (accountName: string) => {
    try {
      const info = await invoke<{ auth_url: string; callback_port: number }>(
        "start_login",
        { accountName }
      );
      return info;
    } catch (err) {
      throw err;
    }
  }, []);

  const completeOAuthLogin = useCallback(async () => {
    try {
      const account = await invoke<AccountInfo>("complete_login");
      await loadAccounts();
      await refreshUsage();
      return account;
    } catch (err) {
      throw err;
    }
  }, [loadAccounts, refreshUsage]);

  const exportAccountsSlimText = useCallback(async () => {
    try {
      return await invoke<string>("export_accounts_slim_text");
    } catch (err) {
      throw err;
    }
  }, []);

  const importAccountsSlimText = useCallback(
    async (payload: string) => {
      try {
        const summary = await invoke<ImportAccountsSummary>("import_accounts_slim_text", {
          payload,
        });
        await loadAccounts();
        await refreshUsage();
        return summary;
      } catch (err) {
        throw err;
      }
    },
    [loadAccounts, refreshUsage]
  );

  const exportAccountsFullEncryptedFile = useCallback(
    async (path: string, passphrase?: string) => {
      try {
        await invoke("export_accounts_full_encrypted_file", { path, passphrase });
      } catch (err) {
        throw err;
      }
    },
    []
  );

  const importAccountsFullEncryptedFile = useCallback(
    async (path: string, passphrase?: string) => {
      try {
        const summary = await invoke<ImportAccountsSummary>(
          "import_accounts_full_encrypted_file",
          { path, passphrase }
        );
        await loadAccounts();
        await refreshUsage();
        return summary;
      } catch (err) {
        throw err;
      }
    },
    [loadAccounts, refreshUsage]
  );

  const cancelOAuthLogin = useCallback(async () => {
    try {
      await invoke("cancel_login");
    } catch (err) {
      console.error("Failed to cancel login:", err);
    }
  }, []);

  const getAppSettings = useCallback(async () => {
    return await invoke<AppSettings>("get_app_settings");
  }, []);

  const saveExportSecurityMode = useCallback(async (mode: ExportSecurityMode) => {
    return await invoke<AppSettings>("save_export_security_mode", { mode });
  }, []);

  const saveScheduledWarmupSettings = useCallback(async (schedule: ScheduledWarmupSettings) => {
    return await invoke<AppSettings>("save_scheduled_warmup_settings", { schedule });
  }, []);

  const getScheduledWarmupStatus = useCallback(async () => {
    return await invoke<ScheduledWarmupStatus>("get_scheduled_warmup_status");
  }, []);

  const dismissMissedScheduledWarmup = useCallback(async () => {
    return await invoke<AppSettings>("dismiss_missed_scheduled_warmup");
  }, []);

  const runScheduledWarmupNow = useCallback(async () => {
    return await invoke<WarmupSummary>("run_scheduled_warmup_now");
  }, []);

  useEffect(() => {
    loadAccounts().then(() => refreshUsage());
    
    // Auto-refresh usage every 60 seconds (same as official Codex CLI)
    const interval = setInterval(() => {
      refreshUsage().catch(() => {});
    }, 60000);
    
    return () => clearInterval(interval);
  }, [loadAccounts, refreshUsage]);

  return {
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
  };
}
