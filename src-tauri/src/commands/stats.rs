use std::collections::{BTreeSet, HashMap, HashSet};

use chrono::{DateTime, Duration, NaiveDate, Timelike, Utc};
use rusqlite::Connection;

use crate::types::{ClaudeStats, DailyModelData, HeatmapDay, ModelTokenBreakdown, ModelTotals};

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

#[tauri::command]
pub async fn get_claude_stats() -> Result<ClaudeStats, String> {
    let home = dirs::home_dir().ok_or_else(|| "Cannot find home directory".to_string())?;
    let logs_path = home.join(".codex").join("logs_2.sqlite");

    if !logs_path.exists() {
        return Ok(ClaudeStats::empty());
    }

    let conn = Connection::open(&logs_path).map_err(|e| e.to_string())?;

    let mut session_ids: HashSet<String> = HashSet::new();
    let mut submission_ids: HashSet<String> = HashSet::new();
    let mut completion_ids: HashSet<String> = HashSet::new();
    let mut message_count = 0u64;
    let mut hour_counts: HashMap<u8, u64> = HashMap::new();
    let mut daily_model: HashMap<NaiveDate, HashMap<String, DailyModelAccumulator>> =
        HashMap::new();
    let mut model_totals_acc: HashMap<String, ModelAccumulator> = HashMap::new();
    let mut active_dates: BTreeSet<NaiveDate> = BTreeSet::new();

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

            if let Some(sid) = conversation_id {
                session_ids.insert(sid);
            }

            let event_time = extract_value(&body, "event.timestamp")
                .and_then(|value| {
                    DateTime::parse_from_rfc3339(&value)
                        .ok()
                        .map(|dt| dt.with_timezone(&Utc))
                })
                .or_else(|| DateTime::<Utc>::from_timestamp(ts, 0));

            let Some(event_time) = event_time else {
                continue;
            };

            let date = event_time.date_naive();
            active_dates.insert(date);

            let entry = daily_model
                .entry(date)
                .or_default()
                .entry(model.clone())
                .or_default();
            entry.input_tokens += input_tokens;
            entry.output_tokens += output_tokens;

            let total_entry = model_totals_acc.entry(model).or_default();
            total_entry.input_tokens += input_tokens;
            total_entry.output_tokens += output_tokens;
        }
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
            message_count += 1;

            if let Some(event_time) = DateTime::<Utc>::from_timestamp(ts, 0) {
                *hour_counts.entry(event_time.hour() as u8).or_insert(0) += 1;
            }
        }
    }

    let mut heatmap: Vec<HeatmapDay> = daily_model
        .iter()
        .map(|(date, models)| HeatmapDay {
            date: date.to_string(),
            count: models
                .values()
                .map(DailyModelAccumulator::total_tokens)
                .sum(),
        })
        .collect();
    heatmap.sort_by(|a, b| a.date.cmp(&b.date));

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

    let grand_total: u64 = model_totals_acc
        .values()
        .map(ModelAccumulator::total_tokens)
        .sum();
    let mut model_totals: Vec<ModelTotals> = model_totals_acc
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

    let today = Utc::now().date_naive();
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

    Ok(ClaudeStats {
        sessions: session_ids.len() as u64,
        messages: message_count,
        total_input_tokens,
        total_output_tokens,
        total_tokens,
        active_days: active_dates.len() as u64,
        current_streak,
        longest_streak,
        peak_hour,
        favorite_model,
        heatmap,
        daily_model_data,
        model_totals,
        fun_fact: make_fun_fact(total_tokens),
    })
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
