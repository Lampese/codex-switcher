import { useRef, useState } from "react";
import type { CustomProvider } from "../types";
import {
  describeFileSource,
  isTauriRuntime,
  openExternalUrl,
  pickAuthJsonFile,
  type FileSource,
} from "../lib/platform";

interface AddAccountModalProps {
  isOpen: boolean;
  onClose: () => void;
  onImportFile: (source: FileSource, name: string, customProvider: CustomProvider | null) => Promise<void>;
  onAddApiKey: (name: string, apiKey: string, customProvider: CustomProvider | null) => Promise<void>;
  onLoadModels: (apiKey: string, baseUrl: string) => Promise<string[]>;
  onStartOAuth: (name: string) => Promise<{ auth_url: string }>;
  onCompleteOAuth: () => Promise<unknown>;
  onCancelOAuth: () => Promise<void>;
}

type SavedProvider = Pick<CustomProvider, "name" | "base_url">;
const PROVIDERS_KEY = "codex-switcher.saved-providers";
function readProviders(): SavedProvider[] {
  try {
    const value: unknown = JSON.parse(localStorage.getItem(PROVIDERS_KEY) || "[]");
    return Array.isArray(value) ? value.filter((p): p is SavedProvider =>
      !!p && typeof p.name === "string" && typeof p.base_url === "string") : [];
  } catch { return []; }
}

type Tab = "oauth" | "api_key" | "import";

