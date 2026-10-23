//! The About page: the identity card — hero logo, name and version,
//! what this is, where it lives, who helped.

use gpui_kit::base::Button;
use gpui_kit::component::ActiveTheme;
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
    /// this is, where it lives, who helped. One calm centered column;
    /// everything a support conversation needs.
    pub(super) fn page_about(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let entity = cx.entity();
        // ghost link button — same vocabulary as the config-file
        // buttons, each handing a URL to the session
        let link = |label: String, url: String, entity: &gpui_kit::Entity<Self>| {
            let entity = entity.clone();
            Button::new(SharedString::from(url.clone()))
                .accessibility_label(label.clone())
                .px_3()
                .h_8()
                .rounded(cx.theme().radius)
                .text_sm()
                .text_color(cx.theme().foreground)
                .border_1()
                .border_color(cx.theme().border)
                .hover(|s| s.bg(cx.theme().secondary_hover))
                .focus_visible(|s| s.border_color(cx.theme().ring))
                .on_click(move |_, _, cx| {
                    entity.update(cx, |_, _| {
                        open_external(url.clone());
                    });
                })
                .child(label)
        };
        let repository = env!("CARGO_PKG_REPOSITORY");
        // `.pt(48).p(PAGE_PAD)` used to cancel itself (p sets the top
        // too), so the hero sat flush under the header. The page is now
        // the shared scrolling body with one centered column capped at a
        // comfortable measure.
        let body = div()
            .p_6()
            .flex()
            .flex_col()
            .items_center()
            .gap_2()
            .w_full()
            .max_w(rems(28.))
            .child(img(ImageSource::Render(ABOUT_LOGO.clone())).size_24())
            .child(
                div()
                    .text_2xl()
                    .text_color(cx.theme().foreground)
                    .child("Shotori"),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!("v{}", env!("CARGO_PKG_VERSION"))),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().foreground)
                    .w_full()
                    .max_w(rems(28.))
                    .text_center()
                    .line_height(relative(1.5))
                    .child(crate::i18n::t!("settings.about.tagline").to_string()),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .justify_center()
                    .gap(px(6.))
                    .pt(px(6.))
                    .child(link(
                        crate::i18n::t!("settings.about.repository").to_string(),
                        repository.to_string(),
                        &entity,
                    ))
                    .child(link(
                        crate::i18n::t!("settings.about.issues").to_string(),
                        format!("{repository}/issues"),
                        &entity,
                    )),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .pt_1()
                    .child(crate::i18n::t!("settings.about.license").to_string()),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .w_full()
                    .max_w(rems(28.))
                    .text_center()
                    .line_height(relative(1.5))
                    .pt(px(6.))
                    .child(crate::i18n::t!("settings.about.credits").to_string()),
            );
        // Auto margins consume spare height but never become negative, so
        // short windows keep the top of the identity block reachable by scroll.
        page_body("page-about")
            .items_center()
            .child(body.my_auto().flex_shrink_0())
    }
}
