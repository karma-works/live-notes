//! The engine behind the UI (SPEC §7.1). `ln-app` only talks to [`Core`] and listens to
//! [`CoreEvent`]s; everything here is testable without a window.

mod ai;
mod ask;
mod chatgpt;
mod events;
mod indexer;
mod models;
mod notes;
mod probe;
mod recording;
mod suggest;
mod summary;

pub use ask::{AskScope, Citation};
pub use chatgpt::ChatGptStatus;
pub use events::{CoreEvent, EventReceiver};
pub use ln_ai::chatgpt::USAGE_URL as CHATGPT_USAGE_URL;
pub use ln_ai::prompts::Task;
pub use ln_audio::{CaptureConfig, Source};
pub use ln_index::{SearchFilter, SearchHit, SourceRow};
pub use models::{AssistantChoice, ModelStatus};
pub use notes::NoteEntry;

/// A setup step that is still open (shown as a checklist in the sidebar).
#[derive(Debug, Clone, PartialEq)]
pub enum SetupStep {
    ModelsDownloading,
    TranscriptionModelMissing,
    MicrophoneDenied,
    /// Provider name.
    AssistantKeyMissing(String),
    /// A role uses the ChatGPT plan, but nobody is signed in (or plan usage was declined).
    ChatGptSignIn,
    LlamaServerMissing,
}
pub use probe::{ChannelProbe, ProbeReport};
pub use recording::RecordingInfo;
pub use suggest::SuggestionCard;

