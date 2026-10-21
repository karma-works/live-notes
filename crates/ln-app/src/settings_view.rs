//! Settings window (SPEC §5 F7). Each change is saved immediately; there is no "Save" button.

use crate::i18n::t;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::progress::Progress;
use gpui_kit::component::radio::RadioGroup;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme as _, Sizable as _, StyledExt as _, WindowExt as _, h_flex, scroll::ScrollableElement as _, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use ln_core::Core;
use ln_core::{ChatGptStatus, UseCost, UseState, UseTask};
use ln_types::{Category, ChatRole, ChineseScript, DecisionProvider, Lang, RoleConfig, SuggestionFrequency, TranscriptionMode, UiLanguage};
use std::collections::HashMap;
use std::sync::Arc;

pub struct SettingsView {
    core: Arc<Core>,
    openai_key: Entity<InputState>,
    openrouter_key: Entity<InputState>,
    role_models: Vec<(ChatRole, Entity<InputState>)>,
    /// One-time confirmation after the first sign-in with plan usage.
    plan_notice: bool,
    limit: Entity<InputState>,
    tested: HashMap<String, Result<(), String>>,
    probe: Option<ln_core::ProbeReport>,
    probing: bool,
    category_name: Entity<InputState>,
    category_hint: Entity<InputState>,
    decision_models: [Entity<InputState>; 2],
}

const EXTRA_LANGS: [&str; 6] = ["it", "pt", "nl", "ja", "ko", "ru"];

impl SettingsView {
    pub fn new(core: Arc<Core>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let s = core.settings();
        let openai_key = cx.new(|cx| InputState::new(window, cx).placeholder("sk-…").masked(true));
        let openrouter_key = cx.new(|cx| InputState::new(window, cx).placeholder("sk-or-…").masked(true));
        let role_models = ChatRole::ALL
            .iter()
            .map(|r| (*r, cx.new(|cx| InputState::new(window, cx).default_value(s.roles.get(*r).model.clone()))))
            .collect();
        let limit = cx.new(|cx| InputState::new(window, cx).default_value(format!("{:.2}", s.costs.monthly_limit_usd)));
        core.chatgpt_load_models();
        let category_name = cx.new(|cx| InputState::new(window, cx).placeholder(t("category-name")));
        let category_hint = cx.new(|cx| InputState::new(window, cx).placeholder(t("category-hint")));
        let decision_models = [
            cx.new(|cx| InputState::new(window, cx).default_value(s.categories.openrouter_model.clone())),
            cx.new(|cx| InputState::new(window, cx).default_value(s.categories.openai_model.clone())),
        ];
        SettingsView {
            core,
            openai_key,
            openrouter_key,
            role_models,
            plan_notice: false,
            limit,
            tested: HashMap::new(),
            probe: None,
            probing: false,
            category_name,
            category_hint,
            decision_models,
        }
    }

    fn add_category(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.category_name.read(cx).value().trim().to_string();
        let hint = self.category_hint.read(cx).value().trim().to_string();
        if name.is_empty() {
            return;
        }
        self.core.update_settings(|s| {
            if !s.categories.list.iter().any(|c| c.name.eq_ignore_ascii_case(&name)) {
                let color = s.categories.next_color();
                s.categories.list.push(Category { name, color, hint });
            }
        });
        self.category_name.update(cx, |i, cx| i.set_value("", window, cx));
        self.category_hint.update(cx, |i, cx| i.set_value("", window, cx));
        cx.notify();
    }

