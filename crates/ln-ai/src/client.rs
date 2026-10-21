use crate::{ChatRequest, ChatResponse, Error, ModelInfo, Result, Transcription, Usage};
use futures::StreamExt;
use ln_types::{Provider, ProviderKind};
use serde_json::{Value, json};
use std::time::Duration;

/// A client for one provider. Cheap to clone.
#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    provider: Provider,
    api_key: Option<String>,
    /// OpenRouter: route only to providers that do not store prompts.
    pub deny_data_collection: bool,
}

impl Client {
    pub fn new(provider: Provider, api_key: Option<String>) -> Self {
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(180))
            .user_agent(concat!("live-notes/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("http client");
        Client { http, provider, api_key: api_key.filter(|k| !k.trim().is_empty()), deny_data_collection: true }
    }

    pub fn provider(&self) -> &Provider {
        &self.provider
    }

    fn needs_key(&self) -> bool {
        matches!(self.provider.kind, ProviderKind::OpenRouter | ProviderKind::OpenAi | ProviderKind::ChatGpt)
    }

    fn url(&self, path: &str) -> String {
        format!("{}/{}", self.provider.base_url.trim_end_matches('/'), path.trim_start_matches('/'))
    }

    fn req(&self, method: reqwest::Method, path: &str) -> Result<reqwest::RequestBuilder> {
        self.req_url(method, &self.url(path))
    }

    /// A request to an absolute URL with the provider's key and headers (endpoints outside
    /// the OpenAI-compatible base, e.g. OpenRouter's `/api/alpha/decisions`).
    pub(crate) fn req_url(&self, method: reqwest::Method, url: &str) -> Result<reqwest::RequestBuilder> {
        if self.needs_key() && self.api_key.is_none() {
            return Err(Error::MissingKey(self.provider.name.clone()));
        }
        let mut b = self.http.request(method, url);
        if let Some(k) = &self.api_key {
            b = b.bearer_auth(k);
        }
        for (k, v) in &self.provider.headers {
            b = b.header(k, v);
        }
        Ok(b)
    }

    fn chat_body(&self, r: &ChatRequest, stream: bool) -> Value {
        let mut body = json!({ "model": r.model, "messages": r.messages, "stream": stream });
        if let Some(t) = r.temperature {
            body["temperature"] = json!(t);
        }
        if let Some(m) = r.max_tokens {
            body["max_tokens"] = json!(m);
        }
        if r.json {
            body["response_format"] = json!({ "type": "json_object" });
        }
        if self.provider.kind == ProviderKind::OpenRouter {
            body["usage"] = json!({ "include": true });
            if self.deny_data_collection {
                body["provider"] = json!({ "data_collection": "deny" });
            }
        }
        if stream && self.provider.kind != ProviderKind::OpenRouter {
            body["stream_options"] = json!({ "include_usage": true });
        }
        body
    }

    pub async fn chat(&self, r: &ChatRequest) -> Result<ChatResponse> {
        if self.provider.kind == ProviderKind::ChatGpt {
            return self.responses_stream(r, |_| true).await;
        }
        let resp = self.req(reqwest::Method::POST, "chat/completions")?.json(&self.chat_body(r, false)).send().await?;
        let v = check(resp).await?;
        let text = v["choices"][0]["message"]["content"].as_str().unwrap_or_default().to_string();
        let usage: Usage = serde_json::from_value(v["usage"].clone()).unwrap_or_default();
        Ok(ChatResponse { text, usage, model: v["model"].as_str().unwrap_or(&r.model).to_string() })
    }

    /// Streams the answer; `on_delta` gets each text piece and may return `false` to cancel.
    pub async fn chat_stream(&self, r: &ChatRequest, mut on_delta: impl FnMut(&str) -> bool) -> Result<ChatResponse> {
        use eventsource_stream::Eventsource;
        if self.provider.kind == ProviderKind::ChatGpt {
            return self.responses_stream(r, on_delta).await;
        }
        let resp = self.req(reqwest::Method::POST, "chat/completions")?.json(&self.chat_body(r, true)).send().await?;
        if !resp.status().is_success() {
            check(resp).await?;
            unreachable!()
        }
        let mut stream = resp.bytes_stream().eventsource();
        let mut text = String::new();
        let mut usage = Usage::default();
        while let Some(ev) = stream.next().await {
            let ev = ev.map_err(|e| Error::Decode(e.to_string()))?;
            if ev.data.trim() == "[DONE]" {
                break;
            }
            let Ok(v) = serde_json::from_str::<Value>(&ev.data) else { continue };
            if let Some(d) = v["choices"][0]["delta"]["content"].as_str()
                && !d.is_empty()
            {
                text.push_str(d);
                if !on_delta(d) {
                    return Err(Error::Cancelled);
                }
            }
            if v.get("usage").is_some_and(|u| !u.is_null()) {
                usage = serde_json::from_value(v["usage"].clone()).unwrap_or_default();
            }
        }
        Ok(ChatResponse { text, usage, model: r.model.clone() })
    }

    /// Responses API body for ChatGPT plan usage: `store: false`, `stream: true`, system
    /// messages as `instructions`, and none of the fields the plan route rejects
    /// (temperature, max_output_tokens, ...).
    fn responses_body(r: &ChatRequest) -> Value {
        use crate::{Content, ContentPart, Role};
        let mut instructions = Vec::new();
        let mut input = Vec::new();
        for m in &r.messages {
            let text_type = if m.role == Role::Assistant { "output_text" } else { "input_text" };
            let content = match &m.content {
                Content::Text(t) => json!([{ "type": text_type, "text": t }]),
                Content::Parts(parts) => Value::Array(
                    parts
                        .iter()
                        .map(|p| match p {
                            ContentPart::Text { text } => json!({ "type": text_type, "text": text }),
                            ContentPart::ImageUrl { image_url } => json!({ "type": "input_image", "image_url": image_url.url }),
                        })
                        .collect(),
                ),
            };
            match m.role {
                Role::System => {
                    if let Content::Text(t) = &m.content {
                        instructions.push(t.clone());
                    }
                }
                Role::User => input.push(json!({ "role": "user", "content": content })),
                Role::Assistant => input.push(json!({ "role": "assistant", "content": content })),
            }
        }
        let mut body = json!({ "model": r.model, "input": input, "store": false, "stream": true });
        if !instructions.is_empty() {
            body["instructions"] = json!(instructions.join("\n\n"));
        }
        body
    }

    async fn responses_stream(&self, r: &ChatRequest, mut on_delta: impl FnMut(&str) -> bool) -> Result<ChatResponse> {
        use eventsource_stream::Eventsource;
        let resp = self.req(reqwest::Method::POST, "responses")?.json(&Self::responses_body(r)).send().await?;
        if !resp.status().is_success() {
            check(resp).await?;
            unreachable!()
        }
        let mut stream = resp.bytes_stream().eventsource();
        let mut text = String::new();
        while let Some(ev) = stream.next().await {
            let ev = ev.map_err(|e| Error::Decode(e.to_string()))?;
            let Ok(v) = serde_json::from_str::<Value>(&ev.data) else { continue };
            let kind = v["type"].as_str().unwrap_or(ev.event.as_str());
            match kind {
                "response.output_text.delta" => {
                    if let Some(d) = v["delta"].as_str()
                        && !d.is_empty()
                    {
                        text.push_str(d);
                        if !on_delta(d) {
                            return Err(Error::Cancelled);
                        }
                    }
                }
                "response.completed" => {
                    let u = &v["response"]["usage"];
                    let usage = Usage {
                        prompt_tokens: u["input_tokens"].as_u64().unwrap_or(0),
                        completion_tokens: u["output_tokens"].as_u64().unwrap_or(0),
                        // Billed to the ChatGPT plan, not per token.
                        cost: Some(0.0),
                    };
                    let model = v["response"]["model"].as_str().unwrap_or(&r.model).to_string();
                    return Ok(ChatResponse { text, usage, model });
                }
                "response.failed" | "error" => {
                    let e = if v["response"]["error"].is_object() { &v["response"]["error"] } else { &v["error"] };
                    let e = if e.is_object() { e } else { &v };
                    return Err(Error::Plan {
                        status: 400,
                        code: e["code"].as_str().unwrap_or("response_failed").to_string(),
                        message: e["message"].as_str().unwrap_or_default().to_string(),
                    });
                }
                "response.incomplete" => {
                    let reason = v["response"]["incomplete_details"]["reason"].as_str().unwrap_or("unknown");
                    return Err(Error::Decode(format!("response incomplete: {reason}")));
                }
                _ => {}
            }
        }
        Err(Error::Decode("stream ended without response.completed".into()))
    }

    pub async fn embeddings(&self, model: &str, inputs: &[String]) -> Result<Vec<Vec<f32>>> {
        let resp = self
            .req(reqwest::Method::POST, "embeddings")?
            .json(&json!({ "model": model, "input": inputs, "encoding_format": "float" }))
            .send()
            .await?;
        let v = check(resp).await?;
        let mut items: Vec<(usize, Vec<f32>)> = v["data"]
            .as_array()
            .ok_or_else(|| Error::Decode("embeddings: no data".into()))?
            .iter()
            .enumerate()
            .map(|(i, d)| {
                let idx = d["index"].as_u64().map(|x| x as usize).unwrap_or(i);
                let e =
                    d["embedding"].as_array().map(|a| a.iter().filter_map(|x| x.as_f64()).map(|x| x as f32).collect()).unwrap_or_default();
                (idx, e)
            })
            .collect();
        items.sort_by_key(|(i, _)| *i);
        if items.len() != inputs.len() {
            return Err(Error::Decode(format!("embeddings: got {} of {}", items.len(), inputs.len())));
        }
        Ok(items.into_iter().map(|(_, e)| e).collect())
    }

    /// `/audio/transcriptions` with a WAV file. `languages` are hints (ISO 639-1).
    pub async fn transcribe(&self, model: &str, wav: Vec<u8>, languages: &[String], prompt: Option<&str>) -> Result<Transcription> {
        let part = reqwest::multipart::Part::bytes(wav).file_name("segment.wav").mime_str("audio/wav")?;
        let mut form = reqwest::multipart::Form::new().text("model", model.to_string()).part("file", part).text("response_format", "json");
        if languages.len() == 1 {
            form = form.text("language", languages[0].clone());
        } else {
            for l in languages {
                form = form.text("languages[]", l.clone());
            }
        }
        if let Some(p) = prompt {
            form = form.text("prompt", p.to_string());
        }
        let resp = self.req(reqwest::Method::POST, "audio/transcriptions")?.multipart(form).send().await?;
        let v = check(resp).await?;
        let mut langs: Vec<String> = Vec::new();
        if let Some(a) = v["languages"].as_array() {
            langs.extend(a.iter().filter_map(|x| x.as_str().or_else(|| x["language"].as_str()).map(str::to_string)));
        }
        if let Some(l) = v["language"].as_str() {
            langs.push(l.to_string());
        }
        Ok(Transcription { text: v["text"].as_str().unwrap_or_default().trim().to_string(), languages: langs })
    }

    pub async fn models(&self) -> Result<Vec<ModelInfo>> {
        // Listing needs no key on OpenRouter; send it anyway when present.
        let mut b = self.http.get(self.url("models"));
        if let Some(k) = &self.api_key {
            b = b.bearer_auth(k);
        }
        let v = check(b.send().await?).await?;
        if let Some(models) = v["models"].as_array() {
            // ChatGPT plan catalog: `slug`, `display_name`, only `visibility: "list"` is for display.
            return Ok(models
                .iter()
                .filter(|m| m["visibility"].as_str().is_none_or(|x| x == "list"))
                .filter_map(|m| {
                    let id = m["slug"].as_str()?.to_string();
                    Some(ModelInfo {
                        name: m["display_name"].as_str().unwrap_or(&id).to_string(),
                        id,
                        input_modalities: vec!["text".into(), "image".into()],
                        context_length: None,
                        price_in: Some(0.0),
                        price_out: Some(0.0),
                    })
                })
                .collect());
        }
        let list = v["data"].as_array().cloned().unwrap_or_default();
        Ok(list
            .iter()
            .map(|m| {
                let per_m = |s: &Value| s.as_str().and_then(|x| x.parse::<f64>().ok()).or_else(|| s.as_f64()).map(|x| x * 1e6);
                ModelInfo {
                    id: m["id"].as_str().unwrap_or_default().to_string(),
                    name: m["name"].as_str().or(m["id"].as_str()).unwrap_or_default().to_string(),
                    input_modalities: m["architecture"]["input_modalities"]
                        .as_array()
                        .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
                        .unwrap_or_else(|| vec!["text".into()]),
                    context_length: m["context_length"].as_u64(),
                    price_in: per_m(&m["pricing"]["prompt"]),
                    price_out: per_m(&m["pricing"]["completion"]),
                }
            })
            .collect())
    }

    /// "Test connection" in settings: a tiny request that checks reachability and the key.
    pub async fn test_connection(&self) -> Result<()> {
        if self.needs_key() && self.api_key.is_none() {
            return Err(Error::MissingKey(self.provider.name.clone()));
        }
        // OpenRouter lists models without a key; `/key` checks the key itself.
        let path = if self.provider.kind == ProviderKind::OpenRouter { "key" } else { "models" };
        let b = self.req(reqwest::Method::GET, path)?;
        check(b.send().await?).await.map(|_| ())
    }
}

pub(crate) async fn check(resp: reqwest::Response) -> Result<Value> {
    let status = resp.status();
    let body = resp.text().await?;
    if !status.is_success() {
        let v = serde_json::from_str::<Value>(&body).unwrap_or(Value::Null);
        let message =
            v["error"]["message"].as_str().or(v["detail"].as_str()).map(str::to_string).unwrap_or_else(|| body.chars().take(200).collect());
        if let Some(code) = v["error"]["code"].as_str()
            && (code.starts_with("subscription_sharing") || code.starts_with("chatpass"))
        {
            return Err(Error::Plan { status: status.as_u16(), code: code.into(), message });
        }
        return Err(Error::Api { status: status.as_u16(), message });
    }
    serde_json::from_str(&body).map_err(|e| Error::Decode(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Message;
    use wiremock::matchers::{body_partial_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn provider(url: &str, kind: ProviderKind) -> Provider {
        Provider {
            id: "t".into(),
            name: "Test".into(),
            kind,
            base_url: url.into(),
            headers: vec![("X-Title".into(), "Live Notes".into())],
            is_cloud: true,
        }
    }

    #[tokio::test]
    async fn chat_sends_openrouter_options() {
        let s = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .and(header("authorization", "Bearer k"))
            .and(header("x-title", "Live Notes"))
            .and(body_partial_json(json!({"provider": {"data_collection": "deny"}, "usage": {"include": true}})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "model": "m", "choices": [{"message": {"content": "Hallo"}}],
                "usage": {"prompt_tokens": 10, "completion_tokens": 2, "cost": 0.0001}})))
            .mount(&s)
            .await;
        let c = Client::new(provider(&s.uri(), ProviderKind::OpenRouter), Some("k".into()));
        let r = c.chat(&ChatRequest::new("m", vec![Message::user("Hi")])).await.unwrap();
        assert_eq!(r.text, "Hallo");
        assert_eq!(r.usage.prompt_tokens, 10);
        assert_eq!(r.usage.cost, Some(0.0001));
    }

    #[tokio::test]
    async fn streaming_and_errors() {
        let s = MockServer::start().await;
        let sse = "data: {\"choices\":[{\"delta\":{\"content\":\"Hal\"}}]}\n\ndata: {\"choices\":[{\"delta\":{\"content\":\"lo\"}}],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":1}}\n\ndata: [DONE]\n\n";
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200).insert_header("content-type", "text/event-stream").set_body_raw(sse, "text/event-stream"),
            )
            .mount(&s)
            .await;
        let c = Client::new(provider(&s.uri(), ProviderKind::Ollama), None);
        let mut got = String::new();
        let r = c
            .chat_stream(&ChatRequest::new("m", vec![Message::user("x")]), |d| {
                got.push_str(d);
                true
            })
            .await
            .unwrap();
        assert_eq!(got, "Hallo");
        assert_eq!(r.usage.prompt_tokens, 3);

        let c2 = Client::new(provider(&s.uri(), ProviderKind::OpenAi), None);
        assert!(matches!(c2.chat(&ChatRequest::new("m", vec![])).await, Err(Error::MissingKey(_))));
    }

    #[tokio::test]
    async fn chatgpt_plan_uses_responses_api() {
        let s = MockServer::start().await;
        let sse = concat!(
            "event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"Hal\"}\n\n",
            "event: response.output_text.delta\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"lo\"}\n\n",
            "event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"model\":\"gpt-x\",\"usage\":{\"input_tokens\":7,\"output_tokens\":2}}}\n\n",
        );
        Mock::given(method("POST"))
            .and(path("/responses"))
            .and(header("authorization", "Bearer at"))
            .and(body_partial_json(json!({"model": "gpt-x", "store": false, "stream": true, "instructions": "Be brief.",
                "input": [{"role": "user", "content": [{"type": "input_text", "text": "Hi"}]}]})))
            .respond_with(ResponseTemplate::new(200).set_body_raw(sse, "text/event-stream"))
            .mount(&s)
            .await;
        Mock::given(method("GET"))
            .and(path("/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"models": [
                {"slug": "gpt-x", "display_name": "GPT X", "visibility": "list"},
                {"slug": "hidden", "display_name": "H", "visibility": "hide"}]})))
            .mount(&s)
            .await;
        let c = Client::new(provider(&s.uri(), ProviderKind::ChatGpt), Some("at".into()));
        let mut req = ChatRequest::new("gpt-x", vec![Message::system("Be brief."), Message::user("Hi")]);
        req.json = true;
        let r = c.chat(&req).await.unwrap();
        assert_eq!(r.text, "Hallo");
        assert_eq!(r.usage.prompt_tokens, 7);
        assert_eq!(r.usage.cost, Some(0.0));
        let body = Client::responses_body(&req);
        for f in ["temperature", "max_output_tokens", "response_format", "text"] {
            assert!(body.get(f).is_none(), "{f} must not be sent on the plan route");
        }
        let m = c.models().await.unwrap();
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].name, "GPT X");
        assert!(matches!(Client::new(provider(&s.uri(), ProviderKind::ChatGpt), None).chat(&req).await, Err(Error::MissingKey(_))));
    }

    #[tokio::test]
    async fn connection_test_checks_the_openrouter_key() {
        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/key"))
            .and(header("authorization", "Bearer good"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": {"label": "x"}})))
            .mount(&s)
            .await;
        Mock::given(method("GET"))
            .and(path("/key"))
            .respond_with(ResponseTemplate::new(401).set_body_json(json!({"error": {"message": "No auth credentials found"}})))
            .mount(&s)
            .await;
        assert!(Client::new(provider(&s.uri(), ProviderKind::OpenRouter), Some("good".into())).test_connection().await.is_ok());
        let e = Client::new(provider(&s.uri(), ProviderKind::OpenRouter), Some("bad".into())).test_connection().await.unwrap_err();
        assert!(matches!(e, Error::Api { status: 401, .. }));
        assert!(matches!(
            Client::new(provider(&s.uri(), ProviderKind::OpenRouter), None).test_connection().await,
            Err(Error::MissingKey(_))
        ));
    }

    #[tokio::test]
    async fn chatgpt_limit_errors_keep_their_code() {
        let s = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/responses"))
            .respond_with(
                ResponseTemplate::new(429)
                    .set_body_json(json!({"error": {"code": "subscription_sharing_usage_limit_exceeded", "message": "limit"}})),
            )
            .mount(&s)
            .await;
        let c = Client::new(provider(&s.uri(), ProviderKind::ChatGpt), Some("at".into()));
        let e = c.chat(&ChatRequest::new("m", vec![Message::user("x")])).await.unwrap_err();
        assert!(matches!(&e, Error::Plan { code, .. } if code == "subscription_sharing_usage_limit_exceeded"));
        assert!(e.user_message(ln_types::UiLanguage::De).contains("Nutzungslimit"));
        // A failure inside the stream.
        let s2 = MockServer::start().await;
        let sse = "data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"code\":\"subscription_sharing_usage_unavailable\",\"message\":\"x\"}}}\n\n";
        Mock::given(method("POST"))
            .and(path("/responses"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(sse, "text/event-stream"))
            .mount(&s2)
            .await;
        let c2 = Client::new(provider(&s2.uri(), ProviderKind::ChatGpt), Some("at".into()));
        let e2 = c2.chat(&ChatRequest::new("m", vec![Message::user("x")])).await.unwrap_err();
        assert!(matches!(&e2, Error::Plan { code, .. } if code == "subscription_sharing_usage_unavailable"));
    }

    #[tokio::test]
    async fn embeddings_transcription_models() {
        let s = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/embeddings"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": [
                {"index": 1, "embedding": [0.0, 1.0]}, {"index": 0, "embedding": [1.0, 0.0]}]})))
            .mount(&s)
            .await;
        Mock::given(method("POST"))
            .and(path("/audio/transcriptions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"text": " 你好 ", "languages": ["zh"]})))
            .mount(&s)
            .await;
        Mock::given(method("GET"))
            .and(path("/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data": [{
                "id": "google/gemma-4-26b-a4b-it", "context_length": 262144,
                "architecture": {"input_modalities": ["image", "text"]},
                "pricing": {"prompt": "0.00000009", "completion": "0.0000003"}}]})))
            .mount(&s)
            .await;
        Mock::given(method("POST"))
            .and(path("/broken"))
            .respond_with(ResponseTemplate::new(401).set_body_json(json!({"error": {"message": "bad key"}})))
            .mount(&s)
            .await;
        let c = Client::new(provider(&s.uri(), ProviderKind::Custom), None);
        let e = c.embeddings("e", &["a".into(), "b".into()]).await.unwrap();
        assert_eq!(e, vec![vec![1.0, 0.0], vec![0.0, 1.0]]);
        let t = c.transcribe("gpt-transcribe", vec![0; 44], &["de".into(), "zh".into()], None).await.unwrap();
        assert_eq!(t.text, "你好");
        assert_eq!(t.languages, vec!["zh"]);
        let m = c.models().await.unwrap();
        assert!(m[0].accepts_images());
        assert!((m[0].price_in.unwrap() - 0.09).abs() < 1e-9);
    }
}
