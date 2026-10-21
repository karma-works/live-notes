//! ⌘K: one field for search and questions (SPEC §5 F5, F6, §6.2).

use crate::i18n::t;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::text::TextView;
use gpui_kit::component::{ActiveTheme as _, IconName, Sizable as _, StyledExt as _, h_flex, scroll::ScrollableElement as _, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use ln_core::{AskScope, Citation, Core, CoreEvent, SearchFilter, SearchHit};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

pub enum SearchEvent {
    Open(PathBuf),
    Close,
}
impl EventEmitter<SearchEvent> for SearchPanel {}

struct Answer {
    id: u64,
    question: String,
    text: String,
    done: bool,
    citations: Vec<Citation>,
}

pub struct SearchPanel {
    core: Arc<Core>,
    input: Entity<InputState>,
    hits: Vec<SearchHit>,
    answer: Option<Answer>,
    history: Vec<(String, String)>,
    searched: bool,
    pending: Option<Task<()>>,
    _subs: Vec<Subscription>,
}

impl SearchPanel {
    pub fn new(core: Arc<Core>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder(t("search-placeholder")));
        let sub = cx.subscribe_in(&input, window, |this, state, ev: &InputEvent, _w, cx| match ev {
            InputEvent::Change => {
                let q = state.read(cx).value().to_string();
                this.search(q, cx);
            }
            InputEvent::PressEnter { secondary, .. } => {
                let q = state.read(cx).value().to_string();
                if *secondary || q.trim_end().ends_with('?') || q.trim_end().ends_with('？') {
                    this.ask(q, cx);
                } else if let Some(h) = this.hits.first() {
                    cx.emit(SearchEvent::Open(h.path.clone()));
                }
            }
            _ => {}
        });
        input.update(cx, |i, cx| i.focus(window, cx));
        SearchPanel { core, input, hits: vec![], answer: None, history: vec![], searched: false, pending: None, _subs: vec![sub] }
    }

    fn search(&mut self, q: String, cx: &mut Context<Self>) {
        let core = self.core.clone();
        self.pending = Some(cx.spawn(async move |this, cx: &mut AsyncApp| {
            cx.background_executor().timer(Duration::from_millis(150)).await;
            let hits = cx.background_spawn(async move { core.search_files(&q, &SearchFilter::default(), 12) }).await;
            let _ = this.update(cx, |s, cx| {
                s.hits = hits;
                s.searched = true;
                cx.notify();
            });
        }));
    }

    fn ask(&mut self, q: String, cx: &mut Context<Self>) {
        if q.trim().is_empty() {
            return;
        }
        if let Some(a) = self.answer.take()
            && a.done
        {
            self.history.push((a.question, a.text));
        }
        let scope = if self.core.recording_active() { AskScope::CurrentRecording } else { AskScope::All };
        let id = self.core.ask(&q, scope, self.history.clone());
        self.answer = Some(Answer { id, question: q, text: String::new(), done: false, citations: vec![] });
        cx.notify();
    }

    pub fn on_event(&mut self, ev: &CoreEvent, cx: &mut Context<Self>) {
        if let CoreEvent::Answer { id, text, done, citations } = ev
            && let Some(a) = self.answer.as_mut()
            && a.id == *id
        {
            a.text = text.clone();
            a.done = *done;
            a.citations = citations.clone();
            cx.notify();
        }
    }
}

impl Render for SearchPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let mut body = v_flex().gap_1();
        if let Some(a) = &self.answer {
            let mut box_ = v_flex().gap_2().p_3().rounded_md().bg(theme.muted.opacity(0.4));
            box_ = box_.child(div().text_xs().text_color(theme.muted_foreground).child(a.question.clone()));
            box_ = box_.child(if a.text.is_empty() && !a.done {
                div().text_sm().text_color(theme.muted_foreground).child("…").into_any_element()
            } else {
                TextView::markdown(("answer", a.id as usize), a.text.clone()).into_any_element()
            });
            for c in &a.citations {
                let p = c.path.clone();
                box_ = box_.child(
                    h_flex()
                        .id(("cite", c.n))
                        .gap_2()
                        .text_xs()
                        .cursor_pointer()
                        .text_color(theme.link)
                        .child(format!("[{}]", c.n))
                        .child(c.title.clone())
                        .on_click(cx.listener(move |_, _, _, cx| cx.emit(SearchEvent::Open(p.clone())))),
                );
            }
            body = body.child(box_);
        }
        if self.searched && self.hits.is_empty() && self.answer.is_none() {
            body = body.child(div().px_2().py_2().text_sm().text_color(theme.muted_foreground).child(t("no-results")));
        }
        for (i, h) in self.hits.iter().enumerate() {
            let p = h.path.clone();
            let rel = h
                .path
                .strip_prefix(self.core.notes_dir())
                .map(|x| x.to_string_lossy().into_owned())
                .unwrap_or_else(|_| h.path.to_string_lossy().into_owned());
            body = body.child(
                v_flex()
                    .id(("hit", i))
                    .px_2()
                    .py_2()
                    .rounded_md()
                    .cursor_pointer()
                    .hover(|d| d.bg(theme.list_hover))
                    .child(h_flex().gap_2().child(div().text_sm().font_semibold().child(h.title.clone())).child(
                        div().text_xs().text_color(theme.muted_foreground).overflow_hidden().text_ellipsis().whitespace_nowrap().child(rel),
                    ))
                    .child(div().text_xs().text_color(theme.muted_foreground).line_clamp(2).child(h.snippet.clone()))
                    .on_click(cx.listener(move |_, _, _, cx| cx.emit(SearchEvent::Open(p.clone())))),
            );
        }
        v_flex()
            .p_2()
            .gap_2()
            .rounded_lg()
            .border_1()
            .border_color(theme.border)
            .bg(theme.popover)
            .shadow_lg()
            .max_h(px(560.))
            .capture_key_down(cx.listener(|_, ev: &KeyDownEvent, _, cx| {
                if ev.keystroke.key == "escape" {
                    cx.emit(SearchEvent::Close);
                }
            }))
            .child(Input::new(&self.input).prefix(gpui_kit::component::Icon::new(IconName::Search).small()).cleanable(true))
            .child(div().id("search-results").max_h(px(440.)).overflow_y_scrollbar().child(body))
            .child(
                h_flex()
                    .px_2()
                    .gap_3()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child("↩ ".to_string() + t("open"))
                    .child("⌘↩ ".to_string() + t("ask"))
                    .when(self.core.recording_active(), |el| el.child(t("recording"))),
            )
    }
}

#[cfg(feature = "screenshots")]
impl SearchPanel {
    pub(crate) fn hit_count(&self) -> usize {
        self.hits.len()
    }

    pub(crate) fn set_query(&mut self, q: &str, window: &mut Window, cx: &mut Context<Self>) {
        let q = q.to_string();
        self.input.update(cx, |i, cx| i.set_value(q.clone(), window, cx));
        self.search(q, cx);
    }
}
