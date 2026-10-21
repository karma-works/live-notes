//! Screenshot tour (feature `screenshots`): seeds demo notes, walks through every view and
//! renders each one to `$LN_SCREENSHOT_DIR/*.png` via GPUI's offscreen rendering.
//! `LN_DEMO_AUDIO_ME` / `LN_DEMO_AUDIO_OTHERS` (WAV) feed the recording instead of devices.

use crate::app::AppView;
use gpui_kit::*;
use ln_core::Core;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

const MANUAL: &str = "# Angebot Shenzhen\n\nVergleich der zwei Angebote vor dem Gespräch mit Frau Li am Freitag.\n\n## Offene Punkte\n\n- [x] Muster bestellen\n- [ ] Lieferzeit klären (交货时间)\n- [ ] Zahlungsziel 60 Tage verhandeln\n\n## Preise\n\n| Anbieter | Stückpreis | MOQ |\n|---|---|---|\n| A | 3,20 $ | 1.000 |\n| B | 2,90 $ | 5.000 |\n\n> B ist günstiger, aber die Mindestmenge ist zu hoch für Q1.\n\n```text\nGesamt A = 1.000 × 3,20 = 3.200 $\n```\n";

const RECORDED: &str = "---\nid: 01JB8Z4K7Q2W3E4R5T6Y7U8I9O\ntitle: Teamrunde Budget\ncreated: 2026-10-20T14:30:12+02:00\nduration: 00:41:07\nlanguages:\n- zh\n- de\ntranscription: local/whisper-large-v3-turbo\nspeakers:\n  others: Frau Li\n---\n\n<!-- ln:summary -->\n## Zusammenfassung\n- Das Budget für Q1 wird nächste Woche final besprochen\n- Frau Li schickt das aktualisierte Angebot bis Freitag\n\n<!-- ln:tasks -->\n## Aufgaben\n- [ ] Angebot schicken — Frau Li, Freitag\n- [ ] Budgetentwurf vorbereiten — Ich\n\n<!-- ln:notes -->\n## Meine Notizen\n- Budget Q1 klären\n- Termin mit Li\n\n<!-- ln:transcript -->\n## Transkript\n[00:00:03] Ich (de): Hallo zusammen, können wir das Budget bis Freitag festzurren?\n[00:00:11] Frau Li (zh): 我们下周再讨论预算，我周五前把报价发给你。\n[00:00:19] Ich (de): Gut, dann machen wir das so.\n[00:00:24] Frau Li (en): Perfect, I will also send the delivery schedule.\n";

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

pub fn run(core: Arc<Core>, handle: AnyWindowHandle, view: Entity<AppView>, cx: &mut App) {
    let Some(dir) = std::env::var_os("LN_SCREENSHOT_DIR").map(PathBuf::from) else { return };
    let _ = std::fs::create_dir_all(&dir);
    let notes = core.notes_dir();
    cx.spawn(async move |cx: &mut AsyncApp| {
        wait(cx, 1500).await;
        if !core.settings().first_run_done {
            shot(cx, handle, &dir, "01-first-run");
            core.complete_first_run(ln_core::AssistantChoice::OpenRouter { key: None });
            view.update(cx, |v, cx| v.close_first_run(cx));
        }
        seed(&notes);
        core.rebuild_index();
        wait(cx, 2500).await;
        let manual = notes.join("Projekte/Angebot Shenzhen.md");
        let _ = cx.update_window(handle, |_, window, cx| view.update(cx, |v, cx| v.open_note(manual.clone(), window, cx)));
        wait(cx, 800).await;
        redraw(cx, &view);
        wait(cx, 300).await;
        shot(cx, handle, &dir, "02-note-reading");

        let _ = cx.update_window(handle, |_, window, cx| {
            if let Some(n) = view.read(cx).note_view() {
                n.update(cx, |n, cx| n.toggle_mode(window, cx));
            }
        });
        wait(cx, 800).await;
        shot(cx, handle, &dir, "03-note-editing");
        let _ = cx.update_window(handle, |_, window, cx| {
            if let Some(n) = view.read(cx).note_view() {
                n.update(cx, |n, cx| n.toggle_mode(window, cx));
            }
        });

        let recorded = notes.join("2026/10/2026-10-20 1430 Teamrunde Budget.md");
        let _ = cx.update_window(handle, |_, window, cx| view.update(cx, |v, cx| v.open_note(recorded.clone(), window, cx)));
        wait(cx, 800).await;
        shot(cx, handle, &dir, "04-recorded-note");

        let _ = cx.update_window(handle, |_, window, cx| {
            view.update(cx, |v, cx| v.open_search(window, cx));
            if let Some(s) = view.read(cx).search_panel() {
                s.update(cx, |s, cx| s.set_query("Budget", window, cx));
            }
        });
        wait(cx, 2500).await;
        redraw(cx, &view);
        wait(cx, 300).await;
        shot(cx, handle, &dir, "05-search");
        let _ = cx.update_window(handle, |_, window, cx| view.update(cx, |v, cx| v.open_search(window, cx)));

        view.update(cx, |v, cx| v.show_sources(cx));
        wait(cx, 600).await;
        shot(cx, handle, &dir, "06-sources");

        view.update(cx, |v, cx| v.open_settings(cx));
        wait(cx, 1200).await;
        let settings_handle = cx.update(|cx| cx.windows().into_iter().find(|w| *w != handle));
        if let Some(h) = settings_handle {
            shot(cx, h, &dir, "07-settings");
            let _ = cx.update_window(h, |_, window, _| window.remove_window());
        }

        let me = std::env::var_os("LN_DEMO_AUDIO_ME").map(PathBuf::from);
        let others = std::env::var_os("LN_DEMO_AUDIO_OTHERS").map(PathBuf::from);
        if let (Some(me), Some(others)) = (me, others) {
            core.update_settings(|s| s.recording_consent_accepted = true);
            let cfg = ln_core::CaptureConfig {
                me: ln_core::Source::File { path: me, realtime: true },
                others: ln_core::Source::File { path: others, realtime: true },
            };
            let _ = cx.update_window(handle, |_, window, cx| view.update(cx, |v, cx| v.start_recording_with(Some(cfg), window, cx)));
            for _ in 0..45 {
                wait(cx, 1000).await;
                if core.recording_segments().len() >= 5 {
                    break;
                }
            }
            wait(cx, 1000).await;
            redraw(cx, &view);
            wait(cx, 300).await;
            shot(cx, handle, &dir, "08-recording");
            let c = core.clone();
            let _ = cx.background_spawn(async move { c.stop_recording() }).await;
            wait(cx, 2000).await;
            shot(cx, handle, &dir, "09-after-recording");
        }
        eprintln!("tour: done");
        cx.update(|cx| {
            core.shutdown();
            cx.quit()
        });
    })
    .detach();
}
