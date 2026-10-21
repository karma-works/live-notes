//! Verification tour (feature `screenshots`), used by the `verify` skill:
//! drives the real app like a user (menu actions are dispatched, not called), checks outcomes,
//! renders every view to `$LN_SCREENSHOT_DIR/*.png` and writes `tour-report.json` there.
//! The process exits with 1 if any check failed.
//!
//! - `LN_DEMO_AUDIO_ME` / `LN_DEMO_AUDIO_OTHERS` (WAV): recording from files instead of devices.
//! - `LN_TOUR_REAL_AUDIO` (WAV): additionally record with the real microphone and system audio
//!   while `afplay` plays this file (system audio must transcribe it).

use crate::app::AppView;
use gpui_kit::*;
use ln_core::Core;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

const MANUAL: &str = "---\ncategories:\n- Angebote\n---\n# Angebot Shenzhen\n\nVergleich der zwei Angebote vor dem Gespräch mit Frau Li am Freitag.\n\n## Offene Punkte\n\n- [x] Muster bestellen\n- [ ] Lieferzeit klären (交货时间)\n- [ ] Zahlungsziel 60 Tage verhandeln\n\n## Preise\n\n| Anbieter | Stückpreis | MOQ |\n|---|---|---|\n| A | 3,20 $ | 1.000 |\n| B | 2,90 $ | 5.000 |\n\n> B ist günstiger, aber die Mindestmenge ist zu hoch für Q1.\n\n```text\nGesamt A = 1.000 × 3,20 = 3.200 $\n```\n";

const RECORDED: &str = "---\nid: 01JB8Z4K7Q2W3E4R5T6Y7U8I9O\ntitle: Teamrunde Budget\ncreated: 2026-10-20T14:30:12+02:00\nduration: 00:41:07\nlanguages:\n- zh\n- de\ntranscription: local/whisper-large-v3-turbo\nspeakers:\n  others: Frau Li\ncategories:\n- Teamrunden\n- Angebote\n---\n\n<!-- ln:summary -->\n## Zusammenfassung\n- Das Budget für Q1 wird nächste Woche final besprochen\n- Frau Li schickt das aktualisierte Angebot bis Freitag\n\n<!-- ln:tasks -->\n## Aufgaben\n- [ ] Angebot schicken — Frau Li, Freitag\n- [ ] Budgetentwurf vorbereiten — Ich\n\n<!-- ln:notes -->\n## Meine Notizen\n- Budget Q1 klären\n- Termin mit Li\n\n<!-- ln:transcript -->\n## Transkript\n[00:00:03] Ich (de): Hallo zusammen, können wir das Budget bis Freitag festzurren?\n[00:00:11] Frau Li (zh): 我们下周再讨论预算，我周五前把报价发给你。\n[00:00:19] Ich (de): Gut, dann machen wir das so.\n[00:00:24] Frau Li (en): Perfect, I will also send the delivery schedule.\n";

fn seed(notes: &Path) {
    let _ = std::fs::create_dir_all(notes.join("2026/10"));
    let _ = std::fs::create_dir_all(notes.join("Projekte"));
    let a = notes.join("Projekte/Angebot Shenzhen.md");
    if !a.exists() {
        let _ = std::fs::write(&a, MANUAL);
    }
    let b = notes.join("2026/10/2026-10-20 1430 Teamrunde Budget.md");
    if !b.exists() {
        let _ = std::fs::write(&b, RECORDED);
    }
    let c = notes.join("Einkauf.md");
    if !c.exists() {
        let _ = std::fs::write(&c, "# Einkauf\n\n- [ ] Milch\n- [ ] Brot\n");
    }
}

async fn wait(cx: &mut AsyncApp, ms: u64) {
    cx.background_executor().timer(Duration::from_millis(ms)).await;
}

fn shot(cx: &mut AsyncApp, handle: AnyWindowHandle, dir: &Path, name: &str) {
    let r = cx.update_window(handle, |_, window, _| window.render_to_image());
    match r {
        Ok(Ok(img)) => {
            let p = dir.join(format!("{name}.png"));
            if let Err(e) = img.save(&p) {
                eprintln!("tour: saving {}: {e}", p.display());
            } else {
                eprintln!("tour: {}", p.display());
            }
        }
        Ok(Err(e)) => eprintln!("tour: render {name}: {e}"),
        Err(e) => eprintln!("tour: window {name}: {e}"),
    }
}

