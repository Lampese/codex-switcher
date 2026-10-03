// Matches SIGN_IN_EXPIRED_PREFIX in src-tauri/src/auth/token_refresh.rs.
const SIGN_IN_EXPIRED_PREFIX = "Sign-in expired: ";

export function isSignInExpiredError(error: unknown): boolean {
  const message = error instanceof Error ? error.message : typeof error === "string" ? error : "";
  return message.startsWith(SIGN_IN_EXPIRED_PREFIX);
}
