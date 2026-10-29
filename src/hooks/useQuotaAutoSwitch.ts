import { useEffect, useRef, useState } from "react";
import type { AccountInfo } from "../types";
import { invokeBackend, isTauriRuntime } from "../lib/platform";
import { AUTO_SWITCH_COOLDOWN_MS, AUTO_SWITCH_INTERVAL_MS, checkQuotaAutoSwitch } from "../lib/quotaAutoSwitch";

export function useQuotaAutoSwitch(options: {
  enabled: boolean;
  blocked: () => boolean;
  switchAccount: (target: AccountInfo, sourceId: string, reopen: boolean) => Promise<void>;
}) {
  const latest = useRef(options);
  latest.current = options;
  const inFlight = useRef(false);
  const nextAttempt = useRef(0);
  const [busy, setBusy] = useState(false);
  const [status, setStatus] = useState("Monitoring quota while Codex Switcher is running.");

  useEffect(() => {
    if (!options.enabled || !isTauriRuntime()) return;
    let disposed = false;
    const check = async () => {
      if (disposed || !latest.current.enabled || inFlight.current || latest.current.blocked() || Date.now() < nextAttempt.current) return;
      inFlight.current = true;
      setBusy(true);
      setStatus("Checking account quota…");
      try {
        const result = await checkQuotaAutoSwitch({
          invoke: async <T,>(command: string, args?: Record<string, unknown>) => {
            // Only read commands use this timeout; account switching must finish.
            let timer: ReturnType<typeof setTimeout> | undefined;
            try {
              return await Promise.race([
                invokeBackend<T>(command, args),
                new Promise<never>((_, reject) => { timer = setTimeout(() => reject(new Error("Quota check timed out")), 15_000); }),
              ]);
            } finally { if (timer) clearTimeout(timer); }
          },
          cancelled: () => disposed || !latest.current.enabled || latest.current.blocked(),
          switchAccount: async (...args) => {
            setStatus("Quota exhausted. Switching accounts…");
            await latest.current.switchAccount(...args);
          },
        });
        setStatus(result.message);
        if (result.switched) nextAttempt.current = Date.now() + AUTO_SWITCH_COOLDOWN_MS;
      } catch (error) {
        setStatus(`Auto-switch paused: ${String(error)}. Retrying in five minutes.`);
        nextAttempt.current = Date.now() + AUTO_SWITCH_COOLDOWN_MS;
      } finally {
        inFlight.current = false;
        setBusy(false);
      }
    };
    void check();
    const timer = window.setInterval(() => void check(), AUTO_SWITCH_INTERVAL_MS);
    return () => { disposed = true; window.clearInterval(timer); };
  }, [options.enabled]);

  return { busy, inFlight, status };
}
