//! Request validation, note rendering and the endpoint end to end (fake summary model).

use super::render::{Body, ClipNote, clean_page_text, html_to_text, render, split_text};
use super::*;
use ln_platform::{MemoryStore, SecretStore};
use ln_types::{AppPaths, Provider, ProviderKind, UiLanguage};
use serde_json::json;
use std::io::{Read, Write};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn page(text: &str) -> ClipRequest {
    ClipRequest {
        kind: ClipKind::Page,
        url: "https://example.com/a/b(c)".into(),
        title: Some("  Ein   Artikel ".into()),
        site_name: Some("Example".into()),
        byline: Some("Erika Muster".into()),
        lang: Some("de-DE".into()),
        text: Some(text.into()),
        html: None,
        selection: None,
        youtube: None,
    }
}

#[test]
fn validation() {
    let mut r = page("Absatz eins.\n\nAbsatz zwei.");
    r.validate().unwrap();
    assert_eq!(r.title.as_deref(), Some("Ein Artikel"));
    assert_eq!(r.lang.as_deref(), Some("de"));

    for bad in ["ftp://x", "javascript:alert(1)", "https://exa mple.com", ""] {
        let mut r = page("x");
        r.url = bad.into();
        assert!(r.validate().is_err(), "{bad}");
    }
    let mut empty = page("   ");
    assert!(empty.validate().is_err());
    let mut sel = page("");
    sel.selection = Some("Nur das".into());
    assert!(sel.validate().is_ok());
    let mut odd_lang = page("x");
    odd_lang.lang = Some("not a language".into());
    odd_lang.validate().unwrap();
    assert_eq!(odd_lang.lang, None);

    // YouTube: the id comes from the URL when missing; empty cues are dropped and sorted.
    let mut yt: ClipRequest = serde_json::from_value(json!({
        "kind": "youtube", "url": "https://youtu.be/dQw4w9WgXcQ", "title": "Song",
        "youtube": {"transcript": [{"start_ms": 5000, "text": "b"}, {"start_ms": 1000, "dur_ms": 900, "text": "a"}, {"start_ms": 2000, "text": "  "}]}
    }))
    .unwrap();
    yt.validate().unwrap();
    let y = yt.youtube.as_ref().unwrap();
    assert_eq!(y.video_id, "dQw4w9WgXcQ");
    assert_eq!(y.transcript.as_ref().unwrap().iter().map(|c| c.text.as_str()).collect::<Vec<_>>(), vec!["a", "b"]);
    let mut no_id: ClipRequest = serde_json::from_value(json!({"kind": "youtube", "url": "https://example.com/v"})).unwrap();
    assert!(no_id.validate().is_err());
    let mut bad_id: ClipRequest =
        serde_json::from_value(json!({"kind": "youtube", "url": "https://youtu.be/x", "youtube": {"video_id": "../../etc"}})).unwrap();
    assert!(bad_id.validate().is_err());
    assert!(serde_json::from_value::<ClipRequest>(json!({"kind": "pdf", "url": "https://x"})).is_err());
}

fn created() -> chrono::DateTime<chrono::FixedOffset> {
    chrono::DateTime::parse_from_rfc3339("2026-11-26T10:15:00+01:00").unwrap()
}

#[test]
fn renders_page_note() {
    let n = ClipNote {
        kind: ClipKind::Page,
        url: "https://example.com/a(b)".into(),
        title: "Ein Artikel".into(),
        site_name: Some("Example".into()),
        byline: Some("Erika".into()),
        channel: None,
        duration_ms: None,
        lang: Some("de".into()),
        created: created(),
        summary: Ok(WebSummary {
            title: "x".into(),
            summary: "Es geht um A. Und um B.".into(),
            key_points: vec!["- A ist wichtig".into(), "B auch".into()],
            language: Some("de".into()),
        }),
        body: Body::Page { text: "Absatz eins.\n\n<!-- ln:transcript -->\nAbsatz zwei.".into(), selection: false },
    };
    let text = render(&n, UiLanguage::De);
    let note = ln_notes::Note::parse(&text).unwrap();
    let f = note.front.as_ref().unwrap();
    assert_eq!(f.source.as_deref(), Some("https://example.com/a(b)"));
    assert_eq!(f.source_kind.as_deref(), Some("page"));
    assert_eq!(f.title.as_deref(), Some("Ein Artikel"));
    assert_eq!(f.languages, vec![Lang::new("de")]);
    assert!(f.channel.is_none() && f.duration.is_none());
    assert!(text.contains("# Ein Artikel\n\n[example.com](https://example.com/a%28b%29) · Example · Erika\n"), "{text}");
    assert!(text.contains("## Zusammenfassung\nEs geht um A. Und um B.\n\n### Kernpunkte\n- A ist wichtig\n- B auch\n"), "{text}");
    assert!(text.contains("## Seitentext\nAbsatz eins."));
    // Outside text can't open sections.
    assert!(note.section(&ln_notes::SectionKind::Transcript).is_none(), "{text}");
    assert_eq!(note.render(), text, "parse and render roundtrip");

    let failed = ClipNote { summary: Err("Der OpenRouter-API-Schlüssel fehlt.".into()), ..n };
    let text = render(&failed, UiLanguage::En);
    assert!(text.contains("## Summary\n> **No summary:** Der OpenRouter-API-Schlüssel fehlt.\n"), "{text}");
    assert!(text.contains("## Page text\n"));
}

