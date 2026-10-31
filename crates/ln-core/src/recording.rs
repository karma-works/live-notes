//! A recording session (SPEC §5 F1–F3): capture → VAD → ASR → journal + Markdown note.
//! Audio lives only in memory; the journal holds text only.
//!
//! Whisper is loaded on demand by a shared worker and released after finishing each recording.
//! Starting shows the recording at once and connects the devices in the background ("connecting");
//! stopping returns at once, the rest of the transcript and the summary follow in the background
//! (`Activity`), and a new recording can start meanwhile: its sections queue behind the old ones.

use crate::events::tr;
use crate::suggest::LiveSuggester;
use crate::{Core, CoreEvent, Error, Result};
use chrono::{DateTime, FixedOffset, Local};
use crossbeam_channel::{Receiver, Sender, unbounded};
use ln_asr::{AsrEngine, AudioRing, ChannelSegmenter, PendingSegment, PostProcessor, Transcriber, VoiceDetector};
use ln_audio::{Capture, CaptureConfig};
use ln_notes::{JournalEntry, JournalWriter, Note, SectionKind, atomic_write, recording_file_name, unique_path};
use ln_types::{Channel, Lang, Segment, TranscriptionMode};
use parking_lot::Mutex;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RecordingMode {
    #[default]
    Meeting,
    Thoughts,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RecordingInfo {
    pub mode: RecordingMode,
    pub note_path: PathBuf,
    pub note_id: String,
    pub elapsed: Duration,
    pub paused: bool,
    /// Devices are still being opened (Bluetooth headsets need a few seconds).
    pub connecting: bool,
    pub level_me: f32,
    pub level_others: f32,
    /// e.g. `local/whisper-large-v3-turbo` or `openai/gpt-transcribe`.
    pub transcriber: String,
    /// Audio goes to a cloud provider.
    pub cloud: bool,
    pub segments: usize,
    pub my_notes: String,
}

/// Background work on a note after its recording, shown in the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Activity {
    /// Microphone and system audio are being opened.
    Connecting,
    /// The recording stopped; the last sections are being transcribed.
    Finishing,
    Summarizing,
}

pub(crate) struct State {
    pub segments: Vec<Segment>,
    pub my_notes: String,
    pub pinned: Vec<String>,
    journal: Option<JournalWriter>,
    ring: AudioRing,
    last_write: Instant,
    pub suggester: LiveSuggester,
}

/// What the ASR worker needs to know about the recording a section belongs to.
pub(crate) struct Ctx {
    id: u64,
    mode: RecordingMode,
    pub state: Arc<Mutex<State>>,
    note_path: PathBuf,
    note_id: String,
    created: DateTime<FixedOffset>,
    cloud_seconds: Mutex<f64>,
}

enum AsrJob {
    Section(Arc<Ctx>, PendingSegment),
    Redo(Arc<Ctx>, usize, Lang),
    /// All sections of this recording are queued: write the note, then summarize.
    Finish {
        ctx: Arc<Ctx>,
        elapsed: Duration,
        summarize: bool,
        done: Option<Sender<()>>,
    },
}

/// One thread with one loaded transcriber, shared by all recordings of the same mode.
pub(crate) struct AsrWorker {
    key: String,
    tx: Sender<AsrJob>,
    name: Arc<Mutex<String>>,
    cloud: bool,
    /// Disconnects once the worker thread has ended and dropped its model.
    done: Receiver<()>,
}

pub(crate) struct Session {
    id: u64,
    pub note_path: PathBuf,
    pub note_id: String,
    started: Option<Instant>,
    capture: Option<Capture>,
    stop: Arc<AtomicBool>,
    audio_thread: Option<JoinHandle<()>>,
    pub ctx: Arc<Ctx>,
    worker: Arc<AsrWorker>,
}

static SESSION_IDS: AtomicU64 = AtomicU64::new(1);

impl Core {
    pub fn recording_active(&self) -> bool {
        self.recording.lock().is_some()
    }

    pub fn recording_info(&self) -> Option<RecordingInfo> {
        let g = self.recording.lock();
        let s = g.as_ref()?;
        let st = s.ctx.state.lock();
        let level = |c| s.capture.as_ref().map(|cap| cap.level(c)).unwrap_or(0.0);
        Some(RecordingInfo {
            mode: s.ctx.mode,
            note_path: s.note_path.clone(),
            note_id: s.note_id.clone(),
            elapsed: s.started.map(|t| t.elapsed()).unwrap_or_default(),
            paused: s.capture.as_ref().is_some_and(|c| c.is_paused()),
            connecting: s.capture.is_none(),
            level_me: level(Channel::Me),
            level_others: level(Channel::Others),
            transcriber: s.worker.name.lock().clone(),
            cloud: s.worker.cloud,
            segments: st.segments.len(),
            my_notes: st.my_notes.clone(),
        })
    }

    pub fn recording_segments(&self) -> Vec<Segment> {
        self.recording.lock().as_ref().map(|s| s.ctx.state.lock().segments.clone()).unwrap_or_default()
    }

