//! Live Notes — native desktop app (SPEC §6). The window is a thin layer over `ln-core`.

mod app;
mod first_run;
mod hotkey;
mod i18n;
mod markdown;
mod note_view;
mod recording_view;
mod search;
mod settings_view;
mod sources_view;
mod theme;
#[cfg(feature = "screenshots")]
mod tour;
mod tray;

use gpui_kit::*;
use ln_core::Core;
use std::sync::Arc;

#[cfg(target_os = "linux")]
const APP_ID: &str = "works.karma.livenotes";

actions!(
    live_notes,
    [
        Quit,
        NewNote,
        ToggleRecording,
        OpenSearch,
        ToggleEdit,
        OpenSettings,
        ToggleSidebar,
        CloseWindow,
        ShowSources,
        GoBack,
        GoForward,
        DeleteNote
    ]
);

fn main() {
    let paths = ln_types::AppPaths::from_env();
    init_logging(&paths);
    let args: Vec<String> = std::env::args().collect();
    // Test runs (verify.sh) must never read or rotate the user's real keys and ChatGPT tokens.
    let secrets: Arc<dyn ln_platform::SecretStore> = if std::env::var("LN_SECRETS").as_deref() == Ok("memory") {
        Arc::new(ln_platform::MemoryStore::default())
    } else {
        Arc::new(ln_platform::KeychainStore)
    };
    let (core, events) = match Core::start(paths, secrets) {
        Ok(x) => x,
        Err(e) => {
            tracing::error!("start failed: {e}");
            eprintln!("Live Notes could not start: {e}");
            std::process::exit(1);
        }
    };
    // Headless audio self-test (verification skill): `--probe-capture SECS --report PATH`.
    if let Some(i) = args.iter().position(|a| a == "--probe-capture") {
        let secs = args.get(i + 1).and_then(|s| s.parse().ok()).unwrap_or(6);
        let report = core.probe_capture(secs, !args.iter().any(|a| a == "--no-transcribe"));
        let json = serde_json::to_string_pretty(&report).unwrap_or_default();
        match args.iter().position(|a| a == "--report").and_then(|j| args.get(j + 1)) {
            Some(path) => {
                let _ = std::fs::write(path, &json);
            }
            None => println!("{json}"),
        }
        tracing::info!("probe: {json}");
        core.shutdown();
        std::process::exit(0);
    }
    // Headless browser-extension endpoint (verification): `--serve-clip` serves until killed and
    // prints status and token, so `curl` can drive `docs/clip-api.md` without a window.
    if args.iter().any(|a| a == "--serve-clip") {
        core.start_clip_server();
        let status = format!("{:?}", core.clip_server_status());
        tracing::info!("serve-clip: {status}");
        println!("{}", serde_json::json!({ "status": status, "token": core.clip_token() }));
        loop {
            std::thread::sleep(std::time::Duration::from_secs(3600));
        }
    }
    tracing::info!("Live Notes {} started, ui={:?}", env!("CARGO_PKG_VERSION"), core.ui_language());
    i18n::set_language(core.ui_language());
    core.resume_downloads();
    // Local endpoint for the browser extension; reading the token may wait for the keyring.
    {
        let c = core.clone();
        std::thread::spawn(move || c.start_clip_server());
    }

    // Markdown files opened from the Finder ("Open With", default app, drag onto the Dock icon)
    // arrive as file:// URLs, also before the window exists; the window picks them up in order.
    let (open_tx, mut open_rx) = futures::channel::mpsc::unbounded::<std::path::PathBuf>();
    let application = gpui_kit::application().with_assets(gpui_kit::assets::Assets);
    application.on_open_urls(move |urls| {
        for path in urls.iter().filter_map(|u| file_url_to_path(u)) {
            tracing::info!("open file from system");
            let _ = open_tx.unbounded_send(path);
        }
    });
    application.run(move |cx| {
        gpui_kit::init(cx);
        #[cfg(target_os = "linux")]
        cx.set_app_identity(APP_ID, "Live Notes");
        theme::apply(cx);
        cx.bind_keys([
            KeyBinding::new("secondary-q", Quit, None),
            KeyBinding::new("secondary-n", NewNote, None),
            KeyBinding::new("secondary-alt-r", ToggleRecording, None),
            KeyBinding::new("secondary-k", OpenSearch, None),
            KeyBinding::new("secondary-e", ToggleEdit, None),
            KeyBinding::new("secondary-,", OpenSettings, None),
            KeyBinding::new("secondary-\\", ToggleSidebar, None),
            KeyBinding::new("secondary-w", CloseWindow, None),
            KeyBinding::new("secondary-[", GoBack, None),
            KeyBinding::new("secondary-]", GoForward, None),
            // Backspace and Delete remove the open note unless a text field has the keyboard. ⌘⌫ is
            // bound last, so menus and tooltips show it; inside a text field the field keeps it.
            KeyBinding::new("backspace", DeleteNote, Some("LiveNotes && !Input")),
            KeyBinding::new("delete", DeleteNote, Some("LiveNotes && !Input")),
            KeyBinding::new("secondary-backspace", DeleteNote, None),
        ]);
        cx.on_action(|_: &Quit, cx: &mut App| cx.quit());
        // Every quit path ends here: menu, ⌘Q, tray, Dock, logout and AppleScript `quit`.
        let quit_core = core.clone();
        cx.on_app_quit(move |_| {
            if !quit_core.shutdown() {
                // A model still loaded makes whisper.cpp abort in its exit-time cleanup; the notes
                // are already saved, so skip that cleanup instead of ending with a crash report.
                unsafe { libc::_exit(0) };
            }
            async {}
        })
        .detach();
        cx.set_menus([
            Menu::new("Live Notes").items([
                MenuItem::action(i18n::t("settings"), OpenSettings),
                MenuItem::separator(),
                MenuItem::action("Quit Live Notes", Quit),
            ]),
            Menu::new(i18n::t("notes")).items([
                MenuItem::action(i18n::t("new-note"), NewNote),
                MenuItem::action(i18n::t("search"), OpenSearch),
                MenuItem::action(i18n::t("edit"), ToggleEdit),
                MenuItem::action(i18n::t("delete-note-menu"), DeleteNote),
                MenuItem::separator(),
                MenuItem::action(i18n::t("record"), ToggleRecording),
                MenuItem::separator(),
                MenuItem::action(i18n::t("sources"), ShowSources),
            ]),
            Menu::new(i18n::t("view")).items([
                MenuItem::action(i18n::t("back"), GoBack),
                MenuItem::action(i18n::t("forward"), GoForward),
                MenuItem::separator(),
                MenuItem::action(i18n::t("toggle-sidebar"), ToggleSidebar),
            ]),
        ]);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(1180.), px(760.)), cx))),
            titlebar: Some(gpui_kit::component::TitleBar::title_bar_options()),
            #[cfg(target_os = "linux")]
            app_id: Some(APP_ID.into()),
            window_min_size: Some(size(px(760.), px(480.))),
            ..Default::default()
        };
        let core2 = core.clone();
        let events = parking_lot::Mutex::new(Some(events));
        let (_handle, _view) = gpui_kit::open_window(options, cx, move |window, cx| {
            let ev = events.lock().take().expect("events");
            cx.new(|cx| app::AppView::new(core2.clone(), ev, window, cx))
        })
        .expect("failed to open window");
        // Fallbacks: menu commands and shortcuts reach the main view even when no element in it has
        // the keyboard focus (e.g. right after the first-run screen or with the window in back).
        macro_rules! fallback {
            ($action:ty, $body:expr) => {{
                let view = _view.clone();
                cx.on_action(move |_: &$action, cx: &mut App| {
                    tracing::info!("fallback action {}", stringify!($action));
                    let view = view.clone();
                    // Actions are dispatched inside a window update; run after it finishes.
                    cx.defer(move |cx| {
                        let _ = _handle.update(cx, |_, window, cx| {
                            window.activate_window();
                            view.update(cx, |v, cx| $body(v, window, cx))
                        });
                    });
                });
            }};
        }
        fallback!(OpenSettings, |v: &mut app::AppView, w: &mut Window, cx: &mut Context<app::AppView>| v.open_settings(w, cx));
        fallback!(GoBack, |v: &mut app::AppView, w: &mut Window, cx: &mut Context<app::AppView>| v.go_back(w, cx));
        fallback!(GoForward, |v: &mut app::AppView, w: &mut Window, cx: &mut Context<app::AppView>| v.go_forward(w, cx));
        fallback!(OpenSearch, |v: &mut app::AppView, w: &mut Window, cx: &mut Context<app::AppView>| v.open_search(w, cx));
        fallback!(NewNote, |v: &mut app::AppView, w: &mut Window, cx: &mut Context<app::AppView>| v.new_note(w, cx));
        fallback!(ToggleRecording, |v: &mut app::AppView, w: &mut Window, cx: &mut Context<app::AppView>| v.toggle_recording(w, cx));
        fallback!(ShowSources, |v: &mut app::AppView, _w: &mut Window, cx: &mut Context<app::AppView>| v.show_sources(cx));
        {
            let view = _view.clone();
            cx.spawn(async move |cx| {
                use futures::StreamExt as _;
                while let Some(path) = open_rx.next().await {
                    let _ = _handle.update(cx, |_, window, cx| {
                        window.activate_window();
                        view.update(cx, |v, cx| v.open_file(path, window, cx))
                    });
                }
            })
            .detach();
        }
        hotkey::install(_handle, _view.clone(), cx);
        tray::install(core.clone(), _handle, _view.clone(), cx);
        #[cfg(feature = "screenshots")]
        tour::run(core.clone(), _handle, _view, cx);
        cx.activate(true);
    });
}

