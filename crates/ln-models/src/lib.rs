//! Local models (SPEC §9): a fixed catalog with URL, size and SHA-256, resumable downloads,
//! and the bundled `llama-server` that serves embeddings and local Gemma over the
//! OpenAI-compatible API.

mod catalog;
mod download;
mod server;

pub use catalog::{CATALOG, ModelFile, ModelKind, ModelSpec, by_id, recommended_llm};
pub use download::{DownloadProgress, Downloader, is_installed, model_path};
pub use server::{LlamaServer, ServerRole, find_llama_server};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("network: {0}")]
    Http(#[from] reqwest::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("download of {file} failed: HTTP {status}")]
    Status { file: String, status: u16 },
    #[error("checksum mismatch for {0}")]
    Checksum(String),
    #[error("unknown model {0}")]
    Unknown(String),
    #[error("llama-server not found")]
    NoServer,
    #[error("llama-server did not start: {0}")]
    ServerStart(String),
    #[error("offline mode is on")]
    Offline,
}
pub type Result<T, E = Error> = std::result::Result<T, E>;