fn redraw(cx: &mut AsyncApp, view: &Entity<AppView>) {
    view.update(cx, |_, cx| cx.notify());
}

struct Report {
    checks: Vec<(String, bool, String)>,
}

impl Report {
    fn check(&mut self, name: &str, ok: bool, detail: impl Into<String>) {
        let detail = detail.into();
        eprintln!("tour: [{}] {name} {detail}", if ok { "ok" } else { "FAIL" });
        self.checks.push((name.into(), ok, detail));
    }
    fn write(&self, dir: &Path) -> bool {
        let all = self.checks.iter().all(|c| c.1);
        let items: Vec<String> =
            self.checks.iter().map(|(n, ok, d)| format!("  {{\"check\": {:?}, \"ok\": {}, \"detail\": {:?}}}", n, ok, d)).collect();
        let json = format!("{{\n\"passed\": {all},\n\"checks\": [\n{}\n]\n}}\n", items.join(",\n"));
        let _ = std::fs::write(dir.join("tour-report.json"), json);
        all
    }
}

fn dispatch(cx: &mut AsyncApp, handle: AnyWindowHandle, action: impl Action) {
    let _ = cx.update_window(handle, |_, window, cx| window.dispatch_action(action.boxed_clone(), cx));
}

/// Longest gap between two UI-thread ticks (ms) since the last reset: a frozen UI shows up here.
fn heartbeat(cx: &mut AsyncApp) -> std::rc::Rc<std::cell::Cell<u128>> {
    let max = std::rc::Rc::new(std::cell::Cell::new(0u128));
    let m = max.clone();
    cx.spawn(async move |cx: &mut AsyncApp| {
        let mut last = std::time::Instant::now();
        loop {
            cx.background_executor().timer(std::time::Duration::from_millis(20)).await;
            let gap = last.elapsed().as_millis();
            if gap > 150 {
                tracing::warn!("ui stall {gap} ms");
            }
            if gap > m.get() {
                m.set(gap);
            }
            last = std::time::Instant::now();
        }
    })
    .detach();
    max
}

/// Screenshot of the whole page: the window is made tall for a moment.
async fn tall_shot(cx: &mut AsyncApp, handle: AnyWindowHandle, view: &Entity<AppView>, dir: &std::path::Path, name: &str) {
    let _ = cx.update_window(handle, |_, window, _| window.resize(size(px(1180.), px(2100.))));
    wait(cx, 600).await;
    redraw(cx, view);
    wait(cx, 300).await;
    shot(cx, handle, dir, name);
    let _ = cx.update_window(handle, |_, window, _| window.resize(size(px(1180.), px(760.))));
    wait(cx, 400).await;
}

fn window_count(cx: &mut AsyncApp) -> usize {
    cx.update(|cx| cx.windows().len())
}

