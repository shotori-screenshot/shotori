//! The OCR page: visual model-set cards with readiness status and in-window
//! pre-download, recognition & formatting switches, custom rapidocr TOML
//! override, and model cache management.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use anyhow::Context as _;
use gpui_kit::assets::IconName;
use gpui_kit::base::{Button, Switch};
use gpui_kit::component::{ActiveTheme, Icon};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

use crate::model::settings::OcrModel;

use super::{SettingsWindow, card_group, open_external, page_body, page_header};

/// Suggested file name for a fresh template config.
const CUSTOM_CONFIG_FILE_NAME: &str = "shotori-custom-ocr.toml";

#[derive(Default)]
pub(super) struct OcrPageState {
    pub(super) downloading: Option<(OcrModel, Arc<crate::ocr::DownloadProgress>)>,
    pub(super) download_error: Option<(OcrModel, String)>,
    pub(super) confirm_clear_cache: bool,
}

impl Drop for OcrPageState {
    fn drop(&mut self) {
        if let Some((_, progress)) = &self.downloading {
            progress.cancel.store(true, Ordering::Relaxed);
        }
    }
}

impl SettingsWindow {
    fn reset_ocr_page(&mut self, cx: &mut Context<Self>) {
        self.settings.reset_ocr_defaults();
        self.ocr_page.confirm_clear_cache = false;
        self.ocr_page.download_error = None;
        self.persist(cx);
    }

