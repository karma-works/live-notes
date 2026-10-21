//! Model access over the OpenAI-compatible API (ADR-0002). One client covers OpenRouter,
//! OpenAI, a LiteLLM proxy, Ollama, LM Studio and the bundled llama-server.

mod client;
mod cost;
pub mod prompts;
mod types;

pub use client::Client;
pub use cost::{CostTracker, UsageRecord, price_per_minute};
pub use types::*;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("network: {0}")]
    Http(#[from] reqwest::Error),
    #[error("{status}: {message}")]
    Api { status: u16, message: String },
    #[error("no API key for {0}")]
    MissingKey(String),
    #[error("offline mode is on")]
    Offline,
    #[error("unexpected response: {0}")]
    Decode(String),
    #[error("cancelled")]
    Cancelled,
}

impl Error {
    /// Short, user-facing explanation (SPEC §6.1 rule 9: no raw exception strings).
    pub fn user_message(&self, ui: ln_types::UiLanguage) -> String {
        use ln_types::UiLanguage::*;
        match (self, ui) {
            (Error::MissingKey(p), De) => format!("Für {p} fehlt ein API-Schlüssel. Trage ihn in den Einstellungen ein."),
            (Error::MissingKey(p), En) => format!("{p} needs an API key. Add it in Settings."),
            (Error::Offline, De) => "Offline-Modus ist an.".into(),
            (Error::Offline, En) => "Offline mode is on.".into(),
            (Error::Api { status: 401 | 403, .. }, De) => "Der API-Schlüssel wurde abgelehnt.".into(),
            (Error::Api { status: 401 | 403, .. }, En) => "The API key was rejected.".into(),
            (Error::Api { status: 429, .. }, De) => "Zu viele Anfragen. Versuche es gleich noch einmal.".into(),
            (Error::Api { status: 429, .. }, En) => "Too many requests. Try again in a moment.".into(),
            (Error::Http(_), De) => "Keine Verbindung zum Modell-Anbieter.".into(),
            (Error::Http(_), En) => "Couldn't reach the model provider.".into(),
            (e, De) => format!("Anfrage fehlgeschlagen ({e})."),
            (e, En) => format!("Request failed ({e})."),
        }
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
