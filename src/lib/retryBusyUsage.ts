/**
 * The backend's "busy" error is raised before a refresh POST begins.
 * Retry the usage read a bounded number of times, never the OAuth POST itself.
 */
export async function retryBusyUsage<T>(
  request: () => Promise<T>,
  pause: (ms: number) => Promise<void> = (ms) =>
    new Promise((resolve) => setTimeout(resolve, ms)),
): Promise<T> {
  for (let attempt = 0; ; attempt++) {
    try {
      return await request();
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      if (!/Background OAuth refresh busy/i.test(message) || attempt >= 2) {
        throw error;
      }
      await pause((attempt + 1) * 300);
    }
  }
}
