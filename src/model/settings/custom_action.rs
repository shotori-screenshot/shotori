//! User-defined custom toolbar actions (`[[custom_actions]]` in `settings.toml`
//! plus drop-in manifests under `<config_dir>/actions/`).
//!
//! Pure configuration and discovery logic — zero Wayland/GPUI or process
//! execution here (execution lives in [`crate::custom_action`]).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::Deserialize;

use super::export::expand_tilde_path;

/// Prefix used inside `[toolbar] items` to reference a custom action by id.
pub const CUSTOM_TOOLBAR_PREFIX: &str = "custom:";

/// Subdirectory inside the config dir scanned for drop-in action manifests.
pub const ACTIONS_SUBDIR: &str = "actions";

/// Subdirectory inside the config dir for user-imported SVG icons.
pub const ICONS_SUBDIR: &str = "icons";

pub const DEFAULT_TIMEOUT_SECS: u16 = 30;
pub const MIN_TIMEOUT_SECS: u16 = 1;
pub const MAX_TIMEOUT_SECS: u16 = 300;
const MAX_ID_LEN: usize = 48;

/// Built-in Lucide icon identifiers accepted directly in `CustomAction::icon`.
/// Kept in sync with `ui::toolbar::custom_builtin_icon`.
pub const BUILTIN_ICON_IDS: &[&str] = &[
    "sparkles",
    "cloud-upload",
    "upload",
    "terminal",
    "code",
    "bot",
    "globe",
    "send",
    "share-2",
    "link",
    "webhook",
    "zap",
    "wand-sparkles",
    "plug",
    "rocket",
    "wrench",
    "image",
    "folder",
    "external-link",
    "play",
];

/// How Shotori delivers the selection payload to a custom action script.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum CustomActionInput {
    /// Encode to a temporary image file (`SHOTORI_IMAGE_PATH`), deleted after exit.
    #[default]
    File,
    /// Pipe encoded image bytes directly to the process's stdin.
    Stdin,
    /// Run built-in OCR first; pass UTF-8 text via stdin and `SHOTORI_TEXT_PATH`.
    OcrText,
    /// Pass no payload (only selection/output metadata environment variables).
    None,
}

impl CustomActionInput {
    pub const ALL: [Self; 4] = [Self::File, Self::Stdin, Self::OcrText, Self::None];

    pub const fn id(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Stdin => "stdin",
            Self::OcrText => "ocr_text",
            Self::None => "none",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|v| v.id() == id)
    }

    pub fn label(self) -> std::borrow::Cow<'static, str> {
        match self {
            Self::File => crate::i18n::t!("settings.custom_actions.input.file"),
            Self::Stdin => crate::i18n::t!("settings.custom_actions.input.stdin"),
            Self::OcrText => crate::i18n::t!("settings.custom_actions.input.ocr_text"),
            Self::None => crate::i18n::t!("settings.custom_actions.input.none"),
        }
    }
}

/// Which pixel buffer to feed into a custom action.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum CustomActionSource {
    /// Cropped selection with in-canvas annotations composited.
    #[default]
    Annotated,
    /// Raw captured pixels without annotations.
    Raw,
}

impl CustomActionSource {
    pub const ALL: [Self; 2] = [Self::Annotated, Self::Raw];

    pub const fn id(self) -> &'static str {
        match self {
            Self::Annotated => "annotated",
            Self::Raw => "raw",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|v| v.id() == id)
    }

    pub fn label(self) -> std::borrow::Cow<'static, str> {
        match self {
            Self::Annotated => crate::i18n::t!("settings.custom_actions.source.annotated"),
            Self::Raw => crate::i18n::t!("settings.custom_actions.source.raw"),
        }
    }
}

