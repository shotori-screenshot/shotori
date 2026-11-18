//! # Selection toolbar: actions + annotation tool controls
//!
//! Buttons dispatch the exact same actions as the keyboard through
//! `dispatch_action` — one action, two triggers, one pipeline. Row two
//! (annotation tools, colors, widths) only appears while a tool is
//! active. Geometry lives in [`crate::model::placement`].

mod annot_options;
mod geometry_options;
mod line_options;
mod number_options;
mod row;
mod spotlight_options;
mod text_options;
use annot_options::annot_options;
use gpui_kit::{
    assets::IconName,
    base::{
        Button,
        slider::{Slider, SliderIndicator, SliderState, SliderThumb, SliderTrack},
    },
    component::color_picker::ColorPickerState,
    *,
};

use crate::model::placement::{BAR_PAD, BTN_GAP, BTN_SIZE, GRIP_W, ROW_GAP, ROW_H};
use crate::model::session::ScreenshotSession;
use crate::ui::{color_picker::ColorPicker, theme};

use crate::actions::{
    ClearAnnotations, CopySelection, OcrSelection, PinSelection, QrSelection, QuitOverlay,
    SaveSelection, ScrollSelection, ToggleArrow, ToggleEllipse, ToggleEraser, ToggleHighlighter,
    ToggleLine, ToggleMagnifier, ToggleMosaic, ToggleNumber, TogglePencil, TogglePolyline,
    ToggleRectangle, ToggleSelect, ToggleSpotlight, ToggleText,
};

