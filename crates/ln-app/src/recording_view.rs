//! The running recording (SPEC §6.2): own notes on top, live transcript below, one main
//! action ("Stop") in the bar, live suggestions on the right (§5 F8).

use crate::i18n::t;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::text::TextView;
use gpui_kit::component::{ActiveTheme as _, IconName, Sizable as _, StyledExt as _, h_flex, scroll::ScrollableElement as _, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use ln_core::{Core, CoreEvent, SuggestionCard};
use ln_types::{Lang, Segment};
use std::path::{Path, PathBuf};
use std::sync::Arc;

struct Card {
    card: SuggestionCard,
    answer: Option<String>,
}

pub struct RecordingView {
    core: Arc<Core>,
    note_path: PathBuf,
    segments: Vec<Segment>,
    notes: Entity<TextareaState>,
    cards: Vec<Card>,
    lang_edit: Option<usize>,
    scroll: ScrollHandle,
    follow: bool,
    _subs: Vec<Subscription>,
}

impl RecordingView {
    pub fn new(core: Arc<Core>, note_path: PathBuf, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let notes = cx.new(|cx| TextareaState::new(window, cx).placeholder(format!("{} …", t("my-notes"))));
        let c2 = core.clone();
        let sub = cx.subscribe_in(&notes, window, move |_, state, ev: &InputEvent, _, cx| {
            if matches!(ev, InputEvent::Change) {
                c2.set_recording_notes(&state.read(cx).value());
            }
        });
        let segments = core.recording_segments();
        RecordingView {
            core,
            note_path,
            segments,
            notes,
            cards: vec![],
            lang_edit: None,
            scroll: ScrollHandle::new(),
            follow: true,
            _subs: vec![sub],
        }
    }

    pub fn note_path(&self) -> &Path {
        &self.note_path
    }

    pub fn on_event(&mut self, ev: &CoreEvent, cx: &mut Context<Self>) {
        match ev {
            CoreEvent::Segment { index, segment } => {
                if *index >= self.segments.len() {
                    self.segments.push(segment.clone());
                }
                if self.follow {
                    self.scroll.scroll_to_bottom();
                }
            }
            CoreEvent::SegmentReplaced { index, segment } => {
                if let Some(s) = self.segments.get_mut(*index) {
                    *s = segment.clone();
                }
            }
            CoreEvent::Suggestion(card) => {
                self.cards.insert(0, Card { card: card.clone(), answer: None });
                self.cards.truncate(8);
            }
            CoreEvent::SuggestionAnswer { id, text, .. } => {
                if let Some(c) = self.cards.iter_mut().find(|c| c.card.id == *id) {
                    c.answer = Some(text.clone());
                }
            }
            _ => {}
        }
        cx.notify();
    }

    fn render_transcript(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let ui = crate::i18n::language();
        let mine = self.core.settings().languages.mine;
        let mut list = v_flex().gap_3().px_8().py_4().max_w(px(760.));
        if self.segments.is_empty() {
            list = list.child(div().text_sm().text_color(theme.muted_foreground).child(t("listening")));
        }
        for (i, s) in self.segments.iter().enumerate() {
            let color = crate::theme::channel_color(s.channel, cx);
            let editing = self.lang_edit == Some(i);
            let mut row = v_flex().id(("seg", i)).child(
                h_flex()
                    .gap_2()
                    .text_xs()
                    .child(div().text_color(theme.muted_foreground).child(ln_notes::format_timestamp(s.start_ms)))
                    .child(div().text_color(color).child(s.channel.label(ui)))
                    .child(
                        div()
                            .id(("lang", i))
                            .px_1()
                            .rounded_sm()
                            .cursor_pointer()
                            .text_color(theme.muted_foreground)
                            .hover(|d| d.bg(theme.muted))
                            .child(s.lang.to_string())
                            .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new(t("set-language")).build(window, cx))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.lang_edit = if this.lang_edit == Some(i) { None } else { Some(i) };
                                cx.notify();
                            })),
                    ),
            );
            if editing {
                let mut langs = h_flex().gap_1().py_1();
                for l in &mine {
                    let lang = l.clone();
                    langs = langs.child(Button::new(SharedString::from(format!("set-{i}-{l}"))).xsmall().label(l.to_string()).on_click(
                        cx.listener(move |this, _, _, cx| {
                            this.core.correct_language(i, Lang::new(lang.as_str()));
                            this.lang_edit = None;
                            cx.notify();
                        }),
                    ));
                }
                row = row.child(langs);
            }
            list = list.child(row.child(div().text_sm().child(s.text.clone())));
        }
        div().id("transcript").flex_1().track_scroll(&self.scroll).overflow_y_scrollbar().child(list)
    }

    fn render_cards(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let mut col = v_flex().gap_3().p_3();
        col = col.child(div().text_xs().font_semibold().text_color(theme.muted_foreground).child(t("related")));
        for c in &self.cards {
            let id = c.card.id;
            let mut card = v_flex().gap_2().p_3().rounded_md().border_1().border_color(theme.border).bg(theme.background);
            if let Some(q) = &c.card.question {
                card = card.child(div().text_xs().text_color(theme.muted_foreground).child(q.clone()));
            }
            for (j, h) in c.card.hits.iter().enumerate() {
                let path = h.path.clone();
                card = card.child(
                    v_flex()
                        .id(SharedString::from(format!("hit-{id}-{j}")))
                        .cursor_pointer()
                        .child(div().text_sm().font_semibold().child(h.title.clone()))
                        .child(div().text_xs().text_color(theme.muted_foreground).line_clamp(3).child(h.snippet.clone()))
                        .on_click(move |_, _, cx| cx.open_url(&format!("file://{}", path.display()))),
                );
            }
            if let Some(a) = &c.answer {
                card = card
                    .child(div().pt_1().border_t_1().border_color(theme.border).child(TextView::markdown(("ans", id as usize), a.clone())));
            }
            card = card.child(
                h_flex()
                    .gap_1()
                    .child(Button::new(("pin", id as usize)).xsmall().label(t("pin")).on_click(cx.listener(move |this, _, _, cx| {
                        this.core.pin_suggestion(id);
                        this.cards.retain(|c| c.card.id != id);
                        cx.notify();
                    })))
                    .child(Button::new(("dismiss", id as usize)).xsmall().ghost().label(t("dismiss")).on_click(cx.listener(
                        move |this, _, _, cx| {
                            this.core.dismiss_suggestion(id);
                            this.cards.retain(|c| c.card.id != id);
                            cx.notify();
                        },
                    ))),
            );
            col = col.child(card);
        }
        div()
            .id("cards")
            .w(px(300.))
            .h_full()
            .flex_none()
            .border_l_1()
            .border_color(theme.border)
            .bg(theme.sidebar)
            .overflow_y_scrollbar()
            .child(col)
    }

    fn level_bar(level: f32, color: Hsla, cx: &App) -> impl IntoElement {
        use gpui_kit::component::ActiveTheme as _;
        // RMS 0.0–0.3 mapped to the meter width.
        let w = (level / 0.3).clamp(0.0, 1.0);
        div().w(px(56.)).h(px(4.)).rounded_full().bg(cx.theme().muted).child(div().h_full().rounded_full().bg(color).w(relative(w)))
    }
}

