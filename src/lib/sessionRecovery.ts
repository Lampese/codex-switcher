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
