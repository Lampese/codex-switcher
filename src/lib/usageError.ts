/** An access-token or usage error does not by itself prove a revoked saved login. */
export function needsRelogin(error: string | null | undefined): boolean {
  if (!error) return false;
  return /refresh_token_(?:reused|expired)|\binvalid_grant\b|missing refresh token|did not return a fresh id_token/i.test(error);
}

/** Use only messages emitted by the upstream token refresh and usage backends. */
export function formatUsageError(error: string): { title: string; message: string } {
  if (needsRelogin(error)) {
    return {
      title: "Switcher sign-in needs renewal",
      message: "Switcher cannot renew this saved login. Remove this account and add it again in Switcher; other apps may still be signed in.",
    };
  }
  if (/\b429\b|too many requests/i.test(error)) {
    return {
      title: "Rate limited, try again later",
      message: "The usage service is limiting requests. Your saved login is not necessarily invalid.",
    };
  }
  if (/failed to parse usage response/i.test(error)) {
    return {
      title: "Usage format changed",
      message: "The account may still be signed in, but Switcher could not read the usage response.",
    };
  }
  if (/failed to send token refresh request|\b401\b/i.test(error)) {
    return {
      title: "Usage check unsuccessful",
      message: "This request failed without proving that the saved login is invalid. Retry before re-authenticating.",
    };
  }
  return { title: "Usage unavailable", message: error };
}
