//! Sign in with ChatGPT with plan usage, for open-source apps that run locally
//! (developers.openai.com/siwc/token-sharing-open-source, 2026-10).
//!
//! Flow: dynamic client registration in the system browser (OIDC + PKCE, loopback callback on
//! 127.0.0.1), code exchange without a client secret, ID-token check against OpenAI's JWKS, then
//! Responses API calls with the access token. Requests are billed to the user's ChatGPT plan.
//! Audio, transcription and embeddings are not part of this flow.

use crate::{Error, Result};
use ring::rand::SecureRandom as _;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{BufRead as _, BufReader, Write as _};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub const APP_NAME: &str = "Live Notes";
pub const RESOURCE: &str = "https://api.openai.com/v1";
pub const USAGE_URL: &str = "https://chatgpt.com/settings/usage";
const SCOPES: &str = "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct";
pub const PLAN_SCOPE: &str = "chatgpt.tokens.use.direct";
const CALLBACK_PATH: &str = "/auth/callback";
/// The port of the first registration; later sign-ins may use any free port.
const PREFERRED_PORT: u16 = 1455;

/// OpenAI's auth endpoints; tests point them at a mock server.
#[derive(Debug, Clone)]
pub struct Endpoints {
    pub issuer: String,
    pub authorize: String,
    pub token: String,
    pub jwks: String,
    pub revoke: String,
}

impl Default for Endpoints {
    fn default() -> Self {
        Endpoints {
            issuer: "https://auth.openai.com".into(),
            authorize: "https://auth.openai.com/api/accounts/authorize".into(),
            token: "https://auth.openai.com/api/accounts/oauth/token".into(),
            jwks: "https://auth.openai.com/.well-known/jwks.json".into(),
            revoke: "https://auth.openai.com/api/accounts/oauth/revoke".into(),
        }
    }
}

/// The account a registration belongs to. Kept after sign-out so the next sign-in reuses the
/// issued client ID (and with it the user's usage limits for this app).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Registration {
    pub client_id: String,
    pub subject: String,
    pub email: Option<String>,
    /// Retained ID token, sent as `id_token_hint` on the next sign-in.
    pub id_token: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Credentials {
    pub registration: Registration,
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: chrono::DateTime<chrono::Utc>,
    pub scopes: Vec<String>,
}

impl Credentials {
    /// The user allowed requests on their ChatGPT plan. A sign-in without it is identity only.
    pub fn plan_usage(&self) -> bool {
        self.scopes.iter().any(|s| s == PLAN_SCOPE)
    }
    pub fn needs_refresh(&self) -> bool {
        self.expires_at - chrono::Utc::now() < chrono::Duration::minutes(5)
    }
}

/// One sign-in attempt: the listener is bound before the browser opens.
pub struct Attempt {
    pub url: String,
    listener: TcpListener,
    redirect_uri: String,
    state: String,
    nonce: String,
    verifier: String,
    /// Issued client ID for a returning account; `None` for a new registration.
    client_id: Option<String>,
    endpoints: Endpoints,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Callback {
    pub code: String,
    pub client_id: String,
}

fn random_b64(bytes: usize) -> String {
    use base64::Engine;
    let mut b = vec![0u8; bytes];
    ring::rand::SystemRandom::new().fill(&mut b).expect("system random");
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b)
}

