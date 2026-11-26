//! Downloading a web page the user asked to save (MCP `clip_url` without page text). Lives here
//! because only ln-ai and ln-models may talk to the network (deny.toml, SPEC §11).

use std::time::Duration;

/// A downloaded page: body as text and the lowercased `Content-Type` (may be empty).
#[derive(Debug, Clone)]
pub struct Page {
    pub content_type: String,
    pub body: String,
}

/// GETs `url` and returns HTML or plain text, at most `limit` bytes. Errors are readable.
pub async fn fetch_page(url: &str, limit: usize, timeout: Duration) -> Result<Page, String> {
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(10))
        .timeout(timeout)
        .user_agent(concat!("Mozilla/5.0 (compatible; LiveNotes/", env!("CARGO_PKG_VERSION"), ")"))
        .build()
        .map_err(|e| e.to_string())?;
    let mut resp = client.get(url).send().await.map_err(|e| format!("couldn't download {url}: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("{url} answered {}", resp.status()));
    }
    let content_type = resp.headers().get(reqwest::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or("").to_ascii_lowercase();
    if !(content_type.is_empty() || content_type.contains("html") || content_type.starts_with("text/")) {
        return Err(format!("{url} is not a web page ({content_type}); pass the text yourself"));
    }
    let mut body = Vec::new();
    while let Some(chunk) = resp.chunk().await.map_err(|e| e.to_string())? {
        body.extend_from_slice(&chunk);
        if body.len() > limit {
            return Err(format!("{url} is larger than {} MB", limit / (1024 * 1024)));
        }
    }
    Ok(Page { content_type, body: String::from_utf8_lossy(&body).into_owned() })
}
