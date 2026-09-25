//! Installer Studio's IDE-style layout: a toolbar, an explorer/workspace/
//! inspector row and a bottom Output/Problems/Build/Doctor panel, all
//! resizable and collapsible, plus a command palette overlay.

use gpui_kit::base::{Disableable as _, h_flex, h_resizable, resizable_panel, v_flex, v_resizable};
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::input::Input;
use gpui_kit::component::scroll::ScrollableElement as _;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use inst_i18n::Language;
use inst_i18n::studio::Msg;

use crate::state::{BottomTab, Section, StudioView};

fn t(msg: Msg, lang: Language) -> &'static str {
    msg.text(lang)
}

impl Render for StudioView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        self.sync_and_validate(cx);
        let theme = cx.theme().clone();

        v_flex()
            .id("studio-root")
            .size_full()
            .text_color(theme.foreground)
            .child(self.render_toolbar(window, cx))
            .child(
                div()
                    .flex_1()
                    .min_h(px(0.))
                    .child(self.render_body(window, cx)),
            )
            .when(self.palette_open, |d| d.child(self.render_palette(cx)))
    }
}

impl StudioView {
    fn render_toolbar(&self, _window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let lang = self.lang;
        let has_project = self.project.is_some();
        let dirty = self.project.as_ref().is_some_and(|p| p.dirty);
        let advanced = self.is_advanced();

        h_flex()
            .id("toolbar")
            .h(px(44.))
            .px_3()
            .gap_2()
            .items_center()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .child(inst_brand::STUDIO_NAME),
            )
            .child(
                Button::new("new")
                    .ghost()
                    .compact()
                    .label(t(Msg::New, lang))
                    .on_click(cx.listener(|view, _, window, cx| view.pick_new_project(window, cx))),
            )
            .child(
                Button::new("open")
                    .ghost()
                    .compact()
                    .label(t(Msg::Open, lang))
                    .on_click(
                        cx.listener(|view, _, window, cx| view.pick_open_project(window, cx)),
                    ),
            )
            .child(
                Button::new("save")
                    .ghost()
                    .compact()
                    .disabled(!has_project)
                    .label(if dirty {
                        format!("{} *", t(Msg::Save, lang))
                    } else {
                        t(Msg::Save, lang).to_owned()
                    })
                    .on_click(cx.listener(|view, _, _, cx| view.save_project(cx))),
            )
            .child(
                Button::new("analyze")
                    .ghost()
                    .compact()
                    .disabled(!has_project)
                    .label(t(Msg::Analyze, lang))
                    .on_click(cx.listener(|view, _, window, cx| view.run_analyze(window, cx))),
            )
            .child(
                Button::new("doctor")
                    .ghost()
                    .compact()
                    .disabled(!has_project)
                    .label(t(Msg::RunDoctor, lang))
                    .on_click(cx.listener(|view, _, _, cx| view.run_doctor(cx))),
            )
            .child(
                Button::new("build")
                    .primary()
                    .compact()
                    .disabled(!has_project || self.busy.is_some())
                    .label(t(Msg::Build, lang))
                    .on_click(cx.listener(|view, _, _, cx| view.run_build(cx))),
            )
            .child(div().flex_1())
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        Button::new("mode-simple")
                            .compact()
                            .when(!advanced, |b| b.primary())
                            .label(t(Msg::Simple, lang))
                            .on_click(cx.listener(|view, _, _, cx| {
                                view.set_advanced(false);
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("mode-advanced")
                            .compact()
                            .when(advanced, |b| b.primary())
                            .label(t(Msg::Advanced, lang))
                            .on_click(cx.listener(|view, _, _, cx| {
                                view.set_advanced(true);
                                cx.notify();
                            })),
                    ),
            )
            .child(
                Button::new("palette")
                    .ghost()
                    .compact()
                    .label(format!("⌘⇧P {}", t(Msg::CommandPalette, lang)))
                    .on_click(cx.listener(|view, _, _, cx| {
                        view.palette_open = true;
                        cx.notify();
                    })),
            )
    }

    fn render_body(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let main_row = h_resizable("main-row")
            .with_state(&self.h_split)
            .when(self.show_explorer, |g| {
                g.child(
                    resizable_panel()
                        .size(px(220.))
                        .size_range(px(160.)..px(420.))
                        .child(self.render_explorer(cx)),
                )
            })
            .child(
                resizable_panel()
                    .size(px(560.))
                    .child(self.render_workspace(window, cx)),
            )
            .when(self.show_inspector, |g| {
                g.child(
                    resizable_panel()
                        .size(px(280.))
                        .size_range(px(200.)..px(460.))
                        .child(self.render_inspector(cx)),
                )
            });

        v_resizable("main-column")
            .with_state(&self.v_split)
            .child(resizable_panel().size(px(520.)).child(main_row))
            .when(self.show_bottom, |g| {
                g.child(
                    resizable_panel()
                        .size(px(220.))
                        .size_range(px(120.)..px(480.))
                        .child(self.render_bottom_panel(cx)),
                )
            })
    }

