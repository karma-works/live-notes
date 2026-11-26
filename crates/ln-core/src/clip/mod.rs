//! Saving web pages and YouTube videos as notes (`docs/clip-api.md`).
//!
//! [`Core::clip`] holds the whole logic and knows nothing about HTTP, so other front ends (an MCP
//! server, a share menu) can reuse it. The local HTTP endpoint for the browser extension lives in
//! [`server`].

mod captions;
pub(crate) mod render;
mod server;
pub(crate) mod youtube;

pub use captions::{CaptionCue, merge_cues, parse_json3, parse_vtt};
pub use server::ClipServerStatus;

use crate::events::tr;
use crate::{Core, CoreEvent};
use chrono::Local;
use ln_ai::prompts::{self, ClipSource, WebSummary};
use ln_notes::{atomic_write, recording_file_name, sanitize_title, unique_path};
use ln_types::{ChatRole, Lang};
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{Arc, Weak};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ClipKind {
    Page,
    Youtube,
}

/// `POST /v1/clip` body.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClipRequest {
    pub kind: ClipKind,
    pub url: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub site_name: Option<String>,
    #[serde(default)]
    pub byline: Option<String>,
    /// BCP-47 language of the page, e.g. `de` or `en-US`.
    #[serde(default)]
    pub lang: Option<String>,
    /// Page text (Readability `textContent`), paragraphs separated by blank lines.
    #[serde(default)]
    pub text: Option<String>,
    /// Readability's HTML; used only when `text` is missing.
    #[serde(default)]
    pub html: Option<String>,
    /// Selected text: when set, only this is saved and summarized.
    #[serde(default)]
    pub selection: Option<String>,
    #[serde(default)]
    pub youtube: Option<YoutubeClip>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct YoutubeClip {
    #[serde(default)]
    pub video_id: String,
    #[serde(default)]
    pub channel: Option<String>,
    #[serde(default)]
    pub duration_s: Option<f64>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub transcript_lang: Option<String>,
    /// `innertube`, `panel` or `timedtext`; only logged.
    #[serde(default)]
    pub transcript_source: Option<String>,
    /// `null`/missing: the app fetches captions itself (yt-dlp), or transcribes the audio.
    #[serde(default)]
    pub transcript: Option<Vec<CaptionCue>>,
}

const MAX_URL: usize = 4096;
const MAX_TITLE_CHARS: usize = 300;
const MAX_CUES: usize = 100_000;

impl ClipRequest {
    /// Checks the request and fills what can be derived (video id from the URL, trimmed fields).
    pub fn validate(&mut self) -> Result<(), String> {
        let url = self.url.trim().to_string();
        if url.len() > MAX_URL || !(url.starts_with("https://") || url.starts_with("http://")) || url.contains(char::is_whitespace) {
            return Err("url must be an http(s) URL".into());
        }
        self.url = url;
        let clean = |o: &mut Option<String>| {
            *o = o.as_deref().map(|s| s.split_whitespace().collect::<Vec<_>>().join(" ")).filter(|s| !s.is_empty());
        };
        clean(&mut self.title);
        clean(&mut self.site_name);
        clean(&mut self.byline);
        if let Some(t) = self.title.as_mut()
            && t.chars().count() > MAX_TITLE_CHARS
        {
            *t = t.chars().take(MAX_TITLE_CHARS).collect();
        }
        self.lang = self.lang.as_deref().and_then(normalize_lang);
        let nonempty = |o: &Option<String>| o.as_deref().is_some_and(|s| !s.trim().is_empty());
        match self.kind {
            ClipKind::Page => {
                if !nonempty(&self.text) && !nonempty(&self.selection) && !nonempty(&self.html) {
                    return Err("page clips need text, html or selection".into());
                }
            }
            ClipKind::Youtube => {
                let yt = self.youtube.get_or_insert_with(Default::default);
                if yt.video_id.trim().is_empty() {
                    yt.video_id = youtube::video_id_from_url(&self.url).unwrap_or_default();
                }
                yt.video_id = yt.video_id.trim().to_string();
                if !youtube::valid_video_id(&yt.video_id) {
                    return Err("youtube.video_id is missing or invalid".into());
                }
                clean(&mut yt.channel);
                yt.transcript_lang = yt.transcript_lang.as_deref().and_then(normalize_lang);
                if yt.duration_s.is_some_and(|d| !d.is_finite() || d < 0.0) {
                    yt.duration_s = None;
                }
                if let Some(t) = yt.transcript.as_mut() {
                    if t.len() > MAX_CUES {
                        return Err("youtube.transcript has too many entries".into());
                    }
                    t.retain(|c| !c.text.trim().is_empty());
                    t.sort_by_key(|c| c.start_ms);
                }
            }
        }
        Ok(())
    }
}

