//! TOML theme settings. `base` picks a complete built-in palette
//! (surfaces and control states switch together by construction); any
//! other key is a single-field override on top of that base — the
//! Appearance editor writes this file, `--theme FILE` / the XDG
//! fallback read it.
use super::{ColorKey, Theme};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Every solid-color field is an optional hex override keyed by
/// [`ColorKey::key`] — serde sees flat fields, the `ColorKey` registry
/// is the one place that knows which strings exist.
#[derive(Deserialize, Serialize, Default, Debug, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct ThemeFile {
    pub base: Option<String>,
    pub dim_color: Option<String>,
    pub accent: Option<String>,
    pub pin_border: Option<String>,
    pub chip_bg: Option<String>,
    pub hint_text: Option<String>,
    pub btn_text: Option<String>,
    pub btn_hover_bg: Option<String>,
    pub toolbar_bg: Option<String>,
    pub toolbar_text: Option<String>,
    pub toolbar_border: Option<String>,
    pub toolbar_hover: Option<String>,
    pub toolbar_selected: Option<String>,
    pub swatch_border: Option<String>,
    pub dim_opacity: Option<f32>,
    /// DEPRECATED (2026-10): the swatch board moved to settings.toml
    /// (`[toolbar] colors`). Parsed so old files keep loading
    /// (deny_unknown_fields would reject them otherwise) but no longer
    /// applied — see the migration note in [`super::load::init`].
    pub annotation_colors: Option<Vec<String>>,
}

impl ThemeFile {
    /// The override for one color field, if set.
    pub fn color(&self, key: ColorKey) -> Option<&String> {
        match key {
            ColorKey::DimColor => self.dim_color.as_ref(),
            ColorKey::Accent => self.accent.as_ref(),
            ColorKey::PinBorder => self.pin_border.as_ref(),
            ColorKey::ChipBg => self.chip_bg.as_ref(),
            ColorKey::HintText => self.hint_text.as_ref(),
            ColorKey::BtnText => self.btn_text.as_ref(),
            ColorKey::BtnHoverBg => self.btn_hover_bg.as_ref(),
            ColorKey::ToolbarBg => self.toolbar_bg.as_ref(),
            ColorKey::ToolbarText => self.toolbar_text.as_ref(),
            ColorKey::ToolbarBorder => self.toolbar_border.as_ref(),
            ColorKey::ToolbarHover => self.toolbar_hover.as_ref(),
            ColorKey::ToolbarSelected => self.toolbar_selected.as_ref(),
            ColorKey::SwatchBorder => self.swatch_border.as_ref(),
        }
    }

    /// Set (or clear with `None`) one color override — the Appearance
    /// editor's write path.
    pub fn set_color(&mut self, key: ColorKey, hex_value: Option<String>) {
        let slot = match key {
            ColorKey::DimColor => &mut self.dim_color,
            ColorKey::Accent => &mut self.accent,
            ColorKey::PinBorder => &mut self.pin_border,
            ColorKey::ChipBg => &mut self.chip_bg,
            ColorKey::HintText => &mut self.hint_text,
            ColorKey::BtnText => &mut self.btn_text,
            ColorKey::BtnHoverBg => &mut self.btn_hover_bg,
            ColorKey::ToolbarBg => &mut self.toolbar_bg,
            ColorKey::ToolbarText => &mut self.toolbar_text,
            ColorKey::ToolbarBorder => &mut self.toolbar_border,
            ColorKey::ToolbarHover => &mut self.toolbar_hover,
            ColorKey::ToolbarSelected => &mut self.toolbar_selected,
            ColorKey::SwatchBorder => &mut self.swatch_border,
        };
        *slot = hex_value;
    }

    /// True when no field is overridden — the base palette alone.
    pub fn is_base_only(&self) -> bool {
        self.dim_color.is_none()
            && self.accent.is_none()
            && self.pin_border.is_none()
            && self.chip_bg.is_none()
            && self.hint_text.is_none()
            && self.btn_text.is_none()
            && self.btn_hover_bg.is_none()
            && self.toolbar_bg.is_none()
            && self.toolbar_text.is_none()
            && self.toolbar_border.is_none()
            && self.toolbar_hover.is_none()
            && self.toolbar_selected.is_none()
            && self.swatch_border.is_none()
            && self.dim_opacity.is_none()
            // annotation_colors is parsed for compatibility only — it
            // no longer counts as a theme override (see the field).
            && self.annotation_colors.is_none()
    }
}

