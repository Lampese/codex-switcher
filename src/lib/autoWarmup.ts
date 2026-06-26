export const AUTO_WARMUP_ALL_STORAGE_KEY = "codex-switcher-auto-warmup-all";
export const AUTO_WARMUP_ACCOUNTS_STORAGE_KEY = "codex-switcher-auto-warmup-accounts";
export const AUTO_WARMUP_LEDGER_STORAGE_KEY = "codex-switcher-auto-warmup-last-success";
export const AUTO_WARMUP_ALL_CHANGED_EVENT = "auto-warmup-all-changed";

export const TIMED_WARMUP_ENABLED_STORAGE_KEY = "codex-switcher-timed-warmup-enabled";
export const TIMED_WARMUP_TIMES_STORAGE_KEY = "codex-switcher-timed-warmup-times";
export const TIMED_WARMUP_LEDGER_STORAGE_KEY = "codex-switcher-timed-warmup-last-fire";

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

/** Validate, normalize, dedupe and sort a list of "HH:MM" times. */
export function normalizeTimedWarmupTimes(times: readonly string[]): string[] {
  const valid = new Set<string>();
  for (const raw of times) {
    const match = /^(\d{1,2}):(\d{1,2})$/.exec(String(raw).trim());
    if (!match) continue;
    const hours = Number(match[1]);
    const minutes = Number(match[2]);
    if (hours < 0 || hours > 23 || minutes < 0 || minutes > 59) continue;
    valid.add(
      `${String(hours).padStart(2, "0")}:${String(minutes).padStart(2, "0")}`
    );
  }
  return Array.from(valid).sort();
}

export function readTimedWarmupEnabled(): boolean {
  if (typeof window === "undefined") return false;
  try {
    return (
      window.localStorage.getItem(TIMED_WARMUP_ENABLED_STORAGE_KEY) === "true"
    );
  } catch {
    return false;
  }
}

export function writeTimedWarmupEnabled(enabled: boolean): void {
  if (typeof window === "undefined") return;
  window.localStorage.setItem(TIMED_WARMUP_ENABLED_STORAGE_KEY, String(enabled));
}

export function readTimedWarmupTimes(): string[] {
  if (typeof window === "undefined") return [];
  try {
    const parsed = JSON.parse(
      window.localStorage.getItem(TIMED_WARMUP_TIMES_STORAGE_KEY) ?? "[]"
    );
    return Array.isArray(parsed) ? normalizeTimedWarmupTimes(parsed) : [];
  } catch {
    return [];
  }
}

export function writeTimedWarmupTimes(times: readonly string[]): void {
  if (typeof window === "undefined") return;
  window.localStorage.setItem(
    TIMED_WARMUP_TIMES_STORAGE_KEY,
    JSON.stringify(normalizeTimedWarmupTimes(times))
  );
}
