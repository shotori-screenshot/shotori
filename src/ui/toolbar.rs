//! # Selection toolbar: actions + annotation tool controls
//!
//! Buttons dispatch the exact same actions as the keyboard through
//! `dispatch_action` — one action, two triggers, one pipeline. Row two
//! (annotation tools, colors, widths) only appears while a tool is
//! active. Geometry lives in [`crate::model::placement`].
use gpui_kit::{
    assets::IconName,
    base::{
        Button,
        slider::{Slider, SliderIndicator, SliderState, SliderThumb, SliderTrack},
    },
    component::Sizable,
    component::color_picker::{ColorPicker, ColorPickerState},
    *,
};

use crate::model::session::ScreenshotSession;
use crate::ui::theme;

use crate::actions::{
    ClearAnnotations, CopySelection, OcrSelection, PinSelection, QuitOverlay, SaveSelection,
    ScrollSelection, ToggleArrow, ToggleEllipse, ToggleEraser, ToggleHighlighter, ToggleLine,
    ToggleMosaic, ToggleNumber, TogglePencil, TogglePolyline, ToggleRectangle, ToggleSelect,
    ToggleText,
};
use crate::model::placement::{GRIP_W, ROW_H};

// The default GPUI asset bundle does not include every toolbar icon.
gpui_kit::assets::icon_assets!(
    ToolbarAssets,
    [
        Eraser,
        Type,
        MirrorRectangular,
        Highlighter,
        Pencil,
        Square,
        Circle,
        Slash,
        Waypoints,
        ArrowUpRight,
        ListOrdered,
        ScanText,
        Save,
        Pin,
        Trash,
        X,
        Copy,
        Settings,
        Settings2,
        PanelTop,
        Keyboard,
        Palette,
        Info,
        PanelLeftOpen,
        PanelLeftClose,
        // The settings title bar's own control cluster (it draws one
        // when the compositor claims server decorations but draws none,
        // e.g. niri's `prefer-no-csd`; the component's cluster is
        // suppressed in that mode).
        WindowMinimize,
        WindowMaximize,
        WindowRestore,
        WindowClose
    ]
);

/// The app's own icon set, embedded from `assets/icons` at build time.
/// The Lucide catalog has no true mosaic/pixelate glyph (its grids read
/// as "table"), so shotori maintains its own SVGs — Lucide conventions
/// kept (24×24 canvas, currentColor) so they tint with the toolbar text
/// color like every bundled icon.
#[derive(rust_embed::RustEmbed)]
#[folder = "assets"]
#[include = "icons/*.svg"]
struct OwnIcons;

/// The application asset source: the app's own icons first, then the
/// selected Lucide icons (`icon_assets!` selects from the gpui-kit
/// bundle — see the crate docs for the composition contract). Registered
/// app-wide in `main.rs` via `with_assets`.
#[derive(Clone, Copy, Debug, Default)]
pub struct ToolbarSource;

impl AssetSource for ToolbarSource {
    fn load(&self, path: &str) -> anyhow::Result<Option<std::borrow::Cow<'static, [u8]>>> {
        if let Some(file) = OwnIcons::get(path) {
            return Ok(Some(file.data));
        }
        ToolbarAssets.load(path)
    }

    fn list(&self, path: &str) -> anyhow::Result<Vec<SharedString>> {
        let mut names: Vec<SharedString> = OwnIcons::iter()
            .filter(|name| name.starts_with(path))
            .map(Into::into)
            .collect();
        names.extend(ToolbarAssets.list(path)?);
        Ok(names)
    }
}

/// One of the app's own icons (see [`OwnIcons`]).
fn own_icon(path: &'static str) -> Svg {
    svg()
        .path(path)
        .size(px(18.))
        .text_color(rgba(theme::c().toolbar_text))
}

/// The active tool's size-slider bundle. The overlay owns the
/// SliderState entity (rebuilt per tool spec) and passes it down;
/// this module only renders.
pub(crate) struct SizeSlider {
    pub(crate) state: Entity<SliderState>,
    /// The value's 0..1 position on the track, for the thumb.
    pub(crate) percentage: f32,
    pub(crate) current: f32,
}

