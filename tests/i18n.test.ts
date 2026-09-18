import test from "node:test";
import assert from "node:assert/strict";
import { resolveInitialLanguage, translate, translateMessage } from "../src/lib/i18n.ts";

test("i18n resolves only the supported English and Simplified Chinese locales", () => {
  assert.equal(resolveInitialLanguage("zh-CN"), "zh-CN");
  assert.equal(resolveInitialLanguage("zh-Hans"), "zh-CN");
  assert.equal(resolveInitialLanguage("zh-TW"), "en-US");
  assert.equal(resolveInitialLanguage("fr-FR"), "en-US");
  assert.equal(resolveInitialLanguage(undefined), "en-US");
});

test("i18n uses English fallback for missing translations", () => {
  assert.equal(translate("settingsTitle", "zh-CN"), "设置");
  assert.equal(translate("missing-key", "zh-CN"), "missing-key");
  assert.equal(translateMessage("missing {{count}}", { count: 3 }, "zh-CN"), "missing 3");
  assert.equal(translateMessage("warmupSentFor", { account: "demo" }, "zh-CN"), "已为 demo 发送预热请求");
});
