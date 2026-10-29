import type { AccountInfo, CodexProcessInfo, UsageInfo } from "../types";

export const AUTO_SWITCH_STORAGE_KEY = "auto-switch-exhausted-codex-account";
export const AUTO_SWITCH_INTERVAL_MS = 60_000;
export const AUTO_SWITCH_COOLDOWN_MS = 5 * 60_000;

export function readAutoSwitchEnabled(): boolean {
  try { return window.localStorage.getItem(AUTO_SWITCH_STORAGE_KEY) === "true"; }
  catch { return false; }
}

export function quotaState(usage: UsageInfo): "available" | "exhausted" | "unknown" {
  if (usage.error) return "unknown";
  // Credits can still fund requests after an included usage window is exhausted.
  if (usage.unlimited_credits === true || usage.has_credits === true) return "available";
  const windows = [
    [usage.primary_used_percent, usage.primary_window_minutes, usage.primary_resets_at],
    [usage.secondary_used_percent, usage.secondary_window_minutes, usage.secondary_resets_at],
  ].filter((window) => window.some((value) => value != null));
  if (windows.some(([used]) => used != null && Number.isFinite(used) && used >= 100)) return "exhausted";
  if (!windows.length || windows.some(([used]) => used == null || !Number.isFinite(used) || used < 0)) return "unknown";
  return "available";
}

interface AutoSwitchDependencies {
  invoke: <T>(command: string, args?: Record<string, unknown>) => Promise<T>;
  cancelled: () => boolean;
  switchAccount: (target: AccountInfo, sourceId: string, reopen: boolean) => Promise<void>;
}

export async function checkQuotaAutoSwitch({ invoke, cancelled, switchAccount }: AutoSwitchDependencies): Promise<{ message: string; switched: boolean }> {
  const result = (message: string, switched = false) => ({ message, switched });
  const active = await invoke<AccountInfo | null>("get_active_account_info");
  if (!active || active.auth_mode === "api_key") return result("Waiting for an active ChatGPT account.");
  const usage = await invoke<UsageInfo>("get_usage", { accountId: active.id });
  const state = quotaState(usage);
  if (state !== "exhausted") return result(state === "available" ? "Monitoring quota. The active account still has capacity." : "Could not confirm the active account’s quota. No switch made.");
  const accounts = await invoke<AccountInfo[]>("list_accounts");
  const index = accounts.findIndex((account) => account.id === active.id);
  if (index < 0) return result("The active account changed. Checking again next minute.");
  const candidates = [...accounts.slice(index + 1), ...accounts.slice(0, index)];
  for (const candidate of candidates) {
    if (cancelled()) return result("Auto-switch check cancelled.");
    if (candidate.auth_mode === "api_key") continue;
    let candidateUsage: UsageInfo;
    try { candidateUsage = await invoke<UsageInfo>("get_usage", { accountId: candidate.id }); }
    catch { continue; }
    if (quotaState(candidateUsage) !== "available") continue;
    // Recheck the source after candidate requests, which can take several seconds.
    const current = await invoke<AccountInfo | null>("get_active_account_info");
    if (current?.id !== active.id) return result("The active account changed. No automatic switch made.");
    if (quotaState(await invoke<UsageInfo>("get_usage", { accountId: active.id })) !== "exhausted") return result("The active account’s quota changed. No switch needed.");
    const processes = await invoke<CodexProcessInfo>("check_codex_processes");
    if (!processes.can_switch) {
      const desktop = await invoke<{ supported: boolean; desktop_count: number }>("get_codex_reopen_info");
      if (!desktop.supported || desktop.desktop_count < 1) return result("Auto-switch paused: running Codex processes cannot be reopened automatically.");
    }
    if (cancelled()) return result("Auto-switch check cancelled.");
    await switchAccount(candidate, active.id, !processes.can_switch);
    return result("Account switched automatically. Monitoring resumes in five minutes.", true);
  }
  return result("No other account has confirmed available quota. Checking again next minute.");
}
