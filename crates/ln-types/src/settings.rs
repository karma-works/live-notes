use crate::{AppPaths, Lang, UiLanguage};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// All user settings, stored as TOML in the app data directory (SPEC §5 F7).
/// API keys are never stored here; they live in the keychain (see `ln-platform`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub first_run_done: bool,
    pub gemma_terms_accepted: bool,
    pub recording_consent_accepted: bool,
    pub general: General,
    pub languages: Languages,
    pub transcription: Transcription,
    pub providers: Vec<Provider>,
    pub roles: Roles,
    pub live_suggestions: LiveSuggestions,
    pub notes: NotesSettings,
    pub privacy: Privacy,
    pub costs: Costs,
    pub sources: Vec<SourceSetting>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct General {
    /// `None` = default `~/Documents/Live Notes`.
    pub notes_dir: Option<PathBuf>,
    /// `None` = follow the system.
    pub ui_language: Option<UiLanguage>,
    pub record_hotkey: String,
    pub reminder_tone_minutes: Option<u32>,
    /// Unload local models after this many idle minutes.
    pub unload_models_after_min: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Languages {
    /// "My languages": language detection only chooses among these.
    pub mine: Vec<Lang>,
    pub chinese_script: ChineseScript,
    /// `None` = summary in the main language of the conversation.
    pub summary_language: Option<Lang>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum ChineseScript {
    #[default]
    Simplified,
    Traditional,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptionMode {
    #[default]
    Local,
    /// `gpt-transcribe`, one VAD section per REST call.
    OpenAiCheap,
    /// `gpt-live-transcribe` over the Realtime API (v1.1, SPEC §16).
    OpenAiLive,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Transcription {
    pub mode: TranscriptionMode,
    /// Local model id from the catalog in `ln-models`.
    pub local_model: String,
    /// Provider id used for OpenAI transcription.
    pub openai_provider: String,
    pub openai_model: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    OpenRouter,
    OpenAi,
    /// The bundled llama-server on 127.0.0.1.
    LocalLlama,
    Ollama,
    LmStudio,
    Custom,
}

/// An OpenAI-compatible endpoint (SPEC ADR-0002).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Provider {
    pub id: String,
    pub name: String,
    pub kind: ProviderKind,
    pub base_url: String,
    /// Extra HTTP headers, e.g. OpenRouter's `X-Title`.
    #[serde(default)]
    pub headers: Vec<(String, String)>,
    /// Data leaves the machine when this provider is used.
    pub is_cloud: bool,
}

impl Provider {
    pub fn openrouter() -> Self {
        Provider {
            id: "openrouter".into(),
            name: "OpenRouter".into(),
            kind: ProviderKind::OpenRouter,
            base_url: "https://openrouter.ai/api/v1".into(),
            headers: vec![
                ("HTTP-Referer".into(), "https://github.com/karma-works/live-notes".into()),
                ("X-Title".into(), "Live Notes".into()),
            ],
            is_cloud: true,
        }
    }
    pub fn openai() -> Self {
        Provider {
            id: "openai".into(),
            name: "OpenAI".into(),
            kind: ProviderKind::OpenAi,
            base_url: "https://api.openai.com/v1".into(),
            headers: vec![],
            is_cloud: true,
        }
    }
    pub fn local_llama() -> Self {
        Provider {
            id: "local".into(),
            name: "Lokal (llama-server)".into(),
            kind: ProviderKind::LocalLlama,
            base_url: "http://127.0.0.1:18432/v1".into(),
            headers: vec![],
            is_cloud: false,
        }
    }
    pub fn ollama() -> Self {
        Provider {
            id: "ollama".into(),
            name: "Ollama".into(),
            kind: ProviderKind::Ollama,
            base_url: "http://127.0.0.1:11434/v1".into(),
            headers: vec![],
            is_cloud: false,
        }
    }
    pub fn lm_studio() -> Self {
        Provider {
            id: "lmstudio".into(),
            name: "LM Studio".into(),
            kind: ProviderKind::LmStudio,
            base_url: "http://127.0.0.1:1234/v1".into(),
            headers: vec![],
            is_cloud: false,
        }
    }
}

/// Which provider and model serve a role.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RoleConfig {
    pub provider: String,
    pub model: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Roles {
    pub assistant: RoleConfig,
    pub image: RoleConfig,
    pub embeddings: RoleConfig,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum SuggestionFrequency {
    Reserved,
    #[default]
    Normal,
    Active,
}

impl SuggestionFrequency {
    /// Minimum gap between two suggestion cards.
    pub fn min_gap_secs(self) -> u64 {
        match self {
            SuggestionFrequency::Reserved => 60,
            SuggestionFrequency::Normal => 20,
            SuggestionFrequency::Active => 10,
        }
    }
    /// New speech needed for a topic-based trigger.
    pub fn topic_window_secs(self) -> u64 {
        match self {
            SuggestionFrequency::Reserved => 90,
            SuggestionFrequency::Normal => 45,
            SuggestionFrequency::Active => 25,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct LiveSuggestions {
    pub enabled: bool,
    pub frequency: SuggestionFrequency,
    /// Show a short answer from the assistant model under the hits.
    pub answers: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct NotesSettings {
    /// Summary, decisions, tasks and title after every recording.
    pub auto_summary: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Privacy {
    /// Blocks every network request, including model downloads.
    pub offline_mode: bool,
    /// OpenRouter: only route to providers that do not store data.
    pub openrouter_deny_data_collection: bool,
    /// Providers the user confirmed once before the first upload.
    pub confirmed_cloud_providers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Costs {
    /// Ask before cloud requests once this monthly sum is reached. 0 = no limit.
    pub monthly_limit_usd: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SourceSetting {
    pub path: PathBuf,
    #[serde(default = "yes")]
    pub include_subdirs: bool,
    #[serde(default)]
    pub excludes: Vec<String>,
    #[serde(default)]
    pub paused: bool,
    #[serde(default)]
    pub read_only: bool,
    /// Content of this source is never sent to cloud roles.
    #[serde(default)]
    pub local_only: bool,
}

fn yes() -> bool {
    true
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            first_run_done: false,
            gemma_terms_accepted: false,
            recording_consent_accepted: false,
            general: General::default(),
            languages: Languages::default(),
            transcription: Transcription::default(),
            providers: vec![
                Provider::openrouter(),
                Provider::openai(),
                Provider::local_llama(),
                Provider::ollama(),
                Provider::lm_studio(),
            ],
            roles: Roles::default(),
            live_suggestions: LiveSuggestions::default(),
            notes: NotesSettings::default(),
            privacy: Privacy::default(),
            costs: Costs::default(),
            sources: vec![],
        }
    }
}

impl Default for General {
    fn default() -> Self {
        General {
            notes_dir: None,
            ui_language: None,
            record_hotkey: "Alt+Cmd+R".into(),
            reminder_tone_minutes: None,
            unload_models_after_min: 5,
        }
    }
}

impl Default for Languages {
    fn default() -> Self {
        Languages { mine: Lang::main_languages(), chinese_script: ChineseScript::Simplified, summary_language: None }
    }
}

impl Default for Transcription {
    fn default() -> Self {
        Transcription {
            mode: TranscriptionMode::Local,
            local_model: "whisper-large-v3-turbo-q5_0".into(),
            openai_provider: "openai".into(),
            openai_model: "gpt-transcribe".into(),
        }
    }
}

impl Default for Roles {
    fn default() -> Self {
        Roles {
            assistant: RoleConfig { provider: "openrouter".into(), model: "google/gemma-4-26b-a4b-it".into() },
            image: RoleConfig { provider: "local".into(), model: "gemma-4-e2b-it".into() },
            embeddings: RoleConfig { provider: "local".into(), model: "embeddinggemma-300m".into() },
        }
    }
}

impl Default for LiveSuggestions {
    fn default() -> Self {
        LiveSuggestions { enabled: true, frequency: SuggestionFrequency::Normal, answers: true }
    }
}

impl Default for NotesSettings {
    fn default() -> Self {
        NotesSettings { auto_summary: true }
    }
}

impl Default for Privacy {
    fn default() -> Self {
        Privacy { offline_mode: false, openrouter_deny_data_collection: true, confirmed_cloud_providers: vec![] }
    }
}

impl Default for Costs {
    fn default() -> Self {
        Costs { monthly_limit_usd: 5.0 }
    }
}

impl Settings {
    pub fn load(path: &Path) -> crate::Result<Settings> {
        match std::fs::read_to_string(path) {
            Ok(s) => toml::from_str(&s).map_err(|e| crate::Error::Settings(e.to_string())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default()),
            Err(e) => Err(e.into()),
        }
    }

    /// Writes atomically (temp file + rename) so a crash never leaves half a file.
    pub fn save(&self, path: &Path) -> crate::Result<()> {
        let text = toml::to_string_pretty(self).map_err(|e| crate::Error::Settings(e.to_string()))?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    pub fn notes_dir(&self, paths: &AppPaths) -> PathBuf {
        self.general.notes_dir.clone().unwrap_or_else(|| paths.default_notes_dir.clone())
    }

    pub fn ui_language(&self) -> UiLanguage {
        self.general.ui_language.unwrap_or_else(UiLanguage::from_env)
    }

    pub fn provider(&self, id: &str) -> Option<&Provider> {
        self.providers.iter().find(|p| p.id == id)
    }

    pub fn source_for(&self, file: &Path) -> Option<&SourceSetting> {
        self.sources.iter().filter(|s| file.starts_with(&s.path)).max_by_key(|s| s.path.as_os_str().len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("settings.toml");
        let mut s = Settings::load(&p).unwrap();
        assert_eq!(s, Settings::default());
        assert_eq!(s.languages.mine.len(), 5);
        s.first_run_done = true;
        s.sources.push(SourceSetting {
            path: "/tmp/x".into(),
            include_subdirs: true,
            excludes: vec![],
            paused: false,
            read_only: false,
            local_only: true,
        });
        s.save(&p).unwrap();
        let back = Settings::load(&p).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn partial_file_uses_defaults() {
        let s: Settings = toml::from_str("first_run_done = true\n[transcription]\nmode = \"open_ai_cheap\"\n").unwrap();
        assert!(s.first_run_done);
        assert_eq!(s.transcription.mode, TranscriptionMode::OpenAiCheap);
        assert_eq!(s.transcription.openai_model, "gpt-transcribe");
        assert_eq!(s.roles.assistant.model, "google/gemma-4-26b-a4b-it");
    }
}
