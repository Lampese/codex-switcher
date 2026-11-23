import { useCallback, useEffect, useRef, useState } from "react";
import type { DesktopReopenPreference } from "../lib/desktopReopen";
import type { CodexClosePreference } from "../lib/codexClosePreference";
import { CHECK_FOR_UPDATES_EVENT } from "./UpdateChecker";
import { invokeBackend, isTauriRuntime } from "../lib/platform";
import {
  DEFAULT_DATE_TIME_PREFERENCES,
  readDateTimePreferences,
  writeDateTimePreferences,
  type DateFormatPreference,
  type DateTimePreferences,
  type HourFormatPreference,
} from "../lib/dateTimeFormat";
import type { DockDisplayMode } from "../types";

type TrayDisplayMode = "icon_and_session" | "active_usage_text" | "hidden";
interface DisplaySettings {
  tray_display_mode: TrayDisplayMode;
  dock_display_mode: DockDisplayMode | null;
}

interface SettingsModalProps {
  reopenPreference: DesktopReopenPreference;
  onReopenPreferenceChange: (value: DesktopReopenPreference) => void;
  closePreference: CodexClosePreference;
  onClosePreferenceChange: (value: CodexClosePreference) => void;
  autoResumeSessions: boolean;
  onAutoResumeSessionsChange: (value: boolean) => void;
  onClose: () => void;
}

