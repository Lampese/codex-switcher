/** Validate, normalize, dedupe and sort a list of "HH:MM" times. */
export function normalizeTimedWarmupTimes(times: readonly string[]): string[] {
  const valid = new Set<string>();
  for (const raw of times) {
    const match = /^(\d{1,2}):(\d{1,2})$/.exec(String(raw).trim());
    if (!match) continue;
    const hours = Number(match[1]);
    const minutes = Number(match[2]);
    if (hours < 0 || hours > 23 || minutes < 0 || minutes > 59) continue;
    valid.add(
      `${String(hours).padStart(2, "0")}:${String(minutes).padStart(2, "0")}`
    );
  }
  return Array.from(valid).sort();
}
