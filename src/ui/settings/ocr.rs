//! The OCR page: the model-set dropdown and the upright-assume
//! switch, with the model's langs · tier · readiness detail line —
//! plus the custom-config row (`[ocr] custom_config`, which overrides
//! the dropdown): pick an existing rapidocr config TOML, or write a
//! new one from the current model's parameters as a template — and
//! the cache row (open the model directory).

use anyhow::Context as _;
use gpui_kit::base::{Button, Switch};
use gpui_kit::component::ActiveTheme;
use gpui_kit::component::select::Select;
use gpui_kit::*;
use std::path::{Path, PathBuf};

use super::{SettingsWindow, card_group, open_external, page_body, page_header};

/// Suggested file name for a fresh template config.
const CUSTOM_CONFIG_FILE_NAME: &str = "shotori-custom-ocr.toml";

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
        // The custom status reads THIS window's live snapshot too (the
        // raw value is passed in, not re-read from the global): a path
        // picked in this window must light the chip up — and disable
        // the dropdown — within the same window lifetime.
        let custom_raw = self.settings.ocr_custom_config().map(str::to_string);
        let custom = crate::ocr::custom_config_status(custom_raw.as_deref(), upright);
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
        let custom_actions_row =
            custom_actions(&custom, custom_raw.as_deref(), model, cx.entity(), cx);
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
                        custom_actions_row,
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

    /// A file dialog settled: a picked or just-saved path enables the
    /// custom config (write-through, so the chip re-validates and the
    /// dropdown disables this frame); a failed template write
    /// surfaces as a notification; a cancel does nothing.
    fn custom_config_picked(
        &mut self,
        outcome: Option<anyhow::Result<PathBuf>>,
        cx: &mut Context<Self>,
    ) {
        let Some(outcome) = outcome else { return };
        match outcome {
            Ok(path) => {
                self.settings
                    .set_ocr_custom_config(Some(path.display().to_string()));
                self.persist(cx);
            }
            Err(e) => notify_template_failed(e),
        }
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

/// The custom row's actions, by state: off offers pick-an-existing
/// and new-from-template; set offers open/replace/clear. The dialogs
/// run on a plain thread (see `run_file_dialog`) — the click handler
/// only captures what render time already knows (the entity, the
/// model choice, the open target).
fn custom_actions(
    custom: &Option<Result<std::path::PathBuf, String>>,
    raw: Option<&str>,
    model: crate::model::settings::OcrModel,
    entity: Entity<SettingsWindow>,
    cx: &App,
) -> AnyElement {
    let t = |key: &'static str| crate::i18n::t!(key).to_string();
    let mut row = div().flex().items_center().gap_1().flex_shrink_0();
    if let Some(path) = custom_config_path(custom, raw) {
        // Fixing a broken config is exactly the moment the open button
        // matters, so it shows for Err paths too.
        row = row
            .child(ghost_button(
                cx,
                "ocr-custom-open",
                t("settings.ocr_page.custom.open"),
                move |_, _, _| open_external(path.display().to_string()),
            ))
            .child(ghost_button(
                cx,
                "ocr-custom-replace",
                t("settings.ocr_page.custom.replace"),
                {
                    let entity = entity.clone();
                    move |_, _, cx| run_file_dialog(entity.clone(), None, cx)
                },
            ))
            .child(ghost_button(
                cx,
                "ocr-custom-clear",
                t("settings.ocr_page.custom.clear"),
                {
                    move |_, _, cx| {
                        entity.update(cx, |this, cx| {
                            this.settings.set_ocr_custom_config(None);
                            this.persist(cx);
                        });
                    }
                },
            ));
    } else {
        row = row
            .child(ghost_button(
                cx,
                "ocr-custom-browse",
                t("settings.ocr_page.custom.browse"),
                {
                    let entity = entity.clone();
                    move |_, _, cx| run_file_dialog(entity.clone(), None, cx)
                },
            ))
            .child(ghost_button(
                cx,
                "ocr-custom-new",
                t("settings.ocr_page.custom.new"),
                {
                    move |_, _, cx| {
                        // Generated here, not in the dialog thread: the
                        // template comes from THIS window's model choice,
                        // and building it is pure string work (the
                        // registry entry plus path joins — no IO).
                        match crate::ocr::template_config_toml(model) {
                            Ok(text) => run_file_dialog(entity.clone(), Some(text), cx),
                            Err(e) => notify_template_failed(e),
                        }
                    }
                },
            ));
    }
    row.into_any_element()
}

/// Template generation/IO failure surface — the click has no other
/// natural error channel, and a silent no-op reads as broken.
fn notify_template_failed(e: anyhow::Error) {
    crate::notify::send(
        crate::i18n::t!("settings.ocr_page.custom.template_failed_title").as_ref(),
        crate::i18n::t!(
            "settings.ocr_page.custom.template_failed_body",
            error = format!("{e:#}")
        )
        .as_ref(),
    );
}

/// Run a native file dialog on a dedicated thread: the xdg-portal
/// round trip blocks, and gpui's executors are async contexts that
/// must not block — the same rule that gives model downloads their
/// own thread. `template` picks the flavor: None opens an existing
/// config; Some(text) saves that text as a new one. The outcome lands
/// in `SettingsWindow::custom_config_picked` on the window's entity;
/// a window closed meanwhile simply drops it.
fn run_file_dialog(entity: Entity<SettingsWindow>, template: Option<String>, cx: &mut App) {
    let title_key = if template.is_some() {
        "settings.ocr_page.custom.new_title"
    } else {
        "settings.ocr_page.custom.pick_title"
    };
    let mut dialog = rfd::FileDialog::new()
        .set_title(crate::i18n::t!(title_key).as_ref())
        .add_filter(
            crate::i18n::t!("settings.ocr_page.custom.toml_filter").as_ref(),
            &["toml"],
        );
    if let Some(dir) = dialog_start_dir() {
        dialog = dialog.set_directory(dir);
    }
    let (tx, rx) = async_channel::bounded::<Option<anyhow::Result<PathBuf>>>(1);
    std::thread::spawn(move || {
        let outcome = match template {
            Some(text) => dialog
                .set_file_name(CUSTOM_CONFIG_FILE_NAME)
                .save_file()
                .map(|path| write_template(&path, text)),
            None => dialog.pick_file().map(Ok),
        };
        // A send error only means the window closed while the dialog
        // was up — the click's effect is gone with it, nothing to report.
        drop(tx.send_blocking(outcome));
    });
    cx.spawn(async move |cx| {
        if let Ok(outcome) = rx.recv().await {
            entity.update(cx, |this, cx| this.custom_config_picked(outcome, cx));
        }
    })
    .detach();
}

/// Write the template text to the chosen path. The content is already
/// validated (`to_toml_string`); this is plain IO, so failures carry
/// the OS error verbatim.
fn write_template(path: &Path, text: String) -> anyhow::Result<PathBuf> {
    std::fs::write(path, text).with_context(|| format!("writing {}", path.display()))?;
    Ok(path.to_path_buf())
}

/// Where dialogs open: next to settings.toml (a hand-made config
/// naturally lives beside it, and that directory exists whenever this
/// window runs) — falling back to `$HOME`, then to rfd's default.
fn dialog_start_dir() -> Option<PathBuf> {
    crate::model::settings::config_path()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
}

/// The cache row's action: open (and if needed create) the model
/// directory in the file manager.
fn open_dir_button(cx: &App) -> AnyElement {
    ghost_button(
        cx,
        "ocr-cache-open",
        crate::i18n::t!("settings.general.file.open").to_string(),
        |_, _, _| match crate::ocr::open_model_dir() {
            Ok(dir) => open_external(dir.display().to_string()),
            Err(e) => crate::notify::send(
                crate::i18n::t!("settings.general.open_failed.title").as_ref(),
                crate::i18n::t!(
                    "settings.general.open_failed.body",
                    error = format!("{e:#}")
                )
                .as_ref(),
            ),
        },
    )
    .into_any_element()
}

/// The shared ghost-button shape (the custom row's actions, the cache
/// row's open). Click behavior is call-site owned: everything here is
/// either instant or hands off to a plain thread.
fn ghost_button(
    cx: &App,
    id: &'static str,
    label: String,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> Button {
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
        .on_click(on_click)
}

/// The configured path from a live status plus its raw setting (Ok
/// gives it directly; Err keeps the normalized path so "open the
/// config to fix it" works even when a model file is missing — the
/// raw must come from the same live snapshot that produced
/// `custom`, or the fallback resurrects a cleared path).
fn custom_config_path(
    custom: &Option<Result<std::path::PathBuf, String>>,
    raw: Option<&str>,
) -> Option<PathBuf> {
    match custom {
        Some(Ok(path)) => Some(path.clone()),
        Some(Err(_)) => crate::ocr::custom_config_from(raw),
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
