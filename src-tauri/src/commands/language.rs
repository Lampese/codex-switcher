//! English/Turkish interface language shared by the webviews and native menus.

use std::{
    fs,
    io::Write,
    sync::{
        atomic::{AtomicBool, Ordering},
        LazyLock, Mutex,
    },
};
use tauri::Emitter;

const LANGUAGE_FILE: &str = "ui-language";

// English is the default; Turkish is used only after it was explicitly stored.
static TURKISH: LazyLock<AtomicBool> = LazyLock::new(|| {
    let stored = crate::auth::get_config_dir()
        .ok()
        .and_then(|dir| fs::read_to_string(dir.join(LANGUAGE_FILE)).ok());
    AtomicBool::new(is_turkish(stored.as_deref()))
});
static SAVE_LOCK: Mutex<()> = Mutex::new(());

fn is_turkish(stored: Option<&str>) -> bool {
    stored.is_some_and(|language| language.trim() == "tr")
}

pub fn text<'a>(english: &'a str, turkish: &'a str) -> &'a str {
    if TURKISH.load(Ordering::Relaxed) {
        turkish
    } else {
        english
    }
}

fn valid_language(language: &str) -> Result<&str, &'static str> {
    match language {
        "tr" | "en" => Ok(language),
        _ => Err("Unsupported interface language"),
    }
}

#[tauri::command]
pub fn get_ui_language() -> &'static str {
    text("en", "tr")
}

#[tauri::command]
pub fn set_ui_language(app: tauri::AppHandle, language: String) -> Result<(), String> {
    valid_language(&language).map_err(str::to_owned)?;
    let _guard = SAVE_LOCK
        .lock()
        .map_err(|_| "Interface language settings unavailable")?;
    let dir = crate::auth::get_config_dir().map_err(|e| e.to_string())?;
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let temp = dir.join(format!("{LANGUAGE_FILE}-{}.tmp", uuid::Uuid::new_v4()));
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let written = options.open(&temp).and_then(|mut file| {
        file.write_all(language.as_bytes())?;
        file.sync_all()
    });
    if let Err(error) = written.and_then(|()| fs::rename(&temp, dir.join(LANGUAGE_FILE))) {
        let _ = fs::remove_file(&temp);
        return Err(error.to_string());
    }
    let changed = TURKISH.swap(language == "tr", Ordering::Relaxed) != (language == "tr");
    if changed {
        crate::app_menu::refresh(&app).map_err(|e| e.to_string())?;
        crate::tray::refresh(&app);
    }
    app.emit("language-changed", language)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_setting_accepts_only_supported_languages() {
        assert_eq!(valid_language("tr").unwrap(), "tr");
        assert_eq!(valid_language("en").unwrap(), "en");
        assert!(valid_language("de").is_err());
        assert!(valid_language("tr/../../").is_err());
    }

    #[test]
    fn english_is_default_unless_turkish_was_stored() {
        assert!(!is_turkish(None));
        assert!(!is_turkish(Some("")));
        assert!(!is_turkish(Some("en")));
        assert!(!is_turkish(Some("de")));
        assert!(is_turkish(Some("tr")));
        assert!(is_turkish(Some("tr\n")));
    }
}
