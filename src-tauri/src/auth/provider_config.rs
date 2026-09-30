//! Minimal, reversible edits to the user's Codex provider selection.
use crate::types::CustomProvider;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use toml_edit::{value, DocumentMut, Item, Table};

pub(super) const SNAPSHOT_FILE: &str = ".codex-switcher-provider.json";
const KEYS: &[&str] = &[
    "model",
    "model_provider",
    "review_model",
    "profile",
    "openai_base_url",
    "cli_auth_credentials_store",
];

#[derive(Serialize, Deserialize)]
struct Snapshot {
    original_config: String,
    provider_id: String,
}

/// Returns new config and snapshot bytes. None means remove the corresponding file.
pub(super) fn prepare(
    config: Option<&[u8]>,
    snapshot: Option<&[u8]>,
    provider: Option<&CustomProvider>,
) -> Result<Option<(Option<Vec<u8>>, Option<Vec<u8>>)>> {
    if provider.is_none() && snapshot.is_none() {
        return Ok(None);
    }
    let original =
        std::str::from_utf8(config.unwrap_or_default()).context("Codex config is not UTF-8")?;
    let mut doc = original
        .parse::<DocumentMut>()
        .context("Invalid Codex config.toml; no account files were changed")?;
    let mut state: Snapshot = match snapshot {
        Some(bytes) => {
            serde_json::from_slice(bytes).context("Invalid provider restore snapshot")?
        }
        None => Snapshot {
            original_config: original.to_string(),
            provider_id: String::new(),
        },
    };
    let mut providers = match doc.get("model_providers") {
        Some(item) => item
            .as_table()
            .cloned()
            .context("model_providers must be a TOML table")?,
        None => Table::new(),
    };
    if let Some(provider) = provider {
        provider.validate()?;
        let normalized = url::Url::parse(&provider.base_url)?.to_string();
        let provider_id = format!(
            "codex_switcher_{:x}",
            Sha256::digest(normalized.trim_end_matches('/').as_bytes())
        );
        if provider_id != state.provider_id {
            anyhow::ensure!(
                !providers.contains_key(&provider_id),
                "Managed provider ID collision"
            );
            if !state.provider_id.is_empty() {
                providers.remove(&state.provider_id);
            }
        }
        state.provider_id = provider_id;
        let mut table = Table::new();
        table["name"] = value(&provider.name);
        table["base_url"] = value(&provider.base_url);
        table["wire_api"] = value("responses");
        table["requires_openai_auth"] = value(true);
        providers.insert(&state.provider_id, Item::Table(table));
        doc["model_providers"] = Item::Table(providers);
        doc["model"] = value(&provider.model);
        doc["model_provider"] = value(&state.provider_id);
        doc["cli_auth_credentials_store"] = value("file");
        for key in ["review_model", "profile", "openai_base_url"] {
            doc.remove(key);
        }
        Ok(Some((
            Some(doc.to_string().into_bytes()),
            Some(serde_json::to_vec_pretty(&state)?),
        )))
    } else {
        let original = state
            .original_config
            .parse::<DocumentMut>()
            .context("Invalid original provider config snapshot")?;
        for key in KEYS {
            doc.remove(key);
            if let Some((formatted_key, item)) = original.as_table().get_key_value(key) {
                doc.as_table_mut()
                    .insert_formatted(formatted_key, item.clone());
            }
        }
        providers.remove(&state.provider_id);
        if providers.is_empty() && original.get("model_providers").is_none() {
            doc.remove("model_providers");
        } else {
            doc["model_providers"] = Item::Table(providers);
        }
        // Every account selected by Switcher is written to auth.json, not the OS keyring.
        doc["cli_auth_credentials_store"] = value("file");
        let restored = Some(doc.to_string().into_bytes());
        Ok(Some((restored, None)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn provider(url: &str) -> CustomProvider {
        CustomProvider {
            name: "Gateway".into(),
            base_url: url.into(),
            model: "model".into(),
        }
    }
    #[test]
    fn stable_provider_identity_and_collision_rejection() {
        let (config, snapshot) = prepare(None, None, Some(&provider("https://gateway.example/v1")))
            .unwrap()
            .unwrap();
        let state: Snapshot = serde_json::from_slice(snapshot.as_deref().unwrap()).unwrap();
        let (_, second_snapshot) =
            prepare(None, None, Some(&provider("https://gateway.example/v1/")))
                .unwrap()
                .unwrap();
        let second: Snapshot = serde_json::from_slice(second_snapshot.as_deref().unwrap()).unwrap();
        assert_eq!(state.provider_id, second.provider_id);
        assert!(prepare(
            config.as_deref(),
            None,
            Some(&provider("https://gateway.example/v1"))
        )
        .is_err());
        let (restored, _) = prepare(config.as_deref(), snapshot.as_deref(), None)
            .unwrap()
            .unwrap();
        let (_, again_snapshot) = prepare(
            restored.as_deref(),
            None,
            Some(&provider("https://gateway.example/v1")),
        )
        .unwrap()
        .unwrap();
        let again: Snapshot = serde_json::from_slice(again_snapshot.as_deref().unwrap()).unwrap();
        assert_eq!(state.provider_id, again.provider_id);
    }
}
