//! The user's ChatGPT plan as a model provider (Sign in with ChatGPT, see `ln_ai::chatgpt`).
//! Tokens live in the keychain; the opaque host ID in the data folder.

use crate::events::tr;
use crate::{Core, CoreEvent};
use ln_ai::ModelInfo;
use ln_ai::chatgpt::{self, Credentials, Endpoints, Registration};
use ln_types::{ChatRole, RoleConfig};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const SECRET_KEY: &str = "chatgpt";
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(300);

#[derive(Debug, Clone, PartialEq)]
pub enum ChatGptStatus {
    SignedOut,
    /// The browser is open; waiting for the user.
    SigningIn,
    SignedIn {
        email: Option<String>,
        /// The user allowed requests on their plan (scope `chatgpt.tokens.use.direct`).
        plan_usage: bool,
    },
}

/// What the keychain holds. The registration stays after sign-out, so the next sign-in reuses
/// the issued client ID and with it the usage limits the user set for this app.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct Stored {
    registration: Option<Registration>,
    credentials: Option<Credentials>,
}

pub(crate) struct State {
    pub(crate) stored: Mutex<Option<Stored>>,
    /// Serialises refreshes: the refresh token rotates.
    refresh: Mutex<()>,
    signing_in: AtomicBool,
    cancel: AtomicBool,
    models: Mutex<Vec<ModelInfo>>,
    pub(crate) endpoints: Mutex<Endpoints>,
}

