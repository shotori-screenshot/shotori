//! `[capture]`, `[scroll]`, `[notify]`, and `[pin]` settings blocks.

use std::path::Path;

use serde::Deserialize;

use crate::model::scroll_sheet::Axis as ScrollAxis;

/// How the overlay selection's dimension badge formats its readout.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum SizeLabelMode {
    /// `W × H` in logical pixels (factory default).
    #[default]
    Logical,
    /// `W × H px` in captured physical device pixels.
    Physical,
    /// `X, Y · W × H` with global logical coordinates.
    Bounds,
    /// Hide the size badge completely.
    Hidden,
}

impl SizeLabelMode {
    pub const ALL: [Self; 4] = [Self::Logical, Self::Physical, Self::Bounds, Self::Hidden];

    pub const fn id(self) -> &'static str {
        match self {
            Self::Logical => "logical",
            Self::Physical => "physical",
            Self::Bounds => "bounds",
            Self::Hidden => "hidden",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|m| m.id() == id.trim())
    }

    pub fn label(self) -> std::borrow::Cow<'static, str> {
        match self {
            Self::Logical => crate::i18n::t!("settings.capture.size_label.logical"),
            Self::Physical => crate::i18n::t!("settings.capture.size_label.physical"),
            Self::Bounds => crate::i18n::t!("settings.capture.size_label.bounds"),
            Self::Hidden => crate::i18n::t!("settings.capture.size_label.hidden"),
        }
    }
}

/// Square inset size of the corner/handle magnifier loupe.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum LoupeSize {
    Small,
    #[default]
    Medium,
    Large,
}

impl LoupeSize {
    pub const ALL: [Self; 3] = [Self::Small, Self::Medium, Self::Large];

    pub const fn id(self) -> &'static str {
        match self {
            Self::Small => "small",
            Self::Medium => "medium",
            Self::Large => "large",
        }
    }

    pub const fn side_px(self) -> f32 {
        match self {
            Self::Small => 120.,
            Self::Medium => 160.,
            Self::Large => 220.,
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|s| s.id() == id.trim())
    }

    pub fn label(self) -> std::borrow::Cow<'static, str> {
        match self {
            Self::Small => crate::i18n::t!("settings.capture.loupe_size.small"),
            Self::Medium => crate::i18n::t!("settings.capture.loupe_size.medium"),
            Self::Large => crate::i18n::t!("settings.capture.loupe_size.large"),
        }
    }
}

/// `[capture]` configuration block: selection HUD, loupe, post-exit behavior,
/// and motion preferences.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaptureSettings {
    /// Query compositor IPC (`niri`, `sway`, `Hyprland`) for window snapping.
    pub window_snap: bool,
    /// Draw full-output alignment crosshairs while aiming or dragging a region.
    pub crosshair: bool,
    /// Selection size chip content (`logical` | `physical` | `bounds` | `hidden`).
    pub size_label: SizeLabelMode,
    /// Show the precision magnifier loupe during corner/handle drags.
    pub loupe: bool,
    /// Loupe magnification factor (`2..=8`, default `3`).
    pub loupe_zoom: u8,
    /// Loupe inset square size (`small` | `medium` | `large`).
    pub loupe_size: LoupeSize,
    /// Stay in the capture overlay after copying to the clipboard.
    pub keep_open_after_copy: bool,
    /// Stay in the capture overlay after a direct save (`export.auto_save = true`).
    pub keep_open_after_save: bool,
    /// Double-click empty space inside the selection to copy and finish.
    pub double_click_copy: bool,
    /// Right-click outside annotation mode cancels the active selection or exits.
    pub right_click_cancel: bool,
    /// Skip non-essential chrome animations (loupe fade, OSD orbit, sidebar slide).
    pub reduce_motion: bool,
}

