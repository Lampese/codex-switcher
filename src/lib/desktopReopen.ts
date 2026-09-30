export type DesktopReopenPreference = "ask" | "always" | "never";

export const DESKTOP_REOPEN_STORAGE_KEY = "desktop-reopen-after-force-close";

export function parseDesktopReopenPreference(value: string | null): DesktopReopenPreference {
  return value === "always" || value === "never" ? value : "ask";
}

export interface ForceCloseOutcome {
  canSwitch: boolean;
  reopenToken: string | null;
}

// Account switching must finish before reopening a desktop that reads auth.json.
export async function finishForceClose(
  outcome: ForceCloseOutcome,
  switchAccount: (() => Promise<void>) | null,
  reopenDesktop: (token: string) => Promise<void>,
): Promise<void> {
  if (!outcome.canSwitch) return;
  if (switchAccount) await switchAccount();
  if (outcome.reopenToken) await reopenDesktop(outcome.reopenToken);
}