    /// Categories with colours, the decision model that assigns them, and its key.
    fn render_categories(&self, cx: &mut Context<Self>) -> Div {
        let theme = cx.theme().clone();
        let s = self.core.settings();
        let c = &s.categories;
        let mut sec =
            Self::section(t("categories"), cx).child(div().text_xs().text_color(theme.muted_foreground).child(t("categories-hint")));
        if c.list.is_empty() {
            sec = sec.child(div().text_sm().text_color(theme.muted_foreground).child(t("categories-empty")));
        }
        for (i, cat) in c.list.iter().enumerate() {
            sec = sec.child(
                h_flex()
                    .gap_2()
                    .child(
                        div()
                            .id(("cat-color", i))
                            .size(px(14.))
                            .flex_none()
                            .rounded_full()
                            .cursor_pointer()
                            .bg(crate::theme::tag_color(Some(cat.color)))
                            .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new(t("category-color")).build(window, cx))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.core.update_settings(|s| {
                                    if let Some(x) = s.categories.list.get_mut(i) {
                                        x.color = x.color.next();
                                    }
                                });
                                cx.notify();
                            })),
                    )
                    .child(
                        v_flex().flex_1().min_w(px(0.)).child(div().text_sm().child(cat.name.clone())).when(!cat.hint.is_empty(), |el| {
                            el.child(div().text_xs().text_color(theme.muted_foreground).child(cat.hint.clone()))
                        }),
                    )
                    .child(Button::new(("cat-del", i)).xsmall().ghost().label(t("category-remove")).on_click(cx.listener(
                        move |this, _, _, cx| {
                            this.core.update_settings(|s| {
                                if i < s.categories.list.len() {
                                    s.categories.list.remove(i);
                                }
                            });
                            cx.notify();
                        },
                    ))),
            );
        }
        sec = sec.child(
            h_flex()
                .gap_2()
                .child(div().flex_1().child(Input::new(&self.category_name)))
                .child(div().flex_1().child(Input::new(&self.category_hint)))
                .child(
                    Button::new("cat-add")
                        .small()
                        .label(t("category-add"))
                        .on_click(cx.listener(|this, _, window, cx| this.add_category(window, cx))),
                ),
        );
        sec = sec.child(Switch::new("cat-auto").label(t("categories-auto")).checked(c.auto_assign).on_change(cx.listener(
            |this, v: &bool, _, cx| {
                let v = *v;
                this.core.update_settings(|s| s.categories.auto_assign = v);
                cx.notify();
            },
        )));
        let idx = match c.provider {
            DecisionProvider::Auto => 0,
            DecisionProvider::OpenRouter => 1,
            DecisionProvider::OpenAi => 2,
        };
        sec = sec.child(div().text_xs().text_color(theme.muted_foreground).child(t("decision-provider"))).child(
            RadioGroup::horizontal("decision-provider")
                .children([t("decision-auto"), "OpenRouter", "OpenAI"])
                .selected_index(Some(idx))
                .on_change(cx.listener(|this, i: &usize, _, cx| {
                    let p = match i {
                        1 => DecisionProvider::OpenRouter,
                        2 => DecisionProvider::OpenAi,
                        _ => DecisionProvider::Auto,
                    };
                    this.core.update_settings(|s| s.categories.provider = p);
                    cx.notify();
                })),
        );
        for (k, label) in [t("decision-openrouter-model"), t("decision-openai-model")].into_iter().enumerate() {
            let input = self.decision_models[k].clone();
            let input2 = input.clone();
            sec = sec.child(
                h_flex()
                    .gap_2()
                    .child(div().w(px(150.)).flex_none().text_xs().text_color(theme.muted_foreground).child(label))
                    .child(div().flex_1().child(Input::new(&input)))
                    .child(Button::new(("save-decision-model", k)).small().label(t("save-key")).on_click(cx.listener(
                        move |this, _, _, cx| {
                            let m = input2.read(cx).value().trim().to_string();
                            if !m.is_empty() {
                                this.core.update_settings(|s| {
                                    if k == 0 {
                                        s.categories.openrouter_model = m;
                                    } else {
                                        s.categories.openai_model = m;
                                    }
                                });
                            }
                            cx.notify();
                        },
                    ))),
            );
        }
        let (status, color) = match self.core.decisions_status() {
            Ok(using) => (format!("{} {using}", t("decision-using")), theme.muted_foreground),
            Err(e) => (e, theme.warning),
        };
        sec = sec.child(div().text_xs().text_color(color).child(status));
        if !self.core.has_api_key("openrouter") && !self.core.has_api_key("openai") {
            sec = sec.child(self.key_row("openrouter", t("openrouter-key"), &self.openrouter_key.clone(), cx));
        }
        sec.when(!c.list.is_empty(), |el| {
            el.child(h_flex().child(Button::new("cat-sort-all").small().ghost().label(t("categories-sort-all")).on_click(cx.listener(
                |this, _, window, cx| {
                    this.core.classify_all_notes();
                    window.push_notification(t("categories-sorting"), cx);
                },
            ))))
        })
    }

    pub fn chatgpt_changed(&mut self, first_sign_in: bool, window: &mut Window, cx: &mut Context<Self>) {
        if first_sign_in {
            self.plan_notice = true;
        }
        // The summary may have switched to ChatGPT: show its model.
        let s = self.core.settings();
        for (r, input) in &self.role_models {
            let m = s.roles.get(*r).model.clone();
            input.update(cx, |i, cx| i.set_value(m, window, cx));
        }
        cx.notify();
    }

    fn render_chatgpt(&self, cx: &mut Context<Self>) -> Div {
        let theme = cx.theme().clone();
        let mut sec = Self::section(t("chatgpt-section"), cx);
        if self.plan_notice {
            sec = sec.child(
                v_flex()
                    .gap_2()
                    .p_3()
                    .rounded_md()
                    .border_1()
                    .border_color(theme.border)
                    .child(div().text_sm().font_semibold().child(t("chatgpt-first-title")))
                    .child(div().text_xs().text_color(theme.muted_foreground).child(t("chatgpt-first-body")))
                    .child(h_flex().child(Button::new("plan-ok").small().primary().label(t("got-it")).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.plan_notice = false;
                            cx.notify();
                        },
                    )))),
            );
        }
        let manage = |id: &'static str| {
            Button::new(id).small().ghost().label(t("chatgpt-manage")).on_click(|_, _, cx| cx.open_url(ln_core::CHATGPT_USAGE_URL))
        };
        match self.core.chatgpt_status() {
            ChatGptStatus::SignedOut => {
                sec.child(div().text_xs().text_color(theme.muted_foreground).child(t("chatgpt-hint"))).child(h_flex().child(
                    Button::new("chatgpt-in").small().primary().label(t("chatgpt-continue")).on_click(cx.listener(|this, _, _, cx| {
                        this.core.chatgpt_sign_in(false);
                        cx.notify();
                    })),
                ))
            }
            ChatGptStatus::SigningIn => sec.child(
                h_flex().gap_2().child(div().flex_1().text_xs().text_color(theme.muted_foreground).child(t("chatgpt-waiting"))).child(
                    Button::new("chatgpt-cancel").small().ghost().label(t("cancel")).on_click(cx.listener(|this, _, _, cx| {
                        this.core.chatgpt_cancel_sign_in();
                        cx.notify();
                    })),
                ),
            ),
            ChatGptStatus::SignedIn { email, plan_usage } => {
                let who = format!("{} {}", t("chatgpt-signed-in"), email.unwrap_or_default());
                let mut row = h_flex().gap_2().child(div().flex_1().text_sm().child(who));
                if plan_usage {
                    row = row.child(manage("chatgpt-manage"));
                }
                row = row.child(Button::new("chatgpt-out").small().ghost().label(t("chatgpt-sign-out")).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.core.chatgpt_sign_out();
                        cx.notify();
                    },
                )));
                sec = sec.child(row);
                if !plan_usage {
                    sec = sec.child(div().text_xs().text_color(theme.warning).child(t("chatgpt-plan-off"))).child(h_flex().child(
                        Button::new("chatgpt-allow").small().primary().label(t("chatgpt-allow")).on_click(cx.listener(|this, _, _, cx| {
                            this.core.chatgpt_sign_in(true);
                            cx.notify();
                        })),
                    ));
                }
                sec
            }
        }
    }

    /// Which model does what, where it runs and where costs arise.
    fn render_overview(&self, cx: &mut Context<Self>) -> Div {
        let theme = cx.theme().clone();
        let mut sec = Self::section(t("overview"), cx).child(div().text_xs().text_color(theme.muted_foreground).child(t("overview-hint")));
        for u in self.core.model_overview() {
            let task = match u.task {
                UseTask::Transcription => t("task-transcription"),
                UseTask::LanguageDetection => t("task-langid"),
                UseTask::Search => t("task-search"),
                UseTask::Questions => t("role-assistant"),
                UseTask::Suggestions => t("role-suggestions"),
                UseTask::Summary => t("role-summary"),
            };
            let place = if u.cost == UseCost::Local { format!("{} · {}", t("local"), u.provider) } else { u.provider.clone() };
            let (cost, paid) = match &u.cost {
                UseCost::Local => (t("cost-local").to_string(), false),
                UseCost::Plan => (t("cost-plan").to_string(), false),
                UseCost::PerToken { input, output } => (format!("{} / {} {}", usd(*input), usd(*output), t("per-1m")), true),
                UseCost::PerTokenUnknown => (t("cost-token-unknown").to_string(), true),
                UseCost::PerMinute(m) => (format!("{} {} (≈ {} {})", usd(*m), t("per-minute"), usd(m * 60.0), t("per-hour")), true),
            };
            let state = match u.state {
                UseState::Ready => None,
                UseState::Off => Some((t("state-off"), theme.muted_foreground)),
                UseState::KeyMissing => Some((t("state-key"), theme.warning)),
                UseState::SignInMissing => Some((t("state-signin"), theme.warning)),
                UseState::ModelMissing => Some((t("state-model"), theme.warning)),
            };
            let mut cost_col = v_flex()
                .w(px(220.))
                .flex_none()
                .child(div().text_xs().text_color(if paid { theme.foreground } else { theme.success }).child(cost));
            let estimate = match (u.per_hour, u.per_question) {
                (_, Some(q)) if q > 0.0 => Some(format!("≈ {} {}", usd(q), t("per-question"))),
                (Some(h), _) if h > 0.0 => Some(format!("≈ {} {}", usd(h), t("per-conversation-hour"))),
                _ => None,
            };
            if let Some(e) = estimate {
                cost_col = cost_col.child(div().text_xs().text_color(theme.muted_foreground).child(e));
            }
            if u.month_usd > 0.0 {
                cost_col = cost_col.child(div().text_xs().text_color(theme.muted_foreground).child(format!(
                    "{} {}",
                    usd(u.month_usd),
                    t("this-month")
                )));
            }
            sec = sec.child(
                h_flex()
                    .items_start()
                    .gap_3()
                    .py_1()
                    .child(div().w(px(150.)).flex_none().text_sm().child(task))
                    .child(
                        v_flex().flex_1().min_w(px(0.)).child(div().text_sm().child(u.model.clone())).child(
                            h_flex()
                                .gap_2()
                                .child(div().text_xs().text_color(theme.muted_foreground).child(place))
                                .when_some(state, |el, (s, c)| el.child(div().text_xs().text_color(c).child(s))),
                        ),
                    )
                    .child(cost_col),
            );
        }
        let hour = match self.core.hourly_estimate() {
            Some(h) if h > 0.0 => format!("≈ {}", usd(h)),
            Some(_) => t("cost-local").to_string(),
            None => t("cost-token-unknown").to_string(),
        };
        let sec = sec
            .child(
                h_flex()
                    .gap_3()
                    .py_1()
                    .border_t_1()
                    .border_color(theme.border)
                    .child(div().w(px(150.)).flex_none().text_sm().font_semibold().child(t("one-hour")))
                    .child(div().flex_1().min_w(px(0.)))
                    .child(div().w(px(220.)).flex_none().text_sm().font_semibold().child(hour)),
            )
            .child(div().text_xs().text_color(theme.muted_foreground).child(t("one-hour-hint")));
        sec.child(div().text_xs().text_color(theme.muted_foreground).child(format!(
            "{} {}: {} / {}",
            t("overview-costs"),
            t("month-cost"),
            usd(self.core.month_cost()),
            usd(self.core.settings().costs.monthly_limit_usd)
        )))
    }

    /// Provider and model for one chat role.
    fn role_row(&self, role: ChatRole, cx: &mut Context<Self>) -> Div {
        let theme = cx.theme().clone();
        let s = self.core.settings();
        let current = s.roles.get(role).clone();
        let chatgpt_ok = matches!(self.core.chatgpt_status(), ChatGptStatus::SignedIn { plan_usage: true, .. });
        let mut providers: Vec<(&'static str, &'static str)> =
            vec![("openrouter", "OpenRouter"), ("local", t("mode-local")), ("ollama", "Ollama")];
        if chatgpt_ok || current.provider == "chatgpt" {
            providers.push(("chatgpt", "ChatGPT"));
        }
        let idx = providers.iter().position(|(id, _)| *id == current.provider);
        let ids: Vec<&'static str> = providers.iter().map(|(id, _)| *id).collect();
        let (label, hint) = match role {
            ChatRole::Assistant => (t("role-assistant"), None),
            ChatRole::Suggestions => (t("role-suggestions"), Some(t("role-suggestions-hint"))),
            ChatRole::Summary => (t("role-summary"), Some(t("role-summary-hint"))),
        };
        let input = self.role_models.iter().find(|(r, _)| *r == role).map(|(_, i)| i.clone()).expect("input per role");
        let needs = match current.provider.as_str() {
            "openrouter" => t("needs-openrouter"),
            "chatgpt" => t("needs-chatgpt"),
            "local" => t("needs-local"),
            "ollama" => t("needs-ollama"),
            _ => t("needs-custom"),
        };
        let estimate = self.core.model_overview().into_iter().find_map(|u| {
            let same = matches!(
                (role, u.task),
                (ChatRole::Assistant, UseTask::Questions)
                    | (ChatRole::Suggestions, UseTask::Suggestions)
                    | (ChatRole::Summary, UseTask::Summary)
            );
            if !same {
                return None;
            }
            match (u.per_hour, u.per_question) {
                (_, Some(q)) if q > 0.0 => Some(format!("≈ {} {}", usd(q), t("per-question"))),
                (Some(h), _) if h > 0.0 => Some(format!("≈ {} {}", usd(h), t("per-conversation-hour"))),
                (Some(_), _) | (_, Some(_)) => Some(t("cost-local").to_string()),
                _ => None,
            }
        });
        let needs_line = match estimate {
            Some(e) => format!("{needs} · {e}"),
            None => needs.to_string(),
        };
        let mut col = v_flex()
            .gap_1()
            .child(div().text_sm().child(label))
            .when_some(hint, |el, h| el.child(div().text_xs().text_color(theme.muted_foreground).child(h)))
            .child(div().text_xs().text_color(theme.muted_foreground).child(needs_line))
            .child(
                RadioGroup::horizontal(SharedString::from(format!("prov-{}", role.id())))
                    .children(providers.iter().map(|(_, name)| *name))
                    .selected_index(idx)
                    .on_change(cx.listener(move |this, i: &usize, window, cx| {
                        let Some(p) = ids.get(*i).copied() else { return };
                        let m = match p {
                            "openrouter" => "google/gemma-4-26b-a4b-it".to_string(),
                            "local" => ln_models::recommended_llm(Core::ram_gb()).id.to_string(),
                            "chatgpt" => this.core.chatgpt_models().first().map(|m| m.id.clone()).unwrap_or_default(),
                            _ => "gemma4".to_string(),
                        };
                        this.core.update_settings(|s| s.roles.set(role, RoleConfig { provider: p.into(), model: m.clone() }));
                        if let Some((_, inp)) = this.role_models.iter().find(|(r, _)| *r == role) {
                            inp.update(cx, |st, cx| st.set_value(m, window, cx));
                        }
                        if p == "local" {
                            this.core.download_models(&[ln_models::recommended_llm(Core::ram_gb()).id]);
                        }
                        cx.notify();
                    })),
            );
        let plan_models = if current.provider == "chatgpt" { self.core.chatgpt_models() } else { vec![] };
        if !plan_models.is_empty() {
            let sel = plan_models.iter().position(|m| m.id == current.model);
            let slugs: Vec<String> = plan_models.iter().map(|m| m.id.clone()).collect();
            col = col.child(
                RadioGroup::vertical(SharedString::from(format!("model-{}", role.id())))
                    .children(plan_models.iter().map(|m| m.name.clone()))
                    .selected_index(sel)
                    .on_change(cx.listener(move |this, i: &usize, _, cx| {
                        if let Some(m) = slugs.get(*i).cloned() {
                            this.core.update_settings(|s| s.roles.set(role, RoleConfig { provider: "chatgpt".into(), model: m }));
                        }
                        cx.notify();
                    })),
            );
        } else {
            let input2 = input.clone();
            col = col.child(
                h_flex()
                    .gap_2()
                    .child(div().text_xs().text_color(theme.muted_foreground).child(t("model")))
                    .child(div().flex_1().child(Input::new(&input)))
                    .child(Button::new(SharedString::from(format!("save-model-{}", role.id()))).small().label(t("save-key")).on_click(
                        cx.listener(move |this, _, _, cx| {
                            let m = input2.read(cx).value().trim().to_string();
                            if !m.is_empty() {
                                let p = this.core.settings().roles.get(role).provider.clone();
                                this.core.update_settings(|s| s.roles.set(role, RoleConfig { provider: p, model: m }));
                            }
                            cx.notify();
                        }),
                    )),
            );
        }
        if current.provider == "chatgpt" && chatgpt_ok {
            col = col.child(
                h_flex().gap_2().child(div().text_xs().text_color(theme.muted_foreground).child(t("chatgpt-using"))).child(
                    Button::new(SharedString::from(format!("manage-{}", role.id())))
                        .xsmall()
                        .ghost()
                        .label(t("chatgpt-manage"))
                        .on_click(|_, _, cx| cx.open_url(ln_core::CHATGPT_USAGE_URL)),
                ),
            );
        }
        col
    }

    fn run_probe(&mut self, cx: &mut Context<Self>) {
        if self.probing {
            return;
        }
        self.probing = true;
        cx.notify();
        let core = self.core.clone();
        cx.spawn(async move |this, cx: &mut AsyncApp| {
            let report = cx.background_spawn(async move { core.probe_capture(5, true) }).await;
            let _ = this.update(cx, |v, cx| {
                v.probing = false;
                v.probe = Some(report);
                cx.notify();
            });
        })
        .detach();
    }

    fn render_probe(&self, cx: &mut Context<Self>) -> Div {
        let theme = cx.theme().clone();
        let mut sec = Self::section(t("recording-section"), cx)
            .child(div().text_xs().text_color(theme.muted_foreground).child(t("test-recording-hint")))
            .child(
                h_flex().child(
                    Button::new("probe")
                        .small()
                        .label(if self.probing { t("test-running") } else { t("test-recording") })
                        .loading(self.probing)
                        .on_click(cx.listener(|this, _, _, cx| this.run_probe(cx))),
                ),
            );
        if let Some(r) = &self.probe {
            for c in &r.channels {
                let name = if c.channel == "me" {
                    ln_types::Channel::Me.label(crate::i18n::language())
                } else {
                    ln_types::Channel::Others.label(crate::i18n::language())
                };
                let (status, color) = if !c.started || c.sample_rate == 0 {
                    (t("probe-failed").to_string(), theme.danger)
                } else if c.nonzero == 0.0 {
                    (t("probe-silent").to_string(), theme.warning)
                } else if c.sections > 0 {
                    (format!("{} · {}", t("probe-ok"), t("probe-speech")), theme.success)
                } else {
                    (t("probe-ok").to_string(), theme.success)
                };
                let level = (c.rms / 0.1).clamp(0.0, 1.0);
                let mut row = v_flex().gap_1().child(
                    h_flex()
                        .gap_2()
                        .text_sm()
                        .child(div().w(px(70.)).child(name))
                        .child(
                            div()
                                .w(px(80.))
                                .h(px(4.))
                                .rounded_full()
                                .bg(theme.muted)
                                .child(div().h_full().rounded_full().bg(color).w(relative(level))),
                        )
                        .child(div().text_xs().text_color(color).child(status)),
                );
                if let Some((lang, text)) = c.transcript.first() {
                    row = row.child(div().text_xs().text_color(theme.muted_foreground).child(format!("{lang}: {text}")));
                }
                if let Some(e) = &c.error {
                    row = row.child(div().text_xs().text_color(theme.muted_foreground).child(e.clone()));
                }
                sec = sec.child(row);
            }
        }
        sec
    }

    pub fn provider_tested(&mut self, provider: &str, result: Result<(), String>, cx: &mut Context<Self>) {
        self.tested.insert(provider.to_string(), result);
        cx.notify();
    }

    fn section(title: &str, cx: &App) -> Div {
        v_flex().gap_2().pb_4().border_b_1().border_color(cx.theme().border).child(div().text_sm().font_semibold().child(title.to_string()))
    }

    fn key_row(&self, provider: &'static str, label: &str, input: &Entity<InputState>, cx: &mut Context<Self>) -> impl IntoElement {
        let has = self.core.has_api_key(provider);
        let status = match self.tested.get(provider) {
            Some(Ok(())) => Some((t("connection-ok").to_string(), cx.theme().success)),
            Some(Err(e)) => Some((e.clone(), cx.theme().danger)),
            None => None,
        };
        let input2 = input.clone();
        v_flex()
            .gap_1()
            .child(div().text_xs().text_color(cx.theme().muted_foreground).child(format!("{label}{}", if has { " ✓" } else { "" })))
            .child(
                h_flex()
                    .gap_2()
                    .child(div().flex_1().child(Input::new(input)))
                    .child(Button::new(SharedString::from(format!("save-{provider}"))).small().label(t("save-key")).on_click(cx.listener(
                        move |this, _, window, cx| {
                            let k = input2.read(cx).value().to_string();
                            if this.core.set_api_key(provider, &k).is_ok() {
                                input2.update(cx, |i, cx| i.set_value("", window, cx));
                                window.push_notification(t("key-saved"), cx);
                            }
                            cx.notify();
                        },
                    )))
                    .child({
                        let input3 = input.clone();
                        Button::new(SharedString::from(format!("test-{provider}"))).small().ghost().label(t("test-connection")).on_click(
                            cx.listener(move |this, _, _, cx| {
                                // Tests the key in the field if one was typed, otherwise the saved one.
                                let typed = input3.read(cx).value().to_string();
                                this.tested.remove(provider);
                                this.core.test_provider(provider, Some(typed));
                                cx.notify();
                            }),
                        )
                    })
                    .when(has, |el| {
                        el.child(
                            Button::new(SharedString::from(format!("remove-{provider}"))).small().ghost().label(t("remove-key")).on_click(
                                cx.listener(move |this, _, window, cx| {
                                    if this.core.set_api_key(provider, "").is_ok() {
                                        this.tested.remove(provider);
                                        window.push_notification(t("key-removed"), cx);
                                    }
                                    cx.notify();
                                }),
                            ),
                        )
                    }),
            )
            .when_some(status, |el, (s, c)| el.child(div().text_xs().text_color(c).child(s)))
    }
}

