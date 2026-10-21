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
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"model": "m", "choices": [{"message": {"content": summary.to_string()}}],
            "usage": {"prompt_tokens": 1000, "completion_tokens": 100}})),
        )
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
        s.providers.push(Provider {
            id: "mock".into(),
            name: "Mock".into(),
            kind: ProviderKind::Custom,
            base_url: uri.clone(),
            headers: vec![],
            is_cloud: true,
        });
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
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(sse, "text/event-stream"))
            .mount(&s)
            .await;
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
    // Cost limit: once reached, cloud roles pause.
    core.costs.record_audio("openai", "gpt-transcribe", None, 60.0 * 60.0 * 30.0);
    core.update_settings(|s| s.costs.monthly_limit_usd = 1.0);
    assert!(core.cost_limit_reached());
    assert!(matches!(core.assistant(), Err(crate::ai::Unavailable::CostLimit)));
    core.shutdown();
}

#[test]
fn summary_on_chatgpt_plan_with_its_own_role() {
    use ln_types::{ChatRole, RoleConfig};
    use wiremock::matchers::{body_partial_json, body_string_contains, header};
    let rt = tokio::runtime::Runtime::new().unwrap();
    let server = rt.block_on(async {
        let s = MockServer::start().await;
        let summary =
            json!({"title": "Plan", "summary": ["Über das Abo zusammengefasst"], "decisions": [], "tasks": [], "note_additions": []});
        let delta = json!({"type": "response.output_text.delta", "delta": summary.to_string()});
        let sse = format!(
            "data: {delta}\n\ndata: {}\n\n",
            json!({"type": "response.completed", "response": {"usage": {"input_tokens": 50, "output_tokens": 20}}})
        );
        Mock::given(method("POST"))
            .and(path("/responses"))
            .and(header("authorization", "Bearer at-new"))
            .and(body_partial_json(json!({"model": "gpt-plan", "store": false, "stream": true})))
            .respond_with(ResponseTemplate::new(200).set_body_raw(sse, "text/event-stream"))
            .mount(&s)
            .await;
        Mock::given(method("POST"))
            .and(path("/token"))
            .and(body_string_contains("grant_type=refresh_token"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"access_token": "at-new", "refresh_token": "rt-2", "expires_in": 3600})),
            )
            .mount(&s)
            .await;
        s
    });
    let (dir, core, _rx) = core_with(&server.uri());
    // Signed out: the checklist asks for the sign-in, the summary role is unavailable.
    core.update_settings(|s| {
        s.roles.set(ChatRole::Summary, RoleConfig { provider: "chatgpt".into(), model: "gpt-plan".into() });
        for p in s.providers.iter_mut() {
            if p.id == "chatgpt" {
                p.base_url = server.uri();
            }
        }
    });
    assert!(core.setup_steps().contains(&SetupStep::ChatGptSignIn));
    assert!(matches!(core.role(ChatRole::Summary), Err(crate::ai::Unavailable::ChatGptSignedOut)));
    assert!(core.role(ChatRole::Assistant).is_ok(), "questions keep their own model");
    // Signed in with an expiring token: it is refreshed first, and the cost limit does not apply.
    core.chatgpt.endpoints.lock().token = format!("{}/token", server.uri());
    core.secrets.set("chatgpt", &crate::chatgpt::test_credentials(1, Some("rt-1"))).unwrap();
    *core.chatgpt.stored.lock() = None;
    core.costs.record_audio("openai", "gpt-transcribe", None, 60.0 * 60.0 * 30.0);
    core.update_settings(|s| s.costs.monthly_limit_usd = 1.0);
    assert!(!core.setup_steps().contains(&SetupStep::ChatGptSignIn));
    assert!(matches!(core.chatgpt_status(), ChatGptStatus::SignedIn { plan_usage: true, .. }));
    assert!(matches!(core.role(ChatRole::Assistant), Err(crate::ai::Unavailable::CostLimit)));
    let note = dir.path().join("n.md");
    std::fs::write(&note, "---\nid: x\n---\n\n<!-- ln:transcript -->\n## Transkript\n[00:00:01] Ich (de): Wir planen das Budget.\n")
        .unwrap();
    crate::summary::summarize(&core, &note);
    let text = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|x| x == "md"))
        .map(|e| std::fs::read_to_string(e.path()).unwrap())
        .collect::<String>();
    assert!(text.contains("Über das Abo zusammengefasst"), "{text}");
    let saved = core.secrets.get("chatgpt").unwrap();
    assert!(saved.contains("at-new") && saved.contains("rt-2"), "rotated tokens are stored");
    core.shutdown();
}

