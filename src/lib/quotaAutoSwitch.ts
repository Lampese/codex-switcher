import type { AccountInfo, CodexProcessInfo, UsageInfo } from "../types";
import { finishForceClose } from "./desktopReopen.ts";
import { finishSessionRecovery, type RecoveryProgress, type RecoveryState } from "./sessionRecovery.ts";

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
    let reopen = !processes.can_switch;
    if (reopen) {
      const desktop = await invoke<{ supported: boolean; desktop_count: number; active_count?: number }>("get_codex_reopen_info");
      if (desktop.active_count === 0) {
        reopen = false;
      } else {
        const activeRoots = desktop.active_count ?? processes.count;
        if (
          !desktop.supported ||
          desktop.desktop_count < 1 ||
          typeof activeRoots !== "number" ||
          desktop.desktop_count !== activeRoots
        ) {
          return result("Auto-switch paused: running Codex processes cannot be reopened automatically.");
        }
      }
    }
    if (cancelled()) return result("Auto-switch check cancelled.");
    await switchAccount(candidate, active.id, reopen);
    return result("Account switched automatically. Monitoring resumes in five minutes.", true);
  }
  return result("No other account has confirmed available quota. Checking again next minute.");
}

export async function runAutomaticDesktopSwitch(options: {
  accountId: string;
  sourceId: string;
  forceClose: boolean;
  autoResumeSessions: boolean;
  isAutoSwitchEnabled: () => boolean;
  invoke: <T>(command: string, args?: Record<string, unknown>) => Promise<T>;
  closeProcesses: (
    reopenDesktop: boolean,
    forceClose: boolean,
    requireAllDesktops: boolean,
  ) => Promise<{ processInfo: CodexProcessInfo | null; reopenToken: string | null } | null>;
  switchAccount: (accountId: string, sourceId: string) => Promise<void>;
  onRecoveryStateChange: (state: RecoveryState) => void;
  showToast: (message: string, isError?: boolean) => void;
  formatError: (err: unknown) => string;
  createProgressChannel?: (onMessage: (progress: RecoveryProgress) => void) => Promise<unknown>;
}): Promise<void> {
  const {
    accountId,
    sourceId,
    forceClose,
    autoResumeSessions,
    isAutoSwitchEnabled,
    invoke,
    closeProcesses,
    switchAccount,
    onRecoveryStateChange,
    showToast,
    formatError,
    createProgressChannel,
  } = options;
  let preparedRecoveryCount = 0;
  let reopened = false;
  try {
    const current = await invoke<AccountInfo | null>("get_active_account_info");
    if (current?.id !== sourceId) throw new Error("The active account changed before closing Codex");
    if (!isAutoSwitchEnabled()) throw new Error("Auto-switch was turned off before closing Codex");
    if (autoResumeSessions) {
      preparedRecoveryCount = await invoke<number>("prepare_codex_session_recovery");
    }
    const result = await closeProcesses(true, forceClose, true);
    if (!result?.processInfo?.can_switch) {
      throw new Error("Could not close Codex; no account switch was made");
    }
    if (!result.reopenToken) {
      throw new Error("Codex closed, but no desktop reopen token was captured; no account switch was made");
    }
    await finishForceClose(
      { canSwitch: true, reopenToken: result.reopenToken },
      async () => {
        const activeBeforeSwitch = await invoke<AccountInfo | null>("get_active_account_info");
        if (activeBeforeSwitch?.id !== sourceId) throw new Error("The active account changed before switching");
        await switchAccount(accountId, sourceId);
        showToast(`Switched account after ${forceClose ? "force closing" : "closing"} Codex.`);
      },
      async (token) => {
        try {
          await invoke("reopen_closed_codex_desktop", { token });
          reopened = true;
          showToast("Account switched. Codex desktop reopened.");
        } catch (err) {
          showToast(`Codex closed and account switched, but reopening failed: ${formatError(err)}`, true);
        }
      },
    );
    const recoveryCount = preparedRecoveryCount;
    preparedRecoveryCount = 0;
    await finishSessionRecovery({
      preparedCount: recoveryCount,
      reopened,
      enabled: autoResumeSessions,
      hasAccountSwitch: true,
      invoke,
      onStateChange: onRecoveryStateChange,
      formatError,
      createProgressChannel,
    });
    if (!reopened) throw new Error("Account switched, but Codex did not reopen; open Codex manually");
  } catch (err) {
    if (preparedRecoveryCount > 0) {
      await invoke("cancel_codex_session_recovery").catch(() => undefined);
    }
    showToast(`Switch failed after closing Codex: ${formatError(err)}`, true);
    throw err;
  }
}