    pub(crate) fn current_transcript_text(&self, max_chars: usize) -> Option<String> {
        let g = self.recording.lock();
        let s = g.as_ref()?;
        let ui = self.ui_language();
        let st = s.ctx.state.lock();
        let text: String = st
            .segments
            .iter()
            .map(|x| format!("[{}] {}: {}\n", ln_notes::format_timestamp(x.start_ms), x.channel.label(ui), x.text))
            .collect();
        let n = text.chars().count();
        Some(if n > max_chars { text.chars().skip(n - max_chars).collect() } else { text })
    }

    // ---- background activity -------------------------------------------------------------

    /// What is happening to a note in the background, if anything.
    pub fn activity(&self, note: &Path) -> Option<Activity> {
        self.activities.lock().iter().find(|(p, _)| p == note).map(|(_, a)| *a)
    }

    pub fn activities(&self) -> Vec<(PathBuf, Activity)> {
        self.activities.lock().clone()
    }

    pub(crate) fn set_activity(&self, note: &Path, a: Option<Activity>) {
        {
            let mut v = self.activities.lock();
            v.retain(|(p, _)| p != note);
            if let Some(a) = a {
                v.push((note.to_path_buf(), a));
            }
        }
        self.emit(CoreEvent::ActivityChanged);
    }

    // ---- ASR worker ----------------------------------------------------------------------

    fn worker_key(&self) -> String {
        let s = self.settings();
        format!(
            "{:?}|{}|{}|{:?}|{:?}",
            s.transcription.mode, s.transcription.local_model, s.transcription.openai_model, s.languages.mine, s.languages.chinese_script
        )
    }

    /// The worker for the current settings; a new one replaces it when the settings changed
    /// (the old one finishes its queue and ends).
    fn asr_worker(self: &Arc<Self>) -> Result<Arc<AsrWorker>> {
        let key = self.worker_key();
        let mut g = self.asr.lock();
        if let Some(w) = g.as_ref()
            && w.key == key
        {
            return Ok(w.clone());
        }
        let mode = self.settings().transcription.mode;
        let loader = self.transcriber_loader(mode)?;
        let cloud = mode != TranscriptionMode::Local;
        let (tx, rx) = unbounded::<AsrJob>();
        let name = Arc::new(Mutex::new(String::new()));
        let me = Arc::downgrade(self);
        let n2 = name.clone();
        let (done_tx, done) = crossbeam_channel::bounded::<()>(0);
        std::thread::Builder::new()
            .name("ln-asr".into())
            .spawn(move || {
                let _done = done_tx;
                asr_worker_loop(&me, loader, rx, &n2, cloud)
            })
            .map_err(|e| Error::User(e.to_string()))?;
        let w = Arc::new(AsrWorker { key, tx, name, cloud, done });
        *g = Some(w.clone());
        Ok(w)
    }

    /// Ends the transcription worker and waits until it has released its model. whisper.cpp's
    /// Metal backend aborts the process at exit while a model is still loaded.
    pub(crate) fn release_asr(&self, wait: std::time::Duration) -> bool {
        let Some(w) = self.asr.lock().take() else { return true };
        let done = w.done.clone();
        drop(w);
        matches!(done.recv_timeout(wait), Err(crossbeam_channel::RecvTimeoutError::Disconnected))
    }

    /// Checks right away that transcription can work (model downloaded, key present) and returns
    /// a loader that the worker runs on its own thread.
    fn transcriber_loader(self: &Arc<Self>, mode: TranscriptionMode) -> Result<Loader> {
        let ui = self.ui_language();
        let s = self.settings();
        if mode != TranscriptionMode::Local {
            if s.privacy.offline_mode {
                return Err(Error::User(tr(
                    ui,
                    "Offline-Modus ist an. Stelle die Transkription auf „Lokal“.",
                    "Offline mode is on. Switch transcription to Local.",
                )));
            }
            if self.cost_limit_reached() {
                return Err(Error::User(self.unavailable_text(&crate::ai::Unavailable::CostLimit)));
            }
            if !self.has_api_key(&s.transcription.openai_provider) {
                return Err(Error::User(tr(
                    ui,
                    "Für OpenAI-Transkription fehlt der OpenAI-Schlüssel (Einstellungen → Transkription).",
                    "OpenAI transcription needs an OpenAI key (Settings → Transcription).",
                )));
            }
            let c = self.client_for(&s.transcription.openai_provider).ok_or_else(|| Error::User("provider".into()))?;
            // v1.0: "live" uses the same per-section REST path (Realtime adapter is v1.1, SPEC §16).
            let handle = self.runtime().clone();
            return Ok(Box::new(move || {
                Ok(Box::new(ln_asr::CloudTranscriber::new(c.clone(), s.transcription.openai_model.clone(), handle.clone())))
            }));
        }
        self.local_model_path()?;
        let me = Arc::downgrade(self);
        Ok(Box::new(move || me.upgrade().ok_or_else(|| Error::User("App closed".into()))?.local_transcriber()))
    }

