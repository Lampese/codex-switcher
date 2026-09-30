import { useCallback, useEffect, useState } from "react";
import { invokeBackend, isTauriRuntime } from "../lib/platform";
import {
  DESKTOP_REOPEN_STORAGE_KEY,
  parseDesktopReopenPreference,
  type DesktopReopenPreference,
} from "../lib/desktopReopen";

interface DesktopReopenInfo {
  supported: boolean;
  desktop_count: number;
}

export function useDesktopReopen(confirmOpen: boolean) {
  const [preference, setPreference] = useState<DesktopReopenPreference>(() => {
    try {
      return parseDesktopReopenPreference(window.localStorage.getItem(DESKTOP_REOPEN_STORAGE_KEY));
    } catch {
      return "ask";
    }
  });
  const [reopen, setReopen] = useState(false);
  const [remember, setRemember] = useState(false);
  const [info, setInfo] = useState<DesktopReopenInfo | null>(null);
  const [checking, setChecking] = useState(false);

  useEffect(() => {
    if (!confirmOpen) return;
    let cancelled = false;
    setReopen(preference === "always");
    setRemember(preference !== "ask");
    setInfo(null);
    if (!isTauriRuntime()) return;
    setChecking(true);
    void invokeBackend<DesktopReopenInfo>("get_codex_reopen_info")
      .then((result) => { if (!cancelled) setInfo(result); })
      .catch(() => { if (!cancelled) setInfo(null); })
      .finally(() => { if (!cancelled) setChecking(false); });
    return () => { cancelled = true; };
  }, [confirmOpen, preference]);

  const savePreference = useCallback((value: DesktopReopenPreference) => {
    // Let the caller report storage failures rather than claiming it was saved.
    window.localStorage.setItem(DESKTOP_REOPEN_STORAGE_KEY, value);
    setPreference(value);
  }, []);

  const available = info?.supported === true && info.desktop_count > 0;
  const rememberSelection = () => {
    if (available && preference === "ask") {
      savePreference(remember ? (reopen ? "always" : "never") : "ask");
    }
  };

  return {
    preference, savePreference, reopen, setReopen, remember, setRemember,
    available, checking, rememberSelection,
  };
}
