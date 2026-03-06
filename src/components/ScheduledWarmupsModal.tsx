import { useEffect, useMemo, useState } from "react";
import type { AccountInfo, ScheduledWarmupSettings } from "../types";

interface ScheduledWarmupsModalProps {
  isOpen: boolean;
  accounts: AccountInfo[];
  initialValue: ScheduledWarmupSettings | null;
  nextRunLabel: string | null;
  onClose: () => void;
  onSave: (schedule: ScheduledWarmupSettings) => Promise<void>;
}

const DEFAULT_TIME = "09:00";

export function ScheduledWarmupsModal({
  isOpen,
  accounts,
  initialValue,
  nextRunLabel,
  onClose,
  onSave,
}: ScheduledWarmupsModalProps) {
  const [enabled, setEnabled] = useState(false);
  const [localTime, setLocalTime] = useState(DEFAULT_TIME);
  const [selectedIds, setSelectedIds] = useState<string[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    if (!isOpen) return;
    setEnabled(initialValue?.enabled ?? false);
    setLocalTime(initialValue?.local_time || DEFAULT_TIME);
    setSelectedIds(initialValue?.account_ids ?? []);
    setError(null);
    setSaving(false);
  }, [initialValue, isOpen]);

  const allSelected = useMemo(
    () => accounts.length > 0 && accounts.every((account) => selectedIds.includes(account.id)),
    [accounts, selectedIds]
  );

  if (!isOpen) return null;

  const toggleAccount = (accountId: string) => {
    setSelectedIds((prev) =>
      prev.includes(accountId)
        ? prev.filter((id) => id !== accountId)
        : [...prev, accountId]
    );
  };

  const handleToggleAll = () => {
    setSelectedIds(allSelected ? [] : accounts.map((account) => account.id));
  };

  const handleSave = async () => {
    if (!localTime) {
      setError("Choose a local time.");
      return;
    }

    if (enabled && selectedIds.length === 0) {
      setError("Select at least one account.");
      return;
    }

    try {
      setSaving(true);
      setError(null);
      await onSave({
        enabled,
        local_time: localTime,
        account_ids: selectedIds,
        last_run_local_date: initialValue?.last_run_local_date ?? null,
        last_missed_prompt_local_date: initialValue?.last_missed_prompt_local_date ?? null,
      });
      onClose();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="fixed inset-0 z-50 bg-black/40 flex items-center justify-center p-4">
      <div className="w-full max-w-2xl rounded-3xl border border-gray-200 bg-white shadow-2xl overflow-hidden">
        <div className="flex items-center justify-between border-b border-gray-100 px-6 py-5">
          <div>
            <h2 className="text-xl font-semibold text-gray-900">Scheduled Warmups</h2>
            <p className="mt-1 text-sm text-gray-500">
              Run a daily warmup in your system local time while the app is open.
            </p>
          </div>
          <button
            onClick={onClose}
            className="text-gray-400 hover:text-gray-600 transition-colors"
          >
            ✕
          </button>
        </div>

        <div className="space-y-5 px-6 py-5">
          <div className="flex items-center justify-between rounded-2xl border border-gray-200 bg-gray-50 px-4 py-3">
            <div>
              <p className="text-sm font-medium text-gray-900">Enable daily scheduled warmups</p>
              <p className="text-xs text-gray-500">
                A missed run will prompt you once when you reopen the app later that day.
              </p>
            </div>
            <button
              type="button"
              onClick={() => setEnabled((prev) => !prev)}
              className={`relative inline-flex h-7 w-12 items-center rounded-full transition-colors ${
                enabled ? "bg-gray-900" : "bg-gray-300"
              }`}
            >
              <span
                className={`inline-block h-5 w-5 transform rounded-full bg-white transition-transform ${
                  enabled ? "translate-x-6" : "translate-x-1"
                }`}
              />
            </button>
          </div>

          <div className="grid gap-4 md:grid-cols-[180px_minmax(0,1fr)] md:items-end">
            <div>
              <label className="block text-sm font-medium text-gray-700 mb-2">Local Time</label>
              <input
                type="time"
                value={localTime}
                onChange={(e) => setLocalTime(e.target.value)}
                className="w-full rounded-xl border border-gray-200 bg-white px-4 py-2.5 text-gray-900 focus:outline-none focus:ring-1 focus:ring-gray-400 focus:border-gray-400"
              />
            </div>
            <div className="rounded-2xl border border-amber-200 bg-amber-50 px-4 py-3 text-sm text-amber-900">
              {nextRunLabel ? `Next run: ${nextRunLabel}` : "Save a valid time to preview the next run."}
            </div>
          </div>

          <div className="rounded-2xl border border-gray-200">
            <div className="flex items-center justify-between border-b border-gray-100 px-4 py-3">
              <div>
                <p className="text-sm font-medium text-gray-900">Accounts</p>
                <p className="text-xs text-gray-500">
                  Choose which accounts are included in the scheduled warmup.
                </p>
              </div>
              <button
                type="button"
                onClick={handleToggleAll}
                disabled={accounts.length === 0}
                className="rounded-lg border border-gray-200 bg-white px-3 py-2 text-sm font-medium text-gray-700 hover:bg-gray-50 disabled:opacity-50"
              >
                {allSelected ? "Clear All" : "Select All"}
              </button>
            </div>
            <div className="max-h-72 space-y-2 overflow-y-auto px-4 py-4">
              {accounts.length === 0 ? (
                <p className="text-sm text-gray-500">Add an account before scheduling warmups.</p>
              ) : (
                accounts.map((account) => (
                  <label
                    key={account.id}
                    className="flex items-start gap-3 rounded-xl border border-gray-200 px-4 py-3 hover:border-gray-300"
                  >
                    <input
                      type="checkbox"
                      checked={selectedIds.includes(account.id)}
                      onChange={() => toggleAccount(account.id)}
                      className="mt-1 h-4 w-4 rounded border-gray-300 text-gray-900 focus:ring-gray-400"
                    />
                    <span className="min-w-0">
                      <span className="block text-sm font-medium text-gray-900">
                        {account.name}
                      </span>
                      <span className="block text-xs text-gray-500">
                        {account.email || account.plan_type || account.auth_mode}
                      </span>
                    </span>
                  </label>
                ))
              )}
            </div>
          </div>

          {error && (
            <div className="rounded-xl border border-red-200 bg-red-50 px-4 py-3 text-sm text-red-700">
              {error}
            </div>
          )}
        </div>

        <div className="flex items-center justify-between border-t border-gray-100 px-6 py-5">
          <p className="text-xs text-gray-500">
            {enabled
              ? "Scheduled runs fire once per local day."
              : "You can save this disabled and enable it later."}
          </p>
          <div className="flex items-center gap-3">
            <button
              onClick={onClose}
              className="rounded-lg bg-gray-100 px-4 py-2.5 text-sm font-medium text-gray-700 hover:bg-gray-200 transition-colors"
            >
              Cancel
            </button>
            <button
              onClick={handleSave}
              disabled={saving}
              className="rounded-lg bg-gray-900 px-4 py-2.5 text-sm font-medium text-white hover:bg-gray-800 transition-colors disabled:opacity-50"
            >
              {saving ? "Saving..." : "Save Schedule"}
            </button>
          </div>
        </div>
      </div>
    </div>
  );
}
