//! Global shortcut ⌥⌘R to start/stop a recording while the app is in the background (SPEC §5 F1).

use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use gpui_kit::*;
use std::time::Duration;

pub fn install(handle: AnyWindowHandle, view: Entity<crate::app::AppView>, cx: &mut App) {
    #[cfg(target_os = "linux")]
    if std::env::var_os("WAYLAND_DISPLAY").is_some_and(|display| !display.is_empty()) {
        wayland(handle, view, cx);
        return;
    }
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

#[cfg(target_os = "linux")]
fn wayland(handle: AnyWindowHandle, view: Entity<crate::app::AppView>, cx: &mut App) {
    use futures::StreamExt;

    let (sender, mut activations) = futures::channel::mpsc::channel(1);
    if let Err(error) = std::thread::Builder::new().name("shortcut-portal".into()).spawn(move || {
        let result = futures::executor::block_on(portal_activations(sender));
        if let Err(error) = result {
            tracing::warn!("global shortcut portal unavailable: {error}");
        }
    }) {
        tracing::warn!("could not start shortcut portal worker: {error}");
        return;
    }
    cx.spawn(async move |cx: &mut AsyncApp| {
        while activations.next().await.is_some() {
            if cx.update_window(handle, |_, window, cx| view.update(cx, |v, cx| v.toggle_recording(window, cx))).is_err() {
                break;
            }
        }
    })
    .detach();
}

#[cfg(target_os = "linux")]
async fn portal_activations(mut sender: futures::channel::mpsc::Sender<()>) -> Result<(), ashpd::Error> {
    use ashpd::desktop::global_shortcuts::{BindShortcutsOptions, GlobalShortcuts, NewShortcut};
    use futures::StreamExt;

    const ID: &str = "toggle-recording";
    let connection = zbus::Connection::session().await?;
    if let Err(error) = ashpd::register_host_app_with_connection(connection.clone(), crate::APP_ID.parse()?).await {
        // Older portals and desktop launchers that already supply an application ID can proceed.
        tracing::debug!("shortcut portal registration: {error}");
    }
    let portal = GlobalShortcuts::with_connection(connection).await?;
    let session = portal.create_session(Default::default()).await?;
    let session_path = serde_json::to_value(&session).map_err(std::io::Error::other)?;
    let mut events = portal.receive_activated().await?;
    let shortcut = NewShortcut::new(ID, crate::i18n::t("record")).preferred_trigger("ALT+LOGO+r");
    portal.bind_shortcuts(&session, &[shortcut], None, BindShortcutsOptions::default()).await?.response()?;
    tracing::debug!("registered global recording shortcut through the Wayland portal");
    while let Some(event) = events.next().await {
        if Some(event.session_handle().as_str()) == session_path.as_str()
            && event.shortcut_id() == ID
            && let Err(error) = sender.try_send(())
            && error.is_disconnected()
        {
            break;
        }
    }
    session.close().await
}