// The default GPUI asset bundle does not include every toolbar icon.
gpui_kit::assets::icon_assets!(
    ToolbarAssets,
    [
        Eraser,
        Type,
        Bold,
        Italic,
        TypeOutline,
        SquareDashed,
        MirrorRectangular,
        Highlighter,
        Pencil,
        Square,
        Circle,
        Slash,
        Waypoints,
        ArrowUpRight,
        ListOrdered,
        Plus,
        Minus,
        ScanText,
        ScanQrCode,
        ArrowDown,
        Pause,
        Play,
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
        WindowClose,
        // Settings window resize affordance.
        MoveDiagonal2,
        Check,
        Search,
        Sun,
        Sparkles,
        Scan,
        ChevronDown,
        ChevronRight,
        // The settings pages' inline help mark (hover tooltip hint).
        CircleQuestionMark,
        Bell,
        FolderOpen,
        Undo2,
        Redo2,
        ExternalLink,
        RotateCw,
        RotateCcw,
        FlipHorizontal2,
        FlipVertical2,
        Contrast,
        Droplet,
        Lock,
        LockOpen,
        Minimize2,
        Maximize2,
        ClipboardPaste,
        Clock,
        RefreshCw,
        Images,
        Eye,
        EyeOff
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

/// Resolve the explicit SVG paint color for a toolbar glyph: a selected
/// button fills with solid `accent`, so its glyph switches to
/// `capsule_on_accent()` for high contrast.
pub(super) fn icon_tint(selected: bool) -> u32 {
    let t = theme::c();
    if selected {
        t.capsule_on_accent()
    } else {
        t.toolbar_text
    }
}

/// One of the app's own icons (see [`OwnIcons`]).
fn own_icon_colored(path: &'static str, color: u32) -> Svg {
    svg().path(path).size(px(18.)).text_color(rgba(color))
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
        self.svg_colored(theme::c().toolbar_text)
    }

    pub(crate) fn svg_for(self, selected: bool) -> Svg {
        self.svg_colored(icon_tint(selected))
    }

    fn svg_colored(self, color: u32) -> Svg {
        match self {
            Self::Kit(name) => svg().path(name.path()),
            Self::Own(path) => svg().path(path),
        }
        .size(px(18.))
        .text_color(rgba(color))
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
    /// Configurable keybinding id in `actions::CONFIGURABLE_KEYS` (`None` when fixed/none).
    pub(crate) key_id: Option<&'static str>,
    /// Factory shortcut hint ("" when the button has none).
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
        key_id: Some("tool_select"),
        shortcut: "V",
        icon: ButtonIcon::Own("icons/select.svg"),
    },
    ButtonMeta {
        button: crate::model::settings::ToolbarButton::Rectangle,
        debug_id: "tb-rectangle",
        label_key: "toolbar.rectangle",
        key_id: Some("tool_rectangle"),
        shortcut: "R",
        icon: ButtonIcon::Kit(IconName::Square),
    },
    ButtonMeta {
        button: crate::model::settings::ToolbarButton::Ellipse,
        debug_id: "tb-ellipse",
        label_key: "toolbar.ellipse",
        key_id: Some("tool_ellipse"),
        shortcut: "E",
        icon: ButtonIcon::Kit(IconName::Circle),
    },
    ButtonMeta {
        button: crate::model::settings::ToolbarButton::Line,
        debug_id: "tb-line",
        label_key: "toolbar.line",
        key_id: Some("tool_line"),
        shortcut: "L",
        icon: ButtonIcon::Kit(IconName::Slash),
    },
    ButtonMeta {
        button: crate::model::settings::ToolbarButton::Polyline,
        debug_id: "tb-polyline",
        label_key: "toolbar.polyline",
        key_id: Some("tool_polyline"),
        shortcut: "P",
        icon: ButtonIcon::Kit(IconName::Waypoints),
    },
    ButtonMeta {
        button: crate::model::settings::ToolbarButton::Arrow,
        debug_id: "tb-arrow",
        label_key: "toolbar.arrow",
        key_id: Some("tool_arrow"),
        shortcut: "A",
        icon: ButtonIcon::Kit(IconName::ArrowUpRight),
    },
    ButtonMeta {
        button: crate::model::settings::ToolbarButton::Number,
        debug_id: "tb-number",
        label_key: "toolbar.number",
        key_id: Some("tool_number"),
        shortcut: "N",
        icon: ButtonIcon::Kit(IconName::ListOrdered),
    },
    ButtonMeta {
        button: crate::model::settings::ToolbarButton::Pencil,
        debug_id: "tb-pencil",
        label_key: "toolbar.pencil",
        key_id: Some("tool_pencil"),
        shortcut: "B",
        icon: ButtonIcon::Kit(IconName::Pencil),
    },
    ButtonMeta {
        button: crate::model::settings::ToolbarButton::Highlighter,
        debug_id: "tb-highlighter",
        label_key: "toolbar.highlighter",
        key_id: Some("tool_highlighter"),
        shortcut: "H",
        icon: ButtonIcon::Kit(IconName::Highlighter),
    },
    ButtonMeta {
        button: crate::model::settings::ToolbarButton::Mosaic,
        debug_id: "tb-mosaic",
        label_key: "toolbar.mosaic",
        key_id: Some("tool_mosaic"),
        shortcut: "M",
        icon: ButtonIcon::Own("icons/mosaic.svg"),
    },
    ButtonMeta {
        button: crate::model::settings::ToolbarButton::Magnifier,
        debug_id: "tb-magnifier",
        label_key: "toolbar.magnifier",
        key_id: Some("tool_magnifier"),
        shortcut: "G",
        icon: ButtonIcon::Kit(IconName::Search),
    },
    ButtonMeta {
        button: crate::model::settings::ToolbarButton::Spotlight,
        debug_id: "tb-spotlight",
        label_key: "toolbar.spotlight",
        key_id: Some("tool_spotlight"),
        shortcut: "Shift+H",
        icon: ButtonIcon::Kit(IconName::Sun),
    },
    ButtonMeta {
        button: crate::model::settings::ToolbarButton::Eraser,
        debug_id: "tb-eraser",
        label_key: "toolbar.eraser",
        key_id: Some("tool_eraser"),
        shortcut: "D",
        icon: ButtonIcon::Kit(IconName::Eraser),
    },
    ButtonMeta {
        button: crate::model::settings::ToolbarButton::Text,
        debug_id: "tb-text",
        label_key: "toolbar.text",
        key_id: Some("tool_text"),
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
        key_id: Some("clear"),
        shortcut: "Ctrl+Shift+Del",
        icon: ButtonIcon::Kit(IconName::Trash),
    },
    ButtonMeta {
        button: crate::model::settings::ToolbarButton::Ocr,
        debug_id: "tb-ocr",
        label_key: "toolbar.ocr",
        key_id: Some("ocr"),
        shortcut: "Ctrl+O",
        icon: ButtonIcon::Kit(IconName::ScanText),
    },
    ButtonMeta {
        button: crate::model::settings::ToolbarButton::Qr,
        debug_id: "tb-qr",
        label_key: "toolbar.qr",
        key_id: Some("qr"),
        shortcut: "Ctrl+Q",
        icon: ButtonIcon::Kit(IconName::ScanQrCode),
    },
    ButtonMeta {
        // own icon: the gpui-kit asset whitelist (`icon_assets!`) does
        // not bundle a pin, and IconName::Pin would render an empty
        // slot — same situation as the mosaic icon
        button: crate::model::settings::ToolbarButton::Pin,
        debug_id: "tb-pin",
        label_key: "toolbar.pin",
        key_id: Some("pin"),
        shortcut: "Ctrl+P",
        icon: ButtonIcon::Own("icons/pin.svg"),
    },
    ButtonMeta {
        // own icon: no long-screenshot glyph in the Lucide whitelist
        // either (document + down arrow)
        button: crate::model::settings::ToolbarButton::Scroll,
        debug_id: "tb-scroll",
        label_key: "toolbar.scroll",
        key_id: Some("scroll"),
        shortcut: "Ctrl+L",
        icon: ButtonIcon::Own("icons/scroll.svg"),
    },
    ButtonMeta {
        button: crate::model::settings::ToolbarButton::Save,
        debug_id: "tb-save",
        label_key: "toolbar.save",
        key_id: Some("save"),
        shortcut: "Ctrl+S",
        icon: ButtonIcon::Kit(IconName::Save),
    },
    ButtonMeta {
        button: crate::model::settings::ToolbarButton::Copy,
        debug_id: "tb-copy",
        label_key: "toolbar.copy",
        key_id: Some("copy"),
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
        key_id: Some("settings"),
        shortcut: "Ctrl+,",
        icon: ButtonIcon::Kit(IconName::Settings),
    },
    ButtonMeta {
        button: crate::model::settings::ToolbarButton::Cancel,
        debug_id: "tb-cancel",
        label_key: "toolbar.cancel",
        key_id: None,
        shortcut: "",
        icon: ButtonIcon::Kit(IconName::X),
    },
];

/// Shared commands for toolbar buttons. Explicit close is unconditional;
/// Escape has a separate action for finishing the current interaction.
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
        B::Magnifier => Box::new(ToggleMagnifier),
        B::Spotlight => Box::new(ToggleSpotlight),
        B::Eraser => Box::new(ToggleEraser),
        B::Text => Box::new(ToggleText),
        B::Clear => Box::new(ClearAnnotations),
        B::Ocr => Box::new(OcrSelection),
        B::Qr => Box::new(QrSelection),
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
        B::Magnifier => edit == Some(K::Magnifier),
        B::Spotlight => edit == Some(K::Spotlight),
        B::Mosaic => matches!(edit, Some(K::Mosaic | K::Blur)),
        B::Eraser => edit == Some(K::Eraser),
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

    /// Resolve the live shortcut string from the user's configured `keys` map,
    /// falling back to the factory `shortcut` when absent.
    pub(crate) fn shortcut_for(&self, keys: &std::collections::BTreeMap<String, String>) -> String {
        let Some(id) = self.key_id else {
            return self.shortcut.to_string();
        };
        let Some(chord) = keys.get(id) else {
            return self.shortcut.to_string();
        };
        let pretty = crate::actions::pretty_keystroke(chord);
        if self.button == crate::model::settings::ToolbarButton::Copy {
            format!("Enter / {pretty}")
        } else {
            pretty
        }
    }

    /// The tooltip / accessibility label ("Name · Shortcut") resolved against `keys`.
    pub(crate) fn tooltip_for(&self, keys: &std::collections::BTreeMap<String, String>) -> String {
        let sc = self.shortcut_for(keys);
        if sc.is_empty() {
            self.label().to_string()
        } else {
            format!("{} · {sc}", self.label())
        }
    }
}