    fn start_ocr_download(&mut self, model: OcrModel, cx: &mut Context<Self>) {
        if let Some((_, prev)) = self.ocr_page.downloading.take() {
            prev.cancel.store(true, Ordering::Relaxed);
        }
        self.ocr_page.download_error = None;
        let upright = self.settings.ocr_assume_upright();
        let progress = Arc::new(crate::ocr::DownloadProgress::for_model(model, upright));
        self.ocr_page.downloading = Some((model, Arc::clone(&progress)));
        crate::ocr::spawn_download_for(model, upright, Arc::clone(&progress));
        cx.notify();

        cx.spawn(async move |this, cx| {
            let mut last = (0u64, 0u64, 0u8);
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(80))
                    .await;
                let snap = (
                    progress.done_bytes.load(Ordering::Relaxed)
                        + progress.bytes.load(Ordering::Relaxed),
                    progress.total.load(Ordering::Relaxed),
                    progress.file_idx.load(Ordering::Relaxed),
                );
                let running = progress.is_running();
                if !running {
                    let ok = progress.finished_ok();
                    let err_msg = (!ok).then(|| progress.error());
                    let _ = this.update(cx, |view, cx| {
                        let still_same = view
                            .ocr_page
                            .downloading
                            .as_ref()
                            .is_some_and(|(_, p)| Arc::ptr_eq(p, &progress));
                        if still_same {
                            view.ocr_page.downloading = None;
                            if let Some(reason) = err_msg
                                && reason != "cancelled"
                            {
                                view.ocr_page.download_error = Some((model, one_line(&reason)));
                            }
                            cx.notify();
                        }
                    });
                    break;
                }
                if snap != last {
                    last = snap;
                    if this.update(cx, |_, cx| cx.notify()).is_err() {
                        progress.cancel.store(true, Ordering::Relaxed);
                        break;
                    }
                }
            }
        })
        .detach();
    }

    fn cancel_ocr_download(&mut self, cx: &mut Context<Self>) {
        if let Some((_, progress)) = self.ocr_page.downloading.take() {
            progress.cancel.store(true, Ordering::Relaxed);
            cx.notify();
        }
    }

    pub(super) fn page_ocr(&self, window: &mut Window, cx: &mut Context<Self>) -> Stateful<Div> {
        let selected_model = self.settings.ocr_model();
        let upright = self.settings.ocr_assume_upright();
        let custom_raw = self.settings.ocr_custom_config().map(str::to_string);
        let custom = crate::ocr::custom_config_status(custom_raw.as_deref(), upright);
        let custom_active = custom.is_some();

        let upright_switch = self.upright_switch(window, cx);
        let join_lines = self.join_lines_switch(window, cx);
        let custom_detail = custom_summary(&custom);
        let custom_actions_row = custom_actions(
            &custom,
            custom_raw.as_deref(),
            selected_model,
            cx.entity(),
            cx,
        );

        let override_banner = custom_active.then(|| {
            let entity = cx.entity();
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_3()
                .px_3p5()
                .py_2p5()
                .rounded(cx.theme().radius)
                .border_1()
                .border_color(cx.theme().primary.opacity(0.5))
                .bg(cx.theme().accent)
                .text_xs()
                .text_color(cx.theme().foreground)
                .child(crate::i18n::t!("settings.ocr_page.custom.overridden").to_string())
                .child(ghost_button(
                    cx,
                    "ocr-switch-presets",
                    crate::i18n::t!("settings.ocr_page.use_presets").to_string(),
                    move |_, _, cx| {
                        entity.update(cx, |this, cx| {
                            this.settings.set_ocr_custom_config(None);
                            this.persist(cx);
                        });
                    },
                ))
        });

        let (cache_files, cache_bytes) = crate::ocr::cache_stats();
        let cache_stats_str = if cache_files == 0 {
            format!(
                "{} · {}",
                crate::i18n::t!("settings.advanced.cache_empty"),
                crate::ocr::model_dir_display()
            )
        } else {
            format!(
                "{} · {}",
                crate::i18n::t!(
                    "settings.advanced.cache_stats",
                    size = super::layout::format_bytes(cache_bytes),
                    count = cache_files
                ),
                crate::ocr::model_dir_display()
            )
        };

        let cache_actions = self.ocr_cache_actions(cache_files > 0, cx);

        let body = super::layout::content_column()
            .child(
                div()
                    .flex()
                    .items_start()
                    .justify_between()
                    .gap_4()
                    .child(page_header(
                        cx,
                        "settings.ocr_page.title",
                        Some("settings.ocr_page.blurb"),
                    ))
                    .child(super::shell::reset_button("settings.ocr_page.reset", cx, {
                        let entity = cx.entity();
                        move |_, _, cx| {
                            entity.update(cx, |this, cx| {
                                this.reset_ocr_page(cx);
                            });
                        }
                    })),
            )
            .children(override_banner)
            .child(super::layout::section_block(
                cx,
                Some(crate::i18n::t!("settings.ocr_page.group.models").as_ref()),
                None,
                super::layout::dim_when(
                    custom_active,
                    self.ocr_model_cards(selected_model, upright, custom_active, window, cx),
                ),
            ))
            .child(card_group(
                cx,
                Some(crate::i18n::t!("settings.ocr_page.group.behavior").as_ref()),
                vec![
                    super::layout::compact_row(
                        cx,
                        crate::i18n::t!("settings.ocr_page.upright.title").to_string(),
                        Some("settings.ocr_page.upright.body"),
                        None,
                        upright_switch,
                    ),
                    super::layout::compact_row(
                        cx,
                        crate::i18n::t!("settings.ocr_page.join_lines.title").to_string(),
                        Some("settings.ocr_page.join_lines.body"),
                        None,
                        join_lines,
                    ),
                ],
            ))
            .child(card_group(
                cx,
                Some(crate::i18n::t!("settings.ocr_page.group.advanced").as_ref()),
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
                        Some(cache_stats_str),
                        cache_actions,
                    ),
                ],
            ));
        page_body("page-ocr").child(body)
    }

    fn ocr_model_cards(
        &self,
        selected_model: OcrModel,
        upright: bool,
        disabled: bool,
        window: &Window,
        cx: &Context<Self>,
    ) -> Div {
        let columns = if window.viewport_size().width < window.rem_size() * 38. {
            1
        } else {
            2
        };
        let mut grid = div().grid().grid_cols(columns).gap_2p5().w_full();
        for model in OcrModel::ALL {
            let selected = !disabled && model == selected_model;
            let ready = crate::ocr::set_ready(model, upright);
            let active_dl = self
                .ocr_page
                .downloading
                .as_ref()
                .filter(|(m, _)| *m == model)
                .map(|(_, p)| p);
            let dl_err = self
                .ocr_page
                .download_error
                .as_ref()
                .filter(|(m, _)| *m == model)
                .map(|(_, e)| e.clone());

            let status_row = if let Some(progress) = active_dl {
                let done = progress.done_bytes.load(Ordering::Relaxed)
                    + progress.bytes.load(Ordering::Relaxed);
                let cur_bytes = progress.bytes.load(Ordering::Relaxed);
                let cur_total = progress.total.load(Ordering::Relaxed);
                let idx = progress.file_idx.load(Ordering::Relaxed).max(1);
                let pct = if cur_total > 0 {
                    (cur_bytes as f32 / cur_total as f32).clamp(0.05, 1.0)
                } else {
                    0.25
                };
                let entity = cx.entity();
                div()
                    .flex()
                    .flex_col()
                    .gap_1p5()
                    .w_full()
                    .pt_1()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .child(
                                div().text_xs().text_color(cx.theme().primary).child(
                                    crate::i18n::t!(
                                        "settings.ocr_page.downloading_status",
                                        idx = idx,
                                        count = progress.file_count.max(1),
                                        done = super::layout::format_bytes(done)
                                    )
                                    .to_string(),
                                ),
                            )
                            .child(
                                Button::new(SharedString::from(format!(
                                    "ocr-cancel-dl-{}",
                                    model.id()
                                )))
                                .px_2()
                                .h_6()
                                .rounded(cx.theme().radius)
                                .border_1()
                                .border_color(cx.theme().border)
                                .text_xs()
                                .text_color(cx.theme().foreground)
                                .hover(|s| s.bg(cx.theme().secondary_hover))
                                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                .on_click(move |_, _, cx| {
                                    cx.stop_propagation();
                                    entity.update(cx, |this, cx| this.cancel_ocr_download(cx));
                                })
                                .child(
                                    crate::i18n::t!("settings.ocr_page.cancel_download")
                                        .to_string(),
                                ),
                            ),
                    )
                    .child(
                        div()
                            .w_full()
                            .h_1()
                            .rounded_full()
                            .bg(cx.theme().border)
                            .overflow_hidden()
                            .child(
                                div()
                                    .h_full()
                                    .w(relative(pct))
                                    .rounded_full()
                                    .bg(cx.theme().primary),
                            ),
                    )
            } else if ready {
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .pt_0p5()
                    .text_xs()
                    .text_color(cx.theme().primary)
                    .child(Icon::new(IconName::Check).size_3())
                    .child(crate::i18n::t!("settings.ocr_page.ready").to_string())
            } else {
                let entity = cx.entity();
                let dl_label = crate::i18n::t!("settings.ocr_page.download_btn").to_string();
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .w_full()
                    .pt_0p5()
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(
                                crate::i18n::t!("settings.ocr_page.downloads_first").to_string(),
                            ),
                    )
                    .child(
                        Button::new(SharedString::from(format!("ocr-dl-{}", model.id())))
                            .accessibility_label(format!("{dl_label}: {}", model.label()))
                            .disabled(disabled)
                            .px_2p5()
                            .h_6()
                            .rounded(cx.theme().radius)
                            .border_1()
                            .border_color(cx.theme().border)
                            .bg(cx.theme().background)
                            .text_xs()
                            .text_color(cx.theme().foreground)
                            .hover(|s| s.bg(cx.theme().secondary_hover))
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(move |_, _, cx| {
                                cx.stop_propagation();
                                entity.update(cx, |this, cx| {
                                    this.select_ocr_model(model, cx);
                                    this.start_ocr_download(model, cx);
                                });
                            })
                            .child(dl_label),
                    )
            };

            let card_body = div()
                .flex()
                .flex_col()
                .items_start()
                .gap_1p5()
                .w_full()
                .min_w_0()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .w_full()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(cx.theme().foreground)
                                .min_w_0()
                                .truncate()
                                .child(model.label()),
                        )
                        .child(
                            div()
                                .size_4()
                                .flex()
                                .items_center()
                                .justify_center()
                                .flex_shrink_0()
                                .text_color(cx.theme().primary)
                                .children(selected.then(|| Icon::new(IconName::Check).size_3p5())),
                        ),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(format!("{} · {}", model.languages(), model.tier())),
                )
                .child(status_row)
                .children(dl_err.map(|reason| {
                    div().text_xs().text_color(cx.theme().danger).child(
                        crate::i18n::t!("settings.ocr_page.download_failed", reason = reason)
                            .to_string(),
                    )
                }));

            let entity = cx.entity();
            let mut card = div()
                .id(SharedString::from(format!("ocr-model-card-{}", model.id())))
                .flex()
                .flex_col()
                .p_3()
                .w_full()
                .min_w_0()
                .rounded(cx.theme().radius)
                .border_1()
                .border_color(if selected {
                    cx.theme().primary
                } else {
                    cx.theme().border
                })
                .bg(if selected {
                    cx.theme().accent
                } else {
                    cx.theme().group_box
                })
                .when(!disabled, |d| {
                    d.cursor_pointer()
                        .hover(|s| s.border_color(cx.theme().primary))
                        .on_click(move |_, _, cx| {
                            entity.update(cx, |this, cx| this.select_ocr_model(model, cx));
                        })
                })
                .child(card_body);
            if selected {
                card = card.debug_selector(|| "ocr-selected-model-card".into());
            }
            grid = grid.child(card);
        }
        grid
    }

    fn ocr_cache_actions(&self, has_files: bool, cx: &Context<Self>) -> AnyElement {
        let mut row = div().flex().items_center().gap_1p5().flex_shrink_0();
        row = row.child(open_dir_button(cx));
        if has_files && self.ocr_page.downloading.is_none() {
            if self.ocr_page.confirm_clear_cache {
                let entity_confirm = cx.entity();
                let entity_cancel = cx.entity();
                let confirm_lbl = crate::i18n::t!("settings.ocr_page.clear_confirm").to_string();
                let cancel_lbl =
                    crate::i18n::t!("settings.advanced.factory_reset.cancel").to_string();
                row = row
                    .child(
                        Button::new("ocr-cache-clear-confirm")
                            .accessibility_label(confirm_lbl.clone())
                            .h_8()
                            .px_3()
                            .rounded(cx.theme().radius)
                            .bg(cx.theme().danger)
                            .text_color(cx.theme().danger_foreground)
                            .text_sm()
                            .on_click(move |_, _, cx| {
                                entity_confirm.update(cx, |this, cx| {
                                    this.ocr_page.confirm_clear_cache = false;
                                    if let Err(e) = crate::ocr::clear_model_cache() {
                                        this.error = Some(format!("{e:#}"));
                                    }
                                    cx.notify();
                                });
                            })
                            .child(confirm_lbl),
                    )
                    .child(ghost_button(
                        cx,
                        "ocr-cache-clear-cancel",
                        cancel_lbl,
                        move |_, _, cx| {
                            entity_cancel.update(cx, |this, cx| {
                                this.ocr_page.confirm_clear_cache = false;
                                cx.notify();
                            });
                        },
                    ));
            } else {
                let entity = cx.entity();
                row = row.child(ghost_button(
                    cx,
                    "ocr-cache-clear-arm",
                    crate::i18n::t!("settings.ocr_page.clear_cache").to_string(),
                    move |_, _, cx| {
                        entity.update(cx, |this, cx| {
                            this.ocr_page.confirm_clear_cache = true;
                            cx.notify();
                        });
                    },
                ));
            }
        }
        row.into_any_element()
    }

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

    fn join_lines_switch(&self, window: &mut Window, cx: &mut Context<Self>) -> Switch {
        let entity = cx.entity();
        super::layout::settings_switch(
            "ocr-join-lines",
            self.settings.ocr_join_lines(),
            crate::i18n::t!("settings.ocr_page.join_lines.title").to_string(),
            window,
            cx,
        )
        .on_change(move |checked, _, _, cx| {
            entity.update(cx, |this, cx| {
                this.settings.set_ocr_join_lines(checked);
                this.persist(cx);
            });
        })
    }

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

