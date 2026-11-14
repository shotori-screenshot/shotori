//! `[export]` settings: default save directory, direct-save mode (issue #42),
//! dialog folder memory, filename template, image format, and compression.

use std::path::{Path, PathBuf};

use anyhow::Context as _;
use serde::Deserialize;

/// Factory filename stem (without extension). Supports `chrono` strftime
/// tokens (`%Y`, `%m`, `%d`, `%H`, `%M`, `%S`, `%3f`, …) plus `{w}` and
/// `{h}` for the exported physical width and height.
pub const DEFAULT_FILENAME_TEMPLATE: &str = "Shotori_%Y-%m-%d_%H-%M-%S_%3f";

/// Default display string shown when `save_dir` is unset (`None`).
pub const DEFAULT_SAVE_DIR_DISPLAY: &str = "~/Pictures/Shotori";

/// Supported file-save image formats. Clipboard and stdout exports stay PNG
/// (`image/png` is the Wayland clipboard contract).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ExportFormat {
    #[default]
    Png,
    Jpeg,
    Webp,
}

impl ExportFormat {
    pub const ALL: [Self; 3] = [Self::Png, Self::Jpeg, Self::Webp];

    pub const fn id(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpeg",
            Self::Webp => "webp",
        }
    }

    pub const fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::Webp => "webp",
        }
    }

    pub const fn filter_extensions(self) -> &'static [&'static str] {
        match self {
            Self::Png => &["png"],
            Self::Jpeg => &["jpg", "jpeg"],
            Self::Webp => &["webp"],
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        match id.trim().to_ascii_lowercase().as_str() {
            "png" => Some(Self::Png),
            "jpeg" | "jpg" => Some(Self::Jpeg),
            "webp" => Some(Self::Webp),
            _ => None,
        }
    }

    pub fn label(self) -> std::borrow::Cow<'static, str> {
        match self {
            Self::Png => crate::i18n::t!("settings.export.format.png"),
            Self::Jpeg => crate::i18n::t!("settings.export.format.jpeg"),
            Self::Webp => crate::i18n::t!("settings.export.format.webp"),
        }
    }
}

/// Compression effort for saved PNG files.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum PngCompression {
    Fast,
    #[default]
    Balanced,
    Best,
}

impl PngCompression {
    pub const ALL: [Self; 3] = [Self::Fast, Self::Balanced, Self::Best];

    pub const fn id(self) -> &'static str {
        match self {
            Self::Fast => "fast",
            Self::Balanced => "balanced",
            Self::Best => "best",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|c| c.id() == id.trim())
    }

    pub const fn to_png(self) -> png::Compression {
        match self {
            Self::Fast => png::Compression::Fast,
            Self::Balanced => png::Compression::Balanced,
            Self::Best => png::Compression::High,
        }
    }

    pub fn label(self) -> std::borrow::Cow<'static, str> {
        match self {
            Self::Fast => crate::i18n::t!("settings.export.png_compression.fast"),
            Self::Balanced => crate::i18n::t!("settings.export.png_compression.balanced"),
            Self::Best => crate::i18n::t!("settings.export.png_compression.best"),
        }
    }
}

/// `[export]` configuration block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportSettings {
    /// Custom default save directory (`None` = `~/Pictures/Shotori`).
    /// Stored un-expanded so `~/...` stays portable across home paths.
    pub save_dir: Option<String>,
    /// Save directly to `save_dir` on Save / Ctrl+S without opening the
    /// native file dialog (issue #42).
    pub auto_save: bool,
    /// When the file dialog is used, start in the directory chosen during
    /// the previous dialog save (persisted under `$XDG_STATE_HOME/shotori`).
    pub remember_last_dir: bool,
    /// Filename stem template (strftime + `{w}`, `{h}`).
    pub filename_template: String,
    /// Default file format for saved images.
    pub format: ExportFormat,
    /// DEFLATE effort when `format == ExportFormat::Png`.
    pub png_compression: PngCompression,
    /// Quality (`1..=100`) when `format == ExportFormat::Jpeg`.
    pub jpeg_quality: u8,
    /// Also copy the image to the clipboard whenever saving to disk.
    pub copy_on_save: bool,
}

impl Default for ExportSettings {
    fn default() -> Self {
        Self {
            save_dir: None,
            auto_save: false,
            remember_last_dir: false,
            filename_template: DEFAULT_FILENAME_TEMPLATE.to_owned(),
            format: ExportFormat::Png,
            png_compression: PngCompression::Balanced,
            jpeg_quality: 90,
            copy_on_save: false,
        }
    }
}

impl ExportSettings {
    /// Human-readable folder string for the Settings UI (`~/Pictures/Shotori`
    /// when using the factory default).
    pub fn display_save_dir(&self) -> &str {
        self.save_dir.as_deref().unwrap_or(DEFAULT_SAVE_DIR_DISPLAY)
    }

