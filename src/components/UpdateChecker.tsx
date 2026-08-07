import { useState, useEffect, useCallback } from "react";
import type { Update } from "@tauri-apps/plugin-updater";
import { isTauriRuntime } from "../lib/platform";
import { useI18n } from "../lib/i18n";

type UpdateStatus =
  | { kind: "idle" }
  | { kind: "checking" }
  | { kind: "available"; update: Update }
  | { kind: "downloading"; downloaded: number; total: number | null }
  | { kind: "ready" }
  | { kind: "up-to-date" }
  | { kind: "error"; message: string };

export function UpdateChecker() {
  const { t } = useI18n();
  const [status, setStatus] = useState<UpdateStatus>({ kind: "idle" });
  const [dismissed, setDismissed] = useState(false);

  const checkForUpdate = useCallback(async (manual = false) => {
    if (!isTauriRuntime()) return;

    try {
      setStatus({ kind: "checking" });
      setDismissed(false);
      const { check } = await import("@tauri-apps/plugin-updater");
      const update = await check();
      if (update) {
        setStatus({ kind: "available", update });
      } else {
        setStatus(manual ? { kind: "up-to-date" } : { kind: "idle" });
      }
    } catch (err) {
      console.error("Update check failed:", err);
      if (manual) {
        const message = err instanceof Error ? err.message : String(err);
        setStatus({ kind: "error", message });
      } else {
        setStatus({ kind: "idle" });
      }
    }
  }, []);

  useEffect(() => {
    if (!isTauriRuntime()) return;
    void checkForUpdate();
  }, [checkForUpdate]);

  const handleDownloadAndInstall = async () => {
    if (status.kind !== "available") return;
    const { update } = status;

    try {
      if (!isTauriRuntime()) return;
      let downloaded = 0;
      let total: number | null = null;

      await update.downloadAndInstall((event) => {
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

  if (
    status.kind === "idle" ||
    status.kind === "checking" ||
    status.kind === "up-to-date" ||
    dismissed
  ) {
    return (
      <button
        type="button"
        onClick={() => void checkForUpdate(true)}
        disabled={status.kind === "checking"}
        className="fixed bottom-6 right-6 z-40 rounded-lg border border-gray-200 bg-white px-3 py-2 text-xs font-medium text-gray-600 shadow-lg transition-colors hover:bg-gray-50 disabled:cursor-wait disabled:opacity-70 dark:border-gray-700 dark:bg-gray-900 dark:text-gray-300 dark:hover:bg-gray-800"
      >
        {status.kind === "checking"
          ? t("updateChecking")
          : status.kind === "up-to-date"
            ? t("updateUpToDate")
            : t("updateCheck")}
      </button>
    );
  }

  const formatBytes = (bytes: number) => {
    if (bytes < 1024) return `${bytes} B`;
    if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
    return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  };

  return (
    <div className="fixed bottom-6 left-1/2 -translate-x-1/2 z-50 max-w-md w-full px-4">
      <div className="bg-white dark:bg-gray-900 border border-gray-200 dark:border-gray-700 rounded-xl shadow-xl p-4">
        {status.kind === "available" && (
          <div className="flex items-start gap-3">
            <div className="flex-1 min-w-0">
              <p className="text-sm font-medium text-gray-900 dark:text-gray-100">
                {t("updateAvailable", { version: status.update.version })}
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
                {t("updateLater")}
              </button>
              <button
                onClick={handleDownloadAndInstall}
                className="px-3 py-1.5 text-xs font-medium rounded-lg bg-gray-900 hover:bg-gray-800 dark:bg-gray-100 dark:hover:bg-gray-200 text-white dark:text-gray-900 transition-colors"
              >
                {t("updateInstall")}
              </button>
            </div>
          </div>
        )}

        {status.kind === "downloading" && (
          <div>
            <div className="flex items-center justify-between mb-2">
              <p className="text-sm font-medium text-gray-900 dark:text-gray-100">{t("updateDownloading")}</p>
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

        {status.kind === "ready" && (
          <div className="flex items-center justify-between">
            <p className="text-sm font-medium text-gray-900 dark:text-gray-100">
              {t("updateReady")}
            </p>
            <div className="flex items-center gap-2 shrink-0">
              <button
                onClick={() => setDismissed(true)}
                className="px-3 py-1.5 text-xs font-medium rounded-lg bg-gray-100 hover:bg-gray-200 dark:bg-gray-800 dark:hover:bg-gray-700 text-gray-600 dark:text-gray-300 transition-colors"
              >
                {t("updateLater")}
              </button>
              <button
                onClick={handleRelaunch}
                className="px-3 py-1.5 text-xs font-medium rounded-lg bg-gray-900 hover:bg-gray-800 dark:bg-gray-100 dark:hover:bg-gray-200 text-white dark:text-gray-900 transition-colors"
              >
                {t("updateRestart")}
              </button>
            </div>
          </div>
        )}

        {status.kind === "error" && (
          <div className="flex items-center justify-between">
            <p className="text-sm text-red-600 dark:text-red-300">
              {t("updateFailed", { message: status.message })}
            </p>
            <div className="flex items-center gap-2 shrink-0 ml-2">
              <button
                onClick={() => void checkForUpdate(true)}
                className="px-3 py-1.5 text-xs font-medium rounded-lg bg-gray-900 hover:bg-gray-800 dark:bg-gray-100 dark:hover:bg-gray-200 text-white dark:text-gray-900 transition-colors"
              >
                {t("updateCheck")}
              </button>
              <button
                onClick={() => setDismissed(true)}
                className="px-3 py-1.5 text-xs font-medium rounded-lg bg-gray-100 hover:bg-gray-200 dark:bg-gray-800 dark:hover:bg-gray-700 text-gray-600 dark:text-gray-300 transition-colors"
              >
                {t("updateDismiss")}
              </button>
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
