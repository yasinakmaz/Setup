//! The property editor: one form per Explorer section, bound directly to
//! the open project. Text fields are synced from their `InputState` once
//! per render (see `StudioView::sync_and_validate`); everything else here
//! mutates the project immediately on click.

use gpui_kit::base::{h_flex, v_flex};
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::Input;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use inst_i18n::Language;
use inst_i18n::studio::Msg;
use inst_model::platform::Target;
use inst_model::project::CompressionProfile;

use crate::state::{Section, StudioView};

fn t(msg: Msg, lang: Language) -> &'static str {
    msg.text(lang)
}

fn field_row(label: &str, control: impl IntoElement) -> impl IntoElement {
    v_flex()
        .gap_1()
        .child(div().text_sm().child(label.to_owned()))
        .child(control)
}

fn toggle_row(
    id: &'static str,
    label: &str,
    checked: bool,
    on_click: impl Fn(&mut StudioView, &mut Context<'_, StudioView>) + 'static,
    cx: &mut Context<'_, StudioView>,
) -> impl IntoElement {
    Checkbox::new(id)
        .label(label.to_owned())
        .checked(checked)
        .on_click(cx.listener(move |view, _, _, cx| {
            on_click(view, cx);
            cx.notify();
        }))
}

pub fn render_section(view: &mut StudioView, cx: &mut Context<'_, StudioView>) -> AnyElement {
    match view.section {
        Section::Product => render_product(view, cx),
        Section::Install => render_install(view, cx),
        Section::Ui => render_ui(view, cx),
        Section::Policy => render_policy(view, cx),
        Section::Targets => render_targets(view, cx),
        Section::Compression => render_compression(view, cx),
        Section::Signing => render_signing(view, cx),
        Section::Update => render_update(view, cx),
    }
}

fn render_product(view: &mut StudioView, cx: &mut Context<'_, StudioView>) -> AnyElement {
    let lang = view.lang;
    let Some(p) = &view.project else {
        return div().into_any_element();
    };
    let mut col = v_flex()
        .gap_4()
        .child(field_row(
            t(Msg::Name, lang),
            Input::new(&p.product_fields.name).id("name"),
        ))
        .child(field_row(
            t(Msg::Identifier, lang),
            div()
                .text_sm()
                .child(p.project.product.id.as_str().to_owned()),
        ))
        .child(field_row(
            t(Msg::Publisher, lang),
            Input::new(&p.product_fields.publisher).id("publisher"),
        ))
        .child(field_row(
            "Version",
            Input::new(&p.product_fields.version).id("version"),
        ));
    if let Some(err) = &p.version_error {
        col = col.child(
            div()
                .text_sm()
                .text_color(cx.theme().danger)
                .child(err.clone()),
        );
    }
    col = col.child(field_row(
        t(Msg::Description, lang),
        Input::new(&p.product_fields.description).id("description"),
    ));
    col.into_any_element()
}

fn render_install(view: &mut StudioView, cx: &mut Context<'_, StudioView>) -> AnyElement {
    let lang = view.lang;
    let Some(p) = &view.project else {
        return div().into_any_element();
    };
    let checked = p.project.install.allow_change_location;
    v_flex()
        .gap_4()
        .child(toggle_row(
            "allow-change-location",
            t(Msg::AllowChangeLocation, lang),
            checked,
            |view, _| {
                if let Some(p) = &mut view.project {
                    p.project.install.allow_change_location =
                        !p.project.install.allow_change_location;
                    p.dirty = true;
                }
            },
            cx,
        ))
        .child(field_row(
            "Privileges",
            div()
                .text_sm()
                .child(format!("{:?}", p.project.install.privileges)),
        ))
        .into_any_element()
}

fn render_ui(view: &mut StudioView, cx: &mut Context<'_, StudioView>) -> AnyElement {
    let lang = view.lang;
    let Some(p) = &view.project else {
        return div().into_any_element();
    };
    let enabled_languages = p.project.ui.languages.clone();
    let fallback = p.project.ui.fallback_language;
    let language_selector = p.project.ui.language_selector;
    let show_license = p.project.ui.show_license;

    let mut lang_rows = h_flex().gap_2().flex_wrap();
    for l in Language::ALL {
        let on = enabled_languages.contains(&l);
        lang_rows = lang_rows.child(
            Button::new(SharedString::from(format!("ui-lang-{}", l.code())))
                .compact()
                .when(on, |b| b.primary())
                .label(l.native_name())
                .on_click(cx.listener(move |view, _, _, cx| {
                    if let Some(p) = &mut view.project {
                        let langs = &mut p.project.ui.languages;
                        if let Some(ix) = langs.iter().position(|x| *x == l) {
                            if langs.len() > 1 {
                                langs.remove(ix);
                            }
                        } else {
                            langs.push(l);
                        }
                        if !langs.contains(&p.project.ui.fallback_language) {
                            p.project.ui.fallback_language = langs[0];
                        }
                        p.dirty = true;
                    }
                    cx.notify();
                })),
        );
    }

    let mut fallback_rows = h_flex().gap_2().flex_wrap();
    for l in enabled_languages.iter().copied() {
        let on = l == fallback;
        fallback_rows = fallback_rows.child(
            Button::new(SharedString::from(format!("ui-fallback-{}", l.code())))
                .compact()
                .when(on, |b| b.primary())
                .label(l.native_name())
                .on_click(cx.listener(move |view, _, _, cx| {
                    if let Some(p) = &mut view.project {
                        p.project.ui.fallback_language = l;
                        p.dirty = true;
                    }
                    cx.notify();
                })),
        );
    }

    v_flex()
        .gap_4()
        .child(field_row(t(Msg::Languages, lang), lang_rows))
        .child(field_row(t(Msg::FallbackLanguage, lang), fallback_rows))
        .child(toggle_row(
            "language-selector",
            "Show a language selector",
            language_selector,
            |view, _| {
                if let Some(p) = &mut view.project {
                    p.project.ui.language_selector = !p.project.ui.language_selector;
                    p.dirty = true;
                }
            },
            cx,
        ))
        .child(toggle_row(
            "show-license",
            "Show the license before installing",
            show_license,
            |view, _| {
                if let Some(p) = &mut view.project {
                    p.project.ui.show_license = !p.project.ui.show_license;
                    p.dirty = true;
                }
            },
            cx,
        ))
        .into_any_element()
}

fn render_policy(view: &mut StudioView, cx: &mut Context<'_, StudioView>) -> AnyElement {
    let lang = view.lang;
    let Some(p) = &view.project else {
        return div().into_any_element();
    };
    let policy = p.project.policy.clone();
    v_flex()
        .gap_3()
        .child(toggle_row(
            "rollback",
            t(Msg::Rollback, lang),
            policy.rollback,
            |view, _| {
                if let Some(p) = &mut view.project {
                    p.project.policy.rollback = !p.project.policy.rollback;
                    p.dirty = true;
                }
            },
            cx,
        ))
        .child(toggle_row(
            "silent-install",
            t(Msg::SilentInstall, lang),
            policy.silent_install,
            |view, _| {
                if let Some(p) = &mut view.project {
                    p.project.policy.silent_install = !p.project.policy.silent_install;
                    p.dirty = true;
                }
            },
            cx,
        ))
        .child(toggle_row(
            "silent-uninstall",
            t(Msg::SilentUninstall, lang),
            policy.silent_uninstall,
            |view, _| {
                if let Some(p) = &mut view.project {
                    p.project.policy.silent_uninstall = !p.project.policy.silent_uninstall;
                    p.dirty = true;
                }
            },
            cx,
        ))
        .child(toggle_row(
            "telemetry",
            t(Msg::Telemetry, lang),
            policy.telemetry,
            |view, _| {
                if let Some(p) = &mut view.project {
                    p.project.policy.telemetry = !p.project.policy.telemetry;
                    p.dirty = true;
                }
            },
            cx,
        ))
        .into_any_element()
}

fn render_targets(view: &mut StudioView, cx: &mut Context<'_, StudioView>) -> AnyElement {
    let Some(p) = &view.project else {
        return div().into_any_element();
    };
    let targets = p.project.targets.clone();
    let mut col = v_flex().gap_2();
    for target in Target::SUPPORTED {
        let on = targets.enabled().contains(&target);
        col = col.child(
            Checkbox::new(SharedString::from(format!("target-{target}")))
                .label(target.to_string())
                .checked(on)
                .on_click(cx.listener(move |view, _, _, cx| {
                    if let Some(p) = &mut view.project {
                        let enabled = p.project.targets.enabled().contains(&target);
                        p.project.targets.set(target, !enabled);
                        p.dirty = true;
                    }
                    cx.notify();
                })),
        );
    }
    col.into_any_element()
}

fn render_compression(view: &mut StudioView, cx: &mut Context<'_, StudioView>) -> AnyElement {
    let lang = view.lang;
    let Some(p) = &view.project else {
        return div().into_any_element();
    };
    let current = p.project.compression.profile;
    let options = [
        (CompressionProfile::Auto, "Auto"),
        (CompressionProfile::SmallestSize, "Smallest"),
        (CompressionProfile::Balanced, "Balanced"),
        (CompressionProfile::FastInstall, "Fast"),
        (CompressionProfile::None, "None"),
    ];
    let mut row = h_flex().gap_2().flex_wrap();
    for (profile, label) in options {
        let on = profile == current;
        row = row.child(
            Button::new(SharedString::from(format!("compression-{label}")))
                .compact()
                .when(on, |b| b.primary())
                .label(label)
                .on_click(cx.listener(move |view, _, _, cx| {
                    if let Some(p) = &mut view.project {
                        p.project.compression.profile = profile;
                        p.dirty = true;
                    }
                    cx.notify();
                })),
        );
    }
    field_row(t(Msg::CompressionProfile, lang), row).into_any_element()
}

fn render_signing(view: &mut StudioView, cx: &mut Context<'_, StudioView>) -> AnyElement {
    let lang = view.lang;
    let Some(p) = &view.project else {
        return div().into_any_element();
    };
    let require_signed = p.project.signing.require_signed;
    toggle_row(
        "require-signed",
        t(Msg::RequireSigned, lang),
        require_signed,
        |view, _| {
            if let Some(p) = &mut view.project {
                p.project.signing.require_signed = !p.project.signing.require_signed;
                p.dirty = true;
            }
        },
        cx,
    )
    .into_any_element()
}

fn render_update(view: &mut StudioView, cx: &mut Context<'_, StudioView>) -> AnyElement {
    let lang = view.lang;
    let Some(p) = &view.project else {
        return div().into_any_element();
    };
    let check = p.project.update.check_for_updates_default;
    v_flex()
        .gap_4()
        .child(toggle_row(
            "check-updates",
            t(Msg::CheckForUpdates, lang),
            check,
            |view, _| {
                if let Some(p) = &mut view.project {
                    p.project.update.check_for_updates_default =
                        !p.project.update.check_for_updates_default;
                    p.dirty = true;
                }
            },
            cx,
        ))
        .child(field_row(
            "Feed URL",
            Input::new(&p.update_fields.feed_url).id("feed-url"),
        ))
        .into_any_element()
}