/// `en-US` → `en`; anything that isn't a language code → `None`.
fn normalize_lang(s: &str) -> Option<String> {
    let l = Lang::new(s);
    let ok = (2..=3).contains(&l.as_str().len()) && l.as_str().bytes().all(|b| b.is_ascii_lowercase());
    ok.then(|| l.as_str().to_string())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ClipStatus {
    Pending,
    Done,
    Error,
}

/// What a pending job is doing (`stage` in `GET /v1/clip/{id}`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClipStage {
    Queued,
    FetchingTranscript,
    DownloadingAudio,
    /// Local transcription waits until a running recording has stopped.
    WaitingForRecording,
    Transcribing,
    Summarizing,
    Writing,
}

/// `GET /v1/clip/{job_id}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClipJob {
    pub status: ClipStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage: Option<ClipStage>,
    /// 0…1 while transcribing audio.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// How many finished jobs are remembered for status queries.
const KEEP_JOBS: usize = 200;
/// Secret store entry of the bearer token.
const TOKEN_KEY: &str = "clip-token";

#[derive(Default)]
pub(crate) struct ClipState {
    jobs: Mutex<Vec<(String, ClipJob)>>,
    /// The token and whether it is kept in the secret store (else only until the app quits).
    token: RwLock<Option<(String, bool)>>,
    server: Mutex<Option<server::Running>>,
    status: Mutex<ClipServerStatus>,
    /// Set once the app asked for the endpoint; settings changes then start or stop it.
    core: Mutex<Option<Weak<Core>>>,
}

/// 32 random bytes, hex.
fn new_token() -> String {
    let mut b = [0u8; 32];
    if let Err(e) = getrandom::fill(&mut b) {
        // Never seen on macOS or Linux; ULIDs carry 80 random bits each from the thread RNG.
        tracing::warn!("clip: OS random numbers unavailable ({e}), using ULIDs");
        let u = [ulid::Ulid::new().0.to_le_bytes(), ulid::Ulid::new().0.to_le_bytes()].concat();
        b.copy_from_slice(&u);
    }
    b.iter().map(|x| format!("{x:02x}")).collect()
}

impl Core {
    // ---- token ---------------------------------------------------------------------------

    /// The bearer token of the clip endpoint; created and stored on first use.
    pub fn clip_token(&self) -> String {
        if let Some((t, _)) = self.clip.token.read().as_ref() {
            return t.clone();
        }
        let mut g = self.clip.token.write();
        if let Some((t, _)) = g.as_ref() {
            return t.clone();
        }
        let stored = self.secrets.get(TOKEN_KEY).filter(|t| t.len() >= 32);
        let entry = match stored {
            Some(t) => (t, true),
            None => {
                let t = new_token();
                let ok = match self.secrets.set(TOKEN_KEY, &t) {
                    Ok(()) => true,
                    Err(e) => {
                        tracing::warn!("clip: token not stored, valid until the app quits: {e}");
                        false
                    }
                };
                (t, ok)
            }
        };
        let t = entry.0.clone();
        *g = Some(entry);
        t
    }

    /// False when the secret store is unavailable: the token then changes on every start.
    pub fn clip_token_stored(&self) -> bool {
        self.clip_token();
        self.clip.token.read().as_ref().is_some_and(|(_, s)| *s)
    }

    /// Replaces the token; the extension needs the new one.
    pub fn regenerate_clip_token(&self) -> String {
        let t = new_token();
        let ok = match self.secrets.set(TOKEN_KEY, &t) {
            Ok(()) => true,
            Err(e) => {
                tracing::warn!("clip: new token not stored: {e}");
                false
            }
        };
        *self.clip.token.write() = Some((t.clone(), ok));
        self.emit(CoreEvent::SettingsChanged);
        t
    }

