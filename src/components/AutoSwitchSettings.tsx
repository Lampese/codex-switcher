import { useState } from "react";
import type { AutoSwitchConfig, AutoSwitchEvent, AccountInfo } from "../types";

interface AutoSwitchSettingsProps {
  config: AutoSwitchConfig;
  isRunning: boolean;
  events: AutoSwitchEvent[];
  accounts: AccountInfo[];
  loading: boolean;
  error: string | null;
  onConfigChange: (config: Partial<AutoSwitchConfig>) => Promise<void>;
  onStart: () => Promise<void>;
  onStop: () => Promise<void>;
  onClearEvents: () => Promise<void>;
}

function formatEventTime(timestamp: number): string {
  const date = new Date(timestamp * 1000);
  return date.toLocaleString();
}

function formatReason(reason: string): string {
  switch (reason) {
    case "PrimaryLimitReached":
      return "5-hour limit reached";
    case "WeeklyLimitReached":
      return "Weekly limit reached";
    case "BothLimitsReached":
      return "Both limits reached";
    default:
      return reason;
  }
}

export function AutoSwitchSettings({
  config,
  isRunning,
  events,
  accounts,
  loading,
  error,
  onConfigChange,
  onStart,
  onStop,
  onClearEvents,
}: AutoSwitchSettingsProps) {
  const [threshold, setThreshold] = useState(config.threshold_percent);
  const [interval, setInterval] = useState(config.check_interval_seconds);
  const [respectWeekly, setRespectWeekly] = useState(config.respect_weekly_limit);

  const handleSaveSettings = async () => {
    await onConfigChange({
      threshold_percent: threshold,
      check_interval_seconds: interval,
      respect_weekly_limit: respectWeekly,
    });
  };

  const getAccountName = (accountId: string): string => {
    const account = accounts.find((a) => a.id === accountId);
    return account?.name ?? accountId.slice(0, 8);
  };

  return (
    <div className="space-y-6">
      {/* Status and Toggle */}
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-3">
          <div
            className={`h-3 w-3 rounded-full ${
              isRunning ? "bg-green-500" : "bg-gray-300"
            }`}
          ></div>
          <span className="text-sm font-medium text-gray-900">
            {isRunning ? "Auto-switch active" : "Auto-switch inactive"}
          </span>
        </div>
        <button
          onClick={isRunning ? onStop : onStart}
          disabled={loading}
          className={`px-4 py-2 text-sm font-medium rounded-lg transition-colors ${
            isRunning
              ? "bg-red-100 text-red-700 hover:bg-red-200"
              : "bg-green-100 text-green-700 hover:bg-green-200"
          } disabled:opacity-50`}
        >
          {loading ? "..." : isRunning ? "Stop" : "Start"}
        </button>
      </div>

      {error && (
        <div className="p-3 bg-red-50 border border-red-200 rounded-lg text-red-600 text-sm">
          {error}
        </div>
      )}

      {/* Settings */}
      <div className="space-y-4">
        <h3 className="text-sm font-medium text-gray-700">Settings</h3>

        {/* Threshold */}
        <div className="flex items-center gap-4">
          <label className="text-sm text-gray-600 w-40">
            Switch when usage ≥
          </label>
          <div className="flex items-center gap-2">
            <input
              type="number"
              min="50"
              max="99"
              step="1"
              value={threshold}
              onChange={(e) => setThreshold(parseFloat(e.target.value))}
              className="w-20 px-2 py-1.5 text-sm border border-gray-300 rounded-lg focus:outline-none focus:ring-2 focus:ring-blue-500"
            />
            <span className="text-sm text-gray-500">%</span>
          </div>
        </div>

        {/* Check Interval */}
        <div className="flex items-center gap-4">
          <label className="text-sm text-gray-600 w-40">
            Check every
          </label>
          <div className="flex items-center gap-2">
            <input
              type="number"
              min="10"
              max="3600"
              step="10"
              value={interval}
              onChange={(e) => setInterval(parseInt(e.target.value, 10))}
              className="w-24 px-2 py-1.5 text-sm border border-gray-300 rounded-lg focus:outline-none focus:ring-2 focus:ring-blue-500"
            />
            <span className="text-sm text-gray-500">seconds</span>
          </div>
        </div>

        {/* Respect Weekly Limit */}
        <div className="flex items-center gap-4">
          <label className="text-sm text-gray-600 w-40">
            Consider weekly limit
          </label>
          <input
            type="checkbox"
            checked={respectWeekly}
            onChange={(e) => setRespectWeekly(e.target.checked)}
            className="h-4 w-4 text-blue-600 focus:ring-blue-500 border-gray-300 rounded"
          />
        </div>

        <button
          onClick={handleSaveSettings}
          disabled={loading}
          className="px-4 py-2 text-sm font-medium rounded-lg bg-gray-900 text-white hover:bg-gray-800 transition-colors disabled:opacity-50"
        >
          Save Settings
        </button>
      </div>

      {/* Recent Events */}
      {events.length > 0 && (
        <div className="space-y-3">
          <div className="flex items-center justify-between">
            <h3 className="text-sm font-medium text-gray-700">Recent Switches</h3>
            <button
              onClick={onClearEvents}
              className="text-xs text-gray-500 hover:text-gray-700"
            >
              Clear
            </button>
          </div>
          <div className="space-y-2">
            {events.slice(0, 5).map((event, i) => (
              <div
                key={i}
                className="p-3 bg-gray-50 rounded-lg text-sm"
              >
                <div className="flex items-center gap-2 text-gray-900">
                  <span className="font-medium">{getAccountName(event.from_account_id)}</span>
                  <span className="text-gray-400">→</span>
                  <span className="font-medium">{getAccountName(event.to_account_id)}</span>
                </div>
                <div className="flex items-center gap-3 text-xs text-gray-500 mt-1">
                  <span>{formatReason(event.reason)}</span>
                  <span>•</span>
                  <span>{event.triggered_at_percent.toFixed(0)}% used</span>
                  <span>•</span>
                  <span>{formatEventTime(event.timestamp)}</span>
                </div>
              </div>
            ))}
          </div>
        </div>
      )}

      {/* Help Text */}
      <div className="text-xs text-gray-500 space-y-1">
        <p>Auto-switch automatically changes to another account when the current account's usage reaches the threshold.</p>
        <p>The monitor pauses while Codex is running to avoid interrupting active sessions.</p>
      </div>
    </div>
  );
}