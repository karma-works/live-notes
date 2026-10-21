//! A recording session (SPEC §5 F1–F3): capture → VAD → ASR → journal + Markdown note.
//! Audio lives only in memory; the journal holds text only.

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
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq)]
pub struct RecordingInfo {
    pub note_path: PathBuf,
    pub note_id: String,
    pub elapsed: Duration,
    pub paused: bool,
    pub level_me: f32,
    pub level_others: f32,
    /// e.g. `local/whisper-large-v3-turbo` or `openai/gpt-transcribe`.
    pub transcriber: String,
    /// Audio goes to a cloud provider.
    pub cloud: bool,
    pub segments: usize,
    pub my_notes: String,
}

enum Job {
    Section(PendingSegment),
    Redo { index: usize, lang: Lang },
    Done,
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

pub(crate) struct Session {
    pub note_path: PathBuf,
    pub note_id: String,
    created: DateTime<FixedOffset>,
    started: Instant,
    capture: Capture,
    stop: Arc<AtomicBool>,
    pub state: Arc<Mutex<State>>,
    audio_thread: Option<JoinHandle<()>>,
    asr_thread: Option<JoinHandle<()>>,
    asr_tx: Sender<Job>,
    transcriber: Arc<Mutex<String>>,
    cloud: bool,
}

impl Core {
    pub fn recording_active(&self) -> bool {
        self.recording.lock().is_some()
    }

    pub fn recording_info(&self) -> Option<RecordingInfo> {
        let g = self.recording.lock();
        let s = g.as_ref()?;
        let st = s.state.lock();
        Some(RecordingInfo {
            note_path: s.note_path.clone(),
            note_id: s.note_id.clone(),
            elapsed: s.started.elapsed(),
            paused: s.capture.is_paused(),
            level_me: s.capture.level(Channel::Me),
            level_others: s.capture.level(Channel::Others),
            transcriber: s.transcriber.lock().clone(),
            cloud: s.cloud,
            segments: st.segments.len(),
            my_notes: st.my_notes.clone(),
        })
    }

    pub fn recording_segments(&self) -> Vec<Segment> {
        self.recording.lock().as_ref().map(|s| s.state.lock().segments.clone()).unwrap_or_default()
    }

    pub(crate) fn current_transcript_text(&self, max_chars: usize) -> Option<String> {
        let g = self.recording.lock();
        let s = g.as_ref()?;
        let ui = self.ui_language();
        let st = s.state.lock();
        let text: String = st.segments.iter().map(|x| format!("[{}] {}: {}\n", ln_notes::format_timestamp(x.start_ms), x.channel.label(ui), x.text)).collect();
        let n = text.chars().count();
        Some(if n > max_chars { text.chars().skip(n - max_chars).collect() } else { text })
    }

