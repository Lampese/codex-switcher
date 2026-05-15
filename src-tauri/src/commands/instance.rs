//! Instance management Tauri commands

use std::path::PathBuf;

use crate::auth::instance_manager::{self, InstanceProfile};
use crate::auth::{get_account, set_active_account, switch_to_account_in_dir, touch_account};

/// List all Codex instances
#[tauri::command]
pub fn list_instances() -> Result<Vec<InstanceProfile>, String> {
    instance_manager::list_instances().map_err(|e| e.to_string())
}

/// Create a new Codex instance (copies from default ~/.codex/)
#[tauri::command]
pub fn create_instance(name: String, user_data_dir: String) -> Result<InstanceProfile, String> {
    instance_manager::create_instance(name, user_data_dir).map_err(|e| e.to_string())
}

/// Create an empty Codex instance
#[tauri::command]
pub fn create_empty_instance(
    name: String,
    user_data_dir: String,
) -> Result<InstanceProfile, String> {
    instance_manager::create_empty_instance(name, user_data_dir).map_err(|e| e.to_string())
}

/// Set the active instance (sets CODEX_HOME env var)
#[tauri::command]
pub fn set_active_instance(instance_id: String) -> Result<InstanceProfile, String> {
    activate_instance(&instance_id)
}

/// Get the currently active instance
#[tauri::command]
pub fn get_active_instance() -> Result<Option<InstanceProfile>, String> {
    instance_manager::get_active_instance().map_err(|e| e.to_string())
}

/// Remove an instance
#[tauri::command]
pub fn remove_instance(instance_id: String, delete_data: bool) -> Result<(), String> {
    instance_manager::remove_instance(&instance_id, delete_data).map_err(|e| e.to_string())
}

/// Bind an account to an instance
#[tauri::command]
pub fn bind_instance_account(
    instance_id: String,
    account_id: Option<String>,
) -> Result<InstanceProfile, String> {
    let updated =
        instance_manager::bind_account(&instance_id, account_id).map_err(|e| e.to_string())?;
    let is_active = instance_manager::get_active_instance()
        .map_err(|e| e.to_string())?
        .is_some_and(|instance| instance.id == updated.id);

    apply_bound_account_to_instance(&updated, is_active)?;
    Ok(updated)
}

#[tauri::command]
pub fn get_instance_launch_command(instance_id: String) -> Result<String, String> {
    instance_manager::get_instance_launch_command(&instance_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn launch_instance_codex(instance_id: String) -> Result<(), String> {
    let instance = activate_instance(&instance_id)?;
    instance_manager::launch_codex_for_instance(&instance).map_err(|e| e.to_string())
}

fn activate_instance(instance_id: &str) -> Result<InstanceProfile, String> {
    let instance = instance_manager::set_active_instance(instance_id).map_err(|e| e.to_string())?;
    apply_bound_account_to_instance(&instance, true)?;
    Ok(instance)
}

fn apply_bound_account_to_instance(
    instance: &InstanceProfile,
    mark_account_active: bool,
) -> Result<(), String> {
    let Some(account_id) = &instance.bind_account_id else {
        return Ok(());
    };

    let account = get_account(account_id)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("Bound account not found: {account_id}"))?;
    let codex_home = PathBuf::from(&instance.user_data_dir);

    switch_to_account_in_dir(&account, &codex_home).map_err(|e| e.to_string())?;

    if mark_account_active {
        set_active_account(account_id).map_err(|e| e.to_string())?;
        touch_account(account_id).map_err(|e| e.to_string())?;
    }

    Ok(())
}
