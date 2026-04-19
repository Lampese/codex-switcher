use std::collections::{BTreeSet, HashMap, HashSet};
use std::fs;
use std::io::{BufRead, BufReader};

use chrono::{DateTime, Duration, NaiveDate, Timelike, Utc};
use serde::Deserialize;

use crate::types::{ClaudeStats, DailyModelData, HeatmapDay, ModelTotals};

// ── Minimal JSONL structures ──────────────────────────────────────────────────

#[derive(Deserialize)]
struct RawEntry {
    #[serde(rename = "type")]
    entry_type: Option<String>,
    #[serde(rename = "sessionId")]
    session_id: Option<String>,
    timestamp: Option<String>,
    message: Option<RawMessage>,
}

#[derive(Deserialize)]
struct RawMessage {
    role: Option<String>,
    model: Option<String>,
    usage: Option<RawUsage>,
}

#[derive(Deserialize, Default)]
struct RawUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
}

// ── Command ───────────────────────────────────────────────────────────────────

#[tauri::command]
pub async fn get_claude_stats() -> Result<ClaudeStats, String> {
    let home = dirs::home_dir().ok_or_else(|| "Cannot find home directory".to_string())?;
    let projects_dir = home.join(".claude").join("projects");

    if !projects_dir.exists() {
        return Ok(ClaudeStats::empty());
    }

    let mut session_ids: HashSet<String> = HashSet::new();
    let mut message_count = 0u64;
    let mut hour_counts: HashMap<u8, u64> = HashMap::new();

    // date → model → total_tokens
    let mut daily_model: HashMap<NaiveDate, HashMap<String, u64>> = HashMap::new();
    // model → (input, output)
    let mut model_input: HashMap<String, u64> = HashMap::new();
    let mut model_output: HashMap<String, u64> = HashMap::new();
    // all dates with any activity
    let mut active_dates: BTreeSet<NaiveDate> = BTreeSet::new();

    let Ok(project_entries) = fs::read_dir(&projects_dir) else {
        return Ok(ClaudeStats::empty());
    };

    for project_entry in project_entries.flatten() {
        let project_path = project_entry.path();
        if !project_path.is_dir() {
            continue;
        }

        let Ok(file_entries) = fs::read_dir(&project_path) else {
            continue;
        };

        for file_entry in file_entries.flatten() {
            let file_path = file_entry.path();
            if file_path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }

            let Ok(file) = fs::File::open(&file_path) else {
                continue;
            };
            let reader = BufReader::new(file);

            for line in reader.lines().flatten() {
                let line = line.trim().to_string();
                if line.is_empty() {
                    continue;
                }

                let Ok(entry) = serde_json::from_str::<RawEntry>(&line) else {
                    continue;
                };

                if let Some(sid) = &entry.session_id {
                    session_ids.insert(sid.clone());
                }

                let time_info = entry.timestamp.as_ref().and_then(|ts| {
                    DateTime::parse_from_rfc3339(ts).ok().map(|dt| {
                        let utc: DateTime<Utc> = dt.into();
                        (utc.date_naive(), utc.hour() as u8)
                    })
                });

                if let Some((date, _)) = time_info {
                    active_dates.insert(date);
                }

                match entry.entry_type.as_deref() {
                    Some("user") => {
                        message_count += 1;
                        if let Some((_, hour)) = time_info {
                            *hour_counts.entry(hour).or_insert(0) += 1;
                        }
                    }
                    // assistant entries may have no outer "type" field
                    Some("assistant") | None => {
                        if let Some(msg) = &entry.message {
                            if msg.role.as_deref() == Some("assistant") {
                                if let Some(usage) = &msg.usage {
                                    let inp = usage.input_tokens
                                        + usage.cache_creation_input_tokens
                                        + usage.cache_read_input_tokens;
                                    let out = usage.output_tokens;

                                    if let Some(model) = &msg.model {
                                        *model_input.entry(model.clone()).or_insert(0) += inp;
                                        *model_output.entry(model.clone()).or_insert(0) += out;

                                        if let Some((date, _)) = time_info {
                                            *daily_model
                                                .entry(date)
                                                .or_default()
                                                .entry(model.clone())
                                                .or_insert(0) += inp + out;
                                        }
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    // ── Heatmap (daily token totals) ─────────────────────────────────────────
    let heatmap: Vec<HeatmapDay> = {
        let mut day_tokens: HashMap<NaiveDate, u64> = HashMap::new();
        for (date, models) in &daily_model {
            let total: u64 = models.values().sum();
            *day_tokens.entry(*date).or_insert(0) += total;
        }
        let mut v: Vec<HeatmapDay> = day_tokens
            .into_iter()
            .map(|(date, count)| HeatmapDay {
                date: date.to_string(),
                count,
            })
            .collect();
        v.sort_by(|a, b| a.date.cmp(&b.date));
        v
    };

    // ── Daily model data (for bar chart) ─────────────────────────────────────
    let daily_model_data: Vec<DailyModelData> = {
        let mut v: Vec<DailyModelData> = daily_model
            .into_iter()
            .map(|(date, models)| DailyModelData {
                date: date.to_string(),
                models,
            })
            .collect();
        v.sort_by(|a, b| a.date.cmp(&b.date));
        v
    };

    // ── Per-model aggregates ─────────────────────────────────────────────────
    let grand_total: u64 = model_input.values().sum::<u64>() + model_output.values().sum::<u64>();
    let model_totals: Vec<ModelTotals> = {
        let mut all_models: Vec<String> = model_input.keys().cloned().collect();
        all_models.sort();
        let mut v: Vec<ModelTotals> = all_models
            .into_iter()
            .map(|model| {
                let inp = *model_input.get(&model).unwrap_or(&0);
                let out = *model_output.get(&model).unwrap_or(&0);
                let total = inp + out;
                ModelTotals {
                    model,
                    input_tokens: inp,
                    output_tokens: out,
                    total_tokens: total,
                    percentage: if grand_total > 0 {
                        (total as f64 / grand_total as f64) * 100.0
                    } else {
                        0.0
                    },
                }
            })
            .collect();
        v.sort_by(|a, b| b.total_tokens.cmp(&a.total_tokens));
        v
    };

    // ── Streaks ──────────────────────────────────────────────────────────────
    let today = Utc::now().date_naive();

    let current_streak: u64 = {
        let start = if active_dates.contains(&today) {
            Some(today)
        } else {
            today
                .checked_sub_signed(Duration::days(1))
                .filter(|d| active_dates.contains(d))
        };

        match start {
            None => 0,
            Some(mut day) => {
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
        }
    };

    let longest_streak: u64 = {
        let mut max = 0u64;
        let mut run = 0u64;
        let mut prev: Option<NaiveDate> = None;
        for &date in &active_dates {
            match prev {
                Some(p) if date == p + Duration::days(1) => run += 1,
                _ => run = 1,
            }
            if run > max {
                max = run;
            }
            prev = Some(date);
        }
        max
    };

    // ── Derived scalars ──────────────────────────────────────────────────────
    let peak_hour = hour_counts
        .into_iter()
        .max_by_key(|(_, c)| *c)
        .map(|(h, _)| h);

    let favorite_model = model_totals.first().map(|m| m.model.clone());
    let active_days = active_dates.len() as u64;
    let total_input_tokens: u64 = model_input.values().sum();
    let total_output_tokens: u64 = model_output.values().sum();
    let total_tokens = total_input_tokens + total_output_tokens;
    let fun_fact = make_fun_fact(total_tokens);

    Ok(ClaudeStats {
        sessions: session_ids.len() as u64,
        messages: message_count,
        total_input_tokens,
        total_output_tokens,
        total_tokens,
        active_days,
        current_streak,
        longest_streak,
        peak_hour,
        favorite_model,
        heatmap,
        daily_model_data,
        model_totals,
        fun_fact,
    })
}

// ── Fun-fact helper ───────────────────────────────────────────────────────────

fn make_fun_fact(total: u64) -> Option<String> {
    if total == 0 {
        return None;
    }
    // Approximate token counts for well-known texts
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

    // Pick the largest book whose multiplier ≥ 2
    let best = BOOKS
        .iter()
        .rev()
        .find(|(_, tokens)| total / tokens >= 2)
        .or_else(|| BOOKS.iter().rev().find(|(_, tokens)| total >= *tokens));

    match best {
        Some((book, tokens)) => {
            let mult = total / tokens;
            Some(format!("You've used ~{}× more tokens than {}.", mult, book))
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
