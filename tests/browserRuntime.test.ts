import assert from "node:assert/strict";
import test from "node:test";
import { getTauriWindow } from "../src/lib/tauriWindow.ts";

test("browser runtime does not resolve a Tauri window", () => {
  assert.equal(getTauriWindow(), null);
});

test("a plain browser-like window without Tauri internals remains safe", () => {
  const previous = Object.getOwnPropertyDescriptor(globalThis, "window");
  Object.defineProperty(globalThis, "window", {
    configurable: true,
    value: {},
  });
  try {
    assert.equal(getTauriWindow(), null);
  } finally {
    if (previous) Object.defineProperty(globalThis, "window", previous);
    else delete (globalThis as { window?: unknown }).window;
  }
});
