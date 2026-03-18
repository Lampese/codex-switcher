import { useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { open } from "@tauri-apps/plugin-dialog";

interface AddAccountModalProps {
  isOpen: boolean;
  onClose: () => void;
  onImportFile: (path: string, name: string) => Promise<void>;
  onStartOAuth: (name: string) => Promise<{ auth_url: string }>;
  onCompleteOAuth: () => Promise<unknown>;
  onCancelOAuth: () => Promise<void>;
}

type Tab = "oauth" | "import";

export function AddAccountModal({
  isOpen,
  onClose,
  onImportFile,
  onStartOAuth,
  onCompleteOAuth,
  onCancelOAuth,
}: AddAccountModalProps) {
  const [activeTab, setActiveTab] = useState<Tab>("oauth");
  const [name, setName] = useState("");
  const [filePath, setFilePath] = useState("");
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [oauthPending, setOauthPending] = useState(false);
  const [authUrl, setAuthUrl] = useState("");
  const [copied, setCopied] = useState(false);
  const isPrimaryDisabled = loading || (activeTab === "oauth" && oauthPending);

  const resetForm = () => {
    setName("");
    setFilePath("");
    setError(null);
    setLoading(false);
    setOauthPending(false);
    setAuthUrl("");
  };

  const handleClose = () => {
    if (oauthPending) {
      void onCancelOAuth();
    }
    resetForm();
    onClose();
  };

  const handleOAuthLogin = async () => {
    if (!name.trim()) {
      setError("Please enter an account name");
      return;
    }

    try {
      setLoading(true);
      setError(null);
      const info = await onStartOAuth(name.trim());
      setAuthUrl(info.auth_url);
      setOauthPending(true);
      setLoading(false);

      await onCompleteOAuth();
      handleClose();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
      setLoading(false);
      setOauthPending(false);
    }
  };

  const handleSelectFile = async () => {
    try {
      const selected = await open({
        multiple: false,
        filters: [{ name: "JSON", extensions: ["json"] }],
        title: "Select auth.json file",
      });

      if (selected) {
        setFilePath(selected);
      }
    } catch (err) {
      console.error("Failed to open file dialog:", err);
    }
  };

  const handleImportFile = async () => {
    if (!name.trim()) {
      setError("Please enter an account name");
      return;
    }
    if (!filePath.trim()) {
      setError("Please select an auth.json file");
      return;
    }

    try {
      setLoading(true);
      setError(null);
      await onImportFile(filePath.trim(), name.trim());
      handleClose();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
      setLoading(false);
    }
  };

  if (!isOpen) return null;

  return (
    <div className="theme-scrim fixed inset-0 z-50 flex items-center justify-center px-4">
      <div
        className="theme-panel w-full max-w-md rounded-2xl shadow-xl shadow-slate-900/15 dark:shadow-slate-950/60"
        style={{ backgroundColor: "var(--theme-surface)" }}
      >
        <div
          className="flex items-center justify-between border-b p-5"
          style={{ borderColor: "var(--theme-border-subtle)" }}
        >
          <h2 className="text-lg font-semibold text-slate-900 dark:text-slate-50">
            Add Account
          </h2>
          <button
            onClick={handleClose}
            className="text-slate-400 transition-colors hover:text-slate-600 dark:text-slate-500 dark:hover:text-slate-300"
          >
            Close
          </button>
        </div>

        <div className="flex border-b" style={{ borderColor: "var(--theme-border-subtle)" }}>
          {(["oauth", "import"] as Tab[]).map((tab) => (
            <button
              key={tab}
              onClick={() => {
                if (tab === "import" && oauthPending) {
                  void onCancelOAuth().catch((err) => {
                    console.error("Failed to cancel login:", err);
                  });
                  setOauthPending(false);
                  setLoading(false);
                }
                setActiveTab(tab);
                setError(null);
              }}
              className={`flex-1 px-4 py-3 text-sm font-medium transition-colors ${
                activeTab === tab
                  ? "border-b-2 text-slate-900 dark:text-[var(--theme-text-primary)]"
                  : "text-slate-400 hover:text-slate-600 dark:text-slate-500 dark:hover:text-slate-300"
              }`}
              style={
                activeTab === tab
                  ? { borderColor: "var(--theme-primary-bg)" }
                  : undefined
              }
            >
              {tab === "oauth" ? "ChatGPT Login" : "Import File"}
            </button>
          ))}
        </div>

        <div className="space-y-4 p-5">
          <div>
            <label className="mb-2 block text-sm font-medium text-slate-700 dark:text-slate-300">
              Account Name
            </label>
            <input
              type="text"
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="e.g., Work Account"
              className="theme-field w-full rounded-lg px-4 py-2.5"
            />
          </div>

          {activeTab === "oauth" && (
            <div className="text-sm text-slate-500 dark:text-slate-400">
              {oauthPending ? (
                <div className="py-4 text-center">
                  <div className="mx-auto mb-3 h-8 w-8 animate-spin rounded-full border-2 border-slate-900 border-t-transparent dark:border-slate-100" />
                  <p className="mb-2 font-medium text-slate-700 dark:text-slate-200">
                    Waiting for browser login...
                  </p>
                  <p className="mb-4 text-xs text-slate-500 dark:text-slate-400">
                    Open the following link in your browser to continue:
                  </p>
                  <div className="theme-panel-elevated mb-2 flex items-center gap-2 rounded-lg p-2">
                    <input
                      type="text"
                      readOnly
                      value={authUrl}
                      className="flex-1 truncate bg-transparent text-xs text-slate-600 focus:outline-none dark:text-slate-300"
                    />
                    <button
                      onClick={() => {
                        void navigator.clipboard.writeText(authUrl);
                        setCopied(true);
                        setTimeout(() => setCopied(false), 2000);
                      }}
                      className={`shrink-0 rounded px-3 py-1.5 text-xs font-medium ${
                        copied
                          ? "theme-toast-success"
                          : "theme-button-secondary"
                      }`}
                    >
                      {copied ? "Copied!" : "Copy"}
                    </button>
                    <button
                      onClick={() => openUrl(authUrl)}
                      className="theme-button-primary shrink-0 rounded px-3 py-1.5 text-xs font-medium"
                    >
                      Open
                    </button>
                  </div>
                </div>
              ) : (
                <p>
                  Click the button below to generate a login link. You will need to
                  open it in your browser to authenticate.
                </p>
              )}
            </div>
          )}

          {activeTab === "import" && (
            <div>
              <label className="mb-2 block text-sm font-medium text-slate-700 dark:text-slate-300">
                Select auth.json file
              </label>
              <div className="flex gap-2">
                <div className="theme-panel-elevated flex-1 truncate rounded-lg px-4 py-2.5 text-sm text-slate-600 dark:text-slate-300">
                  {filePath || "No file selected"}
                </div>
                <button
                  onClick={handleSelectFile}
                  className="theme-button-secondary whitespace-nowrap rounded-lg px-4 py-2.5 text-sm font-medium"
                >
                  Browse...
                </button>
              </div>
              <p className="mt-2 text-xs text-slate-400 dark:text-slate-500">
                Import credentials from an existing Codex auth.json file
              </p>
            </div>
          )}

          {error && (
            <div className="theme-toast-danger rounded-lg p-3 text-sm">
              {error}
            </div>
          )}
        </div>

        <div className="flex gap-3 border-t p-5" style={{ borderColor: "var(--theme-border-subtle)" }}>
          <button
            onClick={handleClose}
            className="theme-button-secondary flex-1 rounded-lg px-4 py-2.5 text-sm font-medium"
          >
            Cancel
          </button>
          <button
            onClick={activeTab === "oauth" ? handleOAuthLogin : handleImportFile}
            disabled={isPrimaryDisabled}
            className="theme-button-primary flex-1 rounded-lg px-4 py-2.5 text-sm font-medium disabled:opacity-50"
          >
            {loading
              ? "Adding..."
              : activeTab === "oauth"
                ? "Generate Login Link"
                : "Import"}
          </button>
        </div>
      </div>
    </div>
  );
}
