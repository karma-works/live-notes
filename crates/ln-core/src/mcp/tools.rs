//! The MCP tools. Each one checks its arguments, calls into [`Core`] and answers with a short text
//! (JSON) plus the same object as `structuredContent`. Failures are tool results with `isError`,
//! so the model sees why; only an unknown tool name is a protocol error.

use crate::clip::render::{host, html_to_text};
use crate::clip::youtube::video_id_from_url;
use crate::{ClipKind, ClipRequest, ClipStatus, Core, CoreEvent, SearchFilter};
use chrono::Local;
use ln_notes::{FrontMatter, Note, atomic_write, sanitize_title, unique_path};
use serde_json::{Value, json};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

const DEFAULT_LIMIT: u64 = 10;
const MAX_LIMIT: u64 = 50;
/// `read_note` cuts longer notes (a day-long transcript) and says so.
pub(crate) const MAX_READ_CHARS: usize = 200_000;
const MAX_NOTE_BYTES: usize = 2 * 1024 * 1024;
/// How long `clip_url` waits for the note before answering with the job id.
const DEFAULT_WAIT_S: u64 = 90;
const MAX_WAIT_S: u64 = 600;
/// Pages fetched for `clip_url`.
const FETCH_LIMIT: usize = 8 * 1024 * 1024;
const FETCH_TIMEOUT: Duration = Duration::from_secs(30);

pub(crate) fn list() -> Value {
    json!([
        {
            "name": "search_notes",
            "title": "Search notes",
            "description": "Full-text and semantic search over the user's Live Notes notes (and other folders they indexed in the app). Returns the best matching files with a snippet each; use read_note with a result's path to read it in full.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {"type": "string", "description": "Words or a question, in any language."},
                    "limit": {"type": "integer", "minimum": 1, "maximum": MAX_LIMIT, "description": "Number of files (default 10)."},
                    "folder": {"type": "string", "description": "Only notes in this folder, relative to the notes folder, e.g. \"Web\" or \"YouTube\"."}
                },
                "required": ["query"]
            },
            "annotations": {"readOnlyHint": true, "openWorldHint": false}
        },
        {
            "name": "read_note",
            "title": "Read note",
            "description": "Returns the full Markdown of one note (front matter included). The path is relative to the notes folder, as search_notes and create_note return it.",
            "inputSchema": {
                "type": "object",
                "properties": {"path": {"type": "string", "description": "e.g. \"2026/11/2026-11-26 1430 Some article.md\""}},
                "required": ["path"]
            },
            "annotations": {"readOnlyHint": true, "openWorldHint": false}
        },
        {
            "name": "create_note",
            "title": "Create note",
            "description": "Writes a new Markdown note into the notes folder. The file is named after the title; an existing note is never overwritten. Returns the new note's path.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "title": {"type": "string", "description": "Becomes the # heading and the file name."},
                    "content": {"type": "string", "description": "Markdown body below the heading."},
                    "folder": {"type": "string", "description": "Folder relative to the notes folder; created if missing. Default: top level."}
                },
                "required": ["title"]
            },
            "annotations": {"readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": false}
        },
        {
            "name": "clip_url",
            "title": "Save web page or YouTube video",
            "description": "Saves a web page or YouTube video as a summarized note, like the Live Notes browser extension (notes land next to recordings in YYYY/MM/, labelled Web or YouTube). Videos are summarized from their captions, or transcribed locally. Pass text when you already have the page content, otherwise Live Notes downloads the page. Waits up to wait_seconds for the note; if it is not ready by then, the answer has a job_id for clip_status.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "url": {"type": "string", "description": "http(s) URL of a page or a YouTube video."},
                    "title": {"type": "string"},
                    "text": {"type": "string", "description": "Page text to save instead of downloading the page."},
                    "lang": {"type": "string", "description": "Language of the content, e.g. \"de\"."},
                    "wait_seconds": {"type": "integer", "minimum": 0, "maximum": MAX_WAIT_S, "description": "Default 90."}
                },
                "required": ["url"]
            },
            "annotations": {"readOnlyHint": false, "destructiveHint": false, "idempotentHint": false, "openWorldHint": true}
        },
        {
            "name": "clip_status",
            "title": "Clip status",
            "description": "State of a clip_url job that was still running: pending (with stage), done (with path) or error.",
            "inputSchema": {
                "type": "object",
                "properties": {"job_id": {"type": "string"}},
                "required": ["job_id"]
            },
            "annotations": {"readOnlyHint": true, "openWorldHint": false}
        }
    ])
}

