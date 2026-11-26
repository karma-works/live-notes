//! Protocol handling, the path and HTML helpers, and every tool end to end over `/mcp`
//! (fake summary model and a fake web page from wiremock).

use super::tools::{page_from_html, resolve_note};
use super::*;
use crate::{ClipServerStatus, SearchFilter};
use ln_platform::{MemoryStore, SecretStore};
use ln_types::{AppPaths, Provider, ProviderKind, UiLanguage};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::time::Duration;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn http(port: u16, method: &str, path: &str, headers: &[(&str, &str)], body: &str) -> (u16, String) {
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
    (status, out.split_once("\r\n\r\n").map(|x| x.1.to_string()).unwrap_or_default())
}

struct Client {
    port: u16,
    auth: String,
    next: u64,
}

impl Client {
    fn rpc(&mut self, method: &str, params: Value) -> Value {
        self.next += 1;
        let body = json!({"jsonrpc": "2.0", "id": self.next, "method": method, "params": params}).to_string();
        let headers = [
            ("Authorization", self.auth.as_str()),
            ("Content-Type", "application/json"),
            ("Accept", "application/json, text/event-stream"),
            ("MCP-Protocol-Version", "2025-06-18"),
        ];
        let (code, text) = http(self.port, "POST", "/mcp", &headers, &body);
        assert_eq!(code, 200, "{text}");
        let v: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(v["id"], self.next, "{v}");
        v
    }

    /// `structuredContent` of a successful call; panics on a tool error.
    fn tool(&mut self, name: &str, args: Value) -> Value {
        let v = self.rpc("tools/call", json!({"name": name, "arguments": args}));
        assert_eq!(v["result"]["isError"], false, "{name}: {v}");
        let r = &v["result"];
        assert_eq!(serde_json::from_str::<Value>(r["content"][0]["text"].as_str().unwrap()).unwrap(), r["structuredContent"]);
        r["structuredContent"].clone()
    }

    /// The error text of a failed call.
    fn tool_error(&mut self, name: &str, args: Value) -> String {
        let v = self.rpc("tools/call", json!({"name": name, "arguments": args}));
        assert_eq!(v["result"]["isError"], true, "{name}: {v}");
        v["result"]["content"][0]["text"].as_str().unwrap().to_string()
    }
}

#[test]
fn protocol() {
    let dir = tempfile::tempdir().unwrap();
    let (core, _rx) = Core::start(AppPaths::in_dir(dir.path()), Arc::new(MemoryStore::default())).unwrap();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let call = |msg: Value| rt.block_on(handle(&core, msg));

    let init = call(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "t", "version": "1"}}}))
    .unwrap();
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(init["result"]["serverInfo"]["name"], "live-notes");
    assert!(init["result"]["capabilities"]["tools"].is_object());
    // An unknown (newer) revision gets ours; the client decides whether it can speak it.
    let newer = call(json!({"jsonrpc": "2.0", "id": "a", "method": "initialize", "params": {"protocolVersion": "2099-01-01"}})).unwrap();
    assert_eq!(newer["result"]["protocolVersion"], PROTOCOL_VERSIONS[0]);
    assert_eq!(newer["id"], "a");

    assert_eq!(call(json!({"jsonrpc": "2.0", "method": "notifications/initialized"})), None);
    assert_eq!(call(json!({"jsonrpc": "2.0", "id": 2, "result": {}})), None);
    assert_eq!(call(json!({"jsonrpc": "2.0", "id": 3, "method": "ping"})).unwrap()["result"], json!({}));
    assert_eq!(call(json!({"jsonrpc": "2.0", "id": 4, "method": "resources/list"})).unwrap()["error"]["code"], METHOD_NOT_FOUND);
    assert_eq!(call(json!([{"jsonrpc": "2.0", "id": 5, "method": "ping"}])).unwrap()["error"]["code"], INVALID_REQUEST);
    assert_eq!(call(json!({"id": 6, "method": "ping"})).unwrap()["error"]["code"], INVALID_REQUEST);
    let unknown = call(json!({"jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": {"name": "rm_rf"}})).unwrap();
    assert_eq!(unknown["error"]["code"], INVALID_PARAMS);

    let tools = call(json!({"jsonrpc": "2.0", "id": 8, "method": "tools/list"})).unwrap();
    let names: Vec<&str> = tools["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["search_notes", "read_note", "create_note", "clip_url", "clip_status"]);
    for t in tools["result"]["tools"].as_array().unwrap() {
        assert_eq!(t["inputSchema"]["type"], "object", "{t}");
        assert!(t["description"].as_str().unwrap().len() > 40);
    }
    // Missing arguments are tool errors the model can read.
    let missing =
        call(json!({"jsonrpc": "2.0", "id": 9, "method": "tools/call", "params": {"name": "read_note", "arguments": {}}})).unwrap();
    assert_eq!(missing["result"]["isError"], true);
    assert!(missing["result"]["content"][0]["text"].as_str().unwrap().contains("`path` is required"));

    assert!(protocol_header_ok(None) && protocol_header_ok(Some("2025-03-26")));
    assert!(!protocol_header_ok(Some("1999-01-01")));
    core.shutdown();
}

