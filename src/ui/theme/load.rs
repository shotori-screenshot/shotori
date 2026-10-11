//! TOML theme settings. Surface colors belong to complete built-in palettes;
//! configuration cannot mix a light background with dark control states.
use super::{PALETTE, Theme};
use serde::Deserialize;
use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Deserialize, Default, Debug)]
#[serde(default, deny_unknown_fields)]
pub struct ThemeFile {
    pub base: Option<String>,
    pub accent: Option<String>,
    pub dim_opacity: Option<f32>,
    pub annotation_colors: Option<Vec<String>>,
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
fn to_hex(v: u32) -> String {
    format!("#{v:08X}")
}
#[derive(Debug)]
pub enum Source {
    Default,
    BuiltIn(String),
    File(PathBuf),
}

pub fn built_in(name: &str) -> Result<Theme, String> {
    match name {
        "dark" => Ok(Theme::dark()),
        "light" => Ok(Theme::light()),
        "high_contrast" => Ok(Theme::high_contrast()),
        _ => Err(format!(
            "unknown theme {name:?} (want auto | dark | light | high_contrast)"
        )),
    }
}

fn apply(theme: &mut Theme, file: &ThemeFile) -> Vec<String> {
    let mut errors = Vec::new();
    if let Some(value) = &file.accent {
        match hex(value) {
            Ok(color) => {
                theme.accent = color;
                theme.pin_border = (color & 0xffffff00) | 0x99;
            }
            Err(e) => errors.push(format!("accent: {e}")),
        }
    }
    if let Some(value) = file.dim_opacity {
        if (0.0..=1.0).contains(&value) {
            theme.dim_opacity = value;
        } else {
            errors.push("dim_opacity must be between 0 and 1".into());
        }
    }
    if let Some(colors) = &file.annotation_colors {
        if colors.len() != PALETTE {
            errors.push(format!(
                "annotation_colors must contain exactly {PALETTE} opaque colors"
            ));
        } else {
            let parsed: Result<Vec<_>, _> = colors.iter().map(|v| hex(v)).collect();
            match parsed {
                Ok(colors) => theme.annotation_colors.copy_from_slice(&colors),
                Err(e) => errors.push(format!("annotation_colors: {e}")),
            }
        }
    }
    theme.adapt_accent();
    errors
}

impl ThemeFile {
    fn palettes(&self) -> (Theme, Option<Theme>, Vec<String>) {
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
    toml::from_str(&text).map_err(|e| format!("{}: {e}. Use a TOML file with base, accent, dim_opacity and annotation_colors; surface colors are managed together.", path.display()))
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
    let palette = theme
        .annotation_colors
        .iter()
        .map(|c| to_hex(*c))
        .collect::<Vec<_>>()
        .join(" ");
    let _ = writeln!(out, "annotation_colors {palette}");
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
    fn old_mixed_surface_overrides_and_unknown_fields_are_rejected() {
        for content in [
            "base = 'dark'\ntoolbar_bg = '#FAFAFC'\ntoolbar_selected = '#453528'",
            "acccent = '#00FF00'",
            r##"{"base":"dark","toolbar_bg":"#FAFAFC"}"##,
        ] {
            assert!(toml::from_str::<ThemeFile>(content).is_err());
        }
    }
    #[test]
    fn invalid_values_preserve_readable_palettes() {
        let file: ThemeFile = toml::from_str("base = 'missing'\naccent = '#FFFFFF00'\ndim_opacity = nan\nannotation_colors = ['#123456']").unwrap();
        let (theme, light, errors) = file.palettes();
        assert_eq!(errors.len(), 4);
        assert!(light.is_some());
        assert_eq!(theme.accent, Theme::dark().accent);
        assert_eq!(theme.annotation_colors, Theme::dark().annotation_colors);
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
        assert_eq!(theme.annotation_colors[6], 0x777777FF);
        let (file, _, _) = resolve(Some("dark"), Some(path));
        assert_eq!(file.palettes().0.toolbar_bg, Theme::dark().toolbar_bg);
    }
    #[test]
    fn malformed_file_falls_back_without_partial_application() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.toml");
        std::fs::write(&path, "base = 'light'\ntoolbar_text = '#FFFFFF'").unwrap();
        let (file, _, errors) = resolve(None, Some(path));
        assert_eq!(errors.len(), 1);
        assert!(file.palettes().1.is_some());
    }
}