/// Where a row-one button's glyph comes from: the bundled Lucide
/// whitelist, or shotori's own `assets/icons` SVG for the concepts
/// Lucide has no glyph for (see [`OwnIcons`]).
#[derive(Clone, Copy)]
pub(crate) enum ButtonIcon {
    Kit(IconName),
    Own(&'static str),
}

impl ButtonIcon {
    pub(crate) fn svg(self) -> Svg {
        match self {
            Self::Kit(name) => svg().path(name.path()),
            Self::Own(path) => svg().path(path),
        }
        .size(px(18.))
        .text_color(rgba(theme::c().toolbar_text))
    }
}

/// Row-one button metadata: ONE source for the bar's own construction
/// and the settings window's editor list — ids, labels, shortcuts and
/// icons cannot drift between the two views of the same toolbar.
/// Order is the display order (tool cluster, then actions, then the
/// confirm pair).
pub(crate) struct ButtonMeta {
    pub(crate) button: crate::model::settings::ToolbarButton,
    /// Element/debug id (`tb-*`) — e2e probes rely on these.
    pub(crate) debug_id: &'static str,
    /// Translation key (`toolbar.*` in locales/ui.yml). Labels are
    /// locale-dependent; the key (and `debug_id`) stay stable.
    pub(crate) label_key: &'static str,
    /// Shortcut hint ("" when the button has none).
    pub(crate) shortcut: &'static str,
    pub(crate) icon: ButtonIcon,
}

pub(crate) const BUTTONS: [ButtonMeta; crate::model::settings::ToolbarButton::ALL.len()] = [
    ButtonMeta {
        // The select tool — first, the Figma position: selection is a
        // mode you enter, never a side effect of drawing (2026-10-06
        // flip). Own icon: Lucide's mouse-pointer isn't in
        // gpui-kit-assets' compiled set.
        button: crate::model::settings::ToolbarButton::Select,
        debug_id: "tb-select",
        label_key: "toolbar.select",
        shortcut: "V",
        icon: ButtonIcon::Own("icons/select.svg"),
    },
    ButtonMeta {
        button: crate::model::settings::ToolbarButton::Rectangle,
        debug_id: "tb-rectangle",
        label_key: "toolbar.rectangle",
        shortcut: "R",
        icon: ButtonIcon::Kit(IconName::Square),
    },
    ButtonMeta {
        button: crate::model::settings::ToolbarButton::Ellipse,
        debug_id: "tb-ellipse",
        label_key: "toolbar.ellipse",
        shortcut: "E",
        icon: ButtonIcon::Kit(IconName::Circle),
    },
    ButtonMeta {
        button: crate::model::settings::ToolbarButton::Line,
        debug_id: "tb-line",
        label_key: "toolbar.line",
        shortcut: "L",
        icon: ButtonIcon::Kit(IconName::Slash),
    },
    ButtonMeta {
        button: crate::model::settings::ToolbarButton::Polyline,
        debug_id: "tb-polyline",
        label_key: "toolbar.polyline",
        shortcut: "P",
        icon: ButtonIcon::Kit(IconName::Waypoints),
    },
    ButtonMeta {
        button: crate::model::settings::ToolbarButton::Arrow,
        debug_id: "tb-arrow",
        label_key: "toolbar.arrow",
        shortcut: "A",
        icon: ButtonIcon::Kit(IconName::ArrowUpRight),
    },
    ButtonMeta {
        button: crate::model::settings::ToolbarButton::Number,
        debug_id: "tb-number",
        label_key: "toolbar.number",
        shortcut: "N",
        icon: ButtonIcon::Kit(IconName::ListOrdered),
    },
    ButtonMeta {
        button: crate::model::settings::ToolbarButton::Pencil,
        debug_id: "tb-pencil",
        label_key: "toolbar.pencil",
        shortcut: "B",
        icon: ButtonIcon::Kit(IconName::Pencil),
    },
    ButtonMeta {
        button: crate::model::settings::ToolbarButton::Highlighter,
        debug_id: "tb-highlighter",
        label_key: "toolbar.highlighter",
        shortcut: "H",
        icon: ButtonIcon::Kit(IconName::Highlighter),
    },
    ButtonMeta {
        button: crate::model::settings::ToolbarButton::Mosaic,
        debug_id: "tb-mosaic",
        label_key: "toolbar.mosaic",
        shortcut: "M",
        icon: ButtonIcon::Own("icons/mosaic.svg"),
    },
    ButtonMeta {
        button: crate::model::settings::ToolbarButton::Eraser,
        debug_id: "tb-eraser",
        label_key: "toolbar.eraser",
        shortcut: "D",
        icon: ButtonIcon::Kit(IconName::Eraser),
    },
    ButtonMeta {
        button: crate::model::settings::ToolbarButton::Text,
        debug_id: "tb-text",
        label_key: "toolbar.text",
        shortcut: "T",
        icon: ButtonIcon::Kit(IconName::Type),
    },
    ButtonMeta {
        // Wipes every placed mark in one undoable step (issue #15).
        // Lives at the end of the tool cluster — undo/redo are
        // keyboard-only, so this is the one history action with a
        // button. Lucide's plain Trash is the conventional "clear"
        // glyph (this bundle has no trash-2); an empty-canvas press
        // is a harmless no-op, like Delete.
        button: crate::model::settings::ToolbarButton::Clear,
        debug_id: "tb-clear",
        label_key: "toolbar.clear",
        shortcut: "Ctrl+Shift+Del",
        icon: ButtonIcon::Kit(IconName::Trash),
    },
    ButtonMeta {
        button: crate::model::settings::ToolbarButton::Ocr,
        debug_id: "tb-ocr",
        label_key: "toolbar.ocr",
        shortcut: "Ctrl+O",
        icon: ButtonIcon::Kit(IconName::ScanText),
    },
    ButtonMeta {
        // own icon: the gpui-kit asset whitelist (`icon_assets!`) does
        // not bundle a pin, and IconName::Pin would render an empty
        // slot — same situation as the mosaic icon
        button: crate::model::settings::ToolbarButton::Pin,
        debug_id: "tb-pin",
        label_key: "toolbar.pin",
        shortcut: "Ctrl+P",
        icon: ButtonIcon::Own("icons/pin.svg"),
    },
    ButtonMeta {
        // own icon: no long-screenshot glyph in the Lucide whitelist
        // either (document + down arrow)
        button: crate::model::settings::ToolbarButton::Scroll,
        debug_id: "tb-scroll",
        label_key: "toolbar.scroll",
        shortcut: "Ctrl+L",
        icon: ButtonIcon::Own("icons/scroll.svg"),
    },
    ButtonMeta {
        button: crate::model::settings::ToolbarButton::Save,
        debug_id: "tb-save",
        label_key: "toolbar.save",
        shortcut: "Ctrl+S",
        icon: ButtonIcon::Kit(IconName::Save),
    },
    ButtonMeta {
        button: crate::model::settings::ToolbarButton::Copy,
        debug_id: "tb-copy",
        label_key: "toolbar.copy",
        shortcut: "Enter / Ctrl+C",
        icon: ButtonIcon::Kit(IconName::Copy),
    },
    ButtonMeta {
        // The visible entry into the settings editor. A regular
        // configurable button like every other: hiding it is a choice
        // (the tray menu and `shotori settings` remain as backstops).
        // Lucide's classic "settings" gear — the one every desktop
        // app uses; the chunkier cog variant reads poorly at 18px
        // (user-reported).
        button: crate::model::settings::ToolbarButton::Settings,
        debug_id: "tb-settings",
        label_key: "toolbar.settings",
        shortcut: "Ctrl+,",
        icon: ButtonIcon::Kit(IconName::Settings),
    },
    ButtonMeta {
        button: crate::model::settings::ToolbarButton::Cancel,
        debug_id: "tb-cancel",
        label_key: "toolbar.cancel",
        shortcut: "Esc",
        icon: ButtonIcon::Kit(IconName::X),
    },
];

/// The action each button dispatches — the exact same actions the
/// keybindings dispatch (one action, two triggers, one pipeline).
fn action_for(button: crate::model::settings::ToolbarButton) -> Box<dyn gpui_kit::Action> {
    use crate::model::settings::ToolbarButton as B;
    match button {
        B::Select => Box::new(ToggleSelect),
        B::Rectangle => Box::new(ToggleRectangle),
        B::Ellipse => Box::new(ToggleEllipse),
        B::Line => Box::new(ToggleLine),
        B::Polyline => Box::new(TogglePolyline),
        B::Arrow => Box::new(ToggleArrow),
        B::Number => Box::new(ToggleNumber),
        B::Pencil => Box::new(TogglePencil),
        B::Highlighter => Box::new(ToggleHighlighter),
        B::Mosaic => Box::new(ToggleMosaic),
        B::Eraser => Box::new(ToggleEraser),
        B::Text => Box::new(ToggleText),
        B::Clear => Box::new(ClearAnnotations),
        B::Ocr => Box::new(OcrSelection),
        B::Save => Box::new(SaveSelection),
        B::Pin => Box::new(PinSelection),
        B::Scroll => Box::new(ScrollSelection),
        B::Cancel => Box::new(QuitOverlay),
        B::Copy => Box::new(CopySelection),
        B::Settings => Box::new(crate::actions::OpenSettings),
    }
}

/// The pressed state a button shows. Tool buttons light up from the
/// annotations state — `tool()` for the shape modes, `edit_kind()` for
/// the multi-variant ones (mosaic/eraser families, number, text); the
/// action cluster has no pressed state.
fn selected_state(
    button: crate::model::settings::ToolbarButton,
    annotations: &crate::annotation::Annotations,
) -> bool {
    use crate::annotation::ShapeKind as K;
    use crate::model::settings::ToolbarButton as B;
    let tool = annotations.tool();
    let edit = annotations.edit_kind();
    match button {
        B::Select => tool == Some(K::Select),
        B::Rectangle => tool == Some(K::Rectangle),
        B::Ellipse => tool == Some(K::Ellipse),
        B::Line => tool == Some(K::Line),
        B::Polyline => tool == Some(K::Polyline),
        B::Arrow => tool == Some(K::Arrow),
        B::Pencil => tool == Some(K::Pencil),
        B::Highlighter => tool == Some(K::Highlighter),
        B::Number => edit == Some(K::Number),
        B::Mosaic => matches!(edit, Some(K::Mosaic | K::Blur)),
        B::Eraser => matches!(edit, Some(K::Eraser | K::EraserRect)),
        B::Text => edit == Some(K::Text),
        _ => false,
    }
}

/// The metadata entry for a button ([`BUTTONS`] covers every
/// `ToolbarButton` — enforced by test). Shared with the settings
/// editor's preview bar.
pub(crate) fn meta_for(
    button: crate::model::settings::ToolbarButton,
) -> Option<&'static ButtonMeta> {
    BUTTONS.iter().find(|meta| meta.button == button)
}