#[test]
fn note_paths_stay_inside_the_notes_folder() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("Notes");
    std::fs::create_dir_all(root.join("Web")).unwrap();
    std::fs::write(root.join("Web/a.md"), "# A\n").unwrap();
    std::fs::write(root.join("b.txt"), "b").unwrap();
    std::fs::write(dir.path().join("secret.md"), "s").unwrap();
    let real = root.canonicalize().unwrap();
    assert_eq!(resolve_note(&root, "Web/a.md").unwrap(), real.join("Web/a.md"));
    assert_eq!(resolve_note(&root, real.join("Web/a.md").to_str().unwrap()).unwrap(), real.join("Web/a.md"));
    assert!(resolve_note(&root, "../secret.md").unwrap_err().contains("outside"));
    assert!(resolve_note(&root, dir.path().join("secret.md").to_str().unwrap()).is_err());
    assert!(resolve_note(&root, "b.txt").is_err());
    assert!(resolve_note(&root, "Web").is_err());
    assert!(resolve_note(&root, "missing.md").unwrap_err().contains("no note"));
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(dir.path().join("secret.md"), root.join("link.md")).unwrap();
        assert!(resolve_note(&root, "link.md").unwrap_err().contains("outside"));
    }
}

#[test]
fn main_text_of_a_whole_page() {
    let html = r#"<!doctype html><html lang="en-GB"><head><title>A  Story &amp; More</title><style>p{}</style></head>
<body><header><nav><a>Home</a></nav></header><main><article><h1>A Story</h1><p>First paragraph.</p>
<aside>Ad</aside><p>Second <b>bold</b> paragraph.</p></article></main><footer>(c)</footer><script>x()</script></body></html>"#;
    let p = page_from_html(html, "https://example.com/story");
    assert_eq!(p.title.as_deref(), Some("A Story & More"));
    assert_eq!(p.lang.as_deref(), Some("en-gb"), "clip validation normalizes it to `en`");
    assert!(p.text.contains("First paragraph.") && p.text.contains("Second bold paragraph."), "{}", p.text);
    for junk in ["Home", "Ad", "(c)", "x()", "p{}"] {
        assert!(!p.text.contains(junk), "{junk} in {}", p.text);
    }
    // No <article>/<main>: the body without its chrome; `<header` is not `<head`.
    let p = page_from_html("<body><header>Menu</header><div>Only this.</div></body>", "https://x.example");
    assert_eq!((p.title, p.lang), (None, None));
    assert!(p.text.contains("Only this.") && !p.text.contains("Menu"), "{}", p.text);
}

