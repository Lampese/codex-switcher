import { useState } from "react";
import { isTauriRuntime, openExternalUrl } from "../lib/platform";

interface OAuthLinkPanelProps {
  authUrl: string;
  onCopyError: (message: string) => void;
}

export function OAuthLinkPanel({ authUrl, onCopyError }: OAuthLinkPanelProps) {
  const [copied, setCopied] = useState<boolean>(false);
  const tauriRuntime = isTauriRuntime();

  return (
    <div className="text-center py-4">
      <div className="animate-spin h-8 w-8 border-2 border-gray-900 dark:border-gray-100 border-t-transparent rounded-full mx-auto mb-3"></div>
      <p className="text-gray-700 dark:text-gray-300 font-medium mb-2">Waiting for browser login...</p>
      <p className="text-xs text-gray-500 dark:text-gray-400 mb-4">
        Please open the following link in your browser to proceed:
      </p>
      <div className="flex items-center gap-2 mb-2 bg-gray-50 dark:bg-gray-800 p-2 rounded-lg border border-gray-200 dark:border-gray-700">
        <input
          type="text"
          readOnly
          value={authUrl}
          className="flex-1 bg-transparent border-none text-xs text-gray-600 dark:text-gray-300 focus:outline-none focus:ring-0 truncate"
        />
        <button
          onClick={() => {
            void navigator.clipboard
              .writeText(authUrl)
              .then(() => {
                setCopied(true);
                setTimeout(() => setCopied(false), 2000);
              })
              .catch(() => {
                onCopyError("Clipboard unavailable. Copy the link manually.");
              });
          }}
          className={`px-3 py-1.5 border rounded text-xs font-medium transition-colors shrink-0 
            ${copied
              ? "bg-green-50 dark:bg-green-900/30 border-green-200 dark:border-green-700 text-green-700 dark:text-green-300"
              : "bg-white dark:bg-gray-900 border-gray-200 dark:border-gray-700 text-gray-700 dark:text-gray-200 hover:bg-gray-50 dark:hover:bg-gray-800"
            }`}
        >
          {copied ? "Copied!" : "Copy"}
        </button>
        <button
          onClick={() => {
            void openExternalUrl(authUrl);
          }}
          className="px-3 py-1.5 bg-gray-900 hover:bg-gray-800 dark:bg-gray-100 dark:hover:bg-gray-200 border border-gray-900 dark:border-gray-100 rounded text-xs font-medium text-white dark:text-gray-900 transition-colors shrink-0"
        >
          Open
        </button>
      </div>
      {!tauriRuntime && (
        <p className="text-xs text-amber-600">
          OAuth login must finish on the same host machine because the callback
          redirects to `localhost`.
        </p>
      )}
    </div>
  );
}
