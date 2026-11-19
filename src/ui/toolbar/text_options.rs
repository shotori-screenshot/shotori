//! Retained text controls; the annotation model owns styles and editing history.
use super::*;
use crate::annotation::text::{self, TextStyle};
use gpui_kit::{
    base::slider::SliderEvent,
    component::{
        Sizable,
        color_picker::ColorPickerEvent,
        input::{Input, InputEvent, InputState},
        popover::Popover,
        select::{SearchableVec, Select, SelectEvent, SelectState},
    },
};

struct TextControls {
    return_focus: FocusHandle,
    font: Entity<SelectState<SearchableVec<String>>>,
    size: Entity<InputState>,
    outline: Entity<ColorPickerState>,
    background: Entity<ColorPickerState>,
    padding: Entity<SliderState>,
    radius: Entity<SliderState>,
    subscriptions: Vec<Subscription>,
}

impl TextControls {
    fn new(
        session: Entity<ScreenshotSession>,
        return_focus: FocusHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let style = session.read(cx).annotations().text_style().clone();
        let font = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(text::families().to_vec()),
                None,
                window,
                cx,
            )
            .searchable(true)
        });
        let size = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(format!("{:.0}", session.read(cx).annotations().text_size()))
        });
        let outline = cx.new(|cx| {
            ColorPickerState::new(window, cx)
                .default_value(u32_to_hsla(style.outline.unwrap_or(0xffffffff)))
        });
        let background = cx.new(|cx| {
            ColorPickerState::new(window, cx)
                .default_value(u32_to_hsla(style.background.unwrap_or(0xffffffff)))
        });
        let padding = cx.new(|_| {
            SliderState::new()
                .min(0.)
                .max(48.)
                .default_value(style.padding)
        });
        let radius = cx.new(|_| {
            SliderState::new()
                .min(0.)
                .max(48.)
                .default_value(style.radius)
        });
        let font_session = session.clone();
        let mut subscriptions = vec![cx.subscribe(&font, move |_, _, event, cx| {
            if let SelectEvent::Confirm(Some(font)) = event {
                edit(&font_session, cx, |style| style.font = Some(font.clone()));
            }
        })];
        subscriptions.push(cx.subscribe_in(
            &font,
            window,
            |this, font, _: &DismissEvent, window, cx| {
                // The generic Select restores its trigger. Annotation controls
                // return to the canvas/editor so drawing shortcuts and typing resume.
                // An outside click may already have focused another input.
                if font.read(cx).focus_handle(cx).is_focused(window) {
                    window.focus(&this.return_focus, cx);
                }
            },
        ));
        let size_session = session.clone();
        subscriptions.push(
            cx.subscribe_in(&size, window, move |_, input, event, window, cx| {
                if !matches!(event, InputEvent::PressEnter { .. } | InputEvent::Blur) {
                    return;
                }
                if let Ok(value) = input.read(cx).value().trim().parse::<f32>()
                    && value.is_finite()
                {
                    size_session.update(cx, |s, cx| {
                        s.edit_annotation_settings(|a| {
                            a.end_size_drag();
                            a.apply_size(value);
                            a.end_size_drag();
                        });
                        cx.notify();
                    });
                }
                let accepted = size_session.read(cx).annotations().text_size();
                input.update(cx, |s, cx| {
                    s.set_value(format!("{accepted:.0}"), window, cx)
                });
            }),
        );
        for (picker, is_background) in [(&outline, false), (&background, true)] {
            let session = session.clone();
            subscriptions.push(cx.subscribe(picker, move |_, _, event, cx| {
                if let ColorPickerEvent::Change(Some(value)) = event {
                    let color = rgba_value(*value);
                    edit(&session, cx, |style| {
                        if is_background {
                            style.background = Some(color);
                        } else {
                            style.outline = Some(color);
                        }
                    });
                }
            }));
        }
        for (slider, is_radius) in [(&padding, false), (&radius, true)] {
            let session = session.clone();
            subscriptions.push(cx.subscribe(slider, move |_, _, event, cx| {
                session.update(cx, |s, cx| {
                    s.edit_annotation_settings(|a| match event {
                        SliderEvent::Change(value) => {
                            let mut style = a.text_style().clone();
                            if is_radius {
                                style.radius = value.start();
                            } else {
                                style.padding = value.start();
                            }
                            a.drag_text_style(style);
                        }
                        SliderEvent::Release(_) => a.end_size_drag(),
                    });
                    cx.notify();
                });
            }));
        }
        let mut this = Self {
            return_focus,
            font,
            size,
            outline,
            background,
            padding,
            radius,
            subscriptions,
        };
        this.sync(&style, window, cx);
        this.subscriptions.push(
            cx.observe_in(&session, window, |this, session, window, cx| {
                let style = session.read(cx).annotations().text_style().clone();
                this.sync(&style, window, cx);
                if !this.size.read(cx).focus_handle(cx).is_focused(window) {
                    let size = format!("{:.0}", session.read(cx).annotations().text_size());
                    if this.size.read(cx).value().as_ref() != size {
                        this.size.update(cx, |s, cx| s.set_value(size, window, cx));
                    }
                }
                cx.notify();
            }),
        );
        this
    }

    fn sync(&self, style: &TextStyle, window: &mut Window, cx: &mut App) {
        if self.font.read(cx).selected_value() != style.font.as_ref() {
            self.font.update(cx, |state, cx| {
                if let Some(font) = &style.font {
                    state.set_selected_value(font, window, cx);
                } else {
                    state.set_selected_index(None, window, cx);
                }
            });
        }
        for (picker, value) in [
            (&self.outline, style.outline),
            (&self.background, style.background),
        ] {
            if let Some(value) = value {
                let value = u32_to_hsla(value);
                if !picker.read(cx).is_open() && picker.read(cx).value() != Some(value) {
                    picker.update(cx, |state, cx| state.set_value(value, window, cx));
                }
            }
        }
        for (slider, value) in [(&self.padding, style.padding), (&self.radius, style.radius)] {
            if slider.read(cx).value().start() != value {
                slider.update(cx, |state, cx| state.set_value(value, window, cx));
            }
        }
    }
}

