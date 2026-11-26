//! The local HTTP endpoint for the browser extension (`docs/clip-api.md`) and MCP clients
//! (`/mcp`, `docs/mcp.md`): 127.0.0.1 only, bearer token, extension origins only, 8 MB body
//! limit. Runs on the core's tokio runtime.

use super::ClipRequest;
use crate::Core;
use http_body_util::{BodyExt as _, Full, Limited};
use hyper::body::{Bytes, Incoming};
use hyper::header::{self, HeaderMap, HeaderValue};
use hyper::{Method, Request, Response, StatusCode};
use std::convert::Infallible;
use std::sync::{Arc, Weak};

pub const BODY_LIMIT: usize = 8 * 1024 * 1024;
/// Version of the HTTP API (`api` in `/v1/ping`).
pub const API_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ClipServerStatus {
    /// Disabled in the settings, or not started by the app.
    #[default]
    Off,
    Running {
        port: u16,
    },
    /// Listening failed, e.g. the port is taken.
    Failed(String),
}

/// A running listener; dropping it stops accepting connections.
pub(crate) struct Running {
    /// The port from the settings (0 = any free port).
    pub configured_port: u16,
    /// The port actually bound.
    pub port: u16,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
}

impl Drop for Running {
    fn drop(&mut self) {
        if let Some(s) = self.stop.take() {
            let _ = s.send(());
        }
    }
}

pub(crate) fn start(core: &Arc<Core>, port: u16) -> std::io::Result<Running> {
    let std_listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port))?;
    std_listener.set_nonblocking(true)?;
    let bound = std_listener.local_addr()?.port();
    let _rt = core.runtime().enter();
    let listener = tokio::net::TcpListener::from_std(std_listener)?;
    let (stop, mut stopped) = tokio::sync::oneshot::channel::<()>();
    let weak = Arc::downgrade(core);
    core.runtime().spawn(async move {
        loop {
            tokio::select! {
                _ = &mut stopped => break,
                accepted = listener.accept() => {
                    let Ok((stream, _)) = accepted else { continue };
                    let weak = weak.clone();
                    tokio::spawn(async move {
                        let service = hyper::service::service_fn(move |req| handle(weak.clone(), bound, req));
                        let io = hyper_util::rt::TokioIo::new(stream);
                        if let Err(e) = hyper::server::conn::http1::Builder::new().serve_connection(io, service).await {
                            tracing::debug!("clip: connection: {e}");
                        }
                    });
                }
            }
        }
        tracing::info!("clip: stopped listening on port {bound}");
    });
    Ok(Running { configured_port: port, port: bound, stop: Some(stop) })
}

type Resp = Response<Full<Bytes>>;

fn json(status: StatusCode, value: serde_json::Value) -> Resp {
    let mut r = Response::new(Full::new(Bytes::from(value.to_string())));
    *r.status_mut() = status;
    r.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
    r.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    r
}

fn error(status: StatusCode, msg: &str) -> Resp {
    json(status, serde_json::json!({ "error": msg }))
}

/// Only browser extensions may call; a web page's `fetch` to localhost always carries its origin.
/// Requests without `Origin` (curl, native tools) still need the token.
pub(crate) fn origin_allowed(origin: Option<&str>) -> bool {
    match origin {
        None => true,
        Some(o) => o.starts_with("moz-extension://") || o.starts_with("chrome-extension://"),
    }
}

/// Guards against DNS rebinding: the request must name the loopback host.
pub(crate) fn host_allowed(host: Option<&str>, port: u16) -> bool {
    let Some(h) = host else { return true };
    let h = h.to_ascii_lowercase();
    let (name, p) = match h.strip_prefix('[') {
        Some(rest) => match rest.split_once(']') {
            Some((n, tail)) => (format!("[{n}]"), tail.strip_prefix(':')),
            None => return false,
        },
        None => match h.split_once(':') {
            Some((n, p)) => (n.to_string(), Some(p)),
            None => (h.clone(), None),
        },
    };
    if p.is_some_and(|p| p != port.to_string()) {
        return false;
    }
    matches!(name.as_str(), "127.0.0.1" | "localhost" | "[::1]")
}

