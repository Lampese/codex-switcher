import { useState, useEffect, useCallback } from "react";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";

type UpdateStatus =
  | { kind: "idle" }
  | { kind: "checking" }
  | { kind: "available"; update: Update }
  | { kind: "downloading"; downloaded: number; total: number | null }
  | { kind: "ready" }
  | { kind: "error"; message: string };

const mutedButtonClass =
  "theme-button-secondary rounded-lg px-3 py-1.5 text-xs font-medium";
const primaryButtonClass =
  "theme-button-primary rounded-lg px-3 py-1.5 text-xs font-medium";

export function UpdateChecker() {
  const [status, setStatus] = useState<UpdateStatus>({ kind: "idle" });
  const [dismissed, setDismissed] = useState(false);

  const checkForUpdate = useCallback(async () => {
    try {
      setStatus({ kind: "checking" });
      setDismissed(false);
      const update = await check();
      setStatus(update ? { kind: "available", update } : { kind: "idle" });
    } catch (err) {
      console.error("Update check failed:", err);
      setStatus({ kind: "idle" });
    }
  }, []);

  useEffect(() => {
    void checkForUpdate();
  }, [checkForUpdate]);

  const handleDownloadAndInstall = async () => {
    if (status.kind !== "available") return;

    try {
      let downloaded = 0;
      let total: number | null = null;

      await status.update.downloadAndInstall((event) => {
        switch (event.event) {
          case "Started":
            total = event.data.contentLength ?? null;
            setStatus({ kind: "downloading", downloaded: 0, total });
            break;
          case "Progress":
            downloaded += event.data.chunkLength;
            setStatus({ kind: "downloading", downloaded, total });
            break;
          case "Finished":
            setStatus({ kind: "ready" });
            break;
        }
      });

      setStatus({ kind: "ready" });
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      console.error("Update install failed:", err);
      setStatus({ kind: "error", message });
    }
  };

  const handleRelaunch = async () => {
    try {
      await relaunch();
    } catch (err) {
      console.error("Relaunch failed:", err);
    }
  };

  if (status.kind === "idle" || status.kind === "checking" || dismissed) {
    return null;
  }

  const formatBytes = (bytes: number) => {
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
    return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  };

  return (
    <div className="fixed bottom-6 left-1/2 z-50 w-full max-w-md -translate-x-1/2 px-4">
      <div
        className="theme-panel rounded-xl p-4 shadow-xl shadow-slate-200/50 dark:shadow-slate-950/60"
        style={{ backgroundColor: "var(--theme-surface)" }}
      >
        {status.kind === "available" && (
          <div className="flex items-start gap-3">
            <div className="min-w-0 flex-1">
              <p className="text-sm font-medium text-slate-900 dark:text-slate-50">
                Update available: v{status.update.version}
              </p>
              {status.update.body && (
                <p className="mt-0.5 truncate text-xs text-slate-500 dark:text-slate-400">
                  {status.update.body}
                </p>
              )}
            </div>
            <div className="flex shrink-0 items-center gap-2">
              <button onClick={() => setDismissed(true)} className={mutedButtonClass}>
                Later
              </button>
              <button onClick={handleDownloadAndInstall} className={primaryButtonClass}>
                Update
              </button>
            </div>
          </div>
        )}

        {status.kind === "downloading" && (
          <div>
            <div className="mb-2 flex items-center justify-between">
              <p className="text-sm font-medium text-slate-900 dark:text-slate-50">
                Downloading update...
              </p>
              <p className="text-xs text-slate-500 dark:text-slate-400">
                {formatBytes(status.downloaded)}
                {status.total ? ` / ${formatBytes(status.total)}` : ""}
              </p>
            </div>
            <div className="theme-progress-track h-1.5 w-full rounded-full">
              <div
                className="theme-button-primary h-1.5 rounded-full transition-all duration-300"
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

        {status.kind === "ready" && (
          <div className="flex items-center justify-between gap-3">
            <p className="text-sm font-medium text-slate-900 dark:text-slate-50">
              Update ready. Restart to apply.
            </p>
            <div className="flex shrink-0 items-center gap-2">
              <button onClick={() => setDismissed(true)} className={mutedButtonClass}>
                Later
              </button>
              <button onClick={handleRelaunch} className={primaryButtonClass}>
                Restart
              </button>
            </div>
          </div>
        )}

        {status.kind === "error" && (
          <div className="flex items-center justify-between gap-3">
            <p className="text-sm text-red-600 dark:text-red-300">
              Update failed: {status.message}
            </p>
            <button onClick={() => setDismissed(true)} className={mutedButtonClass}>
              Dismiss
            </button>
          </div>
        )}
      </div>
    </div>
  );
}
