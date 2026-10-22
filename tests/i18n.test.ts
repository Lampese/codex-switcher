import assert from "node:assert/strict";
import test from "node:test";
import { normalizeLanguage, translate } from "../src/lib/i18nLanguage.ts";
import { localizeProductMessage } from "../src/lib/i18nMessages.ts";
import { shellTranslations } from "../src/lib/i18nShell.ts";
import { componentTranslations } from "../src/lib/i18nComponents.ts";

const dictionary = { ...shellTranslations, ...componentTranslations };

test("English is the default and Turkish requires an explicit selection", () => {
  for (const value of [null, undefined, "", "en", "tr-TR", "de", {}, 1]) {
    assert.equal(normalizeLanguage(value), "en");
  }
  assert.equal(normalizeLanguage("tr"), "tr");
  assert.equal(translate("Settings", "en", dictionary), "Settings");
  assert.equal(translate("Settings", "tr", dictionary), "Ayarlar");
});

test("interpolation preserves values and unknown placeholders in either language", () => {
  const messages = { "Hello {name}: {count} {missing}": "Merhaba {name}: {count} {missing}" };
  const params = { name: "$& <b>{count}</b>", count: 0 };
  assert.equal(translate("Hello {name}: {count} {missing}", "tr", messages, params), "Merhaba $& <b>{count}</b>: 0 {missing}");
  assert.equal(translate("Hello {name}: {count} {missing}", "en", messages, params), "Hello $& <b>{count}</b>: 0 {missing}");
  for (const diagnostic of ["Network error: ECONNREFUSED", "toString", "__proto__"]) {
    assert.equal(translate(diagnostic, "tr", dictionary), diagnostic);
  }
});

test("Turkish catalog entries retain every interpolation placeholder", () => {
  const placeholders = (text: string) => [...new Set(text.match(/\{\w+\}/g) ?? [])].sort();
  for (const [english, turkish] of Object.entries(dictionary)) {
    assert.ok(turkish.trim(), english);
    assert.deepEqual(placeholders(turkish), placeholders(english), english);
  }
});

test("known backend messages translate while diagnostic details stay intact", () => {
  const t = (source: string, params?: Record<string, string | number>) => translate(source, "tr", dictionary, params);
  assert.equal(localizeProductMessage("Account not found: account-123", t), "Hesap bulunamadı: account-123");
  assert.equal(localizeProductMessage("Request failed with status 503", t), "İstek başarısız (durum: 503)");
  assert.match(localizeProductMessage("Cannot switch accounts while 2 Codex processes are running", t), /^2 Codex işlemi/);
  assert.equal(localizeProductMessage("HTTP 500: upstream diagnostic", t), "HTTP 500: upstream diagnostic");
});
