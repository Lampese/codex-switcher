//! Instance management Tauri commands

use crate::auth::instance_manager::{
    self, InstanceProfile,
};

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
    instance_manager::set_active_instance(&instance_id).map_err(|e| e.to_string())
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
    instance_manager::bind_account(&instance_id, account_id).map_err(|e| e.to_string())
}
