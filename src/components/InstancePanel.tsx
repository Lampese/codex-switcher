import { useState } from "react";
import type { InstanceProfile } from "../types";

interface InstancePanelProps {
    instances: InstanceProfile[];
    activeInstance: InstanceProfile | null;
    accounts: { id: string; name: string }[];
    loading: boolean;
    onCreateInstance: (name: string, dir: string) => Promise<InstanceProfile>;
    onCreateEmptyInstance: (name: string, dir: string) => Promise<InstanceProfile>;
    onSwitchInstance: (id: string) => Promise<void>;
    onRemoveInstance: (id: string, deleteData: boolean) => Promise<void>;
    onBindAccount: (instanceId: string, accountId: string | null) => Promise<void>;
}

export function InstancePanel({
    instances,
    activeInstance,
    accounts,
    loading,
    onCreateInstance,
    onCreateEmptyInstance,
    onSwitchInstance,
    onRemoveInstance,
    onBindAccount,
}: InstancePanelProps) {
    const [isCreating, setIsCreating] = useState(false);
    const [newName, setNewName] = useState("");
    const [newDir, setNewDir] = useState("");
    const [createEmpty, setCreateEmpty] = useState(false);
    const [error, setError] = useState<string | null>(null);
    const [switchingId, setSwitchingId] = useState<string | null>(null);
    const [deleteConfirmId, setDeleteConfirmId] = useState<string | null>(null);

    const handleCreate = async () => {
        if (!newName.trim() || !newDir.trim()) {
            setError("Name and directory are required");
            return;
        }
        try {
            setError(null);
            if (createEmpty) {
                await onCreateEmptyInstance(newName, newDir);
            } else {
                await onCreateInstance(newName, newDir);
            }
            setNewName("");
            setNewDir("");
            setIsCreating(false);
        } catch (err) {
            setError(err instanceof Error ? err.message : String(err));
        }
    };

    const handleSwitch = async (id: string) => {
        try {
            setSwitchingId(id);
            await onSwitchInstance(id);
        } finally {
            setSwitchingId(null);
        }
    };

    const handleRemove = async (id: string) => {
        if (deleteConfirmId !== id) {
            setDeleteConfirmId(id);
            setTimeout(() => setDeleteConfirmId(null), 3000);
            return;
        }
        await onRemoveInstance(id, false);
        setDeleteConfirmId(null);
    };

    if (loading && instances.length === 0) {
        return (
            <div className="text-center py-4 text-sm text-gray-500 dark:text-gray-400">
                Loading instances...
            </div>
        );
    }

    return (
        <section>
            <div className="flex items-center justify-between mb-4">
                <h2 className="text-sm font-medium text-gray-500 dark:text-gray-400 uppercase tracking-wider">
                    Instances ({instances.length})
                </h2>
                <button
                    onClick={() => setIsCreating(!isCreating)}
                    className="text-xs px-3 py-1.5 rounded-lg bg-gray-100 text-gray-700 hover:bg-gray-200 dark:bg-gray-800 dark:text-gray-300 dark:hover:bg-gray-700 transition-colors"
                >
                    {isCreating ? "Cancel" : "+ New Instance"}
                </button>
            </div>

            {/* Create Form */}
            {isCreating && (
                <div className="mb-4 p-4 rounded-xl border border-gray-200 bg-white dark:border-gray-800 dark:bg-gray-900">
                    <div className="space-y-3">
                        <input
                            type="text"
                            placeholder="Instance name"
                            value={newName}
                            onChange={(e) => setNewName(e.target.value)}
                            className="w-full px-3 py-2 text-sm rounded-lg border border-gray-300 bg-white dark:border-gray-700 dark:bg-gray-800 dark:text-gray-100 focus:outline-none focus:ring-2 focus:ring-gray-300 dark:focus:ring-gray-600"
                        />
                        <div className="flex gap-2">
                            <input
                                type="text"
                                placeholder="Data directory (e.g. C:\Users\you\.codex-work)"
                                value={newDir}
                                onChange={(e) => setNewDir(e.target.value)}
                                className="flex-1 px-3 py-2 text-sm rounded-lg border border-gray-300 bg-white dark:border-gray-700 dark:bg-gray-800 dark:text-gray-100 focus:outline-none focus:ring-2 focus:ring-gray-300 dark:focus:ring-gray-600"
                            />
                            <button
                                onClick={async () => {
                                    try {
                                        const { open } = await import("@tauri-apps/plugin-dialog");
                                        const selected = await open({
                                            directory: true,
                                            multiple: false,
                                        });
                                        if (selected && typeof selected === 'string') {
                                            setNewDir(selected);
                                        }
                                    } catch (err) {
                                        console.error("Failed to pick folder", err);
                                    }
                                }}
                                className="px-3 py-2 text-sm font-medium rounded-lg text-gray-700 bg-gray-100 hover:bg-gray-200 dark:text-gray-300 dark:bg-gray-800 dark:hover:bg-gray-700 transition-colors"
                            >
                                Browse
                            </button>
                        </div>
                        <label className="flex items-center gap-2 text-sm text-gray-600 dark:text-gray-400">
                            <input
                                type="checkbox"
                                checked={createEmpty}
                                onChange={(e) => setCreateEmpty(e.target.checked)}
                                className="rounded"
                            />
                            Create empty (don't copy from ~/.codex/)
                        </label>
                        {error && (
                            <p className="text-xs text-red-500">{error}</p>
                        )}
                        <button
                            onClick={handleCreate}
                            className="w-full py-2 text-sm font-medium rounded-lg bg-gray-900 text-white hover:bg-gray-800 dark:bg-gray-100 dark:text-gray-900 dark:hover:bg-gray-200 transition-colors"
                        >
                            Create Instance
                        </button>
                    </div>
                </div>
            )}

            {/* Instance List */}
            {instances.length === 0 ? (
                <div className="text-center py-8 text-sm text-gray-500 dark:text-gray-400">
                    No instances yet. Each instance has its own isolated Codex config.
                </div>
            ) : (
                <div className="space-y-2">
                    {instances.map((inst) => {
                        const isActive = activeInstance?.id === inst.id;
                        const boundAccount = accounts.find(
                            (a) => a.id === inst.bind_account_id
                        );

                        return (
                            <div
                                key={inst.id}
                                className={`p-3 rounded-xl border transition-colors ${isActive
                                    ? "border-green-300 bg-green-50 dark:border-green-700 dark:bg-green-900/20"
                                    : "border-gray-200 bg-white dark:border-gray-800 dark:bg-gray-900"
                                    }`}
                            >
                                <div className="flex items-center justify-between">
                                    <div className="min-w-0 flex-1">
                                        <div className="flex items-center gap-2">
                                            <span className="font-medium text-sm text-gray-900 dark:text-gray-100 truncate">
                                                {inst.name}
                                            </span>
                                            {isActive && (
                                                <span className="text-xs px-1.5 py-0.5 rounded bg-green-100 text-green-700 dark:bg-green-800 dark:text-green-300">
                                                    Active
                                                </span>
                                            )}
                                        </div>
                                        <p className="text-xs text-gray-500 dark:text-gray-400 truncate mt-0.5">
                                            {inst.user_data_dir}
                                        </p>
                                        {boundAccount && (
                                            <p className="text-xs text-blue-600 dark:text-blue-400 mt-0.5">
                                                → {boundAccount.name}
                                            </p>
                                        )}
                                    </div>
                                    <div className="flex items-center gap-1 ml-2 shrink-0">
                                        {!isActive && (
                                            <button
                                                onClick={() => handleSwitch(inst.id)}
                                                disabled={switchingId === inst.id}
                                                className="text-xs px-2.5 py-1.5 rounded-lg bg-gray-900 text-white hover:bg-gray-800 dark:bg-gray-100 dark:text-gray-900 dark:hover:bg-gray-200 disabled:opacity-50 transition-colors"
                                            >
                                                {switchingId === inst.id ? "..." : "Use"}
                                            </button>
                                        )}
                                        <select
                                            value={inst.bind_account_id ?? ""}
                                            onChange={(e) =>
                                                onBindAccount(inst.id, e.target.value || null)
                                            }
                                            className="text-xs px-1.5 py-1.5 rounded-lg border border-gray-300 bg-white dark:border-gray-700 dark:bg-gray-800 dark:text-gray-200 max-w-[100px]"
                                            title="Bind account"
                                        >
                                            <option value="">No bind</option>
                                            {accounts.map((a) => (
                                                <option key={a.id} value={a.id}>
                                                    {a.name}
                                                </option>
                                            ))}
                                        </select>
                                        <button
                                            onClick={() => handleRemove(inst.id)}
                                            className={`text-xs px-2 py-1.5 rounded-lg transition-colors ${deleteConfirmId === inst.id
                                                ? "bg-red-600 text-white"
                                                : "text-gray-500 hover:text-red-600 hover:bg-red-50 dark:hover:bg-red-900/30"
                                                }`}
                                            title={
                                                deleteConfirmId === inst.id
                                                    ? "Click again to confirm"
                                                    : "Remove instance"
                                            }
                                        >
                                            ✕
                                        </button>
                                    </div>
                                </div>
                            </div>
                        );
                    })}
                </div>
            )}
        </section>
    );
}