#[test]
fn overview_names_model_place_and_cost_per_task() {
    use crate::{UseCost, UseState, UseTask};
    use ln_types::{ChatRole, RoleConfig};
    let dir = tempfile::tempdir().unwrap();
    let (core, _rx) = Core::start(AppPaths::in_dir(dir.path()), Arc::new(MemoryStore::default())).unwrap();
    core.update_settings(|s| s.roles.set(ChatRole::Summary, RoleConfig { provider: "chatgpt".into(), model: "gpt-plan".into() }));
    let o = core.model_overview();
    let get = |t: UseTask| o.iter().find(|u| u.task == t).cloned().unwrap();
    assert_eq!(get(UseTask::Transcription).cost, UseCost::Local);
    assert!(get(UseTask::Transcription).model.contains("Whisper"), "{:?}", get(UseTask::Transcription));
    let q = get(UseTask::Questions);
    assert_eq!(q.cost, UseCost::PerToken { input: 0.09, output: 0.30 });
    assert_eq!(q.state, UseState::KeyMissing);
    assert_eq!(q.provider, "OpenRouter");
    let sum = get(UseTask::Summary);
    assert_eq!((sum.cost, sum.state), (UseCost::Plan, UseState::SignInMissing));
    core.update_settings(|s| {
        s.transcription.mode = TranscriptionMode::OpenAiCheap;
        s.live_suggestions.enabled = false;
    });
    let o = core.model_overview();
    let t = o.iter().find(|u| u.task == UseTask::Transcription).unwrap();
    assert_eq!(t.cost, UseCost::PerMinute(0.0045));
    assert!(o.iter().all(|u| u.task != UseTask::LanguageDetection), "OpenAI detects the language itself");
    assert_eq!(o.iter().find(|u| u.task == UseTask::Suggestions).unwrap().state, UseState::Off);
    core.shutdown();
}

#[test]
fn key_hint_names_roles_and_chatgpt_takes_over_unready_ones() {
    use ln_types::{ChatRole, RoleConfig};
    let dir = tempfile::tempdir().unwrap();
    let secrets = Arc::new(MemoryStore::default());
    let (core, _rx) = Core::start(AppPaths::in_dir(dir.path()), secrets.clone()).unwrap();
    core.update_settings(|s| s.roles.set(ChatRole::Summary, RoleConfig { provider: "chatgpt".into(), model: "gpt-plan".into() }));
    secrets.set("chatgpt", &crate::chatgpt::test_credentials(60, Some("rt"))).unwrap();
    *core.chatgpt.stored.lock() = None;
    let steps = core.setup_steps();
    let key = steps.iter().find_map(|s| match s {
        SetupStep::KeyMissing { provider, roles } => Some((provider.clone(), roles.clone())),
        _ => None,
    });
    assert_eq!(key, Some(("OpenRouter".into(), vec![ChatRole::Assistant, ChatRole::Suggestions])), "{steps:?}");
    assert!(!steps.contains(&SetupStep::ChatGptSignIn));
    let moved = core.use_chatgpt_for_unready_roles();
    assert_eq!(moved, vec![ChatRole::Assistant, ChatRole::Suggestions]);
    let s = core.settings();
    assert_eq!(s.roles.get(ChatRole::Assistant), &RoleConfig { provider: "chatgpt".into(), model: "gpt-plan".into() });
    assert!(core.setup_steps().iter().all(|s| !matches!(s, SetupStep::KeyMissing { .. })));
    core.shutdown();
}