fn edit(session: &Entity<ScreenshotSession>, cx: &mut App, f: impl FnOnce(&mut TextStyle)) {
    session.update(cx, |s, cx| {
        s.edit_annotation_settings(|a| {
            let mut style = a.text_style().clone();
            f(&mut style);
            a.set_text_style(style);
        });
        cx.notify();
    });
}

fn rgba_value(value: Hsla) -> u32 {
    let rgba: Rgba = value.into();
    let alpha = (rgba.a.clamp(0., 1.) * 255.).round() as u32;
    (hsla_to_u32(value) & 0xffffff00) | alpha
}

fn controls_state(
    session: &Entity<ScreenshotSession>,
    focus: &FocusHandle,
    window: &mut Window,
    cx: &mut App,
) -> Entity<TextControls> {
    let init_session = session.clone();
    let return_focus = focus.clone();
    let state = window.use_keyed_state(
        ("annotation-text-controls", session.entity_id()),
        cx,
        |window, cx| TextControls::new(init_session, return_focus, window, cx),
    );
    state.update(cx, |controls, _| controls.return_focus = focus.clone());
    state
}

pub(super) fn append_items(
    annotations: &crate::annotation::Annotations,
    session: &Entity<ScreenshotSession>,
    focus: &FocusHandle,
    items: &mut Vec<row::OverflowItem>,
) {
    let style = annotations.text_style().clone();
    for (id, active, icon, change) in [
        (
            "toolbar.text_bold",
            style.bold,
            IconName::Bold,
            (|s: &mut TextStyle| s.bold = !s.bold) as fn(&mut TextStyle),
        ),
        ("toolbar.text_italic", style.italic, IconName::Italic, |s| {
            s.italic = !s.italic
        }),
        (
            "toolbar.text_outline",
            style.outline.is_some(),
            IconName::TypeOutline,
            |s| {
                s.outline = if s.outline.is_some() {
                    None
                } else {
                    Some(0xffffffff)
                }
            },
        ),
        (
            "toolbar.text_background",
            style.background.is_some(),
            IconName::SquareDashed,
            |s| {
                s.background = if s.background.is_some() {
                    None
                } else {
                    Some(0xffffffff)
                }
            },
        ),
    ] {
        let btn_session = session.clone();
        let btn_focus = focus.clone();
        items.push(row::OverflowItem::control(
            BTN_SIZE,
            active,
            move |_, _, _| {
                let session = btn_session.clone();
                control(
                    id.trim_start_matches("toolbar.").into(),
                    crate::i18n::t!(id).into_owned(),
                    active,
                    btn_focus.clone(),
                    move |_, cx| edit(&session, cx, change),
                )
                .child(annot_options::menu_icon_colored(
                    icon,
                    18.,
                    icon_tint(active),
                ))
                .into_any_element()
            },
        ));
        if active {
            let color = match id {
                "toolbar.text_outline" => {
                    Some((false, "text-outline-color", "toolbar.text_outline_color"))
                }
                "toolbar.text_background" => Some((
                    true,
                    "text-background-color",
                    "toolbar.text_background_color",
                )),
                _ => None,
            };
            if let Some((is_bg, selector, label)) = color {
                let picker_session = session.clone();
                let picker_focus = focus.clone();
                items.push(row::OverflowItem::control(
                    28.,
                    false,
                    move |_, window, cx| {
                        let state = controls_state(&picker_session, &picker_focus, window, cx);
                        let picker = {
                            let controls = state.read(cx);
                            if is_bg {
                                controls.background.clone()
                            } else {
                                controls.outline.clone()
                            }
                        };
                        div()
                            .debug_selector(move || selector.into())
                            .child(ColorPicker::new(
                                &picker,
                                crate::i18n::t!(label).into_owned(),
                            ))
                            .into_any_element()
                    },
                ));
            }
        }
    }
    if style.background.is_some() {
        let bg_session = session.clone();
        let bg_focus = focus.clone();
        items.push(row::OverflowItem::control(
            BTN_SIZE,
            false,
            move |_, window, cx| {
                let state = controls_state(&bg_session, &bg_focus, window, cx);
                let (padding, radius) = {
                    let controls = state.read(cx);
                    (controls.padding.clone(), controls.radius.clone())
                };
                let t = theme::c();
                let pop = Popover::new("text-background-options")
                    .p(px(0.))
                    .rounded(px(12.))
                    .bg(rgba(t.capsule_sheet_surface()))
                    .border_1()
                    .border_color(rgba(t.capsule_border()))
                    .shadow(theme::capsule_floating_shadow());
                pop.on_open_change({
                    let session = bg_session.clone();
                    let focus = bg_focus.clone();
                    move |open, window, cx| {
                        if !open {
                            window.focus(&focus, cx);
                        }
                        session.update(cx, |s, _| {
                            s.edit_annotation_settings(|a| a.end_size_drag());
                        });
                    }
                })
                .trigger(
                    control(
                        "text-background-options".into(),
                        crate::i18n::t!("toolbar.text_background_settings").into_owned(),
                        false,
                        bg_focus.clone(),
                        |_, _| {},
                    )
                    .on_click(|_, _, _| {})
                    .child(annot_options::menu_icon(IconName::Settings2, 18.)),
                )
                .content(move |_, _, cx| {
                    annotation_menu_panel().flex().flex_col().gap_2().children(
                        [
                            (&padding, "toolbar.text_padding"),
                            (&radius, "toolbar.corner_radius"),
                        ]
                        .into_iter()
                        .map(|(state, key)| {
                            let sc = SizeSlider {
                                state: state.clone(),
                                current: state.read(cx).value().start(),
                                percentage: state.read(cx).percentage().end,
                            };
                            div()
                                .debug_selector(move || key.to_owned())
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .text_sm()
                                        .w(rems(7.))
                                        .flex_shrink_0()
                                        .whitespace_nowrap()
                                        .child(crate::i18n::t!(key).into_owned()),
                                )
                                .child(size_control(&sc, ""))
                        }),
                    )
                })
                .into_any_element()
            },
        ));
    }
    let font_session = session.clone();
    let font_focus = focus.clone();
    items.push(row::OverflowItem::control(
        160.,
        false,
        move |_, window, cx| {
            let state = controls_state(&font_session, &font_focus, window, cx);
            let font = state.read(cx).font.clone();
            Select::new(&font)
                .id("text-font")
                .small()
                .w_40()
                .menu_width(rems(16.))
                .menu_max_h(rems(18.))
                .placeholder(crate::i18n::t!("toolbar.text_font_default").into_owned())
                .accessibility_label(crate::i18n::t!("toolbar.text_font").into_owned())
                .into_any_element()
        },
    ));
    let size_session = session.clone();
    let size_focus = focus.clone();
    items.push(row::OverflowItem::control(
        48.,
        false,
        move |_, window, cx| {
            let state = controls_state(&size_session, &size_focus, window, cx);
            let size_input = state.read(cx).size.clone();
            div()
                .debug_selector(|| "tb-size-readout".into())
                .on_action(|_: &gpui_kit::base::input::Enter, _, cx| cx.stop_propagation())
                .child(
                    Input::new(&size_input)
                        .small()
                        .w(px(48.))
                        .appearance(false)
                        .aria_label(crate::i18n::t!("toolbar.text_size").into_owned()),
                )
                .into_any_element()
        },
    ));
}
