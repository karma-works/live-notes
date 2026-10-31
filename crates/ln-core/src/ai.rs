//! Role resolution: which client serves the assistant, embeddings and images right now.

use crate::Core;
use ln_ai::{ChatRequest, Client, Message};
use ln_types::{ChatRole, Provider, ProviderKind};

/// Why a role is not usable; shown to the user as guidance.
#[derive(Debug, Clone, PartialEq)]
pub enum Unavailable {
    NoKey(String),
    ModelMissing,
    Offline,
    /// Local LLM blocked during a recording on a small machine (SPEC §7.5).
    BusyRecording,
    Server(String),
    /// The monthly cost limit is reached (SPEC §9.5).
    CostLimit,
    /// The role uses the ChatGPT plan, but nobody is signed in or plan usage was declined.
    ChatGptSignedOut,
}

pub struct RoleClient {
    pub client: Client,
    pub model: String,
    pub cloud: bool,
    _lease: Option<crate::models::ModelLease>,
}

impl Core {
    pub(crate) fn assistant(&self) -> Result<RoleClient, Unavailable> {
        self.role(ChatRole::Assistant)
    }

    /// The client for questions, live suggestions or the summary; each can use its own model.
    pub(crate) fn role(&self, role: ChatRole) -> Result<RoleClient, Unavailable> {
        let s = self.settings();
        let c = s.roles.get(role);
        self.resolve(&c.provider, &c.model, true)
    }

    /// Image understanding role; used for image descriptions from v1.1 on (SPEC §16).
    #[allow(dead_code)]
    pub(crate) fn image_role(&self) -> Result<RoleClient, Unavailable> {
        let s = self.settings();
        self.resolve(&s.roles.image.provider, &s.roles.image.model, true)
    }

    fn resolve(&self, provider: &str, model: &str, chat: bool) -> Result<RoleClient, Unavailable> {
        let s = self.settings();
        let p = s.provider(provider).cloned().ok_or(Unavailable::ModelMissing)?;
        if p.is_cloud && s.privacy.offline_mode {
            return Err(Unavailable::Offline);
        }
        if p.kind == ProviderKind::ChatGpt {
            // Billed to the plan, so the monthly cost limit does not apply.
            let token = self.chatgpt_token()?;
            return Ok(RoleClient { client: Client::new(p, Some(token)), model: model.to_string(), cloud: true, _lease: None });
        }
        if p.is_cloud && self.cost_limit_reached() {
            return Err(Unavailable::CostLimit);
        }
        if p.kind == ProviderKind::LocalLlama {
            if chat && self.recording_active() && crate::models::ram_gb() < 16 {
                return Err(Unavailable::BusyRecording);
            }
            if !self.model_installed(model) {
                return Err(Unavailable::ModelMissing);
            }
            let (base, lease) = if chat { self.models().ensure_chat_server(model) } else { self.models().ensure_embed_server() }
                .map_err(Unavailable::Server)?;
            let provider = Provider { base_url: base, ..p };
            return Ok(RoleClient { client: Client::new(provider, None), model: model.to_string(), cloud: false, _lease: Some(lease) });
        }
        if matches!(p.kind, ProviderKind::OpenRouter | ProviderKind::OpenAi) && !self.has_api_key(provider) {
            return Err(Unavailable::NoKey(p.name.clone()));
        }
        let mut c = Client::new(p.clone(), self.api_key(provider));
        c.deny_data_collection = s.privacy.openrouter_deny_data_collection;
        Ok(RoleClient { client: c, model: model.to_string(), cloud: p.is_cloud, _lease: None })
    }

    /// True if the assistant could answer now (for UI hints).
    pub fn assistant_ready(&self) -> bool {
        self.role_ready(ChatRole::Assistant)
    }

    pub fn role_ready(&self, role: ChatRole) -> bool {
        let s = self.settings();
        let c = s.roles.get(role);
        let Some(p) = s.provider(&c.provider) else { return false };
        if p.is_cloud && s.privacy.offline_mode {
            return false;
        }
        match p.kind {
            ProviderKind::LocalLlama => self.model_installed(&c.model),
            ProviderKind::OpenRouter | ProviderKind::OpenAi => self.has_api_key(&p.id),
            ProviderKind::ChatGpt => self.chatgpt_ready(),
            _ => true,
        }
    }

    /// Moves every chat role that can't run (missing key, model) to the ChatGPT plan.
    /// Returns the roles that were moved.
    pub fn use_chatgpt_for_unready_roles(&self) -> Vec<ChatRole> {
        if !self.chatgpt_ready() {
            return vec![];
        }
        let s = self.settings();
        let model = Some(s.roles.get(ChatRole::Summary))
            .filter(|c| c.provider == "chatgpt")
            .map(|c| c.model.clone())
            .or_else(|| self.chatgpt_models().first().map(|m| m.id.clone()));
        let Some(model) = model else { return vec![] };
        let moved: Vec<ChatRole> = ChatRole::ALL.into_iter().filter(|r| !self.role_ready(*r)).collect();
        if !moved.is_empty() {
            self.update_settings(|s| {
                for r in &moved {
                    s.roles.set(*r, ln_types::RoleConfig { provider: "chatgpt".into(), model: model.clone() });
                }
            });
        }
        moved
    }

