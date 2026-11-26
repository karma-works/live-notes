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
    pub categories: Categories,
    /// Local HTTP endpoint for the browser extension (`docs/clip-api.md`).
    pub clip: ClipSettings,
}

/// Default port of the clip endpoint on 127.0.0.1.
pub const CLIP_DEFAULT_PORT: u16 = 47823;

/// The local endpoint that saves web pages and YouTube videos as notes. The bearer token lives
/// in the secret store, never here.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ClipSettings {
    pub enabled: bool,
    /// Port on 127.0.0.1; 0 picks a free port (tests).
    pub port: u16,
}

impl Default for ClipSettings {
    fn default() -> Self {
        ClipSettings { enabled: true, port: CLIP_DEFAULT_PORT }
    }
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
    /// The user's ChatGPT Plus/Pro plan via Sign in with ChatGPT (Responses API, OAuth token).
    ChatGpt,
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
    pub fn builtin() -> Vec<Provider> {
        vec![
            Provider::openrouter(),
            Provider::openai(),
            Provider::chatgpt(),
            Provider::local_llama(),
            Provider::ollama(),
            Provider::lm_studio(),
        ]
    }
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
    /// Requests are billed to the user's ChatGPT plan, not per token.
    pub fn chatgpt() -> Self {
        Provider {
            id: "chatgpt".into(),
            name: "ChatGPT".into(),
            kind: ProviderKind::ChatGpt,
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
    /// Questions with sources.
    pub assistant: RoleConfig,
    /// Live suggestions during a recording: short prompts, should be fast. `None` = as `assistant`.
    pub suggestions: Option<RoleConfig>,
    /// Summary after a recording: runs once in the background. `None` = as `assistant`.
    pub summary: Option<RoleConfig>,
    pub image: RoleConfig,
    pub embeddings: RoleConfig,
}

/// The chat roles the user can give separate models.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChatRole {
    Assistant,
    Suggestions,
    Summary,
}

impl ChatRole {
    pub const ALL: [ChatRole; 3] = [ChatRole::Assistant, ChatRole::Suggestions, ChatRole::Summary];
    pub fn id(self) -> &'static str {
        match self {
            ChatRole::Assistant => "assistant",
            ChatRole::Suggestions => "suggestions",
            ChatRole::Summary => "summary",
        }
    }
}

impl Roles {
    pub fn get(&self, r: ChatRole) -> &RoleConfig {
        match r {
            ChatRole::Assistant => &self.assistant,
            ChatRole::Suggestions => self.suggestions.as_ref().unwrap_or(&self.assistant),
            ChatRole::Summary => self.summary.as_ref().unwrap_or(&self.assistant),
        }
    }
    pub fn set(&mut self, r: ChatRole, c: RoleConfig) {
        match r {
            ChatRole::Assistant => self.assistant = c,
            ChatRole::Suggestions => self.suggestions = Some(c),
            ChatRole::Summary => self.summary = Some(c),
        }
    }
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

/// Colours of the macOS Finder tags; each note category gets one.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "lowercase")]
pub enum TagColor {
    Red,
    Orange,
    Yellow,
    Green,
    #[default]
    Blue,
    Purple,
    Gray,
}

impl TagColor {
    pub const ALL: [TagColor; 7] =
        [TagColor::Red, TagColor::Orange, TagColor::Yellow, TagColor::Green, TagColor::Blue, TagColor::Purple, TagColor::Gray];

    /// The macOS system colour as `0xRRGGBB`.
    pub fn rgb(self) -> u32 {
        match self {
            TagColor::Red => 0xFF3B30,
            TagColor::Orange => 0xFF9500,
            TagColor::Yellow => 0xFFCC00,
            TagColor::Green => 0x34C759,
            TagColor::Blue => 0x007AFF,
            TagColor::Purple => 0xAF52DE,
            TagColor::Gray => 0x8E8E93,
        }
    }

    /// The colour after this one, for cycling through them.
    pub fn next(self) -> TagColor {
        let i = TagColor::ALL.iter().position(|c| *c == self).unwrap_or(0);
        TagColor::ALL[(i + 1) % TagColor::ALL.len()]
    }
}

/// A user-defined note category, e.g. "Chinesisch-Kurs".
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Category {
    pub name: String,
    #[serde(default)]
    pub color: TagColor,
    /// Optional hint for the decision model: what belongs here.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub hint: String,
}

/// Which API serves the decision model.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum DecisionProvider {
    /// OpenRouter when its key is set, otherwise OpenAI.
    #[default]
    Auto,
    OpenRouter,
    OpenAi,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Categories {
    pub list: Vec<Category>,
    /// Sort new and saved notes and new transcripts into categories automatically.
    pub auto_assign: bool,
    pub provider: DecisionProvider,
    /// Decision model on OpenRouter (`POST /api/alpha/decisions`).
    pub openrouter_model: String,
    /// Model for OpenAI's Decisions API (`POST /v1/decisions`).
    pub openai_model: String,
    /// A category is assigned from this probability on.
    pub threshold: f32,
}

impl Default for Categories {
    fn default() -> Self {
        Categories {
            list: vec![],
            auto_assign: true,
            provider: DecisionProvider::Auto,
            openrouter_model: "typesafe/jev-1.14".into(),
            openai_model: "gpt-6-luna".into(),
            threshold: 0.5,
        }
    }
}

impl Categories {
    pub fn color_of(&self, name: &str) -> Option<TagColor> {
        self.list.iter().find(|c| c.name == name).map(|c| c.color)
    }

