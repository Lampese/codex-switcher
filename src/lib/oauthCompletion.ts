/**
 * A successful OAuth callback must not leave the login modal waiting for
 * unrelated usage API requests. The account is persisted by `complete`;
 * reload the local account list, then refresh only that account in the
 * background. Usage failures must not turn a successful login into a failure.
 */
export async function completeOAuthWithoutBlockingOnUsage<T>(
  complete: () => Promise<T>,
  reloadAccounts: () => Promise<unknown>,
  refreshUsage: (account: T) => Promise<unknown>,
  onRefreshError: (error: unknown) => void,
): Promise<T> {
  const account = await complete();
  await reloadAccounts();
  void Promise.resolve().then(() => refreshUsage(account)).catch(onRefreshError);
  return account;
}
