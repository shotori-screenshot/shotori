//! Visible screenshot palettes with a single selected choice.
use super::*;

impl SettingsWindow {
    pub(super) fn theme_choices(&self, window: &Window, cx: &Context<Self>) -> Div {
        let s = super::palette::surfaces(cx);
        let mut choices = vec![
            (
                "shotori-light",
                crate::i18n::t!("settings.appearance.shotori_light").to_string(),
            ),
            (
                "shotori-dark",
                crate::i18n::t!("settings.appearance.shotori_dark").to_string(),
            ),
        ];
        choices.extend(
            theme::load::BUILT_INS
                .iter()
                .map(|preset| (preset.name, preset.display.to_owned())),
        );
        let configured = match self.appearance.file.base.as_deref() {
            // the default, plus the legacy spellings of the retired
            // system-following pair
            None | Some("shotori-dark") | Some("shotori") | Some("auto") => "shotori-dark",
            Some(base) => base,
        };
        let columns = if window.viewport_size().width < window.rem_size() * 32. {
            1
        } else if window.viewport_size().width < window.rem_size() * 64. {
            2
        } else {
            3
        };
        let primary = cx.theme().primary;
        let ring = cx.theme().ring;
        let mut grid = div().grid().grid_cols(columns).gap_2p5().w_full();
        for (base, label) in choices {
            let selected = configured == base;
            let file = ThemeFile {
                base: Some(base.to_owned()),
                ..Default::default()
            };
            let (palette, _) = file.palette();
            // Miniature 2-pane window swatch: left sidebar strip + main body
            // with sample glyph and capsule accent pill.
            let preview = div()
                .flex()
                .w(px(46.))
                .h(px(36.))
                .flex_shrink_0()
                .overflow_hidden()
                .rounded(px(8.))
                .border_1()
                .border_color(rgba(palette.toolbar_border))
                .child(
                    div()
                        .w(px(12.))
                        .h_full()
                        .bg(rgba(palette.chip_bg | 0xFF))
                        .border_r_1()
                        .border_color(rgba(theme::alpha(palette.toolbar_border, 0x66))),
                )
                .child(
                    div()
                        .flex_1()
                        .h_full()
                        .bg(rgba(palette.toolbar_bg))
                        .flex()
                        .flex_col()
                        .items_center()
                        .justify_center()
                        .gap_1()
                        .child(
                            Icon::new(IconName::Type)
                                .size_3p5()
                                .text_color(rgba(palette.toolbar_text)),
                        )
                        .child(
                            div()
                                .w_4()
                                .h(px(3.))
                                .rounded_full()
                                .bg(rgba(palette.accent)),
                        ),
                );
            let swatch_dots = div().flex().items_center().gap_1().children(
                [palette.accent, palette.chip_bg, palette.toolbar_selected]
                    .into_iter()
                    .map(|c| {
                        div()
                            .size_2()
                            .rounded_full()
                            .bg(rgba(c | 0xFF))
                            .border_1()
                            .border_color(rgba(palette.toolbar_border))
                    }),
            );
            let hover_bg = s.selected_card_bg;
            grid = grid.child(
                Button::new(SharedString::from(format!("theme-choice-{base}")))
                    .accessibility_label(label.clone())
                    .selected(selected)
                    .flex()
                    .items_center()
                    .justify_start()
                    .h_auto()
                    .min_h(rems(4.25))
                    .gap_3()
                    .p_3()
                    .w_full()
                    .min_w_0()
                    .border_1()
                    .border_color(if selected { primary } else { s.hairline })
                    .rounded(px(14.))
                    .bg(if selected {
                        s.selected_card_bg
                    } else {
                        s.card_bg
                    })
                    .shadow(if selected {
                        s.selected_shadow.clone()
                    } else {
                        s.card_shadow.clone()
                    })
                    .text_color(cx.theme().foreground)
                    .text_sm()
                    .font_weight(FontWeight::MEDIUM)
                    .line_height(relative(1.35))
                    .hover(move |st| st.border_color(primary).bg(hover_bg))
                    .focus_visible(move |st| st.border_color(ring).bg(hover_bg))
                    .child(preview)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .items_start()
                            .gap_1()
                            .flex_1()
                            .min_w_0()
                            .child(div().w_full().truncate().child(label))
                            .child(swatch_dots),
                    )
                    .child(super::style_cards::selection_circle(selected, cx))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.appearance_set_base(base, window, cx);
                    })),
            );
        }
        grid
    }
}
