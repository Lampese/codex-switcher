import assert from "node:assert/strict";
import test from "node:test";
import { THEMES, THEME_STORAGE_KEY, applyTheme, getThemeAppearance, normalizeTheme, readStoredTheme } from "../src/lib/theme.ts";

test("seven supported themes normalize with a macOS default and legacy compatibility", () => {
  assert.equal(THEMES.length, 7);
  assert.equal(new Set(THEMES.map((theme) => theme.id)).size, 7);
  for (const theme of THEMES) {
    assert.equal(normalizeTheme(theme.id), theme.id);
    assert.equal(getThemeAppearance(theme.id), theme.dark ? "dark" : "light");
    for (const mode of ["light", "dark"] as const) {
      assert.equal(normalizeTheme(`${theme.id}:${mode}`), theme.id);
      assert.equal(getThemeAppearance(`${theme.id}:${mode}`), mode);
    }
  }
  for (const value of [null, undefined, "", "unknown", {}, 2]) assert.equal(normalizeTheme(value), "macos");
  assert.equal(normalizeTheme("light"), "macos");
  assert.equal(normalizeTheme("dark"), "nord");
});

test("stored selection and dark mode are applied consistently", () => {
  const oldWindow = Object.getOwnPropertyDescriptor(globalThis, "window");
  const oldDocument = Object.getOwnPropertyDescriptor(globalThis, "document");
  const dataset: Record<string, string> = {};
  let dark = false;
  let stored = "windows-7";
  try {
    Object.defineProperty(globalThis, "window", { configurable: true, value: { localStorage: { getItem(key: string) { assert.equal(key, THEME_STORAGE_KEY); return stored; } } } });
    Object.defineProperty(globalThis, "document", { configurable: true, value: { documentElement: { dataset, classList: { toggle(name: string, value: boolean) { assert.equal(name, "dark"); dark = value; } } } } });
    assert.equal(readStoredTheme(), "windows-7");
    for (const theme of THEMES) {
      stored = theme.id;
      applyTheme(readStoredTheme());
      assert.equal(dataset.theme, theme.id);
      assert.equal(dark, theme.dark);
      for (const mode of ["light", "dark"] as const) {
        stored = `${theme.id}:${mode}`;
        assert.equal(readStoredTheme(), stored);
        applyTheme(readStoredTheme());
        assert.equal(dataset.theme, theme.id);
        assert.equal(dataset.appearance, mode);
        assert.equal(dark, mode === "dark");
      }
    }
    stored = "windows-xp:invalid";
    assert.equal(readStoredTheme(), "windows-xp");
    stored = "dark";
    assert.equal(readStoredTheme(), "nord");
    Object.defineProperty(globalThis, "window", { configurable: true, value: { get localStorage() { throw new Error("unavailable"); } } });
    assert.equal(readStoredTheme(), "macos");
  } finally {
    if (oldWindow) Object.defineProperty(globalThis, "window", oldWindow); else Reflect.deleteProperty(globalThis, "window");
    if (oldDocument) Object.defineProperty(globalThis, "document", oldDocument); else Reflect.deleteProperty(globalThis, "document");
  }
});
