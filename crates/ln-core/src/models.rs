//! Local models: first-run plan, downloads, llama-server processes, idle unloading (SPEC §4, §7.5).

use crate::events::tr;
use crate::{Core, CoreEvent};
use ln_models::{Downloader, LlamaServer, ModelKind, ModelSpec, ServerRole, by_id, is_installed, model_path};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq)]
pub struct ModelStatus {
    pub id: &'static str,
    pub display: &'static str,
    pub kind: ModelKind,
    pub size: u64,
    pub license: &'static str,
    pub license_url: &'static str,
    pub installed: bool,
    pub progress: Option<f32>,
    pub error: Option<String>,
}

/// Choice on the first-run screen (SPEC §4).
#[derive(Debug, Clone, PartialEq)]
pub enum AssistantChoice {
    OpenRouter { key: Option<String> },
    Local,
    Custom { base_url: String, model: String, key: Option<String> },
}

#[derive(Default)]
struct DlState {
    progress: Option<f32>,
    error: Option<String>,
}

pub struct ModelManager {
    dir: PathBuf,
    state: Mutex<HashMap<&'static str, DlState>>,
    embed: Arc<Mutex<Option<LlamaServer>>>,
    chat: Arc<Mutex<Option<LlamaServer>>>,
    downloading: Mutex<bool>,
}

pub(crate) struct ModelLease(Arc<Mutex<Option<LlamaServer>>>);

impl Drop for ModelLease {
    fn drop(&mut self) {
        if let Some(s) = self.0.lock().as_mut() {
            s.touch();
        }
    }
}

impl ModelManager {
    pub fn new(dir: PathBuf) -> Self {
        ModelManager {
            dir,
            state: Mutex::new(HashMap::new()),
            embed: Arc::new(Mutex::new(None)),
            chat: Arc::new(Mutex::new(None)),
            downloading: Mutex::new(false),
        }
    }

    pub fn installed(&self, id: &str) -> bool {
        by_id(id).is_some_and(|s| is_installed(&self.dir, s))
    }

    pub fn file(&self, id: &str, n: usize) -> Option<PathBuf> {
        let s = by_id(id)?;
        Some(model_path(&self.dir, s, s.files.get(n)?))
    }

    pub fn stop_all(&self) {
        if let Some(mut s) = self.embed.lock().take() {
            s.stop();
        }
        if let Some(mut s) = self.chat.lock().take() {
            s.stop();
        }
    }

    pub fn embed_server_running(&self) -> bool {
        self.embed.try_lock().is_some_and(|mut g| g.as_mut().is_some_and(|s| s.is_running()))
    }

    /// Starts the embeddings server in the background (first search after launch).
    pub fn warm_embed_server(&self) {
        let slot = self.embed.clone();
        let Some(path) = self.file("embeddinggemma-300m", 0).filter(|p| p.exists()) else { return };
        std::thread::spawn(move || {
            let Some(mut g) = slot.try_lock() else { return };
            if g.as_mut().is_some_and(|s| s.is_running()) {
                return;
            }
            if let Ok(s) = LlamaServer::start(ServerRole::Embeddings, &path, None, 2048) {
                *g = Some(s);
            }
        });
    }

    /// Base URL of a running embeddings server, starting it if needed.
    pub fn ensure_embed_server(&self) -> Result<(String, ModelLease), String> {
        let mut g = self.embed.lock();
        if let Some(s) = g.as_mut()
            && s.is_running()
        {
            s.touch();
            return Ok((ServerRole::Embeddings.base_url(), ModelLease(self.embed.clone())));
        }
        let path = self.file("embeddinggemma-300m", 0).filter(|p| p.exists()).ok_or("embedding model not installed")?;
        let s = LlamaServer::start(ServerRole::Embeddings, &path, None, 2048).map_err(|e| e.to_string())?;
        *g = Some(s);
        Ok((ServerRole::Embeddings.base_url(), ModelLease(self.embed.clone())))
    }