    // ---- endpoint ------------------------------------------------------------------------

    /// Runs the local endpoint as the settings say (`clip.enabled`, `clip.port`) and keeps it in
    /// line with later settings changes. The app calls this once at start; tests use port 0.
    pub fn start_clip_server(self: &Arc<Self>) {
        *self.clip.core.lock() = Some(Arc::downgrade(self));
        self.sync_clip_server();
    }

    pub fn clip_server_status(&self) -> ClipServerStatus {
        self.clip.status.lock().clone()
    }

    /// Starts, restarts or stops the endpoint after a settings change.
    pub(crate) fn sync_clip_server(&self) {
        let Some(core) = self.clip.core.lock().as_ref().and_then(Weak::upgrade) else { return };
        let s = self.settings().clip;
        let mut server = self.clip.server.lock();
        let running_port = server.as_ref().map(|r| r.configured_port);
        if s.enabled && running_port == Some(s.port) {
            return;
        }
        // Dropping the handle stops the old listener.
        *server = None;
        let status = if s.enabled {
            // The token is resolved before the first request arrives.
            self.clip_token();
            match server::start(&core, s.port) {
                Ok(r) => {
                    let port = r.port;
                    *server = Some(r);
                    tracing::info!("clip: listening on 127.0.0.1:{port}");
                    ClipServerStatus::Running { port }
                }
                Err(e) => {
                    tracing::warn!("clip: cannot listen on 127.0.0.1:{}: {e}", s.port);
                    ClipServerStatus::Failed(e.to_string())
                }
            }
        } else {
            ClipServerStatus::Off
        };
        drop(server);
        *self.clip.status.lock() = status;
        self.emit(CoreEvent::SettingsChanged);
    }

    pub(crate) fn stop_clip_server(&self) {
        *self.clip.server.lock() = None;
    }

    // ---- jobs ----------------------------------------------------------------------------

    /// Checks the request and saves it as a note in the background. Returns the job id for
    /// [`Core::clip_job`]; an invalid request is rejected at once.
    pub fn clip(self: &Arc<Self>, mut req: ClipRequest) -> Result<String, String> {
        req.validate()?;
        let id = ulid::Ulid::new().to_string().to_lowercase();
        {
            let mut jobs = self.clip.jobs.lock();
            jobs.push((
                id.clone(),
                ClipJob {
                    status: ClipStatus::Pending,
                    stage: Some(ClipStage::Queued),
                    progress: None,
                    path: None,
                    title: None,
                    error: None,
                },
            ));
            let finished = jobs.iter().filter(|(_, j)| j.status != ClipStatus::Pending).count();
            if finished > KEEP_JOBS
                && let Some(i) = jobs.iter().position(|(_, j)| j.status != ClipStatus::Pending)
            {
                jobs.remove(i);
            }
        }
        let me = self.clone();
        let job = id.clone();
        std::thread::Builder::new()
            .name("ln-clip".into())
            .spawn(move || {
                let result = me.clip_now(req, &|stage, progress| {
                    me.update_job(&job, |j| {
                        j.stage = Some(stage);
                        j.progress = progress;
                    })
                });
                me.update_job(&job, |j| {
                    j.stage = None;
                    j.progress = None;
                    match result {
                        Ok((path, title)) => {
                            j.status = ClipStatus::Done;
                            j.path = Some(path);
                            j.title = Some(title);
                        }
                        Err(e) => {
                            j.status = ClipStatus::Error;
                            j.error = Some(e);
                        }
                    }
                });
            })
            .map_err(|e| e.to_string())?;
        Ok(id)
    }

    pub fn clip_job(&self, id: &str) -> Option<ClipJob> {
        self.clip.jobs.lock().iter().find(|(i, _)| i == id).map(|(_, j)| j.clone())
    }

    fn update_job(&self, id: &str, f: impl FnOnce(&mut ClipJob)) {
        if let Some((_, j)) = self.clip.jobs.lock().iter_mut().find(|(i, _)| i == id) {
            f(j);
        }
    }

