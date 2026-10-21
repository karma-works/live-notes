//! One open note: reading view and Markdown editor, auto-save, conflict handling (SPEC §5 F10).

use crate::i18n::t;
use crate::markdown::{Block, blocks};
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Editor, EditorState, InputEvent};
use gpui_kit::component::text::TextView;
use gpui_kit::component::{
    ActiveTheme as _, IconName, Sizable as _, StyledExt as _, WindowExt as _, h_flex, scroll::ScrollableElement as _, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use ln_core::Core;
use ln_notes::{NoteFile, SaveOutcome};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Read,
    Edit,
}

pub enum NoteViewEvent {
    /// The file got a new name (auto-name from the first heading, or deleted → None).
    Moved(Option<PathBuf>),
    OpenLink(PathBuf),
}

impl EventEmitter<NoteViewEvent> for NoteView {}

pub struct NoteView {
    core: Arc<Core>,
    file: NoteFile,
    text: String,
    mode: Mode,
    editor: Entity<EditorState>,
    blocks: Vec<Block>,
    dirty: bool,
    conflict: bool,
    additions: Vec<String>,
    save_task: Option<Task<()>>,
    focus: FocusHandle,
    _subs: Vec<Subscription>,
}

impl NoteView {
    pub fn open(core: Arc<Core>, path: &Path, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let file = NoteFile::open(path).unwrap_or_else(|_| NoteFile::new(path, String::new()));
        let text = file.text.clone();
        let editor =
            cx.new(|cx| EditorState::new(window, cx).language("markdown").soft_wrap(true).line_number(false).default_value(text.clone()));
        let sub = cx.subscribe_in(&editor, window, |this, state, ev: &InputEvent, _w, cx| {
            if matches!(ev, InputEvent::Change) {
                let v = state.read(cx).value().to_string();
                if v != this.text {
                    this.text = v;
                    this.dirty = true;
                    this.schedule_save(cx);
                }
            }
        });
        // An empty new note starts in the editor.
        let mode = if text.trim() == "#" || text.trim().is_empty() { Mode::Edit } else { Mode::Read };
        let additions = core.pending_additions(path);
        let mut v = NoteView {
            core,
            blocks: blocks(&text),
            file,
            text,
            mode,
            editor,
            dirty: false,
            conflict: false,
            additions,
            save_task: None,
            focus: cx.focus_handle(),
            _subs: vec![sub],
        };
        if mode == Mode::Edit {
            v.focus_editor(window, cx);
        }
        v
    }

    #[cfg(feature = "screenshots")]
    pub fn is_editing(&self) -> bool {
        self.mode == Mode::Edit
    }

    pub fn path(&self) -> &Path {
        &self.file.path
    }

