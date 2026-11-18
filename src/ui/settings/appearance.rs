//! Screenshot palette previews and advanced appearance controls. Changes are
//! applied to captures and written through immediately to theme.toml.

use crate::ui::color_picker::ColorPicker;
use gpui_kit::assets::IconName;
use gpui_kit::base::Button;
use gpui_kit::base::slider::{SliderEvent, SliderState, SliderValue};
use gpui_kit::component::color_picker::{ColorPickerEvent, ColorPickerState};
use gpui_kit::component::slider::Slider as ThemedSlider;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme, Icon};
use gpui_kit::*;

use crate::ui::theme::load::{self, ThemeFile};
use crate::ui::theme::{self, ColorKey};

use super::{
    AppearanceState, SettingsWindow, card_group, card_row, page_body, page_header,
    sync_shell_palette,
};

impl SettingsWindow {
    pub(super) fn page_appearance(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        use gpui_kit::component::select::Select;

        let reduce_motion = {
            let entity = cx.entity();
            super::layout::settings_switch(
                "appearance-reduce-motion",
                self.settings.capture().reduce_motion,
                crate::i18n::t!("settings.appearance.reduce_motion.title").to_string(),
                window,
                cx,
            )
            .on_change(move |checked, _, _, cx| {
                entity.update(cx, |this, cx| {
                    this.settings.capture_mut().reduce_motion = checked;
                    this.persist(cx);
                });
            })
        };

        let s = super::palette::surfaces(cx);

        let has_color_overrides = ColorKey::ALL
            .iter()
            .any(|key| self.appearance.file.color(*key).is_some());

        let palette_badge = has_color_overrides.then(|| {
            div()
                .text_xs()
                .font_weight(FontWeight::MEDIUM)
                .px_2p5()
                .py_0p5()
                .rounded_full()
                .border_1()
                .border_color(cx.theme().primary.opacity(0.35))
                .bg(s.selected_card_bg)
                .text_color(cx.theme().primary)
                .child(crate::i18n::t!("settings.appearance.custom_active").to_string())
                .into_any_element()
        });

        let mut body = super::layout::content_column()
            .child(page_header(cx, "settings.preferences.appearance", None))
            .child(card_group(
                cx,
                Some(crate::i18n::t!("settings.appearance.group.interface").as_ref()),
                vec![
                    card_row(
                        cx,
                        crate::i18n::t!("settings.general.language.label").to_string(),
                        None,
                        None,
                        Select::new(&self.language_select).w_full(),
                    ),
                    super::layout::compact_row(
                        cx,
                        crate::i18n::t!("settings.appearance.reduce_motion.title").to_string(),
                        Some("settings.appearance.reduce_motion.body"),
                        None,
                        reduce_motion,
                    ),
                ],
            ))
            .child(super::layout::section_block(
                cx,
                Some(crate::i18n::t!("settings.appearance.base_palette").as_ref()),
                palette_badge,
                self.theme_choices(window, cx),
            ));

        let mut custom = div().flex().flex_col().gap_3().child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap_3()
                .px_4()
                .py_2p5()
                .rounded(px(16.))
                .border_1()
                .border_color(s.hairline)
                .bg(s.card_bg)
                .shadow(s.card_shadow)
                .child(
                    Button::new("custom-colors")
                        .flex_1()
                        .flex()
                        .items_center()
                        .justify_start()
                        .gap_2()
                        .py_1()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(cx.theme().foreground)
                        .focus_visible(|st| st.border_1().border_color(cx.theme().ring))
                        .child(
                            Icon::new(if self.custom_colors {
                                IconName::ChevronDown
                            } else {
                                IconName::ChevronRight
                            })
                            .size_4(),
                        )
                        .child(crate::i18n::t!("settings.preferences.custom_colors").to_string())
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.custom_colors = !this.custom_colors;
                            cx.notify();
                        })),
                )
                .children((self.custom_colors || has_color_overrides).then(|| {
                    super::shell::reset_button("settings.appearance.reset_colors", cx, {
                        let entity = cx.entity();
                        move |_, window, cx| {
                            entity.update(cx, |this, cx| this.appearance_reset(window, cx));
                        }
                    })
                })),
        );
        if self.custom_colors {
            let mut colors = div().flex().flex_col().gap_4();
            for (title, keys) in [
                (
                    "settings.appearance.custom.selection",
                    &[
                        ColorKey::DimColor,
                        ColorKey::Accent,
                        ColorKey::PinBorder,
                        ColorKey::SwatchBorder,
                    ][..],
                ),
                (
                    "settings.appearance.custom.toolbar",
                    &[
                        ColorKey::ToolbarBg,
                        ColorKey::ToolbarText,
                        ColorKey::ToolbarBorder,
                        ColorKey::ToolbarHover,
                        ColorKey::ToolbarSelected,
                    ][..],
                ),
                (
                    "settings.appearance.custom.feedback",
                    &[
                        ColorKey::ChipBg,
                        ColorKey::HintText,
                        ColorKey::BtnText,
                        ColorKey::BtnHoverBg,
                    ][..],
                ),
            ] {
                colors = colors.child(card_group(
                    cx,
                    Some(crate::i18n::t!(title).as_ref()),
                    vec![self.color_grid(keys, cx)],
                ));
            }
            custom = custom.child(colors);
        }
        body = body.child(custom);
        page_body("page-appearance").child(body)
    }

    /// One cell per [`ColorKey`]: picker + label + per-color reset button when overridden.
    fn color_grid(&self, keys: &[ColorKey], cx: &Context<Self>) -> Div {
        let reset_hint = crate::i18n::t!("settings.appearance.reset_single").to_string();
        let mut grid = div()
            .grid()
            .grid_cols(2)
            .gap_x_6()
            .gap_y_2()
            .py_2p5()
            .w_full()
            .min_w_0();
        for (key, state) in self
            .appearance
            .pickers
            .iter()
            .filter(|(key, _)| keys.contains(key))
        {
            let key = *key;
            let overridden = self.appearance.file.color(key).is_some();
            let entity = cx.entity();
            let hint = reset_hint.clone();
            grid = grid.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .min_w_0()
                    .py_1()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2p5()
                            .min_w_0()
                            .child(ColorPicker::new(state, key.label().into_owned()))
                            .child(
                                div()
                                    .text_sm()
                                    .truncate()
                                    .text_color(cx.theme().foreground)
                                    .child(key.label().into_owned()),
                            ),
                    )
                    .children(overridden.then(|| {
                        Button::new(SharedString::from(format!("reset-color-{}", key.key())))
                            .accessibility_label(format!("{}: {}", hint, key.label()))
                            .size_6()
                            .flex_shrink_0()
                            .rounded_full()
                            .text_color(cx.theme().muted_foreground)
                            .hover(|s| {
                                s.bg(cx.theme().secondary_hover)
                                    .text_color(cx.theme().foreground)
                            })
                            .focus_visible(|s| s.border_1().border_color(cx.theme().ring))
                            .tooltip(move |_, cx| cx.new(|_| Tooltip::new(hint.clone())).into())
                            .child(Icon::new(IconName::Undo2).size_3p5())
                            .on_click(move |_, window, cx| {
                                entity.update(cx, |this, cx| {
                                    this.appearance.file.set_color(key, None);
                                    this.appearance_resync(window, cx);
                                    this.appearance_apply(cx);
                                });
                            })
                    })),
            );
        }
        grid
    }

    /// Dim opacity: the same slider used by the layout/interaction harness.
    pub(super) fn opacity_control(&self, cx: &Context<Self>) -> Div {
        opacity_slider(&self.appearance.opacity, cx)
    }
}