    /// Resolve the configured save directory (`~` expanded), falling back to
    /// `~/Pictures/Shotori` when `save_dir` is unset.
    pub fn resolved_save_dir(&self) -> anyhow::Result<PathBuf> {
        match self.save_dir.as_deref() {
            Some(custom) => expand_tilde_path(custom),
            None => crate::model::export::save_dir(),
        }
    }

    /// Normalize and set `save_dir` (`None` or blank resets to default).
    pub fn set_save_dir(&mut self, dir: Option<String>) {
        self.save_dir = dir.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty());
    }

    /// Validate and set `filename_template`; invalid input returns an
    /// explanation without mutating the current template.
    pub fn set_filename_template(&mut self, raw: &str) -> Result<(), &'static str> {
        let valid = validate_filename_template(raw)?;
        self.filename_template = valid;
        Ok(())
    }
}

/// Expand a leading `~` or `~/` against `$HOME`. Relative or absolute paths
/// without `~` pass through unchanged.
pub fn expand_tilde_path(raw: &str) -> anyhow::Result<PathBuf> {
    let trimmed = raw.trim();
    if trimmed == "~" {
        let home = std::env::var_os("HOME").context("HOME environment variable is not set")?;
        return Ok(PathBuf::from(home));
    }
    if let Some(rest) = trimmed.strip_prefix("~/") {
        let home = std::env::var_os("HOME").context("HOME environment variable is not set")?;
        return Ok(PathBuf::from(home).join(rest));
    }
    Ok(PathBuf::from(trimmed))
}

/// Validate a filename stem template: must be a single path component (no
/// `/`, `\`, NUL, or `..`) and contain valid `chrono` strftime specifiers.
pub fn validate_filename_template(raw: &str) -> Result<String, &'static str> {
    let trimmed = raw.trim().trim_matches('.');
    if trimmed.is_empty() {
        return Err("filename template cannot be empty");
    }
    if trimmed.contains('/') || trimmed.contains('\\') || trimmed.contains('\0') {
        return Err("filename template cannot contain path separators");
    }
    if trimmed == "." || trimmed == ".." {
        return Err("filename template cannot be a relative directory marker");
    }
    let probe = trimmed.replace("{w}", "1920").replace("{h}", "1080");
    if chrono::format::StrftimeItems::new(&probe)
        .any(|item| matches!(item, chrono::format::Item::Error))
    {
        return Err("filename template contains an invalid % format specifier");
    }
    Ok(trimmed.to_owned())
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub(super) struct ExportFile {
    save_dir: Option<String>,
    auto_save: Option<bool>,
    remember_last_dir: Option<bool>,
    filename_template: Option<String>,
    format: Option<String>,
    png_compression: Option<String>,
    jpeg_quality: Option<i64>,
    copy_on_save: Option<bool>,
}

impl ExportFile {
    pub(super) fn resolve(self, path: &Path, errors: &mut Vec<String>) -> ExportSettings {
        let defaults = ExportSettings::default();
        let save_dir = self
            .save_dir
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
            .and_then(|dir| {
                if dir.contains('\0') {
                    errors.push(format!(
                        "{}: export.save_dir contains NUL byte (using default)",
                        path.display()
                    ));
                    None
                } else {
                    Some(dir)
                }
            });

        let filename_template = match self.filename_template {
            None => defaults.filename_template,
            Some(raw) => match validate_filename_template(&raw) {
                Ok(valid) => valid,
                Err(reason) => {
                    errors.push(format!(
                        "{}: export.filename_template {raw:?}: {reason} (using {:?})",
                        path.display(),
                        DEFAULT_FILENAME_TEMPLATE
                    ));
                    defaults.filename_template
                }
            },
        };

        let format = match self.format.as_deref() {
            None => defaults.format,
            Some(id) => ExportFormat::from_id(id).unwrap_or_else(|| {
                errors.push(format!(
                    "{}: unknown export.format {id:?} (using png)",
                    path.display()
                ));
                defaults.format
            }),
        };

        let png_compression = match self.png_compression.as_deref() {
            None => defaults.png_compression,
            Some(id) => PngCompression::from_id(id).unwrap_or_else(|| {
                errors.push(format!(
                    "{}: unknown export.png_compression {id:?} (using balanced)",
                    path.display()
                ));
                defaults.png_compression
            }),
        };

        let jpeg_quality = match self.jpeg_quality {
            None => defaults.jpeg_quality,
            Some(q) if (1..=100).contains(&q) => q as u8,
            Some(q) => {
                errors.push(format!(
                    "{}: export.jpeg_quality = {q} must be in 1..=100 (using {})",
                    path.display(),
                    defaults.jpeg_quality
                ));
                defaults.jpeg_quality
            }
        };

        ExportSettings {
            save_dir,
            auto_save: self.auto_save.unwrap_or(defaults.auto_save),
            remember_last_dir: self.remember_last_dir.unwrap_or(defaults.remember_last_dir),
            filename_template,
            format,
            png_compression,
            jpeg_quality,
            copy_on_save: self.copy_on_save.unwrap_or(defaults.copy_on_save),
        }
    }
}

