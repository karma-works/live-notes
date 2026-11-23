//! Which model does what, where it runs and what it costs (settings → "Overview").

use crate::Core;
use ln_types::{ChatRole, ProviderKind, TranscriptionMode};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UseTask {
    Transcription,
    LanguageDetection,
    Search,
    Questions,
    Suggestions,
    Summary,
}

#[derive(Debug, Clone, PartialEq)]
pub enum UseCost {
    /// Runs on this device.
    Local,
    /// USD per 1M input / output tokens.
    PerToken { input: f64, output: f64 },
    /// Billed per token, price not known to the app.
    PerTokenUnknown,
    /// USD per minute of audio.
    PerMinute(f64),
    /// Included in the user's ChatGPT plan.
    Plan,
}

#[derive(Debug, Clone, PartialEq)]
pub enum UseState {
    Ready,
    Off,
    KeyMissing,
    SignInMissing,
    ModelMissing,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ModelUse {
    pub task: UseTask,
    pub model: String,
    /// "Lokal"/"Local" is decided by the UI from `cost`; this is the provider's name.
    pub provider: String,
    pub cost: UseCost,
    pub state: UseState,
    /// Spent this month on this task (USD).
    pub month_usd: f64,
    /// Estimated USD per hour of conversation (`None`: unknown price or not per hour).
    pub per_hour: Option<f64>,
    /// Estimated USD per question (questions only).
    pub per_question: Option<f64>,
}

/// Token assumptions for the estimates (one hour of conversation ≈ 9,000 words).
pub mod estimate {
    /// Summary: whole transcript plus own notes in, title/summary/tasks out.
    pub const SUMMARY: (f64, f64) = (14_000.0, 1_000.0);
    /// Live suggestion: the model turns the last minute into a search query.
    pub const SUGGESTION_QUERY: (f64, f64) = (700.0, 60.0);
    /// Short answer under a card, from three excerpts.
    pub const SUGGESTION_ANSWER: (f64, f64) = (1_800.0, 150.0);
    /// Share of queries that find something worth a card.
    pub const CARD_SHARE: f64 = 0.35;
    /// A question with up to eight excerpts.
    pub const QUESTION: (f64, f64) = (4_000.0, 400.0);
    /// Share of a conversation hour that is speech (only speech is sent for transcription).
    pub const SPEECH_SHARE: f64 = 0.8;

    pub fn tokens(cost: (f64, f64), price: (f64, f64)) -> f64 {
        (cost.0 * price.0 + cost.1 * price.1) / 1e6
    }
}

fn display(id: &str) -> String {
    ln_models::by_id(id).map(|m| m.display.to_string()).unwrap_or_else(|| id.to_string())
}

fn provider_name(s: &ln_types::Settings, id: &str) -> String {
    match s.provider(id) {
        Some(p) if p.kind == ProviderKind::LocalLlama => "llama.cpp".into(),
        Some(p) => p.name.clone(),
        None => id.to_string(),
    }
}

impl Core {
    /// Estimated cost of one hour of conversation with the current settings (USD); `None` when a
    /// price is unknown. Questions are not included (they depend on how much one asks).
    pub fn hourly_estimate(&self) -> Option<f64> {
        self.model_overview().iter().filter(|u| u.task != UseTask::Questions).map(|u| u.per_hour).sum()
    }

