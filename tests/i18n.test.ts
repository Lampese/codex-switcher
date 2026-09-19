import test from "node:test";
import assert from "node:assert/strict";
import enUS from "../src/locales/en-US.json" with { type: "json" };
import zhCN from "../src/locales/zh-CN.json" with { type: "json" };
import {
  resolveInitialLanguage,
  resolveBrowserLanguagePreference,
  resolvePreferredLanguage,
  resolveSupportedLocale,
  translate,
  translateMessage,
} from "../src/lib/i18n.ts";

test("i18n resolves only supported English and Simplified Chinese locales", () => {
  assert.equal(resolveSupportedLocale("zh-CN"), "zh-CN");
  assert.equal(resolveSupportedLocale("zh_CN"), "zh-CN");
  assert.equal(resolveSupportedLocale("zh-Hans"), "zh-CN");
  assert.equal(resolveSupportedLocale("zh-Hans-CN"), "zh-CN");
  assert.equal(resolveSupportedLocale("zh-SG"), "zh-CN");
  assert.equal(resolveSupportedLocale("zh-MO"), null);
  assert.equal(resolveSupportedLocale("zh"), null);
  assert.equal(resolveSupportedLocale("en"), "en-US");
  assert.equal(resolveSupportedLocale("en-GB"), "en-US");

  assert.equal(resolveSupportedLocale("zh-TW"), null);
  assert.equal(resolveSupportedLocale("zh-HK"), null);
  assert.equal(resolveSupportedLocale("zh-Hant"), null);
  assert.equal(resolveSupportedLocale("fr-FR"), null);
  assert.equal(resolveSupportedLocale(undefined), null);

  assert.equal(resolveInitialLanguage("zh-TW"), "en-US");
});

test("browser default uses the first supported browser preference", () => {
  assert.equal(resolvePreferredLanguage(["fr-FR", "zh-CN", "en-US"]), "zh-CN");
  assert.equal(resolvePreferredLanguage(["fr-FR", "en-GB", "zh-CN"]), "en-US");
  assert.equal(resolvePreferredLanguage(["fr-FR", "de-DE"]), "en-US");
});

test("browser preference stays local and explicit choices override browser order", () => {
  assert.equal(
    resolveBrowserLanguagePreference("browser", ["fr-FR", "zh-Hans"]),
    "zh-CN"
  );
  assert.equal(
    resolveBrowserLanguagePreference("en-US", ["zh-CN", "zh-Hans"]),
    "en-US"
  );
  assert.equal(
    resolveBrowserLanguagePreference("zh-CN", ["en-US", "fr-FR"]),
    "zh-CN"
  );
});

test("English and Simplified Chinese catalogs stay in key parity", () => {
  assert.deepEqual(Object.keys(zhCN).sort(), Object.keys(enUS).sort());
});

test("i18n falls back safely and interpolates dynamic values", () => {
  assert.equal(translate("settings.settings.title", "zh-CN"), "设置");
  assert.equal(translate("missing-key", "zh-CN"), "missing-key");
  assert.equal(
    translateMessage("app.warmup.sent.for", { account: "demo" }, "zh-CN"),
    "已为 demo 发送预热请求"
  );
  assert.equal(
    translateMessage("close.processes.success", { action: "Closed", count: 1 }, "en-US"),
    "Closed 1 Codex session."
  );
  assert.equal(
    translateMessage("close.processes.success", { action: "Closed", count: 2 }, "en-US"),
    "Closed 2 Codex sessions."
  );
  assert.equal(
    translateMessage("warmup.all.sent", { count: 1 }, "en-US"),
    "Warm-up sent for 1 account"
  );
  assert.equal(
    translateMessage("app.switch.failed.error", { error: "raw-provider-error/account@example.com" }, "zh-CN"),
    "切换失败：raw-provider-error/account@example.com"
  );
});
