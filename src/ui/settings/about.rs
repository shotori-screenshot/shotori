//! The About page: the identity card — hero logo, name and version,
//! what this is, where it lives, who helped, and one-click environment info.

use gpui_kit::assets::IconName;
use gpui_kit::base::Button;
use gpui_kit::component::{ActiveTheme, Icon};
use gpui_kit::*;

use super::{SettingsWindow, open_external, page_body, png_render_image};

/// The About page's hero logo (256px master — plenty for a 128px
/// display size on HiDPI).
static ABOUT_LOGO: std::sync::LazyLock<std::sync::Arc<RenderImage>> =
    std::sync::LazyLock::new(|| {
        png_render_image(include_bytes!("../../../assets/app/shotori-256.png"))
    });

impl SettingsWindow {
    /// About: the identity card — hero logo, name and version, what
    /// this is, where it lives, who helped, and copyable support info.
    pub(super) fn page_about(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let s = super::palette::surfaces(cx);
        let pill_hover = s.pill_hover;
        let entity = cx.entity();
        let link = |label: String, icon: IconName, url: String, entity: &gpui_kit::Entity<Self>| {
            let entity = entity.clone();
            Button::new(SharedString::from(url.clone()))
                .accessibility_label(label.clone())
                .flex()
                .items_center()
                .gap_1p5()
                .px_3p5()
                .h_8()
                .rounded_full()
                .text_xs()
                .font_weight(FontWeight::MEDIUM)
                .text_color(cx.theme().foreground)
                .border_1()
                .border_color(s.hairline)
                .bg(s.card_bg)
                .shadow(s.card_shadow.clone())
                .hover(move |st| st.bg(pill_hover))
                .focus_visible(|st| st.border_color(cx.theme().ring))
                .on_click(move |_, _, cx| {
                    entity.update(cx, |_, _| {
                        open_external(url.clone());
                    });
                })
                .child(
                    Icon::new(icon)
                        .size_3p5()
                        .text_color(cx.theme().muted_foreground),
                )
                .child(label)
        };

        let copy_label = if self.about_copied_flash {
            crate::i18n::t!("settings.about.copied").to_string()
        } else {
            crate::i18n::t!("settings.about.copy_info").to_string()
        };
        let copy_btn = {
            let entity = entity.clone();
            Button::new("about-copy-sysinfo")
                .accessibility_label(copy_label.clone())
                .flex()
                .items_center()
                .gap_1p5()
                .px_3p5()
                .h_8()
                .rounded_full()
                .text_xs()
                .font_weight(FontWeight::MEDIUM)
                .text_color(cx.theme().foreground)
                .border_1()
                .border_color(s.hairline)
                .bg(s.card_bg)
                .shadow(s.card_shadow.clone())
                .hover(move |st| st.bg(pill_hover))
                .focus_visible(|st| st.border_color(cx.theme().ring))
                .on_click(move |_, _, cx| {
                    entity.update(cx, |this, cx| {
                        this.copy_diagnostic_report(true, cx);
                    });
                })
                .child(
                    Icon::new(if self.about_copied_flash {
                        IconName::Check
                    } else {
                        IconName::Copy
                    })
                    .size_3p5()
                    .text_color(if self.about_copied_flash {
                        cx.theme().primary
                    } else {
                        cx.theme().muted_foreground
                    }),
                )
                .child(copy_label)
        };

        let repository = env!("CARGO_PKG_REPOSITORY");

        let tech_item = |icon: IconName, text: String| {
            div()
                .flex()
                .items_center()
                .gap_3()
                .w_full()
                .child(
                    div()
                        .size_7()
                        .rounded(px(8.))
                        .bg(cx.theme().primary.opacity(0.12))
                        .flex()
                        .items_center()
                        .justify_center()
                        .flex_shrink_0()
                        .text_color(cx.theme().primary)
                        .child(Icon::new(icon).size_3p5()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_xs()
                        .line_height(relative(1.5))
                        .text_color(cx.theme().muted_foreground)
                        .child(text),
                )
        };

        let body = div()
            .p_6()
            .flex()
            .flex_col()
            .items_center()
            .gap_3p5()
            .w_full()
            .max_w(rems(34.))
            .child(
                div()
                    .p_3()
                    .rounded(px(24.))
                    .bg(s.card_bg)
                    .border_1()
                    .border_color(s.hairline)
                    .shadow(s.card_shadow.clone())
                    .child(img(ImageSource::Render(ABOUT_LOGO.clone())).size_24()),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2p5()
                    .child(
                        div()
                            .text_2xl()
                            .font_weight(FontWeight::BOLD)
                            .text_color(cx.theme().foreground)
                            .child("Shotori"),
                    )
                    .child(
                        div()
                            .px_2p5()
                            .py_0p5()
                            .rounded_full()
                            .border_1()
                            .border_color(cx.theme().primary.opacity(0.35))
                            .bg(s.selected_card_bg)
                            .text_xs()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(cx.theme().primary)
                            .child(format!("v{}", env!("CARGO_PKG_VERSION"))),
                    ),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().foreground)
                    .w_full()
                    .max_w(rems(30.))
                    .text_center()
                    .line_height(relative(1.55))
                    .child(crate::i18n::t!("settings.about.tagline").to_string()),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .justify_center()
                    .gap_2()
                    .pt_1()
                    .child(link(
                        crate::i18n::t!("settings.about.repository").to_string(),
                        IconName::ExternalLink,
                        repository.to_string(),
                        &entity,
                    ))
                    .child(link(
                        crate::i18n::t!("settings.about.issues").to_string(),
                        IconName::Info,
                        format!("{repository}/issues"),
                        &entity,
                    ))
                    .child(copy_btn),
            )
            .child(
                div()
                    .w_full()
                    .mt_2()
                    .p(px(18.))
                    .flex()
                    .flex_col()
                    .gap_3()
                    .rounded(px(14.))
                    .border_1()
                    .border_color(s.hairline)
                    .bg(s.card_bg)
                    .shadow(s.card_shadow)
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(cx.theme().muted_foreground)
                            .child(crate::i18n::t!("settings.about.tech_title").to_string()),
                    )
                    .child(tech_item(
                        IconName::Palette,
                        crate::i18n::t!("settings.about.tech_ui").to_string(),
                    ))
                    .child(tech_item(
                        IconName::ScanText,
                        crate::i18n::t!("settings.about.tech_ocr").to_string(),
                    ))
                    .child(tech_item(
                        IconName::Sparkles,
                        crate::i18n::t!("settings.about.tech_themes").to_string(),
                    )),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .pt_1()
                    .child(crate::i18n::t!("settings.about.license").to_string()),
            );

        page_body("page-about")
            .items_center()
            .child(body.my_auto().flex_shrink_0())
    }
}
