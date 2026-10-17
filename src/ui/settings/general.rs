//! The General page: the config-file open buttons, the UI language
//! picker, the smart-region switch and the write-through hint — the
//! installation-level controls (everything else on this page is
//! reference, not configuration).

use gpui_kit::base::Button;
use gpui_kit::component::select::Select;
use gpui_kit::component::switch::Switch;
use gpui_kit::*;

use crate::ui::theme;
use crate::ui::theme::load::{self, ThemeFile};

use super::{PAGE_PAD, SettingsWindow, open_external, page_header};

/// Which config file an action targets (General page's edit buttons).
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
    /// from [`open_config_file`]'s thread.
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
    /// General: read-only facts about this installation plus the UI
    /// language picker (the one control here that changes something).
    pub(super) fn page_general(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let t = theme::c();
        div()
            .id("page-general")
            .flex()
            .flex_col()
            .gap(px(14.))
            .p(px(PAGE_PAD))
            .size_full()
            .overflow_y_scroll()
            .child(page_header(
                crate::i18n::t!("settings.general.title").as_ref(),
                crate::i18n::t!("settings.general.blurb").as_ref(),
            ))
            .child(self.config_files_row(cx))
            .child(self.language_control(cx))
            .child(self.smart_region_control(cx))
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(rgba(t.hint_text))
                    .child(crate::i18n::t!("settings.general.write_through").to_string()),
            )
    }

    /// The smart-region switch (General page). Same write-through
    /// contract as the OCR upright toggle: the flip saves the file and
    /// the next capture reads it — a live overlay is not rewired
    /// mid-session (its detectors, if any, simply stay unused: the
    /// session queries the setting on every hover/press/wheel).
    fn smart_region_control(&self, cx: &Context<Self>) -> Div {
        let t = theme::c();
        let entity = cx.entity();
        div()
            .flex()
            .items_center()
            .gap(px(10.))
            .p(px(10.))
            .rounded(px(6.))
            .border_1()
            .border_color(rgba(t.toolbar_border))
            .child(
                Switch::new("smart-region")
                    .checked(self.settings.smart_region())
                    .on_click(move |checked, _, cx| {
                        entity.update(cx, |this, cx| {
                            this.settings.set_smart_region(*checked);
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
                    .child(
                        div().text_size(px(13.)).text_color(rgba(t.btn_text)).child(
                            crate::i18n::t!("settings.general.smart_region.title").to_string(),
                        ),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgba(t.hint_text))
                            .child(
                                crate::i18n::t!("settings.general.smart_region.body").to_string(),
                            ),
                    ),
            )
    }
    /// The config-file row (General page): one compact button per
    /// file, opening it in the system editor. Replaces the old
    /// read-only path rows — the location is discovered by opening the
    /// file, and always-on paths everywhere was clutter.
    fn config_files_row(&self, cx: &mut Context<Self>) -> Div {
        let t = theme::c();
        let entity = cx.entity();
        let button = |label: &'static str, file: ConfigFile| {
            Button::new(SharedString::from(label))
                .accessibility_label(format!(
                    "{}: {label}",
                    crate::i18n::t!("settings.general.config_files")
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
                .on_click({
                    let entity = entity.clone();
                    move |_, _, cx| {
                        entity.update(cx, |this, cx| this.open_config(file, cx));
                    }
                })
                .child(label)
        };
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgba(t.hint_text))
                    .child(crate::i18n::t!("settings.general.config_files").to_string()),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(6.))
                    .child(button("settings.toml", ConfigFile::Settings))
                    .child(button("theme.toml", ConfigFile::Theme)),
            )
    }

    /// The language picker (General page). A Confirm writes through,
    /// flips the process-global locale and refreshes every window —
    /// `t!` resolves on render, so the whole window including the
    /// component-library texts switches live, no restart.
    fn language_control(&self, _: &Context<Self>) -> Div {
        let t = theme::c();
        div()
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgba(t.hint_text))
                    .child(crate::i18n::t!("settings.general.language.label").to_string()),
            )
            .child(
                div().w(px(280.)).child(
                    Select::new(&self.language_select)
                        .accessibility_label(
                            crate::i18n::t!("settings.general.language.label").to_string(),
                        )
                        .menu_width(px(280.)),
                ),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(rgba(t.hint_text))
                    .child(crate::i18n::t!("settings.general.language.blurb").to_string()),
            )
    }
}
