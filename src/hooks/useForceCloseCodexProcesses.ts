import { useCallback, useRef, useState } from "react";
import type { CodexProcessInfo } from "../types";
import { invokeBackend } from "../lib/platform";
import { useI18n } from "../lib/i18n";

interface KillCodexProcessesResult {
  targeted_count: number;
  killed_pids: number[];
  failed_pids: number[];
  reopen_token?: string | null;
}

interface UseForceCloseCodexProcessesOptions {
  processCount: number;
  checkProcesses: () => Promise<CodexProcessInfo | null>;
  showToast: (message: string, isError?: boolean) => void;
  formatError: (err: unknown) => string;
}

export function useForceCloseCodexProcesses({
  processCount,
  checkProcesses,
  showToast,
  formatError,
}: UseForceCloseCodexProcessesOptions) {
  const { t } = useI18n();
  const translatorRef = useRef(t);
  translatorRef.current = t;
  const [confirmOpen, setConfirmOpen] = useState(false);
  const [isForceClosing, setIsForceClosing] = useState(false);

  const closeCodexProcesses = useCallback(async (reopenDesktop = false, forceClose = false) => {
    try {
      setIsForceClosing(true);

      const result = await invokeBackend<KillCodexProcessesResult>(
        "kill_codex_processes",
        { reopenDesktop, forceClose }
      );
      const latestProcessInfo = await checkProcesses();
      const remainingCount = latestProcessInfo?.count ?? processCount;
      const closedCount = Math.max(0, processCount - remainingCount);

      if (!latestProcessInfo) {
        showToast(translatorRef.current("close.processes.verify_failed"), true);
      } else if (result.targeted_count === 0) {
        showToast(translatorRef.current("close.processes.none"));
      } else if (remainingCount === 0) {
        showToast(
          translatorRef.current("close.processes.success", {
            action: forceClose ? translatorRef.current("close.processes.force") : translatorRef.current("close.processes.graceful"),
            count: processCount,
          })
        );
      } else if (closedCount > 0) {
        showToast(
          translatorRef.current("close.processes.partial", {
            action: forceClose ? translatorRef.current("close.processes.force") : translatorRef.current("close.processes.graceful"),
            closed: closedCount,
            total: processCount,
            remaining: remainingCount,
          }),
          true
        );
      } else {
        showToast(
          translatorRef.current("close.processes.failed", {
            action: forceClose ? translatorRef.current("close.processes.force_close") : translatorRef.current("close.processes.graceful_close"),
            count: remainingCount,
          }),
          true
        );
      }

      return { processInfo: latestProcessInfo, reopenToken: result.reopen_token ?? null };
    } catch (err) {
      console.error("Failed to close Codex processes:", err);
      showToast(translatorRef.current("close.processes.request_failed", { error: formatError(err) }), true);
      return null;
    } finally {
      setConfirmOpen(false);
      setIsForceClosing(false);
    }
  }, [checkProcesses, formatError, processCount, showToast]);

  return {
    forceCloseConfirmOpen: confirmOpen,
    setForceCloseConfirmOpen: setConfirmOpen,
    isForceClosingCodex: isForceClosing,
    closeCodexProcesses,
  };
}