impl ButtonMeta {
    /// The localized button name (settings palette, drag ghost).
    pub(crate) fn label(&self) -> std::borrow::Cow<'static, str> {
        crate::i18n::t!(self.label_key)
    }

    /// The tooltip / accessibility label ("Name · Shortcut").
    pub(crate) fn tooltip(&self) -> String {
        if self.shortcut.is_empty() {
            self.label().to_string()
        } else {
            format!("{} · {}", self.label(), self.shortcut)
        }
    }
}

/// Row one, rendered in the configured order (see
/// `model::settings::Settings::row`): presence in the row IS
/// visibility, the row's order IS the layout. The factory row
/// reproduces the historical two-cluster look through its separators.
fn row_one(
    annotations: &crate::annotation::Annotations,
    focus: &FocusHandle,
    settings: &crate::model::settings::Settings,
) -> Vec<AnyElement> {
    use crate::model::settings::ToolbarItem;
    settings
        .row()
        .iter()
        .map(|item| match item {
            ToolbarItem::Button(button) => {
                // owned: the click closure must outlive the borrowed row
                let button = *button;
                let Some(meta) = meta_for(button) else {
                    // unreachable: BUTTONS covers every ToolbarButton
                    // (button_table_matches_the_settings_vocabulary)
                    return div().into_any_element();
                };
                control(
                    meta.debug_id.into(),
                    meta.tooltip(),
                    focus.clone(),
                    move |window, cx| {
                        window.dispatch_action(action_for(button), cx);
                    },
                )
                .selected(selected_state(button, annotations))
                .child(meta.icon.svg())
                .into_any_element()
            }
            ToolbarItem::Separator => separator().into_any_element(),
        })
        .collect()
}

