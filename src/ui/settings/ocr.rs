//! The OCR page: the model-set dropdown and the upright-assume
//! switch, with the model's langs · tier · readiness detail line.

use gpui_kit::base::Switch;
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::select::Select;
use gpui_kit::*;

use super::{SettingsWindow, card_group, page_body, page_header};

impl SettingsWindow {
    /// OCR: the model-set dropdown + the upright-assume switch. The
    /// dropdown rows stay single-line (the model's localized label);
    /// the current choice's langs · tier · readiness render as one
    /// detail line under it, from `settings` — the file stays the
    /// single truth, the Select is just the picker.
    pub(super) fn page_ocr(&self, window: &mut Window, cx: &mut Context<Self>) -> Stateful<Div> {
        let model = self.settings.ocr_model();
        let ready = crate::ocr::set_ready(model, self.settings.ocr_assume_upright());
        // The model's own facts and its download state share one line
        // under the label: as a right-aligned badge 360px away from the
        // model name, "ready" answered a question nobody had asked yet.
        let status = if ready {
            crate::i18n::t!("settings.ocr_page.ready").to_string()
        } else {
            crate::i18n::t!("settings.ocr_page.downloads_first").to_string()
        };
        let summary = format!("{} · {} · {}", model.languages(), model.tier(), status);
        let upright = self.upright_switch(window, cx);
        let body = super::layout::content_column()
            .child(page_header(
                cx,
                crate::i18n::t!("settings.ocr_page.title").as_ref(),
                crate::i18n::t!("settings.ocr_page.blurb").as_ref(),
            ))
            .child(card_group(
                cx,
                None,
                vec![
                    super::layout::compact_row(
                        cx,
                        crate::i18n::t!("settings.ocr_page.model").to_string(),
                        Some(summary),
                        div().w(rems(20.)).max_w_full().child(
                            Select::new(&self.ocr_select)
                                .w_full()
                                .accessibility_label(
                                    crate::i18n::t!("settings.ocr_page.model").to_string(),
                                )
                                .menu_width(rems(20.)),
                        ),
                    ),
                    super::layout::compact_row(
                        cx,
                        crate::i18n::t!("settings.ocr_page.upright.title").to_string(),
                        Some(crate::i18n::t!("settings.ocr_page.upright.body").to_string()),
                        upright,
                    ),
                ],
            ))
            .child(
                div()
                    .text_sm()
                    .line_height(relative(1.5))
                    .text_color(cx.theme().muted_foreground)
                    .child(
                        crate::i18n::t!(
                            "settings.ocr_page.cache_hint",
                            dir = crate::ocr::model_dir_display()
                        )
                        .to_string(),
                    ),
            );
        page_body("page-ocr").child(body)
    }

    /// The orientation switch: a controlled `Switch` whose owner writes
    /// the requested value back (write-through, like every control here).
    fn upright_switch(&self, window: &mut Window, cx: &mut Context<Self>) -> Switch {
        let entity = cx.entity();
        super::layout::settings_switch(
            "ocr-upright",
            self.settings.ocr_assume_upright(),
            crate::i18n::t!("settings.ocr_page.upright.title").to_string(),
            window,
            cx,
        )
        .on_change(move |checked, _, _, cx| {
            entity.update(cx, |this, cx| {
                this.settings.set_ocr_assume_upright(checked);
                this.persist(cx);
            });
        })
    }
}
