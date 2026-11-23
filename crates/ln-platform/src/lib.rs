//! Operating-system abstraction (SPEC §7.3). Only this crate and `ln-app` touch platform APIs.
//! macOS and Linux provide audio capture and document extraction.

mod secrets;

#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "linux")]
mod linux;

pub use secrets::{KeychainStore, MemoryStore, SecretStore};

/// Mono float samples from one capture device, in the device's native rate.
#[derive(Debug, Clone)]
pub struct AudioChunk {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("not supported on this platform yet")]
    Unsupported,
    #[error("permission denied: {0}")]
    Permission(&'static str),
    #[error("{what} failed (OSStatus {status})")]
    Os { what: &'static str, status: i32 },
    #[error("{0}")]
    Other(String),
}
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// A running capture. Dropping it stops the capture.
pub trait CaptureStream: Send {
    fn stop(&mut self);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Permission {
    Granted,
    Denied,
    NotDetermined,
}

/// Lowers (or restores) the calling thread's priority. In background mode macOS runs it on the
/// efficiency cores and throttles its disk access; threads it starts afterwards inherit this.
pub fn set_current_thread_background(on: bool) {
    #[cfg(target_os = "macos")]
    {
        // <sys/qos.h>
        const QOS_CLASS_USER_INITIATED: u32 = 0x19;
        const QOS_CLASS_BACKGROUND: u32 = 0x09;
        unsafe extern "C" {
            fn pthread_set_qos_class_self_np(qos: u32, relative_priority: i32) -> i32;
        }
        let qos = if on { QOS_CLASS_BACKGROUND } else { QOS_CLASS_USER_INITIATED };
        // SAFETY: only changes the scheduling class of the calling thread.
        let rc = unsafe { pthread_set_qos_class_self_np(qos, 0) };
        if rc != 0 {
            tracing::warn!("setting the thread priority failed: {rc}");
        }
    }
    #[cfg(not(target_os = "macos"))]
    let _ = on;
}

/// Starts the microphone with voice processing (echo cancellation) and sends mono chunks.
pub fn start_microphone(tx: crossbeam_channel::Sender<AudioChunk>) -> Result<Box<dyn CaptureStream>> {
    #[cfg(target_os = "macos")]
    {
        macos::mic::start(tx)
    }
    #[cfg(target_os = "linux")]
    {
        linux::start_capture(tx, false)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = tx;
        Err(Error::Unsupported)
    }
}

/// Starts capturing system audio of all processes except this one.
pub fn start_system_audio(tx: crossbeam_channel::Sender<AudioChunk>) -> Result<Box<dyn CaptureStream>> {
    #[cfg(target_os = "macos")]
    {
        macos::system_audio::start(tx)
    }
    #[cfg(target_os = "linux")]
    {
        linux::start_capture(tx, true)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = tx;
        Err(Error::Unsupported)
    }
}

pub fn microphone_permission() -> Permission {
    #[cfg(target_os = "macos")]
    {
        macos::permissions::microphone()
    }
    #[cfg(not(target_os = "macos"))]
    {
        Permission::Granted
    }
}

/// Asks for microphone access; the callback runs on an arbitrary thread.
pub fn request_microphone_permission(done: impl Fn(bool) + Send + Sync + 'static) {
    #[cfg(target_os = "macos")]
    {
        macos::permissions::request_microphone(done)
    }
    #[cfg(not(target_os = "macos"))]
    {
        done(true)
    }
}

/// Recognises text in an image. `languages` are BCP-47 tags (e.g. `zh-Hans`, `de-DE`).
pub fn ocr_image(path: &std::path::Path, languages: &[&str]) -> Result<String> {
    #[cfg(target_os = "macos")]
    {
        macos::ocr::recognize(path, languages)
    }
    #[cfg(target_os = "linux")]
    {
        linux::ocr_image(path, languages)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (path, languages);
        Err(Error::Unsupported)
    }
}

/// Text of each PDF page.
pub fn pdf_pages(path: &std::path::Path) -> Result<Vec<String>> {
    #[cfg(target_os = "macos")]
    {
        macos::pdf::pages(path)
    }
    #[cfg(target_os = "linux")]
    {
        linux::pdf_pages(path)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = path;
        Err(Error::Unsupported)
    }
}

/// OCR language tags for the user's languages (ISO 639-1).
pub fn ocr_languages(langs: &[ln_types::Lang]) -> Vec<&'static str> {
    let mut v: Vec<&'static str> = langs
        .iter()
        .filter_map(|l| match l.as_str() {
            "zh" => Some("zh-Hans"),
            "de" => Some("de-DE"),
            "en" => Some("en-US"),
            "fr" => Some("fr-FR"),
            "es" => Some("es-ES"),
            "it" => Some("it-IT"),
            "pt" => Some("pt-BR"),
            "ja" => Some("ja-JP"),
            "ko" => Some("ko-KR"),
            _ => None,
        })
        .collect();
    if v.contains(&"zh-Hans") {
        v.push("zh-Hant");
    }
    v
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    #[test]
    fn pdf_of_missing_file_errors() {
        assert!(super::pdf_pages(std::path::Path::new("/nonexistent.pdf")).is_err());
    }
}
