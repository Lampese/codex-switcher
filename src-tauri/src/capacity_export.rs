//! Sanitized capacity projection for the AOS adaptive-router MCP.

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::{collections::HashMap, fs, path::Path};

use crate::{
    api::usage::refresh_all_usage,
    auth::load_accounts,
    commands::check_codex_processes,
    types::{AccountsStore, UsageInfo},
};

const SNAPSHOT_SCHEMA: &str = "aos.capacity-snapshot.v1";
const ADMISSION_SCHEMA: &str = "aos.switcher-admission.v1";

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct AdmissionProjection {
    schema: String,
    observed_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
    host_admitted: bool,
    host_reason: String,
    #[serde(default)]
    lease_valid_by_alias_hash: HashMap<String, bool>,
    receipt_digest: String,
}

#[derive(Debug, Clone, Serialize)]
struct CapacityLane {
    lane_id: String,
    account_alias_hash: String,
    family: &'static str,
    model: &'static str,
    task_classes: [&'static str; 3],
    plan_class: String,
    remaining_percent: Option<f64>,
    reset_at: Option<String>,
    subscription_expires_at: Option<String>,
    running: bool,
    lease_valid: bool,
    refreshed_at: String,
    local_read_only: bool,
    availability_reason: String,
}

#[derive(Debug, Clone, Serialize)]
struct CapacityBody {
    schema: &'static str,
    observed_at: String,
    host_admitted: bool,
    host_reason: String,
    lanes: Vec<CapacityLane>,
}

#[derive(Debug, Clone, Serialize)]
struct CapacitySnapshot {
    schema: &'static str,
    snapshot_id: String,
    observed_at: String,
    host_admitted: bool,
    host_reason: String,
    lanes: Vec<CapacityLane>,
}

fn sha256_text(value: &str) -> String {
    format!("sha256:{:x}", Sha256::digest(value.as_bytes()))
}

fn account_alias_hash(account_id: &str) -> String {
    sha256_text(&format!("codex-switcher-capacity-v1\0{account_id}"))
}

fn safe_plan_class(value: Option<String>) -> String {
    match value
        .as_deref()
        .map(str::trim)
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("free") => "free",
        Some("plus") => "plus",
        Some("pro") => "pro",
        Some("team") => "team",
        Some("business") => "business",
        Some("enterprise") => "enterprise",
        Some("edu") => "edu",
        Some("api_key") => "api",
        _ => "unknown",
    }
    .to_string()
}

fn unix_time(value: Option<i64>) -> Option<DateTime<Utc>> {
    value.and_then(|seconds| Utc.timestamp_opt(seconds, 0).single())
}

fn later_reset(usage: &UsageInfo) -> Option<DateTime<Utc>> {
    [
        unix_time(usage.primary_resets_at),
        unix_time(usage.secondary_resets_at),
    ]
    .into_iter()
    .flatten()
    .max()
}

fn remaining_percent(usage: &UsageInfo) -> Option<f64> {
    [usage.primary_used_percent, usage.secondary_used_percent]
        .into_iter()
        .flatten()
        .map(|used| (100.0 - used).clamp(0.0, 100.0))
        .reduce(f64::min)
}

fn admission_receipt_digest(projection: &AdmissionProjection) -> Result<String> {
    let mut value = serde_json::to_value(projection).context("admission_projection_invalid")?;
    value
        .as_object_mut()
        .context("admission_projection_invalid")?
        .remove("receipt_digest");
    let canonical = serde_json::to_vec(&value).context("admission_projection_invalid")?;
    Ok(format!("sha256:{:x}", Sha256::digest(canonical)))
}

fn validate_admission_projection(
    projection: &AdmissionProjection,
    now: DateTime<Utc>,
) -> Result<()> {
    let fresh_after = now - Duration::minutes(5);
    let future_limit = now + Duration::minutes(1);
    if projection.schema != ADMISSION_SCHEMA
        || projection.observed_at < fresh_after
        || projection.observed_at > future_limit
        || projection.expires_at <= now
        || projection.expires_at > projection.observed_at + Duration::minutes(5)
        || projection.host_reason.is_empty()
        || projection.host_reason.len() > 128
        || !projection.host_reason.chars().all(|character| {
            character.is_ascii_lowercase()
                || character.is_ascii_digit()
                || matches!(character, '_' | '.' | ':' | '-')
        })
        || projection.lease_valid_by_alias_hash.keys().any(|key| {
            key.len() != 71
                || !key.starts_with("sha256:")
                || !key[7..]
                    .chars()
                    .all(|character| character.is_ascii_digit() || ('a'..='f').contains(&character))
        })
        || admission_receipt_digest(projection)? != projection.receipt_digest
    {
        anyhow::bail!("admission_projection_contract_invalid");
    }
    Ok(())
}

