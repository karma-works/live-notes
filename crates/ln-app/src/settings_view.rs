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
use ln_types::{ChineseScript, Lang, SuggestionFrequency, TranscriptionMode, UiLanguage};
use std::collections::HashMap;
use std::sync::Arc;

pub struct SettingsView {
    core: Arc<Core>,
    openai_key: Entity<InputState>,
    openrouter_key: Entity<InputState>,
    assistant_model: Entity<InputState>,
    limit: Entity<InputState>,
    tested: HashMap<String, Result<(), String>>,
    probe: Option<ln_core::ProbeReport>,
    probing: bool,
}

const EXTRA_LANGS: [&str; 6] = ["it", "pt", "nl", "ja", "ko", "ru"];

impl SettingsView {
    pub fn new(core: Arc<Core>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let s = core.settings();
        let openai_key = cx.new(|cx| InputState::new(window, cx).placeholder("sk-…").masked(true));
        let openrouter_key = cx.new(|cx| InputState::new(window, cx).placeholder("sk-or-…").masked(true));
        let assistant_model = cx.new(|cx| InputState::new(window, cx).default_value(s.roles.assistant.model.clone()));
        let limit = cx.new(|cx| InputState::new(window, cx).default_value(format!("{:.2}", s.costs.monthly_limit_usd)));
        SettingsView { core, openai_key, openrouter_key, assistant_model, limit, tested: HashMap::new(), probe: None, probing: false }
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
                    .child(
                        Button::new(SharedString::from(format!("test-{provider}"))).small().ghost().label(t("test-connection")).on_click(
                            cx.listener(move |this, _, _, cx| {
                                this.tested.remove(provider);
                                this.core.test_provider(provider);
                                cx.notify();
                            }),
                        ),
                    ),
            )
            .when_some(status, |el, (s, c)| el.child(div().text_xs().text_color(c).child(s)))
    }
}

fn gb(bytes: u64) -> String {
    let g = bytes as f64 / 1e9;
    if g < 1.0 { format!("{:.0} MB", g * 1000.0) } else { format!("{g:.1} GB") }
}

impl Render for SettingsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
        let transcription =
            Self::section(t("transcription"), cx).child(self.key_row("openai", t("openai-key"), &self.openai_key.clone(), cx)).child(
                RadioGroup::new("mode")
                    .children(if has_openai { vec![t("mode-local"), t("mode-cheap"), t("mode-live")] } else { vec![t("mode-local")] })
                    .selected_index(Some(if has_openai { mode_idx } else { 0 }))
                    .on_change(cx.listener(|this, i: &usize, _, cx| {
                        let m = match i {
                            1 => TranscriptionMode::OpenAiCheap,
                            2 => TranscriptionMode::OpenAiLive,
                            _ => TranscriptionMode::Local,
                        };
                        this.core.update_settings(|s| s.transcription.mode = m);
                        cx.notify();
                    })),
            );

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

        // Assistant
        let prov_idx = match s.roles.assistant.provider.as_str() {
            "openrouter" => 0,
            "local" => 1,
            _ => 2,
        };
        let model_input = self.assistant_model.clone();
        let assistant = Self::section(t("assistant"), cx)
            .child(
                RadioGroup::horizontal("prov")
                    .children(["OpenRouter", t("mode-local"), "Ollama"])
                    .selected_index(Some(prov_idx))
                    .on_change(cx.listener(|this, i: &usize, window, cx| {
                        let (p, m) = match i {
                            0 => ("openrouter", "google/gemma-4-26b-a4b-it".to_string()),
                            1 => ("local", ln_models::recommended_llm(Core::ram_gb()).id.to_string()),
                            _ => ("ollama", "gemma4".to_string()),
                        };
                        this.core.update_settings(|s| s.roles.assistant = ln_types::RoleConfig { provider: p.into(), model: m.clone() });
                        this.assistant_model.update(cx, |st, cx| st.set_value(m, window, cx));
                        if p == "local" {
                            this.core.download_models(&[ln_models::recommended_llm(Core::ram_gb()).id]);
                        }
                        cx.notify();
                    })),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(div().text_xs().text_color(theme.muted_foreground).child(t("model")))
                    .child(div().flex_1().child(Input::new(&self.assistant_model)))
                    .child(Button::new("save-model").small().label(t("save-key")).on_click(cx.listener(move |this, _, _, cx| {
                        let m = model_input.read(cx).value().to_string();
                        this.core.update_settings(|s| s.roles.assistant.model = m);
                        cx.notify();
                    }))),
            )
            .when(prov_idx == 0, |el| el.child(self.key_row("openrouter", t("openrouter-key"), &self.openrouter_key.clone(), cx)))
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
                .max_w(px(600.))
                .child(general)
                .child(self.render_probe(cx))
                .child(transcription)
                .child(languages)
                .child(assistant)
                .child(live)
                .child(privacy)
                .child(models)
                .child(costs),
        )
    }
}
