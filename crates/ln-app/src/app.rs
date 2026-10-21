//! Main window (SPEC §6.2): sidebar, the open note or recording, search overlay.

use crate::first_run::{FirstRunEvent, FirstRunView};
use crate::i18n::t;
use crate::note_view::{NoteView, NoteViewEvent};
use crate::recording_view::RecordingView;
use crate::search::{SearchEvent, SearchPanel};
use crate::settings_view::SettingsView;
use crate::sources_view::SourcesView;
use crate::{CloseWindow, GoBack, GoForward, NewNote, OpenSearch, OpenSettings, ShowSources, ToggleEdit, ToggleRecording, ToggleSidebar};
use chrono::{Datelike, Local, TimeZone};
use futures::StreamExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::progress::Progress;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IconName, Sizable as _, StyledExt as _, TitleBar, WindowExt as _, h_flex,
    scroll::ScrollableElement as _, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use ln_core::{Core, CoreEvent, EventReceiver, NoteEntry};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq)]
enum Selected {
    None,
    Note(PathBuf),
    Recording,
    Sources,
    Settings,
}

pub struct AppView {
    core: Arc<Core>,
    notes: Vec<NoteEntry>,
    folders: Vec<String>,
    selected: Selected,
    note: Option<Entity<NoteView>>,
    recording: Option<Entity<RecordingView>>,
    search: Option<Entity<SearchPanel>>,
    first_run: Option<Entity<FirstRunView>>,
    sources: Option<Entity<SourcesView>>,
    settings: Option<Entity<SettingsView>>,
    /// Browser-like history of the main pane (⌘[ / ⌘]).
    back: Vec<Selected>,
    forward: Vec<Selected>,
    download: Option<f32>,
    starting: bool,
    sidebar_open: bool,
    focus: FocusHandle,
    _subs: Vec<Subscription>,
    _tasks: Vec<Task<()>>,
}

impl AppView {
    pub fn new(core: Arc<Core>, events: EventReceiver, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut subs = Vec::new();
        let first_run = if core.settings().first_run_done {
            None
        } else {
            let fr = cx.new(|cx| FirstRunView::new(core.clone(), window, cx));
            subs.push(cx.subscribe_in(&fr, window, |this, _, ev: &FirstRunEvent, window, cx| {
                let FirstRunEvent::Done = ev;
                this.first_run = None;
                // The first-run screen had the focus; without this, menu commands and shortcuts
                // would not reach the main view.
                window.focus(&this.focus, cx);
                cx.notify();
            }));
            Some(fr)
        };
        let events_task = cx.spawn_in(window, async move |this, cx| {
            let mut events = events;
            while let Some(ev) = events.next().await {
                if this.update_in(cx, |v, window, cx| v.on_core_event(ev, window, cx)).is_err() {
                    break;
                }
            }
        });
        // Recording timer and level meters refresh.
        let tick = cx.spawn(async move |this, cx: &mut AsyncApp| {
            loop {
                cx.background_executor().timer(Duration::from_millis(250)).await;
                let Ok(()) = this.update(cx, |v, cx| {
                    if let Some(r) = &v.recording {
                        r.update(cx, |_, cx| cx.notify());
                    }
                }) else {
                    break;
                };
            }
        });
        let mut v = AppView {
            core,
            notes: vec![],
            folders: vec![],
            selected: Selected::None,
            note: None,
            recording: None,
            search: None,
            first_run,
            sources: None,
            settings: None,
            back: vec![],
            forward: vec![],
            download: None,
            starting: false,
            sidebar_open: true,
            focus: cx.focus_handle(),
            _subs: subs,
            _tasks: vec![events_task, tick],
        };
        v.reload_notes(cx);
        window.focus(&v.focus, cx);
        // Load Whisper now, so that "Start recording" is instant later.
        v.core.warm_up();
        v
    }

    fn reload_notes(&mut self, cx: &mut Context<Self>) {
        let core = self.core.clone();
        cx.spawn(async move |this, cx: &mut AsyncApp| {
            let (notes, folders) = cx.background_spawn(async move { (core.list_notes(), core.note_folders()) }).await;
            let _ = this.update(cx, |v, cx| {
                v.notes = notes;
                v.folders = folders;
                cx.notify();
            });
        })
        .detach();
    }