/// `None` for an unknown tool.
pub(crate) async fn call(core: &Arc<Core>, name: &str, args: Value) -> Option<Value> {
    let result = match name {
        "search_notes" => blocking(core, args, search_notes).await,
        "read_note" => blocking(core, args, read_note).await,
        "create_note" => blocking(core, args, create_note).await,
        "clip_url" => clip_url(core, &args).await,
        "clip_status" => clip_status(core, &args),
        _ => return None,
    };
    Some(match result {
        Ok(v) => json!({"content": [{"type": "text", "text": v.to_string()}], "structuredContent": v, "isError": false}),
        Err(e) => json!({"content": [{"type": "text", "text": e}], "isError": true}),
    })
}

/// Index, embedding model and files block; keep them off the async runtime.
async fn blocking(core: &Arc<Core>, args: Value, f: fn(&Core, &Value) -> Result<Value, String>) -> Result<Value, String> {
    let core = core.clone();
    tokio::task::spawn_blocking(move || f(&core, &args)).await.map_err(|e| e.to_string())?
}

fn str_arg<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty())
}

fn required<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    str_arg(args, key).ok_or_else(|| format!("`{key}` is required"))
}

/// A note's path as the tools show it: relative to the notes folder, `/` separated.
fn display_path(root: &Path, p: &Path) -> (String, bool) {
    match p.strip_prefix(root) {
        Ok(rel) => (rel.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/"), true),
        Err(_) => (p.to_string_lossy().into_owned(), false),
    }
}

/// A folder argument: relative, without `..` or hidden parts.
fn safe_folder(folder: &str) -> Result<PathBuf, String> {
    let p = Path::new(folder.trim_matches('/'));
    let ok = p.components().all(|c| matches!(c, Component::Normal(s) if !s.to_string_lossy().starts_with('.')));
    if !ok {
        return Err(format!("folder must be relative to the notes folder: {folder}"));
    }
    Ok(p.to_path_buf())
}

fn search_notes(core: &Core, args: &Value) -> Result<Value, String> {
    let query = required(args, "query")?;
    let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT) as usize;
    let root = core.notes_dir();
    let mut filter = SearchFilter::default();
    if let Some(f) = str_arg(args, "folder") {
        filter.path_prefix = Some(root.join(safe_folder(f)?));
    }
    let hits = core.search_files(query, &filter, limit);
    let results: Vec<Value> = hits
        .into_iter()
        .map(|h| {
            let (path, in_notes) = display_path(&root, &h.path);
            json!({"path": path, "title": h.title, "snippet": h.snippet, "in_notes_folder": in_notes})
        })
        .collect();
    Ok(json!({"query": query, "results": results}))
}

/// The file a `path` argument names; it must be a Markdown file inside the notes folder.
pub(crate) fn resolve_note(root: &Path, path: &str) -> Result<PathBuf, String> {
    let given = Path::new(path.trim());
    let joined = if given.is_absolute() { given.to_path_buf() } else { root.join(given) };
    let not_found = || format!("no note at {path}");
    let real = joined.canonicalize().map_err(|_| not_found())?;
    let real_root = root.canonicalize().map_err(|e| e.to_string())?;
    if !real.starts_with(&real_root) {
        return Err(format!("{path} is outside the notes folder"));
    }
    if !real.is_file() || !real.extension().is_some_and(|e| e.eq_ignore_ascii_case("md")) {
        return Err(not_found());
    }
    Ok(real)
}

