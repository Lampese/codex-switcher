import { useState, useEffect, useCallback } from "react";
import type { InstanceProfile } from "../types";
import { invokeBackend } from "../lib/platform";

export function useInstances() {
    const [instances, setInstances] = useState<InstanceProfile[]>([]);
    const [activeInstance, setActiveInstance] = useState<InstanceProfile | null>(null);
    const [loading, setLoading] = useState(true);

    const loadInstances = useCallback(async () => {
        try {
            setLoading(true);
            const [list, active] = await Promise.all([
                invokeBackend<InstanceProfile[]>("list_instances"),
                invokeBackend<InstanceProfile | null>("get_active_instance"),
            ]);
            setInstances(list);
            setActiveInstance(active);
        } catch (err) {
            console.error("Failed to load instances:", err);
        } finally {
            setLoading(false);
        }
    }, []);

    const createInstance = useCallback(
        async (name: string, userDataDir: string) => {
            const instance = await invokeBackend<InstanceProfile>("create_instance", {
                name,
                userDataDir,
            });
            await loadInstances();
            return instance;
        },
        [loadInstances]
    );

    const createEmptyInstance = useCallback(
        async (name: string, userDataDir: string) => {
            const instance = await invokeBackend<InstanceProfile>(
                "create_empty_instance",
                { name, userDataDir }
            );
            await loadInstances();
            return instance;
        },
        [loadInstances]
    );

    const switchInstance = useCallback(
        async (instanceId: string) => {
            await invokeBackend<InstanceProfile>("set_active_instance", {
                instanceId,
            });
            await loadInstances();
        },
        [loadInstances]
    );

    const removeInstance = useCallback(
        async (instanceId: string, deleteData: boolean) => {
            await invokeBackend("remove_instance", { instanceId, deleteData });
            await loadInstances();
        },
        [loadInstances]
    );

    const bindAccount = useCallback(
        async (instanceId: string, accountId: string | null) => {
            await invokeBackend<InstanceProfile>("bind_instance_account", {
                instanceId,
                accountId,
            });
            await loadInstances();
        },
        [loadInstances]
    );

    const getLaunchCommand = useCallback(async (instanceId: string) => {
        return invokeBackend<string>("get_instance_launch_command", { instanceId });
    }, []);

    const launchCodex = useCallback(
        async (instanceId: string) => {
            await invokeBackend("launch_instance_codex", { instanceId });
            await loadInstances();
        },
        [loadInstances]
    );

    useEffect(() => {
        loadInstances();
    }, [loadInstances]);

    return {
        instances,
        activeInstance,
        loading,
        loadInstances,
        createInstance,
        createEmptyInstance,
        switchInstance,
        removeInstance,
        bindAccount,
        getLaunchCommand,
        launchCodex,
    };
}
