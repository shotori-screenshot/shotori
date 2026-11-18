//! Advanced preferences: configuration file access & live reload, local
//! cache/state inspection & cleanup, Wayland session diagnostics, and safe
//! two-step factory reset.

use gpui_kit::base::Button;
use gpui_kit::component::ActiveTheme;
use gpui_kit::*;

use crate::ui::theme::load::{self, ThemeFile};

use super::{SettingsWindow, card_group, open_external, page_body, page_header};

/// Which config file a row opens.
#[derive(Clone, Copy)]
enum ConfigFile {
    Settings,
    Theme,
}

#[derive(Default)]
pub(super) struct AdvancedPageState {
    pub(super) confirm_factory_reset: bool,
    pub(super) reloaded_flash: bool,
    pub(super) copied_diag_flash: bool,
}

pub(super) fn session_diagnostics_summary() -> (String, String, Option<&'static str>) {
    let wayland = std::env::var("WAYLAND_DISPLAY")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_default();
    let desktop = std::env::var("XDG_CURRENT_DESKTOP")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            std::env::var("XDG_SESSION_DESKTOP")
                .ok()
                .filter(|s| !s.trim().is_empty())
        })
        .unwrap_or_default();
    let snap_backend = if std::env::var_os("NIRI_SOCKET").is_some() {
        Some("niri (NIRI_SOCKET)")
    } else if std::env::var_os("SWAYSOCK").is_some() {
        Some("sway / i3 IPC (SWAYSOCK)")
    } else if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some() {
        Some("Hyprland (HYPRLAND_INSTANCE_SIGNATURE)")
    } else {
        None
    };
    (wayland, desktop, snap_backend)
}