    fn render_explorer(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let lang = self.lang;
        let theme = cx.theme().clone();
        let mut list = v_flex().size_full().bg(theme.sidebar).p_2().gap_1().child(
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(t(inst_i18n::studio::Msg::Explorer, lang).to_uppercase()),
        );
        if self.project.is_some() {
            for &section in self.visible_sections() {
                let selected = self.section == section;
                list = list.child(
                    Button::new(SharedString::from(format!("section-{section:?}")))
                        .ghost()
                        .w_full()
                        .when(selected, |b| b.secondary())
                        .label(section_label(section, lang))
                        .on_click(cx.listener(move |view, _, _, cx| {
                            view.section = section;
                            cx.notify();
                        })),
                );
            }
        }
        list
    }

    fn render_workspace(
        &mut self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
        let theme = cx.theme().clone();
        let lang = self.lang;
        let content: AnyElement = if self.project.is_some() {
            crate::properties::render_section(self, cx).into_any_element()
        } else {
            v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .gap_3()
                .child(
                    div()
                        .text_color(theme.muted_foreground)
                        .child(t(Msg::NoProjectOpen, lang)),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            Button::new("welcome-new")
                                .primary()
                                .label(t(Msg::New, lang))
                                .on_click(cx.listener(|view, _, window, cx| {
                                    view.pick_new_project(window, cx)
                                })),
                        )
                        .child(
                            Button::new("welcome-open")
                                .ghost()
                                .label(t(Msg::Open, lang))
                                .on_click(cx.listener(|view, _, window, cx| {
                                    view.pick_open_project(window, cx)
                                })),
                        ),
                )
                .into_any_element()
        };
        let _ = window;
        div()
            .size_full()
            .p_4()
            .overflow_y_scrollbar()
            .child(content)
    }

    fn render_inspector(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let lang = self.lang;
        let mut column = v_flex().size_full().p_3().gap_2().bg(theme.sidebar);
        column = column.child(
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(t(Msg::Properties, lang).to_uppercase()),
        );
        if let Some(p) = &self.project {
            column = column
                .child(inspector_row("ID", p.project.product.id.as_str()))
                .child(inspector_row(
                    "Source",
                    &p.project.application.source.display().to_string(),
                ))
                .child(inspector_row(
                    "Diagnostics",
                    &format!(
                        "{} error(s), {} warning(s)",
                        errors(&self.diagnostics),
                        warnings(&self.diagnostics)
                    ),
                ));
            if let Some(report) = &p.doctor {
                column = column.child(inspector_row(
                    "Doctor findings",
                    &report.findings.len().to_string(),
                ));
            }
        }
        column
    }

    fn render_bottom_panel(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let lang = self.lang;
        let theme = cx.theme().clone();
        let tabs = [
            (BottomTab::Output, Msg::Output),
            (BottomTab::Problems, Msg::Problems),
            (BottomTab::Build, Msg::BuildTab),
            (BottomTab::Doctor, Msg::Doctor),
        ];
        let mut root = v_flex().size_full().bg(theme.background);
        let mut tab_row = h_flex()
            .gap_1()
            .px_2()
            .pt_1()
            .border_b_1()
            .border_color(theme.border);
        for (tab, msg) in tabs {
            let selected = self.bottom_tab == tab;
            tab_row = tab_row.child(
                Button::new(SharedString::from(format!("tab-{tab:?}")))
                    .ghost()
                    .compact()
                    .when(selected, |b| b.secondary())
                    .label(t(msg, lang))
                    .on_click(cx.listener(move |view, _, _, cx| {
                        view.bottom_tab = tab;
                        cx.notify();
                    })),
            );
        }
        root = root.child(tab_row);
        let body = match self.bottom_tab {
            BottomTab::Output => self.render_output(lang),
            BottomTab::Problems => self.render_problems(lang),
            BottomTab::Build => self.render_output(lang),
            BottomTab::Doctor => self.render_doctor(cx, lang),
        };
        root.child(
            div()
                .flex_1()
                .min_h(px(0.))
                .overflow_y_scrollbar()
                .p_2()
                .child(body),
        )
    }

    fn render_output(&self, lang: Language) -> AnyElement {
        let mut lines: Vec<String> = Vec::new();
        if let Some(busy) = &self.busy {
            lines.extend(crate::state::lock_progress(busy).lines.clone());
        } else if let Some(p) = &self.project
            && let Some(a) = &p.last_analysis
        {
            lines.extend(a.lines().map(str::to_owned));
        }
        if lines.is_empty() {
            lines.push(t(Msg::Ready, lang).to_owned());
        }
        v_flex()
            .gap_0p5()
            .children(
                lines
                    .into_iter()
                    .map(|l| div().text_sm().font_family("monospace").child(l)),
            )
            .into_any_element()
    }

    fn render_problems(&self, lang: Language) -> AnyElement {
        if self.diagnostics.is_empty() {
            return div()
                .text_sm()
                .child(t(Msg::DoctorNoIssues, lang))
                .into_any_element();
        }
        v_flex()
            .gap_1()
            .children(
                self.diagnostics
                    .iter()
                    .map(|d| div().text_sm().child(d.to_string())),
            )
            .into_any_element()
    }

    fn render_doctor(&self, cx: &mut Context<'_, Self>, lang: Language) -> AnyElement {
        let Some(p) = &self.project else {
            return div().into_any_element();
        };
        let Some(report) = &p.doctor else {
            return div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(t(Msg::RunDoctor, lang))
                .into_any_element();
        };
        if report.findings.is_empty() {
            return div()
                .text_sm()
                .child(t(Msg::DoctorNoIssues, lang))
                .into_any_element();
        }
        let mut column = v_flex().gap_2();
        if !report.safe_fixes().is_empty() {
            column = column.child(
                Button::new("apply-all-safe")
                    .ghost()
                    .compact()
                    .label(t(Msg::ApplyAllSafeFixes, lang))
                    .on_click(cx.listener(|view, _, _, cx| view.apply_safe_fixes(cx))),
            );
        }
        for finding in &report.findings {
            let mut row = v_flex().gap_1().child(div().text_sm().child(format!(
                "[{}] {}: {}",
                finding.code, finding.location, finding.message
            )));
            for fix in finding.fixes.iter().filter(|f| f.is_safe()) {
                let fix = fix.clone();
                row = row.child(
                    Button::new(SharedString::from(format!(
                        "fix-{}-{}",
                        finding.code,
                        fix.label()
                    )))
                    .ghost()
                    .compact()
                    .label(format!("{} ({})", t(Msg::ApplyFix, lang), fix.label()))
                    .on_click(cx.listener(move |view, _, _, cx| {
                        if let Some(p) = &mut view.project {
                            fix.apply(&mut p.project);
                            p.dirty = true;
                        }
                        view.run_doctor(cx);
                    })),
                );
            }
            column = column.child(row);
        }
        column.into_any_element()
    }

    fn render_palette(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let lang = self.lang;
        let theme = cx.theme().clone();
        let has_project = self.project.is_some();
        let actions: Vec<(&'static str, bool)> = vec![
            (t(Msg::New, lang), true),
            (t(Msg::Open, lang), true),
            (t(Msg::Save, lang), has_project),
            (t(Msg::Build, lang), has_project),
            (t(Msg::RunDoctor, lang), has_project),
            (t(Msg::Analyze, lang), has_project),
        ];
        div()
            .id("palette-overlay")
            .absolute()
            .inset_0()
            .flex()
            .items_start()
            .justify_center()
            .pt_20()
            .bg(gpui_kit::rgba(0x00000099))
            .on_click(cx.listener(|view, _, _, cx| {
                view.palette_open = false;
                cx.notify();
            }))
            .child(
                v_flex()
                    .id("palette")
                    .w(px(480.))
                    .max_h(px(360.))
                    .bg(theme.background)
                    .border_1()
                    .border_color(theme.border)
                    .rounded_lg()
                    .p_2()
                    .gap_1()
                    .child(Input::new(&self.palette_query).id("palette-input"))
                    .children(
                        actions
                            .into_iter()
                            .enumerate()
                            .map(|(i, (label, enabled))| {
                                Button::new(SharedString::from(format!("palette-{i}")))
                                    .ghost()
                                    .w_full()
                                    .disabled(!enabled)
                                    .label(label)
                                    .on_click(cx.listener(move |view, _, window, cx| {
                                        view.palette_open = false;
                                        view.run_palette_action(i, window, cx);
                                    }))
                            }),
                    ),
            )
    }
}

fn section_label(section: Section, lang: Language) -> &'static str {
    match section {
        Section::Product => t(Msg::ProductSection, lang),
        Section::Install => t(Msg::InstallSection, lang),
        Section::Ui => t(Msg::UiSection, lang),
        Section::Policy => t(Msg::PolicySection, lang),
        Section::Targets => t(Msg::TargetsSection, lang),
        Section::Compression => t(Msg::CompressionSection, lang),
        Section::Signing => t(Msg::SigningSection, lang),
        Section::Update => t(Msg::UpdateSection, lang),
    }
}

fn inspector_row(label: &str, value: &str) -> impl IntoElement {
    v_flex()
        .gap_0p5()
        .child(div().text_xs().child(label.to_owned()))
        .child(div().text_sm().child(value.to_owned()))
}

fn errors(diags: &[inst_model::validate::Diagnostic]) -> usize {
    diags
        .iter()
        .filter(|d| d.severity == inst_model::validate::Severity::Error)
        .count()
}

fn warnings(diags: &[inst_model::validate::Diagnostic]) -> usize {
    diags
        .iter()
        .filter(|d| d.severity == inst_model::validate::Severity::Warning)
        .count()
}
