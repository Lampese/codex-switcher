import { useState, useEffect, useCallback, useRef } from "react";
import type { Update } from "@tauri-apps/plugin-updater";
import { isTauriRuntime } from "../lib/platform";
import { installUpdate, type UpdateInstallationStatus } from "../lib/updateInstallation";

export const CHECK_FOR_UPDATES_EVENT = "codex-switcher:check-for-updates";
const UPDATE_CHECK_INTERVAL_MS = 6 * 60 * 60 * 1000;

type UpdateStatus =
  | { kind: "idle" }
  | { kind: "checking"; manual: boolean }
  | { kind: "up-to-date" }
  | { kind: "available"; update: Update }
  | UpdateInstallationStatus
  | { kind: "error"; message: string };

export function UpdateChecker() {
  const [status, setStatus] = useState<UpdateStatus>({ kind: "idle" });
  const [dismissed, setDismissed] = useState(false);
  const checkInFlightRef = useRef(false);
  const manualRequestedRef = useRef(false);

  const checkForUpdate = useCallback(async (manual = false) => {
    if (!isTauriRuntime()) return;

    if (manual) {
      // A user request joins an automatic check already in flight, if any.
      manualRequestedRef.current = true;
      setDismissed(false);
      setStatus({ kind: "checking", manual: true });
    }
    if (checkInFlightRef.current) return;

    checkInFlightRef.current = true;
    if (!manual) {
      setStatus({ kind: "checking", manual: false });
      setDismissed(false);
    }

    try {
      const { check } = await import("@tauri-apps/plugin-updater");
      const update = await check();
      if (update) {
        setStatus({ kind: "available", update });
      } else {
        setStatus(manualRequestedRef.current ? { kind: "up-to-date" } : { kind: "idle" });
      }
    } catch (err) {
      console.error("Update check failed:", err);
      if (manualRequestedRef.current) {
        setStatus({
          kind: "error",
          message: err instanceof Error ? err.message : String(err),
        });
      } else {
        setStatus({ kind: "idle" });
      }
    } finally {
      manualRequestedRef.current = false;
      checkInFlightRef.current = false;
    }
  }, []);

  useEffect(() => {
    if (!isTauriRuntime()) return;

    const handleManualCheck = () => { void checkForUpdate(true); };
    window.addEventListener(CHECK_FOR_UPDATES_EVENT, handleManualCheck);
    void checkForUpdate();

    const interval = window.setInterval(() => {
      void checkForUpdate();
    }, UPDATE_CHECK_INTERVAL_MS);

    return () => {
      window.clearInterval(interval);
      window.removeEventListener(CHECK_FOR_UPDATES_EVENT, handleManualCheck);
    };
  }, [checkForUpdate]);

  const handleDownloadAndInstall = async () => {
    if (status.kind !== "available") return;
    const { update } = status;

    try {
      if (!isTauriRuntime()) return;
      await installUpdate(update, setStatus);
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      console.error("Update install failed:", err);
      setStatus({ kind: "error", message });
    }
  };

  const handleRelaunch = async () => {
    try {
      if (!isTauriRuntime()) return;
      const { relaunch } = await import("@tauri-apps/plugin-process");
      await relaunch();
    } catch (err) {
      console.error("Relaunch failed:", err);
    }
  };

  if (!isTauriRuntime()) {
    return null;
  }

  if (status.kind === "idle" || (status.kind === "checking" && !status.manual) || dismissed) {
    return null;
  }

  const formatBytes = (bytes: number) => {
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
    return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  };

  return (
    <div className="fixed bottom-6 left-1/2 -translate-x-1/2 z-50 max-w-md w-full px-4">
      <div className="bg-white dark:bg-gray-900 border border-gray-200 dark:border-gray-700 rounded-xl shadow-xl p-4">
        {status.kind === "checking" && (
          <p className="text-sm font-medium text-gray-900 dark:text-gray-100" role="status">
            Checking for updates...
          </p>
        )}
        {status.kind === "up-to-date" && (
          <div className="flex items-center justify-between gap-3">
            <p className="text-sm font-medium text-gray-900 dark:text-gray-100" role="status">
              Codex Switcher is up to date.
            </p>
            <button onClick={() => setDismissed(true)} className="px-3 py-1.5 text-xs font-medium rounded-lg bg-gray-100 hover:bg-gray-200 dark:bg-gray-800 dark:hover:bg-gray-700 text-gray-600 dark:text-gray-300">
              Dismiss
            </button>
          </div>
        )}
        {status.kind === "available" && (
          <div className="flex items-start gap-3">
            <div className="flex-1 min-w-0">
              <p className="text-sm font-medium text-gray-900 dark:text-gray-100">
                Update available: v{status.update.version}
              </p>
              {status.update.body && (
                <p className="text-xs text-gray-500 dark:text-gray-400 mt-0.5 truncate">
                  {status.update.body}
                </p>
              )}
            </div>
            <div className="flex items-center gap-2 shrink-0">
              <button
                onClick={() => setDismissed(true)}
                className="px-3 py-1.5 text-xs font-medium rounded-lg bg-gray-100 hover:bg-gray-200 dark:bg-gray-800 dark:hover:bg-gray-700 text-gray-600 dark:text-gray-300 transition-colors"
              >
                Later
              </button>
              <button
                onClick={handleDownloadAndInstall}
                className="px-3 py-1.5 text-xs font-medium rounded-lg bg-gray-900 hover:bg-gray-800 dark:bg-gray-100 dark:hover:bg-gray-200 text-white dark:text-gray-900 transition-colors"
              >
                Update
              </button>
            </div>
          </div>
        )}

        {status.kind === "downloading" && (
          <div>
            <div className="flex items-center justify-between mb-2">
              <p className="text-sm font-medium text-gray-900 dark:text-gray-100">Downloading update...</p>
              <p className="text-xs text-gray-500 dark:text-gray-400">
                {formatBytes(status.downloaded)}
                {status.total ? ` / ${formatBytes(status.total)}` : ""}
              </p>
            </div>
            <div className="w-full bg-gray-100 dark:bg-gray-800 rounded-full h-1.5">
              <div
                className="bg-gray-900 dark:bg-gray-100 h-1.5 rounded-full transition-all duration-300"
                style={{
                  width:
                    status.total && status.total > 0
                      ? `${Math.min(100, (status.downloaded / status.total) * 100)}%`
                      : "50%",
                }}
              />
            </div>
          </div>
        )}

        {status.kind === "installing" && (
          <p className="text-sm font-medium text-gray-900 dark:text-gray-100" role="status">
            Installing update. Complete any system authorization prompt...
          </p>
        )}

        {status.kind === "ready" && (
          <div className="flex items-center justify-between">
            <p className="text-sm font-medium text-gray-900 dark:text-gray-100">
              Update ready. Restart to apply.
            </p>
            <div className="flex items-center gap-2 shrink-0">
              <button
                onClick={() => setDismissed(true)}
                className="px-3 py-1.5 text-xs font-medium rounded-lg bg-gray-100 hover:bg-gray-200 dark:bg-gray-800 dark:hover:bg-gray-700 text-gray-600 dark:text-gray-300 transition-colors"
              >
                Later
              </button>
              <button
                onClick={handleRelaunch}
                className="px-3 py-1.5 text-xs font-medium rounded-lg bg-gray-900 hover:bg-gray-800 dark:bg-gray-100 dark:hover:bg-gray-200 text-white dark:text-gray-900 transition-colors"
              >
                Restart
              </button>
            </div>
          </div>
        )}

        {status.kind === "error" && (
          <div className="flex items-center justify-between">
            <p className="text-sm text-red-600 dark:text-red-300">
              Update failed: {status.message}
            </p>
            <button
              onClick={() => setDismissed(true)}
              className="px-3 py-1.5 text-xs font-medium rounded-lg bg-gray-100 hover:bg-gray-200 dark:bg-gray-800 dark:hover:bg-gray-700 text-gray-600 dark:text-gray-300 transition-colors shrink-0 ml-2"
            >
              Dismiss
            </button>
          </div>
        )}
      </div>
    </div>
  );
}
