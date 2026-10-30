/**
 * Import completion only needs the saved account plus a local account-list
 * reload. Slow/failed quota requests must not keep the import dialog open.
 */
export async function completeImportWithoutBlockingOnUsage<T>(
  importAccount: () => Promise<T>,
  reloadAccounts: () => Promise<unknown>,
  refreshUsage: (account: T) => Promise<unknown>,
  onRefreshError: (error: unknown) => void,
): Promise<T> {
  const account = await importAccount();
  await reloadAccounts();
  void Promise.resolve().then(() => refreshUsage(account)).catch(onRefreshError);
  return account;
}