/// The kit owns track/thumb geometry, hit-testing and drag feedback.
fn opacity_slider(state: &Entity<SliderState>, cx: &App) -> Div {
    let value = state.read(cx).value().start();
    let s = super::palette::surfaces(cx);
    div()
        .flex()
        .items_center()
        .justify_end()
        .gap_3()
        .w_full()
        .child(
            div()
                .w(rems(9.))
                .flex_shrink_0()
                .child(ThemedSlider::new(state).horizontal()),
        )
        .child(
            div()
                .min_w(rems(3.75))
                .px_2()
                .py_0p5()
                .rounded_full()
                .bg(s.well_bg)
                .border_1()
                .border_color(s.hairline)
                .text_center()
                .text_xs()
                .font_weight(FontWeight::MEDIUM)
                .text_color(cx.theme().muted_foreground)
                .child(format!("{:.0}%", value * 100.)),
        )
}

// ── Appearance: state plumbing ─────────────────────────────────────────────

impl AppearanceState {
    pub(super) fn new(window: &mut Window, cx: &mut Context<SettingsWindow>) -> Self {
        let path = theme::load::config_path();
        let (file, errors) = load::load_editable(path.as_deref());
        let current = current_palette(&file);

        let mut subs = Vec::new();
        let mut pickers = Vec::new();
        for key in ColorKey::ALL {
            let value = u32_to_hsla(key.get(&current));
            let state = cx.new(|cx| ColorPickerState::new(window, cx).default_value(value));
            let sub = cx.subscribe(&state, move |this, _, event, cx| {
                if let ColorPickerEvent::Change(Some(color)) = event {
                    this.appearance
                        .file
                        .set_color(key, Some(load::to_hex(hsla_to_u32(*color))));
                    this.appearance_apply(cx);
                }
            });
            pickers.push((key, state));
            subs.push(sub);
        }
        let opacity = cx.new(|_| {
            SliderState::new()
                .min(0.)
                .max(1.)
                .step(0.05)
                .default_value(current.dim_opacity)
        });
        subs.push(cx.subscribe(&opacity, |this, _, event, cx| {
            if let SliderEvent::Change(SliderValue::Single(value)) = event {
                let value = (value.clamp(0., 1.) * 100.).round() / 100.;
                this.appearance.file.dim_opacity = Some(value);
                this.appearance_apply(cx);
            }
        }));
        Self {
            file,
            path,
            error: errors.first().cloned(),
            pickers,
            opacity,
            _subscriptions: subs,
        }
    }
}