pub(crate) fn selection_toolbar(
    rect: Bounds<Pixels>,
    output: SharedString,
    annotations: &crate::annotation::Annotations,
    session: Entity<ScreenshotSession>,
    focus: FocusHandle,
    size_slider: Option<SizeSlider>,
    swatch_picker: Option<Entity<ColorPickerState>>,
) -> impl IntoElement {
    let edit_kind = annotations.edit_kind();
    let selected_color = annotations
        .selected()
        .map(|s| s.color)
        .unwrap_or_else(|| annotations.color().0);
    let filter_tool = matches!(
        edit_kind,
        Some(crate::annotation::ShapeKind::Mosaic | crate::annotation::ShapeKind::Blur)
    );
    let eraser_tool = matches!(
        edit_kind,
        Some(crate::annotation::ShapeKind::Eraser | crate::annotation::ShapeKind::EraserRect)
    );
    let settings_focus = focus.clone();
    // Installed once at startup (settings.toml / `shotori settings`);
    // row one below reads it, placement's width basis reads the same.
    let settings = crate::model::settings::current();

    div()
        .id("shotori-toolbar")
        .absolute()
        .left(rect.origin.x)
        .top(rect.origin.y)
        // No fixed width: each row hugs its own natural width (row one
        // ≈ TB_W_ROW1, the settings row is wider). The old fixed TB_W
        // stretched row one, and its flex_1 spacer ballooned into the
        // giant gap between tool cluster and action cluster. The width
        // basis still drives placement/clamping via `toolbar_bounds` —
        // rendering just no longer forces it onto every row.
        .flex()
        .flex_col()
        .items_start()
        .gap(px(6.))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(
            bar()
                .child(grip("tb-grip-left", output.clone(), session.clone()))
                .children(row_one(annotations, &focus, settings))
                .child(grip("tb-grip-right", output, session.clone())),
        )
        .children(edit_kind.is_some().then(|| {
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
                        "Mosaic",
                        crate::annotation::ShapeKind::Mosaic,
                    ),
                    ("tb-blur", "Blur", crate::annotation::ShapeKind::Blur),
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
                            if kind == crate::annotation::ShapeKind::Mosaic {
                                own_icon("icons/mosaic.svg")
                            } else {
                                svg()
                                    .path(IconName::MirrorRectangular.path())
                                    .size(px(18.))
                                    .text_color(rgba(theme::c().toolbar_text))
                            },
                        ),
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
                        "Brush eraser",
                        crate::annotation::ShapeKind::Eraser,
                        IconName::Eraser,
                    ),
                    (
                        "tb-eraser-rect",
                        "Rectangle eraser",
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
                    // The wrap carries the SELECTED affordance (accent
                    // ring + selected surface, same vocabulary as a
                    // lit control) — the picker's own trigger has no
                    // selected state, and without the ring the active
                    // cell reads as just another swatch
                    // (user-reported).
                    options = options.child(
                        div()
                            .flex()
                            .items_center()
                            .rounded(px(5.))
                            .border_1()
                            .border_color(rgba(theme::c().accent))
                            .bg(rgba(theme::c().toolbar_selected))
                            .child(
                                ColorPicker::new(picker)
                                    .small()
                                    .accessibility_label(format!("Edit color: {name}")),
                            ),
                    );
                    continue;
                }
                let session = session.clone();
                options = options.child(
                    control(
                        format!("tb-color-{ix}"),
                        format!("Color: {name}"),
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
        }))
}

