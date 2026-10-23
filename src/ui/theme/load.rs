//! TOML theme settings. `base` picks a complete built-in palette
//! (surfaces and control states switch together by construction); any
//! other key is a single-field override on top of that base — the
//! Appearance editor writes this file, `--theme FILE` / the XDG
//! fallback read it.
use super::{ColorKey, Theme};
use serde::{Deserialize, Serialize};
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

/// One preinstalled community palette: the `theme.toml` / `--theme`
/// identifier plus what the Appearance editor's base selector shows
/// (a proper noun, displayed as-is in every locale).
pub struct BuiltIn {
    /// The `base` value / `--theme` name (kebab-case, no dot — `resolve`
    /// routes any value containing `.` or a slash to the file path).
    pub name: &'static str,
    /// Display name (proper noun).
    pub(crate) display: &'static str,
    palette: fn() -> Theme,
}

/// The selectable presets, in editor display order — community
/// palettes only. The shotori look (the orange dark/light pair) is
/// the selector's first row, not an entry here: it is a palette PAIR
/// following the system, not a fixed palette. The factory
/// `dark`/`light`/`high_contrast` and the legacy `auto` spelling are
/// not selectable anymore either but still resolve (in [`built_in`]
/// and [`ThemeFile::palettes`]) for old theme files.
pub const BUILT_INS: &[BuiltIn] = &[
    BuiltIn {
        name: "catppuccin-frappe",
        display: "Catppuccin Frappé",
        palette: Theme::catppuccin_frappe,
    },
    BuiltIn {
        name: "catppuccin-macchiato",
        display: "Catppuccin Macchiato",
        palette: Theme::catppuccin_macchiato,
    },
    BuiltIn {
        name: "catppuccin-mocha",
        display: "Catppuccin Mocha",
        palette: Theme::catppuccin_mocha,
    },
    BuiltIn {
        name: "catppuccin-latte",
        display: "Catppuccin Latte",
        palette: Theme::catppuccin_latte,
    },
    BuiltIn {
        name: "dracula",
        display: "Dracula",
        palette: Theme::dracula,
    },
    BuiltIn {
        name: "gruvbox-dark",
        display: "Gruvbox Dark",
        palette: Theme::gruvbox_dark,
    },
    BuiltIn {
        name: "gruvbox-light",
        display: "Gruvbox Light",
        palette: Theme::gruvbox_light,
    },
    BuiltIn {
        name: "nord",
        display: "Nord",
        palette: Theme::nord,
    },
    BuiltIn {
        name: "rose-pine",
        display: "Rosé Pine",
        palette: Theme::rose_pine,
    },
    BuiltIn {
        name: "rose-pine-dawn",
        display: "Rosé Pine Dawn",
        palette: Theme::rose_pine_dawn,
    },
    BuiltIn {
        name: "tokyo-night",
        display: "Tokyo Night",
        palette: Theme::tokyo_night,
    },
    BuiltIn {
        name: "tokyo-night-day",
        display: "Tokyo Night Day",
        palette: Theme::tokyo_night_day,
    },
    BuiltIn {
        name: "solarized-light",
        display: "Solarized Light",
        palette: Theme::solarized_light,
    },
    BuiltIn {
        name: "github-light",
        display: "GitHub Light",
        palette: Theme::github_light,
    },
];

