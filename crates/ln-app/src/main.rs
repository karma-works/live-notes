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

actions!(live_notes, [Quit, NewNote, ToggleRecording, OpenSearch, ToggleEdit, OpenSettings, ToggleSidebar, CloseWindow, ShowSources]);

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()))
        .init();
    let paths = ln_types::AppPaths::from_env();
    let (core, events) = match Core::start(paths, Arc::new(ln_platform::KeychainStore)) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("Live Notes could not start: {e}");
            std::process::exit(1);
        }
    };
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
        hotkey::install(_handle, _view.clone(), cx);
        tray::install(core.clone(), _handle, _view.clone(), cx);
        #[cfg(feature = "screenshots")]
        tour::run(core.clone(), _handle, _view, cx);
        cx.activate(true);
    });
}
