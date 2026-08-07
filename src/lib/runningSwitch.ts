export type SwitchAndReopenResult =
  | { status: "success" }
  | { status: "switch_failed"; error: unknown }
  | { status: "reopen_failed"; error: unknown };

interface SwitchAndReopenActions {
  switchAccount: () => Promise<void>;
  reopenCodex: () => Promise<void>;
}

/**
 * Complete the destructive half of a running-account switch after the caller
 * has confirmed that every blocking Codex process has stopped.
 *
 * Reopening is deliberately sequenced after a successful account switch so a
 * newly launched Codex instance cannot reload the previous credentials.
 */
export async function switchAndReopenCodex({
  switchAccount,
  reopenCodex,
}: SwitchAndReopenActions): Promise<SwitchAndReopenResult> {
  try {
    await switchAccount();
  } catch (error) {
    return { status: "switch_failed", error };
  }

  try {
    await reopenCodex();
  } catch (error) {
    return { status: "reopen_failed", error };
  }

  return { status: "success" };
}