fn safe_admission(path: Option<&Path>, now: DateTime<Utc>) -> Result<AdmissionProjection> {
    let Some(path) = path else {
        return Ok(AdmissionProjection {
            schema: ADMISSION_SCHEMA.to_string(),
            observed_at: now,
            expires_at: now,
            host_admitted: false,
            host_reason: "host_admission_receipt_required".to_string(),
            lease_valid_by_alias_hash: HashMap::new(),
            receipt_digest: String::new(),
        });
    };
    let raw = fs::read(path).context("admission_projection_unreadable")?;
    let projection: AdmissionProjection =
        serde_json::from_slice(&raw).context("admission_projection_invalid")?;
    validate_admission_projection(&projection, now)?;
    Ok(projection)
}

fn build_snapshot(
    store: &AccountsStore,
    usage_rows: &[UsageInfo],
    codex_running: bool,
    admission: &AdmissionProjection,
    now: DateTime<Utc>,
) -> Result<CapacitySnapshot> {
    let usage_by_id: HashMap<&str, &UsageInfo> = usage_rows
        .iter()
        .map(|usage| (usage.account_id.as_str(), usage))
        .collect();
    let refreshed_at = now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let mut lanes = Vec::with_capacity(store.accounts.len());
    for account in &store.accounts {
        let alias_hash = account_alias_hash(&account.id);
        let usage = usage_by_id.get(account.id.as_str()).copied();
        let remaining = usage.and_then(|value| remaining_percent(value));
        let reset_at = usage
            .and_then(|value| later_reset(value))
            .map(|value| value.to_rfc3339_opts(chrono::SecondsFormat::Secs, true));
        let expires = account.subscription_expires_at;
        let running =
            codex_running && store.active_account_id.as_deref() == Some(account.id.as_str());
        let lease_valid = admission
            .lease_valid_by_alias_hash
            .get(&alias_hash)
            .copied()
            .unwrap_or(false);
        let availability_reason = if usage.is_none() {
            "usage_unavailable"
        } else if usage.is_some_and(|value| value.error.is_some()) {
            "usage_refresh_failed"
        } else if expires.is_some_and(|value| value <= now) {
            "subscription_expired"
        } else if remaining.is_some_and(|value| value <= 0.0) {
            "allowance_exhausted"
        } else if running {
            "lane_running"
        } else if !admission.host_admitted {
            "host_not_admitted"
        } else if !lease_valid {
            "seat_lease_required"
        } else {
            "available"
        };
        lanes.push(CapacityLane {
            lane_id: format!("codex-seat-{}", &alias_hash[7..23]),
            account_alias_hash: alias_hash,
            family: "codex-spark",
            model: "gpt-5.3-codex-spark",
            task_classes: ["implementation", "test", "refactor"],
            plan_class: safe_plan_class(
                usage
                    .and_then(|value| value.plan_type.clone())
                    .or_else(|| account.plan_type.clone()),
            ),
            remaining_percent: remaining,
            reset_at,
            subscription_expires_at: expires
                .map(|value| value.to_rfc3339_opts(chrono::SecondsFormat::Secs, true)),
            running,
            lease_valid,
            refreshed_at: refreshed_at.clone(),
            local_read_only: false,
            availability_reason: availability_reason.to_string(),
        });
    }
    lanes.sort_by(|left, right| left.lane_id.cmp(&right.lane_id));
    let body = CapacityBody {
        schema: SNAPSHOT_SCHEMA,
        observed_at: refreshed_at.clone(),
        host_admitted: admission.host_admitted,
        host_reason: admission.host_reason.clone(),
        lanes: lanes.clone(),
    };
    let body_value = serde_json::to_value(&body).context("capacity_projection_failed")?;
    let canonical = serde_json::to_vec(&body_value).context("capacity_projection_failed")?;
    Ok(CapacitySnapshot {
        schema: body.schema,
        snapshot_id: format!("sha256:{:x}", Sha256::digest(&canonical)),
        observed_at: body.observed_at,
        host_admitted: body.host_admitted,
        host_reason: body.host_reason,
        lanes: body.lanes,
    })
}