impl Default for CaptureSettings {
    fn default() -> Self {
        Self {
            window_snap: true,
            crosshair: false,
            size_label: SizeLabelMode::Logical,
            loupe: true,
            loupe_zoom: 3,
            loupe_size: LoupeSize::Medium,
            keep_open_after_copy: false,
            keep_open_after_save: false,
            double_click_copy: false,
            right_click_cancel: false,
            reduce_motion: false,
        }
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub(super) struct CaptureFile {
    window_snap: Option<bool>,
    crosshair: Option<bool>,
    size_label: Option<String>,
    loupe: Option<bool>,
    loupe_zoom: Option<i64>,
    loupe_size: Option<String>,
    keep_open_after_copy: Option<bool>,
    keep_open_after_save: Option<bool>,
    double_click_copy: Option<bool>,
    right_click_cancel: Option<bool>,
    reduce_motion: Option<bool>,
}

impl CaptureFile {
    pub(super) fn resolve(self, path: &Path, errors: &mut Vec<String>) -> CaptureSettings {
        let d = CaptureSettings::default();
        let size_label = match self.size_label.as_deref() {
            None => d.size_label,
            Some(id) => SizeLabelMode::from_id(id).unwrap_or_else(|| {
                errors.push(format!(
                    "{}: unknown capture.size_label {id:?} (using logical)",
                    path.display()
                ));
                d.size_label
            }),
        };
        let loupe_zoom = match self.loupe_zoom {
            None => d.loupe_zoom,
            Some(z) if (2..=8).contains(&z) => z as u8,
            Some(z) => {
                errors.push(format!(
                    "{}: capture.loupe_zoom = {z} must be in 2..=8 (using {})",
                    path.display(),
                    d.loupe_zoom
                ));
                d.loupe_zoom
            }
        };
        let loupe_size = match self.loupe_size.as_deref() {
            None => d.loupe_size,
            Some(id) => LoupeSize::from_id(id).unwrap_or_else(|| {
                errors.push(format!(
                    "{}: unknown capture.loupe_size {id:?} (using medium)",
                    path.display()
                ));
                d.loupe_size
            }),
        };
        CaptureSettings {
            window_snap: self.window_snap.unwrap_or(d.window_snap),
            crosshair: self.crosshair.unwrap_or(d.crosshair),
            size_label,
            loupe: self.loupe.unwrap_or(d.loupe),
            loupe_zoom,
            loupe_size,
            keep_open_after_copy: self.keep_open_after_copy.unwrap_or(d.keep_open_after_copy),
            keep_open_after_save: self.keep_open_after_save.unwrap_or(d.keep_open_after_save),
            double_click_copy: self.double_click_copy.unwrap_or(d.double_click_copy),
            right_click_cancel: self.right_click_cancel.unwrap_or(d.right_click_cancel),
            reduce_motion: self.reduce_motion.unwrap_or(d.reduce_motion),
        }
    }
}

impl CaptureSettings {
    pub(super) fn write_toml(
        &self,
        root: &mut toml::map::Map<String, toml::Value>,
        path: &Path,
    ) -> Result<(), String> {
        let section = root
            .entry("capture")
            .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
        let Some(t) = section.as_table_mut() else {
            return Err(format!(
                "{}: [capture] is not a table — refusing to overwrite",
                path.display()
            ));
        };
        t.insert("window_snap".into(), toml::Value::from(self.window_snap));
        t.insert("crosshair".into(), toml::Value::from(self.crosshair));
        t.insert("size_label".into(), toml::Value::from(self.size_label.id()));
        t.insert("loupe".into(), toml::Value::from(self.loupe));
        t.insert(
            "loupe_zoom".into(),
            toml::Value::from(i64::from(self.loupe_zoom)),
        );
        t.insert("loupe_size".into(), toml::Value::from(self.loupe_size.id()));
        t.insert(
            "keep_open_after_copy".into(),
            toml::Value::from(self.keep_open_after_copy),
        );
        t.insert(
            "keep_open_after_save".into(),
            toml::Value::from(self.keep_open_after_save),
        );
        t.insert(
            "double_click_copy".into(),
            toml::Value::from(self.double_click_copy),
        );
        t.insert(
            "right_click_cancel".into(),
            toml::Value::from(self.right_click_cancel),
        );
        t.insert(
            "reduce_motion".into(),
            toml::Value::from(self.reduce_motion),
        );
        Ok(())
    }
}

/// Default initial axis for long screenshots.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ScrollAxisSetting {
    #[default]
    Vertical,
    Horizontal,
}

impl ScrollAxisSetting {
    pub const ALL: [Self; 2] = [Self::Vertical, Self::Horizontal];