fn read_note(core: &Core, args: &Value) -> Result<Value, String> {
    let path = required(args, "path")?;
    let root = core.notes_dir();
    let file = resolve_note(&root, path)?;
    let text = std::fs::read_to_string(&file).map_err(|e| e.to_string())?;
    let title = Note::parse(&text).ok().and_then(|n| n.title()).filter(|t| !t.trim().is_empty());
    let truncated = text.chars().count() > MAX_READ_CHARS;
    let content: String = if truncated { text.chars().take(MAX_READ_CHARS).collect() } else { text };
    let real_root = root.canonicalize().unwrap_or(root);
    let mut out = json!({"path": display_path(&real_root, &file).0, "title": title, "content": content});
    if truncated {
        out["truncated"] = json!(format!("only the first {MAX_READ_CHARS} characters"));
    }
    Ok(out)
}

fn create_note(core: &Core, args: &Value) -> Result<Value, String> {
    let title = required(args, "title")?.split_whitespace().collect::<Vec<_>>().join(" ");
    let content = args.get("content").and_then(Value::as_str).unwrap_or("").trim();
    if content.len() > MAX_NOTE_BYTES {
        return Err("content is larger than 2 MB".into());
    }
    let root = core.notes_dir();
    let dir = match str_arg(args, "folder") {
        Some(f) => root.join(safe_folder(f)?),
        None => root.clone(),
    };
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let created = Local::now().fixed_offset();
    let name = sanitize_title(&title);
    let name = if name.is_empty() { created.format("%Y-%m-%d %H%M").to_string() } else { name };
    let front = FrontMatter {
        id: Some(ulid::Ulid::new().to_string()),
        title: Some(title.clone()),
        created: Some(created),
        app: Some(format!("live-notes/{} (mcp)", env!("CARGO_PKG_VERSION"))),
        ..Default::default()
    };
    let preamble = if content.is_empty() { format!("# {title}\n") } else { format!("# {title}\n\n{content}\n") };
    let text = Note { front: Some(front), preamble, sections: vec![] }.render();
    let path = unique_path(&dir.join(format!("{name}.md")), None);
    atomic_write(&path, &text).map_err(|e| e.to_string())?;
    tracing::info!("mcp: created {}", path.display());
    core.emit(CoreEvent::NotesChanged);
    core.emit(CoreEvent::NoteFileChanged(path.clone()));
    core.indexer.kick();
    Ok(json!({"path": display_path(&root, &path).0, "title": title}))
}