    fn local_model_path(&self) -> Result<PathBuf> {
        let id = self.settings().transcription.local_model;
        self.models().file(&id, 0).filter(|p| p.exists()).or_else(|| std::env::var_os("LN_WHISPER_MODEL").map(PathBuf::from)).ok_or_else(
            || {
                Error::User(tr(
                    self.ui_language(),
                    "Das Transkriptionsmodell wird noch geladen. Bitte warte, bis der Download fertig ist.",
                    "The transcription model is still downloading. Wait until it has finished.",
                ))
            },
        )
    }

    pub(crate) fn local_transcriber_pub(&self) -> Result<Box<dyn Transcriber>> {
        self.local_transcriber()
    }

    fn local_transcriber(&self) -> Result<Box<dyn Transcriber>> {
        let path = self.local_model_path()?;
        #[cfg(feature = "whisper")]
        {
            let id = self.settings().transcription.local_model;
            let name = id.trim_end_matches("-q5_0").trim_end_matches("-q8_0").to_string();
            let lid = self.models().file("whisper-base-q8_0", 0).filter(|p| p.exists());
            let t = ln_asr::WhisperTranscriber::load_with_lid(&path, &name, lid.as_deref()).map_err(|e| Error::User(e.to_string()))?;
            Ok(Box::new(t))
        }
        #[cfg(not(feature = "whisper"))]
        {
            let _ = path;
            Err(Error::User("built without whisper".into()))
        }
    }

    // ---- start / stop --------------------------------------------------------------------

    /// Starts recording into a new note and returns at once; the devices open in the background
    /// (`RecordingInfo::connecting`). `capture` overrides the devices (tests use files).
    pub fn start_recording(self: &Arc<Self>, capture: Option<CaptureConfig>) -> Result<PathBuf> {
        self.start_recording_mode(RecordingMode::Meeting, capture)
    }

    pub fn start_recording_mode(self: &Arc<Self>, mode: RecordingMode, capture: Option<CaptureConfig>) -> Result<PathBuf> {
        if self.recording_active() {
            return Err(Error::User(tr(self.ui_language(), "Es läuft bereits eine Aufnahme.", "A recording is already running.")));
        }
        if mode == RecordingMode::Thoughts && !self.role_ready(ln_types::ChatRole::Summary) {
            return Err(Error::User(tr(
                self.ui_language(),
                "Für Gedankenordnen bitte zuerst ein Zusammenfassungsmodell in den Einstellungen einrichten.",
                "Configure a summary model in Settings before organizing thoughts.",
            )));
        }
        let worker = self.asr_worker()?;
        let ui = self.ui_language();
        let created: DateTime<FixedOffset> = Local::now().fixed_offset();
        let note_id = ulid::Ulid::new().to_string();
        let rel = recording_file_name(&created, None, ui);
        let note_path = unique_path(&self.notes_dir().join(rel), None);
        let tname = {
            let n = worker.name.lock().clone();
            if n.is_empty() { self.transcriber_label() } else { n }
        };
        let mut note = Note::new_recording(note_id.clone(), created, &tname, ui);
        if mode == RecordingMode::Thoughts {
            note.front.as_mut().unwrap().extra.insert("recording_mode".into(), "thoughts".into());
        }
        atomic_write(&note_path, &note.render())?;
        let journal = JournalWriter::create(&self.paths.journal_dir(), &note_id, &note_path, &created.to_rfc3339())?;
        let ctx = Arc::new(Ctx {
            id: SESSION_IDS.fetch_add(1, Ordering::Relaxed),
            mode,
            state: Arc::new(Mutex::new(State {
                segments: vec![],
                my_notes: String::new(),
                pinned: vec![],
                journal: Some(journal),
                ring: AudioRing::default(),
                last_write: Instant::now(),
                suggester: LiveSuggester::default(),
            })),
            note_path: note_path.clone(),
            note_id: note_id.clone(),
            created,
            cloud_seconds: Mutex::new(0.0),
        });
        let id = ctx.id;
        *self.recording.lock() = Some(Session {
            id,
            note_path: note_path.clone(),
            note_id,
            started: None,
            capture: None,
            stop: Arc::new(AtomicBool::new(false)),
            audio_thread: None,
            ctx,
            worker,
        });
        self.set_activity(&note_path, Some(Activity::Connecting));
        self.emit(CoreEvent::RecordingChanged);
        self.emit(CoreEvent::NotesChanged);
        tracing::info!("recording started, connecting devices");
        let me = self.clone();
        let cfg = capture.unwrap_or_else(|| match mode {
            RecordingMode::Meeting => CaptureConfig::default(),
            RecordingMode::Thoughts => CaptureConfig { me: ln_audio::Source::Microphone, others: ln_audio::Source::Off },
        });
        std::thread::Builder::new()
            .name("ln-connect".into())
            .spawn(move || me.connect_devices(id, cfg))
            .map_err(|e| Error::User(e.to_string()))?;
        Ok(note_path)
    }

    /// `local/whisper-large-v3-turbo` before the worker has loaded its model.
    fn transcriber_label(&self) -> String {
        let s = self.settings();
        match s.transcription.mode {
            TranscriptionMode::Local => {
                format!("local/{}", s.transcription.local_model.trim_end_matches("-q5_0").trim_end_matches("-q8_0"))
            }
            _ => format!("{}/{}", s.transcription.openai_provider, s.transcription.openai_model),
        }
    }

