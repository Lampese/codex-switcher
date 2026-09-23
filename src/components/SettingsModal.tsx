import { useCallback, useEffect, useRef, useState } from "react";
import type { DesktopReopenPreference } from "../lib/desktopReopen";
import type { CodexClosePreference } from "../lib/codexClosePreference";
import { invokeBackend, isTauriRuntime } from "../lib/platform";
import {
  useI18n,
  type BrowserLanguagePreference,
  type SupportedLanguage,
} from "../lib/i18n";
import type { DockDisplayMode } from "../types";

type TrayDisplayMode = "icon_and_session" | "active_usage_text" | "hidden";
type DesktopLanguagePreference = "system" | SupportedLanguage;

interface DisplaySettings {
  tray_display_mode: TrayDisplayMode;
  dock_display_mode: DockDisplayMode | null;
  ui_language_preference: DesktopLanguagePreference;
  resolved_language: SupportedLanguage;
}

interface SettingsModalProps {
  reopenPreference: DesktopReopenPreference;
  onReopenPreferenceChange: (value: DesktopReopenPreference) => void;
  closePreference: CodexClosePreference;
  onClosePreferenceChange: (value: CodexClosePreference) => void;
  onClose: () => void;
}

export function SettingsModal({
  reopenPreference,
  onReopenPreferenceChange,
  closePreference,
  onClosePreferenceChange,
  onClose,
}: SettingsModalProps) {
  const { browserPreference, setBrowserPreference, reconcileDesktopLanguage, t } = useI18n();
  const [displaySettings, setDisplaySettings] = useState<DisplaySettings | null>(null);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const requestId = useRef(0);
  const desktop = isTauriRuntime();
  const loadDisplaySettings = useCallback(async () => {
    const currentRequest = ++requestId.current;
    try {
      const settings = await invokeBackend<DisplaySettings>("get_display_settings");
      if (currentRequest === requestId.current) {
        setDisplaySettings(settings);
        setError(null);
      }
    } catch (err) {
      if (currentRequest === requestId.current) setError(String(err));
    }
  }, []);

  useEffect(() => {
    if (!desktop) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void import("@tauri-apps/api/event").then(async ({ listen }) => {
      const stop = await listen("app-settings-changed", () => {
        void loadDisplaySettings();
      });
      if (disposed) stop();
      else {
        unlisten = stop;
        void loadDisplaySettings();
      }
    }).catch((err) => {
      if (!disposed) setError(String(err));
    });
    return () => {
      disposed = true;
      requestId.current += 1;
      unlisten?.();
    };
  }, [desktop, loadDisplaySettings]);

  const changeDisplaySetting = async (command: string, mode: string) => {
    setSaving(true);
    setError(null);
    try {
      await invokeBackend(command, { mode });
      // Changing tray visibility can also adjust the Dock mode, and vice versa.
      await loadDisplaySettings();
    } catch (err) {
      requestId.current += 1;
      setError(String(err));
    } finally {
      setSaving(false);
    }
  };

  const changeLanguage = async (
    next: DesktopLanguagePreference | BrowserLanguagePreference
  ) => {
    setError(null);

    if (!desktop) {
      setBrowserPreference(next as BrowserLanguagePreference);
      return;
    }

    setSaving(true);
    try {
      const authoritative = await invokeBackend<DisplaySettings>("set_language", {
        language: next,
      });
      setDisplaySettings(authoritative);
      reconcileDesktopLanguage(authoritative.resolved_language);
    } catch (err) {
      requestId.current += 1;
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setSaving(false);
    }
  };

  const languagePreference = desktop
    ? displaySettings?.ui_language_preference ?? "system"
    : browserPreference;

  const selectClassName = "w-full rounded-lg border border-gray-300 dark:border-gray-700 bg-white dark:bg-gray-800 px-3 py-2 text-sm text-gray-900 dark:text-gray-100 disabled:opacity-50";

  return (
    <div className="fixed inset-0 bg-black/40 flex items-center justify-center z-50">
      <div role="dialog" aria-modal="true" aria-labelledby="settings-title" className="bg-white dark:bg-gray-900 border border-gray-200 dark:border-gray-700 rounded-2xl w-full max-w-md mx-4 shadow-xl">
        <div className="p-5 border-b border-gray-100 dark:border-gray-800">
          <h2 id="settings-title" className="text-lg font-semibold text-gray-900 dark:text-gray-100">
            {t("settings.settings.title")}
          </h2>
        </div>
        <div className="p-5 space-y-3 max-h-[65vh] overflow-y-auto">
          <label htmlFor="language" className="block text-sm font-medium text-gray-900 dark:text-gray-100">
            {t("settings.language.label")}
          </label>
          <select
            id="language"
            value={languagePreference}
            disabled={saving || (desktop && !displaySettings)}
            onChange={(event) =>
              void changeLanguage(
                event.target.value as DesktopLanguagePreference | BrowserLanguagePreference
              )
            }
            className={selectClassName}
          >
            <option value={desktop ? "system" : "browser"}>
              {t(
                desktop
                  ? "settings.language.system_default"
                  : "settings.language.browser_default"
              )}
            </option>
            <option value="en-US">English</option>
            <option value="zh-CN">简体中文</option>
          </select>
          {!desktop && (
            <p className="text-xs text-gray-500 dark:text-gray-400">
              {t("settings.language.browser_hint")}
            </p>
          )}
          {desktop && (
            <>
              {displaySettings ? (
                <>
                  <label htmlFor="tray-display-mode" className="block text-sm font-medium text-gray-900 dark:text-gray-100">{t("settings.tray")}</label>
                  <select
                    id="tray-display-mode"
                    value={displaySettings.tray_display_mode}
                    disabled={saving}
                    onChange={(event) => void changeDisplaySetting("set_tray_display_mode", event.target.value)}
                    className={selectClassName}
                  >
                    <option value="icon_and_session">{t("settings.icon.session")}</option>
                    <option value="active_usage_text">{t("settings.hourly.weekly")}</option>
                    <option value="hidden">{t("settings.hidden")}</option>
                  </select>
                  {displaySettings.dock_display_mode !== null && (
                    <>
                      <label htmlFor="dock-display-mode" className="block text-sm font-medium text-gray-900 dark:text-gray-100">{t("settings.dock.icon")}</label>
                      <select
                        id="dock-display-mode"
                        value={displaySettings.dock_display_mode}
                        disabled={saving}
                        onChange={(event) => void changeDisplaySetting("set_dock_display_mode", event.target.value)}
                        className={selectClassName}
                      >
                        <option value="show_in_dock">{t("settings.show.in.dock")}</option>
                        <option value="menu_bar_only">{t("common.menu.bar.only")}</option>
                      </select>
                      <p className="text-xs text-gray-500 dark:text-gray-400">{t("settings.at.least.one.display.icon")}</p>
                    </>
                  )}
                </>
              ) : !error && <p className="text-sm text-gray-500 dark:text-gray-400">{t("settings.loading.display.settings")}</p>}
              {error && <p role="alert" className="text-sm text-red-600 dark:text-red-300">{t("settings.could.not.update.display.settings")}: {error}</p>}
              <div className="border-t border-gray-100 dark:border-gray-800" />
            </>
          )}
          <label htmlFor="codex-close-preference" className="block text-sm font-medium text-gray-900 dark:text-gray-100">
            {t("settings.codex.close.method")}
          </label>
          <select id="codex-close-preference" value={closePreference} onChange={(event) => onClosePreferenceChange(event.target.value as CodexClosePreference)} className={selectClassName}>
            <option value="ask">{t("settings.ask.every.time")}</option>
            <option value="graceful">{t("settings.gracefully.close")}</option>
            <option value="force">{t("settings.force.close")}</option>
          </select>
          <p className="text-sm text-gray-500 dark:text-gray-400">
            {t("settings.graceful.close.description")}
          </p>
          <label htmlFor="desktop-reopen-preference" className="block text-sm font-medium text-gray-900 dark:text-gray-100">
            {t("settings.reopen.codex.after.close")}
          </label>
          <select id="desktop-reopen-preference" value={reopenPreference} onChange={(event) => onReopenPreferenceChange(event.target.value as DesktopReopenPreference)} className={selectClassName}>
            <option value="ask">{t("settings.ask.every.time")}</option>
            <option value="always">{t("settings.reopen.desktop.app")}</option>
            <option value="never">{t("settings.keep.closed")}</option>
          </select>
          <p className="text-sm text-gray-500 dark:text-gray-400">
            {t("settings.reopen.preference.description")}
          </p>
        </div>
        <div className="flex justify-end p-5 border-t border-gray-100 dark:border-gray-800">
          <button onClick={onClose} disabled={saving} className="px-4 py-2 text-sm font-medium rounded-lg bg-gray-100 dark:bg-gray-800 hover:bg-gray-200 dark:hover:bg-gray-700 text-gray-700 dark:text-gray-200 disabled:opacity-50">{t("settings.done")}</button>
        </div>
      </div>
    </div>
  );
}
