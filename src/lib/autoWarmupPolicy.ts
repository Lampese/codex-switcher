import type { UsageInfo } from "../types";

const DEFAULT_SESSION_WINDOW_MINUTES = 5 * 60;
const DEFAULT_WEEKLY_WINDOW_MINUTES = 7 * 24 * 60;
const FULL_WINDOW_SLACK_MINUTES = 5;
const LIMIT_FULL_THRESHOLD = 99.5;
export const DEFAULT_MIN_SUCCESS_INTERVAL_MS = 60 * 60 * 1000;

export type AutoWarmupWindowKind = "session" | "weekly";

export interface AutoWarmupWindow {
  kind: AutoWarmupWindowKind;
  windowMinutes: number;
  resetsAt: number;
}

export interface AutoWarmupHistory {
  lastSuccessfulWarmupAt?: number;
  lastAutoWindowKey?: string;
  lastAutoWindowKind?: AutoWarmupWindowKind;
}

function isPresent<T>(value: T | null | undefined): value is T {
  return value !== null && value !== undefined;
}

function hasPrimaryWindow(usage: UsageInfo): boolean {
  return (
    isPresent(usage.primary_used_percent) ||
    isPresent(usage.primary_window_minutes) ||
    isPresent(usage.primary_resets_at)
  );
}

function hasSecondaryWindow(usage: UsageInfo): boolean {
  return (
    isPresent(usage.secondary_used_percent) ||
    isPresent(usage.secondary_window_minutes) ||
    isPresent(usage.secondary_resets_at)
  );
}

export function getAutoWarmupWindowKind(
  usage: UsageInfo | undefined
): AutoWarmupWindowKind | null {
  if (!usage || usage.error) return null;
  if (hasPrimaryWindow(usage)) return "session";
  if (hasSecondaryWindow(usage)) return "weekly";
  return null;
}

export function getAutoWarmupWindow(
  usage: UsageInfo | undefined
): AutoWarmupWindow | null {
  const kind = getAutoWarmupWindowKind(usage);
  if (!usage || !kind) return null;

  if (kind === "session") {
    if (!isPresent(usage.primary_resets_at)) return null;
    return {
      kind,
      windowMinutes: usage.primary_window_minutes ?? DEFAULT_SESSION_WINDOW_MINUTES,
      resetsAt: usage.primary_resets_at,
    };
  }

  if (!isPresent(usage.secondary_resets_at)) return null;
  return {
    kind,
    windowMinutes: usage.secondary_window_minutes ?? DEFAULT_WEEKLY_WINDOW_MINUTES,
    resetsAt: usage.secondary_resets_at,
  };
}

export function getAutoWarmupWindowKey(window: AutoWarmupWindow): string {
  return `${window.kind}:${window.windowMinutes}:${window.resetsAt}`;
}

export function isAutoWarmupWindowFresh(
  window: AutoWarmupWindow,
  nowMs = Date.now()
): boolean {
  if (!Number.isFinite(window.windowMinutes) || window.windowMinutes <= 0) return false;
  if (!Number.isFinite(window.resetsAt) || window.resetsAt <= 0) return false;

  const remainingMs = window.resetsAt * 1000 - nowMs;
  const thresholdMinutes = Math.max(0, window.windowMinutes - FULL_WINDOW_SLACK_MINUTES);
  return remainingMs >= thresholdMinutes * 60 * 1000;
}

export function getDueAutoWarmupWindow(
  usage: UsageInfo | undefined,
  history: AutoWarmupHistory | undefined,
  nowMs = Date.now(),
  minSuccessIntervalMs = DEFAULT_MIN_SUCCESS_INTERVAL_MS
): AutoWarmupWindow | null {
  const window = getAutoWarmupWindow(usage);
  if (!window) return null;
  const weeklyUsedPercent = usage?.secondary_used_percent;
  if (
    window.kind === "session" &&
    isPresent(weeklyUsedPercent) &&
    weeklyUsedPercent >= LIMIT_FULL_THRESHOLD
  ) {
    return null;
  }
  if (!isAutoWarmupWindowFresh(window, nowMs)) return null;

  const windowKey = getAutoWarmupWindowKey(window);
  if (history?.lastAutoWindowKey === windowKey) return null;

  const lastSuccessfulWarmupAt = history?.lastSuccessfulWarmupAt;
  if (
    lastSuccessfulWarmupAt &&
    nowMs - lastSuccessfulWarmupAt < minSuccessIntervalMs &&
    (!history.lastAutoWindowKind || history.lastAutoWindowKind === window.kind)
  ) {
    return null;
  }

  return window;
}