    fn connect_devices(self: &Arc<Self>, id: u64, cfg: CaptureConfig) {
        let t0 = Instant::now();
        let result = Capture::start(&cfg);
        let mut g = self.recording.lock();
        let Some(s) = g.as_mut().filter(|s| s.id == id) else {
            // Stopped while connecting.
            if let Ok(mut c) = result {
                c.stop();
            }
            return;
        };
        match result {
            Ok(cap) => {
                tracing::info!("devices connected in {:?}", t0.elapsed());
                let rx = cap.audio.clone();
                let tx = s.worker.tx.clone();
                let stop = s.stop.clone();
                let ctx = s.ctx.clone();
                let silero = self.models().file("silero-vad-v5", 0).filter(|p| p.exists());
                let me = self.clone();
                s.audio_thread =
                    std::thread::Builder::new().name("ln-audio".into()).spawn(move || audio_loop(rx, tx, ctx, stop, silero, &me)).ok();
                s.capture = Some(cap);
                s.started = Some(Instant::now());
                let path = s.note_path.clone();
                drop(g);
                self.set_activity(&path, None);
                self.emit(CoreEvent::RecordingChanged);
            }
            Err(e) => {
                let s = g.take().expect("session");
                drop(g);
                let ui = self.ui_language();
                tracing::warn!("opening the devices failed: {e}");
                if let Some(j) = s.ctx.state.lock().journal.take() {
                    let _ = j.finish();
                }
                let _ = std::fs::remove_file(&s.note_path);
                self.set_activity(&s.note_path, None);
                self.emit(CoreEvent::Notice(match e {
                    ln_audio::Error::Mic(_) => tr(
                        ui,
                        "Kein Zugriff auf das Mikrofon. Erlaube ihn in den Systemeinstellungen → Datenschutz.",
                        "No microphone access. Allow it in System Settings → Privacy.",
                    ),
                    ln_audio::Error::System(_) => tr(
                        ui,
                        "Kein Zugriff auf System-Audio. Erlaube „Bildschirm- und Systemaudioaufnahme“ in den Systemeinstellungen.",
                        "No access to system audio. Allow audio recording in System Settings → Privacy.",
                    ),
                    ln_audio::Error::File(f) => f,
                }));
                self.emit(CoreEvent::RecordingChanged);
                self.emit(CoreEvent::NotesChanged);
            }
        }
    }

    pub fn pause_recording(&self, paused: bool) {
        if let Some(c) = self.recording.lock().as_ref().and_then(|s| s.capture.as_ref()) {
            c.set_paused(paused);
        }
        self.emit(CoreEvent::RecordingChanged);
    }

    pub fn mute_channel(&self, c: Channel, muted: bool) {
        if let Some(cap) = self.recording.lock().as_ref().and_then(|s| s.capture.as_ref()) {
            cap.set_muted(c, muted);
        }
    }

    /// The user's own notes typed during the recording ("Meine Notizen").
    pub fn set_recording_notes(&self, text: &str) {
        if let Some(s) = self.recording.lock().as_ref() {
            s.ctx.state.lock().my_notes = text.to_string();
        }
    }

    /// Re-transcribes a section in another language while its audio is still in RAM.
    pub fn correct_language(&self, index: usize, lang: Lang) -> bool {
        let g = self.recording.lock();
        let Some(s) = g.as_ref() else { return false };
        if s.ctx.state.lock().ring.get(index).is_none() {
            return false;
        }
        s.worker.tx.send(AsrJob::Redo(s.ctx.clone(), index, lang)).is_ok()
    }

    /// Stops at once. The last sections, the note and the summary follow in the background
    /// (`Activity::Finishing`, then `Summarizing`); a new recording may start meanwhile.
    pub fn stop_recording(self: &Arc<Self>) -> Option<PathBuf> {
        let summarize = self.settings().notes.auto_summary;
        self.stop_session(summarize, None)
    }

    /// Stops and waits until the note is written (tests, shutdown). No summary.
    pub(crate) fn stop_recording_blocking(self: &Arc<Self>) -> Option<PathBuf> {
        let (tx, rx) = crossbeam_channel::bounded(1);
        let p = self.stop_session(false, Some(tx))?;
        let _ = rx.recv_timeout(Duration::from_secs(600));
        Some(p)
    }

    fn stop_session(self: &Arc<Self>, summarize: bool, done: Option<Sender<()>>) -> Option<PathBuf> {
        let mut s = self.recording.lock().take()?;
        tracing::info!("stop: begin");
        s.stop.store(true, Ordering::Relaxed);
        let path = s.note_path.clone();
        self.set_activity(&path, Some(Activity::Finishing));
        self.emit(CoreEvent::RecordingChanged);
        let elapsed = s.started.map(|t| t.elapsed()).unwrap_or_default();
        let finish = move || {
            if let Some(c) = s.capture.as_mut() {
                c.stop();
            }
            if let Some(t) = s.audio_thread.take() {
                let _ = t.join();
            }
            tracing::info!("stop: devices closed, {} queued for transcription", s.ctx.note_path.display());
            let _ = s.worker.tx.send(AsrJob::Finish { ctx: s.ctx.clone(), elapsed, summarize, done });
        };
        let _ = std::thread::Builder::new().name("ln-stop".into()).spawn(finish);
        Some(path)
    }

