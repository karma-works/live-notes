//! The note format (SPEC §8.1). A note is a Markdown file; it is the source of truth.
//!
//! Recording notes contain invisible section markers (`<!-- ln:transcript -->` …) so the
//! app finds its sections regardless of the heading language. Manual notes are plain
//! Markdown and need no markers.

mod file;
mod format;
mod images;
mod journal;
mod transcript;

pub use file::{NoteFile, SaveOutcome, atomic_write, file_fingerprint, recording_file_name, sanitize_title, unique_path};
pub use format::{FrontMatter, Note, Section, SectionKind, headings, toggle_task};
pub use images::{
    ASSETS_DIR, IMAGE_EXTENSIONS, append_to_body, image_markdown, image_targets, is_image_path, local_target, parse_image_line, store_asset,
};
pub use journal::{JournalEntry, JournalWriter, RecoveredSession, recover_sessions};
pub use transcript::{TranscriptLine, format_timestamp, parse_timestamp};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("front matter: {0}")]
    FrontMatter(String),
    #[error("journal: {0}")]
    Journal(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