    fn on_core_event(&mut self, ev: CoreEvent, window: &mut Window, cx: &mut Context<Self>) {
        let t0 = std::time::Instant::now();
        let name = format!("{ev:?}").chars().take(40).collect::<String>();
        self.on_core_event_inner(ev, window, cx);
        if t0.elapsed() > Duration::from_millis(30) {
            tracing::warn!("slow UI: event {name} took {:?}", t0.elapsed());
        }
    }

    fn on_core_event_inner(&mut self, ev: CoreEvent, window: &mut Window, cx: &mut Context<Self>) {
        match ev {
            CoreEvent::NotesChanged => self.reload_notes(cx),
            CoreEvent::NoteFileChanged(p) => {
                if let Some(n) = &self.note
                    && n.read(cx).path() == p
                {
                    n.update(cx, |n, cx| n.file_changed(window, cx));
                }
            }
            CoreEvent::SummaryReady { path } => {
                window.push_notification(t("summary-ready"), cx);
                if matches!(&self.selected, Selected::Note(p) if p.parent() == path.parent()) {
                    self.open_note(path, window, cx);
                }
            }
            CoreEvent::RecordingChanged => {
                if !self.core.recording_active() && self.recording.is_some() {
                    let path = self.recording.as_ref().map(|r| r.read(cx).note_path().to_path_buf());
                    self.recording = None;
                    if let Some(p) = path {
                        self.open_note(p, window, cx);
                    }
                }
            }
            CoreEvent::Segment { .. }
            | CoreEvent::SegmentReplaced { .. }
            | CoreEvent::Suggestion(_)
            | CoreEvent::SuggestionAnswer { .. } => {
                if let Some(r) = &self.recording {
                    r.update(cx, |r, cx| r.on_event(&ev, cx));
                }
            }
            CoreEvent::Answer { .. } => {
                if let Some(s) = &self.search {
                    s.update(cx, |s, cx| s.on_event(&ev, cx));
                }
            }
            CoreEvent::DownloadProgress { fraction, .. } => {
                self.download = Some(fraction);
                if let Some(f) = &self.first_run {
                    f.update(cx, |_, cx| cx.notify());
                }
            }
            CoreEvent::ModelsChanged => {
                self.download = self.core.model_status().iter().find_map(|m| m.progress);
                if self.download.is_none() {
                    self.core.warm_up();
                }
                if let Some(s) = &self.settings {
                    s.update(cx, |_, cx| cx.notify());
                }
            }
            CoreEvent::SourcesChanged | CoreEvent::IndexProgress { .. } => {
                if let Some(s) = &self.sources {
                    s.update(cx, |s, cx| s.refresh(cx));
                }
            }
            CoreEvent::ProviderTested { provider, result } => {
                if let Some(s) = &self.settings {
                    s.update(cx, |s, cx| s.provider_tested(&provider, result, cx));
                }
            }
            CoreEvent::SettingsChanged => {
                crate::i18n::set_language(self.core.ui_language());
                // Load the transcriber for changed transcription settings before the next recording.
                self.core.warm_up();
                cx.notify();
            }
            CoreEvent::ChatGptChanged { first_sign_in } => {
                if let Some(s) = &self.settings {
                    s.update(cx, |s, cx| s.chatgpt_changed(first_sign_in, window, cx));
                }
                if first_sign_in && self.selected != Selected::Settings {
                    window.open_alert_dialog(cx, |alert, _, _| {
                        alert.title(t("chatgpt-first-title")).description(t("chatgpt-first-body")).ok_text(t("got-it"))
                    });
                }
            }
            CoreEvent::ActivityChanged => {
                if let Some(n) = &self.note {
                    n.update(cx, |_, cx| cx.notify());
                }
            }
            CoreEvent::Notice(msg) => window.push_notification(msg, cx),
        }
        cx.notify();
    }

    /// Shows `to` in the main pane and records where we came from (like a browser).
    fn navigate(&mut self, to: Selected) {
        if self.selected == to {
            return;
        }
        let from = std::mem::replace(&mut self.selected, to);
        if from != Selected::None {
            self.back.retain(|s| *s != from);
            self.back.push(from);
        }
        self.forward.clear();
    }

