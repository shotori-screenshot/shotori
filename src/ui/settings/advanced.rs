//! Configuration-file access for advanced preferences.

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

impl SettingsWindow {
    /// Opens the config file in the system editor, creating it if absent.
    /// Creation failures appear inline; editor failures use a notification.
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

    /// Open the settings and theme files through the system editor.
    pub(super) fn page_advanced(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let body = super::layout::content_column()
            .child(page_header(cx, "settings.preferences.advanced", None))
            .child(card_group(
                cx,
                Some(crate::i18n::t!("settings.general.group.files").as_ref()),
                vec![
                    self.file_row("settings.toml", ConfigFile::Settings, cx),
                    self.file_row("theme.toml", ConfigFile::Theme, cx),
                ],
            ));
        page_body("page-general").child(body)
    }

    /// Pairs the file name and resolved path with its editor action.
    fn file_row(&self, name: &'static str, file: ConfigFile, cx: &Context<Self>) -> AnyElement {
        let entity = cx.entity();
        super::layout::compact_row(
            cx,
            name,
            None,
            Some(
                match file {
                    ConfigFile::Settings => self.path.as_ref(),
                    ConfigFile::Theme => self.appearance.path.as_ref(),
                }
                .map(|path| path.display().to_string())
                .unwrap_or_default(),
            ),
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
        )
        .into_any_element()
    }
}
