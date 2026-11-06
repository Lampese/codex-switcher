//! Preserve machine-local project organization across Codex account switches.

use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose, Engine as _};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::{collections::HashSet, fs, io::Write, path::Path};

use crate::types::{AuthData, AuthDotJson, StoredAccount};

const ACCOUNT_STATE: &str = "electron-account-renderer-state-v1";
const PROJECTS: &str = "local-projects";
const MIGRATION_IDS: &str = "app-server-project-id-by-legacy-project-id-by-host";
const MIGRATION_STATE: &str = "app-server-projects-migration-by-host";
const ASSIGNMENTS: &str = "thread-project-assignments";
const MEMBERSHIP_HOSTS: &str = "thread-project-membership-host-ids";
const BASELINES_FILE: &str = ".codex-switcher-project-baselines.json";

/// Called under AUTH_OPERATION_LOCK, after confirming Codex has completely exited.
/// Token refresh alone must not call this: the desktop may still own the state file.
pub fn preserve_local_projects(
    current: Option<&AuthDotJson>,
    target: &StoredAccount,
) -> Result<()> {
    let AuthData::ChatGPT {
        id_token,
        account_id,
        ..
    } = &target.auth_data
    else {
        return Ok(());
    };
    let Some(current) = current.and_then(|auth| auth.tokens.as_ref()) else {
        return Ok(());
    };
    let path = super::get_codex_home()?.join(".codex-global-state.json");
    let original = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).context("Could not read Codex project state"),
    };
    let state: Value = serde_json::from_slice(&original)
        .context("Codex project state is invalid; account switch cancelled")?;
    // Older Codex versions keep their local project list globally already.
    if state.get(ACCOUNT_STATE).is_none() {
        return Ok(());
    }
    let source = renderer_owner(&current.id_token, current.account_id.as_deref())?;
    let target = renderer_owner(id_token, account_id.as_deref())?;
    preserve_local_projects_in_file(&path, &source, &target)
}

/// Shared by account switching and explicit recovery of a known local snapshot.
/// The caller must stop Codex before invoking this function.
pub fn preserve_local_projects_in_file(
    path: &Path,
    source_owner: &str,
    target_owner: &str,
) -> Result<()> {
    let original = fs::read(path).context("Could not read Codex project state")?;
    let mut state: Value = serde_json::from_slice(&original)
        .context("Codex project state is invalid; account switch cancelled")?;
    let baseline_path = path.with_file_name(BASELINES_FILE);
    let baseline_original = read_optional(&baseline_path)?;
    let mut baselines = match &baseline_original {
        Some(bytes) => serde_json::from_slice(bytes)
            .context("Invalid Switcher project baselines; account switch cancelled")?,
        None => json!({"version":1,"accounts":{}}),
    };
    let previous_baselines = baselines.clone();
    if sync_local_projects_with_baselines(&mut state, &mut baselines, source_owner, target_owner)? {
        write_project_state(path, &original, &state)?;
    }
    // Write after the project state. If interrupted, a retry may retain an old
    // project, but cannot falsely consider unseen target additions deleted.
    if baselines != previous_baselines {
        write_json_state(&baseline_path, baseline_original.as_deref(), &baselines)?;
    }
    Ok(())
}

fn renderer_owner(id_token: &str, fallback_account_id: Option<&str>) -> Result<String> {
    let payload = id_token
        .split('.')
        .nth(1)
        .context("Codex ID token has no account identity")?;
    let bytes = general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .or_else(|_| general_purpose::URL_SAFE.decode(payload))
        .context("Could not read Codex account identity")?;
    let claims: Value = serde_json::from_slice(&bytes).context("Invalid Codex account identity")?;
    let auth = &claims["https://api.openai.com/auth"];
    let user = auth["chatgpt_user_id"]
        .as_str()
        .filter(|s| !s.is_empty())
        .context(
            "Codex account identity is missing chatgpt_user_id; sign in again before switching",
        )?;
    let account = auth["chatgpt_account_id"]
        .as_str()
        .or(fallback_account_id)
        .filter(|s| !s.is_empty())
        .context("Codex account identity is missing account ID")?;
    // Matches Codex's SHA-256(JSON.stringify([userId, accountId])).
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&[user, account])?)
    ))
}

/// First-time transfer, preserving projects unique to the target account.
pub fn sync_local_projects(
    state: &mut Value,
    source_owner: &str,
    target_owner: &str,
) -> Result<bool> {
    sync_local_projects_with_baselines(
        state,
        &mut json!({"version":1,"accounts":{}}),
        source_owner,
        target_owner,
    )
}