    /// An entry can be shown again: the note still exists, the recording still runs.
    fn reachable(&self, s: &Selected) -> bool {
        match s {
            Selected::Note(p) => p.exists(),
            Selected::Recording => self.recording.is_some(),
            _ => true,
        }
    }

    pub(crate) fn go_back(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        while let Some(to) = self.back.pop() {
            if self.reachable(&to) {
                let from = std::mem::replace(&mut self.selected, Selected::None);
                if from != Selected::None {
                    self.forward.push(from);
                }
                self.show(to, window, cx);
                return;
            }
        }
    }

    pub(crate) fn go_forward(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        while let Some(to) = self.forward.pop() {
            if self.reachable(&to) {
                let from = std::mem::replace(&mut self.selected, Selected::None);
                if from != Selected::None {
                    self.back.push(from);
                }
                self.show(to, window, cx);
                return;
            }
        }
    }

    /// Shows an entry from the history without recording it again.
    fn show(&mut self, to: Selected, window: &mut Window, cx: &mut Context<Self>) {
        match &to {
            Selected::Note(p) if self.note.as_ref().is_none_or(|n| n.read(cx).path() != p.as_path()) => {
                self.load_note(p.clone(), window, cx)
            }
            Selected::Settings => self.ensure_settings(window, cx),
            Selected::Sources => self.ensure_sources(cx),
            _ => {}
        }
        self.selected = to;
        cx.notify();
    }

    pub(crate) fn open_note(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        self.load_note(path.clone(), window, cx);
        self.navigate(Selected::Note(path));
        cx.notify();
    }

    fn load_note(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(n) = &self.note {
            n.update(cx, |n, cx| n.finish_editing(cx));
        }
        let core = self.core.clone();
        let _slow = SlowGuard(std::time::Instant::now(), "NoteView::open");
        let view = cx.new(|cx| NoteView::open(core, &path, window, cx));
        self._subs.push(cx.subscribe_in(&view, window, |this, _, ev: &NoteViewEvent, window, cx| match ev {
            NoteViewEvent::Moved(Some(p)) => {
                // Renamed: back/forward must lead to the new path, not the old one.
                let old = std::mem::replace(&mut this.selected, Selected::Note(p.clone()));
                for s in this.back.iter_mut().chain(this.forward.iter_mut()) {
                    if *s == old {
                        *s = Selected::Note(p.clone());
                    }
                }
                this.reload_notes(cx);
            }
            NoteViewEvent::Moved(None) => {
                this.note = None;
                this.selected = Selected::None;
                this.reload_notes(cx);
            }
            NoteViewEvent::OpenLink(p) => {
                if p.exists() && p.extension().is_some_and(|e| e == "md") {
                    this.open_note(p.clone(), window, cx);
                } else {
                    cx.open_url(&format!("file://{}", p.display()));
                }
            }
        }));
        self.note = Some(view);
    }

    pub(crate) fn new_note(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.core.create_note(None) {
            Ok(p) => {
                self.open_note(p, window, cx);
                self.reload_notes(cx);
            }
            Err(e) => window.push_notification(e.to_string(), cx),
        }
    }

    pub fn toggle_recording(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let _slow = SlowGuard(std::time::Instant::now(), "toggle_recording");
        tracing::info!("toggle_recording: enter");
        let active = self.core.recording_active();
        tracing::info!("toggle_recording: active={active}");
        if active {
            // Returns at once; the transcript is finished and summarized in the background.
            self.core.stop_recording();
            cx.notify();
            return;
        }
        if !self.core.settings().recording_consent_accepted {
            let me = cx.entity().downgrade();
            let core = self.core.clone();
            window.open_alert_dialog(cx, move |alert, _, _| {
                let me = me.clone();
                let core = core.clone();
                alert
                    .title(t("consent-title"))
                    .description(t("consent-body"))
                    .confirm()
                    .ok_text(t("consent-ok"))
                    .cancel_text(t("cancel"))
                    .on_ok(move |_, window, cx| {
                        core.update_settings(|s| s.recording_consent_accepted = true);
                        let _ = me.update(cx, |v, cx| v.start_recording(window, cx));
                        true
                    })
            });
            return;
        }
        self.start_recording(window, cx);
    }

    pub(crate) fn start_recording(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.start_recording_with(None, window, cx);
    }

