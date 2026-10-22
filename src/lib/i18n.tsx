import { createContext, useContext, useEffect, useState, type ReactNode } from "react";
import { localizeProductMessage } from "./i18nMessages";
import { shellTranslations } from "./i18nShell";
import { componentTranslations } from "./i18nComponents";
import { getLanguage, LANGUAGE_CHANGED_EVENT, LANGUAGE_STORAGE_KEY, normalizeLanguage, rememberLanguage, translate, type Language } from "./i18nLanguage";
// Kept local so platform.ts can import t without an import cycle.
const isTauriRuntime = () => typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
export { getLanguage, getLocale, LANGUAGE_STORAGE_KEY, type Language } from "./i18nLanguage";
const dictionary = { ...shellTranslations, ...componentTranslations };
export function t(source: string, params?: Record<string, string | number>): string {
  return translate(source, getLanguage(), dictionary, params);
}
export const localizeMessage = (message: string) => localizeProductMessage(message, t);
const LanguageContext = createContext<Language>(getLanguage());
function storeLanguage(language: Language) {
  try { localStorage.setItem(LANGUAGE_STORAGE_KEY, language); } catch { /* Session selection still works without storage. */ }
}
export function setLanguage(value: Language) {
  const language = normalizeLanguage(value);
  rememberLanguage(language);
  storeLanguage(language);
  document.documentElement.lang = language;
  window.dispatchEvent(new CustomEvent(LANGUAGE_CHANGED_EVENT, { detail: language }));
  // The native command persists the choice, refreshes native menus and notifies every webview.
  if (isTauriRuntime()) void import("@tauri-apps/api/core").then(({ invoke }) => invoke("set_ui_language", { language })).catch(console.error);
}
export function I18nProvider({ children }: { children: ReactNode }) {
  const [language, updateLanguage] = useState(getLanguage);
  useEffect(() => {
    const apply = (value: unknown) => {
      const next = normalizeLanguage(value);
      rememberLanguage(next);
      updateLanguage(next);
      document.documentElement.lang = next;
    };
    apply(getLanguage());
    let changedSinceMount = false;
    const onChange = (event: Event) => { changedSinceMount = true; apply((event as CustomEvent).detail); };
    const onStorage = (event: StorageEvent) => { if (event.key === LANGUAGE_STORAGE_KEY) { changedSinceMount = true; apply(event.newValue); } };
    window.addEventListener(LANGUAGE_CHANGED_EVENT, onChange);
    window.addEventListener("storage", onStorage);
    let disposed = false;
    let unlisten: (() => void) | undefined;
    if (isTauriRuntime()) {
      void import("@tauri-apps/api/event").then(async ({ listen }) => {
        const stop = await listen<Language>(LANGUAGE_CHANGED_EVENT, ({ payload }) => {
          changedSinceMount = true;
          storeLanguage(normalizeLanguage(payload));
          apply(payload);
        });
        if (disposed) stop(); else unlisten = stop;
      }).catch(console.error);
      // The native setting is authoritative so the main window, tray popup and menus agree.
      void import("@tauri-apps/api/core").then(async ({ invoke }) => {
        const native = normalizeLanguage(await invoke<Language>("get_ui_language"));
        if (disposed || changedSinceMount) return;
        storeLanguage(native);
        apply(native);
      }).catch(console.error);
    }
    return () => { disposed = true; unlisten?.(); window.removeEventListener(LANGUAGE_CHANGED_EVENT, onChange); window.removeEventListener("storage", onStorage); };
  }, []);
  return <LanguageContext.Provider value={language}>{children}</LanguageContext.Provider>;
}
export function useI18n() {
  const language = useContext(LanguageContext);
  return { language, locale: language === "tr" ? "tr-TR" as const : "en-US" as const, setLanguage, t };
}
