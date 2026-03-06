use std::sync::{Arc, Mutex};

use chrono::{DateTime, Duration, Local, LocalResult, NaiveTime, TimeZone};
use tauri::{AppHandle, Emitter, Manager};

use crate::auth::{load_accounts, load_settings, mark_scheduled_warmup_ran};
use crate::commands::warmup_accounts_by_ids;
use crate::types::{
    ScheduledWarmupEvent, ScheduledWarmupSettings, ScheduledWarmupStatus, WarmupSummary,
};

pub const SCHEDULED_WARMUP_EVENT: &str = "scheduled-warmup-result";

#[derive(Clone)]
pub struct ScheduledWarmupRuntimeState {
    inner: Arc<Mutex<ScheduledWarmupRuntime>>,
}

struct ScheduledWarmupRuntime {
    session_started_at: DateTime<Local>,
}

impl ScheduledWarmupRuntimeState {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(ScheduledWarmupRuntime {
                session_started_at: Local::now(),
            })),
        }
    }

    pub fn session_started_at(&self) -> DateTime<Local> {
        self.inner
            .lock()
            .expect("scheduled warmup runtime lock poisoned")
            .session_started_at
    }
}

pub fn spawn_scheduler(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            if let Err(err) = maybe_run_scheduled_warmup(&app).await {
                eprintln!("[ScheduledWarmup] Scheduler check failed: {err}");
            }

            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        }
    });
}

pub fn current_local_date_string() -> String {
    Local::now().date_naive().format("%Y-%m-%d").to_string()
}

pub fn get_scheduled_warmup_status(
    session_started_at: DateTime<Local>,
) -> Result<ScheduledWarmupStatus, String> {
    let settings = load_settings().map_err(|e| e.to_string())?;
    let store = load_accounts().map_err(|e| e.to_string())?;
    let schedule = settings.scheduled_warmup;
    let valid_account_ids = schedule
        .as_ref()
        .map(|value| valid_account_ids(&value.account_ids, &store.accounts))
        .unwrap_or_default();

    let missed_run_today = schedule
        .as_ref()
        .map(|value| is_missed_run_today(value, session_started_at, &valid_account_ids))
        .transpose()?
        .unwrap_or(false);

    let next_run_local_iso = schedule
        .as_ref()
        .and_then(|value| compute_next_run_local_iso(value));

    Ok(ScheduledWarmupStatus {
        schedule,
        valid_account_ids,
        missed_run_today,
        next_run_local_iso,
    })
}

pub async fn run_scheduled_warmup_now(app: &AppHandle) -> Result<WarmupSummary, String> {
    let status = get_scheduled_warmup_status(session_started_at(app))?;
    let schedule = status
        .schedule
        .ok_or_else(|| String::from("Scheduled warmup is not configured"))?;

    if !schedule.enabled {
        return Err(String::from("Scheduled warmup is disabled"));
    }

    if status.valid_account_ids.is_empty() {
        return Err(String::from(
            "Scheduled warmup has no valid accounts selected",
        ));
    }

    let summary = warmup_accounts_by_ids(&status.valid_account_ids).await?;
    if summary.total_accounts > 0 {
        mark_scheduled_warmup_ran(&current_local_date_string()).map_err(|e| e.to_string())?;
        emit_result(app, &summary, "missed_prompt")?;
    }

    Ok(summary)
}

pub fn parse_local_time(local_time: &str) -> Option<NaiveTime> {
    NaiveTime::parse_from_str(local_time, "%H:%M").ok()
}

fn session_started_at(app: &AppHandle) -> DateTime<Local> {
    app.state::<ScheduledWarmupRuntimeState>()
        .session_started_at()
}

fn compute_next_run_local_iso(schedule: &ScheduledWarmupSettings) -> Option<String> {
    let scheduled_time = parse_local_time(&schedule.local_time)?;
    let now = Local::now();
    let today = now.date_naive();
    let today_run = local_datetime_for(today, scheduled_time)?;

    let next_run = if now < today_run {
        today_run
    } else {
        local_datetime_for(today + Duration::days(1), scheduled_time)?
    };

    Some(next_run.to_rfc3339())
}

fn is_missed_run_today(
    schedule: &ScheduledWarmupSettings,
    session_started_at: DateTime<Local>,
    valid_ids: &[String],
) -> Result<bool, String> {
    if !schedule.enabled || valid_ids.is_empty() {
        return Ok(false);
    }

    let scheduled_time =
        parse_local_time(&schedule.local_time).ok_or_else(|| String::from("Invalid local time"))?;
    let now = Local::now();
    let today = now.date_naive();
    let today_string = today.format("%Y-%m-%d").to_string();
    let scheduled_at = match local_datetime_for(today, scheduled_time) {
        Some(value) => value,
        None => return Ok(false),
    };

    Ok(now >= scheduled_at
        && session_started_at > scheduled_at
        && schedule.last_run_local_date.as_deref() != Some(today_string.as_str())
        && schedule.last_missed_prompt_local_date.as_deref() != Some(today_string.as_str()))
}

async fn maybe_run_scheduled_warmup(app: &AppHandle) -> Result<(), String> {
    let settings = load_settings().map_err(|e| e.to_string())?;
    let Some(schedule) = settings.scheduled_warmup else {
        return Ok(());
    };

    if !schedule.enabled {
        return Ok(());
    }

    let scheduled_time =
        parse_local_time(&schedule.local_time).ok_or_else(|| String::from("Invalid local time"))?;
    let store = load_accounts().map_err(|e| e.to_string())?;
    let valid_ids = valid_account_ids(&schedule.account_ids, &store.accounts);

    if valid_ids.is_empty() {
        return Ok(());
    }

    let now = Local::now();
    let today = now.date_naive();
    let today_string = today.format("%Y-%m-%d").to_string();
    if schedule.last_run_local_date.as_deref() == Some(today_string.as_str()) {
        return Ok(());
    }

    let Some(scheduled_at) = local_datetime_for(today, scheduled_time) else {
        return Ok(());
    };

    if session_started_at(app) > scheduled_at || now < scheduled_at {
        return Ok(());
    }

    let summary = warmup_accounts_by_ids(&valid_ids).await?;
    if summary.total_accounts > 0 {
        mark_scheduled_warmup_ran(&today_string).map_err(|e| e.to_string())?;
        emit_result(app, &summary, "scheduled")?;
    }

    Ok(())
}

fn emit_result(app: &AppHandle, summary: &WarmupSummary, trigger: &str) -> Result<(), String> {
    app.emit(
        SCHEDULED_WARMUP_EVENT,
        ScheduledWarmupEvent {
            summary: summary.clone(),
            trigger: trigger.to_string(),
        },
    )
    .map_err(|e| e.to_string())
}

fn local_datetime_for(date: chrono::NaiveDate, time: NaiveTime) -> Option<DateTime<Local>> {
    let naive = date.and_time(time);
    match Local.from_local_datetime(&naive) {
        LocalResult::Single(value) => Some(value),
        LocalResult::Ambiguous(earliest, _) => Some(earliest),
        LocalResult::None => None,
    }
}

fn valid_account_ids(
    requested_ids: &[String],
    accounts: &[crate::types::StoredAccount],
) -> Vec<String> {
    requested_ids
        .iter()
        .filter(|account_id| accounts.iter().any(|account| &account.id == *account_id))
        .cloned()
        .collect()
}