    fn request_mic_then_start(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        tracing::info!("recording: asking for microphone permission");
        let (tx, rx) = futures::channel::oneshot::channel();
        let tx = parking_lot::Mutex::new(Some(tx));
        ln_platform::request_microphone_permission(move |ok| {
            if let Some(tx) = tx.lock().take() {
                let _ = tx.send(ok);
            }
        });
        cx.spawn_in(window, async move |this, cx| {
            let granted = rx.await.unwrap_or(false);
            tracing::info!("recording: microphone permission answered, granted {granted}");
            // Denied → the next call shows the "not allowed" dialog.
            let _ = this.update_in(cx, |v, window, cx| v.start_recording_with(None, window, cx));
        })
        .detach();
    }

    pub(crate) fn start_recording_with(&mut self, capture: Option<ln_core::CaptureConfig>, window: &mut Window, cx: &mut Context<Self>) {
        tracing::info!("recording: start requested (starting already: {})", self.starting);
        if self.starting {
            return;
        }
        if let Some(n) = &self.note {
            n.update(cx, |n, cx| n.save_now(cx));
        }
        // Microphone permission first (only for real devices).
        if capture.is_none() {
            match ln_platform::microphone_permission() {
                ln_platform::Permission::Denied => {
                    window.open_alert_dialog(cx, |alert, _, _| {
                        alert
                            .title(t("mic-denied-title"))
                            .description(t("mic-denied-body"))
                            .confirm()
                            .ok_text(t("open-system-settings"))
                            .cancel_text(t("cancel"))
                            .on_ok(|_, _, cx| {
                                cx.open_url("x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone");
                                true
                            })
                    });
                    return;
                }
                ln_platform::Permission::NotDetermined => {
                    // macOS asks twice (microphone, then system audio): say so first, so the
                    // second prompt isn't taken for a repeated one.
                    let me = cx.entity().downgrade();
                    window.open_alert_dialog(cx, move |alert, _, _| {
                        let me = me.clone();
                        alert.title(t("perm-title")).description(t("perm-body")).ok_text(t("perm-ok")).on_ok(move |_, window, cx| {
                            let _ = me.update(cx, |v, cx| v.request_mic_then_start(window, cx));
                            true
                        })
                    });
                    return;
                }
                ln_platform::Permission::Granted => {}
            }
        }
        // Loading Whisper and opening the devices takes a moment: do it off the UI thread.
        self.starting = true;
        cx.notify();
        let core = self.core.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result = cx.background_spawn(async move { core.start_recording(capture) }).await;
            let _ = this.update_in(cx, |v, window, cx| {
                v.starting = false;
                match result {
                    Ok(path) => {
                        let core = v.core.clone();
                        let _slow = SlowGuard(std::time::Instant::now(), "RecordingView::new");
                        v.recording = Some(cx.new(|cx| RecordingView::new(core, path, window, cx)));
                        v.navigate(Selected::Recording);
                        v.reload_notes(cx);
                    }
                    Err(e) => window.push_notification(e.to_string(), cx),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn open_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.search.is_some() {
            self.search = None;
            window.focus(&self.focus, cx);
            cx.notify();
            return;
        }
        let core = self.core.clone();
        let s = cx.new(|cx| SearchPanel::new(core, window, cx));
        self._subs.push(cx.subscribe_in(&s, window, |this, _, ev: &SearchEvent, window, cx| match ev {
            SearchEvent::Open(p) => {
                this.search = None;
                if p.extension().is_some_and(|e| e == "md") {
                    this.open_note(p.clone(), window, cx);
                } else {
                    cx.open_url(&format!("file://{}", p.display()));
                }
            }
            SearchEvent::Close => {
                this.search = None;
                window.focus(&this.focus, cx);
                cx.notify();
            }
        }));
        self.search = Some(s);
        cx.notify();
    }

    /// Settings open as a page in the main pane; Back returns to where the user came from.
    pub(crate) fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.ensure_settings(window, cx);
        self.navigate(Selected::Settings);
        cx.notify();
    }

    fn ensure_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings.is_none() {
            let core = self.core.clone();
            self.settings = Some(cx.new(|cx| SettingsView::new(core, window, cx)));
        }
    }

    pub(crate) fn show_sources(&mut self, cx: &mut Context<Self>) {
        self.ensure_sources(cx);
        self.navigate(Selected::Sources);
        cx.notify();
    }

    fn ensure_sources(&mut self, cx: &mut Context<Self>) {
        if self.sources.is_none() {
            let core = self.core.clone();
            self.sources = Some(cx.new(|cx| SourcesView::new(core, cx)));
        }
    }

    // ---- rendering -----------------------------------------------------------------------

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let recording = self.core.recording_info();
        let mut list = v_flex().gap_px();
        let groups = group_notes(&self.notes);
        let cats = self.core.settings().categories;
        if self.notes.is_empty() {
            list = list.child(div().px_3().py_2().text_xs().text_color(theme.muted_foreground).child(t("empty-notes")));
        }
        for (label, items) in groups {
            if items.is_empty() {
                continue;
            }
            list = list.child(div().px_3().pt_3().pb_1().text_xs().text_color(theme.muted_foreground).child(label));
            for n in items {
                list = list.child(self.note_row(n, &cats, cx));
            }
        }
        let record_label = match &recording {
            Some(r) => format!("● {}  {}", t("recording"), fmt_elapsed(r.elapsed)),
            None if self.starting => t("starting").to_string(),
            None => t("record").to_string(),
        };
        let selected_sources = self.selected == Selected::Sources;
        v_flex()
            .w(px(250.))
            .h_full()
            .flex_none()
            .bg(theme.sidebar)
            .border_r_1()
            .border_color(theme.sidebar_border)
            .child(
                v_flex()
                    .p_2()
                    .gap_1()
                    .child(
                        h_flex()
                            .id("search-button")
                            .px_2()
                            .py_1()
                            .gap_2()
                            .rounded_md()
                            .border_1()
                            .border_color(theme.border)
                            .bg(theme.background)
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .cursor_pointer()
                            .child(gpui_kit::component::Icon::new(IconName::Search).small())
                            .child(div().flex_1().child(t("search")))
                            .child(div().text_xs().child("⌘K"))
                            .on_click(cx.listener(|this, _, window, cx| this.open_search(window, cx))),
                    )
                    .child(
                        Button::new("record")
                            .w_full()
                            .when(recording.is_some(), |b| b.danger())
                            .when(recording.is_none(), |b| b.ghost())
                            .icon(IconName::Mic)
                            .label(record_label)
                            .on_click(cx.listener(|this, _, window, cx| {
                                if this.recording.is_some() && this.selected != Selected::Recording {
                                    this.navigate(Selected::Recording);
                                    cx.notify();
                                } else {
                                    this.toggle_recording(window, cx);
                                }
                            })),
                    ),
            )
            .child(
                h_flex()
                    .px_3()
                    .pt_2()
                    .child(div().flex_1().text_xs().font_semibold().text_color(theme.sidebar_foreground).child(t("notes")))
                    .child(
                        Button::new("new-note")
                            .xsmall()
                            .ghost()
                            .icon(IconName::Plus)
                            .tooltip(format!("{} ⌘N", t("new-note")))
                            .on_click(cx.listener(|this, _, window, cx| this.new_note(window, cx))),
                    ),
            )
            .child(div().id("note-list").flex_1().overflow_y_scrollbar().child(list))
            .child(
                v_flex()
                    .p_2()
                    .gap_1()
                    .border_t_1()
                    .border_color(theme.sidebar_border)
                    .children(self.render_setup(cx))
                    .child(
                        h_flex()
                            .id("sources")
                            .px_2()
                            .py_1()
                            .gap_2()
                            .rounded_md()
                            .text_sm()
                            .cursor_pointer()
                            .when(selected_sources, |d| d.bg(theme.sidebar_accent))
                            .child(gpui_kit::component::Icon::new(IconName::Folder).small())
                            .child(div().flex_1().child(t("sources")))
                            .child(
                                Button::new("settings")
                                    .xsmall()
                                    .ghost()
                                    .icon(IconName::Settings)
                                    .tooltip(format!("{} ⌘,", t("settings")))
                                    .on_click(cx.listener(|this, _, window, cx| this.open_settings(window, cx))),
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.show_sources(cx))),
                    )
                    .when_some(self.download, |el, f| {
                        el.child(
                            v_flex()
                                .px_2()
                                .gap_1()
                                .child(div().text_xs().text_color(theme.muted_foreground).child(format!(
                                    "{} {:.0} %",
                                    t("downloading"),
                                    f * 100.0
                                )))
                                .child(Progress::new("download").value(f * 100.0).small()),
                        )
                    }),
            )
    }

    /// Back and forward in the title bar, as in a browser.
    fn render_nav(&self, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .gap_1()
            .child(
                Button::new("nav-back")
                    .ghost()
                    .xsmall()
                    .icon(IconName::ChevronLeft)
                    .tooltip(t("back"))
                    .disabled(self.back.is_empty())
                    .on_click(cx.listener(|this, _, window, cx| this.go_back(window, cx))),
            )
            .child(
                Button::new("nav-forward")
                    .ghost()
                    .xsmall()
                    .icon(IconName::ChevronRight)
                    .tooltip(t("forward"))
                    .disabled(self.forward.is_empty())
                    .on_click(cx.listener(|this, _, window, cx| this.go_forward(window, cx))),
            )
    }

    /// Open setup steps as a short checklist with one action each (SPEC §6.1 rule 6).
    fn render_setup(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        use ln_core::SetupStep::*;
        let steps: Vec<_> = self.core.setup_steps().into_iter().filter(|s| *s != ModelsDownloading).collect();
        if steps.is_empty() {
            return None;
        }
        let theme = cx.theme().clone();
        let mut col = v_flex().gap_1().p_2().mb_1().rounded_md().border_1().border_color(theme.border).bg(theme.background);
        col = col.child(div().text_xs().font_semibold().child(t("setup")));
        // 0 = open settings, 1 = macOS privacy settings, 2 = move roles to the ChatGPT plan.
        let chatgpt_ok = matches!(self.core.chatgpt_status(), ln_core::ChatGptStatus::SignedIn { plan_usage: true, .. });
        for (i, step) in steps.iter().enumerate() {
            let (text, actions): (String, Vec<(&str, u8)>) = match step {
                KeyMissing { provider, roles } => {
                    let names: Vec<&str> = roles.iter().map(|r| role_name(*r)).collect();
                    let roles_text = match names.as_slice() {
                        [] => String::new(),
                        [one] => one.to_string(),
                        [rest @ .., last] => format!("{} {} {last}", rest.join(", "), t("and")),
                    };
                    let mut a = vec![(t("setup-key-action"), 0)];
                    if chatgpt_ok {
                        a.insert(0, (t("setup-use-chatgpt"), 2));
                    }
                    (t("setup-key").replace("{roles}", &roles_text).replace("{provider}", provider), a)
                }
                ChatGptSignIn => (t("setup-chatgpt").to_string(), vec![(t("setup-chatgpt-action"), 0)]),
                MicrophoneDenied => (t("setup-mic").to_string(), vec![(t("open-system-settings"), 1)]),
                TranscriptionModelMissing => (t("setup-model").to_string(), vec![]),
                LlamaServerMissing => (t("setup-llama").to_string(), vec![]),
                ModelsDownloading => continue,
            };
            let mut row = v_flex().gap_1().child(div().text_xs().text_color(theme.muted_foreground).child(text));
            let mut buttons = h_flex().gap_1().flex_wrap();
            for (k, (label, kind)) in actions.into_iter().enumerate() {
                buttons = buttons.child(Button::new(("setup", i * 4 + k)).xsmall().label(label).on_click(cx.listener(
                    move |this, _, window, cx| match kind {
                        0 => this.open_settings(window, cx),
                        2 => {
                            this.core.use_chatgpt_for_unready_roles();
                            if let Some(s) = &this.settings {
                                s.update(cx, |s, cx| s.chatgpt_changed(false, window, cx));
                            }
                            cx.notify();
                        }
                        _ => cx.open_url("x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone"),
                    },
                )));
            }
            row = row.child(buttons);
            col = col.child(row);
        }
        Some(col.into_any_element())
    }

    fn note_row(&self, n: &NoteEntry, cats: &ln_types::Categories, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let selected = matches!(&self.selected, Selected::Note(p) if p == &n.path);
        let path = n.path.clone();
        let id = SharedString::from(n.path.to_string_lossy().into_owned());
        // Overlapping coloured dots like Finder tags; only categories that still exist.
        let dots: Vec<AnyElement> = n
            .categories
            .iter()
            .filter_map(|c| cats.color_of(c))
            .enumerate()
            .map(|(i, color)| {
                div()
                    .size(px(9.))
                    .rounded_full()
                    .border_1()
                    .border_color(theme.sidebar)
                    .bg(crate::theme::tag_color(Some(color)))
                    .when(i > 0, |d| d.ml(px(-3.)))
                    .into_any_element()
            })
            .collect();
        h_flex()
            .id(ElementId::Name(id))
            .mx_2()
            .px_2()
            .py_1()
            .gap_2()
            .rounded_md()
            .text_sm()
            .cursor_pointer()
            .when(selected, |d| d.bg(theme.sidebar_accent).text_color(theme.sidebar_accent_foreground))
            .child(div().flex_1().overflow_hidden().text_ellipsis().whitespace_nowrap().child(n.title.clone()))
            .when_some(self.core.activity(&n.path), |d, a| {
                let label = match a {
                    ln_core::Activity::Connecting => t("activity-short-connecting"),
                    ln_core::Activity::Finishing => t("activity-short-finishing"),
                    ln_core::Activity::Summarizing => t("activity-short-summarizing"),
                };
                d.child(
                    h_flex()
                        .gap_1()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(gpui_kit::component::spinner::Spinner::new().xsmall())
                        .child(label),
                )
            })
            .when(!dots.is_empty(), |d| d.child(h_flex().flex_none().children(dots)))
            .when(!n.languages.is_empty(), |d| d.child(div().text_xs().text_color(theme.muted_foreground).child(n.languages.join(" "))))
            .on_click(cx.listener(move |this, ev: &ClickEvent, window, cx| {
                this.open_note(path.clone(), window, cx);
                // Double-click: rename, as in the Finder.
                if ev.click_count() >= 2
                    && let Some(note) = &this.note
                {
                    note.update(cx, |n, cx| n.start_rename(window, cx));
                }
            }))
    }

    fn render_main(&self, cx: &mut Context<Self>) -> AnyElement {
        match &self.selected {
            Selected::Recording if self.recording.is_some() => self.recording.clone().unwrap().into_any_element(),
            Selected::Note(_) if self.note.is_some() => self.note.clone().unwrap().into_any_element(),
            Selected::Sources if self.sources.is_some() => self.sources.clone().unwrap().into_any_element(),
            Selected::Settings if self.settings.is_some() => self.settings.clone().unwrap().into_any_element(),
            _ => {
                let theme = cx.theme().clone();
                v_flex()
                    .size_full()
                    .items_center()
                    .justify_center()
                    .gap_3()
                    .text_color(theme.muted_foreground)
                    .child(if self.notes.is_empty() { t("empty-notes") } else { t("empty-select") })
                    .when(self.notes.is_empty(), |el| {
                        el.child(
                            h_flex()
                                .gap_2()
                                .child(
                                    Button::new("empty-record")
                                        .primary()
                                        .icon(IconName::Mic)
                                        .label(t("record"))
                                        .on_click(cx.listener(|this, _, window, cx| this.toggle_recording(window, cx))),
                                )
                                .child(
                                    Button::new("empty-add")
                                        .label(t("add-folder"))
                                        .on_click(cx.listener(|this, _, _, cx| this.show_sources(cx))),
                                ),
                        )
                    })
                    .into_any_element()
            }
        }
    }
}