async fn clip_url(core: &Arc<Core>, args: &Value) -> Result<Value, String> {
    let url = required(args, "url")?.to_string();
    let wait = args.get("wait_seconds").and_then(Value::as_u64).unwrap_or(DEFAULT_WAIT_S).min(MAX_WAIT_S);
    let mut req = ClipRequest {
        kind: ClipKind::Page,
        url: url.clone(),
        title: str_arg(args, "title").map(String::from),
        site_name: None,
        byline: None,
        lang: str_arg(args, "lang").map(String::from),
        text: str_arg(args, "text").map(String::from),
        html: None,
        selection: None,
        youtube: None,
    };
    if video_id_from_url(&url).is_some() {
        // The app fetches captions (or the audio) itself.
        req.kind = ClipKind::Youtube;
        req.text = None;
    } else if req.text.is_none() {
        if !(url.starts_with("https://") || url.starts_with("http://")) {
            return Err("url must be an http(s) URL".into());
        }
        let page = fetch_page(&url).await?;
        req.title = req.title.or(page.title);
        req.lang = req.lang.or(page.lang);
        req.text = Some(page.text).filter(|t| !t.trim().is_empty());
        if req.text.is_none() {
            return Err(format!("{url} has no readable text; pass the text yourself"));
        }
    }
    let job = core.clip(req)?;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(wait);
    loop {
        let status = job_status(core, &job)?;
        if status["status"] != "pending" || tokio::time::Instant::now() >= deadline {
            return finish(status);
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
}

fn clip_status(core: &Arc<Core>, args: &Value) -> Result<Value, String> {
    finish(job_status(core, required(args, "job_id")?)?)
}

fn job_status(core: &Core, id: &str) -> Result<Value, String> {
    let job = core.clip_job(id).ok_or_else(|| format!("unknown job {id} (jobs are forgotten when the app restarts)"))?;
    let root = core.notes_dir();
    let mut v = json!({"job_id": id, "status": job.status});
    if let Some(s) = job.stage {
        v["stage"] = json!(s);
    }
    if let Some(p) = job.progress {
        v["progress"] = json!(p);
    }
    if let Some(p) = &job.path {
        v["path"] = json!(display_path(&root, p).0);
    }
    if let Some(t) = job.title {
        v["title"] = json!(t);
    }
    if job.status == ClipStatus::Error {
        v["error"] = json!(job.error.unwrap_or_default());
    }
    Ok(v)
}

/// A failed job is a tool error, so the model doesn't take it for a saved note.
fn finish(status: Value) -> Result<Value, String> {
    if status["status"] == "error" {
        return Err(format!("clip failed: {}", status["error"].as_str().unwrap_or("unknown error")));
    }
    Ok(status)
}

pub(crate) struct FetchedPage {
    pub title: Option<String>,
    pub lang: Option<String>,
    pub text: String,
}

async fn fetch_page(url: &str) -> Result<FetchedPage, String> {
    let page = ln_ai::web::fetch_page(url, FETCH_LIMIT, FETCH_TIMEOUT).await?;
    if page.content_type.starts_with("text/") && !page.content_type.contains("html") {
        return Ok(FetchedPage { title: None, lang: None, text: page.body });
    }
    Ok(page_from_html(&page.body, url))
}

/// Title, language and main text of a whole HTML page (the extension sends Readability's
/// result instead): `<article>`, else `<main>`, else `<body>`, without navigation and chrome.
pub(crate) fn page_from_html(html: &str, url: &str) -> FetchedPage {
    let lower = html.to_ascii_lowercase();
    let title =
        inner(html, &lower, "title").map(|t| html_to_text(t).split_whitespace().collect::<Vec<_>>().join(" ")).filter(|t| !t.is_empty());
    let lang = lower.find("<html").and_then(|i| {
        let tag = &lower[i..lower[i..].find('>').map_or(lower.len(), |j| i + j)];
        let at = tag.find("lang=")? + 5;
        let v = tag[at..].trim_start_matches(['"', '\'']);
        Some(v.split(['"', '\'', ' ', '>']).next()?.to_string()).filter(|s| !s.is_empty())
    });
    let main = ["article", "main", "body"].iter().find_map(|t| inner(html, &lower, t)).unwrap_or(html);
    let mut cleaned = main.to_string();
    for tag in ["nav", "header", "footer", "aside", "form", "svg", "template", "script", "style", "noscript"] {
        cleaned = drop_elements(&cleaned, tag);
    }
    let text = html_to_text(&cleaned);
    tracing::info!("mcp: fetched {} ({} chars of text)", host(url), text.len());
    FetchedPage { title, lang, text }
}

/// Inner HTML of the first `<tag …>…</tag>`; `lower` is `html` lowercased (same byte offsets).
fn inner<'a>(html: &'a str, lower: &str, tag: &str) -> Option<&'a str> {
    let open = find_open(lower, tag, 0)?;
    let start = open + lower[open..].find('>')? + 1;
    let end = lower[start..].find(&format!("</{tag}")).map_or(html.len(), |e| start + e);
    Some(&html[start..end])
}

/// Position of `<tag` followed by `>` or whitespace (so `<header` isn't `<head`).
fn find_open(lower: &str, tag: &str, from: usize) -> Option<usize> {
    let pat = format!("<{tag}");
    let mut at = from;
    while let Some(i) = lower[at..].find(&pat) {
        let i = at + i;
        match lower.as_bytes().get(i + pat.len()) {
            Some(b) if b.is_ascii_whitespace() || *b == b'>' || *b == b'/' => return Some(i),
            None => return None,
            _ => at = i + pat.len(),
        }
    }
    None
}

/// Removes every `<tag>…</tag>` (not nested-aware; good enough for page chrome).
fn drop_elements(html: &str, tag: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let close = format!("</{tag}>");
    let mut out = String::with_capacity(html.len());
    let mut at = 0;
    while let Some(i) = find_open(&lower, tag, at) {
        out.push_str(&html[at..i]);
        at = match lower[i..].find(&close) {
            Some(e) => i + e + close.len(),
            None => html.len(),
        };
    }
    out.push_str(&html[at..]);
    out
}
