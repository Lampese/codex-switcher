interface MissedScheduledWarmupModalProps {
  isOpen: boolean;
  timeLabel: string | null;
  accountCount: number;
  running: boolean;
  onRunNow: () => Promise<void>;
  onSkipToday: () => Promise<void>;
}

export function MissedScheduledWarmupModal({
  isOpen,
  timeLabel,
  accountCount,
  running,
  onRunNow,
  onSkipToday,
}: MissedScheduledWarmupModalProps) {
  if (!isOpen) return null;

  return (
    <div className="fixed inset-0 z-[60] bg-black/50 flex items-center justify-center p-4">
      <div className="w-full max-w-lg rounded-3xl border border-gray-200 bg-white shadow-2xl overflow-hidden">
        <div className="border-b border-gray-100 px-6 py-5">
          <h2 className="text-xl font-semibold text-gray-900">Missed Scheduled Warmup</h2>
          <p className="mt-2 text-sm text-gray-500">
            The daily warmup for {timeLabel || "your saved time"} did not run because the app was
            not open.
          </p>
        </div>

        <div className="space-y-4 px-6 py-5">
          <div className="rounded-2xl border border-amber-200 bg-amber-50 px-4 py-3 text-sm text-amber-900">
            {accountCount === 1
              ? "1 selected account is ready to warm now."
              : `${accountCount} selected accounts are ready to warm now.`}
          </div>
          <p className="text-sm text-gray-600">
            Run it now to catch up today, or skip and wait for tomorrow&apos;s scheduled run.
          </p>
        </div>

        <div className="flex items-center justify-end gap-3 border-t border-gray-100 px-6 py-5">
          <button
            onClick={() => {
              void onSkipToday();
            }}
            disabled={running}
            className="rounded-lg bg-gray-100 px-4 py-2.5 text-sm font-medium text-gray-700 hover:bg-gray-200 transition-colors disabled:opacity-50"
          >
            Skip Today
          </button>
          <button
            onClick={() => {
              void onRunNow();
            }}
            disabled={running}
            className="rounded-lg bg-gray-900 px-4 py-2.5 text-sm font-medium text-white hover:bg-gray-800 transition-colors disabled:opacity-50"
          >
            {running ? "Running..." : "Run Warmup Now"}
          </button>
        </div>
      </div>
    </div>
  );
}
