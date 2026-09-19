import { getCurrentWindow, type Window as TauriWindow } from "@tauri-apps/api/window";

let cachedWindow: TauriWindow | null = null;

/**
 * Resolve the Tauri window only inside the Tauri runtime. Browser/LAN mode
 * must be able to evaluate the React bundle without touching Tauri internals.
 */
export function getTauriWindow(): TauriWindow | null {
  if (typeof window === "undefined" || !("__TAURI_INTERNALS__" in window)) {
    return null;
  }
  return (cachedWindow ??= getCurrentWindow());
}
