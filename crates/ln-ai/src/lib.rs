//! Model access over the OpenAI-compatible API (ADR-0002). One client covers OpenRouter,
//! OpenAI, a LiteLLM proxy, Ollama, LM Studio and the bundled llama-server.

pub mod chatgpt;
mod client;
mod cost;
mod decisions;
pub mod prompts;
mod types;

pub use client::Client;
pub use decisions::{Decisions, YesNo};

/// HTTP client for the sign-in flow; ln-core may not depend on reqwest itself (deny.toml).
pub type HttpClient = reqwest::Client;
pub fn http_client() -> HttpClient {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(60))
        .user_agent(concat!("live-notes/", env!("CARGO_PKG_VERSION")))
        .build()
        .expect("http client")
}
pub use cost::{CostTracker, UsageRecord, price_per_minute, token_prices};
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
    /// ChatGPT plan usage or sign-in error with OpenAI's machine-readable code.
    #[error("{status} {code}: {message}")]
    Plan { status: u16, code: String, message: String },
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
            (Error::Plan { code, .. }, ui) => plan_message(code, ui),
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

fn plan_message(code: &str, ui: ln_types::UiLanguage) -> String {
    let de = ui == ln_types::UiLanguage::De;
    let (d, e) = match code {
        "subscription_sharing_usage_limit_exceeded" => (
            "Das Nutzungslimit deines ChatGPT-Abos ist erreicht. Prüfe es unter chatgpt.com/settings/usage.",
            "Your ChatGPT plan's usage limit is reached. Review it at chatgpt.com/settings/usage.",
        ),
        "subscription_sharing_user_not_eligible" => (
            "Dein ChatGPT-Konto kann das Abo hier nicht nutzen (nur Plus und Pro).",
            "Your ChatGPT account can't use its plan here (Plus and Pro only).",
        ),
        "subscription_sharing_usage_unavailable" | "subscription_sharing_user_unavailable" => {
            ("ChatGPT ist gerade nicht erreichbar. Versuche es später noch einmal.", "ChatGPT is unavailable right now. Try again later.")
        }
        "subscription_sharing_unsupported_capability" => (
            "Dieses Modell unterstützt die Anfrage über das ChatGPT-Abo nicht.",
            "This model doesn't support the request on your ChatGPT plan.",
        ),
        _ => (
            "Die Anmeldung bei ChatGPT ist abgelaufen. Melde dich in den Einstellungen erneut an.",
            "Your ChatGPT sign-in expired. Sign in again in Settings.",
        ),
    };
    if de { d.into() } else { e.into() }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