#[test]
fn renders_youtube_note_with_linked_transcript() {
    let cue = |s: u64, t: &str| CaptionCue { start_ms: s, dur_ms: 2000, text: t.into() };
    let n = ClipNote {
        kind: ClipKind::Youtube,
        url: "https://www.youtube.com/watch?v=jNQXAC9IVRw".into(),
        title: "Me at the zoo".into(),
        site_name: None,
        byline: None,
        channel: Some("jawed (official)".into()),
        duration_ms: Some(19_000),
        lang: Some("en".into()),
        created: created(),
        summary: Ok(WebSummary { title: "t".into(), summary: "Elephants.".into(), key_points: vec![], language: None }),
        body: Body::Video {
            video_id: "jNQXAC9IVRw".into(),
            lines: vec![cue(1200, "All right, so here we are"), cue(62_000, "long trunks")],
            transcriber: Some("youtube/captions".into()),
        },
    };
    let text = render(&n, UiLanguage::De);
    let note = ln_notes::Note::parse(&text).unwrap();
    let f = note.front.as_ref().unwrap();
    assert_eq!(f.source_kind.as_deref(), Some("youtube"));
    assert_eq!(f.channel.as_deref(), Some("jawed (official)"));
    assert_eq!(f.duration.as_deref(), Some("00:00:19"));
    assert_eq!(f.transcription.as_deref(), Some("youtube/captions"));
    assert!(text.contains("[YouTube](https://www.youtube.com/watch?v=jNQXAC9IVRw) · jawed (official) · 0:19\n"), "{text}");
    assert!(!text.contains("Kernpunkte"));
    assert!(
        text.contains(
            "## Transkript\n[00:00:01](https://www.youtube.com/watch?v=jNQXAC9IVRw&t=1s) jawed official (en): All right, so here we are\n"
        ),
        "{text}"
    );
    let lines = note.transcript_lines();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[1].at_ms, 62_000);
    assert_eq!(lines[1].link.as_deref(), Some("https://www.youtube.com/watch?v=jNQXAC9IVRw&t=62s"));
    assert_eq!(lines[1].label, "jawed official");
    // The summary stays before the transcript.
    assert!(text.find("<!-- ln:summary -->").unwrap() < text.find("<!-- ln:transcript -->").unwrap());
}

#[test]
fn text_helpers() {
    assert_eq!(clean_page_text("  a \n b\n\n\n\n c  "), "a\nb\n\nc");
    assert_eq!(
        html_to_text("<article><h1>T</h1><p>Eins &amp; <b>zwei</b></p><script>x()</script><ul><li>drei</li></ul></article>"),
        "T\n\nEins & zwei\n\ndrei"
    );
    let parts = split_text("aaaa\nbbbb\ncccc\n", 10);
    assert_eq!(parts, vec!["aaaa\nbbbb\n", "cccc\n"]);
    assert_eq!(split_text(&"x".repeat(25), 10).len(), 3);
}

#[test]
fn token_is_created_once_and_survives_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemoryStore::default());
    let (core, _rx) = Core::start(AppPaths::in_dir(dir.path()), secrets.clone()).unwrap();
    let t = core.clip_token();
    assert_eq!(t.len(), 64);
    assert!(t.bytes().all(|b| b.is_ascii_hexdigit()));
    assert_eq!(core.clip_token(), t);
    assert!(core.clip_token_stored());
    assert_eq!(secrets.get(TOKEN_KEY).as_deref(), Some(t.as_str()));
    let t2 = core.regenerate_clip_token();
    assert_ne!(t, t2);
    assert_eq!(core.clip_token(), t2);
    core.shutdown();
}

/// A secret store that can't store anything (no Secret Service running).
struct BrokenStore;
impl SecretStore for BrokenStore {
    fn get(&self, _: &str) -> Option<String> {
        None
    }
    fn set(&self, _: &str, _: &str) -> ln_platform::Result<()> {
        Err(ln_platform::Error::Other("no secret service".into()))
    }
    fn delete(&self, _: &str) -> ln_platform::Result<()> {
        Ok(())
    }
}