    pub const fn id(self) -> &'static str {
        match self {
            Self::Vertical => "vertical",
            Self::Horizontal => "horizontal",
        }
    }

    pub const fn to_axis(self) -> ScrollAxis {
        match self {
            Self::Vertical => ScrollAxis::Vertical,
            Self::Horizontal => ScrollAxis::Horizontal,
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|a| a.id() == id.trim())
    }

    pub fn label(self) -> std::borrow::Cow<'static, str> {
        match self {
            Self::Vertical => crate::i18n::t!("settings.scroll.axis.vertical"),
            Self::Horizontal => crate::i18n::t!("settings.scroll.axis.horizontal"),
        }
    }
}

/// How shotori handles compositor transparency and background blur while a
/// long screenshot is running. Translucent/blurred backgrounds stay stationary
/// behind scrolling foreground content and confuse the stitcher.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ScrollTransparencyMode {
    /// Automatically disable and restore transparency/blur on supported
    /// compositors (Hyprland IPC, niri IPC, KDE KWin D-Bus).
    #[default]
    Auto,
    /// Execute user-configured shell commands on capture start and end.
    Custom,
    /// Leave compositor transparency and blur untouched.
    Off,
}

impl ScrollTransparencyMode {
    pub const ALL: [Self; 3] = [Self::Auto, Self::Custom, Self::Off];

    pub const fn id(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Custom => "custom",
            Self::Off => "off",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|m| m.id() == id.trim())
    }

    pub fn label(self) -> std::borrow::Cow<'static, str> {
        match self {
            Self::Auto => crate::i18n::t!("settings.scroll.transparency_mode.auto"),
            Self::Custom => crate::i18n::t!("settings.scroll.transparency_mode.custom"),
            Self::Off => crate::i18n::t!("settings.scroll.transparency_mode.disabled"),
        }
    }
}

/// `[scroll]` configuration block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScrollSettings {
    pub default_axis: ScrollAxisSetting,
    /// Maximum stitched length in logical pixels (`2_000..=100_000`).
    pub max_length: u32,
    /// How to handle background transparency & blur during scroll capture
    /// (`auto` | `custom` | `off`, default `auto`).
    pub transparency_mode: ScrollTransparencyMode,
    /// Shell command (`sh -c`) run when a long screenshot begins
    /// (`transparency_mode = "custom"`). Normalized to `None` when blank.
    pub enter_command: Option<String>,
    /// Shell command (`sh -c`) run when a long screenshot ends
    /// (`transparency_mode = "custom"`). Normalized to `None` when blank.
    pub exit_command: Option<String>,
}

impl Default for ScrollSettings {
    fn default() -> Self {
        Self {
            default_axis: ScrollAxisSetting::Vertical,
            max_length: 20_000,
            transparency_mode: ScrollTransparencyMode::Auto,
            enter_command: None,
            exit_command: None,
        }
    }
}

impl ScrollSettings {
    pub fn set_enter_command(&mut self, cmd: Option<String>) {
        self.enter_command = cmd.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    }