fn hex(s: &str) -> Result<u32, String> {
    let digits = s.strip_prefix('#').unwrap_or(s);
    if !matches!(digits.len(), 6 | 8) {
        return Err(format!("{s}: expected #RRGGBB or #RRGGBBAA"));
    }
    let value = u32::from_str_radix(digits, 16).map_err(|_| format!("{s}: invalid hex color"))?;
    let value = if digits.len() == 6 {
        value << 8 | 255
    } else {
        value
    };
    if value & 255 != 255 {
        return Err(format!("{s}: use an opaque color"));
    }
    Ok(value)
}
pub(crate) fn to_hex(v: u32) -> String {
    format!("#{v:08X}")
}
#[derive(Debug)]
pub enum Source {
    Default,
    BuiltIn(String),
    File(PathBuf),
}

/// One preinstalled base palette: the `theme.toml` / `--theme`
/// identifier plus what the Appearance editor's base selector needs.
/// The factory rows carry a translation key; community palettes are
/// proper nouns, displayed as-is in every locale.
pub struct BuiltIn {
    /// The `base` value / `--theme` name (kebab-case, no dot — `resolve`
    /// routes any value containing `.` or a slash to the file path).
    pub name: &'static str,
    /// Display name for untranslated (community) palettes.
    pub(crate) display: &'static str,
    /// `t!` key for the factory rows, None for community palettes.
    pub(crate) label_key: Option<&'static str>,
    palette: fn() -> Theme,
}

impl BuiltIn {
    /// The editor label: translated for the factory rows, the proper
    /// noun otherwise. Resolved per call, so a live locale flip
    /// relabels like everywhere else.
    pub fn label(&self) -> String {
        self.label_key
            .map(|key| crate::i18n::t!(key).to_string())
            .unwrap_or_else(|| self.display.to_string())
    }
}

/// Every preinstalled base, in editor display order: the factory rows
/// first, then the house theme, then community palettes dark→light
/// within each family. `auto` is NOT here — it is not a palette but
/// the dark/light pair following the system appearance, and lives as a
/// separate selector row (see the Appearance editor).
pub const BUILT_INS: &[BuiltIn] = &[
    BuiltIn {
        name: "dark",
        display: "Dark",
        label_key: Some("settings.appearance.dark"),
        palette: Theme::dark,
    },
    BuiltIn {
        name: "light",
        display: "Light",
        label_key: Some("settings.appearance.light"),
        palette: Theme::light,
    },
    BuiltIn {
        name: "high_contrast",
        display: "High contrast",
        label_key: Some("settings.appearance.high_contrast"),
        palette: Theme::high_contrast,
    },
    BuiltIn {
        name: "shotori",
        display: "Shotori",
        label_key: None,
        palette: Theme::shotori,
    },
    BuiltIn {
        name: "catppuccin-frappe",
        display: "Catppuccin Frappé",
        label_key: None,
        palette: Theme::catppuccin_frappe,
    },
    BuiltIn {
        name: "catppuccin-macchiato",
        display: "Catppuccin Macchiato",
        label_key: None,
        palette: Theme::catppuccin_macchiato,
    },
    BuiltIn {
        name: "catppuccin-mocha",
        display: "Catppuccin Mocha",
        label_key: None,
        palette: Theme::catppuccin_mocha,
    },
    BuiltIn {
        name: "catppuccin-latte",
        display: "Catppuccin Latte",
        label_key: None,
        palette: Theme::catppuccin_latte,
    },
    BuiltIn {
        name: "dracula",
        display: "Dracula",
        label_key: None,
        palette: Theme::dracula,
    },
    BuiltIn {
        name: "gruvbox-dark",
        display: "Gruvbox Dark",
        label_key: None,
        palette: Theme::gruvbox_dark,
    },
    BuiltIn {
        name: "nord",
        display: "Nord",
        label_key: None,
        palette: Theme::nord,
    },
    BuiltIn {
        name: "rose-pine",
        display: "Rosé Pine",
        label_key: None,
        palette: Theme::rose_pine,
    },
    BuiltIn {
        name: "tokyo-night",
        display: "Tokyo Night",
        label_key: None,
        palette: Theme::tokyo_night,
    },
];

