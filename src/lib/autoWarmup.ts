export const AUTO_WARMUP_ALL_STORAGE_KEY = "codex-switcher-auto-warmup-all";
export const AUTO_WARMUP_ACCOUNTS_STORAGE_KEY = "codex-switcher-auto-warmup-accounts";
export const AUTO_WARMUP_LEDGER_STORAGE_KEY = "codex-switcher-auto-warmup-last-success";
export const AUTO_WARMUP_ALL_CHANGED_EVENT = "auto-warmup-all-changed";

// Usage refresh interval (ms between automatic usage re-fetches).
export const USAGE_REFRESH_INTERVAL_STORAGE_KEY = "codex-switcher-usage-refresh-interval-ms";
export const USAGE_REFRESH_INTERVAL_DEFAULT_MS = 60_000; // 1 min

// Preset options shown in the UI (label → ms).
export const USAGE_REFRESH_INTERVAL_PRESETS: { label: string; ms: number }[] = [
  { label: "30 s", ms: 30_000 },
  { label: "1 min", ms: 60_000 },
  { label: "5 min", ms: 5 * 60_000 },
  { label: "10 min", ms: 10 * 60_000 },
];

export function readUsageRefreshIntervalMs(): number {
  if (typeof window === "undefined") return USAGE_REFRESH_INTERVAL_DEFAULT_MS;
  try {
    const raw = window.localStorage.getItem(USAGE_REFRESH_INTERVAL_STORAGE_KEY);
    if (!raw) return USAGE_REFRESH_INTERVAL_DEFAULT_MS;
    const parsed = Number(raw);
    // Clamp to sane range: 10 s – 60 min
    if (!Number.isFinite(parsed) || parsed < 10_000 || parsed > 60 * 60_000) {
      return USAGE_REFRESH_INTERVAL_DEFAULT_MS;
    }
    return parsed;
  } catch {
    return USAGE_REFRESH_INTERVAL_DEFAULT_MS;
  }
}

export function writeUsageRefreshIntervalMs(ms: number): void {
  if (typeof window === "undefined") return;
  window.localStorage.setItem(USAGE_REFRESH_INTERVAL_STORAGE_KEY, String(ms));
}

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
