// Configurable date/time display preferences (#132): date pattern, 12/24-hour
// clock, and an optional IANA timezone override, stored in localStorage so
// every formatter can read them synchronously.

const DATE_TIME_PREFS_STORAGE_KEY = "codex-switcher:datetime-prefs";

export type DateFormatPreference = "system" | "dmy" | "mdy" | "ymd";
export type HourFormatPreference = "system" | "h12" | "h24";

export interface DateTimePreferences {
  dateFormat: DateFormatPreference;
  hourFormat: HourFormatPreference;
  /** IANA timezone name, or "system" to use the local timezone. */
  timeZone: string;
}

export const DEFAULT_DATE_TIME_PREFERENCES: DateTimePreferences = {
  dateFormat: "system",
  hourFormat: "system",
  timeZone: "system",
};

const DATE_FORMAT_VALUES: DateFormatPreference[] = ["system", "dmy", "mdy", "ymd"];
const HOUR_FORMAT_VALUES: HourFormatPreference[] = ["system", "h12", "h24"];

function isValidTimeZone(timeZone: string): boolean {
  try {
    new Intl.DateTimeFormat("en-US", { timeZone });
    return true;
  } catch {
    return false;
  }
}

export function sanitizeDateTimePreferences(
  prefs: Partial<DateTimePreferences> | null | undefined,
): DateTimePreferences {
  const dateFormat =
    prefs?.dateFormat && DATE_FORMAT_VALUES.includes(prefs.dateFormat)
      ? prefs.dateFormat
      : DEFAULT_DATE_TIME_PREFERENCES.dateFormat;
  const hourFormat =
    prefs?.hourFormat && HOUR_FORMAT_VALUES.includes(prefs.hourFormat)
      ? prefs.hourFormat
      : DEFAULT_DATE_TIME_PREFERENCES.hourFormat;
  const timeZone =
    prefs?.timeZone && (prefs.timeZone === "system" || isValidTimeZone(prefs.timeZone))
      ? prefs.timeZone
      : DEFAULT_DATE_TIME_PREFERENCES.timeZone;

  return { dateFormat, hourFormat, timeZone };
}

export function readDateTimePreferences(): DateTimePreferences {
  try {
    const raw = window.localStorage.getItem(DATE_TIME_PREFS_STORAGE_KEY);
    if (!raw) return DEFAULT_DATE_TIME_PREFERENCES;
    return sanitizeDateTimePreferences(JSON.parse(raw));
  } catch {
    return DEFAULT_DATE_TIME_PREFERENCES;
  }
}

export function writeDateTimePreferences(prefs: DateTimePreferences): void {
  try {
    window.localStorage.setItem(DATE_TIME_PREFS_STORAGE_KEY, JSON.stringify(prefs));
  } catch {
    // Storage unavailable (e.g. private mode); keep defaults for this session.
  }
}

// A concrete locale per date format so the pattern is honored everywhere,
// independent of the user's OS locale.
const DATE_FORMAT_LOCALES: Record<
  Exclude<DateFormatPreference, "system">,
  string
> = {
  dmy: "en-GB", // 23 Nov 2026
  mdy: "en-US", // Nov 23, 2026
  ymd: "en-CA", // 2026-11-23 (numeric month; textual months are reordered manually)
};

export interface DateTimeFormatOverrides {
  /** Skip the configured preferences and use these instead. */
  prefs?: Partial<DateTimePreferences>;
  /** Override the resolved locale (rare). */
  locale?: string;
}

function resolveFormatOptions(
  prefs: DateTimePreferences,
): { locale?: string; hour12?: boolean; timeZone?: string } {
  return {
    locale:
      prefs.dateFormat === "system"
        ? undefined
        : DATE_FORMAT_LOCALES[prefs.dateFormat],
    hour12:
      prefs.hourFormat === "system" ? undefined : prefs.hourFormat === "h12",
    timeZone: prefs.timeZone === "system" ? undefined : prefs.timeZone,
  };
}

const DATE_FIELD_TYPES = new Set(["day", "month", "year"]);

// No common locale orders the date year-first for every component style
// (en-CA falls back to month-first when the month is textual). Reorder the
// date fields manually so `ymd` is honored regardless of the caller's
// component options, keeping the surrounding parts (e.g. the time) untouched.
function formatWithYearFirst(
  date: Date,
  locale: string | undefined,
  options: Intl.DateTimeFormatOptions
): string {
  const parts = new Intl.DateTimeFormat(locale, options).formatToParts(date);
  const datePartIndexes = parts
    .map((part, index) => ({ type: part.type, index }))
    .filter(({ type }) => DATE_FIELD_TYPES.has(type));

  // Only reorder when all three date fields are rendered; a format without
  // e.g. a year has nothing to put first.
  const hasAllFields = DATE_FIELD_TYPES.size === datePartIndexes.length;
  if (!hasAllFields) {
    return new Intl.DateTimeFormat(locale, options).format(date);
  }

  const valueOf = (type: string) =>
    parts.find((part) => part.type === type)?.value ?? "";
  const separator =
    options.month === "numeric" || options.month === "2-digit" ? "-" : " ";

  const head = parts
    .slice(0, datePartIndexes[0].index)
    .map((part) => part.value)
    .join("");
  const tail = parts
    .slice(datePartIndexes[datePartIndexes.length - 1].index + 1)
    .map((part) => part.value)
    .join("");

  return `${head}${valueOf("year")}${separator}${valueOf("month")}${separator}${valueOf("day")}${tail}`;
}

/** Format a date+time using the user's configured display preferences. */
export function formatDateTime(
  date: Date,
  options: Intl.DateTimeFormatOptions & DateTimeFormatOverrides,
): string {
  const prefs = options.prefs
    ? sanitizeDateTimePreferences(options.prefs)
    : readDateTimePreferences();
  const resolved = resolveFormatOptions(prefs);

  const { prefs: _prefs, locale: overrideLocale, ...rest } = options;
  const resolvedOptions: Intl.DateTimeFormatOptions = {
    ...rest,
    ...(rest.hour12 === undefined && resolved.hour12 !== undefined
      ? { hour12: resolved.hour12 }
      : {}),
    ...(rest.timeZone === undefined && resolved.timeZone !== undefined
      ? { timeZone: resolved.timeZone }
      : {}),
  };
  const resolvedLocale = overrideLocale ?? resolved.locale;

  if (prefs.dateFormat === "ymd") {
    return formatWithYearFirst(date, resolvedLocale, resolvedOptions);
  }

  return new Intl.DateTimeFormat(resolvedLocale, resolvedOptions).format(date);
}

/** Format a date-only value using the configured preferences. */
export function formatDate(
  date: Date,
  options: Intl.DateTimeFormatOptions & DateTimeFormatOverrides = {},
): string {
  return formatDateTime(date, {
    month: "short",
    day: "numeric",
    year: "numeric",
    ...options,
  });
}

/** Format a time-of-day value using the configured preferences. */
export function formatTime(
  date: Date,
  options: Intl.DateTimeFormatOptions & DateTimeFormatOverrides = {},
): string {
  return formatDateTime(date, {
    hour: "numeric",
    minute: "2-digit",
    ...options,
  });
}