    /// Saves a clip as a note and blocks until it is written. Returns path and title. `stage`
    /// reports progress. Must not run on the async runtime (model calls block on it).
    pub fn clip_now(self: &Arc<Self>, mut req: ClipRequest, stage: &dyn Fn(ClipStage, Option<f32>)) -> Result<(PathBuf, String), String> {
        req.validate()?;
        let ui = self.ui_language();
        let result = self.clip_inner(req, stage);
        match &result {
            Ok((path, title)) => {
                tracing::info!("clip: saved {}", path.display());
                self.emit(CoreEvent::NotesChanged);
                self.emit(CoreEvent::NoteFileChanged(path.clone()));
                self.emit(CoreEvent::Notice(format!("{} {title}", tr(ui, "Gespeichert:", "Saved:"))));
                self.indexer.kick();
                self.classify_note(path);
            }
            Err(e) => {
                tracing::warn!("clip failed: {e}");
                self.emit(CoreEvent::Notice(format!("{} {e}", tr(ui, "Speichern der Seite fehlgeschlagen:", "Couldn't save the page:"))));
            }
        }
        result
    }

    fn clip_inner(self: &Arc<Self>, req: ClipRequest, stage: &dyn Fn(ClipStage, Option<f32>)) -> Result<(PathBuf, String), String> {
        let ui = self.ui_language();
        let created = Local::now().fixed_offset();
        let summary_lang = self.settings().languages.summary_language.map(|l| l.to_string());
        let (note, title) = match req.kind {
            ClipKind::Page => {
                let (content, selection) = match req.selection.as_deref().filter(|s| !s.trim().is_empty()) {
                    Some(s) => (s.to_string(), true),
                    None => match req.text.as_deref().filter(|s| !s.trim().is_empty()) {
                        Some(t) => (t.to_string(), false),
                        None => (render::html_to_text(req.html.as_deref().unwrap_or("")), false),
                    },
                };
                let content = render::clean_page_text(&content);
                if content.trim().is_empty() {
                    return Err(tr(ui, "Die Seite enthält keinen Text.", "The page has no text."));
                }
                let title_hint = req.title.clone().unwrap_or_else(|| render::host(&req.url));
                stage(ClipStage::Summarizing, None);
                let out_lang = summary_lang.or_else(|| req.lang.clone());
                let summary = self.clip_summary(ClipSource::Page, &title_hint, &content, out_lang.as_deref());
                let title = pick_title(req.title.as_deref(), &summary, &req.url);
                let lang = req.lang.clone().or_else(|| summary.as_ref().ok().and_then(|s| s.language.clone()));
                let n = render::ClipNote {
                    kind: ClipKind::Page,
                    url: req.url.clone(),
                    title: title.clone(),
                    site_name: req.site_name.clone(),
                    byline: req.byline.clone(),
                    channel: None,
                    duration_ms: None,
                    lang: lang.as_deref().and_then(normalize_lang),
                    created,
                    summary,
                    body: render::Body::Page { text: content, selection },
                };
                (render::render(&n, ui), title)
            }
            ClipKind::Youtube => {
                let yt = req.youtube.clone().unwrap_or_default();
                let mut channel = yt.channel.clone();
                let mut duration_s = yt.duration_s;
                let mut title = req.title.clone();
                let mut description = yt.description.clone();
                let mut lang = yt.transcript_lang.clone().or_else(|| req.lang.clone());
                let mut transcriber = None;
                let cues = match yt.transcript.clone().filter(|t| !t.is_empty()) {
                    Some(t) => {
                        tracing::info!("clip: transcript from the extension ({:?}, {} cues)", yt.transcript_source, t.len());
                        t
                    }
                    None => {
                        let fetched = self.fetch_youtube(&yt.video_id, lang.as_deref(), stage)?;
                        title = title.or(fetched.info.title.clone());
                        channel = channel.or(fetched.info.channel.clone());
                        duration_s = duration_s.or(fetched.info.duration_s);
                        description = description.or(fetched.info.description.clone());
                        lang = fetched.lang.or(lang);
                        transcriber = fetched.transcriber;
                        fetched.cues
                    }
                };
                let lines = merge_cues(&cues);
                if duration_s.is_none() {
                    duration_s = lines.last().map(|l| (l.start_ms + l.dur_ms) as f64 / 1000.0);
                }
                let mut content = String::new();
                if let Some(d) = description.as_deref().filter(|d| !d.trim().is_empty()) {
                    let d: String = d.chars().take(2000).collect();
                    content.push_str(&format!("Video description:\n{d}\n\nTranscript:\n"));
                }
                for l in &lines {
                    content.push_str(&format!("[{}] {}\n", ln_notes::format_timestamp(l.start_ms), l.text));
                }
                let title_hint = title.clone().unwrap_or_else(|| "YouTube".into());
                let summary = if lines.is_empty() {
                    Err(tr(ui, "Das Video hat kein Transkript.", "The video has no transcript."))
                } else {
                    stage(ClipStage::Summarizing, None);
                    let out_lang = summary_lang.or_else(|| lang.clone());
                    self.clip_summary(ClipSource::Video, &title_hint, &content, out_lang.as_deref())
                };
                let title = pick_title(title.as_deref(), &summary, &req.url);
                let lang = lang.or_else(|| summary.as_ref().ok().and_then(|s| s.language.clone()));
                let n = render::ClipNote {
                    kind: ClipKind::Youtube,
                    url: req.url.clone(),
                    title: title.clone(),
                    site_name: None,
                    byline: None,
                    channel,
                    duration_ms: duration_s.map(|d| (d * 1000.0) as u64),
                    lang: lang.as_deref().and_then(normalize_lang),
                    created,
                    summary,
                    body: render::Body::Video { video_id: yt.video_id.clone(), lines, transcriber },
                };
                (render::render(&n, ui), title)
            }
        };
        stage(ClipStage::Writing, None);
        // Same place as recordings (`YYYY/MM/YYYY-MM-DD HHMM Title.md`), so the note list shows
        // clips under today; labels and `source` in the front matter say where they came from.
        let name = sanitize_title(&title);
        let name = if name.is_empty() { "Clip".to_string() } else { name };
        let path = unique_path(&self.notes_dir().join(recording_file_name(&created, Some(&name), ui)), None);
        atomic_write(&path, &note).map_err(|e| e.to_string())?;
        Ok((path, title))
    }