impl Render for RecordingView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _slow = crate::app::SlowGuard(std::time::Instant::now(), "recording_view::render");
        let info = self.core.recording_info();
        let theme = cx.theme().clone();
        let ui = crate::i18n::language();
        let mut langs: Vec<String> = Vec::new();
        for s in &self.segments {
            let l = s.lang.to_string();
            if !langs.contains(&l) {
                langs.push(l);
            }
        }
        let (elapsed, paused, me, others, cloud) = info
            .as_ref()
            .map(|i| (i.elapsed.as_secs(), i.paused, i.level_me, i.level_others, i.cloud))
            .unwrap_or((0, false, 0.0, 0.0, false));
        let connecting = info.as_ref().is_some_and(|i| i.connecting);
        let thoughts = info.as_ref().is_some_and(|i| i.mode == ln_core::RecordingMode::Thoughts);
        let suggestions = !thoughts && self.core.settings().live_suggestions.enabled;
        h_flex()
            .size_full()
            .child(
                v_flex()
                    .flex_1()
                    .min_w(px(0.))
                    .h_full()
                    .child(
                        h_flex()
                            .px_8()
                            .py_3()
                            .gap_3()
                            .border_b_1()
                            .border_color(theme.border)
                            .child(div().text_color(theme.danger).child("●"))
                            .child(div().font_semibold().child(t(if thoughts { "organize-thoughts" } else { "recording" })))
                            .when(connecting, |el| {
                                el.child(
                                    h_flex()
                                        .gap_2()
                                        .text_sm()
                                        .text_color(theme.muted_foreground)
                                        .child(gpui_kit::component::spinner::Spinner::new().xsmall())
                                        .child(t("activity-connecting")),
                                )
                            })
                            .when(!connecting, |el| {
                                el.child(div().text_color(theme.muted_foreground).child(format!("{:02}:{:02}", elapsed / 60, elapsed % 60)))
                            })
                            .child(div().text_xs().text_color(theme.muted_foreground).child(langs.join(" · ")))
                            .child(div().flex_1())
                            .when(cloud, |el| {
                                el.child(
                                    h_flex()
                                        .gap_1()
                                        .text_xs()
                                        .text_color(theme.muted_foreground)
                                        .child(gpui_kit::component::Icon::new(IconName::Globe).xsmall())
                                        .child(t("cloud-audio")),
                                )
                            }),
                    )
                    .child(div().px_8().pt_3().h(px(140.)).child(Textarea::new(&self.notes).h(px(120.))))
                    .when(!thoughts, |el| el.child(self.render_transcript(cx)))
                    .when(thoughts, |el| {
                        el.child(
                            div()
                                .flex_1()
                                .px_8()
                                .py_4()
                                .text_sm()
                                .text_color(theme.muted_foreground)
                                .child(t("organize-thoughts-recording")),
                        )
                    })
                    .child(
                        h_flex()
                            .px_8()
                            .py_3()
                            .gap_3()
                            .border_t_1()
                            .border_color(theme.border)
                            .child(div().text_xs().text_color(theme.muted_foreground).child(ln_types::Channel::Me.label(ui)))
                            .child(Self::level_bar(me, crate::theme::channel_color(ln_types::Channel::Me, cx), cx))
                            .child(div().text_xs().text_color(theme.muted_foreground).child(ln_types::Channel::Others.label(ui)))
                            .child(Self::level_bar(others, crate::theme::channel_color(ln_types::Channel::Others, cx), cx))
                            .child(div().flex_1())
                            .child(
                                Button::new("pause")
                                    .small()
                                    .ghost()
                                    .icon(if paused { IconName::Play } else { IconName::Pause })
                                    .tooltip(if paused { t("resume") } else { t("pause") })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.core.pause_recording(!paused);
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("stop")
                                    .danger()
                                    .label(t("stop"))
                                    .tooltip_with_action(t("stop"), &crate::ToggleRecording, None)
                                    .on_click(|_, window, cx| window.dispatch_action(Box::new(crate::ToggleRecording), cx)),
                            ),
                    ),
            )
            .when(suggestions, |el| el.child(self.render_cards(cx)))
    }
}