/// Known IDs are monotonic so deletions propagate through a third account.
/// The source learns only its own projects; it has not seen target-only
/// projects yet, and authentication can still fail after this preparation.
pub fn sync_local_projects_with_baselines(
    state: &mut Value,
    baselines: &mut Value,
    source_owner: &str,
    target_owner: &str,
) -> Result<bool> {
    let accounts = validate_baselines(baselines)?;
    let mut known_ids: HashSet<String> = string_array(accounts, source_owner)?
        .into_iter()
        .map(str::to_owned)
        .collect();
    let root = state
        .as_object()
        .context("Codex project state must be an object")?;
    let Some(scoped) = root.get(ACCOUNT_STATE) else {
        return Ok(false);
    };
    let scoped = scoped
        .as_object()
        .context("Unsupported Codex account state format")?;
    let owner = scoped
        .get("owner")
        .context("Codex account state has no owner")?;
    if !owner.is_null() && !owner.is_string() {
        bail!("Unsupported Codex account owner format");
    }
    let inactive = scoped
        .get("inactive")
        .and_then(Value::as_object)
        .context("Unsupported Codex inactive account state format")?;
    let source = if owner.as_str() == Some(source_owner) {
        root
    } else {
        slot_globals(inactive.get(source_owner).context(
            "Current account has no saved Codex state; open and close Codex before switching",
        )?)?
    };
    let Some(projects) = object_field(source, PROJECTS)? else {
        return Ok(false);
    };
    let target_is_current = owner.as_str() == Some(target_owner);
    let empty = Map::new();
    let target = if target_is_current {
        root
    } else {
        inactive
            .get(target_owner)
            .map(slot_globals)
            .transpose()?
            .unwrap_or(&empty)
    };
    let old_projects = object_field(target, PROJECTS)?.unwrap_or(&empty);
    validate_local_fields(source)?;
    validate_local_fields(target)?;
    let retained: Map<String, Value> = old_projects
        .iter()
        .filter(|(id, _)| !projects.contains_key(*id) && !known_ids.contains(*id))
        .map(|(id, value)| (id.clone(), value.clone()))
        .collect();
    let mut merged = retained.clone();
    merged.extend(projects.clone());
    let roots = project_roots(projects);
    let retained_roots = project_roots(&retained);
    let old_roots = project_roots(old_projects);
    let mut local_ids: HashSet<String> = projects.keys().cloned().collect();
    let mut old_ids: HashSet<String> = old_projects.keys().cloned().collect();
    old_ids.extend(
        known_ids
            .iter()
            .filter(|id| !projects.contains_key(*id))
            .cloned(),
    );
    let mut retained_ids: HashSet<String> = retained.keys().cloned().collect();
    let mut replacement = target.clone();
    replacement.insert(PROJECTS.into(), Value::Object(merged.clone()));

    // Host keys include CODEX_HOME in recent Codex versions (local:<path>).
    let mut hosts = object_field(target, MIGRATION_IDS)?
        .cloned()
        .unwrap_or_default();
    for (host, ids) in &mut hosts {
        if local_host(host) {
            let ids = ids.as_object_mut().unwrap();
            old_ids.extend(ids.values().map(|id| id.as_str().unwrap().to_owned()));
            ids.retain(|id, _| retained.contains_key(id));
            retained_ids.extend(ids.values().map(|id| id.as_str().unwrap().to_owned()));
        }
    }
    for (host, value) in object_field(source, MIGRATION_IDS)?.unwrap_or(&empty) {
        if local_host(host) {
            let ids = hosts
                .entry(host)
                .or_insert_with(|| json!({}))
                .as_object_mut()
                .unwrap();
            for (id, value) in value.as_object().unwrap() {
                if projects.contains_key(id) {
                    ids.insert(id.clone(), value.clone());
                    local_ids.insert(value.as_str().unwrap().to_owned());
                } else if known_ids.contains(id) {
                    old_ids.insert(value.as_str().unwrap().to_owned());
                }
            }
        }
    }
    replacement.insert(MIGRATION_IDS.into(), Value::Object(hosts));

    let mut saved_roots: Vec<Value> = string_array(source, "electron-saved-workspace-roots")?
        .into_iter()
        .filter(|path| roots.contains(*path))
        .map(|s| json!(s))
        .collect();
    saved_roots.extend(
        string_array(target, "electron-saved-workspace-roots")?
            .into_iter()
            .filter(|path| {
                (retained_roots.contains(*path) || !old_roots.contains(*path))
                    && !roots.contains(*path)
            })
            .map(|path| json!(path)),
    );
    replacement.insert(
        "electron-saved-workspace-roots".into(),
        Value::Array(saved_roots),
    );
    let mut labels: Map<String, Value> = object_field(target, "electron-workspace-root-labels")?
        .unwrap_or(&empty)
        .iter()
        .filter(|(path, _)| retained_roots.contains(*path) || !old_roots.contains(*path))
        .map(|(path, value)| (path.clone(), value.clone()))
        .collect();
    labels.extend(
        object_field(source, "electron-workspace-root-labels")?
            .unwrap_or(&empty)
            .iter()
            .filter(|(path, _)| roots.contains(*path))
            .map(|(path, value)| (path.clone(), value.clone())),
    );
    replacement.insert(
        "electron-workspace-root-labels".into(),
        Value::Object(labels),
    );
    for key in ["pinned-project-ids", "project-order"] {
        let mut ids: Vec<Value> = string_array(source, key)?
            .into_iter()
            .filter(|id| local_ids.contains(*id))
            .map(|s| json!(s))
            .collect();
        ids.extend(
            string_array(target, key)?
                .into_iter()
                .filter(|id| {
                    (retained_ids.contains(*id) || !old_ids.contains(*id))
                        && !local_ids.contains(*id)
                })
                .map(|s| json!(s)),
        );
        replacement.insert(key.into(), Value::Array(ids));
    }

    let mut assignments = object_field(target, ASSIGNMENTS)?
        .cloned()
        .unwrap_or_default();
    let mut memberships = object_field(target, MEMBERSHIP_HOSTS)?
        .cloned()
        .unwrap_or_default();
    let removed: Vec<String> = assignments
        .iter()
        .filter(|(thread, project)| {
            local_assignment_id(project).is_some_and(|id| {
                (old_ids.contains(id) && !retained_ids.contains(id)) || local_ids.contains(id)
            }) && local_membership(&memberships, thread)
        })
        .map(|(thread, _)| thread.clone())
        .collect();
    for thread in removed {
        assignments.remove(&thread);
        memberships.remove(&thread);
    }
    let source_memberships = object_field(source, MEMBERSHIP_HOSTS)?.unwrap_or(&empty);
    for (thread, project) in object_field(source, ASSIGNMENTS)?.unwrap_or(&empty) {
        if local_assignment_id(project).is_some_and(|id| local_ids.contains(id))
            && local_membership(source_memberships, thread)
            && local_membership(&memberships, thread)
            && assignments
                .get(thread)
                .map_or(true, |value| local_assignment_id(value).is_some())
        {
            assignments.insert(thread.clone(), project.clone());
            memberships.insert(
                thread.clone(),
                source_memberships
                    .get(thread)
                    .cloned()
                    .unwrap_or_else(|| json!("local")),
            );
        }
    }
    let mut migration = object_field(target, MIGRATION_STATE)?
        .cloned()
        .unwrap_or_default();
    migration.retain(|host, _| !local_host(host) || !retained.is_empty());
    let source_migration = object_field(source, MIGRATION_STATE)?.unwrap_or(&empty);
    for (host, value) in &mut migration {
        if local_host(host) && !projects.is_empty() && !source_migration.contains_key(host) {
            value["projectsMigrated"] = json!(false);
            value["threadAssignmentsMigrated"] = json!(false);
            value["threadAssignmentsReadMigrated"] = json!(false);
        }
    }
    for (host, value) in source_migration {
        if !local_host(host) {
            continue;
        }
        let mut value = value.clone();
        if let Some(previous) = migration.get(host) {
            for flag in [
                "projectsMigrated",
                "threadAssignmentsMigrated",
                "threadAssignmentsReadMigrated",
            ] {
                value[flag] = json!(value[flag] == true && previous[flag] == true);
            }
            let mut pending =
                string_array(value.as_object().unwrap(), "pendingThreadAssignmentIds")?
                    .into_iter()
                    .map(str::to_owned)
                    .collect::<Vec<_>>();
            for thread in string_array(previous.as_object().unwrap(), "pendingThreadAssignmentIds")?
            {
                if !pending.iter().any(|id| id == thread) {
                    pending.push(thread.to_owned());
                }
            }
            value["pendingThreadAssignmentIds"] = json!(pending);
        } else if !retained.is_empty() {
            value["projectsMigrated"] = json!(false);
            value["threadAssignmentsMigrated"] = json!(false);
            value["threadAssignmentsReadMigrated"] = json!(false);
        }
        migration.insert(host.clone(), value);
    }
    // Pending IDs may have no legacy assignment: Codex still needs to fetch
    // their app-server membership. Keep those local-host migration requests.
    replacement.insert(MIGRATION_STATE.into(), Value::Object(migration));
    replacement.insert(ASSIGNMENTS.into(), Value::Object(assignments));
    replacement.insert(MEMBERSHIP_HOSTS.into(), Value::Object(memberships));
    known_ids.extend(projects.keys().cloned());
    let mut source_known: Vec<_> = known_ids.iter().cloned().collect();
    source_known.sort();
    known_ids.extend(
        string_array(accounts, target_owner)?
            .into_iter()
            .map(str::to_owned),
    );
    known_ids.extend(merged.keys().cloned());
    let mut known_ids: Vec<_> = known_ids.into_iter().collect();
    known_ids.sort();
    baselines["accounts"][source_owner] = json!(source_known);
    baselines["accounts"][target_owner] = json!(known_ids);
    if &replacement == target {
        return Ok(false);
    }
    if target_is_current {
        *state.as_object_mut().unwrap() = replacement;
    } else {
        let inactive = state[ACCOUNT_STATE]["inactive"].as_object_mut().unwrap();
        let slot = inactive
            .entry(target_owner)
            .or_insert_with(|| json!({"atoms": {}, "globals": {}}));
        slot["globals"] = Value::Object(replacement);
    }
    Ok(true)
}

