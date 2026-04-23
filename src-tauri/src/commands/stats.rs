use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, Local, NaiveDate, Timelike, Utc};
use rusqlite::Connection;
use serde_json::Value;

use crate::types::{
    CodexStats, DailyModelData, DailyOverviewData, HeatmapDay, ModelTokenBreakdown, ModelTotals,
};

#[derive(Default)]
struct DailyModelAccumulator {
    input_tokens: u64,
    output_tokens: u64,
}

impl DailyModelAccumulator {
    fn total_tokens(&self) -> u64 {
        self.input_tokens + self.output_tokens
    }
}

#[derive(Default)]
struct ModelAccumulator {
    input_tokens: u64,
    output_tokens: u64,
}

impl ModelAccumulator {
    fn total_tokens(&self) -> u64 {
        self.input_tokens + self.output_tokens
    }
}

#[derive(Default)]
struct StatsAccumulator {
    sessions: u64,
    messages: u64,
    hour_counts: HashMap<u8, u64>,
    daily_activity: HashMap<NaiveDate, u64>,
    daily_sessions: HashMap<NaiveDate, u64>,
    daily_messages: HashMap<NaiveDate, u64>,
    daily_hour_counts: HashMap<NaiveDate, [u64; 24]>,
    daily_model: HashMap<NaiveDate, HashMap<String, DailyModelAccumulator>>,
    model_totals: HashMap<String, ModelAccumulator>,
    active_dates: BTreeSet<NaiveDate>,
}

impl StatsAccumulator {
    fn is_empty(&self) -> bool {
        self.sessions == 0
            && self.messages == 0
            && self.daily_model.is_empty()
            && self.active_dates.is_empty()
    }

    fn record_session(&mut self, date: NaiveDate, mark_activity: bool) {
        self.sessions += 1;
        *self.daily_sessions.entry(date).or_insert(0) += 1;
        self.active_dates.insert(date);

        if mark_activity {
            *self.daily_activity.entry(date).or_insert(0) += 1;
        }
    }

    fn record_message(&mut self, date: NaiveDate, hour: Option<u8>) {
        self.messages += 1;
        *self.daily_messages.entry(date).or_insert(0) += 1;
        *self.daily_activity.entry(date).or_insert(0) += 1;
        self.active_dates.insert(date);

        if let Some(hour) = hour.filter(|hour| *hour < 24) {
            *self.hour_counts.entry(hour).or_insert(0) += 1;
            self.daily_hour_counts.entry(date).or_insert([0; 24])[hour as usize] += 1;
        }
    }

    fn record_tokens(
        &mut self,
        date: NaiveDate,
        model: String,
        input_tokens: u64,
        output_tokens: u64,
    ) {
        if input_tokens == 0 && output_tokens == 0 {
            return;
        }

        let entry = self
            .daily_model
            .entry(date)
            .or_default()
            .entry(model.clone())
            .or_default();
        entry.input_tokens += input_tokens;
        entry.output_tokens += output_tokens;

        let total_entry = self.model_totals.entry(model).or_default();
        total_entry.input_tokens += input_tokens;
        total_entry.output_tokens += output_tokens;

        self.active_dates.insert(date);
    }

    fn merge(&mut self, other: StatsAccumulator) {
        self.sessions += other.sessions;
        self.messages += other.messages;

        for (hour, count) in other.hour_counts {
            *self.hour_counts.entry(hour).or_insert(0) += count;
        }

        for (date, count) in other.daily_activity {
            *self.daily_activity.entry(date).or_insert(0) += count;
        }

        for (date, count) in other.daily_sessions {
            *self.daily_sessions.entry(date).or_insert(0) += count;
        }

        for (date, count) in other.daily_messages {
            *self.daily_messages.entry(date).or_insert(0) += count;
        }

        for (date, buckets) in other.daily_hour_counts {
            let entry = self.daily_hour_counts.entry(date).or_insert([0; 24]);
            for (index, count) in buckets.into_iter().enumerate() {
                entry[index] += count;
            }
        }

        for (date, models) in other.daily_model {
            let day_entry = self.daily_model.entry(date).or_default();
            for (model, acc) in models {
                let model_entry = day_entry.entry(model).or_default();
                model_entry.input_tokens += acc.input_tokens;
                model_entry.output_tokens += acc.output_tokens;
            }
        }

        for (model, acc) in other.model_totals {
            let total_entry = self.model_totals.entry(model).or_default();
            total_entry.input_tokens += acc.input_tokens;
            total_entry.output_tokens += acc.output_tokens;
        }

        self.active_dates.extend(other.active_dates);
    }
}