pub async fn export_capacity(output: &Path, admission_path: Option<&Path>) -> Result<()> {
    let store = load_accounts().context("accounts_unavailable")?;
    let usage = refresh_all_usage(&store.accounts).await;
    let process = check_codex_processes()
        .await
        .map_err(|_| anyhow::anyhow!("codex_process_probe_failed"))?;
    let now = Utc::now();
    let admission = safe_admission(admission_path, now)?;
    let snapshot = build_snapshot(&store, &usage, process.count > 0, &admission, now)?;
    let parent = output.parent().context("capacity_output_parent_missing")?;
    fs::create_dir_all(parent).context("capacity_output_parent_unwritable")?;
    let temporary = parent.join(format!(
        ".{}.{}.tmp",
        output
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("capacity"),
        std::process::id()
    ));
    fs::write(&temporary, serde_json::to_vec_pretty(&snapshot)?)
        .context("capacity_output_write_failed")?;
    #[cfg(unix)]
    fs::set_permissions(&temporary, fs::Permissions::from_mode(0o600))
        .context("capacity_output_permissions_failed")?;
    fs::rename(&temporary, output).context("capacity_output_publish_failed")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::StoredAccount;

    #[test]
    fn projection_never_serializes_account_identity_or_credentials() {
        let account = StoredAccount::new_chatgpt(
            "private name".to_string(),
            Some("private@example.com".to_string()),
            Some("pro".to_string()),
            None,
            "private-id-token".to_string(),
            "private-access-token".to_string(),
            "private-refresh-token".to_string(),
            Some("private-account-id".to_string()),
        );
        let account_id = account.id.clone();
        let store = AccountsStore {
            version: 1,
            accounts: vec![account],
            active_account_id: Some(account_id.clone()),
            masked_account_ids: Vec::new(),
        };
        let usage = UsageInfo {
            account_id,
            plan_type: Some("pro".to_string()),
            primary_used_percent: Some(12.0),
            primary_window_minutes: Some(300),
            primary_resets_at: Some(1_800_000_000),
            secondary_used_percent: Some(40.0),
            secondary_window_minutes: Some(10_080),
            secondary_resets_at: Some(1_800_100_000),
            has_credits: Some(false),
            unlimited_credits: Some(false),
            credits_balance: None,
            error: None,
        };
        let now = Utc.timestamp_opt(1_799_000_000, 0).single().unwrap();
        let mut admission = AdmissionProjection {
            schema: ADMISSION_SCHEMA.to_string(),
            observed_at: now,
            expires_at: now + Duration::minutes(5),
            host_admitted: true,
            host_reason: "green".to_string(),
            lease_valid_by_alias_hash: HashMap::new(),
            receipt_digest: String::new(),
        };
        admission.receipt_digest = admission_receipt_digest(&admission).unwrap();
        validate_admission_projection(&admission, now).unwrap();
        let snapshot = build_snapshot(&store, &[usage], false, &admission, now).unwrap();
        let rendered = serde_json::to_string(&snapshot).unwrap();
        for forbidden in [
            "private name",
            "private@example.com",
            "private-id-token",
            "private-access-token",
            "private-refresh-token",
            "private-account-id",
        ] {
            assert!(!rendered.contains(forbidden));
        }
        assert!(snapshot.lanes[0].account_alias_hash.starts_with("sha256:"));
        assert_eq!(snapshot.lanes[0].plan_class, "pro");
        assert_eq!(snapshot.lanes[0].availability_reason, "seat_lease_required");

        let missing_usage = build_snapshot(&store, &[], false, &admission, now).unwrap();
        assert_eq!(missing_usage.lanes[0].remaining_percent, None);
        assert_eq!(
            missing_usage.lanes[0].availability_reason,
            "usage_unavailable"
        );

        let mut stale = admission.clone();
        stale.observed_at = now - Duration::minutes(6);
        stale.expires_at = now + Duration::minutes(1);
        stale.receipt_digest = admission_receipt_digest(&stale).unwrap();
        assert!(validate_admission_projection(&stale, now).is_err());
    }
}