    /// Summary with the summary model. Long content is read in parts first. `Err` carries the
    /// reason shown in the note instead of a summary.
    pub(crate) fn clip_summary(
        &self,
        source: ClipSource,
        title: &str,
        content: &str,
        out_lang: Option<&str>,
    ) -> Result<WebSummary, String> {
        let ui = self.ui_language();
        let role = self.role(ChatRole::Summary).map_err(|u| self.unavailable_text(&u))?;
        let parts = render::split_text(content, PART_CHARS);
        let (input, partial) = if content.chars().count() <= SINGLE_CHARS {
            (content.to_string(), false)
        } else {
            let total = parts.len().min(MAX_PARTS);
            let mut notes = String::new();
            for (i, part) in parts.iter().take(MAX_PARTS).enumerate() {
                let r =
                    self.chat_once(&role, prompts::clip_part_notes(source, title, part, i + 1, total, out_lang), false, "clip", None)?;
                notes.push_str(&format!("Part {}:\n{}\n\n", i + 1, r.trim()));
            }
            if parts.len() > MAX_PARTS {
                tracing::info!("clip: content cut after {MAX_PARTS} of {} parts", parts.len());
            }
            (notes, true)
        };
        let text = self.chat_once(&role, prompts::web_summary(source, title, &input, out_lang, partial), true, "clip", None)?;
        prompts::parse_web_summary(&text)
            .ok_or_else(|| tr(ui, "Die Zusammenfassung konnte nicht gelesen werden.", "Couldn't read the summary."))
    }

