/** A failed access-token request does not prove that the rotating login token was revoked. */
export function needsRelogin(error: string | null | undefined): boolean {
  if (!error) return false;
  return /ACCOUNT_RELOGIN_REQUIRED:|refresh_token_(?:reused|expired|invalidated)|invalid_grant|token_invalidated|authentication token.*(?:expired|invalid|revoked)|saved session is out of date|outdated refresh token|invalid refresh token|refresh token is invalid|missing refresh token|did not return a fresh id_token|invalid or expired id_token|your session has ended/i.test(error);
}

export function isBackgroundAuthBusy(error: string | null | undefined): boolean {
  return !!error && /Background OAuth refresh busy/i.test(error);
}

/** Only explicit credential invalidation should tell users to sign in again. */
export function formatUsageError(error: string): { title: string; message: string } {
  if (isBackgroundAuthBusy(error)) {
    return {
      title: "Authentication check postponed",
      message: "Another OAuth operation is running. Retry shortly; this does not mean the account is signed out.",
    };
  }
  if (needsRelogin(error)) {
    return {
      title: "Switcher sign-in needs renewal",
      message: "Switcher cannot renew this saved login. Re-login to this account in Switcher; other apps may still be signed in.",
    };
  }
  if (/failed to parse usage response/i.test(error)) {
    return {
      title: "Usage format changed",
      message: "The account may still be signed in, but Switcher could not read the usage response.",
    };
  }
  if (/could not refresh the saved codex switcher session|failed to send token refresh request|\b401\b|\b429\b/i.test(error)) {
    return {
      title: "Authentication check unsuccessful",
      message: "This request failed without proving that the saved login is invalid. Retry before re-authenticating.",
    };
  }
  return { title: "Usage unavailable", message: error };
}