/// How Shotori handles a custom action's `stdout` on exit code 0.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum CustomActionOutput {
    /// Copy trimmed UTF-8 stdout to clipboard and send a notification.
    #[default]
    CopyText,
    /// Decode image bytes from stdout and copy as PNG to clipboard.
    CopyImage,
    /// Decode image bytes from stdout and open as a floating desktop Pin.
    Pin,
    /// Show trimmed UTF-8 stdout as a desktop notification body.
    Notify,
    /// Parse a JSON response object from stdout (`clipboard_text`, `notify_*`, `open_url`, ...).
    Json,
    /// Side-effect only; ignore stdout on success (still notify on non-zero exit).
    Ignore,
}

impl CustomActionOutput {
    pub const ALL: [Self; 6] = [
        Self::CopyText,
        Self::CopyImage,
        Self::Pin,
        Self::Notify,
        Self::Json,
        Self::Ignore,
    ];

    pub const fn id(self) -> &'static str {
        match self {
            Self::CopyText => "copy_text",
            Self::CopyImage => "copy_image",
            Self::Pin => "pin",
            Self::Notify => "notify",
            Self::Json => "json",
            Self::Ignore => "ignore",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|v| v.id() == id)
    }

    pub fn label(self) -> std::borrow::Cow<'static, str> {
        match self {
            Self::CopyText => crate::i18n::t!("settings.custom_actions.output.copy_text"),
            Self::CopyImage => crate::i18n::t!("settings.custom_actions.output.copy_image"),
            Self::Pin => crate::i18n::t!("settings.custom_actions.output.pin"),
            Self::Notify => crate::i18n::t!("settings.custom_actions.output.notify"),
            Self::Json => crate::i18n::t!("settings.custom_actions.output.json"),
            Self::Ignore => crate::i18n::t!("settings.custom_actions.output.ignore"),
        }
    }
}

/// Where a custom action definition was loaded from.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub enum CustomActionOrigin {
    /// Defined inline in `settings.toml` (`[[custom_actions]]`); editable in Settings UI.
    #[default]
    Settings,
    /// Discovered from a drop-in manifest under `<config_dir>/actions/`.
    DropIn(PathBuf),
}

impl CustomActionOrigin {
    pub fn is_drop_in(&self) -> bool {
        matches!(self, Self::DropIn(_))
    }

    pub fn manifest_path(&self) -> Option<&Path> {
        match self {
            Self::Settings => None,
            Self::DropIn(path) => Some(path.as_path()),
        }
    }
}

/// One user-configured custom toolbar action.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct CustomAction {
    pub id: String,
    pub name: String,
    /// Built-in icon name (see [`BUILTIN_ICON_IDS`]) or an `.svg` file path.
    pub icon: String,
    /// Shell command executed via `/bin/sh -c <command>`.
    pub command: String,
    /// Optional working directory (drop-in actions default to their manifest directory).
    pub working_dir: Option<PathBuf>,
    pub input: CustomActionInput,
    pub source: CustomActionSource,
    pub output: CustomActionOutput,
    /// Close overlays immediately and show a non-blocking top-center OSD while running.
    pub close_overlay: bool,
    /// Execution timeout in seconds (`MIN_TIMEOUT_SECS..=MAX_TIMEOUT_SECS`).
    pub timeout_secs: u16,
    /// Optional overlay keyboard shortcut chord (e.g. `"ctrl-u"`).
    pub shortcut: Option<String>,
    pub origin: CustomActionOrigin,
}

impl CustomAction {
    /// Stable toolbar row identifier (`"custom:<id>"`).
    pub fn toolbar_id(&self) -> String {
        format!("{CUSTOM_TOOLBAR_PREFIX}{}", self.id)
    }

    /// True if `self.icon` names one of the curated built-in Lucide presets.
    pub fn is_builtin_icon(&self) -> bool {
        let icon = self.icon.trim();
        !icon.is_empty() && BUILTIN_ICON_IDS.contains(&icon)
    }