#[tauri::command]
pub async fn get_codex_stats() -> Result<CodexStats, String> {
    let home = dirs::home_dir().ok_or_else(|| "Cannot find home directory".to_string())?;
    let codex_dir = home.join(".codex");
    let sessions_root = codex_dir.join("sessions");
    let logs_path = codex_dir.join("logs_2.sqlite");
    let mut stats = StatsAccumulator::default();

    if sessions_root.exists() {
        stats.merge(load_session_history_stats(&sessions_root)?);
    }

    if logs_path.exists() {
        stats.merge(load_sqlite_stats(&logs_path)?);
    }

    if stats.is_empty() {
        return Ok(CodexStats::empty());
    }

    Ok(build_stats(stats))
}

fn load_session_history_stats(root: &Path) -> Result<StatsAccumulator, String> {
    let mut session_files = Vec::new();
    collect_session_files(root, &mut session_files).map_err(|e| e.to_string())?;
    session_files.sort();

    if session_files.is_empty() {
        return Ok(StatsAccumulator::default());
    }

    let mut stats = StatsAccumulator::default();

    for path in session_files {
        let Some(session_date) = session_date_from_path(&path) else {
            continue;
        };

        stats.record_session(session_date, true);
        ingest_session_file(&path, session_date, &mut stats);
    }

    Ok(stats)
}

fn ingest_session_file(path: &Path, session_date: NaiveDate, stats: &mut StatsAccumulator) {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(_) => return,
    };

    let reader = BufReader::new(file);
    let mut current_model: Option<String> = None;
    let mut last_input_tokens = 0u64;
    let mut last_output_tokens = 0u64;

    for line in reader.lines() {
        let Ok(line) = line else {
            continue;
        };

        if line.contains("\"type\":\"turn_context\"") {
            let Ok(value) = serde_json::from_str::<Value>(&line) else {
                continue;
            };

            current_model = value
                .pointer("/payload/model")
                .and_then(Value::as_str)
                .filter(|model| !model.is_empty())
                .map(str::to_string);
            continue;
        }

        if !line.contains("\"type\":\"event_msg\"") {
            continue;
        }

        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };

        match value.pointer("/payload/type").and_then(Value::as_str) {
            Some("user_message") => {
                stats.record_message(session_date, extract_local_hour(&value));
            }
            Some("token_count") => {
                let Some((input_tokens, output_tokens)) = extract_total_token_usage(&value) else {
                    continue;
                };

                let delta_input = input_tokens.saturating_sub(last_input_tokens);
                let delta_output = output_tokens.saturating_sub(last_output_tokens);
                last_input_tokens = input_tokens;
                last_output_tokens = output_tokens;

                let model = current_model
                    .clone()
                    .unwrap_or_else(|| "unknown".to_string());
                stats.record_tokens(session_date, model, delta_input, delta_output);
            }
            _ => {}
        }
    }
}

