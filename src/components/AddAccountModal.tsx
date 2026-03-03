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
  const [authUrl, setAuthUrl] = useState<string>("");
  const [copied, setCopied] = useState<boolean>(false);
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
      onCancelOAuth();
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

      // Wait for completion
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
        filters: [
          {
            name: "JSON",
            extensions: ["json"],
          },
        ],
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
    <div className="fixed inset-0 bg-black/40 flex items-center justify-center z-50">
      <div className="bg-white border border-gray-200 rounded-2xl w-full max-w-md mx-4 shadow-xl">
        {/* Header */}
        <div className="flex items-center justify-between p-5 border-b border-gray-100">
          <h2 className="text-lg font-semibold text-gray-900">Add Account</h2>
          <button
            onClick={handleClose}
            className="text-gray-400 hover:text-gray-600 transition-colors"
          >
            ✕
          </button>
        </div>

        {/* Tabs */}
        <div className="flex border-b border-gray-100">
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
              className={`flex-1 px-4 py-3 text-sm font-medium transition-colors ${activeTab === tab
                  ? "text-gray-900 border-b-2 border-gray-900 -mb-px"
                  : "text-gray-400 hover:text-gray-600"
                }`}
            >
              {tab === "oauth" ? "ChatGPT Login" : "Import File"}
            </button>
          ))}
        </div>

        {/* Content */}
        <div className="p-5 space-y-4">
          {/* Account Name (always shown) */}
          <div>
            <label className="block text-sm font-medium text-gray-700 mb-2">
              Account Name
            </label>
            <input
              type="text"
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="e.g., Work Account"
              className="w-full px-4 py-2.5 bg-white border border-gray-200 rounded-lg text-gray-900 placeholder-gray-400 focus:outline-none focus:border-gray-400 focus:ring-1 focus:ring-gray-400 transition-colors"
            />
          </div>

          {/* Tab-specific content */}
          {activeTab === "oauth" && (
            <div className="text-sm text-gray-500">
              {oauthPending ? (
                <div className="text-center py-4">
                  <div className="animate-spin h-8 w-8 border-2 border-gray-900 border-t-transparent rounded-full mx-auto mb-3"></div>
                  <p className="text-gray-700 font-medium mb-2">Waiting for browser login...</p>
                  <p className="text-xs text-gray-500 mb-4">
                    Please open the following link in your browser to proceed:
                  </p>
                  <div className="flex items-center gap-2 mb-2 bg-gray-50 p-2 rounded-lg border border-gray-200">
                    <input
                      type="text"
                      readOnly
                      value={authUrl}
                      className="flex-1 bg-transparent border-none text-xs text-gray-600 focus:outline-none focus:ring-0 truncate"
                    />
                    <button
                      onClick={() => {
                        navigator.clipboard.writeText(authUrl);
                        setCopied(true);
                        setTimeout(() => setCopied(false), 2000);
                      }}
                      className={`px-3 py-1.5 border rounded text-xs font-medium transition-colors shrink-0 
                        ${copied
                          ? "bg-green-50 border-green-200 text-green-700"
                          : "bg-white border-gray-200 text-gray-700 hover:bg-gray-50"
                        }`}
                    >
                      {copied ? "Copied!" : "Copy"}
                    </button>
                    <button
                      onClick={() => openUrl(authUrl)}
                      className="px-3 py-1.5 bg-gray-900 border border-gray-900 rounded text-xs font-medium text-white hover:bg-gray-800 transition-colors shrink-0"
                    >
                      Open
                    </button>
                  </div>
                </div>
              ) : (
                <p>
                  Click the button below to generate a login link.
                  You will need to open it in your browser to authenticate.
                </p>
              )}
            </div>
          )}

          {activeTab === "import" && (
            <div>
              <label className="block text-sm font-medium text-gray-700 mb-2">
                Select auth.json file
              </label>
              <div className="flex gap-2">
                <div className="flex-1 px-4 py-2.5 bg-gray-50 border border-gray-200 rounded-lg text-sm text-gray-600 truncate">
                  {filePath || "No file selected"}
                </div>
                <button
                  onClick={handleSelectFile}
                  className="px-4 py-2.5 bg-gray-100 hover:bg-gray-200 border border-gray-200 rounded-lg text-sm font-medium text-gray-700 transition-colors whitespace-nowrap"
                >
                  Browse...
                </button>
              </div>
              <p className="text-xs text-gray-400 mt-2">
                Import credentials from an existing Codex auth.json file
              </p>
            </div>
          )}

          {/* Error */}
          {error && (
            <div className="p-3 bg-red-50 border border-red-200 rounded-lg text-red-600 text-sm">
              {error}
            </div>
          )}
        </div>

        {/* Footer */}
        <div className="flex gap-3 p-5 border-t border-gray-100">
          <button
            onClick={handleClose}
            className="flex-1 px-4 py-2.5 text-sm font-medium rounded-lg bg-gray-100 hover:bg-gray-200 text-gray-700 transition-colors"
          >
            Cancel
          </button>
          <button
            onClick={activeTab === "oauth" ? handleOAuthLogin : handleImportFile}
            disabled={isPrimaryDisabled}
            className="flex-1 px-4 py-2.5 text-sm font-medium rounded-lg bg-gray-900 hover:bg-gray-800 text-white transition-colors disabled:opacity-50"
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