    /// Resolve `self.icon` to an SVG filesystem path, returning `None` when
    /// blank or naming a built-in icon preset.
    ///
    /// Relative paths resolve against:
    /// 1. The drop-in manifest's parent directory (for drop-in actions),
    /// 2. `<config_dir>/icons/` (for inline `settings.toml` actions).
    pub fn resolve_icon_path(&self, config_dir: Option<&Path>) -> Option<PathBuf> {
        let raw = self.icon.trim();
        if raw.is_empty() || BUILTIN_ICON_IDS.contains(&raw) {
            return None;
        }
        let expanded = expand_tilde_path(raw).unwrap_or_else(|_| PathBuf::from(raw));
        if expanded.is_absolute() {
            return Some(expanded);
        }
        match &self.origin {
            CustomActionOrigin::DropIn(manifest) => {
                manifest.parent().map(|dir| dir.join(&expanded))
            }
            CustomActionOrigin::Settings => config_dir.map(|dir| icons_dir(dir).join(&expanded)),
        }
    }

    /// Resolve the command working directory, if configured or implicit from a drop-in folder.
    pub fn resolved_working_dir(&self) -> Option<PathBuf> {
        if let Some(dir) = &self.working_dir {
            let raw = dir.to_string_lossy();
            let expanded = expand_tilde_path(&raw).unwrap_or_else(|_| dir.clone());
            if expanded.is_absolute() {
                return Some(expanded);
            }
            if let CustomActionOrigin::DropIn(manifest) = &self.origin {
                return manifest.parent().map(|parent| parent.join(expanded));
            }
            return Some(expanded);
        }
        match &self.origin {
            CustomActionOrigin::DropIn(manifest) => manifest.parent().map(Path::to_path_buf),
            CustomActionOrigin::Settings => None,
        }
    }
}

/// Drop-in actions directory (`<config_dir>/actions`).
pub fn actions_dir(config_dir: &Path) -> PathBuf {
    config_dir.join(ACTIONS_SUBDIR)
}

/// Imported custom SVG icons directory (`<config_dir>/icons`).
pub fn icons_dir(config_dir: &Path) -> PathBuf {
    config_dir.join(ICONS_SUBDIR)
}

/// Validate a custom action slug id: 1..=48 bytes, starts with ASCII
/// alphanumeric, remaining chars `[a-z0-9_-]`.
pub fn is_valid_custom_action_id(id: &str) -> bool {
    if id.is_empty() || id.len() > MAX_ID_LEN {
        return false;
    }
    let bytes = id.as_bytes();
    if !(bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit()) {
        return false;
    }
    bytes
        .iter()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-' || *b == b'_')
}

/// Generate a unique valid slug id from a human display name.
pub fn slugify_custom_action_id(name: &str, existing: &[CustomAction]) -> String {
    let mut base = String::new();
    let mut prev_dash = false;
    for ch in name.chars() {
        let lower = ch.to_ascii_lowercase();
        if lower.is_ascii_lowercase() || lower.is_ascii_digit() {
            base.push(lower);
            prev_dash = false;
        } else if (ch == '-' || ch == '_' || ch.is_whitespace()) && !base.is_empty() && !prev_dash {
            base.push('-');
            prev_dash = true;
        }
    }
    while base.ends_with('-') {
        base.pop();
    }
    if base.is_empty() {
        base = "action".to_owned();
    }
    base.truncate(MAX_ID_LEN.saturating_sub(6));
    let taken = |cand: &str| existing.iter().any(|a| a.id == cand);
    if !taken(&base) {
        return base;
    }
    for n in 2..=9999 {
        let candidate = format!("{base}-{n}");
        if !taken(&candidate) {
            return candidate;
        }
    }
    format!("action-{}", existing.len() + 1)
}

/// Raw serde representation for `[[custom_actions]]` and drop-in `.toml` files.
#[derive(Deserialize, Default, Debug)]
#[serde(default)]
pub(super) struct CustomActionFile {
    pub(super) id: Option<String>,
    pub(super) name: Option<String>,
    pub(super) icon: Option<String>,
    pub(super) command: Option<String>,
    pub(super) working_dir: Option<String>,
    pub(super) input: Option<String>,
    pub(super) source: Option<String>,
    pub(super) output: Option<String>,
    pub(super) close_overlay: Option<bool>,
    pub(super) timeout_secs: Option<u64>,
    pub(super) shortcut: Option<String>,
}

