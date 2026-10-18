//! The About page: the identity card — hero logo, name and version,
//! what this is, where it lives, who helped.

use gpui_kit::base::Button;
use gpui_kit::*;

use crate::ui::theme;

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
        let t = theme::c();
        let entity = cx.entity();
        // ghost link button — same vocabulary as the config-file
        // buttons, each handing a URL to the session
        let link = |label: String, url: String, entity: &gpui_kit::Entity<Self>| {
            let entity = entity.clone();
            Button::new(SharedString::from(url.clone()))
                .accessibility_label(label.clone())
                .px(px(10.))
                .py(px(3.))
                .rounded(px(6.))
                .text_size(px(12.))
                .text_color(rgba(t.btn_text))
                .border_1()
                .border_color(rgba(t.toolbar_border))
                .hover(|s| s.bg(rgba(t.toolbar_hover)))
                .focus_visible(|s| s.border_color(rgba(t.accent)))
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
            .flex()
            .flex_col()
            .items_center()
            .gap(px(8.))
            .max_w(px(440.))
            .child(img(ImageSource::Render(ABOUT_LOGO.clone())).size(px(96.)))
            .child(
                div()
                    .text_size(px(22.))
                    .text_color(rgba(t.btn_text))
                    .child("Shotori"),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgba(t.hint_text))
                    .child(format!("v{}", env!("CARGO_PKG_VERSION"))),
            )
            .child(
                div()
                    .text_size(px(13.))
                    .text_color(rgba(t.btn_text))
                    .max_w(px(440.))
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
                    .text_size(px(11.))
                    .text_color(rgba(t.hint_text))
                    .pt(px(4.))
                    .child(crate::i18n::t!("settings.about.license").to_string()),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(rgba(t.hint_text))
                    .max_w(px(440.))
                    .text_center()
                    .line_height(relative(1.5))
                    .pt(px(6.))
                    .child(crate::i18n::t!("settings.about.credits").to_string()),
            );
        // Centered in the pane, both axes: this is an identity card, not
        // a form. Left-aligned under the pages' gutter it looked like it
        // had failed to fill the space; top-aligned it crowded the
        // header in a tall window. `justify_center` on the scroll body
        // keeps it in the optical middle and still scrolls when the
        // window is shorter than the card.
        page_body("page-about")
            .items_center()
            .justify_center()
            .child(body)
    }
}