#[test]
fn tools_over_http() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let server = rt.block_on(async {
        let s = MockServer::start().await;
        let summary = json!({"title": "Modelltitel", "summary": "Explains A. Then B.", "key_points": ["A", "B"], "language": "en"});
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"model": "m",
                "choices": [{"message": {"content": summary.to_string()}}], "usage": {"prompt_tokens": 100, "completion_tokens": 50}})))
            .mount(&s)
            .await;
        let page = "<html lang=en><head><title>Fetched Story</title></head><body><nav>Menu</nav><article><p>The quokka smiles a lot.</p></article></body></html>";
        Mock::given(method("GET"))
            .and(path("/story"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(page, "text/html; charset=utf-8"))
            .mount(&s)
            .await;
        Mock::given(method("GET")).and(path("/file.zip")).respond_with(ResponseTemplate::new(200).set_body_raw("PK", "application/zip")).mount(&s).await;
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
        s.general.ui_language = Some(UiLanguage::En);
        s.clip.port = 0;
    });
    core.start_clip_server();
    let ClipServerStatus::Running { port } = core.clip_server_status() else { panic!("{:?}", core.clip_server_status()) };
    let auth = format!("Bearer {}", core.clip_token());

    // Same guards as the clip API, plus the MCP transport rules.
    let init = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}"#;
    assert_eq!(http(port, "POST", "/mcp", &[], init).0, 401);
    assert_eq!(http(port, "POST", "/mcp", &[("Authorization", &auth), ("Origin", "https://evil.example")], init).0, 403);
    assert_eq!(http(port, "GET", "/mcp", &[("Authorization", &auth)], "").0, 405);
    assert_eq!(http(port, "POST", "/mcp", &[("Authorization", &auth), ("MCP-Protocol-Version", "1999-01-01")], init).0, 400);
    let (code, body) = http(port, "POST", "/mcp", &[("Authorization", &auth)], "{nope");
    assert_eq!((code, serde_json::from_str::<Value>(&body).unwrap()["error"]["code"].as_i64()), (400, Some(PARSE_ERROR)));
    let note = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
    assert_eq!(http(port, "POST", "/mcp", &[("Authorization", &auth)], note), (202, String::new()));

    let mut c = Client { port, auth, next: 0 };
    assert_eq!(c.rpc("initialize", json!({"protocolVersion": "2025-06-18"}))["result"]["protocolVersion"], "2025-06-18");

    // create_note → read_note.
    let created = c.tool(
        "create_note",
        json!({"title": "Quokka  facts", "content": "Quokkas live on Rottnest Island.\n\n- [ ] visit", "folder": "Animals"}),
    );
    assert_eq!(created["path"], "Animals/Quokka facts.md");
    let again = c.tool("create_note", json!({"title": "Quokka facts"}));
    assert_eq!(again["path"], "Quokka facts.md");
    let third = c.tool("create_note", json!({"title": "Quokka facts", "folder": "Animals"}));
    assert_eq!(third["path"], "Animals/Quokka facts 2.md", "never overwrites");
    assert!(c.tool_error("create_note", json!({"title": "x", "folder": "../outside"})).contains("relative"));
    assert!(c.tool_error("create_note", json!({"title": "x", "folder": ".hidden"})).contains("relative"));
    assert!(c.tool_error("create_note", json!({"content": "no title"})).contains("`title` is required"));

    let read = c.tool("read_note", json!({"path": "Animals/Quokka facts.md"}));
    assert_eq!(read["title"], "Quokka facts");
    let text = read["content"].as_str().unwrap();
    assert!(text.starts_with("---\nid: ") && text.contains("title: Quokka facts\n"), "{text}");
    assert!(text.contains("---\n# Quokka facts\n\nQuokkas live on Rottnest Island.\n\n- [ ] visit\n"), "{text}");
    let note = ln_notes::Note::parse(text).unwrap();
    assert!(note.front.unwrap().created.is_some());
    let outside = core.notes_dir().parent().unwrap().join("outside.md");
    std::fs::write(&outside, "private").unwrap();
    assert!(c.tool_error("read_note", json!({"path": "../outside.md"})).contains("outside the notes folder"));
    assert!(c.tool_error("read_note", json!({"path": outside.to_str().unwrap()})).contains("outside the notes folder"));
    assert!(c.tool_error("read_note", json!({"path": "Animals/none.md"})).contains("no note"));

    // search_notes, once the indexer has seen the note.
    let mut found = json!(null);
    for _ in 0..100 {
        core.indexer.resync_all();
        std::thread::sleep(Duration::from_millis(100));
        if !core.search_files("Rottnest", &SearchFilter::default(), 5).is_empty() {
            found = c.tool("search_notes", json!({"query": "Rottnest", "limit": 5}));
            break;
        }
    }
    let results = found["results"].as_array().expect("note is searchable");
    assert_eq!(results.len(), 1, "{found}");
    assert_eq!(results[0]["path"], "Animals/Quokka facts.md");
    assert_eq!(results[0]["title"], "Quokka facts");
    assert_eq!(results[0]["in_notes_folder"], true);
    assert!(results[0]["snippet"].as_str().unwrap().contains("Rottnest"));
    // The returned path reads back.
    assert_eq!(c.tool("read_note", json!({"path": results[0]["path"]}))["title"], "Quokka facts");
    let elsewhere = c.tool("search_notes", json!({"query": "Rottnest", "folder": "Web"}));
    assert_eq!(elsewhere["results"], json!([]));
    assert!(c.tool_error("search_notes", json!({"query": "  "})).contains("required"));

    // clip_url with the text given, and with the page fetched by the app.
    let given = c.tool("clip_url", json!({"url": "https://example.com/given", "title": "Given Page", "text": "Numbats eat termites."}));
    assert_eq!(given["status"], "done", "{given}");
    assert_eq!(given["title"], "Given Page");
    let p = given["path"].as_str().unwrap();
    let month = chrono::Local::now().format("%Y/%m/").to_string();
    assert!(p.starts_with(&month) && p.ends_with(" Given Page.md"), "{p}");
    let saved = c.tool("read_note", json!({"path": p}))["content"].as_str().unwrap().to_string();
    assert!(saved.contains("Explains A. Then B.") && saved.contains("Numbats eat termites."), "{saved}");

    let fetched = c.tool("clip_url", json!({"url": format!("{uri}/story")}));
    assert_eq!((fetched["status"].as_str(), fetched["title"].as_str()), (Some("done"), Some("Fetched Story")), "{fetched}");
    let saved = c.tool("read_note", json!({"path": fetched["path"]}))["content"].as_str().unwrap().to_string();
    assert!(saved.contains("The quokka smiles a lot.") && !saved.contains("Menu"), "{saved}");
    assert!(saved.contains("languages:\n- en"), "{saved}");
    assert!(c.tool_error("clip_url", json!({"url": format!("{uri}/file.zip")})).contains("not a web page"));
    assert!(c.tool_error("clip_url", json!({"url": format!("{uri}/missing")})).contains("404"));
    assert!(c.tool_error("clip_url", json!({"url": "ftp://example.com/x", "text": "t"})).contains("http(s)"));

    // wait_seconds 0 answers at once with a job id; clip_status follows it.
    let pending =
        c.tool("clip_url", json!({"url": "https://example.com/later", "title": "Later", "text": "Bilbies dig.", "wait_seconds": 0}));
    let job = pending["job_id"].as_str().unwrap().to_string();
    let mut status = pending;
    for _ in 0..200 {
        if status["status"] != "pending" {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
        status = c.tool("clip_status", json!({"job_id": job}));
    }
    assert_eq!(status["status"], "done", "{status}");
    assert!(status["path"].as_str().unwrap().ends_with(" Later.md"));
    assert!(c.tool_error("clip_status", json!({"job_id": "nope"})).contains("unknown job"));

    let listed: Vec<PathBuf> = core.list_notes().into_iter().map(|n| n.path).collect();
    assert_eq!(listed.len(), 6, "{listed:?}");
    core.shutdown();
}