/// Parse one raw action table into a validated [`CustomAction`].
/// `fallback_id` is used for drop-in manifests (filename stem or parent directory name).
fn resolve_one(
    raw: CustomActionFile,
    fallback_id: Option<&str>,
    origin: CustomActionOrigin,
    source_path: &Path,
    errors: &mut Vec<String>,
) -> Option<CustomAction> {
    let raw_id = raw
        .id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or(fallback_id)
        .unwrap_or("");
    if !is_valid_custom_action_id(raw_id) {
        errors.push(format!(
            "{}: custom action has invalid id {raw_id:?} (expected lowercase [a-z0-9_-], starting with a-z/0-9; skipped)",
            source_path.display()
        ));
        return None;
    }
    let id = raw_id.to_owned();
    let command = raw
        .command
        .as_deref()
        .map(str::trim)
        .unwrap_or("")
        .to_owned();
    if command.is_empty() {
        errors.push(format!(
            "{}: custom action {id:?} has an empty command (skipped)",
            source_path.display()
        ));
        return None;
    }
    let name = raw
        .name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(&id)
        .to_owned();
    let icon = raw
        .icon
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("sparkles")
        .to_owned();
    let working_dir = raw
        .working_dir
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(PathBuf::from);
    let input = match raw.input.as_deref() {
        None => CustomActionInput::default(),
        Some(val) => match CustomActionInput::from_id(val) {
            Some(v) => v,
            None => {
                errors.push(format!(
                    "{}: custom action {id:?}: unknown input {val:?} (using file)",
                    source_path.display()
                ));
                CustomActionInput::default()
            }
        },
    };
    let source = match raw.source.as_deref() {
        None => CustomActionSource::default(),
        Some(val) => match CustomActionSource::from_id(val) {
            Some(v) => v,
            None => {
                errors.push(format!(
                    "{}: custom action {id:?}: unknown source {val:?} (using annotated)",
                    source_path.display()
                ));
                CustomActionSource::default()
            }
        },
    };
    let output = match raw.output.as_deref() {
        None => CustomActionOutput::default(),
        Some(val) => match CustomActionOutput::from_id(val) {
            Some(v) => v,
            None => {
                errors.push(format!(
                    "{}: custom action {id:?}: unknown output {val:?} (using copy_text)",
                    source_path.display()
                ));
                CustomActionOutput::default()
            }
        },
    };
    let close_overlay = raw.close_overlay.unwrap_or(true);
    let timeout_secs = match raw.timeout_secs {
        None => DEFAULT_TIMEOUT_SECS,
        Some(v) if (u64::from(MIN_TIMEOUT_SECS)..=u64::from(MAX_TIMEOUT_SECS)).contains(&v) => {
            v as u16
        }
        Some(v) => {
            errors.push(format!(
                "{}: custom action {id:?}: timeout_secs {v} out of range ({MIN_TIMEOUT_SECS}..={MAX_TIMEOUT_SECS}; using {DEFAULT_TIMEOUT_SECS})",
                source_path.display()
            ));
            DEFAULT_TIMEOUT_SECS
        }
    };
    let shortcut = raw
        .shortcut
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);

    Some(CustomAction {
        id,
        name,
        icon,
        command,
        working_dir,
        input,
        source,
        output,
        close_overlay,
        timeout_secs,
        shortcut,
        origin,
    })
}

/// Resolve inline `[[custom_actions]]` entries from `settings.toml`.
pub(super) fn resolve_inline(
    raw_list: Vec<CustomActionFile>,
    settings_path: &Path,
    errors: &mut Vec<String>,
) -> Vec<CustomAction> {
    let mut actions = Vec::with_capacity(raw_list.len());
    let mut seen = BTreeSet::new();
    for raw in raw_list {
        let Some(action) = resolve_one(
            raw,
            None,
            CustomActionOrigin::Settings,
            settings_path,
            errors,
        ) else {
            continue;
        };
        if !seen.insert(action.id.clone()) {
            errors.push(format!(
                "{}: duplicate custom action id {:?} (kept the first)",
                settings_path.display(),
                action.id
            ));
            continue;
        }
        actions.push(action);
    }
    actions
}

