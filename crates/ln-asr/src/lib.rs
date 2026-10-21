//! Language-correct transcription (SPEC §5 F2).
//!
//! Per channel: resample to 16 kHz → VAD sections → language choice restricted to
//! "my languages" → transcription in exactly that language → per-language post-processing.

mod langid;
mod pipeline;
mod post;
mod resample;
mod transcriber;
mod vad;
pub mod wav;

pub use langid::{LANG_CONFIDENCE_MIN, choose_language};
pub use pipeline::{AsrEngine, AudioRing, ChannelSegmenter, PendingSegment};
pub use post::{PostProcessor, is_hallucination};
pub use resample::StreamResampler;
#[cfg(feature = "whisper")]
pub use transcriber::WhisperTranscriber;
pub use transcriber::{CloudTranscriber, Transcriber};
#[cfg(feature = "whisper")]
pub use vad::SileroVad;
pub use vad::{EnergyVad, FRAME, Segmenter, SegmenterConfig, VoiceDetector};

/// Sample rate everything in this crate works with.
pub const RATE: u32 = 16_000;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("model: {0}")]
    Model(String),
    #[error("transcription: {0}")]
    Transcribe(String),
    #[error(transparent)]
    Ai(#[from] ln_ai::Error),
}
pub type Result<T, E = Error> = std::result::Result<T, E>;
