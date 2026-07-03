import { useCallback, useState } from "react";
import type { CodexProcessInfo } from "../types";
import { invokeBackend } from "../lib/platform";
import i18n from "../i18n";

interface KillCodexProcessesResult {
  targeted_count: number;
  killed_pids: number[];
  failed_pids: number[];
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
  const [confirmOpen, setConfirmOpen] = useState(false);
  const [isForceClosing, setIsForceClosing] = useState(false);

  const forceCloseCodexProcesses = useCallback(async () => {
    try {
      setIsForceClosing(true);

      const result = await invokeBackend<KillCodexProcessesResult>(
        "kill_codex_processes"
      );
      const latestProcessInfo = await checkProcesses();
      const remainingCount = latestProcessInfo?.count ?? 0;
      const closedCount = Math.max(0, processCount - remainingCount);

      if (result.targeted_count === 0) {
        showToast(i18n.t("forceClose.noneFound"));
      } else if (remainingCount === 0) {
        showToast(
          i18n.t("forceClose.closed", { count: processCount })
        );
      } else if (closedCount > 0) {
        showToast(
          i18n.t("forceClose.partial", {
            closed: closedCount,
            total: processCount,
            remaining: remainingCount,
          }),
          true
        );
      } else {
        showToast(
          i18n.t("forceClose.couldNotClose", { count: remainingCount }),
          true
        );
      }

      return latestProcessInfo;
    } catch (err) {
      console.error("Failed to force close Codex processes:", err);
      showToast(i18n.t("forceClose.failed", { error: formatError(err) }), true);
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
    forceCloseCodexProcesses,
  };
}