    /// Captions via yt-dlp, else the audio track transcribed locally.
    fn fetch_youtube(
        self: &Arc<Self>,
        video_id: &str,
        lang: Option<&str>,
        stage: &dyn Fn(ClipStage, Option<f32>),
    ) -> Result<Fetched, String> {
        let ui = self.ui_language();
        stage(ClipStage::FetchingTranscript, None);
        let ytdlp = youtube::find_tool("yt-dlp").ok_or_else(|| {
            tr(
                ui,
                "yt-dlp ist nicht installiert. Installiere es (z. B. „brew install yt-dlp“ oder „pacman -S yt-dlp“), damit Live Notes Untertitel laden kann.",
                "yt-dlp is not installed. Install it (e.g. `brew install yt-dlp` or `pacman -S yt-dlp`) so Live Notes can fetch captions.",
            )
        })?;
        let dir = tempfile::Builder::new().prefix("ln-clip-").tempdir().map_err(|e| e.to_string())?;
        let info = youtube::fetch_info(&ytdlp, video_id, dir.path())?;
        // The video's own language comes first (in `choose_subtitles`); the user's languages are
        // no hint for it, and author captions in them are often translations.
        let mut prefs: Vec<String> = lang.map(str::to_string).into_iter().collect();
        prefs.extend(["en".to_string(), "de".to_string()]);
        // Debug switch to exercise the audio path on videos that have captions.
        let skip_subs = std::env::var_os("LN_CLIP_SKIP_SUBS").is_some();
        if !skip_subs && let Some((key, auto)) = youtube::choose_subtitles(&info, &prefs) {
            match youtube::download_captions(&ytdlp, &info, &key, auto, dir.path()) {
                Ok(cues) => {
                    let lang = normalize_lang(&key);
                    return Ok(Fetched {
                        info,
                        cues,
                        lang,
                        transcriber: Some(format!("youtube/{}", if auto { "auto-captions" } else { "captions" })),
                    });
                }
                Err(e) => tracing::warn!("clip: captions {key}: {e}"),
            }
        }
        let ffmpeg = youtube::find_tool("ffmpeg").ok_or_else(|| {
            tr(
                ui,
                "Das Video hat keine Untertitel, und ffmpeg ist nicht installiert. Installiere ffmpeg, damit Live Notes die Tonspur transkribieren kann.",
                "The video has no captions and ffmpeg is not installed. Install ffmpeg so Live Notes can transcribe the audio.",
            )
        })?;
        stage(ClipStage::DownloadingAudio, None);
        let wav = youtube::download_audio(&ytdlp, &ffmpeg, &info, dir.path())?;
        let lang = info.language.as_deref().and_then(normalize_lang).or_else(|| lang.and_then(normalize_lang));
        let (cues, transcriber, detected) = self.transcribe_wav(&wav, lang.as_deref(), stage)?;
        Ok(Fetched { info, cues, lang: lang.or(detected), transcriber: Some(transcriber) })
    }