    pub fn model_overview(&self) -> Vec<ModelUse> {
        let s = self.settings();
        let mut v = Vec::new();
        let local = |id: &str| if self.model_installed(id) { UseState::Ready } else { UseState::ModelMissing };
        // Transcription and language detection.
        match s.transcription.mode {
            TranscriptionMode::Local => {
                v.push(ModelUse {
                    task: UseTask::Transcription,
                    model: display(&s.transcription.local_model),
                    provider: "whisper.cpp".into(),
                    cost: UseCost::Local,
                    state: local(&s.transcription.local_model),
                    month_usd: 0.0,
                    per_hour: Some(0.0),
                    per_question: None,
                });
                v.push(ModelUse {
                    task: UseTask::LanguageDetection,
                    model: display("whisper-base-q8_0"),
                    provider: "whisper.cpp".into(),
                    cost: UseCost::Local,
                    state: local("whisper-base-q8_0"),
                    month_usd: 0.0,
                    per_hour: Some(0.0),
                    per_question: None,
                });
            }
            TranscriptionMode::OpenAiCheap | TranscriptionMode::OpenAiLive => v.push(ModelUse {
                task: UseTask::Transcription,
                model: s.transcription.openai_model.clone(),
                provider: "OpenAI".into(),
                cost: ln_ai::price_per_minute(&s.transcription.openai_model).map(UseCost::PerMinute).unwrap_or(UseCost::PerTokenUnknown),
                state: if self.has_api_key(&s.transcription.openai_provider) { UseState::Ready } else { UseState::KeyMissing },
                month_usd: self.costs.month_purpose_total("transcription"),
                per_hour: ln_ai::price_per_minute(&s.transcription.openai_model).map(|m| m * 60.0 * estimate::SPEECH_SHARE),
                per_question: None,
            }),
        }
        // Search.
        let e = &s.roles.embeddings;
        let e_local = s.provider(&e.provider).is_none_or(|p| !p.is_cloud);
        v.push(ModelUse {
            task: UseTask::Search,
            model: display(&e.model),
            provider: provider_name(&s, &e.provider),
            cost: if e_local { UseCost::Local } else { UseCost::PerTokenUnknown },
            state: if self.embedder_available() { UseState::Ready } else { UseState::ModelMissing },
            month_usd: 0.0,
            per_hour: if e_local { Some(0.0) } else { None },
            per_question: None,
        });
        // Chat roles.
        for (role, task, purpose, on) in [
            (ChatRole::Assistant, UseTask::Questions, "answer", true),
            (ChatRole::Suggestions, UseTask::Suggestions, "suggestion", s.live_suggestions.enabled),
            (ChatRole::Summary, UseTask::Summary, "summary", s.notes.auto_summary),
        ] {
            let c = s.roles.get(role);
            let p = s.provider(&c.provider);
            let kind = p.map(|p| p.kind);
            let cost = match kind {
                Some(ProviderKind::ChatGpt) => UseCost::Plan,
                Some(ProviderKind::LocalLlama | ProviderKind::Ollama | ProviderKind::LmStudio) => UseCost::Local,
                Some(_) if p.is_some_and(|p| !p.is_cloud) => UseCost::Local,
                _ => ln_ai::token_prices(&c.model)
                    .map(|(input, output)| UseCost::PerToken { input, output })
                    .unwrap_or(UseCost::PerTokenUnknown),
            };
            let state = if !on {
                UseState::Off
            } else if self.role_ready(role) {
                UseState::Ready
            } else {
                match kind {
                    Some(ProviderKind::ChatGpt) => UseState::SignInMissing,
                    Some(ProviderKind::OpenRouter | ProviderKind::OpenAi) => UseState::KeyMissing,
                    _ => UseState::ModelMissing,
                }
            };
            let model = if kind == Some(ProviderKind::ChatGpt) {
                self.chatgpt_models().into_iter().find(|m| m.id == c.model).map(|m| m.name).unwrap_or_else(|| c.model.clone())
            } else if kind == Some(ProviderKind::LocalLlama) {
                display(&c.model)
            } else {
                c.model.clone()
            };
            let price = match cost {
                UseCost::Local | UseCost::Plan => Some((0.0, 0.0)),
                UseCost::PerToken { input, output } => Some((input, output)),
                _ => None,
            };
            let triggers = 3600.0 / s.live_suggestions.frequency.topic_window_secs() as f64;
            let answers = if s.live_suggestions.answers { estimate::CARD_SHARE } else { 0.0 };
            let (per_hour, per_question) = match task {
                UseTask::Summary => (price.map(|p| estimate::tokens(estimate::SUMMARY, p)), None),
                UseTask::Suggestions => (
                    price.map(|p| {
                        triggers
                            * (estimate::tokens(estimate::SUGGESTION_QUERY, p) + answers * estimate::tokens(estimate::SUGGESTION_ANSWER, p))
                    }),
                    None,
                ),
                _ => (None, price.map(|p| estimate::tokens(estimate::QUESTION, p))),
            };
            v.push(ModelUse {
                task,
                model,
                provider: provider_name(&s, &c.provider),
                cost,
                state,
                month_usd: self.costs.month_purpose_total(purpose),
                per_hour: if on { per_hour } else { Some(0.0) },
                per_question,
            });
        }
        v
    }
}
