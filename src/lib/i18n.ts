import {
  createContext,
  createElement,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react";
import i18next from "i18next";
import enUS from "../locales/en-US.json" with { type: "json" };
import zhCN from "../locales/zh-CN.json" with { type: "json" };

export type SupportedLanguage = "en-US" | "zh-CN";
export type BrowserLanguagePreference = "browser" | SupportedLanguage;

const LANGUAGE_STORAGE_KEY = "codex-switcher-language";

const i18n = i18next.createInstance({
  resources: {
    "en-US": { translation: enUS },
    "zh-CN": { translation: zhCN },
  },
  fallbackLng: "en-US",
  interpolation: { escapeValue: false },
});
void i18n.init();

type LanguageContextValue = {
  language: SupportedLanguage;
  browserPreference: BrowserLanguagePreference;
  setBrowserPreference: (preference: BrowserLanguagePreference) => void;
  reconcileDesktopLanguage: (language: SupportedLanguage) => void;
  t: (key: string, variables?: Record<string, string | number>) => string;
};

const LanguageContext = createContext<LanguageContextValue | null>(null);

export function resolveSupportedLocale(locale?: string): SupportedLanguage | null {
  if (!locale) return null;
  const normalized = locale.replace(/_/g, "-").toLowerCase();
  const subtags = normalized.split("-");

  if (subtags[0] === "zh") {
    const script = subtags.slice(1).find((subtag) => subtag.length === 4);
    const region = subtags.slice(1).find(
      (subtag) => /^[a-z]{2}$/.test(subtag) || /^\d{3}$/.test(subtag)
    );

    if (
      script === "hant" ||
      region === "tw" ||
      region === "hk" ||
      region === "mo"
    ) {
      return null;
    }

    if (script === "hans" || region === "cn" || region === "sg") {
      return "zh-CN";
    }

    return null;
  }

  if (subtags[0] === "en") {
    return "en-US";
  }

  return null;
}

export function resolvePreferredLanguage(locales: readonly string[]): SupportedLanguage {
  for (const locale of locales) {
    const resolved = resolveSupportedLocale(locale);
    if (resolved) return resolved;
  }
  return "en-US";
}

export function resolveInitialLanguage(locale?: string): SupportedLanguage {
  return resolveSupportedLocale(locale) ?? "en-US";
}

function isTauriRuntime(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

function browserLocales(): string[] {
  if (typeof navigator === "undefined") return [];
  return navigator.languages?.length ? Array.from(navigator.languages) : [navigator.language];
}

function readBrowserPreference(): BrowserLanguagePreference {
  try {
    const stored = window.localStorage.getItem(LANGUAGE_STORAGE_KEY);
    if (stored === "browser" || stored === "en-US" || stored === "zh-CN") {
      return stored;
    }
  } catch {
    // Fall through to browser default.
  }
  return "browser";
}

export function resolveBrowserLanguagePreference(
  preference: BrowserLanguagePreference,
  locales: readonly string[],
): SupportedLanguage {
  return preference === "browser" ? resolvePreferredLanguage(locales) : preference;
}

/** Resolve prompt copy from the current browser UI projection first. */
export function resolveBrowserPresentationLanguage(
  documentLocale: string | undefined,
  locales: readonly string[],
): SupportedLanguage {
  return resolveSupportedLocale(documentLocale) ?? resolvePreferredLanguage(locales);
}

function resolveBrowserPreference(preference: BrowserLanguagePreference): SupportedLanguage {
  return resolveBrowserLanguagePreference(preference, browserLocales());
}

export function translate(key: string, language: SupportedLanguage = "en-US"): string {
  return i18n.t(key, { lng: language, defaultValue: key });
}

export function translateMessage(
  key: string,
  variables: Record<string, string | number> = {},
  language: SupportedLanguage = "en-US"
): string {
  return i18n.t(key, { lng: language, defaultValue: key, ...variables });
}

export function I18nProvider({ children }: { children: ReactNode }) {
  const desktop = isTauriRuntime();
  const [browserPreference, setBrowserPreferenceState] =
    useState<BrowserLanguagePreference>(() => (desktop ? "browser" : readBrowserPreference()));
  const [language, setLanguage] = useState<SupportedLanguage>(() =>
    desktop ? "en-US" : resolveBrowserPreference(readBrowserPreference())
  );

  useEffect(() => {
    if (typeof document !== "undefined") {
      document.documentElement.lang = language;
    }
  }, [language]);

  useEffect(() => {
    if (!desktop) return;

    let disposed = false;
    let stop: (() => void) | undefined;

    const loadDesktopLanguage = () =>
      import("@tauri-apps/api/core")
        .then(({ invoke }) => invoke<{ resolved_language?: string }>("get_display_settings"))
        .then((settings) => {
          if (!disposed) setLanguage(resolveInitialLanguage(settings.resolved_language));
        })
        .catch(() => undefined);

    void loadDesktopLanguage();
    void import("@tauri-apps/api/event")
      .then(async ({ listen }) => {
        stop = await listen("app-settings-changed", () => {
          void loadDesktopLanguage();
        });
      })
      .catch(() => undefined);

    return () => {
      disposed = true;
      stop?.();
    };
  }, [desktop]);

  const setBrowserPreference = useCallback(
    (next: BrowserLanguagePreference) => {
      if (desktop) return;
      setBrowserPreferenceState(next);
      setLanguage(resolveBrowserPreference(next));
      try {
        window.localStorage.setItem(LANGUAGE_STORAGE_KEY, next);
      } catch {
        // Keep the in-memory browser preference when storage is unavailable.
      }
    },
    [desktop]
  );

  const reconcileDesktopLanguage = useCallback(
    (next: SupportedLanguage) => {
      if (desktop) setLanguage(next);
    },
    [desktop]
  );

  const value = useMemo(
    () => ({
      language,
      browserPreference,
      setBrowserPreference,
      reconcileDesktopLanguage,
      t: (key: string, variables: Record<string, string | number> = {}) =>
        translateMessage(key, variables, language),
    }),
    [browserPreference, language, reconcileDesktopLanguage, setBrowserPreference]
  );

  return createElement(LanguageContext.Provider, { value }, children);
}

export function useI18n(): LanguageContextValue {
  const context = useContext(LanguageContext);
  if (!context) throw new Error("useI18n must be used inside I18nProvider");
  return context;
}