/// Logs UI work that takes long enough to be felt.
pub(crate) struct SlowGuard(pub std::time::Instant, pub &'static str);
impl Drop for SlowGuard {
    fn drop(&mut self) {
        if self.0.elapsed() > Duration::from_millis(30) {
            tracing::warn!("slow UI: {} took {:?}", self.1, self.0.elapsed());
        }
    }
}

fn role_name(r: ln_types::ChatRole) -> &'static str {
    match r {
        ln_types::ChatRole::Assistant => t("role-assistant"),
        ln_types::ChatRole::Suggestions => t("role-suggestions"),
        ln_types::ChatRole::Summary => t("role-summary"),
    }
}

fn fmt_elapsed(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60) } else { format!("{:02}:{:02}", s / 60, s % 60) }
}

/// Today / yesterday / this week / earlier, then user folders.
fn group_notes(notes: &[NoteEntry]) -> Vec<(String, Vec<&NoteEntry>)> {
    let today = Local::now().date_naive();
    let mut g: Vec<(String, Vec<&NoteEntry>)> =
        vec![(t("today").into(), vec![]), (t("yesterday").into(), vec![]), (t("this-week").into(), vec![]), (t("earlier").into(), vec![])];
    let mut folders: std::collections::BTreeMap<String, Vec<&NoteEntry>> = Default::default();
    for n in notes {
        let is_date_folder = n.folder.split('/').next().is_some_and(|f| f.len() == 4 && f.chars().all(|c| c.is_ascii_digit()));
        if !n.folder.is_empty() && !is_date_folder {
            folders.entry(n.folder.clone()).or_default().push(n);
            continue;
        }
        let ts = n.created.unwrap_or(n.modified);
        let d = Local.timestamp_opt(ts, 0).single().map(|d| d.date_naive()).unwrap_or(today);
        let days = (today - d).num_days();
        let i = match days {
            0 => 0,
            1 => 1,
            2..=6 if d.iso_week() == today.iso_week() => 2,
            _ => 3,
        };
        g[i].1.push(n);
    }
    for (f, v) in folders {
        g.push((format!("{f}/"), v));
    }
    g
}