    pub fn set_exit_command(&mut self, cmd: Option<String>) {
        self.exit_command = cmd.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub(super) struct ScrollFile {
    default_axis: Option<String>,
    max_length: Option<i64>,
    transparency_mode: Option<String>,
    enter_command: Option<String>,
    exit_command: Option<String>,
}

impl ScrollFile {
    pub(super) fn resolve(self, path: &Path, errors: &mut Vec<String>) -> ScrollSettings {
        let d = ScrollSettings::default();
        let default_axis = match self.default_axis.as_deref() {
            None => d.default_axis,
            Some(id) => ScrollAxisSetting::from_id(id).unwrap_or_else(|| {
                errors.push(format!(
                    "{}: unknown scroll.default_axis {id:?} (using vertical)",
                    path.display()
                ));
                d.default_axis
            }),
        };
        let max_length = match self.max_length {
            None => d.max_length,
            Some(v) if (2_000..=100_000).contains(&v) => v as u32,
            Some(v) => {
                errors.push(format!(
                    "{}: scroll.max_length = {v} must be in 2000..=100000 (using {})",
                    path.display(),
                    d.max_length
                ));
                d.max_length
            }
        };
        let transparency_mode = match self.transparency_mode.as_deref() {
            None => d.transparency_mode,
            Some(id) => ScrollTransparencyMode::from_id(id).unwrap_or_else(|| {
                errors.push(format!(
                    "{}: unknown scroll.transparency_mode {id:?} (using auto)",
                    path.display()
                ));
                d.transparency_mode
            }),
        };
        let enter_command = self
            .enter_command
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let exit_command = self
            .exit_command
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        ScrollSettings {
            default_axis,
            max_length,
            transparency_mode,
            enter_command,
            exit_command,
        }
    }
}

impl ScrollSettings {
    pub(super) fn write_toml(
        &self,
        root: &mut toml::map::Map<String, toml::Value>,
        path: &Path,
    ) -> Result<(), String> {
        let section = root
            .entry("scroll")
            .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
        let Some(t) = section.as_table_mut() else {
            return Err(format!(
                "{}: [scroll] is not a table — refusing to overwrite",
                path.display()
            ));
        };
        t.insert(
            "default_axis".into(),
            toml::Value::from(self.default_axis.id()),
        );
        t.insert(
            "max_length".into(),
            toml::Value::from(i64::from(self.max_length)),
        );
        t.insert(
            "transparency_mode".into(),
            toml::Value::from(self.transparency_mode.id()),
        );
        match &self.enter_command {
            Some(cmd) => {
                t.insert("enter_command".into(), toml::Value::from(cmd.as_str()));
            }
            None => {
                t.remove("enter_command");
            }
        }
        match &self.exit_command {
            Some(cmd) => {
                t.insert("exit_command".into(), toml::Value::from(cmd.as_str()));
            }
            None => {
                t.remove("exit_command");
            }
        }
        Ok(())
    }
}

/// `[notify]` configuration block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotifySettings {
    pub enabled: bool,
    pub on_copy: bool,
    pub on_save: bool,
    pub on_ocr: bool,
    pub on_qr: bool,
    pub show_thumbnail: bool,
    /// Popup duration in seconds (`2..=30`, default `10`).
    pub timeout_secs: u8,
}

impl Default for NotifySettings {
    fn default() -> Self {
        Self {
            enabled: true,
            on_copy: true,
            on_save: true,
            on_ocr: true,
            on_qr: true,
            show_thumbnail: true,
            timeout_secs: 10,
        }
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub(super) struct NotifyFile {
    enabled: Option<bool>,
    on_copy: Option<bool>,
    on_save: Option<bool>,
    on_ocr: Option<bool>,
    on_qr: Option<bool>,
    show_thumbnail: Option<bool>,
    timeout_secs: Option<i64>,
}

impl NotifyFile {
    pub(super) fn resolve(self, path: &Path, errors: &mut Vec<String>) -> NotifySettings {
        let d = NotifySettings::default();
        let timeout_secs = match self.timeout_secs {
            None => d.timeout_secs,
            Some(s) if (2..=30).contains(&s) => s as u8,
            Some(s) => {
                errors.push(format!(
                    "{}: notify.timeout_secs = {s} must be in 2..=30 (using {})",
                    path.display(),
                    d.timeout_secs
                ));
                d.timeout_secs
            }
        };
        NotifySettings {
            enabled: self.enabled.unwrap_or(d.enabled),
            on_copy: self.on_copy.unwrap_or(d.on_copy),
            on_save: self.on_save.unwrap_or(d.on_save),
            on_ocr: self.on_ocr.unwrap_or(d.on_ocr),
            on_qr: self.on_qr.unwrap_or(d.on_qr),
            show_thumbnail: self.show_thumbnail.unwrap_or(d.show_thumbnail),
            timeout_secs,
        }
    }
}

impl NotifySettings {
    pub(super) fn write_toml(
        &self,
        root: &mut toml::map::Map<String, toml::Value>,
        path: &Path,
    ) -> Result<(), String> {
        let section = root
            .entry("notify")
            .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
        let Some(t) = section.as_table_mut() else {
            return Err(format!(
                "{}: [notify] is not a table — refusing to overwrite",
                path.display()
            ));
        };
        t.insert("enabled".into(), toml::Value::from(self.enabled));
        t.insert("on_copy".into(), toml::Value::from(self.on_copy));
        t.insert("on_save".into(), toml::Value::from(self.on_save));
        t.insert("on_ocr".into(), toml::Value::from(self.on_ocr));
        t.insert("on_qr".into(), toml::Value::from(self.on_qr));
        t.insert(
            "show_thumbnail".into(),
            toml::Value::from(self.show_thumbnail),
        );
        t.insert(
            "timeout_secs".into(),
            toml::Value::from(i64::from(self.timeout_secs)),
        );
        Ok(())
    }
}

/// Modifier key required to wheel-zoom a floating pin.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum PinZoomModifier {
    #[default]
    None,
    Ctrl,
    Alt,
    Shift,
}

impl PinZoomModifier {
    pub const ALL: [Self; 4] = [Self::None, Self::Ctrl, Self::Alt, Self::Shift];

