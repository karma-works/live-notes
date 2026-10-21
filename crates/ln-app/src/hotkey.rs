//! Global shortcut ⌥⌘R to start/stop a recording while the app is in the background (SPEC §5 F1).

use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use gpui_kit::*;
use std::time::Duration;

pub fn install(handle: AnyWindowHandle, view: Entity<crate::app::AppView>, cx: &mut App) {
    let Ok(manager) = GlobalHotKeyManager::new() else { return };
    let key = HotKey::new(Some(Modifiers::ALT | Modifiers::SUPER), Code::KeyR);
    if manager.register(key).is_err() {
        return;
    }
    let id = key.id();
    cx.spawn(async move |cx: &mut AsyncApp| {
        // The manager must stay alive for the hotkey to stay registered.
        let _manager = manager;
        loop {
            cx.background_executor().timer(Duration::from_millis(150)).await;
            while let Ok(ev) = GlobalHotKeyEvent::receiver().try_recv() {
                if ev.id == id && ev.state == HotKeyState::Pressed {
                    let _ = cx.update_window(handle, |_, window, cx| view.update(cx, |v, cx| v.toggle_recording(window, cx)));
                }
            }
        }
    })
    .detach();
}
