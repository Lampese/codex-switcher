export type AppLocale = "en-US" | "zh-CN";

export function formatLocalizedDate(
  value: Date | string | number,
  locale: AppLocale,
  options: Intl.DateTimeFormatOptions,
  timeZone?: string,
): string {
  const date = value instanceof Date ? value : new Date(value);
  if (Number.isNaN(date.getTime())) return "";

  return new Intl.DateTimeFormat(locale, {
    ...options,
    ...(timeZone ? { timeZone } : {}),
  }).format(date);
}