/// Discover drop-in manifests under `<config_dir>/actions/`:
/// - `<actions_dir>/<name>.toml` (default id = `<name>`)
/// - `<actions_dir>/<dir>/action.toml` (default id = `<dir>`)
///
/// Entries are sorted deterministically by path; inline actions in `existing_ids`
/// take precedence (duplicates emit a warning and are skipped).
pub(super) fn discover_drop_in(
    dir: &Path,
    existing_ids: &BTreeSet<String>,
    errors: &mut Vec<String>,
) -> Vec<CustomAction> {
    let entries = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => {
            errors.push(format!("{}: {e}", dir.display()));
            return Vec::new();
        }
    };

    let mut manifests: Vec<(PathBuf, Option<String>)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_file()
            && path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("toml"))
        {
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .map(|s| s.to_ascii_lowercase());
            manifests.push((path, stem));
        } else if path.is_dir() {
            let manifest = path.join("action.toml");
            if manifest.is_file() {
                let dir_name = path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .map(|s| s.to_ascii_lowercase());
                manifests.push((manifest, dir_name));
            }
        }
    }
    manifests.sort_by(|a, b| a.0.cmp(&b.0));

    let mut seen = existing_ids.clone();
    let mut actions = Vec::new();
    for (manifest_path, fallback_id) in manifests {
        let text = match std::fs::read_to_string(&manifest_path) {
            Ok(t) => t,
            Err(e) => {
                errors.push(format!("{}: {e}", manifest_path.display()));
                continue;
            }
        };
        let raw: CustomActionFile = match toml::from_str(&text) {
            Ok(parsed) => parsed,
            Err(e) => {
                errors.push(format!("{}: {e}", manifest_path.display()));
                continue;
            }
        };
        let Some(action) = resolve_one(
            raw,
            fallback_id.as_deref(),
            CustomActionOrigin::DropIn(manifest_path.clone()),
            &manifest_path,
            errors,
        ) else {
            continue;
        };
        if !seen.insert(action.id.clone()) {
            errors.push(format!(
                "{}: custom action id {:?} conflicts with an earlier definition (skipped)",
                manifest_path.display(),
                action.id
            ));
            continue;
        }
        actions.push(action);
    }
    actions
}

/// Validate custom action shortcuts against built-in `[keys]`, reserved keys,
/// and other custom actions. Invalid or conflicting chords degrade to `None`
/// with a warning so the action remains usable from the toolbar.
pub(super) fn validate_shortcuts(
    actions: &mut [CustomAction],
    builtin_keys: &BTreeMap<String, String>,
    settings_path: &Path,
    errors: &mut Vec<String>,
) {
    let mut claimed: BTreeMap<String, String> = builtin_keys
        .iter()
        .map(|(id, chord)| (chord.clone(), id.clone()))
        .collect();

    for action in actions.iter_mut() {
        let Some(chord) = action.shortcut.clone() else {
            continue;
        };
        let src = action.origin.manifest_path().unwrap_or(settings_path);
        if !crate::actions::keystroke_valid(&chord) {
            errors.push(format!(
                "{}: custom action {:?} shortcut {chord:?} is not a valid keystroke (cleared)",
                src.display(),
                action.id
            ));
            action.shortcut = None;
            continue;
        }
        if crate::actions::RESERVED_KEYS.contains(&chord.as_str()) {
            errors.push(format!(
                "{}: custom action {:?} shortcut {chord:?} is reserved (cleared)",
                src.display(),
                action.id
            ));
            action.shortcut = None;
            continue;
        }
        if let Some(other) = claimed.get(&chord) {
            errors.push(format!(
                "{}: custom action {:?} shortcut {chord:?} conflicts with {other:?} (cleared)",
                src.display(),
                action.id
            ));
            action.shortcut = None;
            continue;
        }
        claimed.insert(chord, format!("{CUSTOM_TOOLBAR_PREFIX}{}", action.id));
    }
}