/// Compares in constant time, so the token can't be guessed byte by byte from response times.
pub(crate) fn token_matches(headers: &HeaderMap, token: &str) -> bool {
    let Some(given) = headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).and_then(|v| v.strip_prefix("Bearer ")) else {
        return false;
    };
    let (a, b) = (given.trim().as_bytes(), token.as_bytes());
    if a.len() != b.len() || b.is_empty() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Everything before routing: origin, host, preflight, token. `Err` is the response to send.
pub(crate) fn check(method: &Method, headers: &HeaderMap, token: &str, port: u16) -> Result<(), Box<Resp>> {
    let origin = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok());
    if !origin_allowed(origin) {
        return Err(Box::new(error(StatusCode::FORBIDDEN, "origin not allowed")));
    }
    if !host_allowed(headers.get(header::HOST).and_then(|v| v.to_str().ok()), port) {
        return Err(Box::new(error(StatusCode::FORBIDDEN, "host not allowed")));
    }
    if method == Method::OPTIONS {
        let mut r = Response::new(Full::new(Bytes::new()));
        *r.status_mut() = StatusCode::NO_CONTENT;
        r.headers_mut().insert(header::ACCESS_CONTROL_ALLOW_METHODS, HeaderValue::from_static("GET, POST, OPTIONS"));
        r.headers_mut().insert(header::ACCESS_CONTROL_ALLOW_HEADERS, HeaderValue::from_static("authorization, content-type"));
        r.headers_mut().insert(header::ACCESS_CONTROL_MAX_AGE, HeaderValue::from_static("600"));
        return Err(Box::new(r));
    }
    if !token_matches(headers, token) {
        return Err(Box::new(error(StatusCode::UNAUTHORIZED, "missing or wrong token")));
    }
    Ok(())
}

async fn handle(core: Weak<Core>, port: u16, req: Request<Incoming>) -> Result<Resp, Infallible> {
    let origin = req.headers().get(header::ORIGIN).cloned();
    let mut resp = route(core, port, req).await;
    // Extension origins may read the answer.
    if let Some(o) = origin.filter(|o| origin_allowed(o.to_str().ok())) {
        resp.headers_mut().insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, o);
    }
    Ok(resp)
}

async fn route(core: Weak<Core>, port: u16, req: Request<Incoming>) -> Resp {
    let Some(core) = core.upgrade() else { return error(StatusCode::SERVICE_UNAVAILABLE, "app is closing") };
    let token = core.clip_token();
    if let Err(r) = check(req.method(), req.headers(), &token, port) {
        return *r;
    }
    let path = req.uri().path().to_string();
    match (req.method().clone(), path.as_str()) {
        (Method::GET, "/v1/ping") => {
            json(StatusCode::OK, serde_json::json!({"app": "live-notes", "version": env!("CARGO_PKG_VERSION"), "api": API_VERSION}))
        }
        (Method::POST, "/v1/clip") => {
            let body = match read_body(req).await {
                Ok(b) => b,
                Err(r) => return *r,
            };
            let clip: ClipRequest = match serde_json::from_slice(&body) {
                Ok(c) => c,
                Err(e) => return error(StatusCode::BAD_REQUEST, &format!("invalid JSON: {e}")),
            };
            match core.clip(clip) {
                Ok(id) => json(StatusCode::ACCEPTED, serde_json::json!({ "job_id": id })),
                Err(e) => error(StatusCode::BAD_REQUEST, &e),
            }
        }
        (Method::GET, p) if p.starts_with("/v1/clip/") => {
            let id = &p["/v1/clip/".len()..];
            match core.clip_job(id) {
                Some(job) => json(StatusCode::OK, serde_json::to_value(job).unwrap_or_default()),
                None => error(StatusCode::NOT_FOUND, "unknown job"),
            }
        }
        (Method::POST, "/mcp") => {
            let version = req.headers().get("mcp-protocol-version").and_then(|v| v.to_str().ok()).map(str::to_string);
            if !crate::mcp::protocol_header_ok(version.as_deref()) {
                return error(StatusCode::BAD_REQUEST, "unsupported MCP-Protocol-Version");
            }
            let body = match read_body(req).await {
                Ok(b) => b,
                Err(r) => return *r,
            };
            let msg: serde_json::Value = match serde_json::from_slice(&body) {
                Ok(m) => m,
                Err(e) => return json(StatusCode::BAD_REQUEST, crate::mcp::parse_error(&format!("invalid JSON: {e}"))),
            };
            match crate::mcp::handle(&core, msg).await {
                Some(answer) => json(StatusCode::OK, answer),
                None => {
                    let mut r = Response::new(Full::new(Bytes::new()));
                    *r.status_mut() = StatusCode::ACCEPTED;
                    r
                }
            }
        }
        // No server-initiated SSE stream and no sessions to end (Streamable HTTP allows both).
        (_, "/mcp") => error(StatusCode::METHOD_NOT_ALLOWED, "POST only"),
        (_, "/v1/ping" | "/v1/clip") => error(StatusCode::METHOD_NOT_ALLOWED, "method not allowed"),
        _ => error(StatusCode::NOT_FOUND, "not found"),
    }
}

