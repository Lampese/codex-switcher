import { useCallback, useEffect, useRef, useState } from "react";
import type { DesktopReopenPreference } from "../lib/desktopReopen";
import type { CodexClosePreference } from "../lib/codexClosePreference";
import { invokeBackend, isTauriRuntime } from "../lib/platform";
import type { AppSettings, AutoSwitchStrategy, DockDisplayMode } from "../types";

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
  onClose: () => void;
}

export function SettingsModal({
  reopenPreference,
  onReopenPreferenceChange,
  closePreference,
  onClosePreferenceChange,
  onClose,
}: SettingsModalProps) {
  const [displaySettings, setDisplaySettings] = useState<DisplaySettings | null>(null);
  const [appSettings, setAppSettings] = useState<AppSettings | null>(null);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const requestId = useRef(0);
  const desktop = isTauriRuntime();

  const loadSettings = useCallback(async () => {
    const currentRequest = ++requestId.current;
    try {
      const [disp, app] = await Promise.all([
        invokeBackend<DisplaySettings>("get_display_settings"),
        invokeBackend<AppSettings>("get_app_settings"),
      ]);
      if (currentRequest === requestId.current) {
        setDisplaySettings(disp);
        setAppSettings(app);
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
        void loadSettings();
      });
      if (disposed) stop();
      else {
        unlisten = stop;
        void loadSettings();
      }
    }).catch((err) => {
      if (!disposed) setError(String(err));
    });
    return () => {
      disposed = true;
      requestId.current += 1;
      unlisten?.();
    };
  }, [desktop, loadSettings]);

  const changeDisplaySetting = async (command: string, mode: string) => {
    setSaving(true);
    setError(null);
    try {
      await invokeBackend(command, { mode });
      await loadSettings();
    } catch (err) {
      requestId.current += 1;
      setError(String(err));
    } finally {
      setSaving(false);
    }
  };

  const updateAutoRecovery = async (updates: Partial<AppSettings>) => {
    if (!appSettings) return;
    const next: AppSettings = { ...appSettings, ...updates };
    setAppSettings(next);
    setSaving(true);
    setError(null);
    try {
      await invokeBackend("save_auto_recovery_settings", {
        autoRetryCapacityEnabled: next.auto_retry_capacity_enabled,
        autoRetryCapacityMaxAttempts: next.auto_retry_capacity_max_attempts,
        autoRetryCapacityInitialDelaySec: next.auto_retry_capacity_initial_delay_sec,
        autoRetryCapacityEscalateToSwitch: next.auto_retry_capacity_escalate_to_switch,
        autoSwitchLimitEnabled: next.auto_switch_limit_enabled,
        autoSwitchStrategy: next.auto_switch_strategy,
        autoRedeemResetCredits: next.auto_redeem_reset_credits ?? false,
        continuePhrase: next.continue_phrase,
        resetCreditWarningDays: next.reset_credit_warning_days,
        preferredTerminal: next.preferred_terminal,
      });
    } catch (err) {
      setError(String(err));
    } finally {
      setSaving(false);
    }
  };

  const selectClassName = "w-full rounded-lg border border-gray-300 dark:border-gray-700 bg-white dark:bg-gray-800 px-3 py-2 text-sm text-gray-900 dark:text-gray-100 disabled:opacity-50";
  const inputClassName = "w-full rounded-lg border border-gray-300 dark:border-gray-700 bg-white dark:bg-gray-800 px-3 py-2 text-sm text-gray-900 dark:text-gray-100 disabled:opacity-50";

  return (
    <div className="fixed inset-0 bg-black/40 flex items-center justify-center z-50">
      <div role="dialog" aria-modal="true" aria-labelledby="settings-title" className="bg-white dark:bg-gray-900 border border-gray-200 dark:border-gray-700 rounded-2xl w-full max-w-lg mx-4 shadow-xl">
        <div className="p-5 border-b border-gray-100 dark:border-gray-800">
          <h2 id="settings-title" className="text-lg font-semibold text-gray-900 dark:text-gray-100">Settings</h2>
        </div>
        <div className="p-5 space-y-4 max-h-[70vh] overflow-y-auto">
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

          {/* Auto Recovery Section */}
          <div className="space-y-3">
            <h3 className="text-sm font-semibold text-gray-900 dark:text-gray-100 uppercase tracking-wider">
              Auto Recovery & Session Automation
            </h3>

            {/* Model Capacity Auto-Retry */}
            <div className="rounded-xl border border-gray-200 dark:border-gray-800 p-3 space-y-2 bg-gray-50/50 dark:bg-gray-800/30">
              <label className="flex items-center gap-2 cursor-pointer text-sm font-medium text-gray-900 dark:text-gray-100">
                <input
                  type="checkbox"
                  checked={appSettings?.auto_retry_capacity_enabled ?? false}
                  disabled={saving || !appSettings}
                  onChange={(e) => void updateAutoRecovery({ auto_retry_capacity_enabled: e.target.checked })}
                  className="rounded border-gray-300 text-blue-600 focus:ring-blue-500"
                />
                Auto-retry when model is at capacity
              </label>
              <p className="text-xs text-gray-500 dark:text-gray-400">
                Automatically sends queue messages to resume sessions paused by OpenAI capacity limits.
              </p>

              {(appSettings?.auto_retry_capacity_enabled ?? false) && (
                <div className="pt-2 pl-6 space-y-2 border-t border-gray-200/60 dark:border-gray-700/60 text-xs">
                  <div className="flex items-center justify-between gap-2">
                    <span className="text-gray-700 dark:text-gray-300">Max retry attempts:</span>
                    <input
                      type="number"
                      min={1}
                      max={10}
                      value={appSettings?.auto_retry_capacity_max_attempts ?? 3}
                      disabled={saving || !appSettings}
                      onChange={(e) => void updateAutoRecovery({ auto_retry_capacity_max_attempts: Number(e.target.value) })}
                      className="w-16 rounded border border-gray-300 dark:border-gray-700 bg-white dark:bg-gray-800 px-2 py-1 text-xs text-right"
                    />
                  </div>
                  <div className="flex items-center justify-between gap-2">
                    <span className="text-gray-700 dark:text-gray-300">Initial delay (seconds):</span>
                    <input
                      type="number"
                      min={1}
                      max={60}
                      value={appSettings?.auto_retry_capacity_initial_delay_sec ?? 5}
                      disabled={saving || !appSettings}
                      onChange={(e) => void updateAutoRecovery({ auto_retry_capacity_initial_delay_sec: Number(e.target.value) })}
                      className="w-16 rounded border border-gray-300 dark:border-gray-700 bg-white dark:bg-gray-800 px-2 py-1 text-xs text-right"
                    />
                  </div>
                  <label className="flex items-center gap-2 cursor-pointer text-xs text-gray-700 dark:text-gray-300 pt-1">
                    <input
                      type="checkbox"
                      checked={appSettings?.auto_retry_capacity_escalate_to_switch ?? false}
                      disabled={saving || !appSettings}
                      onChange={(e) => void updateAutoRecovery({ auto_retry_capacity_escalate_to_switch: e.target.checked })}
                      className="rounded border-gray-300 text-blue-600 focus:ring-blue-500"
                    />
                    Escalate to account switch if retries continue failing
                  </label>
                </div>
              )}
            </div>

            {/* Usage Limit Auto-Switch */}
            <div className="rounded-xl border border-gray-200 dark:border-gray-800 p-3 space-y-2 bg-gray-50/50 dark:bg-gray-800/30">
              <label className="flex items-center gap-2 cursor-pointer text-sm font-medium text-gray-900 dark:text-gray-100">
                <input
                  type="checkbox"
                  checked={appSettings?.auto_switch_limit_enabled ?? false}
                  disabled={saving || !appSettings}
                  onChange={(e) => void updateAutoRecovery({ auto_switch_limit_enabled: e.target.checked })}
                  className="rounded border-gray-300 text-blue-600 focus:ring-blue-500"
                />
                Auto-switch account on usage limit reached
              </label>
              <p className="text-xs text-gray-500 dark:text-gray-400">
                When a session hits its usage limit, switches to an available account and continues the same conversation. CLI sessions resume in a terminal. On macOS and Linux, the desktop app closes and reopens around the switch; recovery waits while another turn is active.
              </p>

              {(appSettings?.auto_switch_limit_enabled ?? false) && (
                <div className="pt-2 pl-6 space-y-3 border-t border-gray-200/60 dark:border-gray-700/60 text-xs">
                  <div>
                    <label className="block text-gray-700 dark:text-gray-300 mb-1">Switching Strategy:</label>
                    <select
                      value={appSettings?.auto_switch_strategy ?? "smart_balanced"}
                      disabled={saving || !appSettings}
                      onChange={(e) => void updateAutoRecovery({ auto_switch_strategy: e.target.value as AutoSwitchStrategy })}
                      className={selectClassName}
                    >
                      <option value="smart_balanced">Smart Balanced (Banked Resets, Weekly Pacing & Expiring First)</option>
                      <option value="resets_first">Banked Resets First</option>
                      <option value="expiring_subscription_first">Expiring Subscription First</option>
                      <option value="most_remaining_quota">Most Remaining Quota (Effective Capacity)</option>
                      <option value="round_robin">Round Robin</option>
                    </select>
                  </div>

                  <div className="pt-1">
                    <label className="flex items-center gap-2 cursor-pointer font-medium text-gray-900 dark:text-gray-100">
                      <input
                        type="checkbox"
                        checked={appSettings?.auto_redeem_reset_credits ?? false}
                        disabled={saving || !appSettings}
                        onChange={(e) => void updateAutoRecovery({ auto_redeem_reset_credits: e.target.checked })}
                        className="rounded border-gray-300 text-blue-600 focus:ring-blue-500"
                      />
                      Auto-redeem banked reset credits before switching
                    </label>
                    <p className="text-[11px] text-gray-500 dark:text-gray-400 mt-0.5 pl-5">
                      When the active account hits its limit, automatically consumes an available reset credit to restore 100% quota before switching accounts.
                    </p>
                  </div>
                </div>
              )}
            </div>

            {/* Custom Resume Phrase & Warning Days */}
            <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
              <div>
                <label htmlFor="continue-phrase" className="block text-xs font-medium text-gray-900 dark:text-gray-100 mb-1">
                  Resume Prompt Phrase
                </label>
                <input
                  id="continue-phrase"
                  type="text"
                  placeholder="continue or продолжи"
                  value={appSettings?.continue_phrase ?? "continue"}
                  disabled={saving || !appSettings}
                  onChange={(e) => void updateAutoRecovery({ continue_phrase: e.target.value })}
                  className={inputClassName}
                />
                <p className="text-[11px] text-gray-500 dark:text-gray-400 mt-1">
                  Phrase sent to continue task (e.g. &apos;continue&apos; or &apos;продолжи&apos;).
                </p>
              </div>
              <div>
                <label htmlFor="reset-warning-days" className="block text-xs font-medium text-gray-900 dark:text-gray-100 mb-1">
                  Reset Expiration Warning (days)
                </label>
                <input
                  id="reset-warning-days"
                  type="number"
                  min={1}
                  max={30}
                  value={appSettings?.reset_credit_warning_days ?? 3}
                  disabled={saving || !appSettings}
                  onChange={(e) => void updateAutoRecovery({ reset_credit_warning_days: Number(e.target.value) })}
                  className={inputClassName}
                />
                <p className="text-[11px] text-gray-500 dark:text-gray-400 mt-1">
                  Highlights reset credits in red when expiring within N days.
                </p>
              </div>
            </div>
          </div>

          <div className="border-t border-gray-100 dark:border-gray-800" />

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
        </div>
        <div className="flex justify-end p-5 border-t border-gray-100 dark:border-gray-800">
          <button onClick={onClose} disabled={saving} className="px-4 py-2 text-sm font-medium rounded-lg bg-gray-100 dark:bg-gray-800 hover:bg-gray-200 dark:hover:bg-gray-700 text-gray-700 dark:text-gray-200 disabled:opacity-50">Done</button>
        </div>
      </div>
    </div>
  );
}