    /// For tests: waits until file sources are played and everything is transcribed.
    pub fn wait_recording_files_done(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if self.recording.lock().as_ref().and_then(|s| s.capture.as_ref()).is_some_and(|c| c.files_done() && c.audio.is_empty()) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        false
    }

    pub fn pin_suggestion(&self, id: u64) {
        if let Some(s) = self.recording.lock().as_ref() {
            let mut st = s.ctx.state.lock();
            if let Some(line) = st.suggester.pin(id) {
                st.pinned.push(line);
            }
        }
    }

    pub fn dismiss_suggestion(&self, id: u64) {
        if let Some(s) = self.recording.lock().as_ref() {
            s.ctx.state.lock().suggester.dismiss(id);
        }
    }
}

type Loader = Box<dyn Fn() -> Result<Box<dyn Transcriber>> + Send>;

fn audio_loop(
    rx: Receiver<ln_audio::Tagged>,
    tx: Sender<AsrJob>,
    ctx: Arc<Ctx>,
    stop: Arc<AtomicBool>,
    silero: Option<PathBuf>,
    core: &Arc<Core>,
) {
    // Per channel: samples received and non-zero samples, to tell the user when a channel is mute.
    let started = Instant::now();
    let mut stats = [(0u64, 0u64, 0u32); 2];
    let mut warned = [false; 2];
    let mut last_log = Instant::now();
    let vad = |_: Channel| -> Box<dyn VoiceDetector> {
        #[cfg(feature = "whisper")]
        if let Some(p) = &silero
            && let Ok(v) = ln_asr::SileroVad::new(p)
        {
            return Box::new(v);
        }
        let _ = &silero;
        Box::new(ln_asr::EnergyVad::default())
    };
    let send = |p: PendingSegment| {
        let _ = tx.send(AsrJob::Section(ctx.clone(), p));
    };
    let mut segs = [ChannelSegmenter::new(Channel::Me, vad(Channel::Me)), ChannelSegmenter::new(Channel::Others, vad(Channel::Others))];
    loop {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(t) => {
                let i = if t.channel == Channel::Me { 0 } else { 1 };
                stats[i].0 += t.chunk.samples.len() as u64;
                stats[i].1 += t.chunk.samples.iter().filter(|x| **x != 0.0).count() as u64;
                stats[i].2 = t.chunk.sample_rate;
                for p in segs[i].push(&t.chunk.samples, t.chunk.sample_rate) {
                    send(p);
                }
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(_) => break,
        }
        if last_log.elapsed() > Duration::from_secs(10) {
            last_log = Instant::now();
            tracing::info!(
                "audio: me {} samples ({} non-zero) @{} Hz, others {} samples ({} non-zero) @{} Hz",
                stats[0].0,
                stats[0].1,
                stats[0].2,
                stats[1].0,
                stats[1].1,
                stats[1].2
            );
        }
        // The microphone must deliver sound within 6 s; pure digital silence means no permission
        // or a muted device. (System audio may legitimately be silent: nobody talking yet.)
        if !warned[0] && started.elapsed() > Duration::from_secs(6) && stats[0].1 == 0 {
            warned[0] = true;
            tracing::warn!("microphone: {} samples, all zero", stats[0].0);
            let ui = core.ui_language();
            core.emit(crate::CoreEvent::Notice(tr(
                ui,
                "Vom Mikrofon kommt kein Ton. Prüfe das Eingabegerät und die Berechtigung (Systemeinstellungen → Datenschutz & Sicherheit → Mikrofon).",
                "No sound from the microphone. Check the input device and the permission (System Settings → Privacy & Security → Microphone).",
            )));
        }
        if ctx.mode == RecordingMode::Meeting && !warned[1] && started.elapsed() > Duration::from_secs(20) && stats[1].1 == 0 {
            warned[1] = true;
            tracing::warn!("system audio: {} samples, all zero", stats[1].0);
            let ui = core.ui_language();
            core.emit(crate::CoreEvent::Notice(tr(
                ui,
                "Noch kein Ton vom System (Call, Video). Falls etwas läuft: erlaube Live Notes unter Systemeinstellungen → Datenschutz & Sicherheit → Bildschirm- & Systemaudioaufnahme.",
                "No system sound yet (call, video). If something is playing, allow Live Notes in System Settings → Privacy & Security → Screen & System Audio Recording.",
            )));
        }
        if stop.load(Ordering::Relaxed) {
            // Drain what is still queued, then close open sections.
            while let Ok(t) = rx.try_recv() {
                let i = if t.channel == Channel::Me { 0 } else { 1 };
                for p in segs[i].push(&t.chunk.samples, t.chunk.sample_rate) {
                    send(p);
                }
            }
            break;
        }
    }
    for s in segs.iter_mut() {
        if let Some(p) = s.flush() {
            send(p);
        }
    }
}