    fn focus_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editor.update(cx, |e, cx| e.focus(window, cx));
    }

    pub fn toggle_mode(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.mode {
            Mode::Read => {
                self.mode = Mode::Edit;
                self.focus_editor(window, cx);
            }
            Mode::Edit => {
                self.save_now(cx);
                self.blocks = blocks(&self.text);
                self.mode = Mode::Read;
                window.focus(&self.focus, cx);
            }
        }
        cx.notify();
    }

    /// Saves 500 ms after the last keystroke (SPEC §5 F10).
    fn schedule_save(&mut self, cx: &mut Context<Self>) {
        self.save_task = Some(cx.spawn(async move |this, cx: &mut AsyncApp| {
            cx.background_executor().timer(Duration::from_millis(500)).await;
            let _ = this.update(cx, |v, cx| v.save_now(cx));
        }));
    }

    pub fn save_now(&mut self, cx: &mut Context<Self>) {
        if !self.dirty || self.conflict {
            return;
        }
        self.file.text = self.text.clone();
        match self.file.save(false) {
            Ok(SaveOutcome::Saved) => {
                self.dirty = false;
                if let Some(new) = self.core.auto_name_note(&self.file.path) {
                    self.file.path = new.clone();
                    let _ = self.file.reload();
                    cx.emit(NoteViewEvent::Moved(Some(new)));
                }
            }
            Ok(SaveOutcome::Conflict) => self.conflict = true,
            Err(e) => tracing::error!("save {}: {e}", self.file.path.display()),
        }
        cx.notify();
    }

    /// The file changed on disk (watcher or the app's own background writes).
    pub fn file_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.file.changed_on_disk() {
            return;
        }
        if self.dirty {
            self.conflict = true;
        } else {
            self.reload(window, cx);
        }
        self.additions = self.core.pending_additions(&self.file.path);
        cx.notify();
    }

    fn reload(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.file.reload().is_err() {
            cx.emit(NoteViewEvent::Moved(None));
            return;
        }
        self.text = self.file.text.clone();
        self.blocks = blocks(&self.text);
        let text = self.text.clone();
        self.editor.update(cx, |e, cx| e.set_value(text, window, cx));
        self.dirty = false;
        self.conflict = false;
    }

    fn keep_mine(&mut self, cx: &mut Context<Self>) {
        self.file.text = self.text.clone();
        let _ = self.file.save(true);
        self.dirty = false;
        self.conflict = false;
        cx.notify();
    }

    fn toggle_task(&mut self, line: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.save_now(cx);
        if self.core.toggle_task(&self.file.path, line).is_ok() {
            self.reload(window, cx);
        }
        cx.notify();
    }

    fn delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let path = self.file.path.clone();
        let core = self.core.clone();
        let me = cx.entity().downgrade();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let path = path.clone();
            let core = core.clone();
            let me = me.clone();
            alert
                .title(t("delete-note"))
                .description(t("delete-note-body"))
                .confirm()
                .ok_text(t("delete"))
                .cancel_text(t("cancel"))
                .ok_variant(gpui_kit::component::button::ButtonVariant::Danger)
                .on_ok(move |_, _, cx| {
                    let _ = core.delete_note(&path);
                    let _ = me.update(cx, |_, cx| cx.emit(NoteViewEvent::Moved(None)));
                    true
                })
        });
    }

    fn render_header(&self, cx: &Context<Self>) -> impl IntoElement {
        let folder = self
            .file
            .path
            .parent()
            .and_then(|p| p.strip_prefix(self.core.notes_dir()).ok())
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let label = if self.mode == Mode::Read { t("edit") } else { t("read") };
        h_flex()
            .px_6()
            .py_2()
            .gap_2()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(div().flex_1().text_xs().text_color(cx.theme().muted_foreground).child(folder))
            .child(
                Button::new("mode")
                    .small()
                    .ghost()
                    .label(label)
                    .tooltip("⌘E")
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_mode(window, cx))),
            )
            .child(
                Button::new("reveal")
                    .small()
                    .ghost()
                    .icon(IconName::FolderOpen)
                    .tooltip(t("reveal"))
                    .on_click(cx.listener(|this, _, _, cx| cx.reveal_path(&this.file.path))),
            )
            .child(
                Button::new("delete")
                    .small()
                    .ghost()
                    .icon(IconName::Delete)
                    .tooltip(t("delete"))
                    .on_click(cx.listener(|this, _, window, cx| this.delete(window, cx))),
            )
    }

    fn render_conflict(&self, cx: &Context<Self>) -> impl IntoElement {
        h_flex()
            .px_6()
            .py_2()
            .gap_3()
            .bg(cx.theme().warning.opacity(0.15))
            .child(div().flex_1().text_sm().child(t("conflict")))
            .child(Button::new("mine").small().label(t("keep-mine")).on_click(cx.listener(|this, _, _, cx| this.keep_mine(cx))))
            .child(Button::new("theirs").small().label(t("take-theirs")).on_click(cx.listener(|this, _, window, cx| {
                this.reload(window, cx);
                cx.notify();
            })))
    }

    fn render_reading(&self, cx: &Context<Self>) -> impl IntoElement {
        let dir = self.file.path.parent().map(Path::to_path_buf).unwrap_or_default();
        let mut col = v_flex().w_full().max_w(px(720.)).mx_auto().px_8().py_6().gap_2();
        if !self.additions.is_empty() {
            let mut b = v_flex().gap_1().p_3().rounded_md().border_1().border_color(cx.theme().border);
            b = b.child(div().text_xs().text_color(cx.theme().muted_foreground).child(t("suggestion")));
            for (i, a) in self.additions.iter().enumerate() {
                b = b.child(
                    h_flex()
                        .gap_2()
                        .child(div().flex_1().text_sm().child(a.clone()))
                        .child(Button::new(("acc", i)).xsmall().label(t("accept")).on_click(cx.listener(move |this, _, window, cx| {
                            let _ = this.core.accept_addition(&this.file.path, i);
                            this.additions = this.core.pending_additions(&this.file.path);
                            this.reload(window, cx);
                            cx.notify();
                        })))
                        .child(Button::new(("dis", i)).xsmall().ghost().label(t("dismiss")).on_click(cx.listener(
                            move |this, _, _, cx| {
                                this.core.dismiss_addition(&this.file.path, i);
                                this.additions = this.core.pending_additions(&this.file.path);
                                cx.notify();
                            },
                        ))),
                );
            }
            col = col.child(b);
        }
        for (i, block) in self.blocks.iter().enumerate() {
            col = col.child(match block {
                Block::Header { date, duration, languages, labels } => {
                    let mut parts: Vec<String> = Vec::new();
                    parts.extend(date.clone());
                    parts.extend(duration.clone());
                    if !languages.is_empty() {
                        parts.push(languages.join(" · "));
                    }
                    parts.extend(labels.iter().map(|l| format!("#{l}")));
                    div().text_xs().text_color(cx.theme().muted_foreground).child(parts.join("   ")).into_any_element()
                }
                Block::Markdown(md) => {
                    let base = dir.clone();
                    let me = cx.entity().downgrade();
                    TextView::markdown(("md", i), md.clone())
                        .on_link_click(move |url, _ev, _w, cx| {
                            let u = url.to_string();
                            if u.contains("://") {
                                cx.open_url(&u);
                            } else {
                                let p = base.join(u.trim_start_matches("./"));
                                let _ = me.update(cx, |_, cx| cx.emit(NoteViewEvent::OpenLink(p)));
                            }
                        })
                        .into_any_element()
                }
                Block::Task { line, checked, text, indent } => {
                    let line = *line;
                    h_flex()
                        .pl(px(*indent as f32 * 8.))
                        .gap_2()
                        .items_start()
                        .child(
                            div().pt(px(3.)).child(
                                Checkbox::new(("task", line))
                                    .checked(*checked)
                                    .on_change(cx.listener(move |this, _: &bool, window, cx| this.toggle_task(line, window, cx))),
                            ),
                        )
                        .child(
                            div()
                                .flex_1()
                                .when(*checked, |d| d.text_color(cx.theme().muted_foreground).line_through())
                                .child(TextView::markdown(("task-text", line), text.clone())),
                        )
                        .into_any_element()
                }
                Block::TranscriptHeading(h) => div().mt_4().text_lg().font_semibold().child(h.clone()).into_any_element(),
                Block::Transcript(lines) => {
                    let mut v = v_flex().gap_2();
                    for (j, (l, ch)) in lines.iter().enumerate() {
                        let color = ch.map(|c| crate::theme::channel_color(c, cx)).unwrap_or(cx.theme().foreground);
                        v = v.child(
                            v_flex()
                                .id(("tl", j))
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .text_xs()
                                        .child(div().text_color(cx.theme().muted_foreground).child(ln_notes::format_timestamp(l.at_ms)))
                                        .child(div().text_color(color).child(l.label.clone()))
                                        .child(div().text_color(cx.theme().muted_foreground).child(l.lang.to_string())),
                                )
                                .child(div().text_sm().child(l.text.clone())),
                        );
                    }
                    v.into_any_element()
                }
            });
        }
        div().id("reading").size_full().overflow_y_scrollbar().child(col)
    }
}

impl Focusable for NoteView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for NoteView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .track_focus(&self.focus)
            .child(self.render_header(cx))
            .when(self.conflict, |el| el.child(self.render_conflict(cx)))
            .child(match self.mode {
                Mode::Read => self.render_reading(cx).into_any_element(),
                Mode::Edit => div()
                    .flex_1()
                    .size_full()
                    .px_6()
                    .py_4()
                    .child(Editor::new(&self.editor).appearance(false).h(relative(1.)))
                    .into_any_element(),
            })
    }
}