    /// The first colour not used yet, so new categories are easy to tell apart.
    pub fn next_color(&self) -> TagColor {
        TagColor::ALL
            .into_iter()
            .find(|c| !self.list.iter().any(|x| x.color == *c))
            .unwrap_or(TagColor::ALL[self.list.len() % TagColor::ALL.len()])
    }
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
            providers: Provider::builtin(),
            roles: Roles::default(),
            live_suggestions: LiveSuggestions::default(),
            notes: NotesSettings::default(),
            privacy: Privacy::default(),
            costs: Costs::default(),
            sources: vec![],
            categories: Categories::default(),
            clip: ClipSettings::default(),
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
            // `None`: the assistant's model (Gemma 4 26B-A4B, 4B active parameters, fast enough for
            // live prompts). Must stay `None` so older files take over their assistant (`migrated`).
            suggestions: None,
            summary: None,
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
            Ok(s) => toml::from_str::<Settings>(&s).map(Settings::migrated).map_err(|e| crate::Error::Settings(e.to_string())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default().migrated()),
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

    /// Fills what older settings files lack: new built-in providers, and separate models for
    /// suggestions and summary (taken over from the assistant, which served both before).
    fn migrated(mut self) -> Settings {
        for p in Provider::builtin() {
            if self.provider(&p.id).is_none() {
                self.providers.push(p);
            }
        }
        if self.roles.suggestions.is_none() {
            self.roles.suggestions = Some(self.roles.assistant.clone());
        }
        if self.roles.summary.is_none() {
            self.roles.summary = Some(self.roles.assistant.clone());
        }
        self
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
        assert_eq!(s, Settings::default().migrated());
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
        assert_eq!(s.clip, ClipSettings { enabled: true, port: CLIP_DEFAULT_PORT });
    }

    #[test]
    fn clip_settings_roundtrip() {
        let s: Settings = toml::from_str("[clip]\nenabled = false\n").unwrap();
        assert!(!s.clip.enabled);
        assert_eq!(s.clip.port, CLIP_DEFAULT_PORT);
        let back: Settings = toml::from_str(&toml::to_string_pretty(&s).unwrap()).unwrap();
        assert_eq!(back.clip, s.clip);
    }

    #[test]
    fn categories_roundtrip_and_colors() {
        let s: Settings = toml::from_str(
            "[categories]\nprovider = \"open_ai\"\n[[categories.list]]\nname = \"Chinesisch-Kurs\"\ncolor = \"red\"\n[[categories.list]]\nname = \"Arztagent\"\n",
        )
        .unwrap();
        assert_eq!(s.categories.provider, DecisionProvider::OpenAi);
        assert_eq!(s.categories.openrouter_model, "typesafe/jev-1.14");
        assert!(s.categories.auto_assign);
        assert_eq!(s.categories.color_of("Chinesisch-Kurs"), Some(TagColor::Red));
        assert_eq!(s.categories.color_of("Arztagent"), Some(TagColor::Blue));
        assert_eq!(s.categories.color_of("Unbekannt"), None);
        assert_eq!(s.categories.next_color(), TagColor::Orange);
        assert_eq!(TagColor::Gray.next(), TagColor::Red);
        let back: Settings = toml::from_str(&toml::to_string_pretty(&s).unwrap()).unwrap();
        assert_eq!(back.categories, s.categories);
    }

    #[test]
    fn old_file_gets_separate_roles_and_chatgpt() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("settings.toml");
        let old = "[[providers]]\nid = \"local\"\nname = \"Lokal\"\nkind = \"local_llama\"\nbase_url = \"http://127.0.0.1:18432/v1\"\nis_cloud = false\n\n[roles.assistant]\nprovider = \"local\"\nmodel = \"gemma-4-e4b-it\"\n";
        std::fs::write(&p, old).unwrap();
        let s = Settings::load(&p).unwrap();
        assert_eq!(s.roles.get(ChatRole::Summary).provider, "local");
        assert_eq!(s.roles.get(ChatRole::Suggestions).model, "gemma-4-e4b-it");
        assert_eq!(s.provider("chatgpt").map(|p| p.kind), Some(ProviderKind::ChatGpt));
        let mut r = s.roles.clone();
        r.set(ChatRole::Summary, RoleConfig { provider: "chatgpt".into(), model: "m".into() });
        assert_eq!(r.get(ChatRole::Summary).provider, "chatgpt");
        assert_eq!(r.get(ChatRole::Assistant).provider, "local");
    }
}