    pub fn ensure_chat_server(&self, model_id: &str) -> Result<(String, ModelLease), String> {
        let spec = by_id(model_id).ok_or("unknown local model")?;
        let main = self.file(model_id, 0).filter(|p| p.exists()).ok_or("local model not installed")?;
        let mmproj = self.file(model_id, 1).filter(|p| p.exists());
        let mut g = self.chat.lock();
        if let Some(s) = g.as_mut() {
            if s.is_running() && s.model == main {
                s.touch();
                return Ok((ServerRole::Chat.base_url(), ModelLease(self.chat.clone())));
            }
            if Arc::strong_count(&self.chat) > 1 {
                return Err("Local model is busy; retry when the current request finishes".into());
            }
            s.stop();
        }
        let ctx = if spec.id == "gemma-4-e4b-it" && ram_gb() >= 16 { 8192 } else { 4096 };
        let s = LlamaServer::start(ServerRole::Chat, &main, mmproj.as_deref(), ctx).map_err(|e| e.to_string())?;
        *g = Some(s);
        Ok((ServerRole::Chat.base_url(), ModelLease(self.chat.clone())))
    }

    fn unload_idle(&self, after: Duration) {
        for slot in [&self.embed, &self.chat] {
            let mut g = slot.lock();
            if Arc::strong_count(slot) > 1 {
                continue;
            }
            if g.as_ref().is_some_and(|s| s.idle_for() > after)
                && let Some(mut s) = g.take()
            {
                s.stop();
            }
        }
    }
}

pub fn ram_gb() -> u32 {
    #[cfg(target_os = "macos")]
    {
        let out = std::process::Command::new("sysctl").args(["-n", "hw.memsize"]).output().ok();
        let bytes: u64 = out.and_then(|o| String::from_utf8(o.stdout).ok()).and_then(|s| s.trim().parse().ok()).unwrap_or(8 << 30);
        ((bytes as f64) / (1u64 << 30) as f64).round() as u32
    }
    #[cfg(not(target_os = "macos"))]
    {
        std::fs::read_to_string("/proc/meminfo")
            .ok()
            .and_then(|s| s.lines().find(|l| l.starts_with("MemTotal")).and_then(|l| l.split_whitespace().nth(1)?.parse::<u64>().ok()))
            .map(|kb| (kb as f64 / (1u64 << 20) as f64).round() as u32)
            .unwrap_or(8)
    }
}

pub fn spawn_idle_unloader(core: &Arc<Core>) {
    let weak = Arc::downgrade(core);
    std::thread::Builder::new()
        .name("ln-unload".into())
        .spawn(move || {
            loop {
                std::thread::sleep(Duration::from_secs(30));
                let Some(core) = weak.upgrade() else { break };
                let mins = core.settings().general.unload_models_after_min.max(1) as u64;
                core.models.unload_idle(Duration::from_secs(mins * 60));
            }
        })
        .ok();
}

