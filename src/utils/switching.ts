import type { CodexProcessInfo } from "../types";

export function hasRunningCodexProcesses(processInfo: CodexProcessInfo | null | undefined) {
  return !!processInfo && (processInfo.count > 0 || processInfo.background_count > 0);
}

export function getSwitchConfirmationMessage(processInfo: CodexProcessInfo) {
  return `Codex is running (${processInfo.count} foreground, ${processInfo.background_count} background). Do you want Codex Switcher to close and reopen it gracefully before switching accounts?`;
}

export function getSwitchErrorMessage(error: unknown) {
  if (typeof error === "string") return error;
  if (error && typeof error === "object" && "message" in error) {
    const message = (error as { message?: unknown }).message;
    if (typeof message === "string") return message;
  }
  return "Unknown error";
}
