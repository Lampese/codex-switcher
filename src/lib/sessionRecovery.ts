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