/// gpui's 0xRRGGBBAA → picker color.
pub(crate) fn u32_to_hsla(color: u32) -> Hsla {
    rgba(color).into()
}

/// Picker color → storage. Alpha forced opaque: swatches become
/// exported PNG ink, which must be solid — the pickers cannot
/// meaningfully edit a swatch alpha.
pub(crate) fn hsla_to_u32(color: Hsla) -> u32 {
    let c: Rgba = color.into();
    let byte = |v: f32| (v.clamp(0., 1.) * 255.).round() as u32;
    (byte(c.r) << 24) | (byte(c.g) << 16) | (byte(c.b) << 8) | 0xFF
}

/// A drag strip at the toolbar's edge: press and the whole toolbar
/// follows the pointer anywhere on its layer (session-side clamping
/// keeps it inside the window). Visually a matte "grip texture" — a
/// quiet dot matrix like the textured rubber on physical devices — NOT
/// a button: no pill, no hover background. The open/closed hand cursor
/// is the affordance (see `Overlay::cursor_style`).
/// The continuous size control (issue #3, phase 2): an unstyled base
/// slider styled to the toolbar, a live value readout, and the three
/// legacy presets as clickable detents on the track. Behavior comes
/// from the base primitives (track click-to-position, thumb drag);
/// this is presentation only.
fn size_control(sc: &SizeSlider) -> Div {
    /// track length; the thumb travels TRACK_W - THUMB inside it
    const TRACK_W: f32 = 108.;
    const THUMB: f32 = 12.;
    let state = &sc.state;
    div()
        .flex()
        .items_center()
        .gap(px(8.))
        .child(
            div()
                .relative()
                .w(px(TRACK_W))
                .h(px(20.))
                // behavior root: aria + release handling across the strip
                .child(
                    Slider::new(state)
                        .horizontal()
                        .absolute()
                        .size_full()
                        .child(
                            SliderTrack::new(state)
                                .absolute()
                                .left(px(THUMB / 2.))
                                .top(px(8.))
                                .w(px(TRACK_W - THUMB))
                                .h(px(4.))
                                .rounded_full()
                                .bg(rgba(theme::c().toolbar_border))
                                // the indicator is the bounds probe the
                                // track's click-to-position maps through
                                .child(SliderIndicator::new(state).size_full()),
                        ),
                )
                .child(
                    SliderThumb::new(state)
                        .absolute()
                        .top(px(10. - THUMB / 2.))
                        .left(px(sc.percentage * (TRACK_W - THUMB)))
                        .size(px(THUMB))
                        .rounded_full()
                        .bg(rgba(theme::c().toolbar_text))
                        .border_1()
                        .border_color(rgba(theme::c().toolbar_bg)),
                ),
        )
        .child(
            div()
                .debug_selector(|| "tb-size-readout".to_string())
                .w(px(26.))
                .text_size(px(12.))
                .text_color(rgba(theme::c().toolbar_text))
                .child(format!("{:.0}", sc.current)),
        )
}

