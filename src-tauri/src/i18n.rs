use std::{collections::HashMap, sync::LazyLock};

use include_dir::{include_dir, Dir};
use serde::Deserialize;

use crate::types::AppLanguage;

static LOCALES_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../locales");

#[derive(Debug, Deserialize)]
pub struct LocaleDefinition {
    pub code: String,
    pub label: String,
}

static MANIFEST: LazyLock<Vec<LocaleDefinition>> = LazyLock::new(|| {
    let contents = LOCALES_DIR
        .get_file("manifest.json")
        .expect("locales/manifest.json must be embedded")
        .contents_utf8()
        .expect("locales/manifest.json must be UTF-8");
    serde_json::from_str(contents).expect("locales/manifest.json must be valid JSON")
});

static NATIVE_TRANSLATIONS: LazyLock<HashMap<String, HashMap<String, String>>> =
    LazyLock::new(|| {
        MANIFEST
            .iter()
            .map(|locale| {
                let path = format!("{}/native.json", locale.code);
                let contents = LOCALES_DIR
                    .get_file(&path)
                    .unwrap_or_else(|| panic!("{path} must be embedded"))
                    .contents_utf8()
                    .unwrap_or_else(|| panic!("{path} must be UTF-8"));
                let translations = serde_json::from_str(contents)
                    .unwrap_or_else(|error| panic!("{path} must be valid JSON: {error}"));
                (locale.code.clone(), translations)
            })
            .collect()
    });

pub fn supported_languages() -> &'static [LocaleDefinition] {
    &MANIFEST
}

pub fn is_supported(language: &AppLanguage) -> bool {
    if language.as_str() == AppLanguage::SYSTEM_CODE {
        return true;
    }
    MANIFEST
        .iter()
        .any(|locale| locale.code == language.as_str())
}

fn match_locale_code(locale: Option<&str>) -> &'static str {
    let Some(locale) = locale else {
        return AppLanguage::DEFAULT_CODE;
    };
    let normalized = locale.replace('_', "-").to_ascii_lowercase();

    if let Some(exact) = MANIFEST
        .iter()
        .find(|item| item.code.to_ascii_lowercase() == normalized)
    {
        return exact.code.as_str();
    }

    // Do not map Traditional Chinese locales to Simplified Chinese.
    if normalized.starts_with("zh-")
        && !normalized.starts_with("zh-cn")
        && !normalized.starts_with("zh-sg")
        && !normalized.starts_with("zh-hans")
    {
        return AppLanguage::DEFAULT_CODE;
    }

    let primary = normalized.split('-').next().unwrap_or_default();
    MANIFEST
        .iter()
        .find(|item| {
            item.code
                .split('-')
                .next()
                .is_some_and(|code| code.eq_ignore_ascii_case(primary))
        })
        .map(|item| item.code.as_str())
        .unwrap_or(AppLanguage::DEFAULT_CODE)
}

pub fn resolved_code(language: &AppLanguage) -> &'static str {
    if language.as_str() == AppLanguage::SYSTEM_CODE {
        return match_locale_code(sys_locale::get_locale().as_deref());
    }
    MANIFEST
        .iter()
        .find(|item| item.code == language.as_str())
        .map(|item| item.code.as_str())
        .unwrap_or(AppLanguage::DEFAULT_CODE)
}

pub fn text<'a>(language: &AppLanguage, key: &'a str) -> &'a str {
    NATIVE_TRANSLATIONS
        .get(resolved_code(language))
        .and_then(|translations| translations.get(key))
        .or_else(|| {
            NATIVE_TRANSLATIONS
                .get(AppLanguage::DEFAULT_CODE)
                .and_then(|translations| translations.get(key))
        })
        .map(String::as_str)
        .unwrap_or(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_manifest_native_catalogs_load() {
        assert!(!NATIVE_TRANSLATIONS[AppLanguage::DEFAULT_CODE].is_empty());
        for locale in supported_languages() {
            assert!(NATIVE_TRANSLATIONS.contains_key(locale.code.as_str()));
        }
    }

    #[test]
    fn manifest_languages_are_available() {
        assert!(is_supported(&AppLanguage::new("system")));
        assert!(is_supported(&AppLanguage::new("en-US")));
        assert!(is_supported(&AppLanguage::new("zh-CN")));
        assert!(!is_supported(&AppLanguage::new("missing")));
    }

    #[test]
    fn system_locale_falls_back_to_english() {
        assert_eq!(match_locale_code(Some("xx-ZZ")), "en-US");
        assert_eq!(match_locale_code(None), "en-US");
    }

    #[test]
    fn system_locale_matches_supported_variants() {
        assert_eq!(match_locale_code(Some("zh_CN")), "zh-CN");
        assert_eq!(match_locale_code(Some("zh-Hans-CN")), "zh-CN");
        assert_eq!(match_locale_code(Some("zh-TW")), "en-US");
        assert_eq!(match_locale_code(Some("en-GB")), "en-US");
    }

    #[test]
    fn unknown_keys_fall_back_to_the_key() {
        assert_eq!(
            text(&AppLanguage::new("zh-CN"), "missing.key"),
            "missing.key"
        );
    }
}