fn asr_worker_loop(weak: &Weak<Core>, loader: Loader, rx: Receiver<AsrJob>, name: &Arc<Mutex<String>>, cloud: bool) {
    let mut engine: Option<AsrEngine> = None;
    let mut load_failed = false;
    let mut current_session = 0u64;
    let mut fell_back = false;
    let mut background = false;
    // A job taken from the queue while collecting sections to pack, handled next.
    let mut stash: Option<AsrJob> = None;
    loop {
        let job = match stash.take() {
            Some(j) => j,
            None => match rx.recv() {
                Ok(j) => j,
                Err(_) => break,
            },
        };
        let Some(core) = weak.upgrade() else { break };
        let new_engine = |t: Box<dyn Transcriber>| {
            let s = core.settings();
            AsrEngine::new(t, PostProcessor::new(s.languages.chinese_script), s.languages.mine.clone())
        };
        // Load only for audio work; an empty recording must not allocate a model.
        if engine.is_none() && !load_failed && !matches!(job, AsrJob::Finish { .. }) {
            let t0 = Instant::now();
            match loader() {
                Ok(t) => {
                    *name.lock() = t.name();
                    let mut e = new_engine(t);
                    if !cloud {
                        // One short run initialises the GPU kernels now instead of on the first section.
                        let silence = PendingSegment { channel: Channel::Me, start_ms: 0, end_ms: 1200, pcm: Arc::new(vec![0.0; 19_200]) };
                        let _ = e.process(&silence, Some(&Lang::new("en")));
                        e.reset();
                    }
                    tracing::info!("asr: {} ready in {:?}", name.lock(), t0.elapsed());
                    engine = Some(e);
                }
                Err(e) => {
                    tracing::error!("asr: loading the transcriber failed: {e}");
                    load_failed = true;
                    core.emit(CoreEvent::Notice(e.to_string()));
                }
            }
        }
        let ctx = match &job {
            AsrJob::Section(c, _) | AsrJob::Redo(c, _, _) | AsrJob::Finish { ctx: c, .. } => c.clone(),
        };
        if let Some(e) = engine.as_mut()
            && ctx.id != current_session
        {
            e.reset();
            current_session = ctx.id;
        }
        match job {
            AsrJob::Section(ctx, p) => {
                let Some(e) = engine.as_mut() else { continue };
                // No recording running: the rest of a stopped recording may take its time, but must
                // not load the machine (low thread priority, fewer threads, pauses between runs).
                let bg = !cloud && core.recording.lock().is_none();
                if bg != background {
                    background = bg;
                    ln_platform::set_current_thread_background(bg);
                    e.transcriber_mut().set_background(bg);
                    tracing::info!("asr: background mode {}", if bg { "on" } else { "off" });
                }
                let t0 = Instant::now();
                let results = if cloud && !fell_back {
                    let mut r = e.process(&p, None);
                    if r.is_err() {
                        // SPEC §5 F2: fall back to local transcription when the cloud fails.
                        if let Ok(local) = core.local_transcriber() {
                            *name.lock() = local.name();
                            *e = new_engine(local);
                            fell_back = true;
                            core.emit(CoreEvent::Notice(tr(
                                core.ui_language(),
                                "Keine Verbindung zu OpenAI – es wird lokal weiter transkribiert.",
                                "Lost connection to OpenAI – transcribing locally now.",
                            )));
                            core.emit(CoreEvent::RecordingChanged);
                            r = e.process(&p, None);
                        }
                    }
                    if !fell_back {
                        *ctx.cloud_seconds.lock() += (p.end_ms - p.start_ms) as f64 / 1000.0;
                    }
                    vec![(p, r)]
                } else {
                    // Sections already waiting in the queue are packed into fewer Whisper runs.
                    let mut batch = vec![p];
                    let mut queued_ms = 0;
                    while queued_ms < PACK_QUEUE_MS {
                        match rx.try_recv() {
                            Ok(AsrJob::Section(c, q)) if c.id == ctx.id => {
                                queued_ms += q.end_ms - q.start_ms;
                                batch.push(q);
                            }
                            Ok(other) => {
                                stash = Some(other);
                                break;
                            }
                            Err(_) => break,
                        }
                    }
                    if batch.len() > 1 {
                        tracing::debug!("asr: {} queued sections in one batch", batch.len());
                    }
                    e.process_packed(batch)
                };
                for (p, r) in results {
                    deliver(&core, &ctx, p, r, name);
                }
                if background {
                    // Leave the GPU and CPU idle for a third of the time.
                    std::thread::sleep((t0.elapsed() / 2).min(Duration::from_secs(5)));
                }
            }
            AsrJob::Redo(ctx, index, lang) => {
                let pending = ctx.state.lock().ring.get(index).cloned();
                if let (Some(p), Some(e)) = (pending, engine.as_mut())
                    && let Ok(Some(seg)) = e.process(&p, Some(&lang))
                {
                    let mut st = ctx.state.lock();
                    if index < st.segments.len() {
                        st.segments[index] = seg.clone();
                        if let Some(j) = st.journal.as_mut() {
                            let _ = j.append(&JournalEntry::Replace { index, segment: seg.clone() });
                        }
                    }
                    drop(st);
                    core.emit(CoreEvent::SegmentReplaced { index, segment: seg });
                }
            }
            AsrJob::Finish { ctx, elapsed, summarize, done } => {
                let transcriber = name.lock().clone();
                let transcriber = if transcriber.is_empty() { core.transcriber_label() } else { transcriber };
                finish_note(&core, &ctx, elapsed, &transcriber);
                // Drop Whisper (including GPU buffers and language detector) before any LLM work.
                engine = None;
                load_failed = false;
                fell_back = false;
                background = false;
                ln_platform::set_current_thread_background(false);
                // A blocking stop (tests, shutdown) waits on `done`; only a user's stop sorts
                // the note into categories.
                let sort = done.is_none();
                if let Some(d) = done {
                    let _ = d.send(());
                }
                if summarize || (ctx.mode == RecordingMode::Thoughts && sort) {
                    core.set_activity(&ctx.note_path, Some(Activity::Summarizing));
                    let me = core.clone();
                    let path = ctx.note_path.clone();
                    std::thread::spawn(move || {
                        // Sorting after the summary: it reads the summary too, and the file
                        // may have been renamed after its title.
                        let renamed = crate::summary::summarize(&me, &path);
                        me.set_activity(&path, None);
                        if sort {
                            me.classify_note(renamed.as_deref().unwrap_or(&path));
                        }
                    });
                } else {
                    core.set_activity(&ctx.note_path, None);
                    if sort {
                        core.classify_note(&ctx.note_path);
                    }
                }
            }
        }
    }
}