    pub const fn id(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Ctrl => "ctrl",
            Self::Alt => "alt",
            Self::Shift => "shift",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|m| m.id() == id.trim())
    }

    pub fn matches(self, modifiers: &gpui_kit::Modifiers) -> bool {
        match self {
            Self::None => true,
            Self::Ctrl => modifiers.control,
            Self::Alt => modifiers.alt,
            Self::Shift => modifiers.shift,
        }
    }

    pub(crate) fn explicit_matches(self, modifiers: &gpui_kit::Modifiers) -> bool {
        match self {
            Self::None => false,
            Self::Ctrl => modifiers.control,
            Self::Alt => modifiers.alt,
            Self::Shift => modifiers.shift,
        }
    }

    pub fn label(self) -> std::borrow::Cow<'static, str> {
        match self {
            Self::None => crate::i18n::t!("settings.pin.zoom_modifier.none"),
            Self::Ctrl => crate::i18n::t!("settings.pin.zoom_modifier.ctrl"),
            Self::Alt => crate::i18n::t!("settings.pin.zoom_modifier.alt"),
            Self::Shift => crate::i18n::t!("settings.pin.zoom_modifier.shift"),
        }
    }
}

/// Modifier key required to adjust a floating pin's opacity with the wheel.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum PinOpacityModifier {
    #[default]
    Ctrl,
    Alt,
    Shift,
    None,
    Disabled,
}

impl PinOpacityModifier {
    pub const ALL: [Self; 5] = [
        Self::Ctrl,
        Self::Alt,
        Self::Shift,
        Self::None,
        Self::Disabled,
    ];

