use tauri::{AppHandle, Emitter};

use crate::{
    auth::{load_app_settings, save_app_settings},
    types::AppLanguage,
};

pub const LANGUAGE_CHANGED_EVENT: &str = "language-changed";

#[tauri::command]
pub fn get_app_language() -> AppLanguage {
    load_app_settings().unwrap_or_default().language
}

#[tauri::command]
pub fn set_app_language(app: AppHandle, language: AppLanguage) -> Result<AppLanguage, String> {
    let language = save_language(language)?;

    #[cfg(desktop)]
    {
        if let Err(error) = crate::app_menu::refresh(&app) {
            eprintln!("Failed to refresh app menu after language change: {error}");
        }
        crate::tray::refresh(&app);
    }

    let _ = app.emit(LANGUAGE_CHANGED_EVENT, language.clone());
    Ok(language)
}

pub fn save_language(language: AppLanguage) -> Result<AppLanguage, String> {
    if !crate::i18n::is_supported(&language) {
        return Err(format!("Unsupported app language: {}", language.as_str()));
    }

    let mut settings = load_app_settings().unwrap_or_default();
    if settings.language != language {
        settings.language = language.clone();
        save_app_settings(&settings).map_err(|error| error.to_string())?;
    }
    Ok(language)
}