/// Row one for either the selection toolbar (`row`) or an annotating pin (`pin_tool_row`).
fn row_items(
    items: &[crate::model::settings::ToolbarItem],
    annotations: &crate::annotation::Annotations,
    focus: &FocusHandle,
    settings: &crate::model::settings::Settings,
) -> Vec<AnyElement> {
    use crate::model::settings::ToolbarItem;
    items
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
                let selected = selected_state(button, annotations);
                control(
                    meta.debug_id.into(),
                    meta.tooltip_for(settings.keys()),
                    selected,
                    focus.clone(),
                    move |window, cx| {
                        window.dispatch_action(action_for(button), cx);
                    },
                )
                .child(meta.icon.svg_for(selected))
                .into_any_element()
            }
            ToolbarItem::Separator => separator().into_any_element(),
        })
        .collect()
}

/// First annotation tool available on a floating pin when entering edit mode
/// with no tool active: prefers the first configured drawing tool in
/// `pin_tool_row()`, falling back to `Select` or `Rectangle`.
pub(crate) fn initial_pin_tool(
    settings: &crate::model::settings::Settings,
) -> crate::annotation::ShapeKind {
    use crate::annotation::ShapeKind as K;
    use crate::model::settings::{ToolbarButton as B, ToolbarItem};
    let to_kind = |b: B| match b {
        B::Select => Some(K::Select),
        B::Rectangle => Some(K::Rectangle),
        B::Ellipse => Some(K::Ellipse),
        B::Line => Some(K::Line),
        B::Polyline => Some(K::Polyline),
        B::Arrow => Some(K::Arrow),
        B::Number => Some(K::Number),
        B::Pencil => Some(K::Pencil),
        B::Highlighter => Some(K::Highlighter),
        B::Mosaic => Some(K::Mosaic),
        B::Magnifier => Some(K::Magnifier),
        B::Spotlight => Some(K::Spotlight),
        B::Eraser => Some(K::Eraser),
        B::Text => Some(K::Text),
        _ => None,
    };
    let row = settings.pin_tool_row();
    row.iter()
        .find_map(|item| match item {
            ToolbarItem::Button(b) => to_kind(*b).filter(|k| *k != K::Select),
            ToolbarItem::Separator => None,
        })
        .or_else(|| {
            row.iter().find_map(|item| match item {
                ToolbarItem::Button(b) => to_kind(*b),
                ToolbarItem::Separator => None,
            })
        })
        .unwrap_or(K::Rectangle)
}

