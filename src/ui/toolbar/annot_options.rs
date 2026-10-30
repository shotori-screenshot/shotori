//! The annotation settings row — the second toolbar row that appears
//! when a shape tool is active. Split from toolbar.rs: it reacts to the
//! SELECTED shape's kind (filter presets / eraser styles / size /
//! palette), not to the toolbar's own state, and it was a 185-line
//! closure buried mid-chain in `selection_toolbar`.

use super::*;
use crate::annotation::ShapeKind;
use gpui_kit::prelude::FluentBuilder as _;

/// Builds the settings row for the active `edit_kind`. `session` and
/// `settings_focus` are shared with the caller's chain; controls clone
/// what their closures capture.
pub(super) fn annot_options(
    edit_kind: Option<crate::annotation::ShapeKind>,
    annotations: &crate::annotation::Annotations,
    session: &Entity<ScreenshotSession>,
    settings_focus: &FocusHandle,
    size_slider: &Option<SizeSlider>,
    swatch_picker: &Option<Entity<ColorPickerState>>,
    selected_color: u32,
) -> Div {
    let filter_tool = matches!(
        edit_kind,
        Some(crate::annotation::ShapeKind::Mosaic | crate::annotation::ShapeKind::Blur)
    );
    let eraser_tool = matches!(
        edit_kind,
        Some(crate::annotation::ShapeKind::Eraser | crate::annotation::ShapeKind::EraserRect)
    );
    // Settings rows hug their content like every other row — no
    // fixed widths. The filter/eraser rows once carried `.w()`
    // tuned for the old S/M/L preset buttons; when the slider
    // bundle (wider than the three buttons) replaced them, the
    // readout silently spilled past the painted border — near-black
    // on near-black, only obvious on a HiDPI screen (user-reported).
    let mut options = bar().debug_selector(|| "tb-options".to_string());
    if edit_kind == Some(crate::annotation::ShapeKind::Spotlight) {
        let ellipse = annotations.spotlight_ellipse();
        options = options.child(shape_menu(ellipse, true, session, settings_focus));
        if let Some(sc) = size_slider {
            options = options
                .child(
                    div()
                        .text_size(px(12.))
                        .child(crate::i18n::t!("toolbar.spotlight_opacity").into_owned()),
                )
                .child(size_control(sc, ""))
                .child("%");
        }
        return options.child(super::spotlight_options::SpotlightOptions {
            session: session.clone(),
            focus: settings_focus.clone(),
            color: swatch_picker.clone(),
        });
    }
    if edit_kind == Some(crate::annotation::ShapeKind::Magnifier) {
        use crate::annotation::LensOption;
        let (ellipse, smooth, marks, connector) = annotations.lens_options();
        options = options.child(shape_menu(ellipse, false, session, settings_focus));
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
            let session = session.clone();
            options = options.child(
                control(
                    key.into(),
                    crate::i18n::t!(key).into_owned(),
                    settings_focus.clone(),
                    move |_, cx| {
                        session.update(cx, |s, cx| {
                            s.edit_annotations(|a| a.set_lens_option(option, !active));
                            cx.notify();
                        });
                    },
                )
                .selected(active)
                .child(if matches!(option, LensOption::Smooth) {
                    svg()
                        .data(b"<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 18 18'><g fill='none' stroke='black' stroke-width='1.5' stroke-linecap='round' stroke-linejoin='round'><path d='M2 12V8H6V4H10'/><path d='M5 15L15 5'/></g></svg>")
                        .size(px(18.))
                        .text_color(rgba(theme::c().toolbar_text))
                        .into_any_element()
                } else {
                    menu_icon(icon, 18.).into_any_element()
                }),
            );
        }
        options = options.child(super::line_options::connector_menu(
            connector,
            session,
            settings_focus,
        ));
        let shadow = annotations.lens_shadow();
        let shadow_session = session.clone();
        let shadow_icon = svg()
            .data(b"<svg xmlns='http://www.w3.org/2000/svg' width='18' height='18'><g fill='none' stroke='black' stroke-width='1.5' stroke-linecap='round' stroke-linejoin='round'><rect x='3' y='2' width='11' height='10' rx='1.5'/><path d='M5 15H15' opacity='.5'/><path d='M7 17H16' opacity='.25'/></g></svg>")
            .size(px(18.))
            .text_color(rgba(theme::c().toolbar_text));
        options = options.child(
            control(
                "lens-shadow".into(),
                crate::i18n::t!("toolbar.lens_shadow").into_owned(),
                settings_focus.clone(),
                move |_, cx| {
                    shadow_session.update(cx, |s, cx| {
                        s.edit_annotations(|a| a.set_lens_option(LensOption::Shadow, !shadow));
                        cx.notify();
                    });
                },
            )
            .selected(shadow)
            .child(shadow_icon),
        );
        if let Some(sc) = size_slider {
            options = options.child(size_control(sc, "×"));
        }
        if let Some(picker) = swatch_picker {
            options = options.child(separator()).child(ColorPicker::new(
                picker,
                crate::i18n::t!("toolbar.lens_color").into_owned(),
            ));
        }
        return options;
    }
    if filter_tool {
        options = options.child(icon_choice_menu(
            "filter-effect",
            "toolbar.filter_mosaic",
            edit_kind.unwrap_or(ShapeKind::Mosaic),
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
            session,
            settings_focus,
            |a, kind| a.set_filter_effect(kind),
        ));
        let brush = matches!(
            annotations.filter_area(),
            crate::annotation::FilterArea::Brush { .. }
        );
        options = options.child(icon_choice_menu(
            "filter-area",
            "toolbar.region_shape",
            brush,
            &[
                (
                    false,
                    "toolbar.filter_rectangle",
                    OptionIcon::BuiltIn(IconName::Square),
                ),
                (
                    true,
                    "toolbar.filter_brush",
                    OptionIcon::BuiltIn(IconName::Pencil),
                ),
            ],
            session,
            settings_focus,
            |a, brush| a.set_filter_area(brush),
        ));
        if brush {
            options = options.child(icon_choice_menu(
                "filter-size",
                "toolbar.filter_strength",
                annotations.editing_filter_brush_size(),
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
                session,
                settings_focus,
                |a, diameter| a.edit_filter_brush_size(diameter),
            ));
        }
        if let Some(sc) = size_slider {
            options = options.child(size_control(sc, ""));
        }
        return options;
    }
    if eraser_tool {
        options = options.child(icon_choice_menu(
            "eraser-mode",
            "toolbar.eraser_brush",
            edit_kind.unwrap_or(ShapeKind::Eraser),
            &[
                (
                    ShapeKind::Eraser,
                    "toolbar.eraser_brush",
                    OptionIcon::BuiltIn(IconName::Eraser),
                ),
                (
                    ShapeKind::EraserRect,
                    "toolbar.eraser_rect",
                    OptionIcon::BuiltIn(IconName::Square),
                ),
            ],
            session,
            settings_focus,
            |a, kind| {
                if a.tool() != Some(kind) {
                    a.toggle(kind);
                }
            },
        ));
        if edit_kind == Some(ShapeKind::EraserRect) {
            return options;
        }
        options = options.child(separator());
    }

    if matches!(edit_kind, Some(ShapeKind::Rectangle | ShapeKind::Ellipse)) {
        let active = annotations.geometry_fill();
        let fill_session = session.clone();
        options = options.child(
            control(
                "geometry-fill".into(),
                crate::i18n::t!("toolbar.geometry_fill").into_owned(),
                settings_focus.clone(),
                move |_, cx| {
                    fill_session.update(cx, |s, cx| {
                        s.edit_annotations(|a| a.set_geometry_fill(!active));
                        cx.notify();
                    });
                },
            )
            .selected(active)
            .child(
                svg()
                    .data(b"<svg xmlns='http://www.w3.org/2000/svg' width='18' height='18'><rect x='3' y='3' width='12' height='12' rx='1'/></svg>")
                    .size(px(18.))
                    .text_color(rgba(theme::c().toolbar_text)),
            ),
        );
    }
    if edit_kind == Some(ShapeKind::Highlighter) {
        options = options.child(super::line_options::highlighter_menus(
            annotations,
            session,
            settings_focus,
        ));
    }
    if edit_kind == Some(ShapeKind::Pencil) {
        options = options.child(super::line_options::pencil_angle_menu(
            annotations.pencil_angle(),
            session,
            settings_focus,
        ));
        let smooth = annotations.pencil_smooth();
        let session = session.clone();
        options = options.child(
            control(
                "pencil-smooth".into(),
                crate::i18n::t!("toolbar.pencil_smooth").into_owned(),
                settings_focus.clone(),
                move |_, cx| {
                    session.update(cx, |s, cx| {
                        s.edit_annotations(|a| a.set_pencil_smooth(!smooth));
                        cx.notify();
                    });
                },
            )
            .selected(smooth)
            .child(menu_icon(IconName::Sparkles, 18.)),
        );
    }
    if edit_kind == Some(ShapeKind::Text) {
        options = options.child(super::text_options::TextOptions::new(
            session,
            settings_focus,
        ));
    }
    if edit_kind == Some(ShapeKind::Number) {
        options = options.child(super::line_options::number_menu(
            annotations.number_format(),
            session,
            settings_focus,
        ));
        for (up, icon, key) in [
            (false, IconName::Minus, "toolbar.number_decrease"),
            (true, IconName::Plus, "toolbar.number_increase"),
        ] {
            let session = session.clone();
            options = options.child(
                control(
                    key.into(),
                    crate::i18n::t!(key).into_owned(),
                    settings_focus.clone(),
                    move |_, cx| {
                        session.update(cx, |s, cx| {
                            s.edit_annotations(|a| a.step_number_value(up));
                            cx.notify();
                        });
                    },
                )
                .child(menu_icon(icon, 18.)),
            );
        }
        options = options.child(super::number_options::NumberValue::new(session));
    }
    if matches!(edit_kind, Some(ShapeKind::Line | ShapeKind::Polyline)) {
        options = options.child(super::line_options::line_end_menus(
            annotations.line_ends(),
            session,
            settings_focus,
        ));
    }
    if matches!(edit_kind, Some(ShapeKind::Arrow | ShapeKind::Number)) {
        options = options.child(super::line_options::arrow_menu(
            annotations.arrow_style(),
            session,
            settings_focus,
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
        options = options.child(super::line_options::line_menu(
            annotations.line_style(),
            session,
            settings_focus,
        ));
    }

    if edit_kind != Some(ShapeKind::Text)
        && let Some(sc) = &size_slider
    {
        options = options.child(
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
                .child(size_control(sc, "")),
        );
    }
    if edit_kind == Some(ShapeKind::Rectangle) {
        options = options.child(super::geometry_options::CornerRadius::new(
            session,
            settings_focus,
        ));
    }
    if eraser_tool {
        return options;
    }
    options = options.child(separator()).child(div().flex_1());
    let active_ix = annotations.color_index();
    for (ix, color) in crate::model::settings::current()
        .annotation_colors()
        .iter()
        .copied()
        .enumerate()
    {
        let name = crate::model::settings::PALETTE_NAMES[ix];
        if ix == active_ix
            && let Some(picker) = &swatch_picker
        {
            // The SELECTED swatch becomes the inline color
            // editor: clicking it again opens the picker's
            // popover (palette + HSLA sliders + hex). Edits
            // land in settings.toml through
            // `settings::set_annotation_color` (the Overlay
            // owns the state and the subscription — see its
            // `swatch_picker`). Non-selected swatches stay
            // plain one-click selectors, so "pick a color" and
            // "edit a color" never fight over the same click.
            options = options.child(
                ColorPicker::new(
                    picker,
                    crate::i18n::t!("toolbar.color_edit_aria", name = name).to_string(),
                )
                .selected(true),
            );
            continue;
        }
        let session = session.clone();
        options = options.child(
            control(
                format!("tb-color-{ix}"),
                crate::i18n::t!("toolbar.color_label", name = name).to_string(),
                settings_focus.clone(),
                move |_, cx| {
                    session.update(cx, |s, cx| {
                        s.edit_annotation_settings(|a| a.set_color(ix));
                        cx.notify();
                    })
                },
            )
            .w(px(28.))
            .selected(selected_color == color)
            .child(
                div()
                    .size(px(20.))
                    .rounded(px(4.))
                    .bg(rgba(color))
                    .border_1()
                    .border_color(rgba(theme::c().swatch_border)),
            ),
        );
    }
    options
}

#[derive(Clone, Copy)]
enum OptionIcon {
    BuiltIn(IconName),
    Mosaic,
}

fn option_icon(icon: OptionIcon) -> Svg {
    match icon {
        OptionIcon::BuiltIn(icon) => menu_icon(icon, 18.),
        OptionIcon::Mosaic => own_icon("icons/mosaic.svg"),
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
    svg()
        .path(icon.path())
        .size(px(size))
        .text_color(rgba(theme::c().toolbar_text))
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