fn grip(
    id: &'static str,
    output: SharedString,
    session: Entity<ScreenshotSession>,
) -> impl IntoElement {
    let left = id == "tb-grip-left";
    // on_mouse_down and the measure canvas each own a handle — both
    // outlive this function.
    let (measure_output, measure_session) = (output.clone(), session.clone());
    div()
        .id(id)
        // probe visibility: the cursor-alignment tests assert this
        // element's laid-out rect equals `session::toolbar_grips`
        .debug_selector(move || id.to_string())
        // Full row height, and offset only by the bar's side padding —
        // the element's rect is then EXACTLY the strip the cursor
        // should grab (row one only, BAR_PAD inset, ROW_H tall): every
        // pixel that shows the hand grabs, every pixel that grabs shows
        // the hand.
        .h(px(ROW_H))
        .w(px(GRIP_W))
        .flex()
        .on_mouse_down(MouseButton::Left, move |ev, _, cx| {
            session.update(cx, |s, cx| {
                if s.toolbar_drag_begin(&output, ev.position) {
                    cx.notify();
                }
            });
            cx.stop_propagation(); // the press belongs to the toolbar, not the canvas
        })
        .child(grip_texture())
        .child(measure_grip(measure_output, measure_session, left))
}

/// The grip's measure back-channel: an invisible canvas filling the
/// grip element, reporting its LAID-OUT rect to the session at paint
/// time, so the cursor's grab strip follows the rendered grip whatever
/// the bar's real width turned out to be — not a placement-basis
/// derivation that can drift (the TB_W trap; see
/// `toolbar_grips_reported`'s comment). Absolute + size-full keeps it
/// out of the flex layout entirely. Paint runs after layout (and after
/// the render that reads the value), so a geometry change lands one
/// frame late; the derivation stays as the first-frame fallback. The
/// report only notifies on change — no repaint loop.
fn measure_grip(
    output: SharedString,
    session: Entity<ScreenshotSession>,
    left: bool,
) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |viewport, (), _, cx| {
            session.update(cx, |s, cx| {
                if s.report_toolbar_grip(&output, left, viewport) {
                    cx.notify();
                }
            });
        },
    )
    .absolute()
    .size_full()
}