    /// Transcribes a WAV file on the shared transcription worker. Waits while a recording runs, so
    /// the live transcript is never delayed. Returns cues, the transcriber name and the language.
    fn transcribe_wav(
        self: &Arc<Self>,
        wav: &std::path::Path,
        lang: Option<&str>,
        stage: &dyn Fn(ClipStage, Option<f32>),
    ) -> Result<(Vec<CaptionCue>, String, Option<String>), String> {
        while self.recording_active() {
            stage(ClipStage::WaitingForRecording, None);
            std::thread::sleep(Duration::from_secs(2));
        }
        stage(ClipStage::Transcribing, Some(0.0));
        let mut reader = hound::WavReader::open(wav).map_err(|e| format!("wav: {e}"))?;
        let spec = reader.spec();
        let channels = spec.channels.max(1) as usize;
        let total = reader.duration().max(1) as f32;
        let max = (1i64 << (spec.bits_per_sample.max(1) - 1)) as f32;
        let silero = self.models().file("silero-vad-v5", 0).filter(|p| p.exists());
        let vad: Box<dyn ln_asr::VoiceDetector> = {
            #[cfg(feature = "whisper")]
            {
                match silero.as_ref().and_then(|p| ln_asr::SileroVad::new(p).ok()) {
                    Some(v) => Box::new(v),
                    None => Box::new(ln_asr::EnergyVad::default()),
                }
            }
            #[cfg(not(feature = "whisper"))]
            {
                let _ = &silero;
                Box::new(ln_asr::EnergyVad::default())
            }
        };
        let mut seg = ln_asr::ChannelSegmenter::new(ln_types::Channel::Others, vad);
        let fixed = lang.map(Lang::new);
        let mut pending: Vec<ln_asr::PendingSegment> = Vec::new();
        let mut segments: Vec<ln_types::Segment> = Vec::new();
        let (mut name, mut cloud, mut sent) = (String::new(), false, false);
        let mut read = 0usize;
        let mut frame: Vec<f32> = Vec::with_capacity(channels);
        let mut chunk: Vec<f32> = Vec::with_capacity(spec.sample_rate as usize / 10);
        let mut send = |batch: Vec<ln_asr::PendingSegment>, last: bool, segments: &mut Vec<ln_types::Segment>| -> Result<(), String> {
            let (s, n, c) = self.transcribe_sections(batch, fixed.clone(), last).map_err(|e| e.to_string())?;
            segments.extend(s);
            name = n;
            cloud = c;
            Ok(())
        };
        if spec.sample_format == hound::SampleFormat::Float {
            // Float WAVs are rare from ffmpeg; read them in one go.
            let raw = ln_asr::wav::read_mono(wav).map_err(|e| format!("wav: {e}"))?.0;
            for c in raw.chunks(spec.sample_rate as usize / 10) {
                pending.extend(seg.push(c, spec.sample_rate));
            }
        } else {
            for s in reader.samples::<i32>() {
                frame.push(s.map_err(|e| format!("wav: {e}"))? as f32 / max);
                if frame.len() < channels {
                    continue;
                }
                chunk.push(frame.iter().sum::<f32>() / channels as f32);
                frame.clear();
                read += 1;
                if chunk.len() < spec.sample_rate as usize / 10 {
                    continue;
                }
                pending.extend(seg.push(&chunk, spec.sample_rate));
                chunk.clear();
                let queued: u64 = pending.iter().map(|p| p.end_ms - p.start_ms).sum();
                if queued >= BATCH_MS {
                    send(std::mem::take(&mut pending), false, &mut segments)?;
                    sent = true;
                    stage(ClipStage::Transcribing, Some(read as f32 / total));
                }
            }
            if !chunk.is_empty() {
                pending.extend(seg.push(&chunk, spec.sample_rate));
            }
        }
        pending.extend(seg.flush());
        if !pending.is_empty() || sent {
            send(pending, true, &mut segments)?;
        }
        if cloud {
            let secs: f64 = segments.iter().map(|s| s.duration_ms() as f64 / 1000.0).sum();
            let s = self.settings();
            self.costs.record_audio(&s.transcription.openai_provider, &s.transcription.openai_model, None, secs);
        }
        let ui = self.ui_language();
        if segments.is_empty() {
            return Err(tr(ui, "In der Tonspur wurde keine Sprache erkannt.", "No speech was recognised in the audio."));
        }
        let mut by_lang: std::collections::BTreeMap<String, u64> = Default::default();
        for s in &segments {
            *by_lang.entry(s.lang.to_string()).or_default() += s.duration_ms();
        }
        let detected = by_lang.into_iter().max_by_key(|x| x.1).map(|x| x.0);
        let cues = segments.into_iter().map(|s| CaptionCue { start_ms: s.start_ms, dur_ms: s.duration_ms(), text: s.text }).collect();
        Ok((cues, name, detected))
    }
}

/// Audio sent to the transcription worker per batch, so progress can be reported.
const BATCH_MS: u64 = 5 * 60 * 1000;
/// Content up to this size goes to the model in one request (≈ 12k tokens).
const SINGLE_CHARS: usize = 48_000;
/// Longer content is read in parts of this size, at most [`MAX_PARTS`] of them.
const PART_CHARS: usize = 24_000;
const MAX_PARTS: usize = 8;

struct Fetched {
    info: youtube::VideoInfo,
    cues: Vec<CaptionCue>,
    lang: Option<String>,
    transcriber: Option<String>,
}

/// The page's title; the model's title only when the page has none.
fn pick_title(given: Option<&str>, summary: &Result<WebSummary, String>, url: &str) -> String {
    given
        .map(str::to_string)
        .or_else(|| summary.as_ref().ok().map(|s| s.title.trim().to_string()).filter(|t| !t.is_empty()))
        .unwrap_or_else(|| render::host(url))
}

#[cfg(test)]
mod tests;
