import { useCallback, useEffect, useState } from "react";
import {
  CODEX_CLOSE_PREFERENCE_STORAGE_KEY,
  parseCodexClosePreference,
  rememberedCodexClosePreference,
  type CodexClosePreference,
} from "../lib/codexClosePreference";

export function useCodexClosePreference(confirmOpen: boolean) {
  const [preference, setPreference] = useState<CodexClosePreference>(() => {
    try {
      return parseCodexClosePreference(
        window.localStorage.getItem(CODEX_CLOSE_PREFERENCE_STORAGE_KEY),
      );
    } catch {
      return "ask";
    }
  });
  const [forceClose, setForceClose] = useState(false);
  const [remember, setRemember] = useState(false);

  useEffect(() => {
    if (!confirmOpen) return;
    setForceClose(preference === "force");
    setRemember(preference !== "ask");
  }, [confirmOpen, preference]);

  const savePreference = useCallback((value: CodexClosePreference) => {
    window.localStorage.setItem(CODEX_CLOSE_PREFERENCE_STORAGE_KEY, value);
    setPreference(value);
  }, []);

  const rememberSelection = () => {
    savePreference(rememberedCodexClosePreference(forceClose, remember));
  };

  return {
    preference,
    savePreference,
    forceClose,
    setForceClose,
    remember,
    setRemember,
    rememberSelection,
  };
}