/// `file:///Users/me/My%20Notes/a.md` → `/Users/me/My Notes/a.md`. Other schemes are ignored.
fn file_url_to_path(url: &str) -> Option<std::path::PathBuf> {
    let rest = url.strip_prefix("file://")?;
    // An optional host (`file://localhost/...`) comes before the path.
    let path = &rest[rest.find('/')?..];
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(b) = path.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(b);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    use std::os::unix::ffi::OsStringExt as _;
    Some(std::path::PathBuf::from(std::ffi::OsString::from_vec(out)))
}

/// Logs to `~/Library/Logs/Live Notes/live-notes.log` (and stderr). No transcript content is logged.
fn init_logging(paths: &ln_types::AppPaths) {
    use tracing_subscriber::{EnvFilter, fmt, prelude::*};
    let _ = std::fs::create_dir_all(&paths.logs_dir);
    let filter = EnvFilter::try_from_env("LN_LOG")
        .unwrap_or_else(|_| EnvFilter::new("warn,live_notes=info,ln_core=info,ln_audio=info,ln_platform=info,ln_asr=info"));
    let file = std::fs::OpenOptions::new().create(true).append(true).open(paths.logs_dir.join("live-notes.log")).ok();
    let registry = tracing_subscriber::registry().with(filter).with(fmt::layer().with_writer(std::io::stderr));
    match file {
        Some(f) => registry.with(fmt::layer().with_ansi(false).with_writer(std::sync::Mutex::new(f))).init(),
        None => registry.init(),
    }
}

#[cfg(test)]
mod tests {
    use super::file_url_to_path;
    use std::path::PathBuf;

    #[test]
    fn file_urls_become_paths() {
        assert_eq!(file_url_to_path("file:///Users/me/My%20Notes/%C3%84rger.md"), Some(PathBuf::from("/Users/me/My Notes/Ärger.md")));
        assert_eq!(file_url_to_path("file://localhost/tmp/a.md"), Some(PathBuf::from("/tmp/a.md")));
        assert_eq!(file_url_to_path("https://example.com/a.md"), None);
    }
}
