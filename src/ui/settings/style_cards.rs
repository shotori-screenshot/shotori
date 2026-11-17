//! Selectable screenshot UI style cards (`Classic`, `Capsule`) with live
//! miniature chrome diagrams tinted by the active palette.

use super::*;
use crate::model::settings::UiStyle;

impl SettingsWindow {
    pub(super) fn style_choices(&self, window: &Window, cx: &Context<Self>) -> Div {
        let active_style = self.settings.ui_style();
        let columns = if window.viewport_size().width < window.rem_size() * 40. {
            1
        } else {
            2
        };
        let mut grid = div().grid().grid_cols(columns).gap_3().pt_2().w_full();
        for style in UiStyle::ALL {
            let selected = active_style == style;
            let title = crate::i18n::t!(style.title_key()).to_string();
            let body = crate::i18n::t!(style.body_key()).to_string();
            let diagram = style_diagram(style);
            grid = grid.child(
                Button::new(SharedString::from(format!("style-choice-{}", style.id())))
                    .debug_selector(move || format!("style-choice-{}", style.id()))
                    .accessibility_label(title.clone())
                    .selected(selected)
                    .flex()
                    .items_center()
                    .justify_start()
                    .h_auto()
                    .min_h(rems(5.25))
                    .gap_3()
                    .p_3()
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
                    .hover(|s| s.border_color(cx.theme().primary))
                    .focus_visible(|s| s.border_color(cx.theme().ring).bg(cx.theme().accent))
                    .child(diagram)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .items_start()
                            .gap_0p5()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(title),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .line_height(relative(1.35))
                                    .text_color(cx.theme().muted_foreground)
                                    .child(body),
                            ),
                    )
                    .child(
                        div()
                            .size_3p5()
                            .flex_shrink_0()
                            .children(selected.then(|| Icon::new(IconName::Check).size_3p5())),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if this.settings.ui_style() != style {
                            this.settings.set_ui_style(style);
                            this.persist(cx);
                        }
                    })),
            );
        }
        grid
    }
}

/// Miniature schematic of each UI style's selection frame, handle, badge and
/// floating toolbar, live-tinted with [`theme::c`].
fn style_diagram(style: UiStyle) -> Div {
    let t = theme::c();
    let well_bg = if t.is_dark() { 0x13141AFF } else { 0xE2E5ECFF };
    let crop_bg = if t.is_dark() { 0x222530FF } else { 0xFFFFFFFA };
    let mut stage = div()
        .relative()
        .w(px(108.))
        .h(px(66.))
        .flex_shrink_0()
        .overflow_hidden()
        .rounded(px(8.))
        .bg(rgba(well_bg))
        .border_1()
        .border_color(rgba(theme::alpha(t.toolbar_border, 0x88)));

    match style {
        UiStyle::Classic => {
            // Left-aligned accent badge + solid accent border + solid dot + compact bar
            stage = stage
                .child(
                    div()
                        .absolute()
                        .left(px(12.))
                        .top(px(5.))
                        .w(px(22.))
                        .h(px(6.))
                        .rounded(px(2.))
                        .bg(rgba(t.accent)),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(12.))
                        .top(px(13.))
                        .w(px(66.))
                        .h(px(26.))
                        .bg(rgba(crop_bg))
                        .border_1()
                        .border_color(rgba(t.accent)),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(75.5))
                        .top(px(36.5))
                        .size(px(5.))
                        .rounded_full()
                        .bg(rgba(t.accent)),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(12.))
                        .top(px(43.))
                        .h(px(15.))
                        .px(px(4.))
                        .flex()
                        .items_center()
                        .gap(px(3.))
                        .rounded(px(4.))
                        .bg(rgba(t.toolbar_bg))
                        .border_1()
                        .border_color(rgba(t.toolbar_border))
                        .child(
                            div()
                                .size(px(9.))
                                .rounded(px(2.))
                                .bg(rgba(t.toolbar_selected))
                                .border_1()
                                .border_color(rgba(t.accent)),
                        )
                        .child(
                            div()
                                .size(px(8.))
                                .rounded(px(2.))
                                .bg(rgba(theme::alpha(t.toolbar_text, 0x38))),
                        )
                        .child(div().size(px(8.)).rounded(px(2.)).bg(rgba(0xFF4545FF)))
                        .child(div().size(px(8.)).rounded(px(2.)).bg(rgba(0x40C878FF))),
                );
        }
        UiStyle::Capsule => {
            // Centered frosted capsule badge + white hairline + white-ring handle + pill bar
            stage = stage
                .child(
                    div()
                        .absolute()
                        .left(px(41.))
                        .top(px(5.))
                        .w(px(26.))
                        .h(px(6.))
                        .rounded_full()
                        .bg(rgba(t.capsule_surface()))
                        .border_1()
                        .border_color(rgba(t.capsule_border())),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(18.))
                        .top(px(13.))
                        .w(px(72.))
                        .h(px(25.))
                        .bg(rgba(crop_bg))
                        .border_1()
                        .border_color(rgba(0xFFFFFFE6)),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(87.))
                        .top(px(35.))
                        .size(px(6.))
                        .rounded_full()
                        .bg(rgba(0xFFFFFFFF))
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(div().size(px(3.)).rounded_full().bg(rgba(t.accent))),
                )
                .child(
                    div()
                        .absolute()
                        .left(px(17.))
                        .top(px(43.))
                        .h(px(16.))
                        .px(px(6.))
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .rounded_full()
                        .bg(rgba(t.capsule_surface()))
                        .border_1()
                        .border_color(rgba(t.capsule_border()))
                        .child(
                            div()
                                .w(px(2.))
                                .h(px(7.))
                                .rounded_full()
                                .bg(rgba(theme::alpha(t.toolbar_text, 0x45))),
                        )
                        .child(div().size(px(10.)).rounded(px(3.5)).bg(rgba(t.accent)))
                        .child(
                            div()
                                .size(px(8.))
                                .rounded_full()
                                .bg(rgba(theme::alpha(t.toolbar_text, 0x38))),
                        )
                        .child(
                            div()
                                .size(px(9.))
                                .rounded_full()
                                .border_1()
                                .border_color(rgba(t.accent))
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(div().size(px(5.)).rounded_full().bg(rgba(0xFF4545FF))),
                        )
                        .child(div().size(px(7.)).rounded_full().bg(rgba(0x409CFFFF))),
                );
        }
    }
    stage
}