/// The grip's matte dot texture — the visual half of [`grip`], reused
/// by the settings editor's preview bar (which must not drag
/// anything). Sizing belongs to the wrapper so the editor can grow it
/// into an easier grab target.
pub(crate) fn grip_texture() -> impl IntoElement {
    // toolbar_text at low alpha: visible as texture, quiet as texture
    let grain = (theme::c().toolbar_text & 0xFFFFFF00) | 0x4D;
    div()
        // gap between the dot COLUMNS — do not lose this again: without
        // it the three columns pack solid and the airy matte matrix
        // collapses into tight triple lines (user-noticed regression).
        // No justify_center either: flush-left is the exact look that
        // was approved (pixel-verified against the liked build).
        .flex()
        .gap(px(1.75))
        .items_center()
        .children((0..3).map(|_| {
            div()
                .flex()
                .flex_col()
                .gap(px(1.75))
                .children((0..7).map(|_| div().size(px(1.5)).rounded_full().bg(rgba(grain))))
        }))
}

/// A non-dispatching control for the settings editor's preview bar:
/// the exact 30px look of the real button, but the click belongs to
/// the editor (remove), never to the action itself.
pub(crate) fn preview_control(
    meta: &ButtonMeta,
    focus: &FocusHandle,
    on_click: impl Fn(&mut Window, &mut App) + 'static,
) -> Button {
    control(
        meta.debug_id.into(),
        meta.tooltip(),
        focus.clone(),
        on_click,
    )
    .child(meta.icon.svg())
}

/// The bar container — shared with the settings editor's preview.
pub(crate) fn bar() -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(2.))
        .h(px(ROW_H))
        .px(px(crate::model::placement::BAR_PAD))
        .rounded(px(8.))
        .bg(rgba(theme::c().toolbar_bg))
        .border_1()
        .border_color(rgba(theme::c().toolbar_border))
        .shadow_md()
}

/// The 1px group break — shared with the settings editor's preview.
pub(crate) fn separator() -> Div {
    div()
        .w(px(1.))
        .h(px(18.))
        .mx(px(4.))
        .bg(rgba(theme::c().toolbar_border))
}

/// Restore the canvas focus BEFORE changing toolbar state. A focused control
/// may disappear on redraw (e.g. leaving rectangle mode), otherwise scoped
/// shortcut dispatch no longer has the screenshot root in its focus path.
fn control(
    id: String,
    label: String,
    focus: FocusHandle,
    on_click: impl Fn(&mut Window, &mut App) + 'static,
) -> Button {
    let selector = id.clone();
    let tooltip: SharedString = label.clone().into();
    Button::new(SharedString::from(id))
        .debug_selector(move || selector)
        .accessibility_label(label)
        .tooltip(move |_, cx| cx.new(|_| ToolbarTooltip(tooltip.clone())).into())
        .on_click(move |_, window, cx| {
            window.focus(&focus, cx);
            on_click(window, cx);
        })
        .size(px(30.))
        .flex_shrink_0()
        .rounded(px(5.))
        .text_color(rgba(theme::c().toolbar_text))
        .hover(|s| s.bg(rgba(theme::c().toolbar_hover)))
        .focus_visible(|s| s.border_1().border_color(rgba(theme::c().accent)))
        .styles(|s| {
            s.selected(|s| {
                s.bg(rgba(theme::c().toolbar_selected))
                    .text_color(rgba(theme::c().selected_text()))
                    .border_1()
                    .border_color(rgba(theme::c().accent))
            })
        })
}