pub fn built_in(name: &str) -> Result<Theme, String> {
    BUILT_INS
        .iter()
        .find(|built_in| built_in.name == name)
        .map(|built_in| (built_in.palette)())
        .ok_or_else(|| {
            let names = std::iter::once("auto")
                .chain(BUILT_INS.iter().map(|b| b.name))
                .collect::<Vec<_>>()
                .join(" | ");
            format!("unknown theme {name:?} (want {names})")
        })
}

fn apply(theme: &mut Theme, file: &ThemeFile) -> Vec<String> {
    let mut errors = Vec::new();
    for key in ColorKey::ALL {
        if let Some(value) = file.color(key) {
            match hex(value) {
                Ok(color) => key.set(theme, color),
                Err(e) => errors.push(format!("{}: {e}", key.key())),
            }
        }
    }
    // Derived tints: accent pulls pin_border along (the historical
    // single-knob behavior) unless pin_border is set explicitly, and
    // recolors toolbar_selected unless that was set explicitly — an
    // explicit override always wins over the derivation.
    if file.accent.is_some() && file.pin_border.is_none() {
        theme.pin_border = (theme.accent & 0xffffff00) | 0x99;
    }
    if let Some(value) = file.dim_opacity {
        if (0.0..=1.0).contains(&value) {
            theme.dim_opacity = value;
        } else {
            errors.push("dim_opacity must be between 0 and 1".into());
        }
    }
    // annotation_colors: deprecated, parsed for compatibility and
    // reported by `init` — never applied here.
    if file.toolbar_selected.is_none() {
        theme.adapt_accent();
    }
    errors
}

impl ThemeFile {
    /// The resolved palettes this file describes (auto base = the
    /// dark/light pair; a fixed base = one palette). Errors are the
    /// per-field override problems, shared by both palettes' applies.
    pub(crate) fn palettes(&self) -> (Theme, Option<Theme>, Vec<String>) {
        let base = self.base.as_deref().unwrap_or("auto");
        let (mut dark, mut light, mut errors) = if base == "auto" {
            (Theme::dark(), Some(Theme::light()), Vec::new())
        } else {
            match built_in(base) {
                Ok(theme) => (theme, None, Vec::new()),
                Err(error) => (Theme::dark(), Some(Theme::light()), vec![error]),
            }
        };
        errors.extend(apply(&mut dark, self));
        if let Some(light) = &mut light {
            apply(light, self);
        }
        (dark, light, errors)
    }
}

pub fn load_file(path: &Path) -> Result<ThemeFile, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    toml::from_str(&text).map_err(|e| format!("{}: {e}. Use a TOML file with base and per-color overrides (hex); see docs/theme.example.toml.", path.display()))
}

/// The Appearance editor's working copy: the config file if there is
/// one, else a blank base-only file. A MISSING file is the normal
/// first-run state (no error — the first edit creates it); only
/// unreadable or unparsable files travel back as errors, so the user
/// sees their broken file instead of silently editing a fresh one.
pub fn load_editable(path: Option<&Path>) -> (ThemeFile, Vec<String>) {
    match path {
        Some(path) if !path.exists() => (ThemeFile::default(), Vec::new()),
        Some(path) => match load_file(path) {
            Ok(file) => (file, Vec::new()),
            Err(error) => (ThemeFile::default(), vec![error]),
        },
        None => (ThemeFile::default(), Vec::new()),
    }
}

