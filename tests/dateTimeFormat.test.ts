import assert from "node:assert/strict";
import test from "node:test";
import {
  DEFAULT_DATE_TIME_PREFERENCES,
  formatDate,
  formatDateTime,
  formatTime,
  readDateTimePreferences,
  sanitizeDateTimePreferences,
  writeDateTimePreferences,
} from "../src/lib/dateTimeFormat.ts";

const INSTANT = new Date("2026-07-28T15:30:00Z");

function withWindow(localStorage: Record<string, string>, fn: () => void): void {
  const store = {
    getItem: (k: string) => (k in localStorage ? localStorage[k] : null),
    setItem: (k: string, v: string) => {
      localStorage[k] = v;
    },
  };
  (globalThis as { window?: unknown }).window = { localStorage: store };
  try {
    fn();
  } finally {
    delete (globalThis as { window?: unknown }).window;
  }
}

test("sanitizeDateTimePreferences falls back to defaults on bad values", () => {
  assert.deepEqual(sanitizeDateTimePreferences(null), DEFAULT_DATE_TIME_PREFERENCES);
  assert.deepEqual(
    sanitizeDateTimePreferences({ dateFormat: "bogus" as never, hourFormat: "h25" as never, timeZone: "Not/AZone" }),
    DEFAULT_DATE_TIME_PREFERENCES,
  );
  assert.deepEqual(
    sanitizeDateTimePreferences({ dateFormat: "ymd", hourFormat: "h24", timeZone: "Asia/Tokyo" }),
    { dateFormat: "ymd", hourFormat: "h24", timeZone: "Asia/Tokyo" },
  );
});

test("readDateTimePreferences returns defaults without window/localStorage", () => {
  assert.deepEqual(readDateTimePreferences(), DEFAULT_DATE_TIME_PREFERENCES);
});

test("preferences round-trip through localStorage and survive corrupt JSON", () => {
  const backing: Record<string, string> = {};
  withWindow(backing, () => {
    writeDateTimePreferences({ dateFormat: "dmy", hourFormat: "h24", timeZone: "UTC" });
    assert.deepEqual(readDateTimePreferences(), {
      dateFormat: "dmy",
      hourFormat: "h24",
      timeZone: "UTC",
    });
  });
  withWindow({ "codex-switcher:datetime-prefs": "{not json" }, () => {
    assert.deepEqual(readDateTimePreferences(), DEFAULT_DATE_TIME_PREFERENCES);
  });
});

test("date format preference selects day/month ordering", () => {
  const opts = { month: "short", day: "numeric", year: "numeric", timeZone: "UTC" } as const;
  assert.equal(
    formatDate(INSTANT, { ...opts, prefs: { dateFormat: "dmy" } }),
    "28 Jul 2026",
  );
  assert.equal(
    formatDate(INSTANT, { ...opts, prefs: { dateFormat: "mdy" } }),
    "Jul 28, 2026",
  );
  assert.equal(
    formatDate(INSTANT, { ...opts, prefs: { dateFormat: "ymd" } }),
    "2026 Jul 28",
  );
  assert.equal(
    formatDateTime(INSTANT, {
      year: "numeric",
      month: "2-digit",
      day: "2-digit",
      prefs: { dateFormat: "ymd" },
    }),
    "2026-07-28",
  );
});

test("hour format preference switches 12/24-hour clock", () => {
  const base = { hour: "numeric", minute: "2-digit", timeZone: "UTC" } as const;
  assert.equal(formatTime(INSTANT, { ...base, prefs: { hourFormat: "h12" }, locale: "en-US" }), "3:30 PM");
  assert.equal(formatTime(INSTANT, { ...base, prefs: { hourFormat: "h24" }, locale: "en-US" }), "15:30");
});

test("explicit options always win over stored preferences", () => {
  const out = formatDateTime(INSTANT, {
    month: "short",
    day: "numeric",
    hour: "numeric",
    minute: "2-digit",
    hour12: false,
    timeZone: "America/New_York",
    locale: "en-US",
    prefs: { dateFormat: "ymd", hourFormat: "h12", timeZone: "Asia/Tokyo" },
  });
  // New York is UTC-4 in July -> 11:30 on Jul 28, regardless of the stored Tokyo/h12 prefs.
  assert.equal(out, "Jul 28, 11:30");
});