/// Serialize inline (`CustomActionOrigin::Settings`) actions into the top-level
/// `custom_actions` array in `settings.toml`. Drop-in actions are never copied
/// into `settings.toml`.
pub(super) fn write_toml(
    actions: &[CustomAction],
    table: &mut toml::map::Map<String, toml::Value>,
) {
    let inline: Vec<toml::Value> = actions
        .iter()
        .filter(|a| !a.origin.is_drop_in())
        .map(action_to_toml)
        .collect();
    if inline.is_empty() {
        table.remove("custom_actions");
    } else {
        table.insert("custom_actions".into(), toml::Value::Array(inline));
    }
}

fn action_to_toml(action: &CustomAction) -> toml::Value {
    let mut map = toml::map::Map::new();
    map.insert("id".into(), toml::Value::from(action.id.as_str()));
    map.insert("name".into(), toml::Value::from(action.name.as_str()));
    map.insert("icon".into(), toml::Value::from(action.icon.as_str()));
    map.insert("command".into(), toml::Value::from(action.command.as_str()));
    if let Some(dir) = &action.working_dir {
        map.insert(
            "working_dir".into(),
            toml::Value::from(dir.to_string_lossy().as_ref()),
        );
    }
    map.insert("input".into(), toml::Value::from(action.input.id()));
    map.insert("source".into(), toml::Value::from(action.source.id()));
    map.insert("output".into(), toml::Value::from(action.output.id()));
    map.insert(
        "close_overlay".into(),
        toml::Value::from(action.close_overlay),
    );
    map.insert(
        "timeout_secs".into(),
        toml::Value::from(i64::from(action.timeout_secs)),
    );
    if let Some(shortcut) = &action.shortcut {
        map.insert("shortcut".into(), toml::Value::from(shortcut.as_str()));
    }
    toml::Value::Table(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_action_id_validation_rules() {
        assert!(is_valid_custom_action_id("upload-r2"));
        assert!(is_valid_custom_action_id("picgo_1"));
        assert!(is_valid_custom_action_id("0-fast"));
        assert!(!is_valid_custom_action_id(""));
        assert!(!is_valid_custom_action_id("-leading-dash"));
        assert!(!is_valid_custom_action_id("_leading_under"));
        assert!(!is_valid_custom_action_id("UpperCase"));
        assert!(!is_valid_custom_action_id("has space"));
        assert!(!is_valid_custom_action_id("a/b"));
        assert!(!is_valid_custom_action_id(&"a".repeat(MAX_ID_LEN + 1)));
    }

    #[test]
    fn slugify_produces_unique_valid_ids() {
        let mut list = Vec::new();
        let id1 = slugify_custom_action_id("Upload to R2!", &list);
        assert_eq!(id1, "upload-to-r2");
        assert!(is_valid_custom_action_id(&id1));
        list.push(CustomAction {
            id: id1.clone(),
            name: "Upload to R2!".into(),
            icon: "cloud-upload".into(),
            command: "echo hi".into(),
            working_dir: None,
            input: CustomActionInput::File,
            source: CustomActionSource::Annotated,
            output: CustomActionOutput::CopyText,
            close_overlay: true,
            timeout_secs: DEFAULT_TIMEOUT_SECS,
            shortcut: None,
            origin: CustomActionOrigin::Settings,
        });
        let id2 = slugify_custom_action_id("Upload to R2!", &list);
        assert_eq!(id2, "upload-to-r2-2");

        // Non-ASCII names fall back cleanly to "action"
        let id3 = slugify_custom_action_id("上传图床", &list);
        assert_eq!(id3, "action");
        assert!(is_valid_custom_action_id(&id3));
    }

    #[test]
    fn resolve_icon_path_handles_builtin_relative_and_drop_in() {
        let cfg = Path::new("/tmp/shotori-cfg");
        let builtin = CustomAction {
            id: "a".into(),
            name: "A".into(),
            icon: "cloud-upload".into(),
            command: "true".into(),
            working_dir: None,
            input: CustomActionInput::File,
            source: CustomActionSource::Annotated,
            output: CustomActionOutput::CopyText,
            close_overlay: true,
            timeout_secs: 30,
            shortcut: None,
            origin: CustomActionOrigin::Settings,
        };
        assert_eq!(builtin.resolve_icon_path(Some(cfg)), None);

        let mut inline_rel = builtin.clone();
        inline_rel.icon = "picgo.svg".into();
        assert_eq!(
            inline_rel.resolve_icon_path(Some(cfg)),
            Some(cfg.join("icons").join("picgo.svg"))
        );

        let mut drop_in = builtin;
        drop_in.icon = "icon.svg".into();
        drop_in.origin = CustomActionOrigin::DropIn(cfg.join("actions/r2/action.toml"));
        assert_eq!(
            drop_in.resolve_icon_path(Some(cfg)),
            Some(cfg.join("actions/r2/icon.svg"))
        );
    }

    #[test]
    fn drop_in_discovery_supports_single_file_and_directory_bundles() {
        let dir = tempfile::tempdir().unwrap();
        let actions = actions_dir(dir.path());
        std::fs::create_dir_all(actions.join("r2-upload")).unwrap();
        std::fs::write(
            actions.join("r2-upload/action.toml"),
            "name = \"Upload R2\"\nicon = \"icon.svg\"\ncommand = \"./upload.sh\"\noutput = \"copy_text\"\n",
        )
        .unwrap();
        std::fs::write(
            actions.join("translate.toml"),
            "name = \"Translate\"\ncommand = \"trans -b\"\ninput = \"ocr_text\"\noutput = \"notify\"\n",
        )
        .unwrap();
        // Corrupt file degrades to warning without dropping valid actions
        std::fs::write(actions.join("broken.toml"), "not = [toml").unwrap();

        let mut errors = Vec::new();
        let found = discover_drop_in(&actions, &BTreeSet::new(), &mut errors);
        assert_eq!(found.len(), 2);
        assert_eq!(errors.len(), 1);

        let r2 = found.iter().find(|a| a.id == "r2-upload").unwrap();
        assert_eq!(r2.name, "Upload R2");
        assert_eq!(r2.resolved_working_dir(), Some(actions.join("r2-upload")));
        assert_eq!(
            r2.resolve_icon_path(Some(dir.path())),
            Some(actions.join("r2-upload/icon.svg"))
        );

        let tr = found.iter().find(|a| a.id == "translate").unwrap();
        assert_eq!(tr.input, CustomActionInput::OcrText);
        assert_eq!(tr.output, CustomActionOutput::Notify);
    }

    #[test]
    fn invalid_enums_and_conflicting_shortcuts_degrade_cleanly() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        let raw = vec![CustomActionFile {
            id: Some("bad-opts".into()),
            name: Some("Bad".into()),
            icon: None,
            command: Some("echo ok".into()),
            working_dir: None,
            input: Some("bogus".into()),
            source: Some("bogus".into()),
            output: Some("bogus".into()),
            close_overlay: None,
            timeout_secs: Some(999),
            shortcut: Some("ctrl-c".into()), // clashes with built-in copy
        }];
        let mut errors = Vec::new();
        let mut actions = resolve_inline(raw, &path, &mut errors);
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].input, CustomActionInput::File);
        assert_eq!(actions[0].source, CustomActionSource::Annotated);
        assert_eq!(actions[0].output, CustomActionOutput::CopyText);
        assert_eq!(actions[0].timeout_secs, DEFAULT_TIMEOUT_SECS);
        assert_eq!(errors.len(), 4);

        let builtin = crate::actions::default_keys();
        validate_shortcuts(&mut actions, &builtin, &path, &mut errors);
        assert_eq!(actions[0].shortcut, None);
        assert_eq!(errors.len(), 5);
    }
}