impl Core {
    /// Models the first-run screen offers to download (SPEC §4, §16: v1.0 without local image model).
    pub fn first_run_plan(&self, assistant_local: bool) -> Vec<&'static ModelSpec> {
        let mut v = vec![
            by_id("whisper-large-v3-turbo-q5_0").unwrap(),
            by_id("whisper-base-q8_0").unwrap(),
            by_id("silero-vad-v5").unwrap(),
            by_id("embeddinggemma-300m").unwrap(),
        ];
        if assistant_local {
            v.push(ln_models::recommended_llm(ram_gb()));
        }
        v
    }

    /// Applies the first-run choice and starts the downloads in the background.
    pub fn complete_first_run(self: &Arc<Self>, choice: AssistantChoice) {
        let local = matches!(choice, AssistantChoice::Local);
        match &choice {
            AssistantChoice::OpenRouter { key: Some(k) } => {
                let _ = self.set_api_key("openrouter", k);
            }
            AssistantChoice::Custom { key: Some(k), .. } => {
                let _ = self.set_api_key("custom", k);
            }
            _ => {}
        }
        self.update_settings(|s| {
            s.first_run_done = true;
            s.gemma_terms_accepted = true;
            match &choice {
                AssistantChoice::OpenRouter { .. } => {
                    s.roles.assistant = ln_types::RoleConfig { provider: "openrouter".into(), model: "google/gemma-4-26b-a4b-it".into() };
                }
                AssistantChoice::Local => {
                    s.roles.assistant =
                        ln_types::RoleConfig { provider: "local".into(), model: ln_models::recommended_llm(ram_gb()).id.into() };
                }
                AssistantChoice::Custom { base_url, model, .. } => {
                    s.providers.retain(|p| p.id != "custom");
                    s.providers.push(ln_types::Provider {
                        id: "custom".into(),
                        name: "Custom".into(),
                        kind: ln_types::ProviderKind::Custom,
                        base_url: base_url.clone(),
                        headers: vec![],
                        is_cloud: !base_url.contains("127.0.0.1") && !base_url.contains("localhost"),
                    });
                    s.roles.assistant = ln_types::RoleConfig { provider: "custom".into(), model: model.clone() };
                }
            }
            // Suggestions and summary start with the same model; settings can split them later.
            s.roles.suggestions = Some(s.roles.assistant.clone());
            s.roles.summary = Some(s.roles.assistant.clone());
        });
        let ids: Vec<&'static str> = self.first_run_plan(local).iter().map(|m| m.id).collect();
        self.download_models(&ids);
    }

    pub fn model_status(&self) -> Vec<ModelStatus> {
        let st = self.models.state.lock();
        ln_models::CATALOG
            .iter()
            .map(|m| {
                let s = st.get(m.id);
                ModelStatus {
                    id: m.id,
                    display: m.display,
                    kind: m.kind,
                    size: m.total_size(),
                    license: m.license,
                    license_url: m.license_url,
                    installed: is_installed(&self.models.dir, m),
                    progress: s.and_then(|s| s.progress),
                    error: s.and_then(|s| s.error.clone()),
                }
            })
            .collect()
    }

    pub fn model_installed(&self, id: &str) -> bool {
        self.models.installed(id)
    }

    /// Downloads in order, smallest first (SPEC §4). Already installed models are skipped.
    pub fn download_models(self: &Arc<Self>, ids: &[&'static str]) {
        if self.settings().privacy.offline_mode {
            self.emit(CoreEvent::Notice(tr(
                self.ui_language(),
                "Offline-Modus ist an – keine Downloads.",
                "Offline mode is on – no downloads.",
            )));
            return;
        }
        let mut specs: Vec<&'static ModelSpec> =
            ids.iter().filter_map(|i| by_id(i)).filter(|s| !is_installed(&self.models.dir, s)).collect();
        specs.sort_by_key(|s| s.total_size());
        if specs.is_empty() {
            return;
        }
        {
            let mut d = self.models.downloading.lock();
            if *d {
                return;
            }
            *d = true;
        }
        let me = self.clone();
        self.rt.spawn(async move {
            let dl = Downloader::new(me.models.dir.clone());
            for spec in specs {
                me.models.state.lock().entry(spec.id).or_default().progress = Some(0.0);
                me.emit(CoreEvent::ModelsChanged);
                let me2 = me.clone();
                let r = dl
                    .download(spec, move |p| {
                        me2.models.state.lock().entry(p.model_id).or_default().progress = Some(p.fraction());
                        me2.emit(CoreEvent::DownloadProgress { model: p.model_id, fraction: p.fraction() });
                    })
                    .await;
                {
                    let mut st = me.models.state.lock();
                    let e = st.entry(spec.id).or_default();
                    e.progress = None;
                    e.error = r.as_ref().err().map(|e| e.to_string());
                }
                if let Err(e) = r {
                    tracing::error!("download {}: {e}", spec.id);
                    me.emit(CoreEvent::Notice(tr(
                        me.ui_language(),
                        &format!("Download von {} fehlgeschlagen. Er wird beim nächsten Start fortgesetzt.", spec.display),
                        &format!("Download of {} failed. It resumes on the next start.", spec.display),
                    )));
                }
                me.emit(CoreEvent::ModelsChanged);
                if spec.kind == ModelKind::Embedding {
                    me.indexer.kick();
                }
            }
            *me.models.downloading.lock() = false;
        });
    }

    /// Resumes downloads of the first-run plan that did not finish (called at startup by the UI).
    pub fn resume_downloads(self: &Arc<Self>) {
        let s = self.settings();
        if !s.first_run_done {
            return;
        }
        let local = ln_types::ChatRole::ALL.iter().any(|r| s.roles.get(*r).provider == "local");
        let ids: Vec<&'static str> = self.first_run_plan(local).iter().map(|m| m.id).collect();
        self.download_models(&ids);
    }

    pub fn delete_model(&self, id: &str) {
        if let Some(spec) = by_id(id) {
            self.models.stop_all();
            let _ = std::fs::remove_dir_all(self.models.dir.join(spec.id));
            self.emit(CoreEvent::ModelsChanged);
        }
    }

    pub(crate) fn models(&self) -> &ModelManager {
        &self.models
    }
}