fn validate_baselines(baselines: &Value) -> Result<&Map<String, Value>> {
    if baselines.get("version").and_then(Value::as_u64) != Some(1) {
        bail!("Unsupported Switcher project baseline version; account switch cancelled");
    }
    let accounts = baselines
        .get("accounts")
        .and_then(Value::as_object)
        .context("Invalid Switcher project baselines; account switch cancelled")?;
    for owner in accounts.keys() {
        if owner.is_empty()
            || string_array(accounts, owner)?
                .iter()
                .any(|id| id.is_empty())
        {
            bail!("Invalid Switcher project baseline identity; account switch cancelled");
        }
    }
    Ok(accounts)
}

fn project_roots(projects: &Map<String, Value>) -> HashSet<String> {
    projects
        .values()
        .flat_map(|project| project["rootPaths"].as_array().unwrap())
        .map(|root| root.as_str().unwrap().to_owned())
        .collect()
}

fn local_assignment_id(assignment: &Value) -> Option<&str> {
    assignment.as_str().or_else(|| {
        (assignment.get("projectKind").and_then(Value::as_str) == Some("local")
            && assignment.get("projectOrigin").is_none())
        .then(|| assignment.get("projectId").and_then(Value::as_str))
        .flatten()
    })
}

fn slot_globals(slot: &Value) -> Result<&Map<String, Value>> {
    if !slot.get("atoms").is_some_and(Value::is_object) {
        bail!("Unsupported Codex account atom state format");
    }
    slot.get("globals")
        .and_then(Value::as_object)
        .context("Unsupported Codex account globals")
}

