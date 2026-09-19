export function normalizeBackupPassphrase(value: string | null | undefined): string | null {
  if (value === null || value === undefined || value.trim().length === 0) {
    return null;
  }
  return value;
}

export function isPassphraseRequiredError(error: unknown): boolean {
  const message = error instanceof Error ? error.message : String(error);
  return /passphrase is required/i.test(message);
}
