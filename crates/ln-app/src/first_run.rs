//! The only first-run screen (SPEC §4): confirm the model download, pick the assistant, go.

use crate::i18n::t;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::radio::RadioGroup;
use gpui_kit::component::{ActiveTheme as _, StyledExt as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use ln_core::{AssistantChoice, Core};
use std::sync::Arc;

pub enum FirstRunEvent {
    Done,
}
impl EventEmitter<FirstRunEvent> for FirstRunView {}

pub struct FirstRunView {
    core: Arc<Core>,
    choice: usize,
    key: Entity<InputState>,
    custom_url: Entity<InputState>,
    custom_model: Entity<InputState>,
    terms: bool,
    hint: bool,
}

impl FirstRunView {
    pub fn new(core: Arc<Core>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let key = cx.new(|cx| InputState::new(window, cx).placeholder("sk-or-…").masked(true));
        let custom_url = cx.new(|cx| InputState::new(window, cx).placeholder("http://127.0.0.1:4000/v1"));
        let custom_model = cx.new(|cx| InputState::new(window, cx).placeholder("gemma-4-26b"));
        // < 16 GB: OpenRouter preselected (SPEC §4).
        let choice = if Core::ram_gb() < 16 { 0 } else { 1 };
        FirstRunView { core, choice, key, custom_url, custom_model, terms: false, hint: false }
    }

    fn start(&mut self, cx: &mut Context<Self>) {
        if !self.terms {
            self.hint = true;
            cx.notify();
            return;
        }
        let key = self.key.read(cx).value().to_string();
        let choice = match self.choice {
            0 => AssistantChoice::OpenRouter { key: (!key.trim().is_empty()).then_some(key) },
            1 => AssistantChoice::Local,
            _ => AssistantChoice::Custom {
                base_url: self.custom_url.read(cx).value().to_string(),
                model: self.custom_model.read(cx).value().to_string(),
                key: (!key.trim().is_empty()).then_some(key),
            },
        };
        self.core.complete_first_run(choice);
        cx.emit(FirstRunEvent::Done);
    }
}

fn gb(bytes: u64) -> String {
    let g = bytes as f64 / 1e9;
    if g < 1.0 { format!("{:.0} MB", g * 1000.0) } else { format!("{g:.1} GB") }
}

impl Render for FirstRunView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let plan = self.core.first_run_plan(self.choice == 1);
        let total: u64 = plan.iter().map(|m| m.total_size()).sum();
        let mut models = v_flex().gap_1();
        for m in &plan {
            models = models.child(
                h_flex()
                    .gap_3()
                    .text_sm()
                    .child(div().flex_1().child(m.display))
                    .child(div().text_color(theme.muted_foreground).child(m.license))
                    .child(div().w(px(64.)).text_right().text_color(theme.muted_foreground).child(gb(m.total_size()))),
            );
        }
        models = models.child(
            h_flex()
                .pt_1()
                .border_t_1()
                .border_color(theme.border)
                .text_sm()
                .child(div().flex_1())
                .child(div().font_semibold().child(gb(total))),
        );
        let terms_url = "https://ai.google.dev/gemma/terms";
        v_flex().size_full().items_center().justify_center().child(
            v_flex()
                .w(px(520.))
                .gap_5()
                .child(div().text_2xl().font_semibold().child(t("first-run-title")))
                .child(v_flex().gap_2().child(div().text_sm().font_semibold().child(t("first-run-local"))).child(models))
                .child(
                    v_flex()
                        .gap_2()
                        .child(div().text_sm().font_semibold().child(t("first-run-assistant")))
                        .child(
                            RadioGroup::new("assistant")
                                .children([t("first-run-openrouter"), t("first-run-local-llm"), t("first-run-custom")])
                                .selected_index(Some(self.choice))
                                .on_change(cx.listener(|this, i: &usize, _, cx| {
                                    this.choice = *i;
                                    cx.notify();
                                })),
                        )
                        .when(self.choice == 0, |el| {
                            el.child(div().text_xs().text_color(theme.muted_foreground).child(t("first-run-key")))
                                .child(Input::new(&self.key))
                        })
                        .when(self.choice == 2, |el| {
                            el.child(Input::new(&self.custom_url)).child(Input::new(&self.custom_model)).child(Input::new(&self.key))
                        }),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .child(Checkbox::new("terms").label(t("first-run-terms")).checked(self.terms).on_change(cx.listener(
                            |this, v: &bool, _, cx| {
                                this.terms = *v;
                                this.hint = false;
                                cx.notify();
                            },
                        )))
                        .child(
                            div()
                                .id("terms-link")
                                .text_xs()
                                .text_color(theme.link)
                                .cursor_pointer()
                                .child("ai.google.dev/gemma/terms")
                                .on_click(move |_, _, cx| cx.open_url(terms_url)),
                        ),
                )
                .when(self.hint, |el| el.child(div().text_xs().text_color(theme.danger).child(t("first-run-terms"))))
                .child(h_flex().justify_end().child(
                    Button::new("start").primary().label(t("first-run-start")).on_click(cx.listener(|this, _, _, cx| this.start(cx))),
                )),
        )
    }
}

#[cfg(feature = "screenshots")]
impl FirstRunView {
    /// What a user does: accept the terms and press the start button.
    pub(crate) fn complete_for_tour(&mut self, cx: &mut Context<Self>) {
        self.terms = true;
        self.start(cx);
    }
}
