//! Visible screenshot palettes with a single selected choice.
use super::*;

impl SettingsWindow {
    pub(super) fn theme_choices(&self, window: &Window, cx: &Context<Self>) -> Div {
        let mut choices = vec![
            (
                "shotori",
                crate::i18n::t!("settings.appearance.system").to_string(),
            ),
            (
                "light",
                crate::i18n::t!("settings.appearance.light").to_string(),
            ),
            (
                "dark",
                crate::i18n::t!("settings.appearance.dark").to_string(),
            ),
        ];
        choices.extend(
            theme::load::BUILT_INS
                .iter()
                .map(|preset| (preset.name, preset.display.to_owned())),
        );
        let configured = match self.appearance.file.base.as_deref() {
            None | Some("auto") => "shotori",
            Some(base) => base,
        };
        let columns = if window.viewport_size().width < window.rem_size() * 32. {
            1
        } else if window.viewport_size().width < window.rem_size() * 64. {
            2
        } else {
            3
        };
        let mut grid = div().grid().grid_cols(columns).gap_2().pt_2().w_full();
        for (base, label) in choices {
            let selected = configured == base;
            let file = ThemeFile {
                base: Some(base.to_owned()),
                ..Default::default()
            };
            let (dark, light, _) = file.palettes();
            let palette = if theme::system_dark() {
                dark
            } else {
                light.unwrap_or(dark)
            };
            // Sample foreground and accent on their real surface. Hover and
            // selected fills are interaction states, not standalone swatches.
            let preview = div()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_1()
                .size_10()
                .flex_shrink_0()
                .rounded(cx.theme().radius)
                .border_1()
                .border_color(rgba(palette.toolbar_border))
                .bg(rgba(palette.toolbar_bg))
                .child(
                    Icon::new(IconName::Type)
                        .size_4()
                        .text_color(rgba(palette.toolbar_text)),
                )
                .child(
                    div()
                        .w_5()
                        .h(rems(0.125))
                        .rounded_full()
                        .bg(rgba(palette.accent)),
                );
            grid = grid.child(
                Button::new(SharedString::from(format!("theme-choice-{base}")))
                    .accessibility_label(label.clone())
                    .selected(selected)
                    .flex()
                    .items_center()
                    .justify_start()
                    .h_auto()
                    .min_h(rems(4.))
                    .gap_3()
                    .p_2()
                    .w_full()
                    .min_w_0()
                    .border_1()
                    .border_color(if selected {
                        cx.theme().primary
                    } else {
                        cx.theme().border
                    })
                    .rounded(cx.theme().radius)
                    .bg(if selected {
                        cx.theme().accent
                    } else {
                        cx.theme().group_box
                    })
                    .text_color(cx.theme().foreground)
                    .text_sm()
                    .line_height(relative(1.4))
                    .hover(|s| s.border_color(cx.theme().primary))
                    .focus_visible(|s| s.border_color(cx.theme().ring).bg(cx.theme().accent))
                    .child(preview)
                    .child(div().flex_1().min_w_0().child(label))
                    .child(
                        div()
                            .size_3()
                            .flex_shrink_0()
                            .children(selected.then(|| Icon::new(IconName::Check).size_3())),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.appearance_set_base(base, window, cx);
                    })),
            );
        }
        grid
    }
}