/// A fresh opaque host ID (`urn:uuid:` with a v4 UUID), persisted by the caller once per host.
pub fn new_host_id() -> String {
    let mut b = [0u8; 16];
    ring::rand::SystemRandom::new().fill(&mut b).expect("system random");
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!("urn:uuid:{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
}

fn pkce_challenge(verifier: &str) -> String {
    use base64::Engine;
    let d = ring::digest::digest(&ring::digest::SHA256, verifier.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(d.as_ref())
}

/// Binds the loopback listener and builds the authorization URL.
/// `previous`: the saved registration of the account to sign in again, if any.
/// `consent`: ask again for plan usage after the user declined it once.
pub fn begin(endpoints: Endpoints, host_id: &str, previous: Option<&Registration>, consent: bool) -> Result<Attempt> {
    let listener = TcpListener::bind(("127.0.0.1", PREFERRED_PORT))
        .or_else(|_| TcpListener::bind(("127.0.0.1", 0)))
        .map_err(|e| Error::Decode(format!("callback listener: {e}")))?;
    let port = listener.local_addr().map_err(|e| Error::Decode(e.to_string()))?.port();
    let redirect_uri = format!("http://127.0.0.1:{port}{CALLBACK_PATH}");
    let state = random_b64(24);
    let nonce = random_b64(24);
    let verifier = random_b64(48);
    let mut q: Vec<(&str, String)> = vec![
        ("response_type", "code".into()),
        ("redirect_uri", redirect_uri.clone()),
        ("scope", SCOPES.into()),
        ("resource", RESOURCE.into()),
        ("state", state.clone()),
        ("nonce", nonce.clone()),
        ("code_challenge_method", "S256".into()),
        ("code_challenge", pkce_challenge(&verifier)),
        ("ext_agent_host_id", host_id.into()),
    ];
    match previous {
        Some(r) => {
            q.push(("client_id", r.client_id.clone()));
            if let Some(t) = &r.id_token {
                q.push(("id_token_hint", t.clone()));
            }
            if let Some(e) = &r.email {
                q.push(("login_hint", e.clone()));
            }
        }
        None => {
            q.push(("client_id", "dynamic_agent_client".into()));
            q.push(("agent_name_hint", APP_NAME.into()));
        }
    }
    if consent {
        q.push(("prompt", "consent".into()));
    }
    let url = reqwest::Url::parse_with_params(&endpoints.authorize, &q).map_err(|e| Error::Decode(e.to_string()))?.to_string();
    Ok(Attempt { url, listener, redirect_uri, state, nonce, verifier, client_id: previous.map(|r| r.client_id.clone()), endpoints })
}

const PAGE_OK: &str = "<!doctype html><meta charset=utf-8><title>Live Notes</title><body style=\"font:15px -apple-system,sans-serif;margin:3em\"><h3>Live Notes is connected to ChatGPT.</h3><p>You can close this tab and return to the app. · Du kannst diesen Tab schliessen und zur App zurückkehren.</p>";
const PAGE_FAIL: &str = "<!doctype html><meta charset=utf-8><title>Live Notes</title><body style=\"font:15px -apple-system,sans-serif;margin:3em\"><h3>Sign-in was not completed.</h3><p>Return to Live Notes and try again. · Kehre zu Live Notes zurück und versuche es erneut.</p>";

fn respond(mut s: &TcpStream, status: &str, body: &str) {
    let _ = write!(
        s,
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
}

impl Attempt {
    /// Waits for the browser to return to the loopback callback.
    pub fn wait(&self, timeout: Duration, cancel: &AtomicBool) -> Result<Callback> {
        self.listener.set_nonblocking(true).map_err(|e| Error::Decode(e.to_string()))?;
        let end = Instant::now() + timeout;
        while Instant::now() < end {
            if cancel.load(Ordering::Relaxed) {
                return Err(Error::Cancelled);
            }
            let stream = match self.listener.accept() {
                Ok((s, _)) => s,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(100));
                    continue;
                }
                Err(e) => return Err(Error::Decode(e.to_string())),
            };
            let _ = stream.set_nonblocking(false);
            let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
            let mut line = String::new();
            if BufReader::new(&stream).read_line(&mut line).is_err() {
                continue;
            }
            let target = line.split_whitespace().nth(1).unwrap_or_default().to_string();
            let Ok(url) = reqwest::Url::parse(&format!("http://127.0.0.1{target}")) else { continue };
            if url.path() != CALLBACK_PATH {
                respond(&stream, "404 Not Found", "");
                continue;
            }
            let get = |k: &str| url.query_pairs().find(|(n, _)| n == k).map(|(_, v)| v.into_owned());
            if get("state").as_deref() != Some(self.state.as_str()) {
                // Not ours (stale tab or a forged request): ignore and keep waiting.
                respond(&stream, "400 Bad Request", PAGE_FAIL);
                continue;
            }
            if let Some(e) = get("error") {
                respond(&stream, "200 OK", PAGE_FAIL);
                return Err(Error::Api { status: 400, message: get("error_description").unwrap_or(e) });
            }
            let Some(code) = get("code") else {
                respond(&stream, "400 Bad Request", PAGE_FAIL);
                return Err(Error::Decode("callback without code".into()));
            };
            let client_id = match (&self.client_id, get("client_id")) {
                (Some(saved), Some(got)) if *saved != got => {
                    respond(&stream, "200 OK", PAGE_FAIL);
                    return Err(Error::Decode("callback returned a different client ID".into()));
                }
                (Some(saved), _) => saved.clone(),
                (None, Some(got)) if got != "dynamic_agent_client" => got,
                (None, _) => {
                    respond(&stream, "200 OK", PAGE_FAIL);
                    return Err(Error::Decode("registration incomplete: no client ID issued".into()));
                }
            };
            respond(&stream, "200 OK", PAGE_OK);
            return Ok(Callback { code, client_id });
        }
        Err(Error::Decode("sign-in timed out".into()))
    }

    /// Exchanges the code, checks the ID token and returns the credentials to store.
    /// `expected_subject`: on a returning sign-in, the account must not change.
    pub async fn finish(&self, http: &reqwest::Client, cb: &Callback, expected_subject: Option<&str>) -> Result<Credentials> {
        let form = [
            ("grant_type", "authorization_code"),
            ("client_id", cb.client_id.as_str()),
            ("code", cb.code.as_str()),
            ("code_verifier", self.verifier.as_str()),
            ("redirect_uri", self.redirect_uri.as_str()),
            ("resource", RESOURCE),
        ];
        let v = token_request(http, &self.endpoints.token, &form).await?;
        let id_token = v["id_token"].as_str().ok_or_else(|| Error::Decode("token response without id_token".into()))?.to_string();
        let claims = verify_id_token(http, &self.endpoints, &id_token, &cb.client_id, &self.nonce).await?;
        let subject = claims["sub"].as_str().unwrap_or_default().to_string();
        if subject.is_empty() {
            return Err(Error::Decode("id_token without sub".into()));
        }
        if let Some(s) = expected_subject
            && s != subject
        {
            return Err(Error::Decode("signed in with a different ChatGPT account".into()));
        }
        let registration = Registration {
            client_id: cb.client_id.clone(),
            subject,
            email: claims["email"].as_str().map(str::to_string),
            id_token: Some(id_token),
        };
        Ok(credentials_from(registration, &v, None))
    }
}

fn credentials_from(registration: Registration, v: &Value, old_refresh: Option<&str>) -> Credentials {
    let expires_in = v["expires_in"].as_i64().unwrap_or(3600);
    Credentials {
        registration,
        access_token: v["access_token"].as_str().unwrap_or_default().to_string(),
        refresh_token: v["refresh_token"].as_str().map(str::to_string).or(old_refresh.map(str::to_string)),
        expires_at: chrono::Utc::now() + chrono::Duration::seconds(expires_in),
        scopes: v["scope"].as_str().unwrap_or_default().split_whitespace().map(str::to_string).collect(),
    }
}

async fn token_request(http: &reqwest::Client, url: &str, form: &[(&str, &str)]) -> Result<Value> {
    let resp = http.post(url).form(form).send().await?;
    let status = resp.status();
    let body = resp.text().await?;
    let v: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    if !status.is_success() {
        let code = v["error"].as_str().or(v["error"]["code"].as_str()).unwrap_or("").to_string();
        let message = v["error_description"].as_str().or(v["error"]["message"].as_str()).map(str::to_string).unwrap_or(code.clone());
        return Err(Error::Plan { status: status.as_u16(), code, message });
    }
    if v["access_token"].as_str().is_none() {
        return Err(Error::Decode("token response without access_token".into()));
    }
    Ok(v)
}

/// Renews the access token. The refresh token rotates: store the result right away.
pub async fn refresh(http: &reqwest::Client, endpoints: &Endpoints, c: &Credentials) -> Result<Credentials> {
    let rt = c.refresh_token.as_deref().ok_or_else(|| Error::Plan {
        status: 401,
        code: "invalid_grant".into(),
        message: "no refresh token".into(),
    })?;
    let form =
        [("grant_type", "refresh_token"), ("client_id", c.registration.client_id.as_str()), ("refresh_token", rt), ("resource", RESOURCE)];
    let v = token_request(http, &endpoints.token, &form).await?;
    let mut n = credentials_from(c.registration.clone(), &v, Some(rt));
    if n.scopes.is_empty() {
        n.scopes = c.scopes.clone();
    }
    if let Some(t) = v["id_token"].as_str() {
        n.registration.id_token = Some(t.to_string());
    }
    Ok(n)
}

/// True for refresh errors after which only a new sign-in helps.
pub fn refresh_is_terminal(e: &Error) -> bool {
    matches!(e, Error::Plan { code, .. } if matches!(code.as_str(),
        "invalid_grant" | "invalid_refresh_token" | "token_expired" | "refresh_token_expired"
        | "refresh_token_invalidated" | "refresh_token_reused" | "invalid_client"))
}

/// Ends the renewable session. An empty 200 is success, also for an already invalid token.
pub async fn revoke(http: &reqwest::Client, endpoints: &Endpoints, c: &Credentials) -> Result<()> {
    let Some(rt) = c.refresh_token.as_deref() else { return Ok(()) };
    let form = [("token", rt), ("token_type_hint", "refresh_token"), ("client_id", c.registration.client_id.as_str())];
    let resp = http.post(&endpoints.revoke).form(&form).send().await?;
    if resp.status().is_success() {
        Ok(())
    } else {
        Err(Error::Api { status: resp.status().as_u16(), message: "revocation failed".into() })
    }
}

fn b64url(s: &str) -> Result<Vec<u8>> {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(s.trim_end_matches('=')).map_err(|e| Error::Decode(format!("base64: {e}")))
}

/// Checks the RS256 signature against the issuer's JWKS, then issuer, audience, expiry and nonce.
async fn verify_id_token(http: &reqwest::Client, ep: &Endpoints, token: &str, client_id: &str, nonce: &str) -> Result<Value> {
    let jwks: Value = http.get(&ep.jwks).send().await?.error_for_status()?.json().await?;
    verify_with_jwks(&jwks, token, &ep.issuer, client_id, nonce, chrono::Utc::now().timestamp())
}

pub(crate) fn verify_with_jwks(jwks: &Value, token: &str, issuer: &str, client_id: &str, nonce: &str, now: i64) -> Result<Value> {
    let bad = |m: &str| Error::Decode(format!("id_token: {m}"));
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return Err(bad("not a JWT"));
    }
    let header: Value = serde_json::from_slice(&b64url(parts[0])?).map_err(|_| bad("header"))?;
    if header["alg"].as_str() != Some("RS256") {
        return Err(bad("unsupported algorithm"));
    }
    let kid = header["kid"].as_str();
    let key = jwks["keys"]
        .as_array()
        .and_then(|keys| keys.iter().find(|k| k["kty"] == "RSA" && (kid.is_none() || k["kid"].as_str() == kid)))
        .ok_or_else(|| bad("signing key not in JWKS"))?;
    let n = b64url(key["n"].as_str().unwrap_or_default())?;
    let e = b64url(key["e"].as_str().unwrap_or_default())?;
    let signed = format!("{}.{}", parts[0], parts[1]);
    ring::signature::RsaPublicKeyComponents { n: &n, e: &e }
        .verify(&ring::signature::RSA_PKCS1_2048_8192_SHA256, signed.as_bytes(), &b64url(parts[2])?)
        .map_err(|_| bad("signature invalid"))?;
    let claims: Value = serde_json::from_slice(&b64url(parts[1])?).map_err(|_| bad("payload"))?;
    if claims["iss"].as_str() != Some(issuer) {
        return Err(bad("issuer"));
    }
    let aud_ok = match &claims["aud"] {
        Value::String(a) => a == client_id,
        Value::Array(a) => a.iter().any(|x| x.as_str() == Some(client_id)),
        _ => false,
    };
    if !aud_ok {
        return Err(bad("audience"));
    }
    if claims["exp"].as_i64().is_none_or(|exp| exp + 60 < now) {
        return Err(bad("expired"));
    }
    if claims["nonce"].as_str() != Some(nonce) {
        return Err(bad("nonce"));
    }
    Ok(claims)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use serde_json::json;

    // RSA-2048 test key (PKCS#8), generated for these tests only.
    const TEST_KEY: &str = include_str!("../testdata/rsa-test-key.b64");

    fn enc(v: &[u8]) -> String {
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v)
    }

    fn keypair() -> ring::rsa::KeyPair {
        let der = base64::engine::general_purpose::STANDARD.decode(TEST_KEY.split_whitespace().collect::<String>()).unwrap();
        ring::rsa::KeyPair::from_pkcs8(&der).unwrap()
    }

    pub(crate) fn sign(claims: &Value) -> (String, Value) {
        let kp = keypair();
        let h = enc(br#"{"alg":"RS256","kid":"k1","typ":"JWT"}"#);
        let p = enc(claims.to_string().as_bytes());
        let input = format!("{h}.{p}");
        let mut sig = vec![0; kp.public().modulus_len()];
        kp.sign(&ring::signature::RSA_PKCS1_SHA256, &ring::rand::SystemRandom::new(), input.as_bytes(), &mut sig).unwrap();
        let pk: ring::rsa::PublicKeyComponents<Vec<u8>> = kp.public().into();
        let jwks = json!({"keys": [{"kty": "RSA", "kid": "k1", "n": enc(&pk.n), "e": enc(&pk.e)}]});
        (format!("{input}.{}", enc(&sig)), jwks)
    }

    #[test]
    fn id_token_checks() {
        let claims = json!({"iss": "https://auth.openai.com", "aud": "oaiapp_1", "sub": "u1", "exp": 2_000_000_000i64, "nonce": "n", "email": "a@b.c"});
        let (tok, jwks) = sign(&claims);
        let ok = verify_with_jwks(&jwks, &tok, "https://auth.openai.com", "oaiapp_1", "n", 1_900_000_000).unwrap();
        assert_eq!(ok["sub"], "u1");
        assert!(verify_with_jwks(&jwks, &tok, "https://auth.openai.com", "other", "n", 1_900_000_000).is_err());
        assert!(verify_with_jwks(&jwks, &tok, "https://auth.openai.com", "oaiapp_1", "x", 1_900_000_000).is_err());
        assert!(verify_with_jwks(&jwks, &tok, "https://evil", "oaiapp_1", "n", 1_900_000_000).is_err());
        assert!(verify_with_jwks(&jwks, &tok, "https://auth.openai.com", "oaiapp_1", "n", 2_100_000_000).is_err());
        // Tampered payload.
        let mut parts: Vec<String> = tok.split('.').map(str::to_string).collect();
        parts[1] = enc(json!({"iss": "https://auth.openai.com", "aud": "oaiapp_1", "sub": "u2", "exp": 2_000_000_000i64, "nonce": "n"})
            .to_string()
            .as_bytes());
        assert!(verify_with_jwks(&jwks, &parts.join("."), "https://auth.openai.com", "oaiapp_1", "n", 1_900_000_000).is_err());
    }

    #[test]
    fn host_id_and_authorize_url() {
        let h = new_host_id();
        assert!(h.starts_with("urn:uuid:") && h.len() == 45 && h.as_bytes()[23] == b'4');
        let a = begin(Endpoints::default(), &h, None, false).unwrap();
        let u = reqwest::Url::parse(&a.url).unwrap();
        let q: std::collections::HashMap<_, _> = u.query_pairs().into_owned().collect();
        assert_eq!(u.as_str().split('?').next(), Some("https://auth.openai.com/api/accounts/authorize"));
        assert_eq!(q["client_id"], "dynamic_agent_client");
        assert_eq!(q["agent_name_hint"], "Live Notes");
        assert_eq!(q["ext_agent_host_id"], h);
        assert!(q["scope"].contains("chatgpt.tokens.use.direct"));
        assert!(q["redirect_uri"].starts_with("http://127.0.0.1:") && q["redirect_uri"].ends_with("/auth/callback"));
        assert_eq!(q["code_challenge"], pkce_challenge(&a.verifier));
        let reg =
            Registration { client_id: "oaiapp_1".into(), subject: "u".into(), email: Some("a@b.c".into()), id_token: Some("t".into()) };
        let b = begin(Endpoints::default(), &h, Some(&reg), true).unwrap();
        let q: std::collections::HashMap<_, _> = reqwest::Url::parse(&b.url).unwrap().query_pairs().into_owned().collect();
        assert_eq!(q["client_id"], "oaiapp_1");
        assert!(!q.contains_key("agent_name_hint"));
        assert_eq!(q["id_token_hint"], "t");
        assert_eq!(q["login_hint"], "a@b.c");
        assert_eq!(q["prompt"], "consent");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn full_sign_in_against_mock() {
        use wiremock::matchers::{body_string_contains, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let s = MockServer::start().await;
        let ep = Endpoints {
            issuer: "https://auth.openai.com".into(),
            authorize: format!("{}/authorize", s.uri()),
            token: format!("{}/token", s.uri()),
            jwks: format!("{}/jwks", s.uri()),
            revoke: format!("{}/revoke", s.uri()),
        };
        let a = begin(ep.clone(), &new_host_id(), None, false).unwrap();
        let (tok, jwks) = sign(&json!({"iss": "https://auth.openai.com", "aud": "oaiapp_9", "sub": "u9",
            "exp": chrono::Utc::now().timestamp() + 3600, "nonce": a.nonce, "email": "me@x.ch"}));
        Mock::given(method("GET")).and(path("/jwks")).respond_with(ResponseTemplate::new(200).set_body_json(jwks)).mount(&s).await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .and(body_string_contains("grant_type=authorization_code"))
            .and(body_string_contains("client_id=oaiapp_9"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"access_token": "at1", "refresh_token": "rt1",
                "id_token": tok, "expires_in": 3600, "token_type": "Bearer",
                "scope": "chatgpt.tokens.use.direct email offline_access openid profile resource.invoke"})))
            .mount(&s)
            .await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .and(body_string_contains("grant_type=refresh_token"))
            .and(body_string_contains("refresh_token=rt1"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"access_token": "at2", "refresh_token": "rt2", "expires_in": 3600})),
            )
            .mount(&s)
            .await;
        // The browser returning to the loopback callback.
        let port = reqwest::Url::parse(&a.redirect_uri).unwrap().port().unwrap();
        let state = a.state.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            let mut c = TcpStream::connect(("127.0.0.1", port)).unwrap();
            write!(c, "GET /auth/callback?code=c1&state={state}&client_id=oaiapp_9 HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
            let mut out = String::new();
            let _ = std::io::Read::read_to_string(&mut c, &mut out);
            assert!(out.contains("200 OK"));
        });
        let cb = a.wait(Duration::from_secs(5), &AtomicBool::new(false)).unwrap();
        assert_eq!(cb, Callback { code: "c1".into(), client_id: "oaiapp_9".into() });
        let http = reqwest::Client::new();
        let c = a.finish(&http, &cb, None).await.unwrap();
        assert!(c.plan_usage());
        assert_eq!(c.registration.email.as_deref(), Some("me@x.ch"));
        assert!(a.finish(&http, &cb, Some("someone-else")).await.is_err());
        let r = refresh(&http, &ep, &c).await.unwrap();
        assert_eq!(r.access_token, "at2");
        assert_eq!(r.refresh_token.as_deref(), Some("rt2"));
        assert!(r.plan_usage(), "scopes are kept when the refresh response omits them");
    }
}
