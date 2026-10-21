//! Main window (SPEC §6.2): sidebar, the open note or recording, search overlay.

use crate::first_run::{FirstRunEvent, FirstRunView};
use crate::i18n::t;
use crate::note_view::{NoteView, NoteViewEvent};
use crate::recording_view::RecordingView;
use crate::search::{SearchEvent, SearchPanel};
use crate::sources_view::SourcesView;
use crate::{CloseWindow, NewNote, OpenSearch, OpenSettings, ShowSources, ToggleEdit, ToggleRecording, ToggleSidebar};
use chrono::{Datelike, Local, TimeZone};
use futures::StreamExt as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::progress::Progress;
use gpui_kit::component::{
    ActiveTheme as _, IconName, Sizable as _, StyledExt as _, TitleBar, WindowExt as _, h_flex, scroll::ScrollableElement as _, v_flex,
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
    settings: Option<WeakEntity<crate::settings_view::SettingsView>>,
    download: Option<f32>,
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
            subs.push(cx.subscribe_in(&fr, window, |this, _, ev: &FirstRunEvent, _w, cx| {
                let FirstRunEvent::Done = ev;
                this.first_run = None;
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
            download: None,
            sidebar_open: true,
            focus: cx.focus_handle(),
            _subs: subs,
            _tasks: vec![events_task, tick],
        };
        v.reload_notes(cx);
        window.focus(&v.focus, cx);
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
                if let Some(s) = self.settings.as_ref().and_then(|w| w.upgrade()) {
                    s.update(cx, |_, cx| cx.notify());
                }
            }
            CoreEvent::SourcesChanged | CoreEvent::IndexProgress { .. } => {
                if let Some(s) = &self.sources {
                    s.update(cx, |s, cx| s.refresh(cx));
                }
            }
            CoreEvent::ProviderTested { provider, result } => {
                if let Some(s) = self.settings.as_ref().and_then(|w| w.upgrade()) {
                    s.update(cx, |s, cx| s.provider_tested(&provider, result, cx));
                }
            }
            CoreEvent::SettingsChanged => {
                crate::i18n::set_language(self.core.ui_language());
                cx.notify();
            }
            CoreEvent::Notice(msg) => window.push_notification(msg, cx),
        }
        cx.notify();
    }

    pub(crate) fn open_note(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(n) = &self.note {
            n.update(cx, |n, cx| n.save_now(cx));
        }
        let core = self.core.clone();
        let view = cx.new(|cx| NoteView::open(core, &path, window, cx));
        self._subs.push(cx.subscribe_in(&view, window, |this, _, ev: &NoteViewEvent, window, cx| match ev {
            NoteViewEvent::Moved(Some(p)) => {
                this.selected = Selected::Note(p.clone());
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
        self.selected = Selected::Note(path);
        cx.notify();
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
        if self.core.recording_active() {
            let core = self.core.clone();
            cx.background_spawn(async move {
                core.stop_recording();
            })
            .detach();
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

    pub(crate) fn start_recording_with(&mut self, capture: Option<ln_core::CaptureConfig>, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(n) = &self.note {
            n.update(cx, |n, cx| n.save_now(cx));
        }
        match self.core.start_recording(capture) {
            Ok(path) => {
                let core = self.core.clone();
                self.recording = Some(cx.new(|cx| RecordingView::new(core, path, window, cx)));
                self.selected = Selected::Recording;
                self.reload_notes(cx);
            }
            Err(e) => window.push_notification(e.to_string(), cx),
        }
        cx.notify();
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

    pub(crate) fn open_settings(&mut self, cx: &mut Context<Self>) {
        if let Some(w) = self.settings.as_ref().and_then(|w| w.upgrade()) {
            let _ = w;
            return;
        }
        let core = self.core.clone();
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(640.), px(720.)), cx))),
            titlebar: Some(TitlebarOptions { title: Some(t("settings").into()), ..Default::default() }),
            ..Default::default()
        };
        let slot: std::rc::Rc<std::cell::RefCell<Option<WeakEntity<crate::settings_view::SettingsView>>>> = Default::default();
        let slot2 = slot.clone();
        let _ = gpui_kit::open_window(options, cx, move |window, cx| {
            let v = cx.new(|cx| crate::settings_view::SettingsView::new(core.clone(), window, cx));
            *slot2.borrow_mut() = Some(v.downgrade());
            v
        });
        self.settings = slot.borrow().clone();
    }

    pub(crate) fn show_sources(&mut self, cx: &mut Context<Self>) {
        if self.sources.is_none() {
            let core = self.core.clone();
            self.sources = Some(cx.new(|cx| SourcesView::new(core, cx)));
        }
        self.selected = Selected::Sources;
        cx.notify();
    }

    // ---- rendering -----------------------------------------------------------------------

    fn render_sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let recording = self.core.recording_info();
        let mut list = v_flex().gap_px();
        let groups = group_notes(&self.notes);
        if self.notes.is_empty() {
            list = list.child(div().px_3().py_2().text_xs().text_color(theme.muted_foreground).child(t("empty-notes")));
        }
        for (label, items) in groups {
            if items.is_empty() {
                continue;
            }
            list = list.child(div().px_3().pt_3().pb_1().text_xs().text_color(theme.muted_foreground).child(label));
            for n in items {
                list = list.child(self.note_row(n, cx));
            }
        }
        let record_label = match &recording {
            Some(r) => format!("● {}  {}", t("recording"), fmt_elapsed(r.elapsed)),
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
                                    this.selected = Selected::Recording;
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
                            .child(t("sources"))
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

    fn note_row(&self, n: &NoteEntry, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let selected = matches!(&self.selected, Selected::Note(p) if p == &n.path);
        let path = n.path.clone();
        let id = SharedString::from(n.path.to_string_lossy().into_owned());
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
            .when(!n.languages.is_empty(), |d| d.child(div().text_xs().text_color(theme.muted_foreground).child(n.languages.join(" "))))
            .on_click(cx.listener(move |this, _, window, cx| this.open_note(path.clone(), window, cx)))
    }

    fn render_main(&self, cx: &mut Context<Self>) -> AnyElement {
        match &self.selected {
            Selected::Recording if self.recording.is_some() => self.recording.clone().unwrap().into_any_element(),
            Selected::Note(_) if self.note.is_some() => self.note.clone().unwrap().into_any_element(),
            Selected::Sources if self.sources.is_some() => self.sources.clone().unwrap().into_any_element(),
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
            .on_action(cx.listener(|this, _: &OpenSettings, _, cx| this.open_settings(cx)))
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
            .child(TitleBar::new())
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
    pub(crate) fn close_first_run(&mut self, cx: &mut Context<Self>) {
        self.first_run = None;
        cx.notify();
    }
    pub(crate) fn note_view(&self) -> Option<Entity<NoteView>> {
        self.note.clone()
    }
    pub(crate) fn search_panel(&self) -> Option<Entity<SearchPanel>> {
        self.search.clone()
    }
}