export function AddAccountModal({
  isOpen,
  onClose,
  onImportFile,
  onAddApiKey,
  onLoadModels,
  onStartOAuth,
  onCompleteOAuth,
  onCancelOAuth,
}: AddAccountModalProps) {
  const [activeTab, setActiveTab] = useState<Tab>("oauth");
  const [name, setName] = useState("");
  const [apiKey, setApiKey] = useState("");
  const [fileSource, setFileSource] = useState<FileSource | null>(null);
  const [useCustomProvider, setUseCustomProvider] = useState(false);
  const [provider, setProvider] = useState<CustomProvider>({ name: "", base_url: "", model: "" });
  const [savedProviders, setSavedProviders] = useState<SavedProvider[]>(readProviders);
  const [registeringProvider, setRegisteringProvider] = useState(false);
  const registerProvider = () => {
    try {
      const name = provider.name.trim();
      const url = new URL(provider.base_url.trim());
      if (!name || (url.protocol !== "https:" && !(url.protocol === "http:" && ["localhost", "127.0.0.1", "[::1]"].includes(url.hostname))) || url.username || url.password || url.search || url.hash) {
        throw new Error("Enter a provider name and HTTPS base URL (or local HTTP URL), without credentials, query, or fragment.");
      }
      const entry = { name, base_url: url.href.replace(/\/$/, "") };
      const next = [...savedProviders.filter(p => p.base_url !== entry.base_url), entry];
      localStorage.setItem(PROVIDERS_KEY, JSON.stringify(next));
      setSavedProviders(next);
      setProvider({ ...entry, model: provider.model });
      setRegisteringProvider(false);
      setError(null);
    } catch (err) { setError(String(err)); }
  };
  const [models, setModels] = useState<string[]>([]);
  const [modelsLoading, setModelsLoading] = useState(false);
  const [modelsError, setModelsError] = useState<string | null>(null);
  const modelRequest = useRef(0);
  const clearModels = () => {
    modelRequest.current += 1;
    setModels([]);
    setModelsLoading(false);
    setModelsError(null);
  };
  const loadModels = async () => {
    const request = ++modelRequest.current;
    setModelsLoading(true);
    setModelsError(null);
    try {
      const available = await onLoadModels(apiKey.trim(), provider.base_url.trim());
      if (request !== modelRequest.current) return;
      setModels(available);
      if (!available.length) setModelsError("No models returned. Enter the model ID manually.");
    } catch (err) {
      if (request === modelRequest.current) setModelsError(`${String(err)} You can enter the model ID manually.`);
    } finally {
      if (request === modelRequest.current) setModelsLoading(false);
    }
  };
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [oauthPending, setOauthPending] = useState(false);
  const [authUrl, setAuthUrl] = useState<string>("");
  const [copied, setCopied] = useState<boolean>(false);
  const isPrimaryDisabled = loading || (activeTab === "oauth" && oauthPending);
  const tauriRuntime = isTauriRuntime();

  const resetForm = () => {
    setName("");
    setApiKey("");
    clearModels();
    setFileSource(null);
    setUseCustomProvider(false);
    setRegisteringProvider(false);
    setProvider({ name: "", base_url: "", model: "" });
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
      const selected = await pickAuthJsonFile();
      if (selected) setFileSource(selected);
    } catch (err) {
      console.error("Failed to open file dialog:", err);
    }
  };

  const handleAddCredentials = async () => {
    if (activeTab === "import" && !fileSource) {
      setError("Please select an auth.json file");
      return;
    }
    if (activeTab === "api_key" && !apiKey.trim()) {
      setError("Please enter an API key");
      return;
    }
    if (useCustomProvider && (!provider.name.trim() || !provider.base_url.trim() || !provider.model.trim())) {
      setError("Enter the provider name, API base URL, and model, or turn off provider overrides.");
      return;
    }

    try {
      setLoading(true);
      setError(null);
      const customProvider = useCustomProvider ? {
        name: provider.name.trim(), base_url: provider.base_url.trim(), model: provider.model.trim(),
      } : null;
      if (activeTab === "api_key") {
        await onAddApiKey(name.trim(), apiKey.trim(), customProvider);
      } else if (fileSource) {
        await onImportFile(fileSource, name.trim(), customProvider);
      }
      handleClose();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
      setLoading(false);
    }
  };

  if (!isOpen) return null;

  return (
    <div className="fixed inset-0 bg-black/40 flex items-center justify-center z-50">
      <div role="dialog" aria-modal="true" aria-label="Add Account" className="bg-white dark:bg-gray-900 border border-gray-200 dark:border-gray-700 rounded-2xl w-full max-w-md max-h-[90vh] overflow-y-auto mx-4 shadow-xl">
        {/* Header */}
        <div className="flex items-center justify-between p-5 border-b border-gray-100 dark:border-gray-800">
          <h2 className="text-lg font-semibold text-gray-900 dark:text-gray-100">Add Account</h2>
          <button
            onClick={handleClose}
            className="text-gray-400 hover:text-gray-600 dark:hover:text-gray-300 transition-colors"
          >
            ✕
          </button>
        </div>

        {/* Tabs */}
        <div className="flex border-b border-gray-100 dark:border-gray-800">
          {(["oauth", "api_key", "import"] as Tab[]).map((tab) => (
            <button
              key={tab}
              disabled={loading}
              onClick={() => {
                if (tab !== "oauth" && oauthPending) {
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
                  ? "text-gray-900 dark:text-gray-100 border-b-2 border-gray-900 dark:border-gray-100 -mb-px"
                  : "text-gray-400 dark:text-gray-500 hover:text-gray-600 dark:hover:text-gray-300"
                }`}
            >
              {tab === "oauth" ? "ChatGPT Login" : tab === "api_key" ? "API Key" : "Import File"}
            </button>
          ))}
        </div>

        {/* Content */}
        <div className="p-5 space-y-4">
          {/* Account name is optional; the backend derives one when blank. */}
          <div>
            <label className="block text-sm font-medium text-gray-700 dark:text-gray-300 mb-2">
              Account Name (optional)
            </label>
            <input
              type="text"
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder={activeTab === "oauth" ? "Leave blank to use email" : "e.g. Personal account"}
              className="w-full px-4 py-2.5 bg-white dark:bg-gray-800 border border-gray-200 dark:border-gray-700 rounded-lg text-gray-900 dark:text-gray-100 placeholder-gray-400 dark:placeholder-gray-500 focus:outline-none focus:border-gray-400 dark:focus:border-gray-500 focus:ring-1 focus:ring-gray-400 dark:focus:ring-gray-500 transition-colors"
            />
          </div>

          {/* Tab-specific content */}
          {activeTab === "oauth" && (
            <div className="text-sm text-gray-500 dark:text-gray-400">
              {oauthPending ? (
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
                            setError("Clipboard unavailable. Copy the link manually.");
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
              ) : (
                <p>
                  Click the button below to generate a login link.
                  You will need to open it in your browser to authenticate.
                </p>
              )}
            </div>
          )}

          {activeTab === "api_key" && (
            <label className="block text-sm font-medium text-gray-700 dark:text-gray-300">
              API key
              <input type="password" autoComplete="off" spellCheck={false} value={apiKey}
                disabled={loading} onChange={(e) => { setApiKey(e.target.value); clearModels(); }} placeholder="Paste your API key"
                className="mt-2 w-full px-4 py-2.5 bg-white dark:bg-gray-800 border border-gray-200 dark:border-gray-700 rounded-lg text-gray-900 dark:text-gray-100" />
            </label>
          )}

          {activeTab === "import" && (
            <div>
              <label className="block text-sm font-medium text-gray-700 dark:text-gray-300 mb-2">
                Select auth.json file
              </label>
              <div className="flex gap-2">
                <div className="flex-1 px-4 py-2.5 bg-gray-50 dark:bg-gray-800 border border-gray-200 dark:border-gray-700 rounded-lg text-sm text-gray-600 dark:text-gray-300 truncate">
                  {describeFileSource(fileSource)}
                </div>
                <button
                  onClick={handleSelectFile}
                  className="px-4 py-2.5 bg-gray-100 hover:bg-gray-200 dark:bg-gray-800 dark:hover:bg-gray-700 border border-gray-200 dark:border-gray-700 rounded-lg text-sm font-medium text-gray-700 dark:text-gray-200 transition-colors whitespace-nowrap"
                >
                  Browse...
                </button>
              </div>
              <p className="text-xs text-gray-400 dark:text-gray-500 mt-2">
                Import credentials from an existing Codex auth.json file
              </p>
            </div>
          )}

          {activeTab !== "oauth" && (
            <div>
              <label className="flex items-center gap-2 mt-4 text-sm text-gray-700 dark:text-gray-300">
                <input type="checkbox" checked={useCustomProvider} disabled={loading}
                  onChange={(e) => setUseCustomProvider(e.target.checked)} />
                Override provider and model (API key only)
              </label>
              {!useCustomProvider && <p className="text-xs text-gray-500 dark:text-gray-400 mt-2">Leave off for regular OpenAI accounts. Enable for a custom API endpoint and model.</p>}
              {useCustomProvider && (
                <div className="mt-3 space-y-3">
                  <label className="block text-sm text-gray-700 dark:text-gray-300">
                    Provider
                    <select value={registeringProvider ? "" : provider.base_url} disabled={loading}
                      onChange={e => {
                        const selected = savedProviders.find(p => p.base_url === e.target.value);
                        setProvider({ name: selected?.name || "", base_url: selected?.base_url || "", model: "" });
                        setRegisteringProvider(false);
                        clearModels();
                      }}
                      className="mt-1 w-full px-3 py-2 bg-white dark:bg-gray-800 border border-gray-200 dark:border-gray-700 rounded-lg">
                      <option value="">Select a provider</option>
                      {savedProviders.map(p => <option key={p.base_url} value={p.base_url}>{p.name} — {p.base_url}</option>)}
                    </select>
                  </label>
                  <button type="button" disabled={loading} onClick={() => {
                    setRegisteringProvider(true);
                    setProvider({ name: "", base_url: "", model: "" });
                    clearModels();
                  }} className="text-sm text-blue-600 dark:text-blue-400">Register provider</button>
                  {registeringProvider && <div className="space-y-3 rounded-lg border border-gray-200 dark:border-gray-700 p-3">
                    {([["name", "Provider name", "My provider"], ["base_url", "API base URL", "https://gateway.example/v1"]] as const).map(([field, label, placeholder]) => (
                      <label key={field} className="block text-sm text-gray-700 dark:text-gray-300">{label}
                        <input type={field === "base_url" ? "url" : "text"} value={provider[field]} disabled={loading} placeholder={placeholder}
                          onChange={e => { setProvider({ ...provider, [field]: e.target.value }); clearModels(); }}
                          className="mt-1 w-full px-3 py-2 bg-white dark:bg-gray-800 border border-gray-200 dark:border-gray-700 rounded-lg" />
                      </label>
                    ))}
                    <button type="button" onClick={registerProvider} disabled={loading} className="px-3 py-2 text-sm rounded-lg bg-blue-600 text-white">Save provider</button>
                    <p className="text-xs text-gray-500">Saved on this device. API keys belong to each account.</p>
                  </div>}
                  {models.length > 0 && <label className="block text-sm text-gray-700 dark:text-gray-300">
                    Available models
                    <select size={Math.min(models.length + 1, 6)} value={models.includes(provider.model) ? provider.model : ""}
                      disabled={loading || modelsLoading} onChange={e => setProvider({ ...provider, model: e.target.value })}
                      className="mt-1 w-full px-3 py-2 bg-white dark:bg-gray-800 border border-gray-200 dark:border-gray-700 rounded-lg">
                      <option value="" disabled>Select the default model</option>
                      {models.map(model => <option key={model} value={model}>{model}</option>)}
                    </select>
                  </label>}
                  <label className="block text-sm text-gray-700 dark:text-gray-300">Default model
                    <input value={provider.model} disabled={loading} placeholder="Choose a model or enter its ID"
                      onChange={e => setProvider({ ...provider, model: e.target.value })}
                      className="mt-1 w-full px-3 py-2 bg-white dark:bg-gray-800 border border-gray-200 dark:border-gray-700 rounded-lg" />
                  </label>
                  {activeTab === "api_key" && (
                    <div>
                      <button type="button" onClick={() => void loadModels()}
                        disabled={loading || modelsLoading || !apiKey.trim() || !provider.base_url.trim()}
                        className="px-3 py-2 text-sm rounded-lg bg-gray-100 dark:bg-gray-800 text-gray-700 dark:text-gray-300 disabled:opacity-50">
                        {modelsLoading ? "Loading models..." : "Load models"}
                      </button>
                      {models.length > 0 && <p className="text-xs text-gray-500 mt-2">{models.length} models loaded. Select a default from the list above, or enter its ID.</p>}
                      {modelsError && <p role="status" className="text-xs text-amber-700 dark:text-amber-400 mt-2">{modelsError}</p>}
                    </div>
                  )}
                  <p className="text-xs text-gray-500 dark:text-gray-400">
                    Switching activates this provider and model. Switching back restores your previous Codex configuration.
                    Custom providers must support the Responses API; usage and warm-up are unavailable.
                  </p>
                </div>
              )}
            </div>
          )}

          {/* Error */}
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
          <button
            onClick={activeTab === "oauth" ? handleOAuthLogin : handleAddCredentials}
            disabled={isPrimaryDisabled}
            className="flex-1 px-4 py-2.5 text-sm font-medium rounded-lg bg-gray-900 hover:bg-gray-800 dark:bg-gray-100 dark:hover:bg-gray-200 text-white dark:text-gray-900 transition-colors disabled:opacity-50"
          >
            {loading
              ? "Adding..."
              : activeTab === "oauth"
                ? "Generate Login Link"
                : activeTab === "api_key" ? "Add Account" : "Import"}
          </button>
        </div>
      </div>
    </div>
  );
}