impl Default for State {
    fn default() -> Self {
        State {
            stored: Mutex::new(None),
            refresh: Mutex::new(()),
            signing_in: AtomicBool::new(false),
            cancel: AtomicBool::new(false),
            models: Mutex::new(Vec::new()),
            endpoints: Mutex::new(Endpoints::default()),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct HostFile {
    ext_agent_host_id: String,
}

fn http() -> ln_ai::HttpClient {
    ln_ai::http_client()
}

impl Core {
    fn chatgpt_load(&self) -> Stored {
        let mut g = self.chatgpt.stored.lock();
        if g.is_none() {
            let s = self.secrets.get(SECRET_KEY).and_then(|j| serde_json::from_str::<Stored>(&j).ok()).unwrap_or_default();
            *g = Some(s);
        }
        g.clone().unwrap_or_default()
    }

    fn chatgpt_save(&self, s: Stored) {
        match serde_json::to_string(&s) {
            Ok(j) => {
                if let Err(e) = self.secrets.set(SECRET_KEY, &j) {
                    tracing::error!("chatgpt: saving credentials failed: {e}");
                }
            }
            Err(e) => tracing::error!("chatgpt: {e}"),
        }
        *self.chatgpt.stored.lock() = Some(s);
    }

    /// Stable per installation, created before the first sign-in.
    fn chatgpt_host_id(&self) -> String {
        let path = self.paths.data_dir.join("chatgpt-host.json");
        if let Some(h) = std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str::<HostFile>(&t).ok()) {
            return h.ext_agent_host_id;
        }
        let id = chatgpt::new_host_id();
        let _ = std::fs::create_dir_all(&self.paths.data_dir);
        let _ = std::fs::write(&path, serde_json::to_string(&HostFile { ext_agent_host_id: id.clone() }).unwrap_or_default());
        id
    }

    pub fn chatgpt_status(&self) -> ChatGptStatus {
        if self.chatgpt.signing_in.load(Ordering::Relaxed) {
            return ChatGptStatus::SigningIn;
        }
        match self.chatgpt_load().credentials {
            Some(c) => ChatGptStatus::SignedIn { email: c.registration.email.clone(), plan_usage: c.plan_usage() },
            None => ChatGptStatus::SignedOut,
        }
    }

    pub(crate) fn chatgpt_ready(&self) -> bool {
        self.chatgpt_load().credentials.is_some_and(|c| c.plan_usage())
    }

    /// Models the signed-in account may use, as listed by OpenAI (cached after sign-in).
    pub fn chatgpt_models(&self) -> Vec<ModelInfo> {
        self.chatgpt.models.lock().clone()
    }

    /// Opens the browser for Sign in with ChatGPT. `consent`: ask again for plan usage after the
    /// user declined it. Progress arrives as `ChatGptChanged`, failures as `Notice`.
    pub fn chatgpt_sign_in(self: &Arc<Self>, consent: bool) {
        if self.chatgpt.signing_in.swap(true, Ordering::Relaxed) {
            return;
        }
        self.chatgpt.cancel.store(false, Ordering::Relaxed);
        self.emit(CoreEvent::ChatGptChanged { first_sign_in: false });
        let me = self.clone();
        std::thread::Builder::new()
            .name("ln-chatgpt-sign-in".into())
            .spawn(move || {
                let r = me.chatgpt_sign_in_blocking(consent);
                me.chatgpt.signing_in.store(false, Ordering::Relaxed);
                match r {
                    Ok(first) => me.emit(CoreEvent::ChatGptChanged { first_sign_in: first }),
                    Err(msg) => {
                        me.emit(CoreEvent::ChatGptChanged { first_sign_in: false });
                        if let Some(m) = msg {
                            me.emit(CoreEvent::Notice(m));
                        }
                    }
                }
            })
            .ok();
    }

    pub fn chatgpt_cancel_sign_in(&self) {
        self.chatgpt.cancel.store(true, Ordering::Relaxed);
    }

    /// Ok(first sign-in with plan usage) or Err(message; `None` when the user cancelled).
    fn chatgpt_sign_in_blocking(&self, consent: bool) -> Result<bool, Option<String>> {
        let ui = self.ui_language();
        let fail = |e: ln_ai::Error| {
            tracing::warn!("chatgpt sign-in failed: {e}");
            Some(format!("{} {}", tr(ui, "Anmeldung bei ChatGPT fehlgeschlagen:", "ChatGPT sign-in failed:"), e))
        };
        let stored = self.chatgpt_load();
        let first = stored.registration.is_none();
        let endpoints = self.chatgpt.endpoints.lock().clone();
        let attempt = chatgpt::begin(endpoints, &self.chatgpt_host_id(), stored.registration.as_ref(), consent).map_err(fail)?;
        tracing::info!("chatgpt: opening the browser for sign-in (new registration: {first})");
        crate::open_in_browser(&attempt.url);
        let cb = match attempt.wait(SIGN_IN_TIMEOUT, &self.chatgpt.cancel) {
            Ok(cb) => cb,
            Err(ln_ai::Error::Cancelled) => return Err(None),
            Err(ln_ai::Error::Api { message, .. }) if message.contains("access_denied") => {
                return Err(Some(tr(ui, "Die Anmeldung bei ChatGPT wurde abgebrochen.", "ChatGPT sign-in was declined.")));
            }
            Err(e) => return Err(fail(e)),
        };
        let expected = stored.registration.as_ref().map(|r| r.subject.as_str());
        let creds = self.runtime().block_on(attempt.finish(&http(), &cb, expected)).map_err(fail)?;
        let plan = creds.plan_usage();
        tracing::info!("chatgpt: signed in, plan usage {plan}");
        self.chatgpt_save(Stored { registration: Some(creds.registration.clone()), credentials: Some(creds) });
        if !plan {
            return Err(Some(tr(
                ui,
                "Angemeldet, aber die Nutzung deines ChatGPT-Abos wurde nicht erlaubt. Erlaube sie in den Einstellungen.",
                "Signed in, but using your ChatGPT plan was not allowed. Allow it in Settings.",
            )));
        }
        self.chatgpt_refresh_models();
        // The summary is what the plan is for: switch it over unless the user chose ChatGPT already.
        let s = self.settings();
        if s.roles.get(ChatRole::Summary).provider != "chatgpt"
            && let Some(m) = self.chatgpt_models().first()
        {
            let model = m.id.clone();
            self.update_settings(|s| s.roles.set(ChatRole::Summary, RoleConfig { provider: "chatgpt".into(), model }));
        }
        // Roles that could not run at all (no key) use the plan too, instead of staying broken.
        let moved = self.use_chatgpt_for_unready_roles();
        if !moved.is_empty() {
            tracing::info!("chatgpt: roles without a usable model moved to the plan: {moved:?}");
        }
        Ok(first)
    }

    pub(crate) fn chatgpt_refresh_models(&self) {
        let Ok(token) = self.chatgpt_token() else { return };
        let Some(c) = self.client_with_token("chatgpt", token) else { return };
        match self.runtime().block_on(async move { c.models().await }) {
            Ok(m) => *self.chatgpt.models.lock() = m,
            Err(e) => tracing::warn!("chatgpt: model list failed: {e}"),
        }
    }

    /// Loads the model list in the background if it is not cached yet (settings view).
    pub fn chatgpt_load_models(self: &Arc<Self>) {
        if !self.chatgpt_models().is_empty() || !self.chatgpt_ready() {
            return;
        }
        let me = self.clone();
        std::thread::spawn(move || {
            me.chatgpt_refresh_models();
            me.emit(CoreEvent::ChatGptChanged { first_sign_in: false });
        });
    }

    /// Ends the session at OpenAI and clears the tokens; the registration is kept.
    pub fn chatgpt_sign_out(self: &Arc<Self>) {
        let stored = self.chatgpt_load();
        let me = self.clone();
        std::thread::spawn(move || {
            if let Some(c) = &stored.credentials {
                let ep = me.chatgpt.endpoints.lock().clone();
                if let Err(e) = me.runtime().block_on(chatgpt::revoke(&http(), &ep, c)) {
                    tracing::warn!("chatgpt: revocation not confirmed: {e}");
                    me.emit(CoreEvent::Notice(tr(
                        me.ui_language(),
                        "Abgemeldet. OpenAI hat das Ende der Sitzung nicht bestätigt; du kannst die App unter chatgpt.com → Einstellungen trennen.",
                        "Signed out. OpenAI didn't confirm ending the session; you can disconnect the app in ChatGPT settings.",
                    )));
                }
            }
            me.chatgpt_save(Stored { registration: stored.registration, credentials: None });
            me.chatgpt.models.lock().clear();
            me.emit(CoreEvent::ChatGptChanged { first_sign_in: false });
        });
    }

    /// A valid access token, refreshed when it expires within five minutes.
    pub(crate) fn chatgpt_token(&self) -> Result<String, crate::ai::Unavailable> {
        use crate::ai::Unavailable;
        let _g = self.chatgpt.refresh.lock();
        let stored = self.chatgpt_load();
        let Some(c) = stored.credentials.clone() else { return Err(Unavailable::ChatGptSignedOut) };
        if !c.plan_usage() {
            return Err(Unavailable::ChatGptSignedOut);
        }
        if !c.needs_refresh() {
            return Ok(c.access_token);
        }
        let ep = self.chatgpt.endpoints.lock().clone();
        match self.runtime().block_on(chatgpt::refresh(&http(), &ep, &c)) {
            Ok(n) => {
                let t = n.access_token.clone();
                self.chatgpt_save(Stored { registration: Some(n.registration.clone()), credentials: Some(n) });
                Ok(t)
            }
            Err(e) if chatgpt::refresh_is_terminal(&e) => {
                tracing::warn!("chatgpt: refresh rejected, sign-in needed: {e}");
                self.chatgpt_save(Stored { registration: stored.registration, credentials: None });
                self.emit(CoreEvent::ChatGptChanged { first_sign_in: false });
                Err(Unavailable::ChatGptSignedOut)
            }
            Err(e) => {
                tracing::warn!("chatgpt: refresh failed: {e}");
                Err(Unavailable::Server(e.user_message(self.ui_language())))
            }
        }
    }

    /// Without refresh, for the settings' connection test inside the runtime.
    pub(crate) fn chatgpt_token_cached(&self) -> Option<String> {
        self.chatgpt_load().credentials.filter(|c| c.plan_usage()).map(|c| c.access_token)
    }
}

#[cfg(test)]
pub(crate) fn test_credentials(expires_in_min: i64, refresh: Option<&str>) -> String {
    let c = Credentials {
        registration: Registration { client_id: "oaiapp_t".into(), subject: "u".into(), email: Some("me@x.ch".into()), id_token: None },
        access_token: "at-test".into(),
        refresh_token: refresh.map(str::to_string),
        expires_at: chrono::Utc::now() + chrono::Duration::minutes(expires_in_min),
        scopes: vec!["openid".into(), chatgpt::PLAN_SCOPE.into()],
    };
    serde_json::to_string(&Stored { registration: Some(c.registration.clone()), credentials: Some(c) }).unwrap()
}
