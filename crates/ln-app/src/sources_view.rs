//! Source folders (SPEC §5 F4): status per folder, add, pause, remove, "local only".

use crate::i18n::t;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::{ActiveTheme as _, IconName, Sizable as _, StyledExt as _, h_flex, scroll::ScrollableElement as _, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use ln_core::{Core, SourceRow};
use std::path::PathBuf;
use std::sync::Arc;

pub struct SourcesView {
    core: Arc<Core>,
    rows: Vec<SourceRow>,
    embedded: (i64, i64),
}

impl SourcesView {
    pub fn new(core: Arc<Core>, cx: &mut Context<Self>) -> Self {
        let mut v = SourcesView { core, rows: vec![], embedded: (0, 0) };
        v.refresh(cx);
        v
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.rows = self.core.sources();
        let st = self.core.index_stats();
        self.embedded = (st.embedded, st.chunks);
        cx.notify();
    }

    fn add(&mut self, cx: &mut Context<Self>) {
        let rx = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: true,
            prompt: Some(t("add-folder").into()),
        });
        let core = self.core.clone();
        cx.spawn(async move |this, cx: &mut AsyncApp| {
            if let Ok(Ok(Some(paths))) = rx.await {
                for p in paths {
                    core.add_source(&p);
                }
                let _ = this.update(cx, |v, cx| v.refresh(cx));
            }
        })
        .detach();
    }
}

impl Render for SourcesView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _slow = crate::app::SlowGuard(std::time::Instant::now(), "sources_view::render");
        let theme = cx.theme().clone();
        let settings = self.core.settings();
        let mut list = v_flex().gap_2();
        for r in &self.rows {
            let path = r.path.clone();
            let setting = settings.sources.iter().find(|s| s.path == r.path).cloned();
            let is_notes = matches!(r.kind, ln_index::SourceKind::Notes);
            let status = format!(
                "{} {} · {} {}{}",
                r.files,
                t("files"),
                r.indexed,
                t("indexed"),
                if r.errors > 0 { format!(" · {} {}", r.errors, t("errors")) } else { String::new() }
            );
            let mut row = h_flex()
                .gap_3()
                .p_3()
                .rounded_md()
                .border_1()
                .border_color(theme.border)
                .child(gpui_kit::component::Icon::new(if is_notes { IconName::BookOpen } else { IconName::Folder }).small())
                .child(
                    v_flex()
                        .flex_1()
                        .child(div().text_sm().child(r.path.to_string_lossy().into_owned()))
                        .child(div().text_xs().text_color(theme.muted_foreground).child(status)),
                );
            if let Some(s) = setting {
                let p2 = path.clone();
                let p3 = path.clone();
                row = row
                    .child(Checkbox::new(SharedString::from(format!("local-{}", r.id))).label("lokal").checked(s.local_only).on_change(
                        cx.listener(move |this, v: &bool, _, cx| {
                            let p = p2.clone();
                            let v = *v;
                            this.core.update_settings(|st| {
                                if let Some(x) = st.sources.iter_mut().find(|x| x.path == p) {
                                    x.local_only = v;
                                }
                            });
                            cx.notify();
                        }),
                    ))
                    .child(
                        Button::new(SharedString::from(format!("pause-{}", r.id)))
                            .xsmall()
                            .ghost()
                            .label(if s.paused { t("resume") } else { t("pause") })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let p = p3.clone();
                                this.core.update_settings(|st| {
                                    if let Some(x) = st.sources.iter_mut().find(|x| x.path == p) {
                                        x.paused = !x.paused;
                                    }
                                });
                                this.refresh(cx);
                            })),
                    )
                    .child(Button::new(SharedString::from(format!("remove-{}", r.id))).xsmall().ghost().label(t("remove")).on_click(
                        cx.listener(move |this, _, _, cx| {
                            let p: PathBuf = path.clone();
                            this.core.remove_source(&p);
                            this.refresh(cx);
                        }),
                    ));
            }
            list = list.child(row);
        }
        v_flex()
            .size_full()
            .child(
                h_flex()
                    .px_8()
                    .py_4()
                    .gap_3()
                    .child(
                        v_flex()
                            .flex_1()
                            .child(div().text_xl().font_semibold().child(t("sources-title")))
                            .child(div().text_sm().text_color(theme.muted_foreground).child(t("sources-hint"))),
                    )
                    .child(
                        Button::new("add")
                            .primary()
                            .icon(IconName::Plus)
                            .label(t("add-folder"))
                            .on_click(cx.listener(|this, _, _, cx| this.add(cx))),
                    ),
            )
            .child(div().id("sources-list").flex_1().px_8().overflow_y_scrollbar().child(list))
            .when(self.embedded.0 < self.embedded.1, |el| {
                let pct = self.embedded.0 as f32 * 100.0 / self.embedded.1.max(1) as f32;
                el.child(div().px_8().py_3().text_xs().text_color(theme.muted_foreground).child(format!(
                    "{} {:.0} %",
                    t("preparing-search"),
                    pct
                )))
            })
    }
}