use events::EventSender;
use ln_ai::CostTracker;
use ln_index::Index;
use ln_platform::SecretStore;
use ln_types::{AppPaths, Settings, UiLanguage};
use parking_lot::{Mutex, RwLock};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    User(String),
    #[error(transparent)]
    Index(#[from] ln_index::Error),
    #[error(transparent)]
    Notes(#[from] ln_notes::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
pub type Result<T, E = Error> = std::result::Result<T, E>;

pub struct Core {
    pub paths: AppPaths,
    settings: RwLock<Settings>,
    index: Mutex<Index>,
    secrets: Arc<dyn SecretStore>,
    costs: CostTracker,
    rt: tokio::runtime::Runtime,
    events: EventSender,
    models: models::ModelManager,
    indexer: indexer::IndexerHandle,
    recording: Mutex<Option<recording::Session>>,
    /// Suggestions for the user's own notes, not yet accepted (kept out of the file).
    additions: Mutex<std::collections::HashMap<PathBuf, Vec<String>>>,
    chatgpt: chatgpt::State,
}

/// Opens a URL in the user's default browser.
pub(crate) fn open_in_browser(url: &str) {
    #[cfg(target_os = "macos")]
    let r = std::process::Command::new("open").arg(url).spawn();
    #[cfg(not(target_os = "macos"))]
    let r = std::process::Command::new("xdg-open").arg(url).spawn();
    if let Err(e) = r {
        tracing::error!("opening the browser failed: {e}");
    }
}

impl Core {
    /// Opens settings and index, recovers interrupted recordings, starts background indexing.
    pub fn start(paths: AppPaths, secrets: Arc<dyn SecretStore>) -> Result<(Arc<Core>, EventReceiver)> {
        paths.ensure()?;
        let settings = Settings::load(&paths.settings_file()).unwrap_or_else(|e| {
            tracing::warn!("settings unreadable, using defaults: {e}");
            Settings::default()
        });
        let index = Index::open(&paths.index_db())?;
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().thread_name("ln-core").build()?;
        let (events, rx) = events::channel();
        let costs = CostTracker::open(&paths.data_dir.join("costs.jsonl"));
        let models = models::ModelManager::new(paths.models_dir());
        let core = Arc::new(Core {
            paths,
            settings: RwLock::new(settings),
            index: Mutex::new(index),
            secrets,
            costs,
            rt,
            events,
            models,
            indexer: indexer::IndexerHandle::default(),
            recording: Mutex::new(None),
            additions: Mutex::new(Default::default()),
            chatgpt: Default::default(),
        });
        std::fs::create_dir_all(core.notes_dir())?;
        recording::recover(&core);
        indexer::spawn(&core);
        models::spawn_idle_unloader(&core);
        Ok((core, rx))
    }

    // ---- settings ------------------------------------------------------------------------

    pub fn settings(&self) -> Settings {
        self.settings.read().clone()
    }

    pub fn update_settings(&self, f: impl FnOnce(&mut Settings)) {
        let (old_sources, new) = {
            let mut s = self.settings.write();
            let old = s.sources.clone();
            f(&mut s);
            (old, s.clone())
        };
        if let Err(e) = new.save(&self.paths.settings_file()) {
            tracing::error!("saving settings: {e}");
        }
        if old_sources != new.sources {
            self.indexer.resync_all();
        }
        self.events.send(CoreEvent::SettingsChanged);
    }

    pub fn ui_language(&self) -> UiLanguage {
        self.settings.read().ui_language()
    }

    pub fn notes_dir(&self) -> PathBuf {
        self.settings.read().notes_dir(&self.paths)
    }

    /// Physical memory in GB (rounded).
    pub fn ram_gb() -> u32 {
        models::ram_gb()
    }

    // ---- keys ----------------------------------------------------------------------------

    pub fn has_api_key(&self, provider: &str) -> bool {
        self.api_key(provider).is_some()
    }

    pub(crate) fn api_key(&self, provider: &str) -> Option<String> {
        let env = match provider {
            "openrouter" => std::env::var("LN_TEST_OPENROUTER_KEY").ok(),
            "openai" => std::env::var("LN_TEST_OPENAI_KEY").ok(),
            _ => None,
        };
        env.or_else(|| self.secrets.get(provider)).filter(|k| !k.trim().is_empty())
    }

    pub fn set_api_key(&self, provider: &str, key: &str) -> Result<()> {
        let r = if key.trim().is_empty() { self.secrets.delete(provider) } else { self.secrets.set(provider, key.trim()) };
        r.map_err(|e| Error::User(e.to_string()))?;
        self.events.send(CoreEvent::SettingsChanged);
        Ok(())
    }

    /// "Test connection". Runs in the background; the result arrives as `ProviderTested`.
    pub fn test_provider(self: &Arc<Self>, provider: &str) {
        let me = self.clone();
        let id = provider.to_string();
        self.rt.spawn(async move {
            let result = match me.client_for(&id) {
                Some(c) => c.test_connection().await.map_err(|e| e.user_message(me.ui_language())),
                None => Err("Unknown provider".into()),
            };
            me.events.send(CoreEvent::ProviderTested { provider: id, result });
        });
    }

    pub(crate) fn client_for(&self, provider: &str) -> Option<ln_ai::Client> {
        let key = match self.settings.read().provider(provider)?.kind {
            ln_types::ProviderKind::ChatGpt => self.chatgpt_token_cached(),
            _ => self.api_key(provider),
        };
        self.client_with_key(provider, key)
    }

    pub(crate) fn client_with_token(&self, provider: &str, token: String) -> Option<ln_ai::Client> {
        self.client_with_key(provider, Some(token))
    }

    fn client_with_key(&self, provider: &str, key: Option<String>) -> Option<ln_ai::Client> {
        let s = self.settings.read();
        let p = s.provider(provider)?.clone();
        let mut c = ln_ai::Client::new(p, key);
        c.deny_data_collection = s.privacy.openrouter_deny_data_collection;
        Some(c)
    }

    // ---- costs ---------------------------------------------------------------------------

    pub fn month_cost(&self) -> f64 {
        self.costs.month_total()
    }

    pub fn note_cost(&self, note_id: &str) -> f64 {
        self.costs.note_total(note_id)
    }

    // ---- sources -------------------------------------------------------------------------

    pub fn add_source(&self, path: &Path) {
        let p = path.to_path_buf();
        self.update_settings(|s| {
            if !s.sources.iter().any(|x| x.path == p) {
                s.sources.push(ln_types::SourceSetting {
                    path: p,
                    include_subdirs: true,
                    excludes: vec![],
                    paused: false,
                    read_only: false,
                    local_only: false,
                });
            }
        });
        self.events.send(CoreEvent::SourcesChanged);
    }

    /// Removes the source from the index. Files are never touched.
    pub fn remove_source(&self, path: &Path) {
        let p = path.to_path_buf();
        self.update_settings(|s| s.sources.retain(|x| x.path != p));
        let ix = self.index.lock();
        if let Ok(Some(id)) = ix.source_id(path) {
            let _ = ix.remove_source(id);
        }
        drop(ix);
        self.events.send(CoreEvent::SourcesChanged);
    }

    pub fn sources(&self) -> Vec<SourceRow> {
        self.index.lock().sources().unwrap_or_default()
    }

    pub fn index_stats(&self) -> ln_index::IndexStats {
        self.index.lock().stats().unwrap_or_default()
    }

    /// "Rebuild index": drops everything and indexes again.
    pub fn rebuild_index(&self) {
        {
            let ix = self.index.lock();
            for s in ix.sources().unwrap_or_default() {
                let _ = ix.remove_source(s.id);
            }
        }
        self.indexer.resync_all();
    }

    // ---- setup guidance (SPEC §6.1 rule 6) ------------------------------------------------

    /// What is still missing before every feature works, in the order the user should fix it.
    pub fn setup_steps(&self) -> Vec<SetupStep> {
        let mut v = Vec::new();
        if self.model_status().iter().any(|m| m.progress.is_some()) {
            v.push(SetupStep::ModelsDownloading);
        }
        if !self.model_installed(&self.settings().transcription.local_model)
            && self.settings().transcription.mode == ln_types::TranscriptionMode::Local
        {
            v.push(SetupStep::TranscriptionModelMissing);
        }
        if ln_platform::microphone_permission() == ln_platform::Permission::Denied {
            v.push(SetupStep::MicrophoneDenied);
        }
        // One step per missing key or sign-in, even if several roles share it.
        let s = self.settings();
        for role in ln_types::ChatRole::ALL {
            if role == ln_types::ChatRole::Suggestions && !s.live_suggestions.enabled {
                continue;
            }
            if role == ln_types::ChatRole::Summary && !s.notes.auto_summary {
                continue;
            }
            if let Some(step) = self.role_setup_step(role)
                && !v.contains(&step)
            {
                v.push(step);
            }
        }
        if ln_models::find_llama_server().is_none() {
            v.push(SetupStep::LlamaServerMissing);
        }
        v
    }

    // ---- misc ----------------------------------------------------------------------------

    pub(crate) fn emit(&self, e: CoreEvent) {
        self.events.send(e);
    }

    pub fn runtime(&self) -> &tokio::runtime::Handle {
        self.rt.handle()
    }

    /// Stops recording (saving it), background work and local model servers.
    pub fn shutdown(&self) {
        if self.recording.lock().is_some() {
            self.stop_recording_blocking();
        }
        self.indexer.shutdown();
        self.models.stop_all();
    }
}

#[cfg(test)]
mod tests;
