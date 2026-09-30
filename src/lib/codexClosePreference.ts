export type CodexClosePreference = "ask" | "graceful" | "force";

export const CODEX_CLOSE_PREFERENCE_STORAGE_KEY = "codex-close-preference";

export function parseCodexClosePreference(value: string | null): CodexClosePreference {
  return value === "graceful" || value === "force" ? value : "ask";
}

export function rememberedCodexClosePreference(
  forceClose: boolean,
  remember: boolean,
): CodexClosePreference {
  if (!remember) return "ask";
  return forceClose ? "force" : "graceful";
}
