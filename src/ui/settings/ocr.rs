//! The OCR page: the model-set dropdown and the upright-assume
//! switch, with the model's langs · tier · readiness detail line.

use gpui_kit::component::select::Select;
use gpui_kit::component::switch::Switch;
use gpui_kit::*;

use crate::ui::theme;

use super::{CONTENT_MAX, SettingsWindow, card_group, card_row, page_body, page_header};

impl SettingsWindow {
    /// OCR: the model-set dropdown + the upright-assume switch. The
    /// dropdown rows stay single-line (the model's localized label);
    /// the current choice's langs · tier · readiness render as one
    /// detail line under it, from `settings` — the file stays the
    /// single truth, the Select is just the picker.
    pub(super) fn page_ocr(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let t = theme::c();
        let model = self.settings.ocr_model();
        let ready = crate::ocr::set_ready(model, self.settings.ocr_assume_upright());
        let entity = cx.entity();
        // The model's own facts and its download state share one line
        // under the label: as a right-aligned badge 360px away from the
        // model name, "ready" answered a question nobody had asked yet.
        let status = if ready {
            crate::i18n::t!("settings.ocr_page.ready").to_string()
        } else {
            crate::i18n::t!("settings.ocr_page.downloads_first").to_string()
        };
        let summary = format!("{} · {} · {}", model.languages(), model.tier(), status);
        let body = div()
            .flex()
            .flex_col()
            .gap(px(18.))
            .max_w(px(CONTENT_MAX))
            .child(page_header(
                crate::i18n::t!("settings.ocr_page.title").as_ref(),
                crate::i18n::t!("settings.ocr_page.blurb").as_ref(),
            ))
            .child(card_group(
                Some(crate::i18n::t!("settings.ocr_page.group.model").as_ref()),
                vec![card_row(
                    crate::i18n::t!("settings.ocr_page.model").to_string(),
                    Some(summary),
                    div().w_full().child(
                        Select::new(&self.ocr_select)
                            .accessibility_label(
                                crate::i18n::t!("settings.ocr_page.model").to_string(),
                            )
                            .menu_width(px(420.)),
                    ),
                )],
            ))
            .child(card_group(
                Some(crate::i18n::t!("settings.ocr_page.group.direction").as_ref()),
                vec![card_row(
                    crate::i18n::t!("settings.ocr_page.upright.title").to_string(),
                    Some(crate::i18n::t!("settings.ocr_page.upright.body").to_string()),
                    self.upright_switch(cx),
                )],
            ))
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(rgba(t.hint_text))
                    .child(
                        crate::i18n::t!(
                            "settings.ocr_page.cache_hint",
                            dir = crate::ocr::model_dir_display()
                        )
                        .to_string(),
                    ),
            );
        let _ = entity;
        page_body("page-ocr").child(body)
    }

    /// The orientation switch: a controlled `Switch` whose owner writes
    /// the requested value back (write-through, like every control here).
    fn upright_switch(&self, cx: &Context<Self>) -> Switch {
        let t = theme::c();
        let entity = cx.entity();
        Switch::new("ocr-upright")
            .checked(self.settings.ocr_assume_upright())
            .on_click(move |checked, _, cx| {
                entity.update(cx, |this, cx| {
                    this.settings.set_ocr_assume_upright(*checked);
                    this.persist(cx);
                });
            })
            .color(rgba(t.accent))
    }
}
