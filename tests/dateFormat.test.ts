import assert from "node:assert/strict";
import test from "node:test";
import { formatLocalizedDate } from "../src/lib/dateFormat.ts";

const value = new Date("2026-08-21T17:22:00Z");

test("absolute dates follow the selected application locale", () => {
  assert.equal(
    formatLocalizedDate(
      value,
      "en-US",
      { year: "numeric", month: "short", day: "numeric" },
      "UTC",
    ),
    "Aug 21, 2026",
  );
  assert.equal(
    formatLocalizedDate(
      value,
      "zh-CN",
      { year: "numeric", month: "short", day: "numeric" },
      "UTC",
    ),
    "2026年8月21日",
  );
});

test("date and time output does not inherit an unrelated system locale", () => {
  const english = formatLocalizedDate(
    value,
    "en-US",
    { month: "long", day: "numeric", hour: "numeric", minute: "2-digit" },
    "UTC",
  );
  const chinese = formatLocalizedDate(
    value,
    "zh-CN",
    { month: "long", day: "numeric", hour: "numeric", minute: "2-digit" },
    "UTC",
  );

  assert.match(english, /August 21/);
  assert.doesNotMatch(english, /月|日/);
  assert.match(chinese, /8月21日/);
});