fn custom_actions(
    custom: &Option<Result<std::path::PathBuf, String>>,
    raw: Option<&str>,
    model: crate::model::settings::OcrModel,
    entity: Entity<SettingsWindow>,
    cx: &App,
) -> AnyElement {
    let t = |key: &'static str| crate::i18n::t!(key).to_string();
    let mut row = div().flex().items_center().gap_1p5().flex_shrink_0();
    if let Some(path) = custom_config_path(custom, raw) {
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
                    move |_, _, cx| match crate::ocr::template_config_toml(model) {
                        Ok(text) => run_file_dialog(entity.clone(), Some(text), cx),
                        Err(e) => notify_template_failed(e),
                    }
                },
            ));
    }
    row.into_any_element()
}

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
        drop(tx.send_blocking(outcome));
    });
    cx.spawn(async move |cx| {
        if let Ok(outcome) = rx.recv().await {
            entity.update(cx, |this, cx| this.custom_config_picked(outcome, cx));
        }
    })
    .detach();
}

fn write_template(path: &Path, text: String) -> anyhow::Result<PathBuf> {
    std::fs::write(path, text).with_context(|| format!("writing {}", path.display()))?;
    Ok(path.to_path_buf())
}

fn dialog_start_dir() -> Option<PathBuf> {
    crate::model::settings::config_path()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
}

fn open_dir_button(cx: &App) -> Button {
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
}

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

fn one_line(s: &str) -> String {
    let flat = s.replace('\n', " ");
    let cut: String = flat.chars().take(160).collect();
    if cut.len() < flat.len() {
        format!("{cut}…")
    } else {
        cut
    }
}
