import i18n from "i18next";
import { initReactI18next } from "react-i18next";
import manifest from "../../locales/manifest.json";
import { invokeBackend, isTauriRuntime } from "../lib/platform";

export type AppLanguage = string;
export const SYSTEM_LANGUAGE = "system";

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
  interpolation: { escapeValue: false },
  returnNull: false,
});

let languagePreference: AppLanguage = SYSTEM_LANGUAGE;
const preferenceListeners = new Set<(language: AppLanguage) => void>();

function matchSupportedLanguage(locale: string): AppLanguage | null {
  const normalized = locale.replace(/_/g, "-").toLowerCase();
  const exact = supportedLanguages.find(({ code }) => code.toLowerCase() === normalized);
  if (exact) return exact.code;

  if (
    normalized.startsWith("zh-") &&
    !normalized.startsWith("zh-cn") &&
    !normalized.startsWith("zh-sg") &&
    !normalized.startsWith("zh-hans")
  ) {
    return null;
  }

  const primary = normalized.split("-")[0];
  return supportedLanguages.find(({ code }) => code.split("-")[0].toLowerCase() === primary)?.code ?? null;
}

function resolveLanguage(preference: AppLanguage): AppLanguage {
  if (preference !== SYSTEM_LANGUAGE) {
    return supportedLanguages.some(({ code }) => code === preference) ? preference : "en-US";
  }

  const browserLanguages = typeof navigator === "undefined"
    ? []
    : navigator.languages?.length
      ? navigator.languages
      : [navigator.language];
  for (const locale of browserLanguages) {
    const matched = matchSupportedLanguage(locale);
    if (matched) return matched;
  }
  return "en-US";
}

function applyDocumentLanguage(language: string): void {
  document.documentElement.lang = language;
}

async function applyLanguage(preference: AppLanguage): Promise<void> {
  await i18nReady;
  const language = resolveLanguage(preference);
  languagePreference = preference;
  await i18n.changeLanguage(language);
  applyDocumentLanguage(language);
  preferenceListeners.forEach((listener) => listener(preference));
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
  await i18nReady;
  try {
    const language = await invokeBackend<AppLanguage>("get_app_language");
    await applyLanguage(language);
  } catch (error) {
    console.error("Failed to load app language:", error);
    applyDocumentLanguage("en-US");
  }

  if (isTauriRuntime()) {
    try {
      const { listen } = await import("@tauri-apps/api/event");
      await listen<AppLanguage>("language-changed", ({ payload }) => {
        void applyLanguage(payload);
      });
    } catch (error) {
      console.error("Failed to listen for app language changes:", error);
    }
  }
}

export async function changeAppLanguage(language: AppLanguage): Promise<void> {
  const saved = await invokeBackend<AppLanguage>("set_app_language", { language });
  await applyLanguage(saved);
}

export default i18n;
