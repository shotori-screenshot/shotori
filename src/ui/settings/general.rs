//! The General page: the config-file rows, the UI language picker and
//! the smart-region switch — installation-level facts and the two
//! controls that change how the app behaves everywhere.
//!
//! Layout is the shared group-card system (see `super::card_group`):
//! one card per concern, one row per setting, the control in a fixed
//! right-hand column. The old page mixed bare labels, one-control cards
//! and a trailing hint paragraph, which is what made it read as
//! arbitrarily placed.

use gpui_kit::base::Button;
use gpui_kit::component::select::Select;
use gpui_kit::component::switch::Switch;
use gpui_kit::*;

use crate::ui::theme;
use crate::ui::theme::load::{self, ThemeFile};

use super::{
    CONTENT_MAX, SettingsWindow, card_group, card_row, open_external, page_body, page_header,
};

/// Which config file a row opens.
#[derive(Clone, Copy)]
enum ConfigFile {
    Settings,
    Theme,
}

impl SettingsWindow {
    /// Open a config file in the system editor. A MISSING file is
    /// materialized first — settings gets the current state written
    /// out, theme a base-only shotori file — so "edit my config" never
    /// hands the editor a void. Materialization failures land in the
    /// usual error slot; opening failures surface as a notification
    /// from [`open_external`]'s thread.
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
                        base: Some("shotori".into()),
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

    /// General: the two config files, the UI language, smart region.
    pub(super) fn page_general(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let body = div()
            .flex()
            .flex_col()
            .gap(px(18.))
            .max_w(px(CONTENT_MAX))
            .child(page_header(
                crate::i18n::t!("settings.general.title").as_ref(),
                crate::i18n::t!("settings.general.blurb").as_ref(),
            ))
            .child(card_group(
                Some(crate::i18n::t!("settings.general.group.files").as_ref()),
                vec![
                    self.file_row(
                        "settings.toml",
                        crate::i18n::t!("settings.general.file.settings.desc").to_string(),
                        ConfigFile::Settings,
                        cx,
                    ),
                    self.file_row(
                        "theme.toml",
                        crate::i18n::t!("settings.general.file.theme.desc").to_string(),
                        ConfigFile::Theme,
                        cx,
                    ),
                ],
            ))
            .child(card_group(
                Some(crate::i18n::t!("settings.general.group.interface").as_ref()),
                vec![card_row(
                    crate::i18n::t!("settings.general.language.label").to_string(),
                    Some(crate::i18n::t!("settings.general.language.blurb").to_string()),
                    div().w_full().child(
                        Select::new(&self.language_select)
                            .accessibility_label(
                                crate::i18n::t!("settings.general.language.label").to_string(),
                            )
                            .menu_width(px(240.)),
                    ),
                )],
            ))
            .child(card_group(
                Some(crate::i18n::t!("settings.general.group.selection").as_ref()),
                vec![card_row(
                    crate::i18n::t!("settings.general.smart_region.title").to_string(),
                    Some(crate::i18n::t!("settings.general.smart_region.body").to_string()),
                    self.smart_region_switch(cx),
                )],
            ));
        page_body("page-general").child(body)
    }

    /// One config file: the file name, what it holds, and the button
    /// that opens it. The name IS the label — a "Config files" heading
    /// plus two anonymous buttons made the reader match buttons to
    /// files by guesswork.
    fn file_row(
        &self,
        name: &'static str,
        description: String,
        file: ConfigFile,
        cx: &mut Context<Self>,
    ) -> Div {
        let t = theme::c();
        let entity = cx.entity();
        card_row(
            name,
            Some(description),
            Button::new(SharedString::from(name))
                .accessibility_label(format!(
                    "{}: {name}",
                    crate::i18n::t!("settings.general.file.open")
                ))
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
                    entity.update(cx, |this, cx| this.open_config(file, cx));
                })
                .child(crate::i18n::t!("settings.general.file.open").to_string()),
        )
    }

    /// The smart-region switch. Same write-through contract as the OCR
    /// upright toggle: the flip saves the file and the next capture reads
    /// it — a live overlay is not rewired mid-session (its detectors, if
    /// any, simply stay unused: the session queries the setting on every
    /// hover/press/wheel).
    fn smart_region_switch(&self, cx: &Context<Self>) -> Switch {
        let t = theme::c();
        let entity = cx.entity();
        Switch::new("smart-region")
            .checked(self.settings.smart_region())
            .on_click(move |checked, _, cx| {
                entity.update(cx, |this, cx| {
                    this.settings.set_smart_region(*checked);
                    this.persist(cx);
                });
            })
            .color(rgba(t.accent))
    }
}
