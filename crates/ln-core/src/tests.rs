//! End-to-end tests of the engine with files instead of devices and a mock model server.

use super::*;
use ln_platform::MemoryStore;
use ln_types::{Provider, ProviderKind, TranscriptionMode};
use serde_json::json;
use std::time::Duration;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn tone_wav(path: &std::path::Path, parts: &[(u32, f32)]) {
    let spec = hound::WavSpec { channels: 1, sample_rate: 16_000, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    let mut i = 0usize;
    for (ms, amp) in parts {
        for _ in 0..(*ms as usize * 16) {
            w.write_sample(((i as f32 * 0.07).sin() * amp * 32000.0) as i16).unwrap();
            i += 1;
        }
    }
    w.finalize().unwrap();
}

async fn mock_server() -> MockServer {
    let s = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/audio/transcriptions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"text": "我們下週再討論預算.", "languages": ["zh"]})))
        .mount(&s)
        .await;
    let summary = json!({"title": "Budget nächste Woche", "summary": ["Budget wird nächste Woche besprochen"], "decisions": [],
        "tasks": [{"text": "Angebot schicken", "assignee": "Li", "due": "Freitag"}], "note_additions": ["Li schickt das Angebot bis Freitag"]});
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({"model": "m", "choices": [{"message": {"content": summary.to_string()}}],
            "usage": {"prompt_tokens": 1000, "completion_tokens": 100}})))
        .mount(&s)
        .await;
    s
}

fn core_with(uri: &str) -> (tempfile::TempDir, Arc<Core>, EventReceiver) {
    let dir = tempfile::tempdir().unwrap();
    let paths = AppPaths::in_dir(dir.path());
    let secrets = Arc::new(MemoryStore::default());
    secrets.set("openai", "test-key").unwrap();
    secrets.set("openrouter", "test-key").unwrap();
    let (core, rx) = Core::start(paths, secrets).unwrap();
    let uri = uri.to_string();
    core.update_settings(|s| {
        s.first_run_done = true;
        for p in s.providers.iter_mut() {
            if p.id == "openai" || p.id == "openrouter" {
                p.base_url = uri.clone();
            }
        }
        s.providers.push(Provider { id: "mock".into(), name: "Mock".into(), kind: ProviderKind::Custom, base_url: uri.clone(), headers: vec![], is_cloud: true });
        s.transcription.mode = TranscriptionMode::OpenAiCheap;
        s.live_suggestions.enabled = false;
        // No embedding server in tests: full-text search only.
        s.roles.embeddings.provider = "none".into();
    });
    (dir, core, rx)
}

#[test]
fn recording_from_files_to_summarised_note() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let server = rt.block_on(mock_server());
    let (dir, core, _rx) = core_with(&server.uri());
    let me = dir.path().join("me.wav");
    let others = dir.path().join("others.wav");
    tone_wav(&me, &[(500, 0.0), (1500, 0.3), (800, 0.0)]);
    tone_wav(&others, &[(2800, 0.0), (1500, 0.4), (800, 0.0)]);
    let cfg = CaptureConfig { me: Source::File { path: me, realtime: false }, others: Source::File { path: others, realtime: false } };
    let note = core.start_recording(Some(cfg)).unwrap();
    assert!(core.recording_active());
    core.set_recording_notes("- Budget klären");
    assert!(core.wait_recording_files_done(Duration::from_secs(10)));
    std::thread::sleep(Duration::from_millis(300));
    let saved = core.stop_recording_blocking().unwrap();
    assert_eq!(saved, note);
    let text = std::fs::read_to_string(&note).unwrap();
    assert!(text.contains("<!-- ln:transcript -->"), "{text}");
    assert!(text.contains("我们下周再讨论预算。"), "converted to simplified: {text}");
    assert!(text.contains("- Budget klären"));
    assert!(text.contains("transcription: openai/gpt-transcribe"));
    let parsed = ln_notes::Note::parse(&text).unwrap();
    let lines = parsed.transcript_lines();
    assert_eq!(lines.len(), 2, "{text}");
    assert!(lines[0].at_ms < lines[1].at_ms);
    assert!(core.note_cost(&parsed.front.as_ref().unwrap().id.clone().unwrap()) > 0.0, "cloud audio is billed");

    // Summary (normally started by stop_recording in the background).
    crate::summary::summarize(&core, &note);
    let notes = core.list_notes();
    assert_eq!(notes.len(), 1);
    let renamed = &notes[0].path;
    assert!(renamed.to_string_lossy().ends_with("Budget nächste Woche.md"), "{}", renamed.display());
    let text = std::fs::read_to_string(renamed).unwrap();
    assert!(text.contains("- [ ] Angebot schicken — Li, Freitag"));
    let pos_summary = text.find("<!-- ln:summary -->").unwrap();
    assert!(pos_summary < text.find("<!-- ln:transcript -->").unwrap());
    // Additions stay out of the file until accepted.
    assert!(!text.contains("Li schickt das Angebot"));
    assert_eq!(core.pending_additions(renamed).len(), 1);
    core.accept_addition(renamed, 0).unwrap();
    assert!(std::fs::read_to_string(renamed).unwrap().contains("- Li schickt das Angebot bis Freitag"));
    assert!(core.pending_additions(renamed).is_empty());
    core.shutdown();
}