fn load_sqlite_stats(logs_path: &Path) -> Result<StatsAccumulator, String> {
    let conn = Connection::open(logs_path).map_err(|e| e.to_string())?;

    let mut stats = StatsAccumulator::default();
    let mut session_first_dates: HashMap<String, NaiveDate> = HashMap::new();
    let mut submission_ids: HashSet<String> = HashSet::new();
    let mut completion_ids: HashSet<String> = HashSet::new();

    {
        let mut stmt = conn
            .prepare(
                "SELECT ts, feedback_log_body
                 FROM logs
                 WHERE target = 'codex_otel.log_only'
                   AND feedback_log_body IS NOT NULL
                   AND feedback_log_body LIKE '%event.name=\"codex.sse_event\" event.kind=response.completed%'
                 ORDER BY ts ASC, ts_nanos ASC, id ASC",
            )
            .map_err(|e| e.to_string())?;

        let rows = stmt
            .query_map([], |row| {
                let ts: i64 = row.get(0)?;
                let body: String = row.get(1)?;
                Ok((ts, body))
            })
            .map_err(|e| e.to_string())?;

        for row in rows {
            let (ts, body) = row.map_err(|e| e.to_string())?;
            let model = extract_value(&body, "model")
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "unknown".to_string());
            let conversation_id = extract_value(&body, "conversation.id");
            let input_tokens = extract_u64(&body, "input_token_count").unwrap_or(0);
            let output_tokens = extract_u64(&body, "output_token_count").unwrap_or(0);
            let dedupe_key = format!(
                "{}|{}|{}|{}|{}",
                extract_value(&body, "event.timestamp").unwrap_or_else(|| ts.to_string()),
                conversation_id.clone().unwrap_or_default(),
                model,
                input_tokens,
                output_tokens
            );

            if !completion_ids.insert(dedupe_key) {
                continue;
            }

            let event_time = extract_value(&body, "event.timestamp")
                .and_then(parse_rfc3339_local)
                .or_else(|| {
                    DateTime::<Utc>::from_timestamp(ts, 0).map(|dt| dt.with_timezone(&Local))
                });

            let Some(event_time) = event_time else {
                continue;
            };

            let date = event_time.date_naive();
            stats.active_dates.insert(date);
            *stats.daily_activity.entry(date).or_insert(0) += 1;
            stats.record_tokens(date, model, input_tokens, output_tokens);

            if let Some(sid) = conversation_id {
                session_first_dates
                    .entry(sid)
                    .and_modify(|existing| {
                        if date < *existing {
                            *existing = date;
                        }
                    })
                    .or_insert(date);
            }
        }
    }

    for first_date in session_first_dates.into_values() {
        stats.record_session(first_date, false);
    }

    {
        let mut stmt = conn
            .prepare(
                "SELECT ts, feedback_log_body
                 FROM logs
                 WHERE target = 'codex_otel.log_only'
                   AND feedback_log_body IS NOT NULL
                   AND feedback_log_body LIKE '%otel.name=\"op.dispatch.user_input\"%'
                   AND feedback_log_body LIKE '%submission.id=%'
                 ORDER BY ts ASC, ts_nanos ASC, id ASC",
            )
            .map_err(|e| e.to_string())?;

        let rows = stmt
            .query_map([], |row| {
                let ts: i64 = row.get(0)?;
                let body: String = row.get(1)?;
                Ok((ts, body))
            })
            .map_err(|e| e.to_string())?;

        for row in rows {
            let (ts, body) = row.map_err(|e| e.to_string())?;
            let Some(submission_id) = extract_value(&body, "submission.id") else {
                continue;
            };
            if !submission_ids.insert(submission_id) {
                continue;
            }

            if let Some(event_time) = DateTime::<Utc>::from_timestamp(ts, 0) {
                let local_time = event_time.with_timezone(&Local);
                stats.record_message(local_time.date_naive(), Some(local_time.hour() as u8));
            }
        }
    }

    Ok(stats)
}

