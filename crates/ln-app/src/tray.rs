//! Menu bar icon (SPEC §5 F9): recording state, start/stop, search, open window, quit.

use crate::app::AppView;
use crate::i18n::t;
use gpui_kit::*;
use ln_core::Core;
use std::sync::Arc;
use std::time::Duration;
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIconBuilder};

/// 18×18 circle: outline when idle (template image, follows the menu bar colour), filled red
/// while recording.
fn icon(recording: bool) -> Icon {
    let n = 36u32; // @2x
    let mut rgba = vec![0u8; (n * n * 4) as usize];
    let c = (n as f32 - 1.0) / 2.0;
    for y in 0..n {
        for x in 0..n {
            let d = ((x as f32 - c).powi(2) + (y as f32 - c).powi(2)).sqrt();
            let i = ((y * n + x) * 4) as usize;
            let (inside, ring) = (d <= 12.0, (10.0..=13.0).contains(&d));
            let a = if recording { inside } else { ring };
            if a {
                let (r, g, b) = if recording { (230, 60, 60) } else { (0, 0, 0) };
                rgba[i..i + 4].copy_from_slice(&[r, g, b, 255]);
            }
        }
    }
    Icon::from_rgba(rgba, n, n).expect("icon")
}

pub fn install(core: Arc<Core>, handle: AnyWindowHandle, view: Entity<AppView>, cx: &mut App) {
    #[cfg(target_os = "linux")]
    if let Err(error) = gtk::init() {
        tracing::warn!("tray unavailable: {error}");
        return;
    }
    let menu = Menu::new();
    let record = MenuItem::new(t("record"), true, None);
    let search = MenuItem::new(t("search"), true, None);
    let open = MenuItem::new(t("open"), true, None);
    let quit = MenuItem::new("Quit Live Notes", true, None);
    let _ = menu.append_items(&[&record, &search, &open, &PredefinedMenuItem::separator(), &quit]);
    let Ok(tray) = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_icon(icon(false))
        .with_icon_as_template(true)
        .with_tooltip("Live Notes")
        .build()
    else {
        return;
    };
    let ids = (record.id().clone(), search.id().clone(), open.id().clone(), quit.id().clone());
    cx.spawn(async move |cx: &mut AsyncApp| {
        let tray = tray;
        let mut was_recording = false;
        loop {
            cx.background_executor().timer(Duration::from_millis(250)).await;
            #[cfg(target_os = "linux")]
            while gtk::events_pending() {
                gtk::main_iteration_do(false);
            }
            let rec = core.recording_active();
            if rec != was_recording {
                was_recording = rec;
                let _ = tray.set_icon(Some(icon(rec)));
                #[cfg(target_os = "macos")]
                tray.set_icon_as_template(!rec);
                record.set_text(if rec { t("stop") } else { t("record") });
            }
            while let Ok(ev) = MenuEvent::receiver().try_recv() {
                let id = ev.id;
                if id == ids.0 {
                    let _ = cx.update_window(handle, |_, window, cx| view.update(cx, |v, cx| v.toggle_recording(window, cx)));
                } else if id == ids.1 {
                    cx.update(|cx| cx.activate(true));
                    let _ = cx.update_window(handle, |_, window, cx| {
                        window.activate_window();
                        view.update(cx, |v, cx| v.open_search(window, cx))
                    });
                } else if id == ids.2 {
                    cx.update(|cx| cx.activate(true));
                    let _ = cx.update_window(handle, |_, window, _| window.activate_window());
                } else if id == ids.3 {
                    cx.update(|cx| cx.quit());
                }
            }
        }
    })
    .detach();
}