/// Audio taken from the queue at once for packing (`AsrEngine::process_packed`).
const PACK_QUEUE_MS: u64 = 90_000;

/// Stores a transcribed section: transcript, journal, audio for language corrections, note file.
fn deliver(core: &Arc<Core>, ctx: &Arc<Ctx>, p: PendingSegment, r: ln_asr::Result<Option<Segment>>, name: &Arc<Mutex<String>>) {
    match r {
        Ok(Some(seg)) => {
            // Lock order everywhere: `recording` before `state` (the UI does the same).
            let live = core.recording.lock().as_ref().is_some_and(|s| s.id == ctx.id);
            let mut st = ctx.state.lock();
            let index = st.segments.len();
            st.segments.push(seg.clone());
            if let Some(j) = st.journal.as_mut() {
                let _ = j.append(&JournalEntry::Segment(seg.clone()));
            }
            if live && ctx.mode == RecordingMode::Meeting {
                st.ring.push(index, p);
            }
            // After the stop nobody reads along: rewriting the note (and the UI reloading it)
            // once a minute is enough; the journal keeps every section safe meanwhile.
            let every = if live { Duration::from_secs(10) } else { Duration::from_secs(60) };
            if st.last_write.elapsed() > every {
                st.last_write = Instant::now();
                let _ = write_note(core, &ctx.note_path, &st, ctx.created, None, &name.lock());
            }
            // Live suggestions only while this recording runs.
            let trigger = if live && ctx.mode == RecordingMode::Meeting {
                st.suggester.observe(&seg, &core.settings().live_suggestions)
            } else {
                None
            };
            drop(st);
            if live {
                core.emit(CoreEvent::Segment { index, segment: seg });
            }
            if let Some(t) = trigger {
                crate::suggest::spawn(core, t, ctx.note_path.clone(), ctx.note_id.clone(), ctx.state.clone());
            }
        }
        Ok(None) => {}
        Err(e) => tracing::warn!("transcription failed: {e}"),
    }
}

/// Writes the final note after the last section, closes the journal, records cloud costs.
fn finish_note(core: &Arc<Core>, ctx: &Ctx, elapsed: Duration, transcriber: &str) {
    let mut st = ctx.state.lock();
    if st.segments.is_empty() && elapsed > Duration::from_secs(5) {
        core.emit(CoreEvent::Notice(tr(
            core.ui_language(),
            "In dieser Aufnahme wurde keine Sprache erkannt. Prüfe Mikrofon und Systemaudio unter Einstellungen → Aufnahme testen.",
            "No speech was recognised in this recording. Check microphone and system audio in Settings → Test recording.",
        )));
    }
    tracing::info!("recording stopped after {:?}: {} segments", elapsed, st.segments.len());
    let _ = write_note(core, &ctx.note_path, &st, ctx.created, Some(elapsed), transcriber);
    if let Some(j) = st.journal.take() {
        let _ = j.finish();
    }
    st.ring.clear();
    drop(st);
    let secs = *ctx.cloud_seconds.lock();
    if secs > 0.0 {
        let s = core.settings();
        core.costs.record_audio(&s.transcription.openai_provider, &s.transcription.openai_model, Some(&ctx.note_id), secs);
    }
    core.emit(CoreEvent::NoteFileChanged(ctx.note_path.clone()));
    core.emit(CoreEvent::NotesChanged);
    core.indexer.kick();
}