    /// Starts recording into a new note. `capture` overrides the devices (tests use files).
    pub fn start_recording(self: &Arc<Self>, capture: Option<CaptureConfig>) -> Result<PathBuf> {
        if self.recording_active() {
            return Err(Error::User(tr(self.ui_language(), "Es läuft bereits eine Aufnahme.", "A recording is already running.")));
        }
        let settings = self.settings();
        let ui = self.ui_language();
        let transcriber = self.make_transcriber(settings.transcription.mode)?;
        let name = transcriber.name();
        let cloud = !name.starts_with("local/");
        let created: DateTime<FixedOffset> = Local::now().fixed_offset();
        let note_id = ulid::Ulid::new().to_string();
        let rel = recording_file_name(&created, None, ui);
        let note_path = unique_path(&self.notes_dir().join(rel), None);
        let note = Note::new_recording(note_id.clone(), created, &name, ui);
        atomic_write(&note_path, &note.render())?;
        let journal = JournalWriter::create(&self.paths.journal_dir(), &note_id, &note_path, &created.to_rfc3339())?;

        let cfg = capture.unwrap_or_default();
        let cap = Capture::start(&cfg).map_err(|e| {
            let _ = std::fs::remove_file(&note_path);
            Error::User(match e {
                ln_audio::Error::Mic(_) => tr(ui, "Kein Zugriff auf das Mikrofon. Erlaube ihn in den Systemeinstellungen → Datenschutz.", "No microphone access. Allow it in System Settings → Privacy."),
                ln_audio::Error::System(_) => tr(ui, "Kein Zugriff auf System-Audio. Erlaube „Bildschirm- und Systemaudioaufnahme“ in den Systemeinstellungen.", "No access to system audio. Allow audio recording in System Settings → Privacy."),
                ln_audio::Error::File(f) => f,
            })
        })?;

        let state = Arc::new(Mutex::new(State {
            segments: vec![],
            my_notes: String::new(),
            pinned: vec![],
            journal: Some(journal),
            ring: AudioRing::default(),
            last_write: Instant::now(),
            suggester: LiveSuggester::default(),
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let (asr_tx, asr_rx) = unbounded::<Job>();
        let transcriber_name = Arc::new(Mutex::new(name));

        let audio_thread = {
            let rx = cap.audio.clone();
            let tx = asr_tx.clone();
            let stop = stop.clone();
            let silero = self.models().file("silero-vad-v5", 0).filter(|p| p.exists());
            std::thread::Builder::new()
                .name("ln-audio".into())
                .spawn(move || audio_loop(rx, tx, stop, silero))
                .expect("audio thread")
        };
        let asr_thread = {
            let me = self.clone();
            let state = state.clone();
            let path = note_path.clone();
            let id = note_id.clone();
            let tn = transcriber_name.clone();
            let mut engine = AsrEngine::new(transcriber, PostProcessor::new(settings.languages.chinese_script), settings.languages.mine.clone());
            std::thread::Builder::new()
                .name("ln-asr".into())
                .spawn(move || {
                    asr_loop(&me, &mut engine, asr_rx, &state, &path, &id, created, &tn);
                })
                .expect("asr thread")
        };

        *self.recording.lock() = Some(Session {
            note_path: note_path.clone(),
            note_id,
            created,
            started: Instant::now(),
            capture: cap,
            stop,
            state,
            audio_thread: Some(audio_thread),
            asr_thread: Some(asr_thread),
            asr_tx,
            transcriber: transcriber_name,
            cloud,
        });
        self.emit(CoreEvent::RecordingChanged);
        self.emit(CoreEvent::NotesChanged);
        Ok(note_path)
    }

    fn make_transcriber(self: &Arc<Self>, mode: TranscriptionMode) -> Result<Box<dyn Transcriber>> {
        let ui = self.ui_language();
        let s = self.settings();
        if mode != TranscriptionMode::Local {
            if s.privacy.offline_mode {
                return Err(Error::User(tr(ui, "Offline-Modus ist an. Stelle die Transkription auf „Lokal“.", "Offline mode is on. Switch transcription to Local.")));
            }
            if !self.has_api_key(&s.transcription.openai_provider) {
                return Err(Error::User(tr(ui, "Für OpenAI-Transkription fehlt der OpenAI-Schlüssel (Einstellungen → Transkription).", "OpenAI transcription needs an OpenAI key (Settings → Transcription).")));
            }
            let c = self.client_for(&s.transcription.openai_provider).ok_or_else(|| Error::User("provider".into()))?;
            // v1.0: "live" uses the same per-section REST path (Realtime adapter is v1.1, SPEC §16).
            return Ok(Box::new(ln_asr::CloudTranscriber::new(c, s.transcription.openai_model.clone(), self.runtime().clone())));
        }
        self.local_transcriber()
    }

    fn local_transcriber(&self) -> Result<Box<dyn Transcriber>> {
        let ui = self.ui_language();
        let id = self.settings().transcription.local_model;
        let path = self
            .models()
            .file(&id, 0)
            .filter(|p| p.exists())
            .or_else(|| std::env::var_os("LN_WHISPER_MODEL").map(PathBuf::from))
            .ok_or_else(|| Error::User(tr(ui, "Das Transkriptionsmodell wird noch geladen. Bitte warte, bis der Download fertig ist.", "The transcription model is still downloading. Wait until it has finished.")))?;
        #[cfg(feature = "whisper")]
        {
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

    pub fn pause_recording(&self, paused: bool) {
        if let Some(s) = self.recording.lock().as_ref() {
            s.capture.set_paused(paused);
        }
        self.emit(CoreEvent::RecordingChanged);
    }

    pub fn mute_channel(&self, c: Channel, muted: bool) {
        if let Some(s) = self.recording.lock().as_ref() {
            s.capture.set_muted(c, muted);
        }
    }

    /// The user's own notes typed during the recording ("Meine Notizen").
    pub fn set_recording_notes(&self, text: &str) {
        if let Some(s) = self.recording.lock().as_ref() {
            s.state.lock().my_notes = text.to_string();
        }
    }

    /// Re-transcribes a section in another language while its audio is still in RAM.
    pub fn correct_language(&self, index: usize, lang: Lang) -> bool {
        let g = self.recording.lock();
        let Some(s) = g.as_ref() else { return false };
        if s.state.lock().ring.get(index).is_none() {
            return false;
        }
        s.asr_tx.send(Job::Redo { index, lang }).is_ok()
    }

    /// Stops and saves; the summary runs afterwards in the background.
    pub fn stop_recording(self: &Arc<Self>) -> Option<PathBuf> {
        let path = self.stop_recording_blocking();
        if let Some(p) = &path
            && self.settings().notes.auto_summary
        {
            let me = self.clone();
            let p = p.clone();
            std::thread::spawn(move || crate::summary::summarize(&me, &p));
        }
        path
    }

    pub(crate) fn stop_recording_blocking(&self) -> Option<PathBuf> {
        let mut s = self.recording.lock().take()?;
        s.stop.store(true, Ordering::Relaxed);
        s.capture.stop();
        if let Some(t) = s.audio_thread.take() {
            let _ = t.join();
        }
        let _ = s.asr_tx.send(Job::Done);
        if let Some(t) = s.asr_thread.take() {
            let _ = t.join();
        }
        let elapsed = s.started.elapsed();
        let mut st = s.state.lock();
        let _ = write_note(self, &s.note_path, &st, s.created, Some(elapsed), &s.transcriber.lock());
        if let Some(j) = st.journal.take() {
            let _ = j.finish();
        }
        st.ring.clear();
        drop(st);
        self.emit(CoreEvent::RecordingChanged);
        self.emit(CoreEvent::NoteFileChanged(s.note_path.clone()));
        self.emit(CoreEvent::NotesChanged);
        self.indexer.kick();
        Some(s.note_path.clone())
    }

    /// For tests: waits until file sources are played and everything is transcribed.
    pub fn wait_recording_files_done(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if self.recording.lock().as_ref().is_some_and(|s| s.capture.files_done() && s.capture.audio.is_empty()) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        false
    }

    pub fn pin_suggestion(&self, id: u64) {
        if let Some(s) = self.recording.lock().as_ref() {
            let mut st = s.state.lock();
            if let Some(line) = st.suggester.pin(id) {
                st.pinned.push(line);
            }
        }
    }

    pub fn dismiss_suggestion(&self, id: u64) {
        if let Some(s) = self.recording.lock().as_ref() {
            s.state.lock().suggester.dismiss(id);
        }
    }
}

fn audio_loop(rx: Receiver<ln_audio::Tagged>, tx: Sender<Job>, stop: Arc<AtomicBool>, silero: Option<PathBuf>) {
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
    let mut segs = [ChannelSegmenter::new(Channel::Me, vad(Channel::Me)), ChannelSegmenter::new(Channel::Others, vad(Channel::Others))];
    loop {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(t) => {
                let s = &mut segs[if t.channel == Channel::Me { 0 } else { 1 }];
                for p in s.push(&t.chunk.samples, t.chunk.sample_rate) {
                    let _ = tx.send(Job::Section(p));
                }
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(_) => break,
        }
        if stop.load(Ordering::Relaxed) {
            // Drain what is still queued, then close open sections.
            while let Ok(t) = rx.try_recv() {
                let s = &mut segs[if t.channel == Channel::Me { 0 } else { 1 }];
                for p in s.push(&t.chunk.samples, t.chunk.sample_rate) {
                    let _ = tx.send(Job::Section(p));
                }
            }
            break;
        }
    }
    for s in segs.iter_mut() {
        if let Some(p) = s.flush() {
            let _ = tx.send(Job::Section(p));
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn asr_loop(core: &Arc<Core>, engine: &mut AsrEngine, rx: Receiver<Job>, state: &Arc<Mutex<State>>, path: &Path, note_id: &str, created: DateTime<FixedOffset>, tname: &Arc<Mutex<String>>) {
    let mut cloud_seconds = 0.0f64;
    let cloud = !engine.transcriber_name().starts_with("local/");
    let mut fell_back = false;
    while let Ok(job) = rx.recv() {
        match job {
            Job::Done => break,
            Job::Section(p) => {
                let mut r = engine.process(&p, None);
                if r.is_err() && cloud && !fell_back {
                    // SPEC §5 F2: fall back to local transcription when the cloud fails.
                    if let Ok(local) = core.local_transcriber() {
                        let s = core.settings();
                        *engine = AsrEngine::new(local, PostProcessor::new(s.languages.chinese_script), s.languages.mine.clone());
                        *tname.lock() = engine.transcriber_name();
                        fell_back = true;
                        core.emit(CoreEvent::Notice(tr(core.ui_language(), "Keine Verbindung zu OpenAI – es wird lokal weiter transkribiert.", "Lost connection to OpenAI – transcribing locally now.")));
                        core.emit(CoreEvent::RecordingChanged);
                        r = engine.process(&p, None);
                    }
                }
                if cloud && !fell_back {
                    cloud_seconds += (p.end_ms - p.start_ms) as f64 / 1000.0;
                }
                match r {
                    Ok(Some(seg)) => {
                        let mut st = state.lock();
                        let index = st.segments.len();
                        st.segments.push(seg.clone());
                        if let Some(j) = st.journal.as_mut() {
                            let _ = j.append(&JournalEntry::Segment(seg.clone()));
                        }
                        st.ring.push(index, p);
                        let write_due = st.last_write.elapsed() > Duration::from_secs(10);
                        if write_due {
                            st.last_write = Instant::now();
                            let _ = write_note(core, path, &st, created, None, &tname.lock());
                        }
                        let trigger = st.suggester.observe(&seg, &core.settings().live_suggestions);
                        drop(st);
                        core.emit(CoreEvent::Segment { index, segment: seg });
                        if let Some(t) = trigger {
                            crate::suggest::spawn(core, t, path.to_path_buf(), note_id.to_string(), state.clone());
                        }
                    }
                    Ok(None) => {}
                    Err(e) => tracing::warn!("transcription failed: {e}"),
                }
            }
            Job::Redo { index, lang } => {
                let pending = state.lock().ring.get(index).cloned();
                if let Some(p) = pending
                    && let Ok(Some(seg)) = engine.process(&p, Some(&lang))
                {
                    let mut st = state.lock();
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
        }
    }
    if cloud_seconds > 0.0 {
        let s = core.settings();
        core.costs.record_audio(&s.transcription.openai_provider, &s.transcription.openai_model, Some(note_id), cloud_seconds);
    }
}

/// Writes the note: keeps whatever is in the file, replaces notes/transcript/pinned sections.
fn write_note(core: &Core, path: &Path, st: &State, created: DateTime<FixedOffset>, elapsed: Option<Duration>, transcriber: &str) -> Result<()> {
    let ui = core.ui_language();
    let mut note = std::fs::read_to_string(path).ok().and_then(|t| Note::parse(&t).ok()).unwrap_or_else(|| Note::new_recording(ulid::Ulid::new().to_string(), created, transcriber, ui));
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
        let mut note = std::fs::read_to_string(&s.note_path).ok().and_then(|t| Note::parse(&t).ok()).unwrap_or_else(|| Note::new_recording(s.note_id.clone(), created, "recovered", ui));
        note.set_transcript(&s.segments, ui);
        if atomic_write(&s.note_path, &note.render()).is_ok() {
            let _ = std::fs::remove_file(&s.journal);
            core.emit(CoreEvent::Notice(tr(ui, "Eine unterbrochene Aufnahme wurde wiederhergestellt.", "An interrupted recording was restored.")));
        }
    }
}
