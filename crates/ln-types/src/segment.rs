use crate::Lang;
use serde::{Deserialize, Serialize};

/// The two recording channels (SPEC §5 F1). The IDs are stable; labels are localised.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Channel {
    /// Microphone of the user.
    Me,
    /// System audio: everybody else in the call.
    Others,
}

impl Channel {
    pub fn id(self) -> &'static str {
        match self {
            Channel::Me => "me",
            Channel::Others => "others",
        }
    }

    pub fn label(self, ui: crate::UiLanguage) -> &'static str {
        match (self, ui) {
            (Channel::Me, crate::UiLanguage::De) => "Ich",
            (Channel::Others, crate::UiLanguage::De) => "Andere",
            (Channel::Me, crate::UiLanguage::En) => "Me",
            (Channel::Others, crate::UiLanguage::En) => "Others",
        }
    }

    /// Recognises a channel from any localised label or id.
    pub fn from_label(s: &str) -> Option<Channel> {
        match s.trim().to_lowercase().as_str() {
            "me" | "ich" => Some(Channel::Me),
            "others" | "andere" => Some(Channel::Others),
            _ => None,
        }
    }

    pub fn all() -> [Channel; 2] {
        [Channel::Me, Channel::Others]
    }
}

/// One transcribed speech section.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Segment {
    pub channel: Channel,
    /// Start relative to the beginning of the recording.
    pub start_ms: u64,
    pub end_ms: u64,
    pub lang: Lang,
    pub text: String,
    /// Confidence of the language decision, 0..1.
    pub lang_confidence: f32,
}

impl Segment {
    pub fn duration_ms(&self) -> u64 {
        self.end_ms.saturating_sub(self.start_ms)
    }
}
