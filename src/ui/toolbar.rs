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
        Settings
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
    /// Settings-row name; the toolbar tooltip is "name · shortcut".
    pub(crate) name: &'static str,
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
        name: "Select",
        shortcut: "V",
        icon: ButtonIcon::Own("icons/select.svg"),
    },
    ButtonMeta {
        button: crate::model::settings::ToolbarButton::Rectangle,
        debug_id: "tb-rectangle",
        name: "Rectangle",
        shortcut: "R",
        icon: ButtonIcon::Kit(IconName::Square),
    },
    ButtonMeta {
        button: crate::model::settings::ToolbarButton::Ellipse,
        debug_id: "tb-ellipse",
        name: "Ellipse",
        shortcut: "E",
        icon: ButtonIcon::Kit(IconName::Circle),
    },
    ButtonMeta {
        button: crate::model::settings::ToolbarButton::Line,
        debug_id: "tb-line",
        name: "Line",
        shortcut: "L",
        icon: ButtonIcon::Kit(IconName::Slash),
    },
    ButtonMeta {
        button: crate::model::settings::ToolbarButton::Polyline,
        debug_id: "tb-polyline",
        name: "Polyline",
        shortcut: "P",
        icon: ButtonIcon::Kit(IconName::Waypoints),
    },
    ButtonMeta {
        button: crate::model::settings::ToolbarButton::Arrow,
        debug_id: "tb-arrow",
        name: "Arrow",
        shortcut: "A",
        icon: ButtonIcon::Kit(IconName::ArrowUpRight),
    },
    ButtonMeta {
        button: crate::model::settings::ToolbarButton::Number,
        debug_id: "tb-number",
        name: "Sequence number",
        shortcut: "N",
        icon: ButtonIcon::Kit(IconName::ListOrdered),
    },
    ButtonMeta {
        button: crate::model::settings::ToolbarButton::Pencil,
        debug_id: "tb-pencil",
        name: "Pencil",
        shortcut: "B",
        icon: ButtonIcon::Kit(IconName::Pencil),
    },
    ButtonMeta {
        button: crate::model::settings::ToolbarButton::Highlighter,
        debug_id: "tb-highlighter",
        name: "Highlighter",
        shortcut: "H",
        icon: ButtonIcon::Kit(IconName::Highlighter),
    },
    ButtonMeta {
        button: crate::model::settings::ToolbarButton::Mosaic,
        debug_id: "tb-mosaic",
        name: "Mosaic / Blur",
        shortcut: "M",
        icon: ButtonIcon::Own("icons/mosaic.svg"),
    },
    ButtonMeta {
        button: crate::model::settings::ToolbarButton::Eraser,
        debug_id: "tb-eraser",
        name: "Eraser",
        shortcut: "D",
        icon: ButtonIcon::Kit(IconName::Eraser),
    },
    ButtonMeta {
        button: crate::model::settings::ToolbarButton::Text,
        debug_id: "tb-text",
        name: "Text",
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
        name: "Clear annotations",
        shortcut: "Ctrl+Shift+Del",
        icon: ButtonIcon::Kit(IconName::Trash),
    },
    ButtonMeta {
        button: crate::model::settings::ToolbarButton::Ocr,
        debug_id: "tb-ocr",
        name: "Recognize text",
        shortcut: "Ctrl+O",
        icon: ButtonIcon::Kit(IconName::ScanText),
    },
    ButtonMeta {
        button: crate::model::settings::ToolbarButton::Save,
        debug_id: "tb-save",
        name: "Save",
        shortcut: "Ctrl+S",
        icon: ButtonIcon::Kit(IconName::Save),
    },
    ButtonMeta {
        // own icon: the gpui-kit asset whitelist (`icon_assets!`) does
        // not bundle a pin, and IconName::Pin would render an empty
        // slot — same situation as the mosaic icon
        button: crate::model::settings::ToolbarButton::Pin,
        debug_id: "tb-pin",
        name: "Pin to screen",
        shortcut: "Ctrl+P",
        icon: ButtonIcon::Own("icons/pin.svg"),
    },
    ButtonMeta {
        // own icon: no long-screenshot glyph in the Lucide whitelist
        // either (document + down arrow)
        button: crate::model::settings::ToolbarButton::Scroll,
        debug_id: "tb-scroll",
        name: "Long screenshot (scroll)",
        shortcut: "Ctrl+L",
        icon: ButtonIcon::Own("icons/scroll.svg"),
    },
    ButtonMeta {
        button: crate::model::settings::ToolbarButton::Cancel,
        debug_id: "tb-cancel",
        name: "Cancel",
        shortcut: "Esc",
        icon: ButtonIcon::Kit(IconName::X),
    },
    ButtonMeta {
        button: crate::model::settings::ToolbarButton::Copy,
        debug_id: "tb-copy",
        name: "Copy",
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
        name: "Settings",
        shortcut: "Ctrl+,",
        icon: ButtonIcon::Kit(IconName::Settings),
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
    /// The tooltip / accessibility label ("Name · Shortcut").
    pub(crate) fn tooltip(&self) -> String {
        if self.shortcut.is_empty() {
            self.name.to_owned()
        } else {
            format!("{} · {}", self.name, self.shortcut)
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
            for (ix, color) in theme::c().annotation_colors.into_iter().enumerate() {
                let name = theme::PALETTE_NAMES[ix];
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
    div()
        .id(id)
        // Full row height, and offset only by the bar's side padding —
        // the element's rect is then EXACTLY `session::toolbar_grips`
        // (row one only, BAR_PAD inset, ROW_H tall): every pixel that
        // shows the hand grabs, every pixel that grabs shows the hand.
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