fn build_stats(stats: StatsAccumulator) -> CodexStats {
    let StatsAccumulator {
        sessions,
        messages,
        hour_counts,
        daily_activity,
        daily_sessions,
        daily_messages,
        daily_hour_counts,
        daily_model,
        model_totals,
        active_dates,
    } = stats;

    let mut heatmap: Vec<HeatmapDay> = active_dates
        .iter()
        .map(|date| {
            let token_total = daily_model
                .get(date)
                .map(|models| {
                    models
                        .values()
                        .map(DailyModelAccumulator::total_tokens)
                        .sum()
                })
                .unwrap_or(0);
            let fallback_activity = daily_activity.get(date).copied().unwrap_or(1);

            HeatmapDay {
                date: date.to_string(),
                count: if token_total > 0 {
                    token_total
                } else {
                    fallback_activity
                },
            }
        })
        .collect();
    heatmap.sort_by(|a, b| a.date.cmp(&b.date));

    let mut daily_overview_data: Vec<DailyOverviewData> = active_dates
        .iter()
        .map(|date| DailyOverviewData {
            date: date.to_string(),
            sessions: daily_sessions.get(date).copied().unwrap_or(0),
            messages: daily_messages.get(date).copied().unwrap_or(0),
            hourly_messages: daily_hour_counts
                .get(date)
                .copied()
                .unwrap_or([0; 24])
                .to_vec(),
        })
        .collect();
    daily_overview_data.sort_by(|a, b| a.date.cmp(&b.date));

    let mut daily_model_data: Vec<DailyModelData> = daily_model
        .into_iter()
        .map(|(date, models)| {
            let details = models
                .iter()
                .map(|(model, acc)| {
                    (
                        model.clone(),
                        ModelTokenBreakdown {
                            input_tokens: acc.input_tokens,
                            output_tokens: acc.output_tokens,
                            total_tokens: acc.total_tokens(),
                        },
                    )
                })
                .collect::<HashMap<_, _>>();

            let combined = details
                .iter()
                .map(|(model, detail)| (model.clone(), detail.total_tokens))
                .collect::<HashMap<_, _>>();

            DailyModelData {
                date: date.to_string(),
                models: combined,
                details,
            }
        })
        .collect();
    daily_model_data.sort_by(|a, b| a.date.cmp(&b.date));

    let grand_total: u64 = model_totals
        .values()
        .map(ModelAccumulator::total_tokens)
        .sum();
    let mut model_totals: Vec<ModelTotals> = model_totals
        .into_iter()
        .map(|(model, acc)| {
            let total_tokens = acc.total_tokens();
            ModelTotals {
                model,
                input_tokens: acc.input_tokens,
                output_tokens: acc.output_tokens,
                total_tokens,
                percentage: if grand_total > 0 {
                    (total_tokens as f64 / grand_total as f64) * 100.0
                } else {
                    0.0
                },
            }
        })
        .collect();
    model_totals.sort_by(|a, b| {
        b.total_tokens
            .cmp(&a.total_tokens)
            .then_with(|| a.model.cmp(&b.model))
    });

    let today = Local::now().date_naive();
    let current_streak = current_streak(&active_dates, today);
    let longest_streak = longest_streak(&active_dates);
    let peak_hour = hour_counts
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .map(|(hour, _)| hour);
    let favorite_model = model_totals.first().map(|item| item.model.clone());
    let total_input_tokens: u64 = model_totals.iter().map(|item| item.input_tokens).sum();
    let total_output_tokens: u64 = model_totals.iter().map(|item| item.output_tokens).sum();
    let total_tokens = total_input_tokens + total_output_tokens;

    CodexStats {
        sessions,
        messages,
        total_input_tokens,
        total_output_tokens,
        total_tokens,
        active_days: active_dates.len() as u64,
        current_streak,
        longest_streak,
        peak_hour,
        favorite_model,
        heatmap,
        daily_overview_data,
        daily_model_data,
        model_totals,
        fun_fact: make_fun_fact(total_tokens),
    }
}

fn collect_session_files(root: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;

        if file_type.is_dir() {
            collect_session_files(&path, out)?;
        } else if file_type.is_file()
            && path.extension().and_then(|ext| ext.to_str()) == Some("jsonl")
        {
            out.push(path);
        }
    }

    Ok(())
}

fn session_date_from_path(path: &Path) -> Option<NaiveDate> {
    let day = path.parent()?.file_name()?.to_str()?.parse::<u32>().ok()?;
    let month = path
        .parent()?
        .parent()?
        .file_name()?
        .to_str()?
        .parse::<u32>()
        .ok()?;
    let year = path
        .parent()?
        .parent()?
        .parent()?
        .file_name()?
        .to_str()?
        .parse::<i32>()
        .ok()?;

    NaiveDate::from_ymd_opt(year, month, day)
}

fn extract_local_hour(value: &Value) -> Option<u8> {
    let timestamp = value.get("timestamp")?.as_str()?;
    let local_time = parse_rfc3339_local(timestamp)?;
    Some(local_time.hour() as u8)
}

fn extract_total_token_usage(value: &Value) -> Option<(u64, u64)> {
    let usage = value.pointer("/payload/info/total_token_usage")?;
    let input_tokens = usage.get("input_tokens")?.as_u64()?;
    let output_tokens = usage.get("output_tokens")?.as_u64()?;
    Some((input_tokens, output_tokens))
}

fn parse_rfc3339_local(value: impl AsRef<str>) -> Option<DateTime<Local>> {
    DateTime::parse_from_rfc3339(value.as_ref())
        .ok()
        .map(|dt| dt.with_timezone(&Local))
}

fn extract_value(body: &str, key: &str) -> Option<String> {
    let quoted_marker = format!("{key}=\"");
    if let Some(start) = body.find(&quoted_marker) {
        let value_start = start + quoted_marker.len();
        let value_end = body[value_start..].find('"')?;
        return Some(body[value_start..value_start + value_end].to_string());
    }

    let marker = format!("{key}=");
    let start = body.find(&marker)?;
    let value_start = start + marker.len();
    let value = body[value_start..]
        .split_whitespace()
        .next()?
        .trim_end_matches(',')
        .trim_end_matches('}')
        .trim_end_matches(']')
        .to_string();
    Some(value)
}