fn validate_local_fields(state: &Map<String, Value>) -> Result<()> {
    if let Some(projects) = object_field(state, PROJECTS)? {
        for (id, project) in projects {
            if project.get("id").and_then(Value::as_str) != Some(id)
                || !project.get("name").is_some_and(Value::is_string)
                || !project
                    .get("rootPaths")
                    .and_then(Value::as_array)
                    .is_some_and(|paths| paths.iter().all(Value::is_string))
            {
                bail!("Unsupported Codex local project record; account switch cancelled");
            }
        }
    }
    for key in [
        "electron-saved-workspace-roots",
        "pinned-project-ids",
        "project-order",
    ] {
        string_array(state, key)?;
    }
    for key in ["electron-workspace-root-labels", MEMBERSHIP_HOSTS] {
        if let Some(values) = object_field(state, key)? {
            if !values.values().all(Value::is_string) {
                bail!("Invalid Codex {key} entry; account switch cancelled");
            }
        }
    }
    if let Some(assignments) = object_field(state, ASSIGNMENTS)? {
        for assignment in assignments.values() {
            if !assignment.is_string()
                && !(assignment
                    .get("projectKind")
                    .and_then(Value::as_str)
                    .is_some_and(|kind| matches!(kind, "local" | "remote"))
                    && assignment.get("projectId").is_some_and(Value::is_string))
            {
                bail!("Invalid Codex thread project assignment; account switch cancelled");
            }
            if assignment.get("projectKind").and_then(Value::as_str) == Some("local")
                && assignment
                    .get("projectOrigin")
                    .is_some_and(|origin| origin.as_str() != Some("chatgpt"))
            {
                bail!("Unsupported Codex project origin; account switch cancelled");
            }
        }
    }
    for key in [MIGRATION_IDS, MIGRATION_STATE] {
        if let Some(hosts) = object_field(state, key)? {
            for (host, value) in hosts {
                if !local_host(host) {
                    continue;
                }
                let local = value
                    .as_object()
                    .context("Invalid local project migration record")?;
                if key == MIGRATION_IDS {
                    if !local
                        .values()
                        .all(|id| id.as_str().is_some_and(|id| !id.is_empty()))
                    {
                        bail!("Invalid local project migration ID; account switch cancelled");
                    }
                } else {
                    if local.get("version").and_then(Value::as_u64) != Some(1) {
                        bail!(
                            "Unsupported local project migration version; account switch cancelled"
                        );
                    }
                    for flag in [
                        "projectsMigrated",
                        "threadAssignmentsMigrated",
                        "threadAssignmentsReadMigrated",
                    ] {
                        if local.get(flag).is_some_and(|flag| !flag.is_boolean()) {
                            bail!(
                                "Invalid local project migration status; account switch cancelled"
                            );
                        }
                    }
                    string_array(local, "pendingThreadAssignmentIds")?;
                }
            }
        }
    }
    Ok(())
}

fn object_field<'a>(
    state: &'a Map<String, Value>,
    key: &str,
) -> Result<Option<&'a Map<String, Value>>> {
    state
        .get(key)
        .map(|value| {
            value
                .as_object()
                .with_context(|| format!("Invalid Codex {key} field"))
        })
        .transpose()
}

fn string_array<'a>(state: &'a Map<String, Value>, key: &str) -> Result<Vec<&'a str>> {
    let Some(value) = state.get(key) else {
        return Ok(Vec::new());
    };
    value
        .as_array()
        .with_context(|| format!("Invalid Codex {key} field"))?
        .iter()
        .map(|v| {
            v.as_str()
                .with_context(|| format!("Invalid Codex {key} entry"))
        })
        .collect()
}

fn local_host(host: &str) -> bool {
    host == "local" || host.starts_with("local:")
}

fn local_membership(hosts: &Map<String, Value>, thread: &str) -> bool {
    hosts
        .get(thread)
        .map_or(true, |host| host.as_str().is_some_and(local_host))
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).context("Could not read Switcher project baselines"),
    }
}

/// Back up before replacing the file, reject a concurrent desktop write, and
/// remove our own backup only after verifying the completed replacement.
pub fn write_project_state(path: &Path, original: &[u8], state: &Value) -> Result<()> {
    write_json_state(path, Some(original), state)
}

