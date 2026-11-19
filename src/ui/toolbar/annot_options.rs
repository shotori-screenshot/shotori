//! The annotation settings row — the second toolbar row that appears
//! when a shape tool is active. Builds overflow-aware [`row::OverflowItem`]
//! entries so narrow outputs spill trailing controls into the `...` popover
//! instead of clipping or drawing a scrollbar.

use super::*;
use crate::annotation::ShapeKind;
use gpui_kit::prelude::FluentBuilder as _;

const CHOICE_W: f32 = 42.;
const SIZE_SLIDER_W: f32 = 142.;
const SWATCH_W: f32 = 28.;

/// Builds the settings row items for the active `edit_kind`. `session` and
/// `settings_focus` are shared with the caller; controls clone what their
/// closures capture so items can render either inline or in the `...` popover.
#[allow(clippy::too_many_arguments)]
pub(super) fn annot_options_items(
    edit_kind: Option<crate::annotation::ShapeKind>,
    annotations: &crate::annotation::Annotations,
    session: &Entity<ScreenshotSession>,
    settings_focus: &FocusHandle,
    size_slider: &Option<SizeSlider>,
    swatch_picker: &Option<Entity<ColorPickerState>>,
    selected_color: u32,
) -> Vec<row::OverflowItem> {
    let filter_tool = matches!(
        edit_kind,
        Some(crate::annotation::ShapeKind::Mosaic | crate::annotation::ShapeKind::Blur)
    );
    let eraser_tool = edit_kind == Some(crate::annotation::ShapeKind::Eraser);
    let mut items = Vec::new();

    if edit_kind == Some(crate::annotation::ShapeKind::Spotlight) {
        let ellipse = annotations.spotlight_ellipse();
        let s = session.clone();
        let f = settings_focus.clone();
        items.push(row::OverflowItem::control(
            CHOICE_W,
            false,
            move |_, _, _| shape_menu(ellipse, true, &s, &f).into_any_element(),
        ));
        if let Some(sc) = size_slider.clone() {
            items.push(row::OverflowItem::control(228., false, move |_, _, _| {
                div()
                    .flex()
                    .items_center()
                    .gap(px(BTN_GAP))
                    .child(
                        div()
                            .text_size(px(12.))
                            .child(crate::i18n::t!("toolbar.spotlight_opacity").into_owned()),
                    )
                    .child(size_control(&sc, ""))
                    .child("%")
                    .into_any_element()
            }));
        }
        super::spotlight_options::append_items(
            annotations,
            session,
            settings_focus,
            swatch_picker,
            &mut items,
        );
        return items;
    }

    if edit_kind == Some(crate::annotation::ShapeKind::Magnifier) {
        use crate::annotation::LensOption;
        let (ellipse, smooth, marks, connector) = annotations.lens_options();
        {
            let s = session.clone();
            let f = settings_focus.clone();
            items.push(row::OverflowItem::control(
                CHOICE_W,
                false,
                move |_, _, _| shape_menu(ellipse, false, &s, &f).into_any_element(),
            ));
        }
        for (option, active, key, icon) in [
            (
                LensOption::Annotations,
                marks,
                "toolbar.lens_annotations",
                IconName::Pencil,
            ),
            (
                LensOption::Smooth,
                smooth,
                "toolbar.lens_smooth",
                IconName::Sparkles,
            ),
        ] {
            let s = session.clone();
            let f = settings_focus.clone();
            items.push(row::OverflowItem::control(BTN_SIZE, false, move |_, _, _| {
                let session = s.clone();
                let fg = icon_tint(active);
                control(
                    key.into(),
                    crate::i18n::t!(key).into_owned(),
                    active,
                    f.clone(),
                    move |_, cx| {
                        session.update(cx, |s, cx| {
                            s.edit_annotations(|a| a.set_lens_option(option, !active));
                            cx.notify();
                        });
                    },
                )
                .child(if matches!(option, LensOption::Smooth) {
                    svg()
                        .data(b"<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 18 18'><g fill='none' stroke='black' stroke-width='1.5' stroke-linecap='round' stroke-linejoin='round'><path d='M2 12V8H6V4H10'/><path d='M5 15L15 5'/></g></svg>")
                        .size(px(18.))
                        .text_color(rgba(fg))
                        .into_any_element()
                } else {
                    menu_icon_colored(icon, 18., fg).into_any_element()
                })
                .into_any_element()
            }));
        }
        {
            let s = session.clone();
            let f = settings_focus.clone();
            items.push(row::OverflowItem::control(
                CHOICE_W,
                false,
                move |_, _, _| {
                    super::line_options::connector_menu(connector, &s, &f).into_any_element()
                },
            ));
        }
        {
            let shadow = annotations.lens_shadow();
            let s = session.clone();
            let f = settings_focus.clone();
            items.push(row::OverflowItem::control(BTN_SIZE, false, move |_, _, _| {
                let shadow_session = s.clone();
                let shadow_icon = svg()
                    .data(b"<svg xmlns='http://www.w3.org/2000/svg' width='18' height='18'><g fill='none' stroke='black' stroke-width='1.5' stroke-linecap='round' stroke-linejoin='round'><rect x='3' y='2' width='11' height='10' rx='1.5'/><path d='M5 15H15' opacity='.5'/><path d='M7 17H16' opacity='.25'/></g></svg>")
                    .size(px(18.))
                    .text_color(rgba(icon_tint(shadow)));
                control(
                    "lens-shadow".into(),
                    crate::i18n::t!("toolbar.lens_shadow").into_owned(),
                    shadow,
                    f.clone(),
                    move |_, cx| {
                        shadow_session.update(cx, |s, cx| {
                            s.edit_annotations(|a| a.set_lens_option(LensOption::Shadow, !shadow));
                            cx.notify();
                        });
                    },
                )
                .child(shadow_icon)
                .into_any_element()
            }));
        }
        if let Some(sc) = size_slider.clone() {
            items.push(row::OverflowItem::control(
                SIZE_SLIDER_W,
                false,
                move |_, _, _| size_control(&sc, "×").into_any_element(),
            ));
        }
        if let Some(picker) = swatch_picker.clone() {
            items.push(row::OverflowItem::separator());
            items.push(row::OverflowItem::control(
                SWATCH_W,
                false,
                move |_, _, _| {
                    ColorPicker::new(&picker, crate::i18n::t!("toolbar.lens_color").into_owned())
                        .opaque()
                        .into_any_element()
                },
            ));
        }
        return items;
    }

    if filter_tool {
        {
            let effect = edit_kind.unwrap_or(ShapeKind::Mosaic);
            let s = session.clone();
            let f = settings_focus.clone();
            items.push(row::OverflowItem::control(
                CHOICE_W,
                false,
                move |_, _, _| {
                    icon_choice_menu(
                        "filter-effect",
                        "toolbar.filter_mosaic",
                        effect,
                        &[
                            (
                                ShapeKind::Mosaic,
                                "toolbar.filter_mosaic",
                                OptionIcon::Mosaic,
                            ),
                            (
                                ShapeKind::Blur,
                                "toolbar.filter_blur",
                                OptionIcon::BuiltIn(IconName::MirrorRectangular),
                            ),
                        ],
                        &s,
                        &f,
                        |a, kind| a.set_filter_effect(kind),
                    )
                    .into_any_element()
                },
            ));
        }
        let area_kind = annotations.filter_area_kind();
        let brush = area_kind == crate::annotation::FilterAreaKind::Brush;
        {
            let s = session.clone();
            let f = settings_focus.clone();
            items.push(row::OverflowItem::control(
                CHOICE_W,
                false,
                move |_, _, _| {
                    icon_choice_menu(
                        "filter-area",
                        "toolbar.region_shape",
                        area_kind,
                        &[
                            (
                                crate::annotation::FilterAreaKind::Rectangle,
                                "toolbar.filter_rectangle",
                                OptionIcon::BuiltIn(IconName::Square),
                            ),
                            (
                                crate::annotation::FilterAreaKind::Ellipse,
                                "toolbar.filter_ellipse",
                                OptionIcon::BuiltIn(IconName::Circle),
                            ),
                            (
                                crate::annotation::FilterAreaKind::Brush,
                                "toolbar.filter_brush",
                                OptionIcon::BuiltIn(IconName::Pencil),
                            ),
                        ],
                        &s,
                        &f,
                        |a, area_kind| a.set_filter_area_kind(area_kind),
                    )
                    .into_any_element()
                },
            ));
        }
        if brush {
            let editing_diameter = annotations.editing_filter_brush_size();
            let s = session.clone();
            let f = settings_focus.clone();
            items.push(row::OverflowItem::control(
                CHOICE_W,
                false,
                move |_, _, _| {
                    icon_choice_menu(
                        "filter-size",
                        "toolbar.filter_strength",
                        editing_diameter,
                        &[
                            (
                                false,
                                "toolbar.filter_strength",
                                OptionIcon::BuiltIn(IconName::Settings2),
                            ),
                            (
                                true,
                                "toolbar.filter_diameter",
                                OptionIcon::BuiltIn(IconName::Circle),
                            ),
                        ],
                        &s,
                        &f,
                        |a, diameter| a.edit_filter_brush_size(diameter),
                    )
                    .into_any_element()
                },
            ));
        }
        if let Some(sc) = size_slider.clone() {
            items.push(row::OverflowItem::control(
                SIZE_SLIDER_W,
                false,
                move |_, _, _| size_control(&sc, "").into_any_element(),
            ));
        }
        return items;
    }

    if matches!(edit_kind, Some(ShapeKind::Rectangle | ShapeKind::Ellipse)) {
        let mode = annotations.geometry_fill_mode();
        let s = session.clone();
        let f = settings_focus.clone();
        items.push(row::OverflowItem::control(
            CHOICE_W,
            false,
            move |_, _, _| super::line_options::geometry_fill_menu(mode, &s, &f).into_any_element(),
        ));
    }
    if edit_kind == Some(ShapeKind::Highlighter) {
        let s = session.clone();
        let f = settings_focus.clone();
        items.push(row::OverflowItem::control(
            CHOICE_W * 2. + BTN_GAP,
            false,
            move |_, _, cx| {
                super::line_options::highlighter_menus(s.read(cx).annotations(), &s, &f)
                    .into_any_element()
            },
        ));
    }
    if edit_kind == Some(ShapeKind::Pencil) {
        let angle = annotations.pencil_angle();
        let s = session.clone();
        let f = settings_focus.clone();
        items.push(row::OverflowItem::control(
            CHOICE_W,
            false,
            move |_, _, _| super::line_options::pencil_angle_menu(angle, &s, &f).into_any_element(),
        ));
        let smooth = annotations.pencil_smooth();
        let s = session.clone();
        let f = settings_focus.clone();
        items.push(row::OverflowItem::control(
            BTN_SIZE,
            false,
            move |_, _, _| {
                let session = s.clone();
                control(
                    "pencil-smooth".into(),
                    crate::i18n::t!("toolbar.pencil_smooth").into_owned(),
                    smooth,
                    f.clone(),
                    move |_, cx| {
                        session.update(cx, |s, cx| {
                            s.edit_annotations(|a| a.set_pencil_smooth(!smooth));
                            cx.notify();
                        });
                    },
                )
                .child(menu_icon_colored(
                    IconName::Sparkles,
                    18.,
                    icon_tint(smooth),
                ))
                .into_any_element()
            },
        ));
    }
    if edit_kind == Some(ShapeKind::Text) {
        super::text_options::append_items(annotations, session, settings_focus, &mut items);
    }
    if edit_kind == Some(ShapeKind::Number) {
        let active = annotations.number_fill();
        {
            let s = session.clone();
            let f = settings_focus.clone();
            items.push(row::OverflowItem::control(BTN_SIZE, false, move |_, _, _| {
                let fill_session = s.clone();
                control(
                    "number-fill".into(),
                    crate::i18n::t!("toolbar.number_fill").into_owned(),
                    active,
                    f.clone(),
                    move |_, cx| {
                        fill_session.update(cx, |s, cx| {
                            s.edit_annotations(|a| a.set_number_fill(!active));
                            cx.notify();
                        });
                    },
                )
                .child(
                    svg()
                        .data(if active {
                            b"<svg xmlns='http://www.w3.org/2000/svg' width='18' height='18' viewBox='0 0 18 18'><path fill-rule='evenodd' d='M9 2A7 7 0 1 1 9 16A7 7 0 1 1 9 2ZM9.75 5.6L8.65 5.6L6.9 6.8L6.9 8.05L8.35 7.1L8.35 12.5L9.75 12.5Z'/></svg>" as &[u8]
                        } else {
                            b"<svg xmlns='http://www.w3.org/2000/svg' width='18' height='18' viewBox='0 0 18 18'><circle cx='9' cy='9' r='6.25' fill='none' stroke='black' stroke-width='1.5'/><path d='M9.75 5.6H8.65L6.9 6.8V8.05L8.35 7.1V12.5H9.75Z'/></svg>"
                        })
                        .size(px(18.))
                        .text_color(rgba(icon_tint(active))),
                )
                .into_any_element()
            }));
        }
        {
            let format = annotations.number_format();
            let s = session.clone();
            let f = settings_focus.clone();
            items.push(row::OverflowItem::control(
                CHOICE_W,
                false,
                move |_, _, _| super::line_options::number_menu(format, &s, &f).into_any_element(),
            ));
        }
        for (up, icon, key) in [
            (false, IconName::Minus, "toolbar.number_decrease"),
            (true, IconName::Plus, "toolbar.number_increase"),
        ] {
            let s = session.clone();
            let f = settings_focus.clone();
            items.push(row::OverflowItem::control(
                BTN_SIZE,
                false,
                move |_, _, _| {
                    let session = s.clone();
                    control(
                        key.into(),
                        crate::i18n::t!(key).into_owned(),
                        false,
                        f.clone(),
                        move |_, cx| {
                            session.update(cx, |s, cx| {
                                s.edit_annotations(|a| a.step_number_value(up));
                                cx.notify();
                            });
                        },
                    )
                    .child(menu_icon(icon, 18.))
                    .into_any_element()
                },
            ));
        }
        {
            let s = session.clone();
            items.push(row::OverflowItem::control(80., false, move |_, _, _| {
                super::number_options::NumberValue::new(&s).into_any_element()
            }));
        }
    }
    if matches!(edit_kind, Some(ShapeKind::Line | ShapeKind::Polyline)) {
        let ends = annotations.line_ends();
        let s = session.clone();
        let f = settings_focus.clone();
        items.push(row::OverflowItem::control(
            CHOICE_W * 2. + BTN_GAP,
            false,
            move |_, _, _| super::line_options::line_end_menus(ends, &s, &f).into_any_element(),
        ));
    }
    if matches!(edit_kind, Some(ShapeKind::Arrow | ShapeKind::Number)) {
        let arrow = annotations.arrow_style();
        let s = session.clone();
        let f = settings_focus.clone();
        items.push(row::OverflowItem::control(
            CHOICE_W,
            false,
            move |_, _, _| super::line_options::arrow_menu(arrow, &s, &f).into_any_element(),
        ));
    }
    if matches!(
        edit_kind,
        Some(
            crate::annotation::ShapeKind::Line
                | crate::annotation::ShapeKind::Polyline
                | crate::annotation::ShapeKind::Arrow
                | crate::annotation::ShapeKind::Rectangle
                | crate::annotation::ShapeKind::Ellipse
        )
    ) && (edit_kind != Some(ShapeKind::Arrow) || annotations.arrow_style().supports_line_style())
    {
        let line = annotations.line_style();
        let s = session.clone();
        let f = settings_focus.clone();
        items.push(row::OverflowItem::control(
            CHOICE_W,
            false,
            move |_, _, _| super::line_options::line_menu(line, &s, &f).into_any_element(),
        ));
    }

    if edit_kind != Some(ShapeKind::Text)
        && let Some(sc) = size_slider.clone()
    {
        items.push(row::OverflowItem::control(
            SIZE_SLIDER_W,
            false,
            move |_, _, _| {
                div()
                    .id("tb-primary-size")
                    .debug_selector(|| "tb-primary-size".into())
                    .when(
                        matches!(edit_kind, Some(ShapeKind::Rectangle | ShapeKind::Ellipse)),
                        |row| {
                            row.tooltip(|_, cx| {
                                cx.new(|_| {
                                    ToolbarTooltip(
                                        crate::i18n::t!("toolbar.line_width").into_owned().into(),
                                    )
                                })
                                .into()
                            })
                        },
                    )
                    .child(size_control(&sc, ""))
                    .into_any_element()
            },
        ));
    }
    if edit_kind == Some(ShapeKind::Rectangle) {
        let s = session.clone();
        let f = settings_focus.clone();
        items.push(row::OverflowItem::control(54., false, move |_, _, _| {
            super::geometry_options::CornerRadius::new(&s, &f).into_any_element()
        }));
    }
    if eraser_tool {
        return items;
    }

    items.push(row::OverflowItem::separator());
    let active_ix = annotations.active_swatch_index();
    for (ix, color) in crate::model::settings::current()
        .annotation_colors()
        .iter()
        .copied()
        .enumerate()
    {
        let name = crate::model::settings::PALETTE_NAMES[ix];
        if active_ix == Some(ix)
            && let Some(picker) = swatch_picker.clone()
        {
            items.push(row::OverflowItem::control(
                SWATCH_W,
                false,
                move |_, _, _| {
                    ColorPicker::new(
                        &picker,
                        crate::i18n::t!("toolbar.color_edit_aria", name = name).to_string(),
                    )
                    .opaque()
                    .selected(true)
                    .into_any_element()
                },
            ));
            continue;
        }
        let s = session.clone();
        let f = settings_focus.clone();
        let is_selected = selected_color == color;
        items.push(row::OverflowItem::control(
            SWATCH_W,
            false,
            move |popup, _, _| {
                let session = s.clone();
                let focus = f.clone();
                let popup = popup.clone();
                let t = theme::c();
                let selector = format!("tb-color-{ix}");
                let label = crate::i18n::t!("toolbar.color_label", name = name).to_string();
                let tooltip: SharedString = label.clone().into();
                Button::new(SharedString::from(selector.clone()))
                    .debug_selector(move || selector)
                    .accessibility_label(label)
                    .tooltip(move |_, cx| cx.new(|_| ToolbarTooltip(tooltip.clone())).into())
                    .on_click(move |_, window, cx| {
                        if let Some(popup) = &popup {
                            popup.update(cx, |state, cx| state.dismiss(window, cx));
                        }
                        window.focus(&focus, cx);
                        session.update(cx, |s, cx| {
                            s.edit_annotation_settings(|a| a.set_color(ix));
                            cx.notify();
                        });
                    })
                    .size(px(28.))
                    .flex_shrink_0()
                    .rounded_full()
                    .border_2()
                    .border_color(if is_selected {
                        rgba(t.accent)
                    } else {
                        rgba(0x00000000)
                    })
                    .hover(move |s| {
                        s.border_color(if is_selected {
                            rgba(t.accent)
                        } else {
                            rgba(theme::alpha(t.toolbar_text, 0x66))
                        })
                    })
                    .focus_visible(move |s| s.border_color(rgba(t.accent)))
                    .child(
                        div()
                            .size(px(18.))
                            .rounded_full()
                            .bg(rgba(color))
                            .border_1()
                            .border_color(rgba(t.swatch_border)),
                    )
                    .into_any_element()
            },
        ));
    }
    items
}