fn extract_u64(body: &str, key: &str) -> Option<u64> {
    extract_value(body, key)?.parse().ok()
}

fn current_streak(active_dates: &BTreeSet<NaiveDate>, today: NaiveDate) -> u64 {
    let start = if active_dates.contains(&today) {
        Some(today)
    } else {
        today
            .checked_sub_signed(Duration::days(1))
            .filter(|date| active_dates.contains(date))
    };

    let Some(mut day) = start else {
        return 0;
    };

    let mut streak = 0u64;
    loop {
        if active_dates.contains(&day) {
            streak += 1;
            match day.checked_sub_signed(Duration::days(1)) {
                Some(prev) => day = prev,
                None => break,
            }
        } else {
            break;
        }
    }
    streak
}

fn longest_streak(active_dates: &BTreeSet<NaiveDate>) -> u64 {
    let mut longest = 0u64;
    let mut run = 0u64;
    let mut previous: Option<NaiveDate> = None;

    for &date in active_dates {
        match previous {
            Some(prev) if date == prev + Duration::days(1) => run += 1,
            _ => run = 1,
        }
        longest = longest.max(run);
        previous = Some(date);
    }

    longest
}

fn make_fun_fact(total: u64) -> Option<String> {
    if total == 0 {
        return None;
    }

    const BOOKS: &[(&str, u64)] = &[
        ("Animal Farm", 39_000),
        ("The Great Gatsby", 74_000),
        ("The Catcher in the Rye", 87_000),
        ("Harry Potter and the Sorcerer's Stone", 118_000),
        ("Dune", 268_000),
        ("Moby Dick", 322_000),
        ("War and Peace", 580_000),
        ("The Bible", 783_000),
        ("a complete Encyclopedia Britannica", 44_000_000),
    ];

    let best = BOOKS
        .iter()
        .rev()
        .find(|(_, tokens)| total / tokens >= 2)
        .or_else(|| BOOKS.iter().rev().find(|(_, tokens)| total >= *tokens));

    match best {
        Some((book, tokens)) => {
            let mult = total / tokens;
            Some(format!("You've used ~{}x more tokens than {}.", mult, book))
        }
        None => {
            let (book, tokens) = BOOKS[0];
            let pct = (total as f64 / tokens as f64 * 100.0) as u64;
            Some(format!(
                "You've processed {}% of the tokens in {}.",
                pct, book
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn date(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).unwrap()
    }

    #[test]
    fn build_stats_preserves_daily_overview_after_merging_sources() {
        let mut session_history = StatsAccumulator::default();
        let day_one = date(2026, 4, 1);
        session_history.record_session(day_one, true);
        session_history.record_message(day_one, Some(10));
        session_history.record_message(day_one, Some(10));
        session_history.record_tokens(day_one, "gpt-5".to_string(), 120, 80);

        let mut sqlite = StatsAccumulator::default();
        let day_two = date(2026, 4, 2);
        sqlite.record_session(day_two, false);
        sqlite.record_message(day_two, Some(16));
        sqlite.record_tokens(day_two, "gpt-4.1".to_string(), 30, 20);

        session_history.merge(sqlite);
        let stats = build_stats(session_history);

        assert_eq!(stats.sessions, 2);
        assert_eq!(stats.messages, 3);
        assert_eq!(stats.total_tokens, 250);
        assert_eq!(stats.active_days, 2);
        assert_eq!(stats.peak_hour, Some(10));
        assert_eq!(stats.daily_overview_data.len(), 2);

        let first_day = &stats.daily_overview_data[0];
        assert_eq!(first_day.date, "2026-04-01");
        assert_eq!(first_day.sessions, 1);
        assert_eq!(first_day.messages, 2);
        assert_eq!(first_day.hourly_messages[10], 2);

        let second_day = &stats.daily_overview_data[1];
        assert_eq!(second_day.date, "2026-04-02");
        assert_eq!(second_day.sessions, 1);
        assert_eq!(second_day.messages, 1);
        assert_eq!(second_day.hourly_messages[16], 1);
    }
}