    pub const fn id(self) -> &'static str {
        match self {
            Self::Ctrl => "ctrl",
            Self::Alt => "alt",
            Self::Shift => "shift",
            Self::None => "none",
            Self::Disabled => "disabled",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|m| m.id() == id.trim())
    }

    pub(crate) fn explicit_matches(self, modifiers: &gpui_kit::Modifiers) -> bool {
        match self {
            Self::Ctrl => modifiers.control,
            Self::Alt => modifiers.alt,
            Self::Shift => modifiers.shift,
            Self::None | Self::Disabled => false,
        }
    }

    pub fn label(self) -> std::borrow::Cow<'static, str> {
        match self {
            Self::Ctrl => crate::i18n::t!("settings.pin.opacity_modifier.ctrl"),
            Self::Alt => crate::i18n::t!("settings.pin.opacity_modifier.alt"),
            Self::Shift => crate::i18n::t!("settings.pin.opacity_modifier.shift"),
            Self::None => crate::i18n::t!("settings.pin.opacity_modifier.none"),
            Self::Disabled => crate::i18n::t!("settings.pin.opacity_modifier.disabled"),
        }
    }
}

/// Resolved wheel behavior on a floating pin when both zoom and opacity
/// modifiers may be configured independently.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PinWheelAction {
    Zoom,
    Opacity,
}

/// Arbitrate a scroll event between zoom and per-pin opacity adjustment:
/// explicit modifier chords (`Ctrl`/`Alt`/`Shift`) always beat unmodified
/// (`None`) wheel bindings so the factory pair (`zoom = none`, `opacity = ctrl`)
/// zooms on bare wheel and adjusts opacity on `Ctrl+Wheel`.
pub fn resolve_pin_wheel_action(
    modifiers: &gpui_kit::Modifiers,
    zoom: PinZoomModifier,
    opacity: PinOpacityModifier,
) -> Option<PinWheelAction> {
    if zoom.explicit_matches(modifiers) {
        Some(PinWheelAction::Zoom)
    } else if opacity.explicit_matches(modifiers) {
        Some(PinWheelAction::Opacity)
    } else if zoom == PinZoomModifier::None {
        Some(PinWheelAction::Zoom)
    } else if opacity == PinOpacityModifier::None {
        Some(PinWheelAction::Opacity)
    } else {
        None
    }
}

/// Action triggered by double-clicking a floating pin.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum PinDoubleClick {
    #[default]
    None,
    Close,
    CopyAndClose,
    ResetZoom,
    Collapse,
}

impl PinDoubleClick {
    pub const ALL: [Self; 5] = [
        Self::None,
        Self::Close,
        Self::CopyAndClose,
        Self::ResetZoom,
        Self::Collapse,
    ];

    pub const fn id(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Close => "close",
            Self::CopyAndClose => "copy_close",
            Self::ResetZoom => "reset_zoom",
            Self::Collapse => "collapse",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|m| m.id() == id.trim())
    }

    pub fn label(self) -> std::borrow::Cow<'static, str> {
        match self {
            Self::None => crate::i18n::t!("settings.pin.double_click.none"),
            Self::Close => crate::i18n::t!("settings.pin.double_click.close"),
            Self::CopyAndClose => crate::i18n::t!("settings.pin.double_click.copy_close"),
            Self::ResetZoom => crate::i18n::t!("settings.pin.double_click.reset_zoom"),
            Self::Collapse => crate::i18n::t!("settings.pin.double_click.collapse"),
        }
    }
}

/// `[pin]` configuration block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PinSettings {
    pub zoom_modifier: PinZoomModifier,
    pub opacity_modifier: PinOpacityModifier,
    /// Percentage zoom increment per wheel line (`5..=50`, default `12` = `1.12×`).
    pub zoom_step: u8,
    /// Default opacity percentage (`20..=100`, default `100`).
    pub opacity: u8,
    /// Double-click behavior on a floating pin.
    pub double_click: PinDoubleClick,
    pub show_border: bool,
    pub shadow: bool,
    pub snap_to_edge: bool,
    /// Magnetically align pins against sibling pins while dragging.
    pub snap_to_pins: bool,
    /// Maximum persisted entries in `$XDG_DATA_HOME/shotori/pin-history` (`0..=200`, `0` = off).
    pub max_history: u16,
}