impl Focusable for AppView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for AppView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t0 = std::time::Instant::now();
        let _slow = SlowGuard(t0, "AppView::render");
        if let Some(fr) = &self.first_run {
            return v_flex().size_full().bg(cx.theme().background).child(TitleBar::new()).child(fr.clone()).into_any_element();
        }
        let _ = window;
        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .key_context("LiveNotes")
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &NewNote, window, cx| this.new_note(window, cx)))
            .on_action(cx.listener(|this, _: &ToggleRecording, window, cx| this.toggle_recording(window, cx)))
            .on_action(cx.listener(|this, _: &OpenSearch, window, cx| this.open_search(window, cx)))
            .on_action(cx.listener(|this, _: &OpenSettings, window, cx| this.open_settings(window, cx)))
            .on_action(cx.listener(|this, _: &GoBack, window, cx| this.go_back(window, cx)))
            .on_action(cx.listener(|this, _: &GoForward, window, cx| this.go_forward(window, cx)))
            .on_action(cx.listener(|this, _: &ShowSources, _, cx| this.show_sources(cx)))
            .on_action(cx.listener(|this, _: &ToggleSidebar, _, cx| {
                this.sidebar_open = !this.sidebar_open;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ToggleEdit, window, cx| {
                if let Some(n) = &this.note
                    && matches!(this.selected, Selected::Note(_))
                {
                    n.update(cx, |n, cx| n.toggle_mode(window, cx));
                }
            }))
            .on_action(cx.listener(|_, _: &CloseWindow, window, _| window.remove_window()))
            .child(TitleBar::new().child(self.render_nav(cx)))
            .child(
                h_flex()
                    .flex_1()
                    .w_full()
                    .overflow_hidden()
                    .when(self.sidebar_open, |el| el.child(self.render_sidebar(cx)))
                    .child(div().flex_1().min_w(px(0.)).h_full().overflow_hidden().child(self.render_main(cx))),
            )
            .when_some(self.search.clone(), |el, s| {
                el.child(div().absolute().top(px(64.)).left_0().right_0().flex().justify_center().child(div().w(px(640.)).child(s)))
            })
            .into_any_element()
    }
}

#[cfg(feature = "screenshots")]
impl AppView {
    pub(crate) fn first_run_view(&self) -> Option<Entity<FirstRunView>> {
        self.first_run.clone()
    }
    pub(crate) fn note_view(&self) -> Option<Entity<NoteView>> {
        self.note.clone()
    }
    pub(crate) fn search_panel(&self) -> Option<Entity<SearchPanel>> {
        self.search.clone()
    }
    /// Note list as shown in the sidebar: title and categories.
    pub(crate) fn note_list(&self) -> Vec<(String, Vec<String>)> {
        self.notes.iter().map(|n| (n.title.clone(), n.categories.clone())).collect()
    }
    /// What the main pane shows, for the tour's checks.
    pub(crate) fn page(&self) -> String {
        match &self.selected {
            Selected::Note(p) => format!("Note {}", p.display()),
            other => format!("{other:?}"),
        }
    }
}
