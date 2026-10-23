//! Settings-specific color presentation over the kit's existing picker state.
//! The stock picker changes a swatch's actual color on hover (lighten/darken),
//! which clips differently for pale and dark colors. A neutral outer ring
//! keeps the sampled color honest and gives every tile the same feedback.
use gpui_kit::base::slider::SliderState;
use gpui_kit::base::{Button, ColorPicker as PickerRoot, ColorPickerState, ColorSwatch};
use gpui_kit::component::input::Input;
use gpui_kit::component::popover::Popover;
use gpui_kit::component::slider::Slider;
use gpui_kit::component::tab::{Tab, TabBar};
use gpui_kit::component::{ActiveTheme, Sizable};
use gpui_kit::*;

#[derive(IntoElement)]
pub(super) struct SettingsColorPicker {
    state: Entity<ColorPickerState>,
    label: SharedString,
}

impl SettingsColorPicker {
    pub(super) fn new(state: &Entity<ColorPickerState>, label: String) -> Self {
        Self {
            state: state.clone(),
            label: label.into(),
        }
    }

    fn swatch(&self, id: impl Into<ElementId>, color: Hsla, cx: &App) -> ColorSwatch {
        let selected = self.state.read(cx).value() == Some(color);
        let state = self.state.clone();
        let hover = if selected {
            cx.theme().primary
        } else {
            cx.theme().foreground.opacity(0.7)
        };
        let ring = if selected {
            cx.theme().primary
        } else {
            cx.theme().transparent
        };
        ColorSwatch::new(id, color)
            .selected(selected)
            .size_6()
            .p(rems(0.125))
            .rounded(cx.theme().radius)
            .border_2()
            .border_color(ring)
            .hover(move |style| style.border_color(hover))
            .focus_visible(|style| style.border_color(cx.theme().ring))
            .child(
                div()
                    .size_full()
                    .rounded_sm()
                    .bg(color)
                    .border_1()
                    .border_color(cx.theme().border),
            )
            .on_click(move |color, _, window, cx| {
                state.update(cx, |state, cx| state.select_color(color, window, cx));
            })
    }

    fn palette(&self, cx: &App) -> Div {
        use gpui_kit::component::theme as colors;
        // Keep the kit's existing palette and featured colors. Only the tile
        // presentation changes; saved and selectable colors stay the same.
        let scales: [fn(usize) -> Hsla; 9] = [
            colors::stone,
            colors::red,
            colors::orange,
            colors::yellow,
            colors::green,
            colors::cyan,
            colors::blue,
            colors::purple,
            colors::pink,
        ];
        let shades = [950, 900, 800, 700, 600, 500, 400, 300, 200, 100, 50];
        let t = cx.theme();
        let featured = [
            t.red,
            t.red_light,
            t.blue,
            t.blue_light,
            t.green,
            t.green_light,
            t.yellow,
            t.yellow_light,
            t.cyan,
            t.cyan_light,
            t.magenta,
            t.magenta_light,
        ];
        let favorites = div().flex().gap_0p5().children(
            featured
                .into_iter()
                .enumerate()
                .map(|(index, color)| self.swatch(("featured", index), color, cx)),
        );
        let mut panel = div()
            .flex()
            .flex_col()
            .gap_1()
            .child(favorites)
            .child(div().my_2().border_t_1().border_color(t.border));
        for (row, scale) in scales.into_iter().enumerate() {
            let line = div()
                .flex()
                .gap_0p5()
                .children(shades.into_iter().enumerate().map(|(column, shade)| {
                    self.swatch(
                        ("palette-cell", row * shades.len() + column),
                        scale(shade),
                        cx,
                    )
                    .debug_selector(move || format!("palette-{row}-{column}"))
                }));
            panel = panel.child(line);
        }
        panel
    }