#[test]
fn stop_is_instant_and_a_new_recording_can_start_while_the_last_one_finishes() {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let server = rt.block_on(async {
        let s = MockServer::start().await;
        // Slow transcription: the queue is still busy when the user stops.
        Mock::given(method("POST"))
            .and(path("/audio/transcriptions"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_delay(Duration::from_millis(700))
                    .set_body_json(json!({"text": "Das Budget steht.", "languages": ["de"]})),
            )
            .mount(&s)
            .await;
        s
    });
    let (dir, core, _rx) = core_with(&server.uri());
    core.update_settings(|s| s.notes.auto_summary = false);
    let me = dir.path().join("me.wav");
    let others = dir.path().join("others.wav");
    tone_wav(&me, &[(300, 0.0), (1200, 0.3), (600, 0.0), (1200, 0.3), (600, 0.0)]);
    tone_wav(&others, &[(4000, 0.0)]);
    let cfg = || CaptureConfig {
        me: Source::File { path: me.clone(), realtime: false },
        others: Source::File { path: others.clone(), realtime: false },
    };
    let t0 = std::time::Instant::now();
    let first = core.start_recording(Some(cfg())).unwrap();
    assert!(t0.elapsed() < Duration::from_millis(300), "start returns at once: {:?}", t0.elapsed());
    assert!(core.wait_recording_files_done(Duration::from_secs(10)));
    let t1 = std::time::Instant::now();
    assert_eq!(core.stop_recording(), Some(first.clone()));
    assert!(t1.elapsed() < Duration::from_millis(100), "stop returns at once: {:?}", t1.elapsed());
    assert!(!core.recording_active());
    assert_eq!(core.activity(&first), Some(crate::Activity::Finishing));
    // A second recording while the first is still being transcribed.
    std::thread::sleep(Duration::from_millis(1100));
    let second = core.start_recording(Some(cfg())).unwrap();
    assert_ne!(first, second);
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while core.activity(&first).is_some() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(core.activity(&first), None);
    let text = std::fs::read_to_string(&first).unwrap();
    let lines = ln_notes::Note::parse(&text).unwrap().transcript_lines().len();
    assert_eq!(lines, 2, "both sections of the first recording are in its note: {text}");
    assert!(core.wait_recording_files_done(Duration::from_secs(10)));
    assert_eq!(core.stop_recording_blocking(), Some(second.clone()));
    let text2 = std::fs::read_to_string(&second).unwrap();
    assert_eq!(ln_notes::Note::parse(&text2).unwrap().transcript_lines().len(), 2, "{text2}");
    core.shutdown();
}

#[test]
fn cost_estimates_per_hour() {
    use crate::UseTask;
    let dir = tempfile::tempdir().unwrap();
    let (core, _rx) = Core::start(AppPaths::in_dir(dir.path()), Arc::new(MemoryStore::default())).unwrap();
    let o = core.model_overview();
    let get = |t: UseTask| o.iter().find(|u| u.task == t).cloned().unwrap();
    // Gemma 4 26B-A4B on OpenRouter: 14k in + 1k out ≈ 0.0016 $ per summary.
    let sum = get(UseTask::Summary).per_hour.unwrap();
    assert!((sum - 0.00156).abs() < 0.0001, "{sum}");
    let sugg = get(UseTask::Suggestions).per_hour.unwrap();
    assert!(sugg > 0.003 && sugg < 0.03, "{sugg}");
    assert!(get(UseTask::Questions).per_question.unwrap() > 0.0);
    assert_eq!(get(UseTask::Transcription).per_hour, Some(0.0));
    let total = core.hourly_estimate().unwrap();
    assert!((total - (sum + sugg)).abs() < 1e-9);
    core.update_settings(|s| s.transcription.mode = TranscriptionMode::OpenAiCheap);
    let t = core.model_overview().into_iter().find(|u| u.task == UseTask::Transcription).unwrap();
    assert!((t.per_hour.unwrap() - 0.216).abs() < 1e-9, "0.0045 $/min × 60 × 0.8");
    core.shutdown();
}

#[test]
fn notes_are_sorted_into_categories_with_one_request() {
    use ln_types::{Category, DecisionProvider, TagColor};
    use wiremock::matchers::body_partial_json;
    let rt = tokio::runtime::Runtime::new().unwrap();
    let server = rt.block_on(async {
        let s = MockServer::start().await;
        // OpenRouter (preferred when both keys are set): one request, one question per category.
        Mock::given(method("POST"))
            .and(path("/alpha/decisions"))
            .and(body_partial_json(json!({"model": "typesafe/jev-1.14",
                "questions": {"c0": {"type": "noul"}, "c1": {"type": "noul"}, "c2": {"type": "noul"}}})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "answers": {"c0": {"type": "noul", "noul": 0.93}, "c1": {"type": "noul", "noul": 0.08}, "c2": {"type": "noul", "noul": 0.61}},
                "usage": {"input_tokens": 90, "output_tokens": 0, "cost": 0.0000038}})))
            .expect(1)
            .mount(&s)
            .await;
        Mock::given(method("POST"))
            .and(path("/decisions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "answers": [{"name": "c1", "type": "predicate", "probability": 0.8}], "usage": {"input_tokens": 90}})))
            .expect(1)
            .mount(&s)
            .await;
        s
    });
    let (_dir, core, _rx) = core_with(&server.uri());
    let cat = |n: &str, c| Category { name: n.into(), color: c, hint: String::new() };
    core.update_settings(|s| {
        s.categories.list = vec![
            cat("Chinesisch-Kurs", TagColor::Red),
            cat("Arztagent", TagColor::Blue),
            Category { name: "KI-Projekte".into(), color: TagColor::Green, hint: "neue KI-Projekte".into() },
        ];
    });
    let p = core.create_note(None).unwrap();
    std::fs::write(&p, "# Lektion 4\n\n- 声调 üben\n- Idee: Vokabeltrainer mit KI\n").unwrap();
    let got = core.classify_note_now(&p, false).unwrap().unwrap();
    assert_eq!(got, vec!["Chinesisch-Kurs".to_string(), "KI-Projekte".to_string()]);
    let text = std::fs::read_to_string(&p).unwrap();
    assert!(text.starts_with("---\ncategories:\n- Chinesisch-Kurs\n- KI-Projekte\n---\n# Lektion 4\n"), "{text}");
    // Unchanged content: no second request.
    assert_eq!(core.classify_note_now(&p, false).unwrap(), None);
    // The note list carries the categories; a note with category front matter is still auto-named.
    let renamed = core.auto_name_note(&p).unwrap();
    assert!(renamed.ends_with("Lektion 4.md"));
    let entry = core.list_notes().into_iter().find(|n| n.path == renamed).unwrap();
    assert_eq!(entry.categories.len(), 2);
    assert!(core.month_cost() > 0.0, "decision call is billed");

    // OpenAI's Decisions API when chosen; a hand-typed category outside the list stays.
    core.update_settings(|s| s.categories.provider = DecisionProvider::OpenAi);
    std::fs::write(&renamed, "---\ncategories:\n- Privat\n---\n# Lektion 4\n\nAnforderung: Termine per Telefon\n").unwrap();
    let got = core.classify_note_now(&renamed, false).unwrap().unwrap();
    assert_eq!(got, vec!["Arztagent".to_string(), "Privat".to_string()]);
    assert_eq!(core.decisions_status().unwrap(), "OpenAI · gpt-6-luna");

    // Renaming by hand changes the name in the list.
    let moved = core.rename_note(&renamed, "Vokabeln Lektion 4").unwrap();
    assert!(moved.ends_with("Vokabeln Lektion 4.md"));
    assert_eq!(core.list_notes().into_iter().find(|n| n.path == moved).unwrap().title, "Vokabeln Lektion 4");
    core.shutdown();
}