struct ToolbarTooltip(SharedString);
impl Render for ToolbarTooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_2()
            .py_1()
            .rounded(px(5.))
            .bg(rgba(theme::c().toolbar_bg))
            .border_1()
            .border_color(rgba(theme::c().toolbar_border))
            .text_size(px(12.))
            .text_color(rgba(theme::c().toolbar_text))
            .child(self.0.clone())
    }
}

#[cfg(test)]
mod tests {
    use crate::model::placement::{TB_W_ROW1, round_px, toolbar_anchor, toolbar_bounds};
    use gpui_kit::{Bounds, point, px, size};
    #[test]
    fn toolbar_clamps_horizontally() {
        let b = Bounds::new(point(px(1800.), px(100.)), size(px(100.), px(100.)));
        assert_eq!(
            toolbar_anchor(&b, size(px(1920.), px(1080.)), TB_W_ROW1, super::ROW_H).0,
            1920. - TB_W_ROW1 - 8.
        );
        // the composed rect: anchor + the width clamp the render side applies
        let rect = toolbar_bounds(
            &round_px(b),
            size(px(1920.), px(1080.)),
            TB_W_ROW1,
            super::ROW_H,
        );
        assert_eq!(rect.size.width, px(TB_W_ROW1));
        // narrow window: the toolbar shrinks to the window minus breathing room
        let rect = toolbar_bounds(
            &round_px(b),
            size(px(400.), px(400.)),
            TB_W_ROW1,
            super::ROW_H,
        );
        assert_eq!(rect.size.width, px(384.));
    }
    #[test]
    fn toolbar_icons_are_bundled() {
        use gpui_kit::AssetSource;
        // the app's composed source: own SVGs first, then selected
        // Lucide — every icon the BUTTONS table references must load,
        // a missing whitelist entry renders an empty slot
        for meta in super::BUTTONS {
            let path = match meta.icon {
                super::ButtonIcon::Kit(name) => name.path().to_string(),
                super::ButtonIcon::Own(path) => path.to_string(),
            };
            assert!(
                super::ToolbarSource.load(path.as_str()).unwrap().is_some(),
                "{} ({}) is missing from the asset source",
                path,
                meta.debug_id
            );
        }
    }

    #[test]
    fn title_bar_control_icons_are_bundled() {
        use gpui_kit::AssetSource;
        use gpui_kit::assets::IconName;
        // The settings title bar draws its own minimize/maximize/close
        // cluster when the compositor claims server decorations without
        // drawing a bar; the `IconName` constants compile even when the
        // whitelist omits them, so pin the assets here.
        for icon in [
            IconName::WindowMinimize,
            IconName::WindowMaximize,
            IconName::WindowRestore,
            IconName::WindowClose,
        ] {
            assert!(
                super::ToolbarSource.load(&icon.path()).unwrap().is_some(),
                "{:?} is missing from the asset source",
                icon
            );
        }
    }

    #[test]
    fn button_table_matches_the_settings_vocabulary() {
        // the settings window edits exactly the row the toolbar
        // renders: same buttons, same order, every entry addressable
        // by its settings id
        assert_eq!(
            super::BUTTONS
                .iter()
                .map(|meta| meta.button)
                .collect::<Vec<_>>(),
            crate::model::settings::ToolbarButton::ALL.to_vec()
        );
        for meta in super::BUTTONS {
            assert_eq!(
                crate::model::settings::ToolbarButton::from_id(meta.button.id()),
                Some(meta.button)
            );
        }
    }
}