#[allow(clippy::too_many_arguments)]
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
    // Installed once at startup (settings.toml / `shotori settings`);
    // row one below reads it, placement's width basis reads the same.
    let settings = crate::model::settings::current();

    div()
        .id("shotori-toolbar")
        .absolute()
        .left(rect.origin.x)
        .top(rect.origin.y)
        // Rows retain their natural width within the shared placement bounds.
        .w(rect.size.width)
        .flex()
        .flex_col()
        .items_center()
        .gap(px(ROW_GAP))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(row::ToolbarRow::new(
            "toolbar-actions-scroll",
            rect.size.width,
            bar()
                .child(grip("tb-grip-left", output.clone(), session.clone()))
                .children(row_items(settings.row(), annotations, &focus, settings))
                .child(grip("tb-grip-right", output, session.clone())),
        ))
        .children(edit_kind.is_some().then(|| {
            row::ToolbarRow::new(
                "annotation-options-scroll",
                rect.size.width,
                annot_options(
                    edit_kind,
                    annotations,
                    &session,
                    &focus,
                    &size_slider,
                    &swatch_picker,
                    selected_color,
                ),
            )
        }))
}

/// Floating pin annotation toolbar backed by the main overlay's tool vocabulary
/// and Row-2 option controls (`ToolbarCluster::Tools` only, no explicit undo/redo).
pub(crate) fn pin_annotation_toolbar(
    rect: Bounds<Pixels>,
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
    let settings = crate::model::settings::current();
    let pin_row = settings.pin_tool_row();

    div()
        .id("shotori-pin-toolbar")
        .debug_selector(|| "shotori-pin-toolbar".into())
        .absolute()
        .left(rect.origin.x)
        .top(rect.origin.y)
        .w(rect.size.width)
        .flex()
        .flex_col()
        .items_center()
        .gap(px(ROW_GAP))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
        .child(row::ToolbarRow::new(
            "pin-toolbar-actions-scroll",
            rect.size.width,
            bar().children(row_items(&pin_row, annotations, &focus, settings)),
        ))
        .children(edit_kind.is_some().then(|| {
            row::ToolbarRow::new(
                "pin-annotation-options-scroll",
                rect.size.width,
                annot_options(
                    edit_kind,
                    annotations,
                    &session,
                    &focus,
                    &size_slider,
                    &swatch_picker,
                    selected_color,
                ),
            )
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
fn size_control(sc: &SizeSlider, suffix: &str) -> Div {
    const TRACK_W: f32 = 108.;
    const THUMB_SIZE: f32 = 14.;
    let t = theme::c();
    let state = &sc.state;
    let travel = TRACK_W - THUMB_SIZE;
    let fill_w = (sc.percentage * travel).clamp(0., travel);
    let thumb_fill = if t.is_dark() { 0xDBDDE4FF } else { 0xFAFBFDFF };
    let thumb_border = if t.is_dark() { 0x0000002E } else { 0x00000020 };

    let thumb = SliderThumb::new(state)
        .debug_selector(|| "annotation-slider-thumb".into())
        .absolute()
        .top(px(10. - THUMB_SIZE / 2.))
        .left(px(fill_w))
        .size(px(THUMB_SIZE))
        .rounded_full()
        .bg(rgba(thumb_fill))
        .border_1()
        .border_color(rgba(thumb_border))
        .shadow(vec![
            BoxShadow::new(px(0.), px(1.), rgba(0x0000002B).into()).blur_radius(px(2.)),
        ]);

    let track_bg = rgba(theme::alpha(t.toolbar_border, 0x99));

    div()
        .flex()
        .items_center()
        .gap(px(8.))
        .child(
            div()
                .debug_selector(|| "annotation-slider-track".into())
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
                                .left(px(THUMB_SIZE / 2.))
                                .top(px(8.))
                                .w(px(travel))
                                .h(px(4.))
                                .rounded_full()
                                .bg(track_bg)
                                .child(
                                    div()
                                        .absolute()
                                        .left_0()
                                        .top_0()
                                        .h_full()
                                        .w(px(fill_w))
                                        .rounded_full()
                                        .bg(rgba(t.accent)),
                                )
                                // the indicator is the bounds probe the
                                // track's click-to-position maps through
                                .child(SliderIndicator::new(state).size_full()),
                        ),
                )
                .child(thumb),
        )
        .child(
            div()
                .debug_selector(|| "tb-size-readout".to_string())
                .w(px(26.))
                .text_size(px(12.))
                .text_color(rgba(t.hint_text))
                .child(format!("{:.0}{suffix}", sc.current)),
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
        // should grab (row one only, `BAR_PAD` inset, `ROW_H` tall):
        // every pixel that shows the hand grabs, every pixel that grabs shows
        // the hand.
        .h(px(ROW_H))
        .w(px(GRIP_W))
        .flex()
        .items_center()
        .justify_center()
        .on_mouse_down(MouseButton::Left, move |ev, _, cx| {
            session.update(cx, |s, cx| {
                if s.toolbar_drag_begin(&output, ev.position) {
                    cx.notify();
                }
            });
            cx.stop_propagation(); // the press belongs to the toolbar, not the canvas
        })
        .child(grip_visual())
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

/// Capsule drag handle visual, reused by the settings editor's
/// live toolbar preview.
pub(crate) fn grip_visual() -> AnyElement {
    let pill = (theme::c().toolbar_text & 0xFFFFFF00) | 0x3D;
    div()
        .w(px(3.5))
        .h(px(16.))
        .rounded_full()
        .bg(rgba(pill))
        .into_any_element()
}

/// The move handle's grip: a compact horizontal grabber bar.
pub(crate) fn chip_grip() -> AnyElement {
    let pill = (theme::c().toolbar_text & 0xFFFFFF00) | 0x70;
    div()
        .w(px(18.))
        .h(px(3.5))
        .rounded_full()
        .bg(rgba(pill))
        .into_any_element()
}

/// A non-dispatching control for the settings editor's preview bar:
/// matches the capsule toolbar's button geometry, while the click
/// belongs to the editor (remove), never to the action itself.
pub(crate) fn preview_control(
    meta: &ButtonMeta,
    keys: &std::collections::BTreeMap<String, String>,
    focus: &FocusHandle,
    on_click: impl Fn(&mut Window, &mut App) + 'static,
) -> Button {
    control(
        meta.debug_id.into(),
        meta.tooltip_for(keys),
        false,
        focus.clone(),
        on_click,
    )
    .child(meta.icon.svg())
}

/// Corner radius of the outer floating bar — shared between the scroll
/// wrapper's shadow silhouette (`ToolbarRow`) and the bar fill container.
pub(crate) fn bar_radius() -> Pixels {
    px(14.)
}

/// Outer elevation shadow painted outside any `overflow_x_scroll` clip mask.
pub(crate) fn bar_shadow() -> Vec<BoxShadow> {
    theme::capsule_floating_shadow()
}

/// The bar container — shared with the settings editor's preview.
/// Callers inside horizontal scroll containers attach `bar_shadow` on
/// the outer non-scrolling wrapper so rectangular scroll masks never slice
/// drop shadows outside rounded corners.
pub(crate) fn bar() -> Div {
    let t = theme::c();
    div()
        .text_color(rgba(t.toolbar_text))
        .flex()
        .items_center()
        .gap(px(BTN_GAP))
        .h(px(ROW_H))
        .px(px(BAR_PAD))
        .rounded(bar_radius())
        .bg(rgba(t.capsule_surface()))
        .border_1()
        .border_color(rgba(t.capsule_border()))
}

/// The 1px group break — shared with the settings editor's preview.
pub(crate) fn separator() -> Div {
    let t = theme::c();
    div()
        .w(px(1.))
        .h(px(20.))
        .mx(px(5.))
        .rounded_full()
        .bg(rgba(t.capsule_border()))
}

/// Restore the canvas focus BEFORE changing toolbar state. A focused control
/// may disappear on redraw (e.g. leaving rectangle mode), otherwise scoped
/// shortcut dispatch no longer has the screenshot root in its focus path.
fn control(
    id: String,
    label: String,
    selected: bool,
    focus: FocusHandle,
    on_click: impl Fn(&mut Window, &mut App) + 'static,
) -> Button {
    let selector = id.clone();
    let tooltip: SharedString = label.clone().into();
    let t = theme::c();
    Button::new(SharedString::from(id))
        .debug_selector(move || selector)
        .accessibility_label(label)
        .tooltip(move |_, cx| cx.new(|_| ToolbarTooltip(tooltip.clone())).into())
        .on_click(move |_, window, cx| {
            window.focus(&focus, cx);
            on_click(window, cx);
        })
        .selected(selected)
        .flex_shrink_0()
        .bg(rgba(if selected { t.accent } else { 0x0000_0000 }))
        .text_color(rgba(if selected {
            t.capsule_on_accent()
        } else {
            t.toolbar_text
        }))
        .focus_visible(move |s| s.border_1().border_color(rgba(t.accent)))
        .size(px(BTN_SIZE))
        .rounded(px(8.))
        .hover(move |s| {
            if selected {
                s.bg(rgba(t.accent)).text_color(rgba(t.capsule_on_accent()))
            } else {
                s.bg(rgba(t.capsule_hover()))
            }
        })
        .styles(move |s| {
            s.selected(move |s| s.bg(rgba(t.accent)).text_color(rgba(t.capsule_on_accent())))
        })
}

#[allow(clippy::too_many_arguments)]
fn choice_menu<T: Copy + PartialEq + 'static, V: Copy + 'static>(
    id: &'static str,
    label: &'static str,
    value: T,
    choices: &'static [(T, &'static str, V)],
    focus: &FocusHandle,
    preview: fn(V, u32) -> Svg,
    apply: impl Fn(T, &mut App) + 'static,
) -> impl IntoElement {
    let selected = choices.iter().find(|(choice, _, _)| *choice == value);
    let current_label = selected.map(|(_, key, _)| *key).unwrap_or(label);
    let fg = theme::c().toolbar_text;
    let trigger = control(
        id.into(),
        crate::i18n::t!(current_label).into_owned(),
        false,
        focus.clone(),
        |_, _| {},
    )
    // Popover owns focus after mouse-down; activation must not return it to the canvas.
    .on_click(|_, _, _| {})
    .w(px(42.))
    .p(px(0.))
    .child(
        div()
            .flex()
            .items_center()
            .gap(px(1.))
            .children(selected.map(|(_, _, icon)| preview(*icon, fg)))
            .child(annot_options::menu_icon(IconName::ChevronDown, 10.)),
    );
    let focus = focus.clone();
    let apply = std::rc::Rc::new(apply);
    let t = theme::c();
    gpui_kit::component::popover::Popover::new(id)
        .p(px(0.))
        .rounded(px(12.))
        .bg(rgba(t.capsule_sheet_surface()))
        .border_1()
        .border_color(rgba(t.capsule_border()))
        .shadow(theme::capsule_floating_shadow())
        .trigger(trigger)
        .content(move |_, _, cx| {
            let popup = cx.entity();
            let mut panel = annotation_menu_panel().flex().flex_col().gap(px(2.));
            for &(choice, key, path) in choices {
                let apply = apply.clone();
                let popup = popup.clone();
                let is_sel = choice == value;
                panel = panel.child(
                    control(
                        key.into(),
                        crate::i18n::t!(key).into_owned(),
                        is_sel,
                        focus.clone(),
                        move |window, cx| {
                            popup.update(cx, |state, cx| state.dismiss(window, cx));
                            apply(choice, cx);
                        },
                    )
                    .w(px(36.))
                    .child(preview(path, icon_tint(is_sel))),
                );
            }
            panel
        })
}

/// Deferred menu surfaces must consume presses before the window-level canvas listener.
fn annotation_menu_panel() -> Div {
    div()
        .p(px(4.))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
        .on_mouse_up(MouseButton::Left, |_, _, cx| cx.stop_propagation())
}

pub(crate) struct ToolbarTooltip(pub(crate) SharedString);
impl Render for ToolbarTooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let t = theme::c();
        div()
            .px(px(9.))
            .py(px(4.))
            .rounded(px(7.))
            .bg(rgba(t.capsule_sheet_surface()))
            .border_1()
            .border_color(rgba(t.capsule_border()))
            .shadow(vec![
                BoxShadow::new(px(0.), px(3.), rgba(0x00000038).into()).blur_radius(px(10.)),
            ])
            .text_size(px(11.5))
            .text_color(rgba(t.toolbar_text))
            .child(self.0.clone())
    }
}

