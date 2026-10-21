//! Indexing of source folders (SPEC §5 F4): scan, extract text, chunk, store in `ln-index`.
//! Embeddings are added afterwards by `ln-core`, so full-text search works before the
//! embedding model is downloaded.

mod chunk;
mod extract;
mod scan;
mod sync;
mod watch;

pub use chunk::{chunk_blocks, estimate_tokens};
pub use extract::{Block, ExtractOptions, Extracted, FileKind, PARSER_VERSION, detect_lang, extract, kind_of};
pub use scan::{ScanOptions, ScannedFile, scan};
pub use sync::{SyncReport, index_file, sync_source};
pub use watch::{FolderWatcher, WatchEvent};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("index: {0}")]
    Index(#[from] ln_index::Error),
    #[error("unsupported file type")]
    Unsupported,
    #[error("{0}")]
    Parse(String),
}
pub type Result<T, E = Error> = std::result::Result<T, E>;
