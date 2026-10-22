// Translate known product messages while preserving unknown diagnostic details.
export function localizeProductMessage(message: string, t: (source: string, params?: Record<string, string | number>) => string): string {
  const patterns: Array<[RegExp, (match: RegExpMatchArray) => string]> = [
    [/^Cannot switch accounts while (\d+) Codex process(?: is|es are) running$/, m => t("Cannot switch accounts while {count} Codex process(es) are running. Close Codex first.", { count: m[1] })],
    [/^Account not found: (.*)$/s, m => t("Account not found: {id}", { id: m[1] })],
    [/^Request failed with status (\d+)$/, m => t("Request failed with status {status}", { status: m[1] })],
  ];
  for (const [pattern, format] of patterns) { const match = message.match(pattern); if (match) return format(match); }
  return t(message);
}