/// Writes the note: keeps whatever is in the file, replaces notes/transcript/pinned sections.
fn write_note(
    core: &Core,
    path: &Path,
    st: &State,
    created: DateTime<FixedOffset>,
    elapsed: Option<Duration>,
    transcriber: &str,
) -> Result<()> {
    let ui = core.ui_language();
    let mut note = std::fs::read_to_string(path)
        .ok()
        .and_then(|t| Note::parse(&t).ok())
        .unwrap_or_else(|| Note::new_recording(ulid::Ulid::new().to_string(), created, transcriber, ui));
    if let Some(f) = note.front.as_mut() {
        f.transcription = Some(transcriber.to_string());
        if let Some(e) = elapsed {
            f.duration = Some(ln_notes::format_timestamp(e.as_millis() as u64));
        }
    }
    let notes_body = if st.my_notes.trim().is_empty() { "\n".to_string() } else { format!("{}\n", st.my_notes.trim_end()) };
    note.set_section(SectionKind::Notes, &notes_body, ui);
    if !st.pinned.is_empty() {
        let body: String = st.pinned.iter().map(|l| format!("- {l}\n")).collect();
        note.set_section(SectionKind::Suggestions, &body, ui);
    }
    note.set_transcript(&st.segments, ui);
    atomic_write(path, &note.render())?;
    Ok(())
}

/// Writes notes of recordings that were interrupted by a crash (SPEC §5 F1).
pub fn recover(core: &Arc<Core>) {
    let Ok(sessions) = ln_notes::recover_sessions(&core.paths.journal_dir()) else { return };
    let ui = core.ui_language();
    for s in sessions {
        let created = DateTime::parse_from_rfc3339(&s.created).unwrap_or_else(|_| Local::now().fixed_offset());
        let mut note = std::fs::read_to_string(&s.note_path)
            .ok()
            .and_then(|t| Note::parse(&t).ok())
            .unwrap_or_else(|| Note::new_recording(s.note_id.clone(), created, "recovered", ui));
        note.set_transcript(&s.segments, ui);
        if atomic_write(&s.note_path, &note.render()).is_ok() {
            let _ = std::fs::remove_file(&s.journal);
            core.emit(CoreEvent::Notice(tr(
                ui,
                "Eine unterbrochene Aufnahme wurde wiederhergestellt.",
                "An interrupted recording was restored.",
            )));
        }
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    struct TrackedTranscriber(Arc<AtomicUsize>);
    impl Drop for TrackedTranscriber {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }
    impl Transcriber for TrackedTranscriber {
        fn name(&self) -> String {
            "test".into()
        }
        fn detect(&mut self, _: &[f32]) -> ln_asr::Result<Vec<(Lang, f32)>> {
            Ok(vec![("de".into(), 1.0)])
        }
        fn transcribe(&mut self, _: &[f32], _: Option<&Lang>, _: &[Lang], _: &str) -> ln_asr::Result<(String, Option<Lang>)> {
            Ok(("Gedanke".into(), Some("de".into())))
        }
    }

    #[test]
    fn worker_releases_model_after_finish_and_reloads_for_next_recording() {
        let dir = tempfile::tempdir().unwrap();
        let (core, _events) = Core::start(ln_types::AppPaths::in_dir(dir.path()), Arc::new(ln_platform::MemoryStore::default())).unwrap();
        let live = Arc::new(AtomicUsize::new(0));
        let loads = Arc::new(AtomicUsize::new(0));
        let l = live.clone();
        let n = loads.clone();
        let loader: Loader = Box::new(move || {
            l.fetch_add(1, Ordering::SeqCst);
            n.fetch_add(1, Ordering::SeqCst);
            Ok(Box::new(TrackedTranscriber(l.clone())))
        });
        let (tx, rx) = unbounded();
        let weak = Arc::downgrade(&core);
        let worker = std::thread::spawn(move || asr_worker_loop(&weak, loader, rx, &Arc::new(Mutex::new(String::new())), true));
        for id in 1..=2 {
            let ctx = Arc::new(Ctx {
                id,
                mode: RecordingMode::Meeting,
                state: Arc::new(Mutex::new(State {
                    segments: vec![],
                    my_notes: String::new(),
                    pinned: vec![],
                    journal: None,
                    ring: AudioRing::default(),
                    last_write: Instant::now(),
                    suggester: LiveSuggester::default(),
                })),
                note_path: core.notes_dir().join(format!("test-{id}.md")),
                note_id: id.to_string(),
                created: Local::now().fixed_offset(),
                cloud_seconds: Mutex::new(0.0),
            });
            tx.send(AsrJob::Section(
                ctx.clone(),
                PendingSegment { channel: Channel::Me, start_ms: 0, end_ms: 1200, pcm: Arc::new(vec![0.1; 19_200]) },
            ))
            .unwrap();
            let (done, wait) = crossbeam_channel::bounded(1);
            tx.send(AsrJob::Finish { ctx, elapsed: Duration::from_secs(2), summarize: false, done: Some(done) }).unwrap();
            wait.recv_timeout(Duration::from_secs(10)).unwrap();
            assert_eq!(live.load(Ordering::SeqCst), 0, "model must be dropped before completion");
            assert_eq!(loads.load(Ordering::SeqCst), id as usize, "next recording reloads the model");
        }
        drop(tx);
        worker.join().unwrap();
        core.shutdown();
    }
}
