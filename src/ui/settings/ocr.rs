//! The OCR page: the model-set dropdown and the upright-assume
//! switch, with the model's langs · tier · readiness detail line —
//! plus the custom-config row (`[ocr] custom_config`, which overrides
//! the dropdown) and the cache row (open the model directory).

use gpui_kit::base::{Button, Switch};
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::select::Select;
use gpui_kit::*;
use std::path::PathBuf;

use super::{SettingsWindow, card_group, open_external, page_body, page_header};

impl SettingsWindow {
    /// OCR: the model-set dropdown + the upright-assume switch, the
    /// custom-config row, and the cache row. The dropdown rows stay
    /// single-line (the model's localized label); the current choice's
    /// langs · tier · readiness render as one detail line under it,
    /// from `settings` — the file stays the single truth, the Select
    /// is just the picker.
    pub(super) fn page_ocr(&self, window: &mut Window, cx: &mut Context<Self>) -> Stateful<Div> {
        let model = self.settings.ocr_model();
        // The live toggle state, not the startup global — persist()
        // never reinstalls the global, so the chip must follow what a
        // restart would actually read.
        let upright = self.settings.ocr_assume_upright();
        let custom = crate::ocr::custom_config_status(upright);
        // A custom config outranks the dropdown entirely: its status
        // takes over the dropdown's detail line (a "Ready" about a
        // model that will never run is a lie), and the picker disables
        // — picking there would write a setting that has no effect.
        let status = match &custom {
            Some(_) => crate::i18n::t!("settings.ocr_page.custom.overridden").to_string(),
            None => {
                let ready = crate::ocr::set_ready(model, self.settings.ocr_assume_upright());
                if ready {
                    crate::i18n::t!("settings.ocr_page.ready").to_string()
                } else {
                    crate::i18n::t!("settings.ocr_page.downloads_first").to_string()
                }
            }
        };
        let summary = format!("{} · {} · {}", model.languages(), model.tier(), status);
        let upright = self.upright_switch(window, cx);
        // The custom row's live detail: path · ready, or the first
        // problem; None while off — the off how-to lives in the row's
        // help mark instead.
        let custom_detail = custom_summary(&custom);
        let body = super::layout::content_column()
            .child(page_header(cx, "settings.ocr_page.title", None))
            .child(card_group(
                cx,
                None,
                vec![
                    super::layout::compact_row(
                        cx,
                        crate::i18n::t!("settings.ocr_page.model").to_string(),
                        None,
                        Some(summary),
                        div().w(rems(20.)).max_w_full().child(
                            Select::new(&self.ocr_select)
                                .w_full()
                                .disabled(custom.is_some())
                                .accessibility_label(
                                    crate::i18n::t!("settings.ocr_page.model").to_string(),
                                )
                                .menu_width(rems(20.)),
                        ),
                    ),
                    super::layout::compact_row(
                        cx,
                        crate::i18n::t!("settings.ocr_page.upright.title").to_string(),
                        Some("settings.ocr_page.upright.body"),
                        None,
                        upright,
                    ),
                ],
            ))
            // The custom config and the model directory share one card:
            // a "Cache" caption over a single "Model cache" row only
            // repeated the row's own label.
            .child(card_group(
                cx,
                None,
                vec![
                    super::layout::compact_row(
                        cx,
                        crate::i18n::t!("settings.ocr_page.custom.title").to_string(),
                        Some("settings.ocr_page.custom.off"),
                        custom_detail,
                        custom_open_button(&custom, cx),
                    ),
                    super::layout::compact_row(
                        cx,
                        crate::i18n::t!("settings.ocr_page.cache_row").to_string(),
                        Some("settings.ocr_page.cache_hint"),
                        Some(crate::ocr::model_dir_display()),
                        open_dir_button(cx),
                    ),
                ],
            ));
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

/// The custom row's live detail line, when a config is set: path ·
/// ready, or the first problem (parse errors and missing files, one
/// line — the raw anyhow chain is multi-line and would wreck the
/// row). Off renders no detail: the off how-to is the row's help mark.
fn custom_summary(custom: &Option<Result<std::path::PathBuf, String>>) -> Option<String> {
    match custom {
        None => None,
        Some(Ok(path)) => Some(format!(
            "{} · {}",
            path.display(),
            crate::i18n::t!("settings.ocr_page.ready")
        )),
        Some(Err(reason)) => Some(
            crate::i18n::t!("settings.ocr_page.custom.broken", reason = one_line(reason))
                .to_string(),
        ),
    }
}

/// The custom row's action: open the config file in the system editor
/// whenever a path is configured — fixing a broken config is exactly
/// the moment the button matters. Off renders no control (the row is
/// informational until `[ocr] custom_config` is set by hand).
fn custom_open_button(custom: &Option<Result<std::path::PathBuf, String>>, cx: &App) -> AnyElement {
    let Some(path) = custom_config_path(custom) else {
        return div().into_any_element();
    };
    ghost_button(
        cx,
        "ocr-custom-open",
        crate::i18n::t!("settings.ocr_page.custom.open").to_string(),
        path.display().to_string(),
    )
    .into_any_element()
}

/// The cache row's action: open (and if needed create) the model
/// directory in the file manager.
fn open_dir_button(cx: &App) -> AnyElement {
    ghost_button(
        cx,
        "ocr-cache-open",
        crate::i18n::t!("settings.general.file.open").to_string(),
        crate::ocr::model_dir_display(),
    )
    .into_any_element()
}

/// The shared open-button shape (the Advanced page's file rows).
/// Opening happens on a plain thread via `open_external`; directory
/// creation failure surfaces as a notification, matching every other
/// fire-and-forget opener here.
fn ghost_button(cx: &App, id: &'static str, label: String, target: String) -> Button {
    let cache_dir = id == "ocr-cache-open";
    let theme = cx.theme();
    Button::new(SharedString::from(id))
        .accessibility_label(label.clone())
        .flex()
        .items_center()
        .h_8()
        .px_3()
        .flex_shrink_0()
        .rounded(theme.radius)
        .text_sm()
        .text_color(theme.foreground)
        .border_1()
        .border_color(theme.border)
        .hover(|s| s.bg(theme.secondary_hover))
        .focus_visible(|s| s.border_color(theme.ring))
        .child(label)
        .on_click(move |_, _, _| {
            if cache_dir {
                match crate::ocr::open_model_dir() {
                    Ok(dir) => open_external(dir.display().to_string()),
                    Err(e) => crate::notify::send(
                        crate::i18n::t!("settings.general.open_failed.title").as_ref(),
                        crate::i18n::t!(
                            "settings.general.open_failed.body",
                            error = format!("{e:#}")
                        )
                        .as_ref(),
                    ),
                }
            } else {
                open_external(target.clone());
            }
        })
}

/// The configured path from a `custom_config_status` value (Ok gives
/// it directly; Err keeps the expanded path so "open the config to
/// fix it" works even when a model file is missing).
fn custom_config_path(custom: &Option<Result<std::path::PathBuf, String>>) -> Option<PathBuf> {
    match custom {
        Some(Ok(path)) => Some(path.clone()),
        Some(Err(_)) => crate::ocr::custom_config(),
        None => None,
    }
}

/// Collapse an error chain into one row-safe line.
fn one_line(s: &str) -> String {
    let flat = s.replace('\n', " ");
    let cut: String = flat.chars().take(160).collect();
    if cut.len() < flat.len() {
        format!("{cut}…")
    } else {
        cut
    }
}
