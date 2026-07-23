import { useState } from "react";
import { isTauriRuntime, openExternalUrl } from "../lib/platform";
import { invokeBackend } from "../lib/platform";
import type { AccountInfo } from "../types";

interface ReAuthModalProps {
  account: AccountInfo;
  onClose: () => void;
  onSuccess: (updated: AccountInfo) => void;
}

export function ReAuthModal({ account, onClose, onSuccess }: ReAuthModalProps) {
  const [loading, setLoading] = useState(false);
  const [oauthPending, setOauthPending] = useState(false);
  const [authUrl, setAuthUrl] = useState("");
  const [copied, setCopied] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const tauriRuntime = isTauriRuntime();

  const handleClose = () => {
    if (oauthPending) {
      void invokeBackend("cancel_login").catch(() => {});
    }
    onClose();
  };

  const handleStartReAuth = async () => {
    try {
      setLoading(true);
      setError(null);
      const info = await invokeBackend<{ auth_url: string }>("start_reauth", {
        accountId: account.id,
      });
      setAuthUrl(info.auth_url);
      setOauthPending(true);
      setLoading(false);

      // Wait for the browser callback to complete.
      const updated = await invokeBackend<AccountInfo>("complete_reauth", {
        accountId: account.id,
      });
      onSuccess(updated);
      onClose();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
      setLoading(false);
      setOauthPending(false);
    }
  };

  return (
    <div className="fixed inset-0 bg-black/40 flex items-center justify-center z-50">
      <div className="bg-white dark:bg-gray-900 border border-gray-200 dark:border-gray-700 rounded-2xl w-full max-w-md mx-4 shadow-xl">
        {/* Header */}
        <div className="flex items-center justify-between p-5 border-b border-gray-100 dark:border-gray-800">
          <div>
            <h2 className="text-lg font-semibold text-gray-900 dark:text-gray-100">
              Re-authenticate Account
            </h2>
            <p className="text-sm text-gray-500 dark:text-gray-400 mt-0.5">
              {account.name}
              {account.email ? ` · ${account.email}` : ""}
            </p>
          </div>
          <button
            onClick={handleClose}
            className="text-gray-400 hover:text-gray-600 dark:hover:text-gray-300 transition-colors"
          >
            ✕
          </button>
        </div>

        {/* Content */}
        <div className="p-5 space-y-4">
          {/* Expiry warning */}
          <div className="flex items-start gap-3 p-3 rounded-lg bg-amber-50 dark:bg-amber-900/20 border border-amber-200 dark:border-amber-700">
            <span className="text-amber-500 text-lg leading-none mt-0.5">⚠</span>
            <p className="text-sm text-amber-800 dark:text-amber-200">
              This account's session has expired. Sign in again to refresh the
              credentials — your account settings and history will be preserved.
            </p>
          </div>

          {oauthPending ? (
            <div className="text-center py-2">
              <div className="animate-spin h-8 w-8 border-2 border-gray-900 dark:border-gray-100 border-t-transparent rounded-full mx-auto mb-3" />
              <p className="text-gray-700 dark:text-gray-300 font-medium mb-2">
                Waiting for browser login...
              </p>
              <p className="text-xs text-gray-500 dark:text-gray-400 mb-4">
                Open the link below in your browser to complete authentication:
              </p>
              <div className="flex items-center gap-2 bg-gray-50 dark:bg-gray-800 p-2 rounded-lg border border-gray-200 dark:border-gray-700">
                <input
                  type="text"
                  readOnly
                  value={authUrl}
                  className="flex-1 bg-transparent border-none text-xs text-gray-600 dark:text-gray-300 focus:outline-none focus:ring-0 truncate"
                />
                <button
                  onClick={() => {
                    void navigator.clipboard.writeText(authUrl).then(() => {
                      setCopied(true);
                      setTimeout(() => setCopied(false), 2000);
                    });
                  }}
                  className={`px-3 py-1.5 border rounded text-xs font-medium transition-colors shrink-0 ${
                    copied
                      ? "bg-green-50 dark:bg-green-900/30 border-green-200 dark:border-green-700 text-green-700 dark:text-green-300"
                      : "bg-white dark:bg-gray-900 border-gray-200 dark:border-gray-700 text-gray-700 dark:text-gray-200 hover:bg-gray-50 dark:hover:bg-gray-800"
                  }`}
                >
                  {copied ? "Copied!" : "Copy"}
                </button>
                <button
                  onClick={() => void openExternalUrl(authUrl)}
                  className="px-3 py-1.5 bg-gray-900 hover:bg-gray-800 dark:bg-gray-100 dark:hover:bg-gray-200 border border-gray-900 dark:border-gray-100 rounded text-xs font-medium text-white dark:text-gray-900 transition-colors shrink-0"
                >
                  Open
                </button>
              </div>
              {!tauriRuntime && (
                <p className="text-xs text-amber-600 mt-2">
                  OAuth login must finish on the same host machine because the
                  callback redirects to localhost.
                </p>
              )}
            </div>
          ) : (
            <p className="text-sm text-gray-600 dark:text-gray-300">
              Click the button below to generate a login link. You will be asked
              to sign in with ChatGPT in your browser.
            </p>
          )}

          {error && (
            <div className="p-3 bg-red-50 dark:bg-red-900/20 border border-red-200 dark:border-red-700 rounded-lg text-red-600 dark:text-red-300 text-sm">
              {error}
            </div>
          )}
        </div>

        {/* Footer */}
        <div className="flex gap-3 p-5 border-t border-gray-100 dark:border-gray-800">
          <button
            onClick={handleClose}
            className="flex-1 px-4 py-2.5 text-sm font-medium rounded-lg bg-gray-100 hover:bg-gray-200 dark:bg-gray-800 dark:hover:bg-gray-700 text-gray-700 dark:text-gray-200 transition-colors"
          >
            Cancel
          </button>
          {!oauthPending && (
            <button
              onClick={() => void handleStartReAuth()}
              disabled={loading}
              className="flex-1 px-4 py-2.5 text-sm font-medium rounded-lg bg-gray-900 hover:bg-gray-800 dark:bg-gray-100 dark:hover:bg-gray-200 text-white dark:text-gray-900 transition-colors disabled:opacity-50"
            >
              {loading ? "Starting..." : "Generate Login Link"}
            </button>
          )}
        </div>
      </div>
    </div>
  );
}
