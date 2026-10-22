export type Language = "en" | "tr";
export const LANGUAGE_STORAGE_KEY = "codex-switcher-language";
export const LANGUAGE_CHANGED_EVENT = "language-changed";
// English is the default; only an explicit "tr" selects Turkish.
export function normalizeLanguage(value: unknown): Language {
  return value === "tr" ? "tr" : "en";
}
let sessionLanguage: Language | undefined;
export function rememberLanguage(value: Language) { sessionLanguage = value; }
export function getLanguage(): Language {
  if (sessionLanguage) return sessionLanguage;
  try { return normalizeLanguage(localStorage.getItem(LANGUAGE_STORAGE_KEY)); }
  catch { return typeof document === "undefined" ? "en" : normalizeLanguage(document.documentElement.lang); }
}
export function getLocale(): "tr-TR" | "en-US" {
  return getLanguage() === "tr" ? "tr-TR" : "en-US";
}
export function translate(source: string, language: Language, dictionary: Record<string, string>, params: Record<string, string | number> = {}): string {
  const text = language === "tr" && Object.prototype.hasOwnProperty.call(dictionary, source) ? dictionary[source] : source;
  return text.replace(/\{(\w+)\}/g, (match, key: string) => Object.prototype.hasOwnProperty.call(params, key) ? String(params[key]) : match);
}
