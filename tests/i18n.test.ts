import test from "node:test";
import assert from "node:assert/strict";
import enUS from "../src/locales/en-US.json" with { type: "json" };
import zhCN from "../src/locales/zh-CN.json" with { type: "json" };
import {
  resolveInitialLanguage,
  resolvePreferredLanguage,
  resolveSupportedLocale,
  translate,
  translateMessage,
} from "../src/lib/i18n.ts";

test("i18n resolves only supported English and Simplified Chinese locales", () => {
  assert.equal(resolveSupportedLocale("zh-CN"), "zh-CN");
  assert.equal(resolveSupportedLocale("zh-Hans"), "zh-CN");
  assert.equal(resolveSupportedLocale("zh-Hans-CN"), "zh-CN");
  assert.equal(resolveSupportedLocale("zh-SG"), "zh-CN");
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
});