pub fn built_in(name: &str) -> Result<Theme, String> {
    // Hidden legacy names (2026-10): the factory palettes stopped being
    // selectable presets when the shotori pair became light/dark rows;
    // theme.toml files written by older editors still carry them, so
    // they keep resolving — silently, just not listed. `shotori` and
    // `auto` (the retired system-following pair) resolve to its dark
    // half in [`ThemeFile::palette`].
    let theme = match name {
        "shotori-dark" => Some(Theme::dark()),
        "shotori-light" => Some(Theme::light()),
        // hidden legacy spellings, kept resolvable for old theme.toml files
        "dark" => Some(Theme::dark()),
        "light" => Some(Theme::light()),
        "high_contrast" => Some(Theme::high_contrast()),
        _ => BUILT_INS
            .iter()
            .find(|built_in| built_in.name == name)
            .map(|built_in| (built_in.palette)()),
    };
    theme.ok_or_else(|| {
        let names = ["shotori-dark", "shotori-light"]
            .into_iter()
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
    /// The resolved palette this file describes: one fixed theme. The
    /// default (and the legacy `shotori` / `auto` spellings of the
    /// retired system-following pair) is `shotori-dark`; every other
    /// base is a fixed palette. Errors are the per-field override
    /// problems.
    pub(crate) fn palette(&self) -> (Theme, Vec<String>) {
        let base = self.base.as_deref().unwrap_or("shotori-dark");
        let (mut theme, mut errors) = if matches!(base, "shotori-dark" | "shotori" | "auto") {
            (Theme::dark(), Vec::new())
        } else {
            match built_in(base) {
                Ok(theme) => (theme, Vec::new()),
                Err(error) => (Theme::dark(), vec![error]),
            }
        };
        errors.extend(apply(&mut theme, self));
        (theme, errors)
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
        toml::Value::String(file.base.clone().unwrap_or_else(|| "shotori-dark".into())),
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
         # base = shotori-dark (default), shotori-light or a preinstalled palette; every other key is optional.\n{body}"
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
    let (theme, palette_errors) = file.palette();
    errors.extend(palette_errors);
    for error in &errors {
        eprintln!("[shotori] theme: {error}");
    }
    if args.print_theme {
        print_theme(&theme, &source);
        std::process::exit(i32::from(!errors.is_empty()));
    }
    super::set(theme);
}

/// `--print-theme` output: source line, one line per field, palette tail.
fn print_theme(theme: &Theme, source: &Source) {
    use std::io::Write;
    // writeln + ignore errors: piping into `head` must not panic on the
    // broken pipe, a truncated dump is the expected outcome there
    let mut out = std::io::stdout().lock();
    let src = match source {
        Source::Default => "default (shotori-dark)".to_string(),
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
    fn default_and_legacy_palettes() {
        // no base = shotori-dark, one fixed palette (the
        // system-following pair is retired)
        let (theme, errors) = ThemeFile::default().palette();
        assert!(errors.is_empty());
        assert_eq!(theme.toolbar_bg, Theme::dark().toolbar_bg);
        // dark/light/high_contrast are hidden legacy names now — files
        // written by older editors must keep resolving.
        for base in [
            "dark",
            "light",
            "high_contrast",
            "catppuccin-mocha",
            "shotori-light",
        ] {
            let file: ThemeFile = toml::from_str(&format!("base = {base:?}")).unwrap();
            let (theme, errors) = file.palette();
            assert!(errors.is_empty());
            assert_eq!(theme.toolbar_bg, built_in(base).unwrap().toolbar_bg);
        }
        // the legacy auto-pair spellings resolve to its dark half
        for base in ["shotori", "auto"] {
            let file: ThemeFile = toml::from_str(&format!("base = '{base}'")).unwrap();
            let (theme, errors) = file.palette();
            assert!(errors.is_empty(), "{base}");
            assert_eq!(theme.toolbar_bg, Theme::dark().toolbar_bg, "{base}");
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
        let (theme, errors) = file.palette();
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
        let (theme, errors) = file.palette();
        // annotation_colors no longer reports (deprecated, ignored);
        // the other three bad values still do.
        assert_eq!(errors.len(), 3);
        assert_eq!(theme.accent, Theme::dark().accent);
        assert_eq!(theme.dim_opacity, 0.55);
    }
    #[test]
    fn toml_comments_palette_and_precedence() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("theme.toml");
        std::fs::write(&path, include_str!("../../../docs/theme.example.toml")).unwrap();
        assert!(load_file(&path).unwrap().palette().1.is_empty());
        std::fs::write(&path,"# theme\nbase = 'light'\naccent = '#123456'\ndim_opacity = 0.2\nannotation_colors = ['#111111', '#222222', '#333333', '#444444', '#555555', '#666666', '#777777']").unwrap();
        let (file, _, _errors) = resolve(None, Some(path.clone()));
        let (theme, _) = file.palette();
        assert_eq!(theme.accent, 0x123456FF);
        assert_eq!(theme.dim_opacity, 0.2);
        // annotation_colors parsed (old file shape) but no longer
        // applied by the theme — the board lives in settings now.
        let (file, _, _) = resolve(Some("dark"), Some(path));
        assert_eq!(file.palette().0.toolbar_bg, Theme::dark().toolbar_bg);
    }
    #[test]
    fn malformed_file_falls_back_without_partial_application() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.toml");
        // Invalid TOML syntax: the file never parses, resolve falls
        // back to the default file with the error attached.
        std::fs::write(&path, "base = 'light'\ntoolbar_text = ").unwrap();
        let (file, _, errors) = resolve(None, Some(path.clone()));
        assert_eq!(errors.len(), 1);
        let (theme, palette_errors) = file.palette();
        assert!(palette_errors.is_empty());
        assert_eq!(theme.toolbar_bg, Theme::dark().toolbar_bg);
        // A bad VALUE parses but applies nothing for that field —
        // resolve stays quiet (the file loaded), palette() reports
        // the per-field error.
        std::fs::write(&path, "base = 'light'\ntoolbar_text = 'whiteish'").unwrap();
        let (file, _, errors) = resolve(None, Some(path));
        assert!(errors.is_empty());
        let (theme, palette_errors) = file.palette();
        assert_eq!(palette_errors.len(), 1);
        assert!(palette_errors[0].contains("toolbar_text"));
        // the bad field left it at the base
        assert_eq!(theme.toolbar_text, Theme::light().toolbar_text);
    }
    #[test]
    fn explicit_overrides_beat_derivations() {
        // accent alone pulls pin_border and toolbar_selected along …
        let file: ThemeFile = toml::from_str("accent = '#123456'").unwrap();
        let (theme, _) = file.palette();
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
        let (theme, _) = file.palette();
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
            let (from_file, errors) = file.palette();
            assert!(errors.is_empty(), "{}", built.name);
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
        // literals ARE the derivation, by construction. The factory
        // trio is included even though it is no longer listed — `auto`
        // resolves through them.
        let palettes: Vec<(&str, Theme)> = BUILT_INS
            .iter()
            .map(|b| (b.name, built_in(b.name).unwrap()))
            .chain([
                ("shotori-dark", Theme::dark()),
                ("shotori-light", Theme::light()),
                ("dark", Theme::dark()),
                ("light", Theme::light()),
                ("high_contrast", Theme::high_contrast()),
            ])
            .collect();
        for (name, mut theme) in palettes {
            let selected = theme.toolbar_selected;
            theme.adapt_accent();
            assert_eq!(theme.toolbar_selected, selected, "{name}");
        }
    }

    #[test]
    fn unknown_base_error_lists_the_choices() {
        let error = match built_in("nope") {
            Ok(_) => panic!("unknown base resolved"),
            Err(error) => error,
        };
        assert!(error.contains("shotori-dark"));
        assert!(error.contains("shotori-light"));
        assert!(error.contains("catppuccin-mocha"));
        assert!(error.contains("tokyo-night"));
        // the hidden legacy names still resolve but are not advertised
        assert!(!error.contains("auto"));
        assert!(!error.contains("high_contrast"));
    }
}
