use crate::{Citation, SuggestionCard};
use futures::channel::mpsc;
use ln_types::Segment;
use std::path::PathBuf;

/// Everything the UI needs to redraw. Sent from background threads.
#[derive(Debug, Clone)]
pub enum CoreEvent {
    SettingsChanged,
    ModelsChanged,
    DownloadProgress {
        model: &'static str,
        fraction: f32,
    },
    SourcesChanged,
    IndexProgress {
        files: i64,
        chunks: i64,
        embedded: i64,
    },
    NotesChanged,
    /// A note file changed on disk (external edit or the app's own write).
    NoteFileChanged(PathBuf),
    RecordingChanged,
    Segment {
        index: usize,
        segment: Segment,
    },
    SegmentReplaced {
        index: usize,
        segment: Segment,
    },
    /// Summary, tasks and title written; `path` is the (possibly renamed) file.
    SummaryReady {
        path: PathBuf,
    },
    Suggestion(SuggestionCard),
    SuggestionAnswer {
        id: u64,
        text: String,
        done: bool,
    },
    Answer {
        id: u64,
        text: String,
        done: bool,
        citations: Vec<Citation>,
    },
    ProviderTested {
        provider: String,
        result: Result<(), String>,
    },
    /// ChatGPT sign-in state changed. `first_sign_in`: the plan is used for the first time on
    /// this account, show the one-time confirmation.
    ChatGptChanged {
        first_sign_in: bool,
    },
    /// A note's background work changed (see `Core::activity`).
    ActivityChanged,
    /// Short user-facing message (already localised).
    Notice(String),
}

#[derive(Clone)]
pub struct EventSender(mpsc::UnboundedSender<CoreEvent>);
pub type EventReceiver = mpsc::UnboundedReceiver<CoreEvent>;

impl EventSender {
    pub fn send(&self, e: CoreEvent) {
        let _ = self.0.unbounded_send(e);
    }
}

pub fn channel() -> (EventSender, EventReceiver) {
    let (tx, rx) = mpsc::unbounded();
    (EventSender(tx), rx)
}

/// Picks the German or English text.
pub(crate) fn tr(ui: ln_types::UiLanguage, de: &str, en: &str) -> String {
    match ui {
        ln_types::UiLanguage::De => de.to_string(),
        ln_types::UiLanguage::En => en.to_string(),
    }
}

impl std::fmt::Debug for EventSender {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("EventSender")
    }
}