    /// What the user still has to do before `role` works, if it is a key or a sign-in.
    pub(crate) fn role_setup_step(&self, role: ChatRole) -> Option<crate::SetupStep> {
        if self.role_ready(role) {
            return None;
        }
        let s = self.settings();
        let p = s.provider(&s.roles.get(role).provider)?;
        match p.kind {
            ProviderKind::ChatGpt => Some(crate::SetupStep::ChatGptSignIn),
            ProviderKind::OpenRouter | ProviderKind::OpenAi if !s.privacy.offline_mode => {
                Some(crate::SetupStep::KeyMissing { provider: p.name.clone(), roles: vec![role] })
            }
            _ => None,
        }
    }

    /// Embeds texts with the embeddings role. `query` adds the query prompt (SPEC §8.4).
    pub(crate) fn embed(&self, texts: &[String], query: bool) -> Result<Vec<Vec<f32>>, Unavailable> {
        let s = self.settings();
        let role = self.resolve(&s.roles.embeddings.provider, &s.roles.embeddings.model, false)?;
        let inputs: Vec<String> =
            if query { texts.iter().map(|t| format!("task: search result | query: {t}")).collect() } else { texts.to_vec() };
        let c = role.client.clone();
        let model = role.model.clone();
        self.runtime().block_on(async move { c.embeddings(&model, &inputs).await }).map_err(|e| Unavailable::Server(e.to_string()))
    }

    pub(crate) fn embedder_available(&self) -> bool {
        let s = self.settings();
        match s.provider(&s.roles.embeddings.provider) {
            Some(p) if p.kind == ProviderKind::LocalLlama => {
                self.model_installed(&s.roles.embeddings.model) && ln_models::find_llama_server().is_some()
            }
            Some(p) if p.is_cloud => !s.privacy.offline_mode && self.has_api_key(&p.id),
            Some(_) => true,
            None => false,
        }
    }

    /// True if a query embedding can be computed without waiting for a model to load. Otherwise
    /// the local embedding server starts in the background and this search uses full text.
    pub(crate) fn embedder_warm(&self) -> bool {
        let s = self.settings();
        let local = s.provider(&s.roles.embeddings.provider).is_some_and(|p| p.kind == ProviderKind::LocalLlama);
        if !local || self.models().embed_server_running() {
            return true;
        }
        self.models().warm_embed_server();
        false
    }

    pub(crate) fn cost_limit_reached(&self) -> bool {
        let limit = self.settings().costs.monthly_limit_usd;
        limit > 0.0 && self.costs.month_total() >= limit
    }

    /// One non-streaming chat call with cost tracking.
    pub(crate) fn chat_once(
        &self,
        role: &RoleClient,
        messages: Vec<Message>,
        json: bool,
        purpose: &str,
        note_id: Option<&str>,
    ) -> Result<String, String> {
        let mut req = ChatRequest::new(role.model.clone(), messages);
        req.json = json;
        let c = role.client.clone();
        let r = self.runtime().block_on(async move { c.chat(&req).await }).map_err(|e| e.user_message(self.ui_language()))?;
        self.costs.record_chat(&role.client.provider().id, &role.model, purpose, note_id, &r.usage);
        Ok(r.text)
    }

    pub(crate) fn unavailable_text(&self, u: &Unavailable) -> String {
        use crate::events::tr;
        let ui = self.ui_language();
        match u {
            Unavailable::NoKey(p) => tr(
                ui,
                &format!("Der {p}-API-Schlüssel fehlt. Trage ihn in den Einstellungen ein oder wähle dort ein anderes Modell."),
                &format!("The {p} API key is missing. Add it in Settings or choose another model there."),
            ),
            Unavailable::ModelMissing => tr(ui, "Das Modell ist noch nicht geladen.", "The model isn't downloaded yet."),
            Unavailable::Offline => tr(ui, "Offline-Modus ist an.", "Offline mode is on."),
            Unavailable::BusyRecording => {
                tr(ui, "Während der Aufnahme ist das lokale Modell pausiert.", "The local model is paused during recording.")
            }
            Unavailable::Server(e) => tr(ui, &format!("Modell nicht erreichbar ({e})."), &format!("Model unavailable ({e}).")),
            Unavailable::ChatGptSignedOut => tr(
                ui,
                "Melde dich in den Einstellungen mit ChatGPT an, um dein Abo zu nutzen.",
                "Sign in with ChatGPT in Settings to use your plan.",
            ),
            Unavailable::CostLimit => tr(
                ui,
                "Das Monatslimit für Cloud-Kosten ist erreicht. Du kannst es in den Einstellungen anheben.",
                "The monthly cloud cost limit is reached. You can raise it in Settings.",
            ),
        }
    }
}