#[test]
fn notes_search_and_ask() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let server = rt.block_on(async {
        let s = MockServer::start().await;
        let sse = "data: {\"choices\":[{\"delta\":{\"content\":\"Bis Ende Q1 [1].\"}}]}\n\ndata: [DONE]\n\n";
        Mock::given(method("POST")).and(path("/chat/completions")).respond_with(ResponseTemplate::new(200).set_body_raw(sse, "text/event-stream")).mount(&s).await;
        s
    });
    let (_dir, core, mut rx) = core_with(&server.uri());
    let p = core.create_note(None).unwrap();
    std::fs::write(&p, "# Angebot Shenzhen\n\nDer Preis gilt bis Ende Q1.\n- [ ] Muster bestellen\n").unwrap();
    let renamed = core.auto_name_note(&p).unwrap();
    assert!(renamed.ends_with("Angebot Shenzhen.md"));
    core.toggle_task(&renamed, 3).unwrap();
    assert!(std::fs::read_to_string(&renamed).unwrap().contains("- [x] Muster bestellen"));
    let folder = core.create_folder("Projekte").unwrap();
    let moved = core.move_note(&renamed, "Projekte").unwrap();
    assert!(moved.starts_with(&folder));
    assert_eq!(core.note_folders(), vec!["Projekte".to_string()]);

    // Wait for the background indexer to pick up the note.
    let mut hits = vec![];
    for _ in 0..100 {
        core.indexer.resync_all();
        std::thread::sleep(Duration::from_millis(100));
        hits = core.search_files("Preis", &SearchFilter::default(), 5);
        if !hits.is_empty() {
            break;
        }
    }
    assert_eq!(hits.len(), 1, "note is searchable");
    assert_eq!(hits[0].title, "Angebot Shenzhen");

    let id = core.ask("Bis wann gilt der Preis?", AskScope::All, vec![]);
    let answer = rt.block_on(async {
        use futures::StreamExt;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            let ev = tokio::time::timeout_at(deadline, rx.next()).await.expect("answer in time").unwrap();
            if let CoreEvent::Answer { id: i, text, done: true, citations } = ev
                && i == id
            {
                break (text, citations);
            }
        }
    });
    assert_eq!(answer.0, "Bis Ende Q1 [1].");
    assert_eq!(answer.1[0].title, "Angebot Shenzhen");
    core.shutdown();
}

#[test]
fn missing_key_gives_guidance_and_first_run_plan() {
    let dir = tempfile::tempdir().unwrap();
    let (core, _rx) = Core::start(AppPaths::in_dir(dir.path()), Arc::new(MemoryStore::default())).unwrap();
    assert!(!core.assistant_ready());
    let plan: Vec<_> = core.first_run_plan(false).iter().map(|m| m.id).collect();
    assert_eq!(plan, vec!["whisper-large-v3-turbo-q5_0", "whisper-base-q8_0", "silero-vad-v5", "embeddinggemma-300m"]);
    assert_eq!(core.first_run_plan(true).len(), 5);
    core.update_settings(|s| s.transcription.mode = TranscriptionMode::OpenAiCheap);
    let err = core.start_recording(None).unwrap_err().to_string();
    assert!(err.contains("OpenAI"), "{err}");
    core.shutdown();
}