impl SettingsWindow {
    /// Apply the working theme live — the settings window's own chrome
    /// re-skins, previewing the next capture's palette — and write
    /// theme.toml through.
    pub(super) fn appearance_apply(&mut self, cx: &mut Context<Self>) {
        let (theme, palette_errors) = self.appearance.file.palette();
        theme::set(theme);
        sync_shell_palette(cx);
        self.appearance.error = match palette_errors.first() {
            Some(error) => Some(error.clone()),
            None => match &self.appearance.path {
                Some(path) => load::save(path, &self.appearance.file).err(),
                None => Some(crate::i18n::t!("settings.no_config_dir").to_string()),
            },
        };
        cx.notify();
    }

    /// Re-seed every picker and the slider from the resolved palette.
    pub(super) fn appearance_resync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current = current_palette(&self.appearance.file);
        for (key, state) in &self.appearance.pickers {
            state.update(cx, |state, cx| {
                state.set_value(u32_to_hsla(key.get(&current)), window, cx)
            });
        }
        self.appearance.opacity.update(cx, |state, cx| {
            state.set_value(current.dim_opacity, window, cx)
        });
    }

    pub(super) fn appearance_set_base(
        &mut self,
        base: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .appearance
            .file
            .base
            .as_deref()
            .unwrap_or("shotori-dark")
            == base
        {
            return;
        }
        self.appearance.file.base = Some(base.to_string());
        self.appearance_resync(window, cx);
        self.appearance_apply(cx);
    }

    /// Clear custom colors while preserving the base and capture opacity.
    pub(super) fn appearance_reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for key in ColorKey::ALL {
            self.appearance.file.set_color(key, None);
        }
        self.appearance_resync(window, cx);
        self.appearance_apply(cx);
    }
}

fn current_palette(file: &ThemeFile) -> crate::ui::theme::Theme {
    let (theme, _) = file.palette();
    theme
}

use crate::ui::toolbar::{hsla_to_u32, u32_to_hsla};

#[cfg(test)]
mod tests {
    use super::opacity_slider;
    use gpui_kit::base::slider::{SliderState, SliderValue};
    use gpui_kit::component::Theme;
    use gpui_kit::{
        AppContext, Context, Entity, IntoElement, Modifiers, Render, TestAppContext, Window, point,
        px,
    };

    struct SliderFixture(Entity<SliderState>);

    impl Render for SliderFixture {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            opacity_slider(&self.0, cx)
        }
    }

    #[gpui_kit::test]
    fn standard_opacity_slider_maps_pointer_positions_across_text_sizes(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::base::init(cx);
            gpui_kit::component::init(cx);
        });
        let state = cx.new(|_| {
            SliderState::new()
                .min(0.)
                .max(1.)
                .step(0.05)
                .default_value(0.55)
        });
        let (_, cx) = cx.add_window_view(|_, _| SliderFixture(state.clone()));
        for rem in [16., 20., 24.] {
            for value in [0., 0.55, 1.] {
                cx.update(|window, cx| {
                    Theme::global_mut(cx).font_size = px(rem);
                    Theme::sync_base(cx);
                    window.set_rem_size(px(rem));
                    state.update(cx, |state, cx| state.set_value(value, window, cx));
                    window.draw(cx).clear(cx);
                });
                let rail = cx.update(|_, cx| state.read(cx).bounds());
                assert!(rail.size.width > px(0.));
                cx.simulate_click(
                    point(rail.left() + rail.size.width * 0.25, rail.center().y),
                    Modifiers::default(),
                );
                cx.update(|_, cx| {
                    assert!(matches!(state.read(cx).value(), SliderValue::Single(v)
                        if (v - 0.25).abs() < 0.001));
                });
            }
        }
    }
}
