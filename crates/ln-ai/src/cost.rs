//! Cost tracking (SPEC §9.5). Costs come from the provider's `usage.cost` when available,
//! otherwise from per-token prices, or per-minute prices for transcription.

use crate::Usage;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UsageRecord {
    pub at: chrono::DateTime<chrono::Utc>,
    pub provider: String,
    pub model: String,
    /// What it was for: "summary", "suggestion", "answer", "transcription", ...
    pub purpose: String,
    /// The note it belongs to, if any.
    pub note_id: Option<String>,
    pub usd: f64,
}

/// USD per minute of audio for known transcription models (OpenAI pricing page, 2026-10-21).
pub fn price_per_minute(model: &str) -> Option<f64> {
    match model {
        "gpt-transcribe" => Some(0.0045),
        "gpt-live-transcribe" | "gpt-realtime-whisper" => Some(0.017),
        "whisper-1" | "gpt-4o-transcribe" => Some(0.006),
        "gpt-4o-mini-transcribe" => Some(0.003),
        _ => None,
    }
}

/// Fallback token prices (USD per 1M tokens) when the provider does not report cost.
fn token_prices(model: &str) -> Option<(f64, f64)> {
    match model {
        "google/gemma-4-26b-a4b-it" => Some((0.09, 0.30)),
        "google/gemma-4-31b-it" => Some((0.20, 0.40)),
        "google/gemini-2.5-flash-lite" => Some((0.10, 0.40)),
        "qwen/qwen3-embedding-4b" => Some((0.02, 0.0)),
        _ => None,
    }
}

pub fn cost_of(model: &str, usage: &Usage) -> f64 {
    if let Some(c) = usage.cost {
        return c;
    }
    token_prices(model)
        .map(|(i, o)| (usage.prompt_tokens as f64 * i + usage.completion_tokens as f64 * o) / 1e6)
        .unwrap_or(0.0)
}

/// Append-only cost log in JSON lines.
pub struct CostTracker {
    path: PathBuf,
    records: Mutex<Vec<UsageRecord>>,
}

impl CostTracker {
    pub fn open(path: &Path) -> CostTracker {
        let records = std::fs::read_to_string(path)
            .map(|s| s.lines().filter_map(|l| serde_json::from_str(l).ok()).collect())
            .unwrap_or_default();
        CostTracker { path: path.to_path_buf(), records: Mutex::new(records) }
    }

    pub fn record(&self, r: UsageRecord) {
        if r.usd <= 0.0 {
            return;
        }
        if let Ok(line) = serde_json::to_string(&r) {
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&self.path) {
                let _ = writeln!(f, "{line}");
            }
        }
        self.records.lock().push(r);
    }

    pub fn record_chat(&self, provider: &str, model: &str, purpose: &str, note_id: Option<&str>, usage: &Usage) {
        self.record(UsageRecord {
            at: chrono::Utc::now(),
            provider: provider.into(),
            model: model.into(),
            purpose: purpose.into(),
            note_id: note_id.map(str::to_string),
            usd: cost_of(model, usage),
        });
    }

    pub fn record_audio(&self, provider: &str, model: &str, note_id: Option<&str>, seconds: f64) {
        let usd = price_per_minute(model).unwrap_or(0.0) * seconds / 60.0;
        self.record(UsageRecord {
            at: chrono::Utc::now(),
            provider: provider.into(),
            model: model.into(),
            purpose: "transcription".into(),
            note_id: note_id.map(str::to_string),
            usd,
        });
    }

    pub fn month_total(&self) -> f64 {
        use chrono::Datelike;
        let now = chrono::Utc::now();
        self.records.lock().iter().filter(|r| r.at.year() == now.year() && r.at.month() == now.month()).map(|r| r.usd).sum()
    }

    pub fn note_total(&self, note_id: &str) -> f64 {
        self.records.lock().iter().filter(|r| r.note_id.as_deref() == Some(note_id)).map(|r| r.usd).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn costs_add_up_and_persist() {
        let dir = std::env::temp_dir().join(format!("ln-cost-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("costs.jsonl");
        let _ = std::fs::remove_file(&p);
        let t = CostTracker::open(&p);
        t.record_chat("openrouter", "google/gemma-4-26b-a4b-it", "summary", Some("n1"), &Usage { prompt_tokens: 16_000, completion_tokens: 1_500, cost: None });
        t.record_audio("openai", "gpt-transcribe", Some("n1"), 50.0 * 60.0);
        let expected = 16_000.0 * 0.09 / 1e6 + 1_500.0 * 0.30 / 1e6 + 50.0 * 0.0045;
        assert!((t.note_total("n1") - expected).abs() < 1e-9);
        let again = CostTracker::open(&p);
        assert!((again.month_total() - expected).abs() < 1e-9);
    }
}
