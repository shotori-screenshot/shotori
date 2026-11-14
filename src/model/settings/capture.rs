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

/// `[scroll]` configuration block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScrollSettings {
    pub default_axis: ScrollAxisSetting,
    /// Maximum stitched length in logical pixels (`2_000..=100_000`).
    pub max_length: u32,
}

impl Default for ScrollSettings {
    fn default() -> Self {
        Self {
            default_axis: ScrollAxisSetting::Vertical,
            max_length: 20_000,
        }
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub(super) struct ScrollFile {
    default_axis: Option<String>,
    max_length: Option<i64>,
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
        ScrollSettings {
            default_axis,
            max_length,
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

    pub fn label(self) -> std::borrow::Cow<'static, str> {
        match self {
            Self::None => crate::i18n::t!("settings.pin.zoom_modifier.none"),
            Self::Ctrl => crate::i18n::t!("settings.pin.zoom_modifier.ctrl"),
            Self::Alt => crate::i18n::t!("settings.pin.zoom_modifier.alt"),
            Self::Shift => crate::i18n::t!("settings.pin.zoom_modifier.shift"),
        }
    }
}

/// `[pin]` configuration block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PinSettings {
    pub zoom_modifier: PinZoomModifier,
    /// Percentage zoom increment per wheel line (`5..=50`, default `12` = `1.12×`).
    pub zoom_step: u8,
    pub show_border: bool,
    pub shadow: bool,
    pub snap_to_edge: bool,
}

impl Default for PinSettings {
    fn default() -> Self {
        Self {
            zoom_modifier: PinZoomModifier::None,
            zoom_step: 12,
            show_border: true,
            shadow: true,
            snap_to_edge: false,
        }
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub(super) struct PinFile {
    zoom_modifier: Option<String>,
    zoom_step: Option<i64>,
    show_border: Option<bool>,
    shadow: Option<bool>,
    snap_to_edge: Option<bool>,
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
        PinSettings {
            zoom_modifier,
            zoom_step,
            show_border: self.show_border.unwrap_or(d.show_border),
            shadow: self.shadow.unwrap_or(d.shadow),
            snap_to_edge: self.snap_to_edge.unwrap_or(d.snap_to_edge),
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
            "zoom_step".into(),
            toml::Value::from(i64::from(self.zoom_step)),
        );
        t.insert("show_border".into(), toml::Value::from(self.show_border));
        t.insert("shadow".into(), toml::Value::from(self.shadow));
        t.insert("snap_to_edge".into(), toml::Value::from(self.snap_to_edge));
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
    }
}
