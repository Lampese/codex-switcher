export function needsRelogin(error: string | null | undefined): boolean {
  return !!error && /\b401\b|\bunauthorized\b|invalid_grant|invalid_token|token_(?:expired|invalidated|revoked)|refresh_token_(?:reused|expired|invalidated)|(?:refresh|authentication) token[^\n]*(?:expired|invalid|revoked|reused)|missing refresh token/i.test(error);
}