impl ExportSettings {
    pub(super) fn write_toml(
        &self,
        root: &mut toml::map::Map<String, toml::Value>,
        path: &Path,
    ) -> Result<(), String> {
        let section = root
            .entry("export")
            .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
        let Some(export) = section.as_table_mut() else {
            return Err(format!(
                "{}: [export] is not a table — refusing to overwrite",
                path.display()
            ));
        };
        match &self.save_dir {
            Some(dir) => {
                export.insert("save_dir".into(), toml::Value::from(dir.as_str()));
            }
            None => {
                export.remove("save_dir");
            }
        }
        export.insert("auto_save".into(), toml::Value::from(self.auto_save));
        export.insert(
            "remember_last_dir".into(),
            toml::Value::from(self.remember_last_dir),
        );
        export.insert(
            "filename_template".into(),
            toml::Value::from(self.filename_template.as_str()),
        );
        export.insert("format".into(), toml::Value::from(self.format.id()));
        export.insert(
            "png_compression".into(),
            toml::Value::from(self.png_compression.id()),
        );
        export.insert(
            "jpeg_quality".into(),
            toml::Value::from(i64::from(self.jpeg_quality)),
        );
        export.insert("copy_on_save".into(), toml::Value::from(self.copy_on_save));
        Ok(())
    }
}

/// State file recording the last directory chosen in `rfd::FileDialog`
/// (`$XDG_STATE_HOME/shotori/last-save-dir`, else `~/.local/state/shotori/last-save-dir`).
/// Stored outside `settings.toml` so saving a screenshot never rewrites the
/// user's configuration file.
fn last_dir_state_path() -> Option<PathBuf> {
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))
        .map(|base| base.join(crate::APP_ID).join("last-save-dir"))
}

pub fn read_last_save_dir() -> Option<PathBuf> {
    let state_file = last_dir_state_path()?;
    let raw = std::fs::read_to_string(state_file).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let dir = PathBuf::from(trimmed);
    dir.is_dir().then_some(dir)
}

pub fn write_last_save_dir(dir: &Path) {
    let Some(state_file) = last_dir_state_path() else {
        return;
    };
    if let Some(parent) = state_file.parent()
        && let Err(e) = std::fs::create_dir_all(parent)
    {
        eprintln!("[shotori] state dir {}: {e}", parent.display());
        return;
    }
    if let Err(e) = std::fs::write(&state_file, dir.to_string_lossy().as_bytes()) {
        eprintln!("[shotori] state write {}: {e}", state_file.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_and_compression_ids_round_trip() {
        for fmt in ExportFormat::ALL {
            assert_eq!(ExportFormat::from_id(fmt.id()), Some(fmt));
        }
        assert_eq!(ExportFormat::from_id("jpg"), Some(ExportFormat::Jpeg));
        assert_eq!(ExportFormat::from_id("JPG"), Some(ExportFormat::Jpeg));
        assert_eq!(ExportFormat::from_id("tiff"), None);

        for comp in PngCompression::ALL {
            assert_eq!(PngCompression::from_id(comp.id()), Some(comp));
        }
        assert_eq!(PngCompression::from_id("ultra"), None);
    }

    #[test]
    fn filename_template_validation_guards_separators_and_bad_strftime() {
        assert_eq!(
            validate_filename_template("Shotori_%Y-%m-%d_{w}x{h}").unwrap(),
            "Shotori_%Y-%m-%d_{w}x{h}"
        );
        assert!(validate_filename_template("").is_err());
        assert!(validate_filename_template("   ").is_err());
        assert!(validate_filename_template("../escape").is_err());
        assert!(validate_filename_template("sub/file").is_err());
        assert!(validate_filename_template("sub\\file").is_err());
        assert!(validate_filename_template("trailing_%").is_err());
    }

    #[test]
    fn tilde_expansion_handles_home_and_plain_paths() {
        let plain = expand_tilde_path("/tmp/shots").unwrap();
        assert_eq!(plain, PathBuf::from("/tmp/shots"));
        if let Some(home) = std::env::var_os("HOME") {
            assert_eq!(
                expand_tilde_path("~/Desktop/Shots").unwrap(),
                PathBuf::from(&home).join("Desktop/Shots")
            );
            assert_eq!(expand_tilde_path("~").unwrap(), PathBuf::from(home));
        }
    }
}