#[derive(Clone, Copy)]
enum OptionIcon {
    BuiltIn(IconName),
    Mosaic,
}

fn option_icon(icon: OptionIcon, color: u32) -> Svg {
    match icon {
        OptionIcon::BuiltIn(icon) => menu_icon_colored(icon, 18., color),
        OptionIcon::Mosaic => own_icon_colored("icons/mosaic.svg", color),
    }
}

fn icon_choice_menu<T: Copy + PartialEq + 'static>(
    id: &'static str,
    label: &'static str,
    value: T,
    choices: &'static [(T, &'static str, OptionIcon)],
    session: &Entity<ScreenshotSession>,
    focus: &FocusHandle,
    apply: fn(&mut crate::annotation::Annotations, T),
) -> impl IntoElement {
    let session = session.clone();
    choice_menu(
        id,
        label,
        value,
        choices,
        focus,
        option_icon,
        move |value, cx| {
            session.update(cx, |s, cx| {
                s.edit_annotations(|a| apply(a, value));
                cx.notify();
            });
        },
    )
}

fn shape_menu(
    ellipse: bool,
    spotlight: bool,
    session: &Entity<ScreenshotSession>,
    focus: &FocusHandle,
) -> impl IntoElement {
    icon_choice_menu(
        if spotlight {
            "spotlight-shape"
        } else {
            "lens-shape"
        },
        "toolbar.region_shape",
        ellipse,
        &[
            (
                false,
                "toolbar.lens_rectangle",
                OptionIcon::BuiltIn(IconName::Square),
            ),
            (
                true,
                "toolbar.lens_ellipse",
                OptionIcon::BuiltIn(IconName::Circle),
            ),
        ],
        session,
        focus,
        if spotlight {
            |a, value| a.set_spotlight_ellipse(value)
        } else {
            |a, value| a.set_lens_option(crate::annotation::LensOption::Ellipse, value)
        },
    )
}