pub fn run(core: Arc<Core>, handle: AnyWindowHandle, view: Entity<AppView>, cx: &mut App) {
    let Some(dir) = std::env::var_os("LN_SCREENSHOT_DIR").map(PathBuf::from) else { return };
    let _ = std::fs::create_dir_all(&dir);
    let notes = core.notes_dir();
    cx.spawn(async move |cx: &mut AsyncApp| {
        let mut r = Report { checks: vec![] };
        wait(cx, 1500).await;

        // 1. First run: the screen appears and is completed like a user would (button → event).
        if !core.settings().first_run_done {
            let shown = view.read_with(cx, |v, _| v.first_run_view().is_some());
            r.check("first_run_shown", shown, "");
            shot(cx, handle, &dir, "01-first-run");
            let _ = cx.update_window(handle, |_, _, cx| {
                if let Some(fr) = view.read(cx).first_run_view() {
                    fr.update(cx, |f, cx| f.complete_for_tour(cx));
                }
            });
            wait(cx, 500).await;
            r.check("first_run_closed", view.read_with(cx, |v, _| v.first_run_view().is_none()), "");
        }

        // 2. Menu command right after the first run must reach the view (regression: lost focus).
        //    Settings open as a page in the main window, not in a second window.
        let before = window_count(cx);
        dispatch(cx, handle, crate::OpenSettings);
        wait(cx, 1200).await;
        let page = view.read_with(cx, |v, _| v.page());
        r.check("menu_opens_settings", page == "Settings", page.clone());
        r.check("settings_in_main_window", window_count(cx) == before, format!("windows {before} → {}", window_count(cx)));
        tall_shot(cx, handle, &view, &dir, "07-settings").await;

        // 3. Setup guidance names the missing key.
        let steps = core.setup_steps();
        let wants_key = !core.assistant_ready();
        let shows_key = steps.iter().any(|s| matches!(s, ln_core::SetupStep::KeyMissing { .. }));
        r.check("setup_lists_missing_key", shows_key == wants_key, format!("{steps:?}"));

        // 3b. Summary on the ChatGPT plan, signed out: its own setup step, the other roles keep
        //     their models, and the settings show the sign-in.
        let before_roles = core.settings().roles.clone();
        core.update_settings(|s| {
            s.roles.set(ln_types::ChatRole::Summary, ln_types::RoleConfig { provider: "chatgpt".into(), model: "gpt-plan".into() })
        });
        let roles = core.settings().roles;
        let separate = roles.get(ln_types::ChatRole::Summary).provider == "chatgpt"
            && roles.get(ln_types::ChatRole::Suggestions) == before_roles.get(ln_types::ChatRole::Suggestions)
            && roles.assistant == before_roles.assistant;
        r.check("roles_are_separate", separate, format!("{roles:?}"));
        let steps = core.setup_steps();
        let signed_out = core.chatgpt_status() == ln_core::ChatGptStatus::SignedOut;
        r.check("setup_asks_for_chatgpt_sign_in", !signed_out || steps.contains(&ln_core::SetupStep::ChatGptSignIn), format!("{steps:?}"));
        redraw(cx, &view);
        tall_shot(cx, handle, &view, &dir, "07b-settings-chatgpt").await;
        dispatch(cx, handle, crate::GoBack);
        wait(cx, 400).await;
        shot(cx, handle, &dir, "07c-setup-chatgpt");
        core.update_settings(|s| s.roles = before_roles.clone());

        // Categories with macOS tag colours (no key in the tour: nothing is sent to a model).
        core.update_settings(|s| {
            let cat = |name: &str, color, hint: &str| ln_types::Category { name: name.into(), color, hint: hint.into() };
            s.categories.list = vec![
                cat("Angebote", ln_types::TagColor::Orange, "Lieferanten, Preise, Muster"),
                cat("Teamrunden", ln_types::TagColor::Blue, ""),
                cat("Chinesisch-Kurs", ln_types::TagColor::Red, ""),
            ];
        });
        seed(&notes);
        core.rebuild_index();
        wait(cx, 2500).await;
        let list = view.read_with(cx, |v, _| v.note_list());
        let tagged = list.iter().filter(|(_, c)| !c.is_empty()).count();
        r.check("categories_in_note_list", tagged >= 2, format!("{list:?}"));
        dispatch(cx, handle, crate::OpenSettings);
        wait(cx, 800).await;
        tall_shot(cx, handle, &view, &dir, "07d-settings-categories").await;
        let manual = notes.join("Projekte/Angebot Shenzhen.md");
        let _ = cx.update_window(handle, |_, window, cx| view.update(cx, |v, cx| v.open_note(manual.clone(), window, cx)));
        wait(cx, 800).await;
        redraw(cx, &view);
        wait(cx, 300).await;
        shot(cx, handle, &dir, "02-note-reading");

        // 3c. Rename by hand from the header; the note list shows the new name, then back.
        let note_view = view.read_with(cx, |v, _| v.note_view());
        if let Some(n) = &note_view {
            let _ = cx.update_window(handle, |_, window, cx| n.update(cx, |n, cx| n.start_rename(window, cx)));
            wait(cx, 300).await;
            shot(cx, handle, &dir, "02b-note-rename");
            let _ = cx.update_window(handle, |_, window, cx| n.update(cx, |n, cx| n.rename_for_tour("Angebot Shenzhen Q1", window, cx)));
            wait(cx, 1200).await;
        }
        let list = view.read_with(cx, |v, _| v.note_list());
        r.check("rename_shows_in_note_list", list.iter().any(|(t, _)| t == "Angebot Shenzhen Q1"), format!("{list:?}"));
        if let Some(n) = &note_view {
            let _ = cx.update_window(handle, |_, window, cx| n.update(cx, |n, cx| n.rename_for_tour("Angebot Shenzhen", window, cx)));
            wait(cx, 800).await;
        }

        // 4. ⌘E toggles the editor.
        dispatch(cx, handle, crate::ToggleEdit);
        wait(cx, 800).await;
        let editing = view.read_with(cx, |v, cx| v.note_view().is_some_and(|n| n.read(cx).is_editing()));
        r.check("shortcut_toggles_editor", editing, "");
        shot(cx, handle, &dir, "03-note-editing");
        dispatch(cx, handle, crate::ToggleEdit);
        wait(cx, 300).await;

        let recorded = notes.join("2026/10/2026-10-20 1430 Teamrunde Budget.md");
        let _ = cx.update_window(handle, |_, window, cx| view.update(cx, |v, cx| v.open_note(recorded.clone(), window, cx)));
        wait(cx, 800).await;
        shot(cx, handle, &dir, "04-recorded-note");

        // 4b. Back and forward like a browser: note → settings → back to the note → forward.
        dispatch(cx, handle, crate::OpenSettings);
        wait(cx, 600).await;
        dispatch(cx, handle, crate::GoBack);
        wait(cx, 600).await;
        let page = view.read_with(cx, |v, _| v.page());
        r.check("back_returns_to_note", page.contains("Teamrunde Budget"), page.clone());
        dispatch(cx, handle, crate::GoBack);
        wait(cx, 600).await;
        let page = view.read_with(cx, |v, _| v.page());
        r.check("back_twice_reaches_previous_note", page.contains("Angebot Shenzhen"), page.clone());
        dispatch(cx, handle, crate::GoForward);
        wait(cx, 600).await;
        let page = view.read_with(cx, |v, _| v.page());
        r.check("forward_goes_ahead", page.contains("Teamrunde Budget"), page);

        // 5. ⌘K opens search; a query finds notes.
        dispatch(cx, handle, crate::OpenSearch);
        wait(cx, 500).await;
        let _ = cx.update_window(handle, |_, window, cx| {
            if let Some(s) = view.read(cx).search_panel() {
                s.update(cx, |s, cx| s.set_query("Budget", window, cx));
            }
        });
        wait(cx, 2500).await;
        let hits = view.read_with(cx, |v, cx| v.search_panel().map(|s| s.read(cx).hit_count()).unwrap_or(0));
        r.check("search_finds_notes", hits >= 1, format!("{hits} hits"));
        redraw(cx, &view);
        wait(cx, 300).await;
        shot(cx, handle, &dir, "05-search");
        dispatch(cx, handle, crate::OpenSearch);

        view.update(cx, |v, cx| v.show_sources(cx));
        wait(cx, 600).await;
        shot(cx, handle, &dir, "06-sources");

        // 6. Recording from files: language-correct transcript ends up in the note.
        core.update_settings(|s| s.recording_consent_accepted = true);
        let me = std::env::var_os("LN_DEMO_AUDIO_ME").map(PathBuf::from);
        let others = std::env::var_os("LN_DEMO_AUDIO_OTHERS").map(PathBuf::from);
        if let (Some(me), Some(others)) = (me, others) {
            let cfg = ln_core::CaptureConfig {
                me: ln_core::Source::File { path: me, realtime: true },
                others: ln_core::Source::File { path: others, realtime: true },
            };
            let beat = heartbeat(cx);
            wait(cx, 200).await;
            beat.set(0);
            let _ = cx.update_window(handle, |_, window, cx| view.update(cx, |v, cx| v.start_recording_with(Some(cfg), window, cx)));
            for _ in 0..40 {
                wait(cx, 100).await;
                if core.recording_active() {
                    break;
                }
            }
            wait(cx, 1500).await;
            r.check("ui_responsive_on_start", beat.get() < 250, format!("longest UI stall {} ms", beat.get()));
            beat.set(0);
            for _ in 0..50 {
                wait(cx, 1000).await;
                if core.recording_segments().len() >= 5 {
                    break;
                }
            }
            wait(cx, 1000).await;
            redraw(cx, &view);
            wait(cx, 300).await;
            r.check("ui_responsive_while_transcribing", beat.get() < 400, format!("longest UI stall {} ms", beat.get()));
            shot(cx, handle, &dir, "08-recording");
            let segs = core.recording_segments();
            let langs: std::collections::BTreeSet<String> = segs.iter().map(|s| s.lang.to_string()).collect();
            r.check(
                "file_recording_transcribes",
                segs.len() >= 4 && langs.len() >= 3,
                format!("{} segments, languages {langs:?}", segs.len()),
            );
            let path = core.recording_info().map(|i| i.note_path);
            // Let the heartbeat tick once after the screenshot (it blocks the UI thread itself).
            wait(cx, 300).await;
            beat.set(0);
            dispatch(cx, handle, crate::ToggleRecording);
            wait(cx, 400).await;
            let page = view.read_with(cx, |v, _| v.page());
            let opened = path.as_ref().is_some_and(|p| page.ends_with(&p.display().to_string()));
            r.check("stop_opens_note_at_once", opened && !core.recording_active(), page);
            shot(cx, handle, &dir, "09a-finishing");
            wait(cx, 300).await;
            beat.set(0);
            wait(cx, 2600).await;
            r.check("ui_responsive_on_stop", beat.get() < 250, format!("longest UI stall {} ms", beat.get()));
            for _ in 0..60 {
                if path.as_ref().is_none_or(|p| core.activity(p).is_none()) {
                    break;
                }
                wait(cx, 500).await;
            }
            let lines = path
                .and_then(|p| std::fs::read_to_string(p).ok())
                .map(|t| ln_notes::Note::parse(&t).unwrap_or_default().transcript_lines().len())
                .unwrap_or(0);
            r.check("transcript_saved_in_note", lines >= 4, format!("{lines} lines"));
            shot(cx, handle, &dir, "09-after-recording");
        }

        // 7. Real devices: system audio must pick up what afplay plays.
        if let Some(wav) = std::env::var_os("LN_TOUR_REAL_AUDIO").map(PathBuf::from) {
            // The tour can't click the app's explanation dialog: ask macOS directly (the user
            // answers the system prompt once per build).
            if ln_platform::microphone_permission() == ln_platform::Permission::NotDetermined {
                let (tx, rx) = futures::channel::oneshot::channel();
                let tx = parking_lot::Mutex::new(Some(tx));
                ln_platform::request_microphone_permission(move |ok| {
                    if let Some(tx) = tx.lock().take() {
                        let _ = tx.send(ok);
                    }
                });
                let _ = rx.await;
            }
            let beat = heartbeat(cx);
            wait(cx, 300).await;
            beat.set(0);
            dispatch(cx, handle, crate::ToggleRecording);
            for _ in 0..40 {
                wait(cx, 250).await;
                if core.recording_active() {
                    break;
                }
            }
            wait(cx, 1000).await;
            r.check("real_recording_starts", core.recording_active(), "");
            r.check("real_ui_responsive_on_start", beat.get() < 250, format!("longest UI stall {} ms", beat.get()));
            let mut player = std::process::Command::new("afplay").arg(&wav).spawn().ok();
            let secs = 12 + ln_asr_secs(&wav);
            for _ in 0..secs {
                wait(cx, 1000).await;
            }
            if let Some(p) = player.as_mut() {
                let _ = p.wait();
            }
            wait(cx, 3000).await;
            shot(cx, handle, &dir, "10-real-recording");
            let segs = core.recording_segments();
            let others = segs.iter().filter(|s| s.channel == ln_types::Channel::Others).count();
            let info = core.recording_info();
            r.check("real_system_audio_transcribes", others >= 1, format!("{} segments ({others} from system audio)", segs.len()));
            r.check(
                "real_microphone_delivers",
                info.as_ref().is_some_and(|i| i.level_me > 0.0),
                format!("mic level {:?}", info.as_ref().map(|i| i.level_me)),
            );
            wait(cx, 300).await;
            beat.set(0);
            dispatch(cx, handle, crate::ToggleRecording);
            wait(cx, 4000).await;
            r.check("real_ui_responsive_on_stop", beat.get() < 250, format!("longest UI stall {} ms", beat.get()));
        }

        let passed = r.write(&dir);
        eprintln!("tour: done ({})", if passed { "passed" } else { "FAILED" });
        core.shutdown();
        std::process::exit(if passed { 0 } else { 1 });
    })
    .detach();
}

/// Duration of a WAV file in whole seconds.
fn ln_asr_secs(p: &Path) -> u64 {
    std::fs::metadata(p).map(|m| m.len() / 32_000).unwrap_or(10)
}
