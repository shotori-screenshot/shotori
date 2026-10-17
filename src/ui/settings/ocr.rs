//! The OCR page: the model-set dropdown and the upright-assume
//! switch, with the model's langs · tier · readiness detail line.

use gpui_kit::component::select::Select;
use gpui_kit::component::switch::Switch;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::ui::theme;

use super::{PAGE_PAD, SettingsWindow, page_header};

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
        div()
            .id("page-ocr")
            .flex()
            .flex_col()
            .gap(px(10.))
            .p(px(PAGE_PAD))
            .size_full()
            .overflow_y_scroll()
            .child(page_header(
                crate::i18n::t!("settings.ocr_page.title").as_ref(),
                crate::i18n::t!("settings.ocr_page.blurb").as_ref(),
            ))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgba(t.hint_text))
                            .child(crate::i18n::t!("settings.ocr_page.model").to_string()),
                    )
                    .child(
                        div().w(px(360.)).child(
                            Select::new(&self.ocr_select)
                                .accessibility_label(
                                    crate::i18n::t!("settings.ocr_page.model").to_string(),
                                )
                                .menu_width(px(420.)),
                        ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .text_size(px(11.))
                                    .text_color(rgba(t.hint_text))
                                    .child(format!("{} · {}", model.languages(), model.tier())),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_size(px(11.))
                                    .text_color(rgba(if ready { t.hint_text } else { t.accent }))
                                    .child(if ready {
                                        crate::i18n::t!("settings.ocr_page.ready").to_string()
                                    } else {
                                        crate::i18n::t!("settings.ocr_page.downloads_first")
                                            .to_string()
                                    }),
                            ),
                    ),
            )
            // upright toggle: a real Switch (controlled — the owner
            // writes the requested value back and notifies)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .p(px(10.))
                    .rounded(px(6.))
                    .border_1()
                    .border_color(rgba(t.toolbar_border))
                    .child(
                        Switch::new("ocr-upright")
                            .checked(self.settings.ocr_assume_upright())
                            .on_click(move |checked, _, cx| {
                                entity.update(cx, |this, cx| {
                                    this.settings.set_ocr_assume_upright(*checked);
                                    this.persist(cx);
                                });
                            })
                            .color(rgba(t.accent)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(1.))
                            .flex_1()
                            .min_w(px(0.))
                            .child(div().text_size(px(13.)).text_color(rgba(t.btn_text)).child(
                                crate::i18n::t!("settings.ocr_page.upright.title").to_string(),
                            ))
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(rgba(t.hint_text))
                                    .child(
                                        crate::i18n::t!("settings.ocr_page.upright.body")
                                            .to_string(),
                                    ),
                            ),
                    ),
            )
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
            )
            .when_some(self.error.clone(), |el, error| {
                // errors only — the file's location is a button away
                // on the General page, an always-on path line was
                // clutter
                el.child(
                    div()
                        .text_size(px(11.))
                        .text_right()
                        .text_color(rgba(t.accent))
                        .child(error),
                )
            })
    }
}
