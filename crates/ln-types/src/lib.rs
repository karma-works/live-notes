//! Shared types for Live Notes: speaker channels, languages, transcript segments,
//! settings and filesystem paths. Every other crate depends on this one, so it
//! stays free of heavy dependencies.

mod lang;
mod paths;
mod segment;
mod settings;

pub use lang::{Lang, MAIN_LANGUAGES};
pub use paths::AppPaths;
pub use segment::{Channel, Segment};
pub use settings::*;

/// UI language. Only German and English in v1.0 (SPEC §16).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum UiLanguage {
    De,
    #[default]
    En,
}

impl UiLanguage {
    /// Picks the UI language from a locale string such as `de_CH.UTF-8` or `en-US`.
    pub fn from_locale(locale: &str) -> Self {
        if locale.to_ascii_lowercase().starts_with("de") { UiLanguage::De } else { UiLanguage::En }
    }

    /// Reads the system locale from the usual environment variables.
    pub fn from_env() -> Self {
        for key in ["LN_UI_LANG", "LC_ALL", "LC_MESSAGES", "LANG"] {
            if let Ok(v) = std::env::var(key)
                && !v.is_empty()
            {
                return Self::from_locale(&v);
            }
        }
        // Apps started from the Finder get no LANG: ask macOS for the preferred languages.
        #[cfg(target_os = "macos")]
        if let Ok(out) = std::process::Command::new("defaults").args(["read", "-g", "AppleLanguages"]).output()
            && let Some(first) =
                String::from_utf8_lossy(&out.stdout).lines().map(|l| l.trim().trim_matches(['"', ',', '(', ')'])).find(|l| !l.is_empty())
        {
            return Self::from_locale(first);
        }
        UiLanguage::En
    }

    pub fn code(self) -> &'static str {
        match self {
            UiLanguage::De => "de",
            UiLanguage::En => "en",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("settings: {0}")]
    Settings(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
