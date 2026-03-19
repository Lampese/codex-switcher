import { useState, useEffect, useCallback, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import type {
  AccountInfo,
  UsageInfo,
  AccountWithUsage,
  WarmupSummary,
  ImportAccountsSummary,
  LiveAuthSyncResult,
} from "../types";

export function useAccounts() {
  const [accounts, setAccounts] = useState<AccountWithUsage[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const accountsRef = useRef<AccountWithUsage[]>([]);
  const maxConcurrentUsageRequests = 10;

  useEffect(() => {
    accountsRef.current = accounts;
  }, [accounts]);

  const buildUsageError = useCallback(
    (accountId: string, message: string, planType: string | null): UsageInfo => ({
      account_id: accountId,
      plan_type: planType,
      primary_used_percent: null,
      primary_window_minutes: null,
      primary_resets_at: null,
      secondary_used_percent: null,
      secondary_window_minutes: null,
      secondary_resets_at: null,
      has_credits: null,
      unlimited_credits: null,
      credits_balance: null,
      error: message,
    }),
    []
  );

  const runWithConcurrency = useCallback(
    async <T,>(
      items: T[],
      worker: (item: T) => Promise<void>,
      concurrency: number
    ) => {
      if (items.length === 0) return;
      const limit = Math.min(Math.max(concurrency, 1), items.length);
      let index = 0;
      const runners = Array.from({ length: limit }, async () => {
        while (true) {
          const current = index++;
          if (current >= items.length) return;
          await worker(items[current]);
        }
      });
      await Promise.allSettled(runners);
    },
    []
  );

  const loadAccounts = useCallback(async (preserveUsage = false) => {
    try {
      setLoading(true);
      setError(null);
      const accountList = await invoke<AccountInfo[]>("list_accounts");
      
      if (preserveUsage) {
        // Preserve existing usage data when just updating account info
        setAccounts((prev) => {
          const usageMap = new Map(
            prev.map((a) => [a.id, { usage: a.usage, usageLoading: a.usageLoading }])
          );
          return accountList.map((a) => ({
            ...a,
            usage: usageMap.get(a.id)?.usage,
            usageLoading: usageMap.get(a.id)?.usageLoading,
          }));
        });
      } else {
        setAccounts(accountList.map((a) => ({ ...a, usageLoading: false })));
      }
      return accountList;
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
      return [];
    } finally {
      setLoading(false);
    }
  }, []);

  const syncLiveAuth = useCallback(
    async (reloadOnChange = true) => {
      try {
        const result = await invoke<LiveAuthSyncResult>("sync_live_auth");
        if (reloadOnChange && result.changed) {
          return await loadAccounts(true);
        }
        return null;
      } catch (err) {
        console.error("Failed to sync live auth:", err);
        return null;
      }
    },
    [loadAccounts]
  );

  const refreshUsage = useCallback(
    async (accountList?: AccountInfo[] | AccountWithUsage[]) => {
    try {
      const list = accountList ?? accountsRef.current;
      if (list.length === 0) {
        return;
      }

      const accountIds = list.map((account) => account.id);
      const accountIdSet = new Set(accountIds);

      setAccounts((prev) =>
        prev.map((account) =>
          accountIdSet.has(account.id)
            ? { ...account, usageLoading: true }
            : account
        )
      );

      await runWithConcurrency(
        accountIds,
        async (accountId) => {
          try {
            const usage = await invoke<UsageInfo>("get_usage", { accountId });
            setAccounts((prev) =>
              prev.map((account) =>
                account.id === accountId
                  ? { ...account, usage, usageLoading: false }
                  : account
              )
            );
          } catch (err) {
            console.error("Failed to refresh usage:", err);
            const message = err instanceof Error ? err.message : String(err);
            setAccounts((prev) =>
              prev.map((account) =>
                account.id === accountId
                  ? {
                      ...account,
                      usage: buildUsageError(accountId, message, account.plan_type ?? null),
                      usageLoading: false,
                    }
                  : account
              )
            );
          }
        },
        maxConcurrentUsageRequests
      );
    } catch (err) {
      console.error("Failed to refresh usage:", err);
      throw err;
    }
    },
    [buildUsageError, maxConcurrentUsageRequests, runWithConcurrency]
  );

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
      const message = err instanceof Error ? err.message : String(err);
      setAccounts((prev) =>
        prev.map((a) =>
          a.id === accountId
            ? {
                ...a,
                usage: buildUsageError(accountId, message, a.plan_type ?? null),
                usageLoading: false,
              }
            : a
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
    async (accountId: string) => {
      try {
        await invoke("switch_account", { accountId });
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
        const accountList = await loadAccounts();
        await refreshUsage(accountList);
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
      const accountList = await loadAccounts();
      await refreshUsage(accountList);
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
        const accountList = await loadAccounts();
        await refreshUsage(accountList);
        return summary;
      } catch (err) {
        throw err;
      }
    },
    [loadAccounts, refreshUsage]
  );

  const exportAccountsFullEncryptedFile = useCallback(
    async (path: string) => {
      try {
        await invoke("export_accounts_full_encrypted_file", { path });
      } catch (err) {
        throw err;
      }
    },
    []
  );

  const importAccountsFullEncryptedFile = useCallback(
    async (path: string) => {
      try {
        const summary = await invoke<ImportAccountsSummary>(
          "import_accounts_full_encrypted_file",
          { path }
        );
        const accountList = await loadAccounts();
        await refreshUsage(accountList);
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

  const loadMaskedAccountIds = useCallback(async () => {
    try {
      return await invoke<string[]>("get_masked_account_ids");
    } catch (err) {
      console.error("Failed to load masked account IDs:", err);
      return [];
    }
  }, []);

  const saveMaskedAccountIds = useCallback(async (ids: string[]) => {
    try {
      await invoke("set_masked_account_ids", { ids });
    } catch (err) {
      console.error("Failed to save masked account IDs:", err);
    }
  }, []);

  useEffect(() => {
    let cancelled = false;

    const initialize = async () => {
      await syncLiveAuth(false);
      if (cancelled) return;

      const accountList = await loadAccounts();
      if (cancelled) return;

      await refreshUsage(accountList);
    };

    void initialize();

    const syncInterval = setInterval(() => {
      syncLiveAuth(true).catch(() => {});
    }, 5000);

    const usageInterval = setInterval(() => {
      refreshUsage().catch(() => {});
    }, 60000);

    const handleWindowFocus = () => {
      syncLiveAuth(true).catch(() => {});
    };

    window.addEventListener("focus", handleWindowFocus);

    return () => {
      cancelled = true;
      clearInterval(syncInterval);
      clearInterval(usageInterval);
      window.removeEventListener("focus", handleWindowFocus);
    };
  }, [loadAccounts, refreshUsage, syncLiveAuth]);

  return {
    accounts,
    loading,
    error,
    loadAccounts,
    syncLiveAuth,
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
  };
}
