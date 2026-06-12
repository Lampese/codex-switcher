export const AUTO_WARMUP_ALL_STORAGE_KEY = "codex-switcher-auto-warmup-all";
export const AUTO_WARMUP_ACCOUNTS_STORAGE_KEY = "codex-switcher-auto-warmup-accounts";
export const AUTO_WARMUP_LEDGER_STORAGE_KEY = "codex-switcher-auto-warmup-last-success";
export const AUTO_WARMUP_ALL_CHANGED_EVENT = "auto-warmup-all-changed";

export function readAutoWarmupAllEnabled(): boolean {
  if (typeof window === "undefined") return false;
  try {
    return window.localStorage.getItem(AUTO_WARMUP_ALL_STORAGE_KEY) === "true";
  } catch {
    return false;
  }
}

export function writeAutoWarmupAllEnabled(enabled: boolean): void {
  if (typeof window === "undefined") return;
  window.localStorage.setItem(AUTO_WARMUP_ALL_STORAGE_KEY, String(enabled));
}
