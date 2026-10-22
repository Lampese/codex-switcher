export const THEME_CHANGED_EVENT = "theme-changed";
export const THEME_STORAGE_KEY = "codex-switcher-theme";

export const THEMES = [
  { id: "macos", name: "macOS", description: "Quiet and familiar", dark: false },
  { id: "windows-xp", name: "Windows XP", description: "Classic blue and silver", dark: false },
  { id: "windows-7", name: "Windows 7", description: "Soft glass and sky blue", dark: false },
  { id: "terminal", name: "Terminal", description: "Phosphor green on black", dark: true },
  { id: "paper", name: "Paper", description: "Warm paper and ink", dark: false },
  { id: "nord", name: "Nord", description: "Cool and comfortable", dark: true },
  { id: "mono", name: "Mono", description: "Black, white and nothing extra", dark: false },
] as const;
export type ThemeId = (typeof THEMES)[number]["id"];
// Keep old event payloads compatible with tray windows from a previous version.
export type ThemeAppearance = "light" | "dark";
export type ThemeMode = ThemeId | ThemeAppearance | `${ThemeId}:${ThemeAppearance}`;

export function normalizeTheme(value: unknown): ThemeId {
  if (value === "dark") return "nord";
  if (value === "light") return "macos";
  const id = typeof value === "string" ? value.split(":")[0] : value;
  return THEMES.find((theme) => theme.id === id)?.id ?? "macos";
}

export function getThemeAppearance(value: ThemeMode): ThemeAppearance {
  const explicit = value.split(":")[1];
  if (explicit === "light" || explicit === "dark") return explicit;
  return THEMES.find((theme) => theme.id === normalizeTheme(value))?.dark ? "dark" : "light";
}

export function readStoredTheme(): ThemeMode {
  if (typeof window === "undefined") return "macos";
  try {
    const stored = window.localStorage.getItem(THEME_STORAGE_KEY);
    const theme = normalizeTheme(stored);
    const appearance = stored?.split(":")[1];
    return appearance === "light" || appearance === "dark" ? `${theme}:${appearance}` : theme;
  } catch {
    return "macos";
  }
}

export function applyTheme(value: ThemeMode): void {
  const theme = normalizeTheme(value);
  document.documentElement.dataset.theme = theme;
  const appearance = getThemeAppearance(value);
  document.documentElement.dataset.appearance = appearance;
  document.documentElement.classList.toggle("dark", appearance === "dark");
}

export function syncThemeFromStorage(): void {
  applyTheme(readStoredTheme());
}
