use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentPart {
    Text { text: String },
    ImageUrl { image_url: ImageUrl },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImageUrl {
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Content {
    Text(String),
    Parts(Vec<ContentPart>),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Content,
}

impl Message {
    pub fn system(t: impl Into<String>) -> Self {
        Message { role: Role::System, content: Content::Text(t.into()) }
    }
    pub fn user(t: impl Into<String>) -> Self {
        Message { role: Role::User, content: Content::Text(t.into()) }
    }
    pub fn assistant(t: impl Into<String>) -> Self {
        Message { role: Role::Assistant, content: Content::Text(t.into()) }
    }
    /// A user message with an image, as a data URL.
    pub fn user_with_image(t: impl Into<String>, mime: &str, bytes: &[u8]) -> Self {
        use base64::Engine;
        let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
        Message {
            role: Role::User,
            content: Content::Parts(vec![
                ContentPart::Text { text: t.into() },
                ContentPart::ImageUrl { image_url: ImageUrl { url: format!("data:{mime};base64,{b64}") } },
            ]),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<Message>,
    pub temperature: Option<f32>,
    pub max_tokens: Option<u32>,
    /// Ask for a JSON object response where the provider supports it.
    pub json: bool,
}

impl ChatRequest {
    pub fn new(model: impl Into<String>, messages: Vec<Message>) -> Self {
        ChatRequest { model: model.into(), messages, temperature: Some(0.4), max_tokens: None, json: false }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub prompt_tokens: u64,
    #[serde(default)]
    pub completion_tokens: u64,
    /// OpenRouter reports the exact cost in USD.
    #[serde(default)]
    pub cost: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChatResponse {
    pub text: String,
    pub usage: Usage,
    pub model: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Transcription {
    pub text: String,
    /// Languages the provider detected (ISO 639-1), if it reports them.
    pub languages: Vec<String>,
}

/// A model as listed by `/models`, with what the role picker needs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub input_modalities: Vec<String>,
    pub context_length: Option<u64>,
    /// USD per 1M tokens.
    pub price_in: Option<f64>,
    pub price_out: Option<f64>,
}

impl ModelInfo {
    pub fn accepts_images(&self) -> bool {
        self.input_modalities.iter().any(|m| m == "image")
    }
    pub fn accepts_audio(&self) -> bool {
        self.input_modalities.iter().any(|m| m == "audio")
    }
}
