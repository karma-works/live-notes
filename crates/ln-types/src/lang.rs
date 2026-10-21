use serde::{Deserialize, Serialize};
use std::fmt;

/// ISO 639-1 language code, lowercase (`de`, `en`, `zh`, ...).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Lang(String);

/// The five main languages (SPEC §5 F2). They are the default for "my languages".
pub const MAIN_LANGUAGES: [&str; 5] = ["de", "en", "zh", "fr", "es"];

impl Lang {
    pub fn new(code: impl AsRef<str>) -> Self {
        let c = code.as_ref().trim().to_ascii_lowercase();
        // Normalise regional variants to the base language; zh-cn → zh.
        let base = c.split(['-', '_']).next().unwrap_or("").to_string();
        Lang(base)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_cjk(&self) -> bool {
        matches!(self.0.as_str(), "zh" | "ja" | "ko" | "yue")
    }

    pub fn main_languages() -> Vec<Lang> {
        MAIN_LANGUAGES.iter().map(Lang::new).collect()
    }

    /// Native name, used in the language picker.
    pub fn display_name(&self) -> &'static str {
        match self.0.as_str() {
            "de" => "Deutsch",
            "en" => "English",
            "zh" => "中文",
            "fr" => "Français",
            "es" => "Español",
            "it" => "Italiano",
            "pt" => "Português",
            "nl" => "Nederlands",
            "ja" => "日本語",
            "ko" => "한국어",
            "ru" => "Русский",
            "pl" => "Polski",
            "tr" => "Türkçe",
            "ar" => "العربية",
            _ => "?",
        }
    }
}

impl fmt::Display for Lang {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for Lang {
    fn from(s: &str) -> Self {
        Lang::new(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalises_regional_codes() {
        assert_eq!(Lang::new("zh-CN").as_str(), "zh");
        assert_eq!(Lang::new("DE").as_str(), "de");
        assert!(Lang::new("zh").is_cjk());
        assert_eq!(Lang::main_languages().len(), 5);
    }
}