    fn channel(
        &self,
        label: String,
        state: &Entity<SliderState>,
        colors: Vec<Hsla>,
        value: f32,
        cx: &App,
    ) -> Div {
        div()
            .flex()
            .items_center()
            .gap_3()
            .child(
                div()
                    .w_16()
                    .flex_shrink_0()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(label),
            )
            .child(
                div()
                    .relative()
                    .flex()
                    .items_center()
                    .flex_1()
                    .min_w_0()
                    .h_8()
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .right_0()
                            .h_1p5()
                            .rounded_full()
                            .overflow_hidden()
                            .flex()
                            .children(
                                colors
                                    .into_iter()
                                    .map(|color| div().flex_1().h_full().bg(color)),
                            ),
                    )
                    .child(
                        Slider::new(state)
                            .w_full()
                            .bg(cx.theme().transparent)
                            .text_color(cx.theme().foreground),
                    ),
            )
            .child(
                div()
                    .w_8()
                    .text_right()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!("{value:.0}")),
            )
    }

    fn channels(&self, cx: &App) -> Div {
        let state = self.state.read(cx);
        let color = state.displayed_color().unwrap_or(hsla(0., 0., 0., 1.));
        let sliders = state.sliders();
        let samples = |f: &dyn Fn(f32) -> Hsla| (0..64).map(|i| f(i as f32 / 63.)).collect();
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(self.channel(
                crate::i18n::t!("settings.picker.hue").to_string(),
                sliders.hue(),
                samples(&|h| hsla(h, 1., 0.5, 1.)),
                color.h * 360.,
                cx,
            ))
            .child(self.channel(
                crate::i18n::t!("settings.picker.saturation").to_string(),
                sliders.saturation(),
                samples(&|s| hsla(color.h, s, color.l, 1.)),
                color.s * 100.,
                cx,
            ))
            .child(self.channel(
                crate::i18n::t!("settings.picker.lightness").to_string(),
                sliders.lightness(),
                samples(&|l| hsla(color.h, color.s, l, 1.)),
                color.l * 100.,
                cx,
            ))
            .child(self.channel(
                crate::i18n::t!("settings.picker.alpha").to_string(),
                sliders.alpha(),
                samples(&|a| cx.theme().popover.blend(hsla(color.h, color.s, color.l, a))),
                color.a * 100.,
                cx,
            ))
    }

    fn content(&self, window: &mut Window, cx: &mut App) -> Div {
        self.state
            .update(cx, |state, cx| state.sync_pending_value(window, cx));
        let state = self.state.read(cx);
        let active_tab = state.active_tab();
        let hex = state.hex_input().clone();
        let tabs = self.state.clone();
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(
                TabBar::new("picker-mode")
                    .segmented()
                    .selected_index(active_tab)
                    .on_click(move |tab: &usize, _, cx| {
                        tabs.update(cx, |state, cx| state.set_active_tab(*tab, cx))
                    })
                    .child(
                        Tab::new()
                            .flex_1()
                            .label(crate::i18n::t!("settings.picker.palette").to_string()),
                    )
                    .child(Tab::new().flex_1().label("HSLA")),
            )
            .child(if active_tab == 0 {
                self.palette(cx)
            } else {
                self.channels(cx)
            })
            .child(
                div()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .pt_3()
                    .child(Input::new(&hex).small()),
            )
    }
}

