use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::{
    auth::{load_app_settings, save_app_settings},
    types::AppLanguage,
};

pub const LANGUAGE_CHANGED_EVENT: &str = "language-changed";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppLanguageState {
    pub preference: AppLanguage,
    pub resolved_code: String,
}

fn language_state(preference: AppLanguage) -> AppLanguageState {
    let resolved_code = crate::i18n::resolved_code(&preference).to_string();
    AppLanguageState {
        preference,
        resolved_code,
    }
}

#[tauri::command]
pub fn get_app_language() -> AppLanguageState {
    language_state(load_app_settings().unwrap_or_default().language)
}

#[tauri::command]
pub fn set_app_language(app: AppHandle, language: AppLanguage) -> Result<AppLanguageState, String> {
    let state = save_language(language)?;

    #[cfg(desktop)]
    {
        if let Err(error) = crate::app_menu::refresh(&app) {
            eprintln!("Failed to refresh app menu after language change: {error}");
        }
        crate::tray::refresh(&app);
    }

    let _ = app.emit(LANGUAGE_CHANGED_EVENT, state.clone());
    Ok(state)
}

pub fn save_language(language: AppLanguage) -> Result<AppLanguageState, String> {
    if !crate::i18n::is_supported(&language) {
        return Err(format!("Unsupported app language: {}", language.as_str()));
    }

    let mut settings = load_app_settings().unwrap_or_default();
    if settings.language != language {
        settings.language = language.clone();
        save_app_settings(&settings).map_err(|error| error.to_string())?;
    }
    Ok(language_state(language))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_language_state_keeps_preference_and_resolved_code() {
        let state = language_state(AppLanguage::new("zh-CN"));
        assert_eq!(state.preference.as_str(), "zh-CN");
        assert_eq!(state.resolved_code, "zh-CN");

        let serialized = serde_json::to_value(state).unwrap();
        assert_eq!(serialized["preference"], "zh-CN");
        assert_eq!(serialized["resolvedCode"], "zh-CN");
    }
}