/// "0,09 $" in German, "$0.09" in English; small amounts keep two significant digits.
fn usd(v: f64) -> String {
    let v = if v.abs() < 0.00005 { 0.0 } else { v };
    let digits = if v == 0.0 || v >= 0.1 {
        2
    } else if v >= 0.01 {
        3
    } else {
        4
    };
    let mut n = format!("{v:.digits$}");
    while digits > 2 && n.ends_with('0') {
        n.pop();
    }
    match crate::i18n::language() {
        UiLanguage::De => format!("{} $", n.replace('.', ",")),
        UiLanguage::En => format!("${n}"),
    }
}

fn gb(bytes: u64) -> String {
    let g = bytes as f64 / 1e9;
    if g < 1.0 { format!("{:.0} MB", g * 1000.0) } else { format!("{g:.1} GB") }
}

impl Render for SettingsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _slow = crate::app::SlowGuard(std::time::Instant::now(), "settings_view::render");
        let s = self.core.settings();
        let theme = cx.theme().clone();
        let core = self.core.clone();

        // General
        let notes_dir = s.notes_dir(&self.core.paths).to_string_lossy().into_owned();
        let ui_idx = match s.general.ui_language {
            None => 0,
            Some(UiLanguage::De) => 1,
            Some(UiLanguage::En) => 2,
        };
        let general = Self::section(t("general"), cx)
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        v_flex()
                            .flex_1()
                            .child(div().text_xs().text_color(theme.muted_foreground).child(t("notes-folder")))
                            .child(div().text_sm().child(notes_dir)),
                    )
                    .child(Button::new("notes-dir").small().label(t("change")).on_click(cx.listener(|this, _, _, cx| {
                        let rx = cx.prompt_for_paths(PathPromptOptions { files: false, directories: true, multiple: false, prompt: None });
                        let core = this.core.clone();
                        cx.spawn(async move |this, cx: &mut AsyncApp| {
                            if let Ok(Ok(Some(mut p))) = rx.await
                                && let Some(dir) = p.pop()
                            {
                                core.update_settings(|s| s.general.notes_dir = Some(dir));
                                let _ = this.update(cx, |_, cx| cx.notify());
                            }
                        })
                        .detach();
                    }))),
            )
            .child(div().text_xs().text_color(theme.muted_foreground).child(t("ui-language")))
            .child(RadioGroup::horizontal("ui-lang").children([t("system"), "Deutsch", "English"]).selected_index(Some(ui_idx)).on_change(
                cx.listener(|this, i: &usize, _, cx| {
                    let v = match i {
                        1 => Some(UiLanguage::De),
                        2 => Some(UiLanguage::En),
                        _ => None,
                    };
                    this.core.update_settings(|s| s.general.ui_language = v);
                    crate::i18n::set_language(this.core.ui_language());
                    cx.notify();
                }),
            ));

        // Transcription
        let mode_idx = match s.transcription.mode {
            TranscriptionMode::Local => 0,
            TranscriptionMode::OpenAiCheap => 1,
            TranscriptionMode::OpenAiLive => 2,
        };
        let has_openai = self.core.has_api_key("openai");
        let per_hour = |model: &str| {
            ln_core::price_per_minute(model)
                .map(|m| format!("≈ {} {}", usd(m * 60.0 * 0.8), t("per-conversation-hour")))
                .unwrap_or_default()
        };
        let modes = [
            (t("mode-local"), format!("{} · {}", t("mode-local-desc"), t("cost-local"))),
            (t("mode-cheap"), format!("gpt-transcribe · {} · {}", t("needs-openai"), per_hour("gpt-transcribe"))),
            (t("mode-live"), format!("gpt-live-transcribe · {} · {}", t("needs-openai"), per_hour("gpt-live-transcribe"))),
        ];
        let mut mode_list = v_flex().gap_1();
        for (name, desc) in &modes {
            mode_list = mode_list.child(
                h_flex()
                    .gap_2()
                    .text_xs()
                    .child(div().w(px(110.)).flex_none().text_color(theme.foreground).child(name.to_string()))
                    .child(div().flex_1().text_color(theme.muted_foreground).child(desc.clone())),
            );
        }
        let transcription = Self::section(t("transcription"), cx)
            .child(
                RadioGroup::horizontal("mode")
                    .children([t("mode-local"), t("mode-cheap"), t("mode-live")])
                    .selected_index(Some(mode_idx))
                    .on_change(cx.listener(|this, i: &usize, _, cx| {
                        let m = match i {
                            1 => TranscriptionMode::OpenAiCheap,
                            2 => TranscriptionMode::OpenAiLive,
                            _ => TranscriptionMode::Local,
                        };
                        this.core.update_settings(|s| s.transcription.mode = m);
                        cx.notify();
                    })),
            )
            .child(mode_list)
            .when(mode_idx > 0 && !has_openai, |el| el.child(div().text_xs().text_color(theme.warning).child(t("openai-key-needed"))))
            .child(div().text_xs().text_color(theme.muted_foreground).child(t("openai-key-hint")))
            .child(self.key_row("openai", t("openai-key"), &self.openai_key.clone(), cx));

        // Languages
        let mut lang_row = h_flex().flex_wrap().gap_3();
        for code in ln_types::MAIN_LANGUAGES.iter().chain(EXTRA_LANGS.iter()) {
            let l = Lang::new(code);
            let on = s.languages.mine.contains(&l);
            let code2 = code.to_string();
            lang_row =
                lang_row.child(Checkbox::new(SharedString::from(format!("lang-{code}"))).label(l.display_name()).checked(on).on_change(
                    cx.listener(move |this, v: &bool, _, cx| {
                        let l = Lang::new(&code2);
                        let v = *v;
                        this.core.update_settings(|s| {
                            s.languages.mine.retain(|x| *x != l);
                            if v {
                                s.languages.mine.push(l);
                            }
                            if s.languages.mine.is_empty() {
                                s.languages.mine = Lang::main_languages();
                            }
                        });
                        cx.notify();
                    }),
                ));
        }
        let script_idx = if s.languages.chinese_script == ChineseScript::Simplified { 0 } else { 1 };
        let languages = Self::section(t("languages"), cx)
            .child(div().text_xs().text_color(theme.muted_foreground).child(t("my-languages")))
            .child(lang_row)
            .child(div().text_xs().text_color(theme.muted_foreground).child(t("chinese-script")))
            .child(
                RadioGroup::horizontal("script").children([t("simplified"), t("traditional")]).selected_index(Some(script_idx)).on_change(
                    cx.listener(|this, i: &usize, _, cx| {
                        let v = if *i == 0 { ChineseScript::Simplified } else { ChineseScript::Traditional };
                        this.core.update_settings(|s| s.languages.chinese_script = v);
                        cx.notify();
                    }),
                ),
            );

        // Assistant: one model per role (questions, live suggestions, summary).
        let uses_openrouter = ChatRole::ALL.iter().any(|r| s.roles.get(*r).provider == "openrouter");
        let mut assistant = Self::section(t("assistant"), cx);
        for role in ChatRole::ALL {
            assistant = assistant.child(self.role_row(role, cx));
        }
        let assistant = assistant
            .when(uses_openrouter, |el| el.child(self.key_row("openrouter", t("openrouter-key"), &self.openrouter_key.clone(), cx)))
            .child(Switch::new("auto-summary").label(t("auto-summary")).checked(s.notes.auto_summary).on_change(cx.listener(
                |this, v: &bool, _, cx| {
                    let v = *v;
                    this.core.update_settings(|s| s.notes.auto_summary = v);
                    cx.notify();
                },
            )));

        // Live suggestions
        let freq_idx = match s.live_suggestions.frequency {
            SuggestionFrequency::Reserved => 0,
            SuggestionFrequency::Normal => 1,
            SuggestionFrequency::Active => 2,
        };
        let live = Self::section(t("live-suggestions"), cx)
            .child(Switch::new("sugg").label(t("suggestions-on")).checked(s.live_suggestions.enabled).on_change(cx.listener(
                |this, v: &bool, _, cx| {
                    let v = *v;
                    this.core.update_settings(|s| s.live_suggestions.enabled = v);
                    cx.notify();
                },
            )))
            .child(Switch::new("sugg-ans").label(t("suggestions-answers")).checked(s.live_suggestions.answers).on_change(cx.listener(
                |this, v: &bool, _, cx| {
                    let v = *v;
                    this.core.update_settings(|s| s.live_suggestions.answers = v);
                    cx.notify();
                },
            )))
            .child(
                RadioGroup::horizontal("freq")
                    .children([t("freq-reserved"), t("freq-normal"), t("freq-active")])
                    .selected_index(Some(freq_idx))
                    .on_change(cx.listener(|this, i: &usize, _, cx| {
                        let f = match i {
                            0 => SuggestionFrequency::Reserved,
                            2 => SuggestionFrequency::Active,
                            _ => SuggestionFrequency::Normal,
                        };
                        this.core.update_settings(|s| s.live_suggestions.frequency = f);
                        cx.notify();
                    })),
            );

        // Privacy
        let privacy = Self::section(t("privacy"), cx)
            .child(Switch::new("offline").label(t("offline")).checked(s.privacy.offline_mode).on_change(cx.listener(
                |this, v: &bool, _, cx| {
                    let v = *v;
                    this.core.update_settings(|s| s.privacy.offline_mode = v);
                    cx.notify();
                },
            )))
            .child(Switch::new("deny").label(t("deny-data")).checked(s.privacy.openrouter_deny_data_collection).on_change(cx.listener(
                |this, v: &bool, _, cx| {
                    let v = *v;
                    this.core.update_settings(|s| s.privacy.openrouter_deny_data_collection = v);
                    cx.notify();
                },
            )));

        // Models
        let mut models = Self::section(t("models"), cx);
        for m in self.core.model_status() {
            let id = m.id;
            let right: AnyElement = if let Some(p) = m.progress {
                div().w(px(120.)).child(Progress::new(SharedString::from(format!("p-{id}"))).value(p * 100.0).small()).into_any_element()
            } else if m.installed {
                h_flex()
                    .gap_2()
                    .child(div().text_xs().text_color(theme.muted_foreground).child(t("installed")))
                    .child(Button::new(SharedString::from(format!("del-{id}"))).xsmall().ghost().label(t("delete")).on_click(cx.listener(
                        move |this, _, _, cx| {
                            this.core.delete_model(id);
                            cx.notify();
                        },
                    )))
                    .into_any_element()
            } else {
                let c = core.clone();
                Button::new(SharedString::from(format!("dl-{id}")))
                    .xsmall()
                    .label(t("download"))
                    .on_click(move |_, _, _| c.download_models(&[id]))
                    .into_any_element()
            };
            models = models.child(
                h_flex()
                    .gap_2()
                    .text_sm()
                    .child(div().flex_1().child(m.display))
                    .child(div().text_xs().text_color(theme.muted_foreground).child(format!("{} · {}", m.license, gb(m.size))))
                    .child(right)
                    .when_some(m.error.clone(), |el, e| el.child(div().text_xs().text_color(theme.danger).child(e))),
            );
        }

        // Costs and data
        let limit_input = self.limit.clone();
        let costs = Self::section(t("costs"), cx)
            .child(
                h_flex()
                    .gap_2()
                    .text_sm()
                    .child(div().flex_1().child(t("month-cost")))
                    .child(div().child(format!("{:.3} $", self.core.month_cost()))),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(div().text_xs().text_color(theme.muted_foreground).child(t("monthly-limit")))
                    .child(div().w(px(100.)).child(Input::new(&self.limit)))
                    .child(Button::new("save-limit").small().label(t("save-key")).on_click(cx.listener(move |this, _, _, cx| {
                        if let Ok(v) = limit_input.read(cx).value().trim().replace(',', ".").parse::<f64>() {
                            this.core.update_settings(|s| s.costs.monthly_limit_usd = v.max(0.0));
                        }
                        cx.notify();
                    }))),
            )
            .child(
                h_flex().child(
                    Button::new("rebuild")
                        .small()
                        .ghost()
                        .label(t("rebuild-index"))
                        .on_click(cx.listener(|this, _, _, _| this.core.rebuild_index())),
                ),
            );

        div().id("settings").size_full().bg(theme.background).text_color(theme.foreground).overflow_y_scrollbar().child(
            v_flex()
                .p_6()
                .gap_4()
                .max_w(px(680.))
                .child(div().text_xl().font_semibold().child(t("settings")))
                .child(self.render_overview(cx))
                .child(general)
                .child(self.render_probe(cx))
                .child(transcription)
                .child(languages)
                .child(self.render_chatgpt(cx))
                .child(assistant)
                .child(self.render_categories(cx))
                .child(live)
                .child(privacy)
                .child(models)
                .child(costs),
        )
    }
}