impl SettingsWindow {
    /// Build and copy a concise Markdown diagnostic report for issue filing.
    pub(super) fn copy_diagnostic_report(&mut self, from_about: bool, cx: &mut Context<Self>) {
        let (wayland, desktop, snap) = session_diagnostics_summary();
        let report = format!(
            "Shotori v{} ({}/{})\nWayland: {}\nDesktop: {}\nWindow-snap IPC: {}\nConfig: {}\nOCR model: {}",
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS,
            std::env::consts::ARCH,
            if wayland.is_empty() {
                "unset"
            } else {
                wayland.as_str()
            },
            if desktop.is_empty() {
                "unset"
            } else {
                desktop.as_str()
            },
            snap.unwrap_or("none"),
            self.path
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "none".into()),
            self.settings.ocr_model().id(),
        );
        cx.write_to_clipboard(ClipboardItem::new_string(report));
        if from_about {
            self.about_copied_flash = true;
        } else {
            self.advanced_page.copied_diag_flash = true;
        }
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(1800))
                .await;
            let _ = this.update(cx, |view, cx| {
                if from_about {
                    view.about_copied_flash = false;
                } else {
                    view.advanced_page.copied_diag_flash = false;
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Opens the config file in the system editor, creating it if absent.
    fn open_config(&mut self, file: ConfigFile, cx: &mut Context<Self>) {
        let Some(path) = (match file {
            ConfigFile::Settings => self.path.clone(),
            ConfigFile::Theme => self.appearance.path.clone(),
        }) else {
            crate::notify::send(
                crate::i18n::t!("settings.general.open_failed.title").as_ref(),
                crate::i18n::t!("settings.no_config_dir").as_ref(),
            );
            return;
        };
        if !path.exists() {
            let materialized = match file {
                ConfigFile::Settings => self.settings.save(&path),
                ConfigFile::Theme => load::save(
                    &path,
                    &ThemeFile {
                        base: Some("shotori-dark".into()),
                        ..Default::default()
                    },
                ),
            };
            if let Err(error) = materialized {
                self.error = Some(error);
                cx.notify();
                return;
            }
        }
        open_external(path.display().to_string());
    }

    pub(super) fn page_advanced(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let entity = cx.entity();

        let reload_label = if self.advanced_page.reloaded_flash {
            crate::i18n::t!("settings.advanced.reloaded").to_string()
        } else {
            crate::i18n::t!("settings.advanced.reload").to_string()
        };
        let reload_btn = {
            let entity = entity.clone();
            super::layout::action_button(
                cx,
                "advanced-reload-disk",
                reload_label,
                move |_, window, cx| {
                    entity.update(cx, |this, cx| this.reload_from_disk(window, cx));
                },
            )
            .into_any_element()
        };

        // Storage & caches
        let (notify_files, notify_bytes) = crate::notify::cache_stats();
        let notify_cache_dir = crate::notify::cache_dir_display()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "?".into());
        let notify_cache_desc = if notify_files == 0 {
            format!(
                "{} · {notify_cache_dir}",
                crate::i18n::t!("settings.advanced.cache_empty")
            )
        } else {
            format!(
                "{} · {notify_cache_dir}",
                crate::i18n::t!(
                    "settings.advanced.cache_stats",
                    size = super::layout::format_bytes(notify_bytes),
                    count = notify_files
                )
            )
        };
        let notify_cache_actions = {
            let entity = entity.clone();
            div()
                .flex()
                .items_center()
                .gap_1p5()
                .child(super::layout::action_button(
                    cx,
                    "advanced-notify-cache-open",
                    crate::i18n::t!("settings.general.file.open").to_string(),
                    |_, _, _| match crate::notify::open_cache_dir() {
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
                ))
                .child(
                    super::layout::action_button(
                        cx,
                        "advanced-notify-cache-clear",
                        crate::i18n::t!("settings.advanced.clear_cache").to_string(),
                        move |_, _, cx| {
                            entity.update(cx, |_, cx| {
                                crate::notify::clear_cache();
                                cx.notify();
                            });
                        },
                    )
                    .disabled(notify_files == 0),
                )
        };

        let last_dir = crate::model::settings::read_last_save_dir();
        let last_dir_desc = last_dir
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| crate::i18n::t!("settings.advanced.last_dir.none").to_string());
        let last_dir_action = {
            let entity = entity.clone();
            let has_last = last_dir.is_some();
            super::layout::action_button(
                cx,
                "advanced-last-dir-clear",
                crate::i18n::t!("settings.advanced.last_dir.clear").to_string(),
                move |_, _, cx| {
                    entity.update(cx, |_, cx| {
                        crate::model::settings::clear_last_save_dir();
                        cx.notify();
                    });
                },
            )
            .disabled(!has_last)
        };

        let (ocr_files, ocr_bytes) = crate::ocr::cache_stats();
        let ocr_cache_desc = if ocr_files == 0 {
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
                    size = super::layout::format_bytes(ocr_bytes),
                    count = ocr_files
                ),
                crate::ocr::model_dir_display()
            )
        };
        let ocr_cache_open = super::layout::action_button(
            cx,
            "advanced-ocr-cache-open",
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
        );

        // Session diagnostics
        let (wayland, desktop, snap_backend) = session_diagnostics_summary();
        let unset_label = crate::i18n::t!("settings.advanced.diag.unset").to_string();
        let diag_badge = |text: String, active: bool| {
            div()
                .px_2p5()
                .py_1()
                .rounded(cx.theme().radius)
                .border_1()
                .border_color(cx.theme().border)
                .bg(cx.theme().background)
                .text_xs()
                .text_color(if active {
                    cx.theme().foreground
                } else {
                    cx.theme().muted_foreground
                })
                .child(text)
        };
        let copy_diag_label = if self.advanced_page.copied_diag_flash {
            crate::i18n::t!("settings.advanced.diag.copied").to_string()
        } else {
            crate::i18n::t!("settings.advanced.diag.copy").to_string()
        };
        let copy_diag_btn = {
            let entity = entity.clone();
            super::layout::action_button(
                cx,
                "advanced-copy-diag",
                copy_diag_label,
                move |_, _, cx| {
                    entity.update(cx, |this, cx| this.copy_diagnostic_report(false, cx));
                },
            )
            .into_any_element()
        };

        // Two-step factory reset
        let factory_reset_control = if self.advanced_page.confirm_factory_reset {
            let entity_confirm = entity.clone();
            let entity_cancel = entity.clone();
            let confirm_lbl =
                crate::i18n::t!("settings.advanced.factory_reset.confirm").to_string();
            let cancel_lbl = crate::i18n::t!("settings.advanced.factory_reset.cancel").to_string();
            div()
                .flex()
                .items_center()
                .gap_1p5()
                .child(
                    Button::new("advanced-factory-reset-confirm")
                        .accessibility_label(confirm_lbl.clone())
                        .h_8()
                        .px_3()
                        .rounded(cx.theme().radius)
                        .bg(cx.theme().danger)
                        .text_color(cx.theme().danger_foreground)
                        .text_sm()
                        .on_click(move |_, window, cx| {
                            entity_confirm
                                .update(cx, |this, cx| this.factory_reset_all(window, cx));
                        })
                        .child(confirm_lbl),
                )
                .child(super::layout::action_button(
                    cx,
                    "advanced-factory-reset-cancel",
                    cancel_lbl,
                    move |_, _, cx| {
                        entity_cancel.update(cx, |this, cx| {
                            this.advanced_page.confirm_factory_reset = false;
                            cx.notify();
                        });
                    },
                ))
        } else {
            let entity = entity.clone();
            div().child(super::layout::action_button(
                cx,
                "advanced-factory-reset-arm",
                crate::i18n::t!("settings.advanced.factory_reset.arm").to_string(),
                move |_, _, cx| {
                    entity.update(cx, |this, cx| {
                        this.advanced_page.confirm_factory_reset = true;
                        cx.notify();
                    });
                },
            ))
        };

        let body = super::layout::content_column()
            .child(page_header(
                cx,
                "settings.preferences.advanced",
                Some("settings.advanced.blurb"),
            ))
            .child(super::layout::card_group_with_action(
                cx,
                Some(crate::i18n::t!("settings.general.group.files").as_ref()),
                Some(reload_btn),
                vec![
                    self.file_row("settings.toml", ConfigFile::Settings, true, cx),
                    self.file_row("theme.toml", ConfigFile::Theme, false, cx),
                ],
            ))
            .child(card_group(
                cx,
                Some(crate::i18n::t!("settings.advanced.group.storage").as_ref()),
                vec![
                    super::layout::compact_row(
                        cx,
                        crate::i18n::t!("settings.advanced.notify_cache.title").to_string(),
                        Some("settings.advanced.notify_cache.hint"),
                        Some(notify_cache_desc),
                        notify_cache_actions,
                    ),
                    super::layout::compact_row(
                        cx,
                        crate::i18n::t!("settings.advanced.last_dir.title").to_string(),
                        None,
                        Some(last_dir_desc),
                        last_dir_action,
                    ),
                    super::layout::compact_row(
                        cx,
                        crate::i18n::t!("settings.advanced.ocr_cache.title").to_string(),
                        None,
                        Some(ocr_cache_desc),
                        ocr_cache_open,
                    ),
                ],
            ))
            .child(super::layout::card_group_with_action(
                cx,
                Some(crate::i18n::t!("settings.advanced.group.diagnostics").as_ref()),
                Some(copy_diag_btn),
                vec![
                    super::layout::compact_row(
                        cx,
                        crate::i18n::t!("settings.advanced.diag.wayland").to_string(),
                        None,
                        None,
                        diag_badge(
                            if wayland.is_empty() {
                                unset_label.clone()
                            } else {
                                wayland
                            },
                            true,
                        ),
                    ),
                    super::layout::compact_row(
                        cx,
                        crate::i18n::t!("settings.advanced.diag.desktop").to_string(),
                        None,
                        None,
                        diag_badge(
                            if desktop.is_empty() {
                                unset_label
                            } else {
                                desktop
                            },
                            true,
                        ),
                    ),
                    super::layout::compact_row(
                        cx,
                        crate::i18n::t!("settings.advanced.diag.snap_backend").to_string(),
                        None,
                        None,
                        diag_badge(
                            snap_backend.map(str::to_owned).unwrap_or_else(|| {
                                crate::i18n::t!("settings.advanced.diag.snap_unavailable")
                                    .to_string()
                            }),
                            snap_backend.is_some(),
                        ),
                    ),
                ],
            ))
            .child(card_group(
                cx,
                Some(crate::i18n::t!("settings.advanced.group.danger").as_ref()),
                vec![super::layout::compact_row(
                    cx,
                    crate::i18n::t!("settings.advanced.factory_reset.title").to_string(),
                    None,
                    Some(crate::i18n::t!("settings.advanced.factory_reset.desc").to_string()),
                    factory_reset_control,
                )],
            ));
        page_body("page-general").child(body)
    }

    /// Pairs the file name, disk presence status, and resolved path with its editor actions.
    fn file_row(
        &self,
        name: &'static str,
        file: ConfigFile,
        include_open_dir: bool,
        cx: &Context<Self>,
    ) -> AnyElement {
        let entity = cx.entity();
        let path_ref = match file {
            ConfigFile::Settings => self.path.as_ref(),
            ConfigFile::Theme => self.appearance.path.as_ref(),
        };
        let exists = path_ref.is_some_and(|p| p.exists());
        let status = if exists {
            crate::i18n::t!("settings.advanced.file_present")
        } else {
            crate::i18n::t!("settings.advanced.file_default")
        };
        let desc = path_ref
            .map(|path| format!("{status} · {}", path.display()))
            .unwrap_or_default();

        let mut actions = div().flex().items_center().gap_1p5().flex_shrink_0();
        if include_open_dir
            && let Some(dir) = path_ref.and_then(|p| p.parent().map(|d| d.to_path_buf()))
        {
            actions = actions.child(super::layout::action_button(
                cx,
                "advanced-open-config-dir",
                crate::i18n::t!("settings.advanced.open_dir").to_string(),
                move |_, _, _| {
                    if let Err(err) = std::fs::create_dir_all(&dir) {
                        crate::notify::send(
                            crate::i18n::t!("settings.general.open_failed.title").as_ref(),
                            crate::i18n::t!(
                                "settings.general.open_failed.body",
                                error = err.to_string()
                            )
                            .as_ref(),
                        );
                    } else {
                        open_external(dir.display().to_string());
                    }
                },
            ));
        }
        actions = actions.child(
            Button::new(SharedString::from(name))
                .accessibility_label(format!(
                    "{}: {name}",
                    crate::i18n::t!("settings.general.file.open")
                ))
                .flex()
                .items_center()
                .h_8()
                .px_3()
                .flex_shrink_0()
                .rounded(cx.theme().radius)
                .text_sm()
                .text_color(cx.theme().foreground)
                .border_1()
                .border_color(cx.theme().border)
                .hover(|s| s.bg(cx.theme().secondary_hover))
                .focus_visible(|s| s.border_color(cx.theme().ring))
                .child(crate::i18n::t!("settings.general.file.open").to_string())
                .on_click(move |_, _, cx| {
                    entity.update(cx, |this, cx| this.open_config(file, cx));
                }),
        );

        super::layout::compact_row(cx, name, None, Some(desc), actions).into_any_element()
    }
}
