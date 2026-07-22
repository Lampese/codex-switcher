import i18n from "i18next";
import { initReactI18next } from "react-i18next";
import manifest from "../../locales/manifest.json";
import { invokeBackend, isTauriRuntime } from "../lib/platform";

export type AppLanguage = string;
export const SYSTEM_LANGUAGE = "system";

export interface AppLanguageState {
  preference: AppLanguage;
  resolvedCode: AppLanguage;
}

export const supportedLanguages = manifest as Array<{
  code: AppLanguage;
  label: string;
}>;

const localeModules = import.meta.glob("../../locales/*/ui.json", {
  eager: true,
  import: "default",
}) as Record<string, object>;

const resources = Object.fromEntries(
  supportedLanguages.map(({ code }) => {
    const path = `../../locales/${code}/ui.json`;
    const translation = localeModules[path];
    if (!translation) throw new Error(`Missing UI translations for ${code}`);
    return [code, { translation }];
  })
);

const i18nReady = i18n.use(initReactI18next).init({
  resources,
  lng: "en-US",
  fallbackLng: "en-US",
  initAsync: false,
  interpolation: { escapeValue: false },
  returnNull: false,
});

let languagePreference: AppLanguage = SYSTEM_LANGUAGE;
const preferenceListeners = new Set<(language: AppLanguage) => void>();

function applyDocumentLanguage(language: string): void {
  document.documentElement.lang = language;
}

async function applyLanguage(state: AppLanguageState): Promise<void> {
  await i18nReady;
  const language = supportedLanguages.some(({ code }) => code === state.resolvedCode)
    ? state.resolvedCode
    : "en-US";
  languagePreference = state.preference;
  await i18n.changeLanguage(language);
  applyDocumentLanguage(language);
  preferenceListeners.forEach((listener) => listener(state.preference));
}

export function getLanguagePreference(): AppLanguage {
  return languagePreference;
}

export function subscribeLanguagePreference(
  listener: (language: AppLanguage) => void
): () => void {
  preferenceListeners.add(listener);
  return () => preferenceListeners.delete(listener);
}

export async function initializeI18n(): Promise<void> {
  try {
    await i18nReady;
    const state = await invokeBackend<AppLanguageState>("get_app_language");
    await applyLanguage(state);
  } catch (error) {
    console.error("Failed to load app language:", error);
    applyDocumentLanguage("en-US");
  }

  if (isTauriRuntime()) {
    try {
      const { listen } = await import("@tauri-apps/api/event");
      await listen<AppLanguageState>("language-changed", ({ payload }) => {
        void applyLanguage(payload).catch((error) => {
          console.error("Failed to apply app language change:", error);
        });
      });
    } catch (error) {
      console.error("Failed to listen for app language changes:", error);
    }
  }
}

export async function changeAppLanguage(language: AppLanguage): Promise<void> {
  const state = await invokeBackend<AppLanguageState>("set_app_language", { language });
  await applyLanguage(state);
}

export default i18n;