/// Write the editing state back as `theme.toml` — only the overridden
/// keys are written (base + deltas), so the file stays a minimal,
/// human-editable diff against the base palette. The header says who
/// manages it; unknown keys cannot exist (`deny_unknown_fields`).
pub fn save(path: &Path, file: &ThemeFile) -> Result<(), String> {
    let mut table = toml::Table::new();
    table.insert(
        "base".into(),
        toml::Value::String(file.base.clone().unwrap_or_else(|| "auto".into())),
    );
    for key in ColorKey::ALL {
        if let Some(value) = file.color(key) {
            table.insert(key.key().into(), toml::Value::String(value.clone()));
        }
    }
    if let Some(opacity) = file.dim_opacity {
        table.insert("dim_opacity".into(), toml::Value::Float(opacity as f64));
    }
    if let Some(colors) = &file.annotation_colors {
        table.insert(
            "annotation_colors".into(),
            toml::Value::Array(colors.iter().cloned().map(toml::Value::String).collect()),
        );
    }
    let body = table.to_string();
    let text = format!(
        "# Shotori theme — managed by `shotori settings`; base + overrides.\n\
         # base = auto | dark | light | high_contrast, every other key is optional.\n{body}"
    );
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn config_path() -> Option<PathBuf> {
    // shared with the settings loader (one config dir, two files)
    crate::model::settings::config_dir().map(|dir| dir.join("theme.toml"))
}

fn resolve(value: Option<&str>, config: Option<PathBuf>) -> (ThemeFile, Source, Vec<String>) {
    let path = match value {
        Some(value) if value.contains('/') || value.contains('\\') || value.contains('.') => {
            Some(PathBuf::from(value))
        }
        Some(value) => {
            return (
                ThemeFile {
                    base: Some(value.into()),
                    ..Default::default()
                },
                Source::BuiltIn(value.into()),
                Vec::new(),
            );
        }
        None => config,
    };
    match path {
        Some(path) => match load_file(&path) {
            Ok(file) => (file, Source::File(path), Vec::new()),
            Err(error) => (ThemeFile::default(), Source::File(path), vec![error]),
        },
        None => (ThemeFile::default(), Source::Default, Vec::new()),
    }
}

pub fn init(args: &crate::args::Cli) {
    let config = if args.no_config {
        None
    } else {
        config_path().filter(|p| p.exists())
    };
    let (file, source, mut errors) = resolve(args.theme.as_deref(), config);
    // Migration note (2026-10): annotation_colors now lives in
    // settings.toml ([toolbar] colors). Old theme files that still
    // carry the key load fine — it is silently ignored so a theme file
    // shared between shotori versions keeps working; this note tells
    // the owner where the board is configured now.
    if file.annotation_colors.is_some() {
        eprintln!(
            "[shotori] theme: annotation_colors is no longer part of the theme \
             (moved to settings.toml [toolbar] colors); the key was ignored"
        );
    }
    let (dark, light, palette_errors) = file.palettes();
    errors.extend(palette_errors);
    for error in &errors {
        eprintln!("[shotori] theme: {error}");
    }
    if args.print_theme {
        if let Some(light) = &light {
            let _ = writeln!(
                std::io::stdout(),
                "auto: follows the system; both complete palettes are shown below"
            );
            let _ = writeln!(std::io::stdout(), "[dark appearance]");
            print_theme(&dark, &source);
            let _ = writeln!(std::io::stdout(), "[light appearance]");
            print_theme(light, &source);
        } else {
            print_theme(&dark, &source);
        }
        std::process::exit(i32::from(!errors.is_empty()));
    }
    super::set(dark, light);
}

/// `--print-theme` output: source line, one line per field, palette tail.
fn print_theme(theme: &Theme, source: &Source) {
    use std::io::Write;
    // writeln + ignore errors: piping into `head` must not panic on the
    // broken pipe, a truncated dump is the expected outcome there
    let mut out = std::io::stdout().lock();
    let src = match source {
        Source::Default => "default (auto)".to_string(),
        Source::BuiltIn(n) => format!("built-in {n}"),
        Source::File(p) => p.display().to_string(),
    };
    let _ = writeln!(out, "shotori theme — source: {src}");
    let _ = writeln!(
        out,
        "dim            {} @ opacity {:.2}",
        to_hex(theme.dim_color),
        theme.dim_opacity
    );
    let _ = writeln!(out, "accent         {}", to_hex(theme.accent));
    let _ = writeln!(out, "pin_border     {}", to_hex(theme.pin_border));
    let _ = writeln!(out, "chip_bg        {}", to_hex(theme.chip_bg));
    let _ = writeln!(out, "hint_text      {}", to_hex(theme.hint_text));
    let _ = writeln!(out, "btn_text       {}", to_hex(theme.btn_text));
    let _ = writeln!(out, "btn_hover_bg   {}", to_hex(theme.btn_hover_bg));
    let _ = writeln!(out, "toolbar_bg     {}", to_hex(theme.toolbar_bg));
    let _ = writeln!(out, "toolbar_text   {}", to_hex(theme.toolbar_text));
    let _ = writeln!(out, "toolbar_border {}", to_hex(theme.toolbar_border));
    let _ = writeln!(out, "toolbar_hover  {}", to_hex(theme.toolbar_hover));
    let _ = writeln!(out, "toolbar_selected {}", to_hex(theme.toolbar_selected));
    let _ = writeln!(out, "swatch_border   {}", to_hex(theme.swatch_border));
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn auto_and_fixed_palettes() {
        let (_, light, errors) = ThemeFile::default().palettes();
        assert!(light.is_some() && errors.is_empty());
        for base in ["dark", "light", "high_contrast"] {
            let file: ThemeFile = toml::from_str(&format!("base = {base:?}")).unwrap();
            let (theme, auto, errors) = file.palettes();
            assert!(auto.is_none() && errors.is_empty());
            assert_eq!(theme.toolbar_bg, built_in(base).unwrap().toolbar_bg);
        }
    }
    #[test]
    fn unknown_fields_are_still_rejected() {
        // A typo'd key must fail loudly (deny_unknown_fields) — the
        // days of silently ignoring surface overrides are over, they
        // are legal now and every key must be spelled right.
        for content in [
            "acccent = '#00FF00'",
            "base = 'dark'\ntollbar_bg = '#FAFAFC'",
        ] {
            assert!(toml::from_str::<ThemeFile>(content).is_err());
        }
    }
    #[test]
    fn surface_overrides_are_composable_now() {
        // The v2 contract: any subset of fields on any base — the
        // Appearance editor writes exactly this shape.
        let file: ThemeFile =
            toml::from_str("base = 'dark'\ntoolbar_bg = '#FAFAFC'\ntoolbar_text = '#30343B'")
                .unwrap();
        let (theme, _, errors) = file.palettes();
        assert!(errors.is_empty());
        assert_eq!(theme.toolbar_bg, 0xFAFAFCFF);
        assert_eq!(theme.toolbar_text, 0x30343BFF);
    }
    #[test]
    fn invalid_values_preserve_readable_palettes() {
        let file: ThemeFile = toml::from_str(
            "base = 'missing'\naccent = '#FFFFFF00'\ndim_opacity = nan\nannotation_colors = ['#123456']",
        )
        .unwrap();
        let (theme, light, errors) = file.palettes();
        // annotation_colors no longer reports (deprecated, ignored);
        // the other three bad values still do.
        assert_eq!(errors.len(), 3);
        assert!(light.is_some());
        assert_eq!(theme.accent, Theme::dark().accent);
        assert_eq!(theme.dim_opacity, 0.55);
    }
    #[test]
    fn toml_comments_palette_and_precedence() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("theme.toml");
        std::fs::write(&path, include_str!("../../../docs/theme.example.toml")).unwrap();
        assert!(load_file(&path).unwrap().palettes().2.is_empty());
        std::fs::write(&path,"# theme\nbase = 'light'\naccent = '#123456'\ndim_opacity = 0.2\nannotation_colors = ['#111111', '#222222', '#333333', '#444444', '#555555', '#666666', '#777777']").unwrap();
        let (file, _, errors) = resolve(None, Some(path.clone()));
        let (theme, auto, _) = file.palettes();
        assert!(errors.is_empty() && auto.is_none());
        assert_eq!(theme.accent, 0x123456FF);
        assert_eq!(theme.dim_opacity, 0.2);
        // annotation_colors parsed (old file shape) but no longer
        // applied by the theme — the board lives in settings now.
        let (file, _, _) = resolve(Some("dark"), Some(path));
        assert_eq!(file.palettes().0.toolbar_bg, Theme::dark().toolbar_bg);
    }
    #[test]
    fn malformed_file_falls_back_without_partial_application() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.toml");
        // Invalid TOML syntax: the file never parses, resolve falls
        // back to the default (auto) file with the error attached.
        std::fs::write(&path, "base = 'light'\ntoolbar_text = ").unwrap();
        let (file, _, errors) = resolve(None, Some(path.clone()));
        assert_eq!(errors.len(), 1);
        assert!(file.palettes().1.is_some());
        // A bad VALUE parses but applies nothing for that field —
        // resolve stays quiet (the file loaded), palettes() reports
        // the per-field error.
        std::fs::write(&path, "base = 'light'\ntoolbar_text = 'whiteish'").unwrap();
        let (file, _, errors) = resolve(None, Some(path));
        assert!(errors.is_empty());
        let (theme, _single, palette_errors) = file.palettes();
        assert_eq!(palette_errors.len(), 1);
        assert!(palette_errors[0].contains("toolbar_text"));
        // fixed base = one palette; the bad field left it at the base
        assert_eq!(theme.toolbar_text, Theme::light().toolbar_text);
    }
    #[test]
    fn explicit_overrides_beat_derivations() {
        // accent alone pulls pin_border and toolbar_selected along …
        let file: ThemeFile = toml::from_str("accent = '#123456'").unwrap();
        let (theme, _, _) = file.palettes();
        assert_eq!(theme.accent, 0x123456FF);
        assert_eq!(theme.pin_border, 0x12345699);
        let mut derived = theme;
        derived.adapt_accent();
        assert_eq!(theme.toolbar_selected, derived.toolbar_selected);
        // … but an explicit value for a derived field wins. (Opaque
        // hex only — the file format has no alpha but the derived
        // pin tints carry one.)
        let file: ThemeFile = toml::from_str(
            "accent = '#123456'\npin_border = '#ABCDEFFF'\ntoolbar_selected = '#111111FF'",
        )
        .unwrap();
        let (theme, _, _) = file.palettes();
        assert_eq!(theme.pin_border, 0xABCDEFFF);
        assert_eq!(theme.toolbar_selected, 0x111111FF);
    }
    #[test]
    fn save_round_trips_and_writes_only_overrides() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("theme.toml");
        let file: ThemeFile = toml::from_str(
            "base = 'light'\naccent = '#123456'\ntoolbar_bg = '#202028'\ndim_opacity = 0.3",
        )
        .unwrap();
        save(&path, &file).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        // only overridden keys + base are written
        assert!(text.contains("base = \"light\""));
        assert!(text.contains("accent"));
        assert!(text.contains("toolbar_bg"));
        assert!(!text.contains("toolbar_text"));
        let reloaded = load_file(&path).unwrap();
        assert_eq!(reloaded, file);
        assert!(!reloaded.is_base_only());
        // base-only files round-trip too
        let file = ThemeFile::default();
        save(&path, &file).unwrap();
        assert!(load_file(&path).unwrap().is_base_only());
    }

    #[test]
    fn every_preinstalled_base_resolves_and_round_trips() {
        for built in BUILT_INS {
            let direct = built_in(built.name).unwrap();
            // the same name through a theme file resolves to the same
            // palette (single palette, no auto pair, no errors)
            let file: ThemeFile = toml::from_str(&format!("base = '{}'", built.name)).unwrap();
            let (from_file, auto, errors) = file.palettes();
            assert!(auto.is_none() && errors.is_empty(), "{}", built.name);
            assert_eq!(from_file.toolbar_bg, direct.toolbar_bg, "{}", built.name);
            assert_eq!(from_file.accent, direct.accent, "{}", built.name);
            assert_eq!(
                from_file.toolbar_selected, direct.toolbar_selected,
                "{}",
                built.name
            );
        }
    }

    #[test]
    fn base_names_cannot_be_mistaken_for_paths() {
        // resolve() routes any value containing '.' or a slash to the
        // file-path branch — a name with one would be unreachable via
        // `--theme` and `base =`.
        for built in BUILT_INS {
            assert!(!built.name.contains(['.', '/', '\\']), "{}", built.name);
        }
    }

    #[test]
    fn preinstalled_palettes_are_pairwise_distinct() {
        // guards the copy-paste slip: two registry rows sharing one
        // palette (surface + accent identify a theme here)
        for (i, a) in BUILT_INS.iter().enumerate() {
            for b in &BUILT_INS[i + 1..] {
                let palette_a = built_in(a.name).unwrap();
                let palette_b = built_in(b.name).unwrap();
                assert!(
                    palette_a.toolbar_bg != palette_b.toolbar_bg
                        || palette_a.accent != palette_b.accent,
                    "{} and {} are the same palette",
                    a.name,
                    b.name
                );
            }
        }
    }

    #[test]
    fn preinstalled_selected_matches_the_accent_adaptation() {
        // apply() re-derives toolbar_selected via adapt_accent unless
        // the file overrides it — a built-in whose literal differs from
        // the derivation would render differently via `--theme name`
        // (the direct palette) vs base = "name" (a file load). The
        // literals ARE the derivation, by construction.
        for built in BUILT_INS {
            let mut theme = built_in(built.name).unwrap();
            let selected = theme.toolbar_selected;
            theme.adapt_accent();
            assert_eq!(theme.toolbar_selected, selected, "{}", built.name);
        }
    }

    #[test]
    fn unknown_base_error_lists_the_choices() {
        let error = match built_in("nope") {
            Ok(_) => panic!("unknown base resolved"),
            Err(error) => error,
        };
        assert!(error.contains("auto"));
        assert!(error.contains("catppuccin-mocha"));
        assert!(error.contains("tokyo-night"));
    }
}