export function SettingsModal({
  reopenPreference,
  onReopenPreferenceChange,
  closePreference,
  onClosePreferenceChange,
  autoResumeSessions,
  onAutoResumeSessionsChange,
  onClose,
}: SettingsModalProps) {
  const [displaySettings, setDisplaySettings] = useState<DisplaySettings | null>(null);
  const [dateTimePrefs, setDateTimePrefs] = useState<DateTimePreferences>(
    DEFAULT_DATE_TIME_PREFERENCES
  );
  const [autoRedeem, setAutoRedeem] = useState(false);
  const [wslConfig, setWslConfig] = useState<{
    supported: boolean;
    enabled: boolean;
    distro: string | null;
    distros: string[];
  } | null>(null);
  const [saving, setSaving] = useState(false);
  const [appVersion, setAppVersion] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const requestId = useRef(0);
  const desktop = isTauriRuntime();

  useEffect(() => {
    if (!desktop) return;
    let active = true;
    void import("@tauri-apps/api/app")
      .then(({ getVersion }) => getVersion())
      .then((version) => { if (active) setAppVersion(version); })
      .catch((err) => console.error("Failed to read app version:", err));
    return () => { active = false; };
  }, [desktop]);

  const handleCheckForUpdates = () => {
    window.dispatchEvent(new Event(CHECK_FOR_UPDATES_EVENT));
    onClose();
  };

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
    setDateTimePrefs(readDateTimePreferences());
    if (!desktop) return;
    void invokeBackend<boolean>("get_auto_redeem_enabled")
      .then(setAutoRedeem)
      .catch((err) => console.error("Failed to read auto-redeem setting:", err));
    void invokeBackend<{
      supported: boolean;
      enabled: boolean;
      distro: string | null;
      distros: string[];
    }>("get_wsl_config")
      .then(setWslConfig)
      .catch((err) => console.error("Failed to read WSL setting:", err));
  }, [desktop]);

  const updateDateTimePrefs = (patch: Partial<DateTimePreferences>) => {
    setDateTimePrefs((current) => {
      const next = { ...current, ...patch };
      writeDateTimePreferences(next);
      return next;
    });
  };

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

  const selectClassName = "w-full rounded-lg border border-gray-300 dark:border-gray-700 bg-white dark:bg-gray-800 px-3 py-2 text-sm text-gray-900 dark:text-gray-100 disabled:opacity-50";

  return (
    <div className="fixed inset-0 bg-black/40 flex items-center justify-center z-50">
      <div role="dialog" aria-modal="true" aria-labelledby="settings-title" className="bg-white dark:bg-gray-900 border border-gray-200 dark:border-gray-700 rounded-2xl w-full max-w-md mx-4 shadow-xl">
        <div className="p-5 border-b border-gray-100 dark:border-gray-800">
          <h2 id="settings-title" className="text-lg font-semibold text-gray-900 dark:text-gray-100">Settings</h2>
        </div>
        <div className="p-5 space-y-3 max-h-[65vh] overflow-y-auto">
          {desktop && (
            <>
              {displaySettings ? (
                <>
                  <label htmlFor="tray-display-mode" className="block text-sm font-medium text-gray-900 dark:text-gray-100">Tray</label>
                  <select
                    id="tray-display-mode"
                    value={displaySettings.tray_display_mode}
                    disabled={saving}
                    onChange={(event) => void changeDisplaySetting("set_tray_display_mode", event.target.value)}
                    className={selectClassName}
                  >
                    <option value="icon_and_session">Icon + Session</option>
                    <option value="active_usage_text">Hourly + Weekly</option>
                    <option value="hidden">Hidden</option>
                  </select>
                  {displaySettings.dock_display_mode !== null && (
                    <>
                      <label htmlFor="dock-display-mode" className="block text-sm font-medium text-gray-900 dark:text-gray-100">Dock Icon</label>
                      <select
                        id="dock-display-mode"
                        value={displaySettings.dock_display_mode}
                        disabled={saving}
                        onChange={(event) => void changeDisplaySetting("set_dock_display_mode", event.target.value)}
                        className={selectClassName}
                      >
                        <option value="show_in_dock">Show in Dock</option>
                        <option value="menu_bar_only">Menu Bar Only</option>
                      </select>
                      <p className="text-xs text-gray-500 dark:text-gray-400">At least one of the Dock or tray icons stays visible so you can reopen Codex Switcher.</p>
                    </>
                  )}
                </>
              ) : !error && <p className="text-sm text-gray-500 dark:text-gray-400">Loading display settings...</p>}
              {error && <p role="alert" className="text-sm text-red-600 dark:text-red-300">Could not update display settings: {error}</p>}
              <div className="border-t border-gray-100 dark:border-gray-800" />
            </>
          )}
          {desktop && (
            <>
              <div className="flex items-center justify-between gap-3">
                <div className="min-w-0">
                  <p className="text-sm font-medium text-gray-900 dark:text-gray-100">App updates</p>
                  <p className="text-xs text-gray-500 dark:text-gray-400">
                    {appVersion ? `Version v${appVersion}. ` : ""}Checks automatically on launch and every 6 hours.
                  </p>
                </div>
                <button type="button" onClick={handleCheckForUpdates}
                  className="shrink-0 rounded-lg border border-gray-300 dark:border-gray-700 px-3 py-2 text-xs font-medium text-gray-900 dark:text-gray-100 hover:bg-gray-100 dark:hover:bg-gray-800">
                  Check for Updates
                </button>
              </div>
              <div className="border-t border-gray-100 dark:border-gray-800" />
            </>
          )}
          <label htmlFor="date-format" className="block text-sm font-medium text-gray-900 dark:text-gray-100">
            Date format
          </label>
          <select
            id="date-format"
            value={dateTimePrefs.dateFormat}
            onChange={(event) =>
              updateDateTimePrefs({ dateFormat: event.target.value as DateFormatPreference })
            }
            className={selectClassName}
          >
            <option value="system">System default</option>
            <option value="dmy">Day Month Year (23 Nov 2026)</option>
            <option value="mdy">Month Day, Year (Nov 23, 2026)</option>
            <option value="ymd">Year-Month-Day (2026-11-23)</option>
          </select>
          <label htmlFor="hour-format" className="block text-sm font-medium text-gray-900 dark:text-gray-100">
            Time format
          </label>
          <select
            id="hour-format"
            value={dateTimePrefs.hourFormat}
            onChange={(event) =>
              updateDateTimePrefs({ hourFormat: event.target.value as HourFormatPreference })
            }
            className={selectClassName}
          >
            <option value="system">System default</option>
            <option value="h12">12-hour</option>
            <option value="h24">24-hour</option>
          </select>
          <label htmlFor="timezone-override" className="block text-sm font-medium text-gray-900 dark:text-gray-100">
            Timezone
          </label>
          <input
            id="timezone-override"
            type="text"
            list="timezone-override-options"
            value={dateTimePrefs.timeZone === "system" ? "" : dateTimePrefs.timeZone}
            placeholder="System timezone"
            onChange={(event) => {
              const value = event.target.value.trim();
              if (!value) {
                updateDateTimePrefs({ timeZone: "system" });
                return;
              }
              try {
                new Intl.DateTimeFormat("en-US", { timeZone: value });
                updateDateTimePrefs({ timeZone: value });
              } catch {
                // Invalid IANA name; keep it in the field but don't persist.
                setDateTimePrefs((current) => ({ ...current, timeZone: value }));
              }
            }}
            className={selectClassName}
          />
          <datalist id="timezone-override-options">
            {(Intl as unknown as { supportedValuesOf?: (key: string) => string[] })
              .supportedValuesOf?.("timeZone")
              .map((zone) => <option key={zone} value={zone} />)}
          </datalist>
          <p className="text-sm text-gray-500 dark:text-gray-400">
            Applies to dates and times shown in the app, e.g. credit expiries and reset times.
          </p>
          <div className="border-t border-gray-100 dark:border-gray-800" />
          {desktop ? (
            <>
              <label htmlFor="auto-redeem-credits" className="flex items-start gap-2">
                <input
                  id="auto-redeem-credits"
                  type="checkbox"
                  checked={autoRedeem}
                  onChange={async (event) => {
                    const value = event.target.checked;
                    setAutoRedeem(value);
                    try {
                      await invokeBackend("set_auto_redeem_enabled", { enabled: value });
                    } catch (err) {
                      setAutoRedeem(!value);
                      setError(String(err));
                    }
                  }}
                  className="mt-1"
                />
                <span>
                  <span className="block text-sm font-medium text-gray-900 dark:text-gray-100">
                    Auto-redeem reset credits
                  </span>
                  <span className="block text-sm text-gray-500 dark:text-gray-400">
                    Consume available rate-limit reset credits shortly before they expire.
                  </span>
                </span>
              </label>
              <div className="border-t border-gray-100 dark:border-gray-800" />
            </>
          ) : null}
          {wslConfig?.supported ? (
            <>
              <label htmlFor="use-wsl-codex" className="flex items-start gap-2">
                <input
                  id="use-wsl-codex"
                  type="checkbox"
                  checked={wslConfig.enabled}
                  onChange={async (event) => {
                    const value = event.target.checked;
                    const previous = wslConfig;
                    setWslConfig({ ...wslConfig, enabled: value });
                    try {
                      await invokeBackend("set_wsl_config", {
                        enabled: value,
                        distro: wslConfig.distro,
                      });
                    } catch (err) {
                      setWslConfig(previous);
                      setError(String(err));
                    }
                  }}
                  className="mt-1"
                />
                <span>
                  <span className="block text-sm font-medium text-gray-900 dark:text-gray-100">
                    Use WSL Codex
                  </span>
                  <span className="block text-sm text-gray-500 dark:text-gray-400">
                    Manage the Codex installation inside Windows Subsystem for Linux.
                  </span>
                </span>
              </label>
              {wslConfig.enabled && wslConfig.distros.length > 0 ? (
                <select
                  aria-label="WSL distribution"
                  value={wslConfig.distro ?? wslConfig.distros[0]}
                  onChange={async (event) => {
                    const distro = event.target.value;
                    const previous = wslConfig;
                    setWslConfig({ ...wslConfig, distro });
                    try {
                      await invokeBackend("set_wsl_config", {
                        enabled: wslConfig.enabled,
                        distro,
                      });
                    } catch (err) {
                      setWslConfig(previous);
                      setError(String(err));
                    }
                  }}
                  className={selectClassName}
                >
                  {wslConfig.distros.map((distro) => (
                    <option key={distro} value={distro}>
                      {distro}
                    </option>
                  ))}
                </select>
              ) : null}
              <div className="border-t border-gray-100 dark:border-gray-800" />
            </>
          ) : null}
          <label htmlFor="codex-close-preference" className="block text-sm font-medium text-gray-900 dark:text-gray-100">
            Codex close method
          </label>
          <select id="codex-close-preference" value={closePreference} onChange={(event) => onClosePreferenceChange(event.target.value as CodexClosePreference)} className={selectClassName}>
            <option value="ask">Ask every time</option>
            <option value="graceful">Gracefully close</option>
            <option value="force">Force close</option>
          </select>
          <p className="text-sm text-gray-500 dark:text-gray-400">
            Graceful close lets Codex finish cleanup. Force close stops it immediately and may lose unsaved work.
          </p>
          <label htmlFor="desktop-reopen-preference" className="block text-sm font-medium text-gray-900 dark:text-gray-100">
            Reopen Codex after close
          </label>
          <select id="desktop-reopen-preference" value={reopenPreference} onChange={(event) => onReopenPreferenceChange(event.target.value as DesktopReopenPreference)} className={selectClassName}>
            <option value="ask">Ask every time</option>
            <option value="always">Reopen desktop app</option>
            <option value="never">Keep closed</option>
          </select>
          <p className="text-sm text-gray-500 dark:text-gray-400">
            Applies to detected Codex desktop apps on macOS and Windows. When switching accounts, the app reopens after the switch succeeds.
          </p>
          <label className="flex items-center gap-2 text-sm font-medium text-gray-900 dark:text-gray-100">
            <input
              type="checkbox"
              checked={autoResumeSessions}
              onChange={(event) => onAutoResumeSessionsChange(event.target.checked)}
              className="h-4 w-4 accent-orange-600"
            />
            Auto-resume interrupted conversations after switching
          </label>
          <p className="text-sm text-gray-500 dark:text-gray-400">
            With desktop reopening enabled, continue recent quota-stopped conversations after switching. On macOS, also continue running chats interrupted by the restart. Finished chats and chats waiting for input are left alone. Switcher checks that each continuation starts.
          </p>
        </div>
        <div className="flex justify-end p-5 border-t border-gray-100 dark:border-gray-800">
          <button onClick={onClose} disabled={saving} className="px-4 py-2 text-sm font-medium rounded-lg bg-gray-100 dark:bg-gray-800 hover:bg-gray-200 dark:hover:bg-gray-700 text-gray-700 dark:text-gray-200 disabled:opacity-50">Done</button>
        </div>
      </div>
    </div>
  );
}
