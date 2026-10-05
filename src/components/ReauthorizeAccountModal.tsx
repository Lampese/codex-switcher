import { useState } from "react";
import { OAuthLinkPanel } from "./OAuthLinkPanel";

interface ReauthorizeAccountModalProps {
  accountLabel: string;
  onClose: () => void;
  onStartOAuth: () => Promise<{ auth_url: string }>;
  onCompleteOAuth: () => Promise<void>;
  onCancelOAuth: () => Promise<void>;
}

export function ReauthorizeAccountModal({
  accountLabel,
  onClose,
  onStartOAuth,
  onCompleteOAuth,
  onCancelOAuth,
}: ReauthorizeAccountModalProps) {
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [oauthPending, setOauthPending] = useState(false);
  const [authUrl, setAuthUrl] = useState<string>("");

  const handleClose = () => {
    if (oauthPending) {
      void onCancelOAuth();
    }
    onClose();
  };

  const handleSignIn = async () => {
    try {
      setLoading(true);
      setError(null);
      const info = await onStartOAuth();
      setAuthUrl(info.auth_url);
      setOauthPending(true);
      setLoading(false);

      await onCompleteOAuth();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
      setLoading(false);
      setOauthPending(false);
    }
  };

  return (
    <div className="fixed inset-0 bg-black/40 flex items-center justify-center z-50">
      <div className="bg-white dark:bg-gray-900 border border-gray-200 dark:border-gray-700 rounded-2xl w-full max-w-md mx-4 shadow-xl">
        <div className="p-5 border-b border-gray-100 dark:border-gray-800">
          <h2 className="text-lg font-semibold text-gray-900 dark:text-gray-100">Sign in again</h2>
        </div>

        <div className="p-5 space-y-4 text-sm text-gray-600 dark:text-gray-300">
          {oauthPending ? (
            <OAuthLinkPanel authUrl={authUrl} onCopyError={setError} />
          ) : (
            <p>
              The sign-in for{" "}
              <span className="font-medium text-gray-900 dark:text-gray-100">{accountLabel}</span>{" "}
              has expired. Sign in to the same ChatGPT account and workspace in your browser, and
              Codex Switcher will switch to it.
            </p>
          )}

          {error && (
            <div className="p-3 bg-red-50 dark:bg-red-900/20 border border-red-200 dark:border-red-700 rounded-lg text-red-600 dark:text-red-300 text-sm">
              {error}
            </div>
          )}
        </div>

        <div className="flex gap-3 p-5 border-t border-gray-100 dark:border-gray-800">
          <button
            onClick={handleClose}
            className="flex-1 px-4 py-2.5 text-sm font-medium rounded-lg bg-gray-100 hover:bg-gray-200 dark:bg-gray-800 dark:hover:bg-gray-700 text-gray-700 dark:text-gray-200 transition-colors"
          >
            Cancel
          </button>
          <button
            onClick={handleSignIn}
            disabled={loading || oauthPending}
            className="flex-1 px-4 py-2.5 text-sm font-medium rounded-lg bg-gray-900 hover:bg-gray-800 dark:bg-gray-100 dark:hover:bg-gray-200 text-white dark:text-gray-900 transition-colors disabled:opacity-50"
          >
            {loading ? "Starting..." : "Generate Login Link"}
          </button>
        </div>
      </div>
    </div>
  );
}
