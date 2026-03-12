import type { CodexProcessInfo } from "../types";
import { getSwitchConfirmationMessage } from "../utils/switching";

interface SwitchConfirmationModalProps {
  isOpen: boolean;
  processInfo: CodexProcessInfo | null;
  onCancel: () => void;
  onKeepRunning: () => void;
  onRestartRunning: () => void;
}

export function SwitchConfirmationModal({
  isOpen,
  processInfo,
  onCancel,
  onKeepRunning,
  onRestartRunning,
}: SwitchConfirmationModalProps) {
  if (!isOpen || !processInfo) return null;

  return (
    <div className="fixed inset-0 z-[70] bg-black/50 flex items-center justify-center p-4">
      <div className="w-full max-w-lg rounded-3xl border border-gray-200 bg-white shadow-2xl overflow-hidden">
        <div className="border-b border-gray-100 px-6 py-5">
          <h2 className="text-xl font-semibold text-gray-900">Switch Account</h2>
          <p className="mt-2 text-sm leading-6 text-gray-600">
            {getSwitchConfirmationMessage(processInfo)}
          </p>
        </div>

        <div className="px-6 py-5">
          <div className="rounded-2xl border border-amber-200 bg-amber-50 px-4 py-3 text-sm text-amber-900">
            Keeping current sessions running only affects future Codex launches. Existing sessions
            will continue using their current login until you restart them manually.
          </div>
        </div>

        <div className="flex items-center justify-end gap-3 border-t border-gray-100 px-6 py-5">
          <button
            onClick={onCancel}
            className="rounded-lg bg-gray-100 px-4 py-2.5 text-sm font-medium text-gray-700 hover:bg-gray-200 transition-colors"
          >
            Cancel
          </button>
          <button
            onClick={onKeepRunning}
            className="rounded-lg border border-gray-200 bg-white px-4 py-2.5 text-sm font-medium text-gray-700 hover:bg-gray-50 transition-colors"
          >
            Switch New Sessions Only
          </button>
          <button
            onClick={onRestartRunning}
            className="rounded-lg bg-gray-900 px-4 py-2.5 text-sm font-medium text-white hover:bg-gray-800 transition-colors"
          >
            Restart and Switch
          </button>
        </div>
      </div>
    </div>
  );
}
