export const AUTO_RESUME_SESSIONS_STORAGE_KEY = "auto-resume-interrupted-codex-sessions";

export function readAutoResumeSessions(): boolean {
  try {
    return window.localStorage.getItem(AUTO_RESUME_SESSIONS_STORAGE_KEY) === "true";
  } catch {
    return false;
  }
}

export function writeAutoResumeSessions(enabled: boolean): void {
  window.localStorage.setItem(AUTO_RESUME_SESSIONS_STORAGE_KEY, String(enabled));
}

export interface RecoveryProgress {
  completed: number;
  total: number;
  current_session_id: string | null;
}

export interface RecoveryReport {
  resumed: string[];
  skipped: string[];
  failed: { session_id: string; reason: string }[];
  timed_out: boolean;
}

export type RecoveryState =
  | { status: "running"; progress: RecoveryProgress }
  | { status: "complete"; report: RecoveryReport }
  | { status: "error"; message: string };

export async function finishSessionRecovery(options: {
  preparedCount: number;
  reopened: boolean;
  enabled: boolean;
  hasAccountSwitch: boolean;
  invoke: <T>(command: string, args?: Record<string, unknown>) => Promise<T>;
  onStateChange: (state: RecoveryState) => void;
  formatError: (err: unknown) => string;
  createProgressChannel?: (onMessage: (progress: RecoveryProgress) => void) => Promise<unknown>;
}): Promise<void> {
  const { preparedCount, reopened, enabled, hasAccountSwitch, invoke, onStateChange, formatError } = options;
  if (preparedCount > 0 && reopened) {
    onStateChange({ status: "running", progress: { completed: 0, total: preparedCount, current_session_id: null } });
    let acceptingProgress = true;
    try {
      const makeChannel = options.createProgressChannel ?? (async (onMessage) => {
        const { Channel } = await import("@tauri-apps/api/core");
        return new Channel<RecoveryProgress>(onMessage);
      });
      const onProgress = await makeChannel((progress) => {
        if (acceptingProgress) onStateChange({ status: "running", progress });
      });
      const report = await invoke<RecoveryReport>("resume_codex_session_recovery", { onProgress });
      acceptingProgress = false;
      onStateChange({ status: "complete", report });
    } catch (err) {
      acceptingProgress = false;
      onStateChange({ status: "error", message: `Account switched, but auto-resume failed: ${formatError(err)}` });
    }
  } else if (preparedCount > 0) {
    await invoke("cancel_codex_session_recovery").catch(() => undefined);
  } else if (hasAccountSwitch && enabled && reopened) {
    onStateChange({ status: "complete", report: { resumed: [], skipped: [], failed: [], timed_out: false } });
  }
}