#[test]
fn token_without_secret_store_lives_in_memory() {
    let dir = tempfile::tempdir().unwrap();
    let (core, _rx) = Core::start(AppPaths::in_dir(dir.path()), Arc::new(BrokenStore)).unwrap();
    let t = core.clip_token();
    assert_eq!(t.len(), 64);
    assert!(!core.clip_token_stored());
    assert_eq!(core.clip_token(), t);
    core.shutdown();
}

// ---- endpoint end to end ------------------------------------------------------------------

/// A minimal HTTP/1.1 client (the app's own crates may not use reqwest outside `ln-ai`).
fn http(port: u16, method: &str, path: &str, headers: &[(&str, &str)], body: &str) -> (u16, serde_json::Value) {
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    let mut req = format!("{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\nContent-Length: {}\r\n", body.len());
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\n");
    req.push_str(body);
    s.write_all(req.as_bytes()).unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    let status = out.split_whitespace().nth(1).unwrap().parse().unwrap();
    let body = out.split_once("\r\n\r\n").map(|x| x.1).unwrap_or("");
    (status, serde_json::from_str(body).unwrap_or(serde_json::Value::Null))
}

fn wait_done(port: u16, auth: &str, job: &str) -> serde_json::Value {
    for _ in 0..200 {
        let (code, v) = http(port, "GET", &format!("/v1/clip/{job}"), &[("Authorization", auth)], "");
        assert_eq!(code, 200, "{v}");
        if v["status"] != "pending" {
            return v;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    panic!("job {job} did not finish");
}

#[test]
fn endpoint_saves_page_and_video_notes() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let server = rt.block_on(async {
        let s = MockServer::start().await;
        let summary =
            json!({"title": "Modelltitel", "summary": "Der Artikel erklärt A. Danach B.", "key_points": ["A", "B"], "language": "de"});
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"model": "m", "choices": [{"message": {"content": summary.to_string()}}],
                "usage": {"prompt_tokens": 100, "completion_tokens": 50}})),
            )
            .mount(&s)
            .await;
        s
    });
    let dir = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemoryStore::default());
    secrets.set("openrouter", "test-key").unwrap();
    let (core, _rx) = Core::start(AppPaths::in_dir(dir.path()), secrets).unwrap();
    let uri = server.uri();
    core.update_settings(|s| {
        s.first_run_done = true;
        s.providers.push(Provider {
            id: "mock".into(),
            name: "Mock".into(),
            kind: ProviderKind::Custom,
            base_url: uri.clone(),
            headers: vec![],
            is_cloud: true,
        });
        s.roles.summary = Some(ln_types::RoleConfig { provider: "mock".into(), model: "m".into() });
        s.roles.embeddings.provider = "none".into();
        s.general.ui_language = Some(UiLanguage::De);
        s.clip.port = 0;
    });
    assert_eq!(core.clip_server_status(), ClipServerStatus::Off, "the app starts the endpoint, not Core::start");
    core.start_clip_server();
    let ClipServerStatus::Running { port } = core.clip_server_status() else { panic!("{:?}", core.clip_server_status()) };
    let token = core.clip_token();
    let auth = format!("Bearer {token}");
    let ext = ("Origin", "moz-extension://4f1c-test");

    // Guards.
    assert_eq!(http(port, "GET", "/v1/ping", &[], "").0, 401);
    assert_eq!(http(port, "GET", "/v1/ping", &[("Authorization", "Bearer nope")], "").0, 401);
    assert_eq!(http(port, "GET", "/v1/ping", &[("Authorization", &auth), ("Origin", "https://evil.example")], "").0, 403);
    assert_eq!(http(port, "OPTIONS", "/v1/clip", &[ext], "").0, 204);
    let (code, v) = http(port, "GET", "/v1/ping", &[("Authorization", &auth), ext], "");
    assert_eq!((code, v["app"].as_str(), v["api"].as_u64()), (200, Some("live-notes"), Some(1)));
    assert_eq!(http(port, "POST", "/v1/clip", &[("Authorization", &auth)], "{not json").0, 400);
    assert_eq!(http(port, "POST", "/v1/clip", &[("Authorization", &auth)], r#"{"kind":"page","url":"https://x.example"}"#).0, 400);
    assert_eq!(http(port, "GET", "/v1/clip/unknown", &[("Authorization", &auth)], "").0, 404);
    // An announced body over the limit is refused before it is read.
    let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    let head = format!(
        "POST /v1/clip HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\nAuthorization: {auth}\r\nContent-Length: {}\r\n\r\n",
        super::server::BODY_LIMIT + 1
    );
    s.write_all(head.as_bytes()).unwrap();
    let mut out = String::new();
    let _ = s.read_to_string(&mut out);
    assert!(out.starts_with("HTTP/1.1 413"), "{out}");

    // A page.
    let body = json!({"kind": "page", "url": "https://example.com/artikel", "title": "Ein Artikel", "site_name": "Example",
        "lang": "de", "text": "Erster Absatz.\n\nZweiter Absatz."});
    let (code, v) =
        http(port, "POST", "/v1/clip", &[("Authorization", &auth), ("Content-Type", "application/json"), ext], &body.to_string());
    assert_eq!(code, 202, "{v}");
    let job = wait_done(port, &auth, v["job_id"].as_str().unwrap());
    assert_eq!(job["status"], "done", "{job}");
    assert_eq!(job["title"], "Ein Artikel");
    let page_path = PathBuf::from(job["path"].as_str().unwrap());
    assert!(page_path.starts_with(core.notes_dir().join("Web")), "{}", page_path.display());
    assert!(page_path.file_name().unwrap().to_string_lossy().ends_with(" Ein Artikel.md"));
    let text = std::fs::read_to_string(&page_path).unwrap();
    assert!(text.contains("source_kind: page"), "{text}");
    assert!(text.contains("Der Artikel erklärt A. Danach B.\n\n### Kernpunkte\n- A\n- B\n"), "{text}");
    assert!(text.contains("## Seitentext\nErster Absatz.\n\nZweiter Absatz.\n"), "{text}");

    // The same page again gets its own file.
    let (_, v) = http(port, "POST", "/v1/clip", &[("Authorization", &auth)], &body.to_string());
    let again = wait_done(port, &auth, v["job_id"].as_str().unwrap());
    assert_ne!(again["path"], job["path"]);
    assert!(again["path"].as_str().unwrap().ends_with(" Ein Artikel 2.md"));

    // A video with the transcript from the extension.
    let video = json!({"kind": "youtube", "url": "https://www.youtube.com/watch?v=jNQXAC9IVRw", "title": "Me at the zoo",
        "youtube": {"video_id": "jNQXAC9IVRw", "channel": "jawed", "duration_s": 19, "transcript_lang": "en", "transcript_source": "innertube",
            "transcript": [{"start_ms": 1200, "dur_ms": 2160, "text": "All right, so here we are, in front of the elephants"},
                           {"start_ms": 5318, "dur_ms": 2656, "text": "the cool thing about these guys is that they have really..."}]}});
    let (code, v) = http(port, "POST", "/v1/clip", &[("Authorization", &auth), ext], &video.to_string());
    assert_eq!(code, 202, "{v}");
    let job = wait_done(port, &auth, v["job_id"].as_str().unwrap());
    assert_eq!(job["status"], "done", "{job}");
    let p = PathBuf::from(job["path"].as_str().unwrap());
    assert!(p.starts_with(core.notes_dir().join("YouTube")));
    let text = std::fs::read_to_string(&p).unwrap();
    let note = ln_notes::Note::parse(&text).unwrap();
    assert_eq!(note.front.as_ref().unwrap().channel.as_deref(), Some("jawed"));
    assert_eq!(note.front.as_ref().unwrap().duration.as_deref(), Some("00:00:19"));
    let lines = note.transcript_lines();
    assert_eq!(lines.len(), 1, "short cues are merged: {text}");
    assert_eq!(lines[0].link.as_deref(), Some("https://www.youtube.com/watch?v=jNQXAC9IVRw&t=1s"));
    assert!(lines[0].text.ends_with("they have really..."));
    // The new notes are listed (and indexed by the regular watcher).
    let listed: Vec<_> = core.list_notes().into_iter().map(|n| n.path).collect();
    assert!(listed.contains(&p) && listed.contains(&page_path));

    // Turning the endpoint off closes the port.
    core.update_settings(|s| s.clip.enabled = false);
    assert_eq!(core.clip_server_status(), ClipServerStatus::Off);
    std::thread::sleep(std::time::Duration::from_millis(100));
    assert!(std::net::TcpStream::connect(("127.0.0.1", port)).is_err());
    core.shutdown();
}

#[test]
fn page_is_saved_without_a_summary_model() {
    let dir = tempfile::tempdir().unwrap();
    let (core, _rx) = Core::start(AppPaths::in_dir(dir.path()), Arc::new(MemoryStore::default())).unwrap();
    core.update_settings(|s| {
        s.general.ui_language = Some(UiLanguage::En);
        s.roles.embeddings.provider = "none".into();
    });
    let mut req = page("Only text.");
    req.lang = None;
    let (path, title) = core.clip_now(req, &|_, _| {}).unwrap();
    assert_eq!(title, "Ein Artikel");
    let text = std::fs::read_to_string(path).unwrap();
    assert!(text.contains("## Summary\n> **No summary:** The OpenRouter API key is missing."), "{text}");
    assert!(text.contains("## Page text\nOnly text.\n"));
    core.shutdown();
}