impl RenderOnce for SettingsColorPicker {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let state = self.state.read(cx);
        let open = state.is_open();
        let color = state.value().unwrap_or(cx.theme().background);
        let focus = self.state.focus_handle(cx);
        let focused = focus.is_focused(window);
        let root_state = self.state.clone();
        let popup_state = self.state.clone();
        let id: ElementId = ("settings-color", self.state.entity_id()).into();
        PickerRoot::new(id)
            .open(open)
            .track_focus(&focus)
            .accessibility_label(self.label.clone())
            .on_open_change(move |open, _, cx| {
                root_state.update(cx, |state, cx| state.set_open(open, cx))
            })
            .child(
                Popover::new("panel")
                    .open(open)
                    .w(rems(22.))
                    .on_open_change(move |open: &bool, _, cx| {
                        popup_state.update(cx, |state, cx| state.set_open(*open, cx))
                    })
                    .trigger(
                        Button::new("swatch-trigger")
                            .focusable(false)
                            .size_7()
                            .p(rems(0.125))
                            .rounded(cx.theme().radius)
                            .border_2()
                            .border_color(if open || focused {
                                cx.theme().ring
                            } else {
                                cx.theme().transparent
                            })
                            .hover(|s| s.border_color(cx.theme().foreground.opacity(0.7)))
                            .focus_visible(|s| s.border_color(cx.theme().ring))
                            .child(
                                div()
                                    .size_full()
                                    .rounded_sm()
                                    .bg(color)
                                    .border_1()
                                    .border_color(cx.theme().border),
                            ),
                    )
                    .children(open.then(|| self.content(window, cx))),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::SettingsColorPicker;
    use gpui_kit::base::ColorPickerState;
    use gpui_kit::component::{Root, theme};
    use gpui_kit::{
        AppContext, Context, Entity, Focusable, IntoElement, Modifiers, Render, RenderOnce,
        TestAppContext, Window, hsla,
    };

    struct PickerFixture {
        state: Entity<ColorPickerState>,
        palette_only: bool,
    }

    impl Render for PickerFixture {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let picker = SettingsColorPicker::new(&self.state, "Accent".into());
            if self.palette_only {
                picker.palette(cx).into_any_element()
            } else {
                picker.render(window, cx).into_any_element()
            }
        }
    }

    #[gpui_kit::test]
    fn editor_surfaces_are_opaque_even_when_screenshot_chips_are_translucent(
        cx: &mut TestAppContext,
    ) {
        cx.update(|cx| {
            gpui_kit::base::init(cx);
            gpui_kit::component::init(cx);
            crate::ui::settings::sync_shell_palette(cx);
            let theme = gpui_kit::component::Theme::global(cx);
            assert_eq!(theme.background.a, 1.);
            assert_eq!(theme.popover.a, 1.);
        });
    }

    #[gpui_kit::test]
    fn hovering_dark_and_light_tiles_preserves_color_and_tile_geometry(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::base::init(cx);
            gpui_kit::component::init(cx);
        });
        let (view, cx) = cx.add_window_view(|window, cx| PickerFixture {
            state: cx
                .new(|cx| ColorPickerState::new(window, cx).default_value(hsla(0.4, 0.5, 0.5, 1.))),
            palette_only: true,
        });
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
        let before = cx.update(|_, cx| view.read(cx).state.read(cx).value());
        for selector in ["palette-0-0", "palette-0-10", "palette-1-5"] {
            let bounds = cx
                .debug_bounds(selector)
                .expect("palette tile should render");
            cx.simulate_mouse_move(bounds.center(), None, Modifiers::default());
            cx.update(|window, cx| {
                window.draw(cx).clear(cx);
                assert_eq!(view.read(cx).state.read(cx).value(), before);
            });
            assert_eq!(cx.debug_bounds(selector), Some(bounds));
        }
        let red = cx
            .debug_bounds("palette-1-5")
            .expect("red tile should render");
        cx.simulate_click(red.center(), Modifiers::default());
        cx.update(|_, cx| assert_eq!(view.read(cx).state.read(cx).value(), Some(theme::red(500))));
    }

    #[gpui_kit::test]
    fn picker_keeps_standard_keyboard_open_and_escape_dismissal(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::base::init(cx);
            gpui_kit::component::init(cx);
        });
        let mut state_handle = None;
        let (_, cx) = cx.add_window_view(|window, cx| {
            let state = cx.new(|cx| ColorPickerState::new(window, cx));
            state_handle = Some(state.clone());
            let view = cx.new(|_| PickerFixture {
                state,
                palette_only: false,
            });
            Root::new(view, window, cx)
        });
        let state = state_handle.expect("picker state should exist");
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
            window.focus(&state.focus_handle(cx), cx);
        });
        cx.simulate_keystrokes("enter");
        cx.update(|window, cx| {
            assert!(state.read(cx).is_open());
            window.draw(cx).clear(cx);
        });
        cx.simulate_keystrokes("escape");
        cx.update(|_, cx| assert!(!state.read(cx).is_open()));
    }
}