/// The request body, at most [`BODY_LIMIT`]; an announced larger body is refused unread.
async fn read_body(req: Request<Incoming>) -> Result<Bytes, Box<Resp>> {
    let too_big = req
        .headers()
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<usize>().ok())
        .is_some_and(|n| n > BODY_LIMIT);
    if too_big {
        return Err(Box::new(error(StatusCode::PAYLOAD_TOO_LARGE, "body larger than 8 MB")));
    }
    match Limited::new(req.into_body(), BODY_LIMIT).collect().await {
        Ok(b) => Ok(b.to_bytes()),
        Err(_) => Err(Box::new(error(StatusCode::PAYLOAD_TOO_LARGE, "body larger than 8 MB"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(*k, HeaderValue::from_str(v).unwrap());
        }
        h
    }

    #[test]
    fn origins() {
        assert!(origin_allowed(None));
        assert!(origin_allowed(Some("moz-extension://1234-abcd")));
        assert!(origin_allowed(Some("chrome-extension://abcdef")));
        assert!(!origin_allowed(Some("https://evil.example")));
        assert!(!origin_allowed(Some("null")));
        assert!(!origin_allowed(Some("http://127.0.0.1:47823")));
    }

    #[test]
    fn hosts() {
        assert!(host_allowed(None, 47823));
        assert!(host_allowed(Some("127.0.0.1:47823"), 47823));
        assert!(host_allowed(Some("localhost:47823"), 47823));
        assert!(host_allowed(Some("LOCALHOST"), 47823));
        assert!(host_allowed(Some("[::1]:47823"), 47823));
        assert!(!host_allowed(Some("127.0.0.1:1234"), 47823));
        assert!(!host_allowed(Some("evil.example:47823"), 47823));
        assert!(!host_allowed(Some("127.0.0.1.evil.example"), 47823));
    }

    #[test]
    fn tokens_and_preflight() {
        let t = "a".repeat(64);
        let ok = headers(&[("authorization", &format!("Bearer {t}"))]);
        assert!(token_matches(&ok, &t));
        assert!(!token_matches(&headers(&[("authorization", &format!("Bearer {}", "b".repeat(64)))]), &t));
        assert!(!token_matches(&headers(&[("authorization", "Bearer short")]), &t));
        assert!(!token_matches(&headers(&[("authorization", &t)]), &t));
        assert!(!token_matches(&HeaderMap::new(), &t));
        assert!(!token_matches(&headers(&[("authorization", "Bearer ")]), ""));

        assert!(check(&Method::GET, &ok, &t, 1).is_ok());
        let status = |r: Result<(), Box<Resp>>| r.err().map(|r| r.status());
        assert_eq!(status(check(&Method::GET, &HeaderMap::new(), &t, 1)), Some(StatusCode::UNAUTHORIZED));
        let web = headers(&[("authorization", &format!("Bearer {t}")), ("origin", "https://evil.example")]);
        assert_eq!(status(check(&Method::POST, &web, &t, 1)), Some(StatusCode::FORBIDDEN));
        let pre = headers(&[("origin", "moz-extension://x")]);
        assert_eq!(status(check(&Method::OPTIONS, &pre, &t, 1)), Some(StatusCode::NO_CONTENT));
        let rebound = headers(&[("authorization", &format!("Bearer {t}")), ("host", "attacker.example:1")]);
        assert_eq!(status(check(&Method::GET, &rebound, &t, 1)), Some(StatusCode::FORBIDDEN));
    }
}
