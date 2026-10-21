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

actions!(
    live_notes,
    [Quit, NewNote, ToggleRecording, OpenSearch, ToggleEdit, OpenSettings, ToggleSidebar, CloseWindow, ShowSources, GoBack, GoForward]
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
    tracing::info!("Live Notes {} started, ui={:?}", env!("CARGO_PKG_VERSION"), core.ui_language());
    i18n::set_language(core.ui_language());
    core.resume_downloads();

    gpui_kit::application().with_assets(gpui_kit::assets::Assets).run(move |cx| {
        gpui_kit::init(cx);
        theme::apply(cx);
        cx.bind_keys([
            KeyBinding::new("cmd-q", Quit, None),
            KeyBinding::new("cmd-n", NewNote, None),
            KeyBinding::new("alt-cmd-r", ToggleRecording, None),
            KeyBinding::new("cmd-k", OpenSearch, None),
            KeyBinding::new("cmd-e", ToggleEdit, None),
            KeyBinding::new("cmd-,", OpenSettings, None),
            KeyBinding::new("cmd-\\", ToggleSidebar, None),
            KeyBinding::new("cmd-w", CloseWindow, None),
            KeyBinding::new("cmd-[", GoBack, None),
            KeyBinding::new("cmd-]", GoForward, None),
        ]);
        let quit_core = core.clone();
        cx.on_action(move |_: &Quit, cx: &mut App| {
            quit_core.shutdown();
            cx.quit();
        });
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
                MenuItem::separator(),
                MenuItem::action(i18n::t("record"), ToggleRecording),
                MenuItem::separator(),
                MenuItem::action(i18n::t("sources"), ShowSources),
            ]),
            Menu::new(i18n::t("view")).items([MenuItem::action(i18n::t("back"), GoBack), MenuItem::action(i18n::t("forward"), GoForward)]),
        ]);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(1180.), px(760.)), cx))),
            titlebar: Some(gpui_kit::component::TitleBar::title_bar_options()),
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
        hotkey::install(_handle, _view.clone(), cx);
        tray::install(core.clone(), _handle, _view.clone(), cx);
        #[cfg(feature = "screenshots")]
        tour::run(core.clone(), _handle, _view, cx);
        cx.activate(true);
    });
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
