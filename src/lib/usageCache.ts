import type { AccountInfo, AccountWithUsage, CachedUsageInfo, UsageInfo } from "../types";

const BROWSER_USAGE_CACHE_STORAGE_KEY = "codex-switcher.usage-cache.v1";

export function extractCachedUsageEntriesFromAccounts(
  accountList: AccountInfo[]
): CachedUsageInfo[] {
  return accountList
    .filter(
      (account) =>
        !!account.cached_usage &&
        !!account.cached_usage_updated_at &&
        !account.cached_usage.error
    )
    .map((account) => ({
      account_id: account.id,
      usage: account.cached_usage!,
      updated_at: account.cached_usage_updated_at!,
    }));
}

export function mergeCachedUsageEntries(
  ...entryGroups: CachedUsageInfo[][]
): CachedUsageInfo[] {
  return entryGroups.reduce<CachedUsageInfo[]>(
    (merged, entries) =>
      entries.reduce(
        (nextEntries, entry) => upsertCachedUsageEntry(nextEntries, entry),
        merged
      ),
    []
  );
}

export function mergeAccountsWithCachedUsage(
  accountList: AccountInfo[],
  previousAccounts: AccountWithUsage[],
  cachedEntries: CachedUsageInfo[],
  preserveExistingUsage: boolean
): AccountWithUsage[] {
  const previousById = new Map(previousAccounts.map((account) => [account.id, account]));
  const cachedById = new Map(cachedEntries.map((entry) => [entry.account_id, entry]));

  return accountList.map((account) => {
    const previous = previousById.get(account.id);
    const cached = cachedById.get(account.id);
    const shouldReusePreviousUsage =
      preserveExistingUsage && !!previous?.usage && !previous.usage.error;

    return {
      ...account,
      usage: shouldReusePreviousUsage ? previous?.usage : cached?.usage ?? previous?.usage,
      usageLoading: preserveExistingUsage ? previous?.usageLoading ?? false : false,
      usageUpdatedAt: preserveExistingUsage
        ? previous?.usageUpdatedAt ?? cached?.updated_at ?? null
        : cached?.updated_at ?? null,
    };
  });
}

export function filterCachedUsageEntries(
  entries: CachedUsageInfo[],
  accountIds: ReadonlySet<string>
): CachedUsageInfo[] {
  return entries.filter((entry) => accountIds.has(entry.account_id));
}

export function markAccountsUsageLoading(
  accounts: AccountWithUsage[],
  accountIds: ReadonlySet<string>
): AccountWithUsage[] {
  return accounts.map((account) =>
    accountIds.has(account.id) ? { ...account, usageLoading: true } : account
  );
}

export function applyUsageFetchResult(
  accounts: AccountWithUsage[],
  accountId: string,
  usage: UsageInfo,
  updatedAt: string
): AccountWithUsage[] {
  return accounts.map((account) =>
    account.id === accountId
      ? {
          ...account,
          usage,
          usageLoading: false,
          usageUpdatedAt: usage.error ? account.usageUpdatedAt ?? null : updatedAt,
        }
      : account
  );
}

export function applyUsageFetchError(
  accounts: AccountWithUsage[],
  accountId: string,
  usage: UsageInfo
): AccountWithUsage[] {
  return accounts.map((account) =>
    account.id === accountId
      ? {
          ...account,
          usage: account.usage && !account.usage.error ? account.usage : usage,
          usageLoading: false,
        }
      : account
  );
}

export function loadCachedUsageFromBrowser(): CachedUsageInfo[] {
  if (typeof window === "undefined") {
    return [];
  }

  try {
    const raw = window.localStorage.getItem(BROWSER_USAGE_CACHE_STORAGE_KEY);
    if (!raw) {
      return [];
    }

    const parsed = JSON.parse(raw) as { entries?: unknown };
    if (!Array.isArray(parsed.entries)) {
      return [];
    }

    return parsed.entries.filter(isCachedUsageInfo);
  } catch (error) {
    console.error("Failed to read browser usage cache:", error);
    return [];
  }
}

export function persistCachedUsageToBrowser(entries: CachedUsageInfo[]): void {
  if (typeof window === "undefined") {
    return;
  }

  try {
    window.localStorage.setItem(
      BROWSER_USAGE_CACHE_STORAGE_KEY,
      JSON.stringify({
        version: 1,
        entries,
      })
    );
  } catch (error) {
    console.error("Failed to save browser usage cache:", error);
  }
}

export function saveCachedUsageToBrowser(entry: CachedUsageInfo): void {
  const entries = upsertCachedUsageEntry(loadCachedUsageFromBrowser(), entry);
  persistCachedUsageToBrowser(entries);
}

function upsertCachedUsageEntry(
  entries: CachedUsageInfo[],
  entry: CachedUsageInfo
): CachedUsageInfo[] {
  const merged = new Map(entries.map((current) => [current.account_id, current]));
  const existing = merged.get(entry.account_id);

  if (
    !existing ||
    getCachedUsageTimestamp(entry.updated_at) >= getCachedUsageTimestamp(existing.updated_at)
  ) {
    merged.set(entry.account_id, entry);
  }

  return Array.from(merged.values()).sort((left, right) =>
    left.account_id.localeCompare(right.account_id)
  );
}

function getCachedUsageTimestamp(updatedAt: string): number {
  const timestamp = Date.parse(updatedAt);
  return Number.isNaN(timestamp) ? 0 : timestamp;
}

function isCachedUsageInfo(value: unknown): value is CachedUsageInfo {
  if (!value || typeof value !== "object") {
    return false;
  }

  const entry = value as Partial<CachedUsageInfo>;
  return (
    typeof entry.account_id === "string" &&
    typeof entry.updated_at === "string" &&
    !!entry.usage &&
    typeof entry.usage === "object"
  );
}
