export function normalizeBackupPassphrase(value: string | null | undefined): string | null {
  const normalized = value?.trim() ?? "";
  return normalized || null;
}

export function isPassphraseRequiredError(error: unknown): boolean {
  const message = error instanceof Error ? error.message : String(error);
  return /passphrase is required/i.test(message);
}
