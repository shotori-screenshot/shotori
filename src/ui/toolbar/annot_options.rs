//! The annotation settings row — the second toolbar row that appears
//! when a shape tool is active. Split from toolbar.rs: it reacts to the
//! SELECTED shape's kind (filter presets / eraser styles / size /
//! palette), not to the toolbar's own state, and it was a 185-line
//! closure buried mid-chain in `selection_toolbar`.

use super::*;

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
    if filter_tool {
        for (id, label, kind) in [
            (
                "tb-pixelate",
                crate::i18n::t!("toolbar.filter_mosaic"),
                crate::annotation::ShapeKind::Mosaic,
            ),
            (
                "tb-blur",
                crate::i18n::t!("toolbar.filter_blur"),
                crate::annotation::ShapeKind::Blur,
            ),
        ] {
            let session = session.clone();
            options = options.child(
                control(
                    id.into(),
                    label.into(),
                    settings_focus.clone(),
                    move |_, cx| {
                        session.update(cx, |s, cx| {
                            s.edit_annotations(|a| {
                                if a.tool() != Some(kind) {
                                    a.toggle(kind);
                                }
                            });
                            cx.notify();
                        });
                    },
                )
                .selected(edit_kind == Some(kind))
                .child(if kind == crate::annotation::ShapeKind::Mosaic {
                    own_icon("icons/mosaic.svg")
                } else {
                    svg()
                        .path(IconName::MirrorRectangular.path())
                        .size(px(18.))
                        .text_color(rgba(theme::c().toolbar_text))
                }),
            );
        }
        if let Some(sc) = &size_slider {
            options = options.child(separator()).child(size_control(sc));
        }
        return options;
    }

    if eraser_tool {
        for (id, label, kind, icon) in [
            (
                "tb-eraser-brush",
                crate::i18n::t!("toolbar.eraser_brush"),
                crate::annotation::ShapeKind::Eraser,
                IconName::Eraser,
            ),
            (
                "tb-eraser-rect",
                crate::i18n::t!("toolbar.eraser_rect"),
                crate::annotation::ShapeKind::EraserRect,
                IconName::Square,
            ),
        ] {
            let session = session.clone();
            options = options.child(
                control(
                    id.into(),
                    label.into(),
                    settings_focus.clone(),
                    move |_, cx| {
                        session.update(cx, |s, cx| {
                            s.edit_annotations(|a| {
                                if a.tool() != Some(kind) {
                                    a.toggle(kind);
                                }
                            });
                            cx.notify();
                        });
                    },
                )
                .selected(edit_kind == Some(kind))
                .child(
                    svg()
                        .path(icon.path())
                        .size(px(18.))
                        .text_color(rgba(theme::c().toolbar_text)),
                ),
            );
        }
        if edit_kind == Some(crate::annotation::ShapeKind::EraserRect) {
            return options;
        }
        options = options.child(separator());
    }

    if let Some(sc) = &size_slider {
        options = options.child(size_control(sc));
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
