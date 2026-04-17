import { useState, useEffect, useCallback } from "react";
import type { AutoSwitchConfig, AutoSwitchEvent } from "../types";
import { invokeBackend } from "../lib/platform";

const DEFAULT_CONFIG: AutoSwitchConfig = {
  enabled: false,
  threshold_percent: 95,
  check_interval_seconds: 60,
  respect_weekly_limit: true,
  excluded_account_ids: [],
  priority_order: [],
};

export function useAutoSwitch() {
  const [config, setConfig] = useState<AutoSwitchConfig>(DEFAULT_CONFIG);
  const [isRunning, setIsRunning] = useState(false);
  const [events, setEvents] = useState<AutoSwitchEvent[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // Load config on mount
  useEffect(() => {
    loadConfig();
    loadStatus();
    loadEvents();
  }, []);

  const loadConfig = useCallback(async () => {
    try {
      const cfg = await invokeBackend<AutoSwitchConfig>("get_auto_switch_config");
      setConfig(cfg ?? DEFAULT_CONFIG);
    } catch (err) {
      console.error("Failed to load auto-switch config:", err);
    }
  }, []);

  const loadStatus = useCallback(async () => {
    try {
      const running = await invokeBackend<boolean>("auto_switch_status");
      setIsRunning(running);
    } catch (err) {
      console.error("Failed to get auto-switch status:", err);
    }
  }, []);

  const loadEvents = useCallback(async () => {
    try {
      const evts = await invokeBackend<AutoSwitchEvent[]>("get_auto_switch_events");
      setEvents(evts ?? []);
    } catch (err) {
      console.error("Failed to load auto-switch events:", err);
    }
  }, []);

  const saveConfig = useCallback(
    async (newConfig: Partial<AutoSwitchConfig>) => {
      try {
        setLoading(true);
        setError(null);
        const merged = { ...config, ...newConfig };
        await invokeBackend("set_auto_switch_config", { config: merged });
        setConfig(merged);
        // Refresh status after config change
        const running = await invokeBackend<boolean>("auto_switch_status");
        setIsRunning(running);
      } catch (err) {
        const message = err instanceof Error ? err.message : String(err);
        setError(message);
        throw err;
      } finally {
        setLoading(false);
      }
    },
    [config]
  );

  const start = useCallback(async () => {
    try {
      setLoading(true);
      setError(null);
      await invokeBackend("start_auto_switch");
      setIsRunning(true);
      setConfig((prev) => ({ ...prev, enabled: true }));
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      setError(message);
      throw err;
    } finally {
      setLoading(false);
    }
  }, []);

  const stop = useCallback(async () => {
    try {
      setLoading(true);
      setError(null);
      await invokeBackend("stop_auto_switch");
      setIsRunning(false);
      setConfig((prev) => ({ ...prev, enabled: false }));
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      setError(message);
      throw err;
    } finally {
      setLoading(false);
    }
  }, []);

  const clearEvents = useCallback(async () => {
    try {
      await invokeBackend("clear_auto_switch_events");
      setEvents([]);
    } catch (err) {
      console.error("Failed to clear auto-switch events:", err);
    }
  }, []);

  // Poll for events when running
  useEffect(() => {
    if (!isRunning) return;

    const interval = setInterval(() => {
      loadEvents().catch(() => {});
    }, 5000);

    return () => clearInterval(interval);
  }, [isRunning, loadEvents]);

  return {
    config,
    isRunning,
    events,
    loading,
    error,
    loadConfig,
    saveConfig,
    start,
    stop,
    clearEvents,
    loadEvents,
  };
}