impl Default for PinSettings {
    fn default() -> Self {
        Self {
            zoom_modifier: PinZoomModifier::None,
            opacity_modifier: PinOpacityModifier::Ctrl,
            zoom_step: 12,
            opacity: 100,
            double_click: PinDoubleClick::None,
            show_border: true,
            shadow: true,
            snap_to_edge: false,
            snap_to_pins: true,
            max_history: 50,
        }
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub(super) struct PinFile {
    zoom_modifier: Option<String>,
    opacity_modifier: Option<String>,
    zoom_step: Option<i64>,
    opacity: Option<i64>,
    double_click: Option<String>,
    show_border: Option<bool>,
    shadow: Option<bool>,
    snap_to_edge: Option<bool>,
    snap_to_pins: Option<bool>,
    max_history: Option<i64>,
}

impl PinFile {
    pub(super) fn resolve(self, path: &Path, errors: &mut Vec<String>) -> PinSettings {
        let d = PinSettings::default();
        let zoom_modifier = match self.zoom_modifier.as_deref() {
            None => d.zoom_modifier,
            Some(id) => PinZoomModifier::from_id(id).unwrap_or_else(|| {
                errors.push(format!(
                    "{}: unknown pin.zoom_modifier {id:?} (using none)",
                    path.display()
                ));
                d.zoom_modifier
            }),
        };
        let opacity_modifier = match self.opacity_modifier.as_deref() {
            None => d.opacity_modifier,
            Some(id) => PinOpacityModifier::from_id(id).unwrap_or_else(|| {
                errors.push(format!(
                    "{}: unknown pin.opacity_modifier {id:?} (using ctrl)",
                    path.display()
                ));
                d.opacity_modifier
            }),
        };
        let zoom_step = match self.zoom_step {
            None => d.zoom_step,
            Some(s) if (5..=50).contains(&s) => s as u8,
            Some(s) => {
                errors.push(format!(
                    "{}: pin.zoom_step = {s} must be in 5..=50 (using {})",
                    path.display(),
                    d.zoom_step
                ));
                d.zoom_step
            }
        };
        let opacity = match self.opacity {
            None => d.opacity,
            Some(v) if (20..=100).contains(&v) => v as u8,
            Some(v) => {
                errors.push(format!(
                    "{}: pin.opacity = {v} must be in 20..=100 (using {})",
                    path.display(),
                    d.opacity
                ));
                d.opacity
            }
        };
        let double_click = match self.double_click.as_deref() {
            None => d.double_click,
            Some(id) => PinDoubleClick::from_id(id).unwrap_or_else(|| {
                errors.push(format!(
                    "{}: unknown pin.double_click {id:?} (using none)",
                    path.display()
                ));
                d.double_click
            }),
        };
        let max_history = match self.max_history {
            None => d.max_history,
            Some(v) if (0..=200).contains(&v) => v as u16,
            Some(v) => {
                errors.push(format!(
                    "{}: pin.max_history = {v} must be in 0..=200 (using {})",
                    path.display(),
                    d.max_history
                ));
                d.max_history
            }
        };
        PinSettings {
            zoom_modifier,
            opacity_modifier,
            zoom_step,
            opacity,
            double_click,
            show_border: self.show_border.unwrap_or(d.show_border),
            shadow: self.shadow.unwrap_or(d.shadow),
            snap_to_edge: self.snap_to_edge.unwrap_or(d.snap_to_edge),
            snap_to_pins: self.snap_to_pins.unwrap_or(d.snap_to_pins),
            max_history,
        }
    }
}

impl PinSettings {
    pub(super) fn write_toml(
        &self,
        root: &mut toml::map::Map<String, toml::Value>,
        path: &Path,
    ) -> Result<(), String> {
        let section = root
            .entry("pin")
            .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
        let Some(t) = section.as_table_mut() else {
            return Err(format!(
                "{}: [pin] is not a table — refusing to overwrite",
                path.display()
            ));
        };
        t.insert(
            "zoom_modifier".into(),
            toml::Value::from(self.zoom_modifier.id()),
        );
        t.insert(
            "opacity_modifier".into(),
            toml::Value::from(self.opacity_modifier.id()),
        );
        t.insert(
            "zoom_step".into(),
            toml::Value::from(i64::from(self.zoom_step)),
        );
        t.insert("opacity".into(), toml::Value::from(i64::from(self.opacity)));
        t.insert(
            "double_click".into(),
            toml::Value::from(self.double_click.id()),
        );
        t.insert("show_border".into(), toml::Value::from(self.show_border));
        t.insert("shadow".into(), toml::Value::from(self.shadow));
        t.insert("snap_to_edge".into(), toml::Value::from(self.snap_to_edge));
        t.insert("snap_to_pins".into(), toml::Value::from(self.snap_to_pins));
        t.insert(
            "max_history".into(),
            toml::Value::from(i64::from(self.max_history)),
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enum_ids_round_trip() {
        for mode in SizeLabelMode::ALL {
            assert_eq!(SizeLabelMode::from_id(mode.id()), Some(mode));
        }
        for sz in LoupeSize::ALL {
            assert_eq!(LoupeSize::from_id(sz.id()), Some(sz));
        }
        for axis in ScrollAxisSetting::ALL {
            assert_eq!(ScrollAxisSetting::from_id(axis.id()), Some(axis));
        }
        for modif in PinZoomModifier::ALL {
            assert_eq!(PinZoomModifier::from_id(modif.id()), Some(modif));
        }
        for modif in PinOpacityModifier::ALL {
            assert_eq!(PinOpacityModifier::from_id(modif.id()), Some(modif));
        }
        for dc in PinDoubleClick::ALL {
            assert_eq!(PinDoubleClick::from_id(dc.id()), Some(dc));
        }
    }

    #[test]
    fn wheel_modifier_arbitration_prefers_explicit_chord_over_unmodified() {
        let plain = gpui_kit::Modifiers::default();
        let ctrl = gpui_kit::Modifiers {
            control: true,
            ..Default::default()
        };
        let alt = gpui_kit::Modifiers {
            alt: true,
            ..Default::default()
        };

        // Factory default: plain wheel zooms, Ctrl+wheel adjusts opacity.
        assert_eq!(
            resolve_pin_wheel_action(&plain, PinZoomModifier::None, PinOpacityModifier::Ctrl),
            Some(PinWheelAction::Zoom)
        );
        assert_eq!(
            resolve_pin_wheel_action(&ctrl, PinZoomModifier::None, PinOpacityModifier::Ctrl),
            Some(PinWheelAction::Opacity)
        );

        // Inverted: plain wheel adjusts opacity, Ctrl+wheel zooms.
        assert_eq!(
            resolve_pin_wheel_action(&plain, PinZoomModifier::Ctrl, PinOpacityModifier::None),
            Some(PinWheelAction::Opacity)
        );
        assert_eq!(
            resolve_pin_wheel_action(&ctrl, PinZoomModifier::Ctrl, PinOpacityModifier::None),
            Some(PinWheelAction::Zoom)
        );

        // Two explicit chords: bare wheel does nothing.
        assert_eq!(
            resolve_pin_wheel_action(&plain, PinZoomModifier::Ctrl, PinOpacityModifier::Alt),
            None
        );
        assert_eq!(
            resolve_pin_wheel_action(&alt, PinZoomModifier::Ctrl, PinOpacityModifier::Alt),
            Some(PinWheelAction::Opacity)
        );
    }
}