#[cfg(test)]
mod tests {
    use crate::model::placement::{TB_W, round_px, toolbar_anchor, toolbar_bounds};
    use gpui_kit::{Bounds, point, px, size};
    #[test]
    fn toolbar_clamps_horizontally() {
        let b = Bounds::new(point(px(1800.), px(100.)), size(px(100.), px(100.)));
        assert_eq!(
            toolbar_anchor(&b, size(px(1920.), px(1080.)), TB_W, super::ROW_H).0,
            1920. - TB_W - 8.
        );
        // the composed rect: anchor + the width clamp the render side applies
        let rect = toolbar_bounds(&round_px(b), size(px(1920.), px(1080.)), TB_W, super::ROW_H);
        assert_eq!(rect.size.width, px(TB_W));
        // narrow window: the toolbar shrinks to the window minus breathing room
        let rect = toolbar_bounds(&round_px(b), size(px(400.), px(400.)), TB_W, super::ROW_H);
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
            IconName::Bell,
            IconName::FolderOpen,
            IconName::Undo2,
            IconName::ExternalLink,
        ] {
            assert!(
                super::ToolbarSource.load(&icon.path()).unwrap().is_some(),
                "{:?} is missing from the asset source",
                icon
            );
        }
    }

    #[test]
    fn tooltip_reflects_custom_shortcuts() {
        let mut keys = crate::model::settings::Settings::default().keys().clone();
        keys.insert("tool_arrow".into(), "ctrl-shift-a".into());
        keys.insert("copy".into(), "ctrl-shift-c".into());
        let arrow = super::meta_for(crate::model::settings::ToolbarButton::Arrow).unwrap();
        assert_eq!(arrow.shortcut_for(&keys), "Ctrl+Shift+A");
        let copy = super::meta_for(crate::model::settings::ToolbarButton::Copy).unwrap();
        assert_eq!(copy.shortcut_for(&keys), "Enter / Ctrl+Shift+C");
        let cancel = super::meta_for(crate::model::settings::ToolbarButton::Cancel).unwrap();
        assert_eq!(cancel.shortcut_for(&keys), "");
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