/// Raw SVG elements require an explicit paint color; button text color is not inherited.
pub(super) fn menu_icon(icon: IconName, size: f32) -> Svg {
    menu_icon_colored(icon, size, theme::c().toolbar_text)
}

pub(super) fn menu_icon_colored(icon: IconName, size: f32, color: u32) -> Svg {
    svg()
        .path(icon.path())
        .size(px(size))
        .text_color(rgba(color))
}

#[cfg(test)]
mod tests {
    use super::{IconName, ToolbarSource, menu_icon};
    use gpui_kit::Styled;

    #[test]
    fn menu_icons_have_assets_and_an_explicit_visible_paint_color() {
        use gpui_kit::AssetSource;

        for name in [
            IconName::Circle,
            IconName::Square,
            IconName::Settings2,
            IconName::ChevronDown,
            IconName::Ellipsis,
            IconName::Sparkles,
            IconName::Pencil,
            IconName::Slash,
            IconName::Plus,
            IconName::Minus,
            IconName::Bold,
            IconName::Italic,
            IconName::TypeOutline,
            IconName::SquareDashed,
            IconName::MirrorRectangular,
        ] {
            assert!(ToolbarSource.load(name.path().as_ref()).unwrap().is_some());
            let mut icon = menu_icon(name, 18.);
            let color = icon
                .style()
                .text
                .color
                .expect("SVG paint requires its own color");
            assert!(color.a > 0., "menu icon must not be transparent");
        }
    }
}