fn write_json_state(path: &Path, original: Option<&[u8]>, state: &Value) -> Result<()> {
    if read_optional(path)?.as_deref() != original {
        bail!("Codex project state changed; close Codex and retry the switch");
    }
    let contents = serde_json::to_vec_pretty(state)?;
    let suffix = uuid::Uuid::new_v4();
    let backup = path.with_extension(format!("switcher-projects-{suffix}.bak"));
    let temporary = path.with_extension(format!("switcher-projects-{suffix}.tmp"));
    let permissions = if original.is_some() {
        Some(fs::metadata(path)?.permissions())
    } else {
        None
    };
    let write_new = |file: &Path, bytes: &[u8]| -> Result<()> {
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut output = options.open(file)?;
        output.write_all(bytes)?;
        output.sync_all()?;
        if let Some(permissions) = &permissions {
            fs::set_permissions(file, permissions.clone())?;
        }
        Ok(())
    };
    if let Some(original) = original {
        write_new(&backup, original).context("Could not back up Codex project state")?;
    }
    let result = (|| {
        write_new(&temporary, &contents)?;
        if read_optional(path)?.as_deref() != original {
            bail!("Codex project state changed; close Codex and retry the switch");
        }
        fs::rename(&temporary, path).context("Could not replace Codex project state")?;
        if fs::read(path)? != contents {
            bail!("Could not verify Codex project state; recovery backup retained");
        }
        if original.is_some() {
            fs::remove_file(&backup).context("Could not remove verified project-state backup")?;
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::test_support::TestHome;
    use crate::types::TokenData;
    use serde_json::json;

    fn token(user: &str, account: Option<&str>) -> String {
        let payload = json!({"https://api.openai.com/auth": {
            "chatgpt_user_id": user,
            "chatgpt_account_id": account
        }});
        format!(
            "header.{}.signature",
            general_purpose::URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).unwrap())
        )
    }

    fn account(user: &str, workspace: &str) -> StoredAccount {
        StoredAccount::new_chatgpt(
            "Test".into(),
            None,
            None,
            None,
            token(user, Some(workspace)),
            "access".into(),
            "refresh".into(),
            Some(workspace.into()),
        )
    }

    fn current_auth(user: &str, workspace: &str) -> AuthDotJson {
        AuthDotJson {
            openai_api_key: None,
            tokens: Some(TokenData {
                id_token: token(user, Some(workspace)),
                access_token: "access".into(),
                refresh_token: "refresh".into(),
                account_id: Some(workspace.into()),
            }),
            last_refresh: None,
        }
    }

    fn state_file(bytes: &[u8]) -> std::path::PathBuf {
        let path = super::super::get_codex_home()
            .unwrap()
            .join(".codex-global-state.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, bytes).unwrap();
        path
    }

    fn fixture() -> Value {
        json!({
            "electron-account-renderer-state-v1": {
                "owner": "a",
                "inactive": {
                    "b": {"atoms": {"cloud-cache": "b-private"}, "globals": {
                        "local-projects": {"removed": {"id": "removed", "name": "Old", "rootPaths": ["/old"]}},
                        "pinned-project-ids": ["removed", "remote-project"],
                        "project-order": ["removed", "remote-project"],
                        "remote-projects": [{"id": "remote-project"}],
                        "thread-project-assignments": {"old-chat": "removed", "remote-chat": "remote-project"},
                        "thread-project-membership-host-ids": {"old-chat": "local", "remote-chat": "ssh:server"},
                        "thread-writable-roots": {"private-chat": ["/private"]},
                        "queued-follow-ups": {"private-chat": "Do not copy"},
                        "app-server-project-id-by-legacy-project-id-by-host": {"local:/codex": {"removed": "db-old"}, "ssh:server": {"remote-project": "db-remote"}}
                    }}
                }
            },
            "electron-persisted-atom-state": {"cloud-cache": "a-private"},
            "local-projects": {"p": {"id": "p", "name": "Renamed", "rootPaths": ["/project"]}},
            "electron-saved-workspace-roots": ["/project", "/unrelated"],
            "electron-workspace-root-labels": {"/project": "Label", "/unrelated": "Private"},
            "pinned-project-ids": ["p", "cloud-project"],
            "project-order": ["p"],
            "thread-project-assignments": {"local-chat": "p", "remote-chat-a": "p"},
            "thread-project-membership-host-ids": {"local-chat": "local", "remote-chat-a": "ssh:other"},
            "app-server-project-id-by-legacy-project-id-by-host": {"local:/codex": {"p": "db-p"}, "ssh:other": {"remote": "db-other"}},
            "app-server-projects-migration-by-host": {"local:/codex": {"version": 1, "projectsMigrated": true}, "ssh:other": {"version": 1}},
            "thread-writable-roots": {"local-chat": ["/extra-permission"]}
        })
    }

    #[test]
    fn new_account_keeps_projects_without_copying_private_state() {
        let mut state = fixture();
        let before = state.clone();
        assert!(sync_local_projects(&mut state, "a", "new").unwrap());
        let slot = &state["electron-account-renderer-state-v1"]["inactive"]["new"];
        assert_eq!(slot["atoms"], json!({}));
        assert_eq!(slot["globals"]["local-projects"], before["local-projects"]);
        assert_eq!(slot["globals"]["pinned-project-ids"], json!(["p"]));
        assert_eq!(
            slot["globals"]["thread-project-assignments"],
            json!({"local-chat":"p"})
        );
        assert!(slot["globals"].get("thread-writable-roots").is_none());
        assert_eq!(
            state["electron-persisted-atom-state"],
            before["electron-persisted-atom-state"]
        );
        assert_eq!(
            state["electron-account-renderer-state-v1"]["inactive"]["b"],
            before["electron-account-renderer-state-v1"]["inactive"]["b"]
        );
    }

    #[test]
    fn first_share_preserves_independent_target_projects() {
        let mut state = fixture();
        state[MIGRATION_STATE]["local:/codex"]["pendingThreadAssignmentIds"] =
            json!(["source-awaiting-server"]);
        let target = &mut state[ACCOUNT_STATE]["inactive"]["b"]["globals"];
        target["electron-saved-workspace-roots"] = json!(["/old", "/private"]);
        target["electron-workspace-root-labels"] =
            json!({"/old":"Old label", "/private":"Private"});
        target[ASSIGNMENTS]["old-chat"] = json!({"projectKind":"local", "projectId":"db-old"});
        target[MIGRATION_STATE] = json!({"local:/codex": {
            "version":1, "projectsMigrated":true, "threadAssignmentsMigrated":false,
            "pendingThreadAssignmentIds":["old-chat", "target-awaiting-server"]
        }});
        let before = state[ACCOUNT_STATE]["inactive"]["b"]["globals"].clone();
        let mut baselines = json!({"version":1,"accounts":{}});
        sync_local_projects_with_baselines(&mut state, &mut baselines, "a", "b").unwrap();
        let target = &state[ACCOUNT_STATE]["inactive"]["b"]["globals"];
        assert_eq!(target[PROJECTS]["removed"], before[PROJECTS]["removed"]);
        assert_eq!(
            target[ASSIGNMENTS]["old-chat"],
            before[ASSIGNMENTS]["old-chat"]
        );
        assert_eq!(target[MIGRATION_IDS]["local:/codex"]["removed"], "db-old");
        assert_eq!(
            target["electron-saved-workspace-roots"],
            json!(["/project", "/old", "/private"])
        );
        assert_eq!(
            target["electron-workspace-root-labels"],
            json!({"/project":"Label", "/old":"Old label", "/private":"Private"})
        );
        assert_eq!(
            target["pinned-project-ids"],
            json!(["p", "removed", "remote-project"])
        );
        assert_eq!(
            target["project-order"],
            json!(["p", "removed", "remote-project"])
        );
        assert_eq!(
            target[MIGRATION_STATE]["local:/codex"]["pendingThreadAssignmentIds"],
            json!([
                "source-awaiting-server",
                "old-chat",
                "target-awaiting-server"
            ])
        );
        assert_eq!(
            target[MIGRATION_STATE]["local:/codex"]["threadAssignmentsMigrated"],
            false
        );
        assert_eq!(baselines["accounts"]["a"], json!(["p"]));
        assert_eq!(baselines["accounts"]["b"], json!(["p", "removed"]));
        // Authentication can fail after preparation; retry must not delete a
        // project the current source account has never had the chance to see.
        let prepared = state.clone();
        assert!(!sync_local_projects_with_baselines(&mut state, &mut baselines, "a", "b").unwrap());
        assert_eq!(state, prepared);
        // Still on A after a failed authentication, the user deletes p.
        // Preserve B's unseen independent project but propagate A's deletion.
        state[PROJECTS] = json!({});
        sync_local_projects_with_baselines(&mut state, &mut baselines, "a", "b").unwrap();
        assert_eq!(
            state[ACCOUNT_STATE]["inactive"]["b"]["globals"][PROJECTS],
            before[PROJECTS]
        );
    }

    #[test]
    fn desktop_object_assignments_are_copied_without_remote_memberships() {
        let mut state = fixture();
        state[ASSIGNMENTS] = json!({
            "local-chat":{"projectKind":"local","projectId":"p"},
            "remote-chat":{"projectKind":"remote","projectId":"p"},
            "cloud-chat":{"projectKind":"local","projectId":"p","projectOrigin":"chatgpt"}
        });
        sync_local_projects(&mut state, "a", "new").unwrap();
        assert_eq!(
            state[ACCOUNT_STATE]["inactive"]["new"]["globals"][ASSIGNMENTS],
            json!({"local-chat":{"projectKind":"local","projectId":"p"}})
        );
        let cloud = json!({"projectKind":"local","projectId":"p","projectOrigin":"chatgpt"});
        state[ACCOUNT_STATE]["inactive"]["b"]["globals"][ASSIGNMENTS]["local-chat"] = cloud.clone();
        sync_local_projects(&mut state, "a", "b").unwrap();
        assert_eq!(
            state[ACCOUNT_STATE]["inactive"]["b"]["globals"][ASSIGNMENTS]["local-chat"],
            cloud
        );
    }

    #[test]
    fn known_deleted_projects_clean_stale_assignments_and_order() {
        let mut state = fixture();
        state[MIGRATION_IDS]["local:/codex"]["removed"] = json!("source-db-old");
        let target = &mut state[ACCOUNT_STATE]["inactive"]["b"]["globals"];
        target[PROJECTS] = json!({});
        target["pinned-project-ids"] =
            json!(["removed", "db-old", "source-db-old", "remote-project"]);
        target["project-order"] = target["pinned-project-ids"].clone();
        target[ASSIGNMENTS]["alias-chat"] = json!({"projectKind":"local", "projectId":"db-old"});
        target[ASSIGNMENTS]["source-alias-chat"] =
            json!({"projectKind":"local", "projectId":"source-db-old"});
        let mut baselines = json!({"version":1,"accounts":{"a":["p","removed"]}});
        sync_local_projects_with_baselines(&mut state, &mut baselines, "a", "b").unwrap();
        let target = &state[ACCOUNT_STATE]["inactive"]["b"]["globals"];
        assert_eq!(
            target[ASSIGNMENTS],
            json!({"local-chat":"p", "remote-chat":"remote-project"})
        );
        assert_eq!(target["pinned-project-ids"], json!(["p", "remote-project"]));
        assert_eq!(target["project-order"], json!(["p", "remote-project"]));
    }

    #[test]
    fn independent_target_assignments_are_still_read_by_desktop_migration() {
        let mut state = fixture();
        state[MIGRATION_STATE]["local:/codex"]["threadAssignmentsReadMigrated"] = json!(true);
        state[ACCOUNT_STATE]["inactive"]["b"]["globals"][MIGRATION_STATE] = json!({
            "local:/codex":{"version":1,"threadAssignmentsReadMigrated":false,
                "pendingThreadAssignmentIds":[]}
        });
        sync_local_projects(&mut state, "a", "b").unwrap();
        assert_eq!(
            state[ACCOUNT_STATE]["inactive"]["b"]["globals"][MIGRATION_STATE]["local:/codex"]
                ["threadAssignmentsReadMigrated"],
            false
        );
        // Conversely, a complete target migration must not hide new source
        // assignments when the source has no corresponding migration record.
        let mut state = fixture();
        state[MIGRATION_STATE] = json!({});
        state[ACCOUNT_STATE]["inactive"]["b"]["globals"][MIGRATION_STATE] = json!({
            "local:/codex":{"version":1,"projectsMigrated":true,
                "threadAssignmentsMigrated":true,"threadAssignmentsReadMigrated":true}
        });
        sync_local_projects(&mut state, "a", "b").unwrap();
        for flag in [
            "projectsMigrated",
            "threadAssignmentsMigrated",
            "threadAssignmentsReadMigrated",
        ] {
            assert_eq!(
                state[ACCOUNT_STATE]["inactive"]["b"]["globals"][MIGRATION_STATE]["local:/codex"]
                    [flag],
                false
            );
        }
    }

    #[test]
    fn latest_projects_replace_deleted_and_renamed_projects_only() {
        let mut state = fixture();
        let before = state.clone();
        let mut baselines = json!({"version":1,"accounts":{"a":["p","removed"]}});
        assert!(sync_local_projects_with_baselines(&mut state, &mut baselines, "a", "b").unwrap());
        let b = &state["electron-account-renderer-state-v1"]["inactive"]["b"];
        let old = &before["electron-account-renderer-state-v1"]["inactive"]["b"];
        assert_eq!(b["globals"]["local-projects"], before["local-projects"]);
        assert_eq!(
            b["globals"]["pinned-project-ids"],
            json!(["p", "remote-project"])
        );
        assert_eq!(
            b["globals"]["electron-saved-workspace-roots"],
            json!(["/project"])
        );
        assert_eq!(
            b["globals"]["electron-workspace-root-labels"],
            json!({"/project":"Label"})
        );
        assert_eq!(
            b["globals"]["thread-project-assignments"],
            json!({"local-chat":"p", "remote-chat":"remote-project"})
        );
        assert_eq!(
            b["globals"]["app-server-project-id-by-legacy-project-id-by-host"],
            json!({"local:/codex":{"p":"db-p"},"ssh:server":{"remote-project":"db-remote"}})
        );
        for key in [
            "remote-projects",
            "thread-writable-roots",
            "queued-follow-ups",
        ] {
            assert_eq!(b["globals"][key], old["globals"][key]);
        }
        assert_eq!(b["atoms"], old["atoms"]);
        assert!(!sync_local_projects_with_baselines(&mut state, &mut baselines, "a", "b").unwrap());
    }

    #[test]
    fn explicitly_empty_source_does_not_resurrect_removed_projects() {
        let mut state = fixture();
        state["local-projects"] = json!({});
        let mut baselines = json!({"version":1,"accounts":{"a":["p","removed"]}});
        sync_local_projects_with_baselines(&mut state, &mut baselines, "a", "b").unwrap();
        let b = &state["electron-account-renderer-state-v1"]["inactive"]["b"]["globals"];
        assert_eq!(b["local-projects"], json!({}));
        assert_eq!(b["pinned-project-ids"], json!(["remote-project"]));
        assert!(b["thread-project-assignments"].get("old-chat").is_none());
    }

    #[test]
    fn deletion_propagates_through_three_accounts_and_readdition_wins() {
        let mut state = fixture();
        state[ACCOUNT_STATE]["inactive"]["c"] = state[ACCOUNT_STATE]["inactive"]["b"].clone();
        let mut baselines = json!({"version":1,"accounts":{"a":["removed","p"]}});
        sync_local_projects_with_baselines(&mut state, &mut baselines, "a", "b").unwrap();
        sync_local_projects_with_baselines(&mut state, &mut baselines, "b", "c").unwrap();
        assert!(state[ACCOUNT_STATE]["inactive"]["c"]["globals"][PROJECTS]
            .get("removed")
            .is_none());
        assert_eq!(baselines["accounts"]["c"], json!(["p", "removed"]));
        state[ACCOUNT_STATE]["inactive"]["c"]["globals"][PROJECTS]["removed"] =
            json!({"id":"removed", "name":"Added again", "rootPaths":["/new"]});
        sync_local_projects_with_baselines(&mut state, &mut baselines, "c", "a").unwrap();
        assert_eq!(state[PROJECTS]["removed"]["name"], "Added again");
    }

    #[test]
    fn malformed_baselines_leave_both_inputs_unchanged() {
        for mut baselines in [
            json!({"version":2,"accounts":{}}),
            json!({"version":1,"accounts":{"a":[42]}}),
            json!({"version":1,"accounts":[]}),
        ] {
            let mut state = fixture();
            let previous = state.clone();
            let old_baselines = baselines.clone();
            assert!(
                sync_local_projects_with_baselines(&mut state, &mut baselines, "a", "b").is_err()
            );
            assert_eq!(state, previous);
            assert_eq!(baselines, old_baselines);
        }
    }

    #[test]
    fn source_can_be_archived_at_shutdown_and_target_can_be_current() {
        let mut state = fixture();
        sync_local_projects(&mut state, "a", "b").unwrap();
        state["electron-account-renderer-state-v1"]["owner"] = Value::Null;
        state.as_object_mut().unwrap().remove("local-projects");
        sync_local_projects(&mut state, "b", "new").unwrap();
        assert_eq!(
            state["electron-account-renderer-state-v1"]["inactive"]["new"]["globals"]
                ["local-projects"]["p"]["name"],
            "Renamed"
        );
        state["electron-account-renderer-state-v1"]["owner"] = json!("a");
        sync_local_projects(&mut state, "b", "a").unwrap();
        assert_eq!(state["local-projects"]["p"]["name"], "Renamed");
    }

    #[test]
    fn legacy_and_unknown_states_do_not_silently_destroy_data() {
        let mut legacy = json!({"local-projects":{"p":{}}});
        let before = legacy.clone();
        assert!(!sync_local_projects(&mut legacy, "a", "b").unwrap());
        assert_eq!(legacy, before);
        let mut malformed = fixture();
        malformed["electron-account-renderer-state-v1"]["inactive"] = json!([]);
        let before = malformed.clone();
        assert!(sync_local_projects(&mut malformed, "a", "b").is_err());
        assert_eq!(malformed, before);
        let mut missing_source = fixture();
        assert!(sync_local_projects(&mut missing_source, "unknown", "b").is_err());
    }

    #[test]
    fn unsupported_local_records_fail_without_partial_mutations() {
        let malformed_fields = [
            (MIGRATION_IDS, json!({"local:/codex":{"p":42}})),
            (MIGRATION_IDS, json!({"local:/codex":{"p":{"id":"db-p"}}})),
            (MIGRATION_STATE, json!({"local:/codex":false})),
            (MIGRATION_STATE, json!({"local:/codex":{"version":2}})),
            (
                MIGRATION_STATE,
                json!({"local:/codex":{"version":1,"projectsMigrated":"yes"}}),
            ),
            (
                PROJECTS,
                json!({"p":{"id":"other", "name":"P", "rootPaths":[]}}),
            ),
            (ASSIGNMENTS, json!({"chat": {"projectId":"p"}})),
        ];
        for (key, value) in malformed_fields {
            for target in [false, true] {
                let mut state = fixture();
                if target {
                    state[ACCOUNT_STATE]["inactive"]["b"]["globals"][key] = value.clone();
                } else {
                    state[key] = value.clone();
                }
                let before = state.clone();
                assert!(
                    sync_local_projects(&mut state, "a", "b").is_err(),
                    "{key}, target={target}"
                );
                assert_eq!(state, before);
            }
        }
    }

    #[test]
    fn identity_hash_matches_desktop_and_rejects_unknown_users() {
        let expected = "ec722400cd9960baf62a0fbe8b3266286ac8feae6873e6da8a5a5c42ba1cc168";
        assert_eq!(
            renderer_owner(&token("user-a", Some("workspace-a")), Some("ignored")).unwrap(),
            expected
        );
        assert_eq!(
            renderer_owner(&token("user-a", None), Some("workspace-a")).unwrap(),
            expected
        );
        assert!(renderer_owner("not-a-token", None).is_err());
        assert!(renderer_owner(&token("", Some("workspace-a")), None).is_err());
        assert!(renderer_owner(&token("user-a", None), None).is_err());
    }

    #[test]
    fn file_replacement_is_verified_and_does_not_accumulate_backups() {
        let _home = TestHome::new();
        let original = serde_json::to_vec(&fixture()).unwrap();
        let path = state_file(&original);
        let mut changed: Value = serde_json::from_slice(&original).unwrap();
        sync_local_projects(&mut changed, "a", "b").unwrap();
        write_project_state(&path, &original, &changed).unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&fs::read(&path).unwrap()).unwrap(),
            changed
        );
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
    }

    #[test]
    fn concurrent_file_change_is_preserved_and_switch_fails() {
        let _home = TestHome::new();
        let path = state_file(b"{\"newer\":true}");
        assert!(write_project_state(&path, b"{}", &fixture()).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"{\"newer\":true}");
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
    }

    #[test]
    fn full_file_flow_uses_current_identity_and_preserves_credentials() {
        let _home = TestHome::new();
        let source = renderer_owner(&token("user-a", Some("workspace-a")), None).unwrap();
        let target = renderer_owner(&token("user-b", Some("workspace-b")), None).unwrap();
        let mut state = fixture();
        state[ACCOUNT_STATE]["owner"] = json!(source);
        let path = state_file(&serde_json::to_vec(&state).unwrap());
        let auth_path = path.parent().unwrap().join("auth.json");
        fs::write(&auth_path, b"unchanged-credentials").unwrap();
        preserve_local_projects(
            Some(&current_auth("user-a", "workspace-a")),
            &account("user-b", "workspace-b"),
        )
        .unwrap();
        let result: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert_eq!(
            result[ACCOUNT_STATE]["inactive"][target]["globals"][PROJECTS],
            state[PROJECTS]
        );
        assert_eq!(fs::read(auth_path).unwrap(), b"unchanged-credentials");
    }

    #[test]
    fn legacy_and_api_key_transitions_do_not_touch_state() {
        let _home = TestHome::new();
        let legacy = b"{\"local-projects\":{}}";
        let path = state_file(legacy);
        let current = current_auth("user-a", "workspace-a");
        preserve_local_projects(Some(&current), &account("user-b", "workspace-b")).unwrap();
        assert_eq!(fs::read(&path).unwrap(), legacy);
        fs::write(&path, b"not-json").unwrap();
        preserve_local_projects(
            Some(&current),
            &StoredAccount::new_api_key("key".into(), "dummy".into()),
        )
        .unwrap();
        let api_auth = AuthDotJson {
            openai_api_key: Some("dummy".into()),
            tokens: None,
            last_refresh: None,
        };
        preserve_local_projects(Some(&api_auth), &account("user-b", "workspace-b")).unwrap();
        assert_eq!(fs::read(path).unwrap(), b"not-json");
    }

    #[test]
    fn invalid_state_or_identity_leaves_file_untouched() {
        let _home = TestHome::new();
        let path = state_file(b"{truncated");
        let current = current_auth("user-a", "workspace-a");
        assert!(
            preserve_local_projects(Some(&current), &account("user-b", "workspace-b")).is_err()
        );
        assert_eq!(fs::read(&path).unwrap(), b"{truncated");
        let original = serde_json::to_vec(&fixture()).unwrap();
        fs::write(&path, &original).unwrap();
        assert!(preserve_local_projects(Some(&current), &account("", "workspace-b")).is_err());
        assert_eq!(fs::read(path).unwrap(), original);
    }

    #[cfg(unix)]
    #[test]
    fn replacement_preserves_restrictive_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let _home = TestHome::new();
        let path = state_file(b"{}");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        write_project_state(&path, b"{}", &fixture()).unwrap();
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
