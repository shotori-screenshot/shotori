//! # Shotori theme system
//!
//! Every visual constant of the overlay lives here, grouped into a
//! [`Theme`] struct instead of loose constants, so the look can be
//! swapped as a whole. Resolution order (see [`load`]):
//!
//! 1. a built-in base theme — `auto` (default) / `dark` / `light` / `high_contrast`
//! 2. an optional TOML settings file — `--theme <file>` or the XDG
//!    config fallback (`~/.config/shotori/theme.toml`)
//!
//! The theme is installed once at startup ([`set`]) and read everywhere
//! ([`c`]); the overlay lives for seconds, so system appearance changes switch the complete palette.
//!
//! Colors are `u32` in gpui's `0xRRGGBBAA` layout (e.g. the orange
//! accent is `0xFF6A00FF`).

use std::sync::RwLock;

pub mod load;

/// The complete visual vocabulary of the overlay. One struct so a theme
/// is a value: build it, override it, ship it.
///
/// The annotation swatch board is NOT here: it is toolbar
/// configuration (which brush colors exist), living in
/// [`crate::model::settings`] — the built-in palettes never disagreed
/// on it, and theming a user's customized board on top would be
/// surprising. Swatch chrome (the outline color) stays: that IS
/// appearance.
#[derive(Clone, Copy)]
pub struct Theme {
    /// Dim layer OUTSIDE the selection: color at full opacity + strength.
    /// What gets painted is [`Theme::dim`] (color with alpha applied).
    pub dim_color: u32,
    /// Dim strength, 0.0 (clear) .. 1.0 (solid). Default dark: 0.55.
    pub dim_opacity: f32,
    /// Accent color: selection border, focused controls, OCR chip border.
    pub accent: u32,
    /// Pin window border when unfocused (used by the `pin` branch).
    pub pin_border: u32,

    /// Hint bar / chip background (OCR setup dialog).
    pub chip_bg: u32,
    /// Hint bar / chip text.
    pub hint_text: u32,
    /// Classic (dark chip) button text.
    pub btn_text: u32,
    /// Classic button hover background.
    pub btn_hover_bg: u32,

    /// Floating annotation toolbar. dark uses dark surfaces (the
    /// historical near-white toolbar clashed with the dark chrome).
    pub toolbar_bg: u32,
    pub toolbar_text: u32,
    pub toolbar_border: u32,
    pub toolbar_hover: u32,
    pub toolbar_selected: u32,
    /// Swatch outline: needs contrast against BOTH the toolbar and the
    /// swatch fill (a #222222 swatch on a dark toolbar is invisible
    /// without it), so it does not simply reuse `toolbar_border`.
    pub swatch_border: u32,
}

impl Theme {
    /// The default look since v0.1 — dark chips over a 55% dim.
    pub const fn dark() -> Self {
        Self {
            dim_color: 0x000000FF,
            dim_opacity: 0.55,
            accent: 0xFF6A00FF,
            pin_border: 0xFF6A0099,
            chip_bg: 0x16161DE6,
            hint_text: 0xAAAAAAFF,
            btn_text: 0xEEEEEEFF,
            btn_hover_bg: 0xFFFFFF26,
            toolbar_bg: 0x202028FF,
            toolbar_text: 0xE8E8EEFF,
            toolbar_border: 0x3E3E4CFF,
            toolbar_hover: 0x2C2C3AFF,
            toolbar_selected: 0x453528FF,
            swatch_border: 0xFFFFFF38,
        }
    }

    /// Light chips for bright wallpapers: same accent, softer surfaces.
    /// Toolbar fields are spelled out (not inherited): dark owns dark
    /// surfaces since the white-toolbar fix, inheritance would leak them.
    pub const fn light() -> Self {
        Self {
            chip_bg: 0xFAFAFCE6,
            hint_text: 0x30343BFF,
            btn_text: 0x30343BFF,
            btn_hover_bg: 0x00000014,
            pin_border: 0x30343B99,
            toolbar_bg: 0xFAFAFCFF,
            toolbar_text: 0x30343BFF,
            toolbar_border: 0xC3C7CEFF,
            toolbar_hover: 0xEAEDF2FF,
            toolbar_selected: 0xFFE5D3FF,
            swatch_border: 0x00000026,
            ..dark_const()
        }
    }

    /// Maximum-contrast UI copy: pure black chips, white text, hard borders.
    pub const fn high_contrast() -> Self {
        Self {
            chip_bg: 0x000000FA,
            hint_text: 0xFFFFFFFF,
            btn_text: 0xFFFFFFFF,
            toolbar_bg: 0xFFFFFFFF,
            toolbar_text: 0x000000FF,
            toolbar_border: 0x000000FF,
            toolbar_hover: 0xD9D9D9FF,
            toolbar_selected: 0xDCDCDCFF,
            swatch_border: 0x00000059,
            ..dark_const()
        }
    }

    /// Accent tint stays within the active surface palette. Never import a
    /// background from another appearance when only the accent changes.
    pub(crate) fn adapt_accent(&mut self) {
        let bg = self.toolbar_bg.to_be_bytes();
        let accent = self.accent.to_be_bytes();
        let tint = |i: usize| ((bg[i] as u32 * 88 + accent[i] as u32 * 12) / 100) as u8;
        self.toolbar_selected = u32::from_be_bytes([tint(0), tint(1), tint(2), 255]);
    }

    pub(crate) fn accent_text(&self) -> u32 {
        if contrast(0xFFFFFFFF, self.accent) >= 4.5 {
            0xFFFFFFFF
        } else {
            0x000000FF
        }
    }

    pub(crate) fn selected_text(&self) -> u32 {
        if contrast(self.accent, self.toolbar_selected) >= 4.5 {
            self.accent
        } else {
            self.toolbar_text
        }
    }

    /// The dim layer quad color: `dim_color` with `dim_opacity` baked in
    /// (this is what `rgba()` wants at the call sites).
    pub fn dim(&self) -> u32 {
        let a = (self.dim_opacity.clamp(0., 1.) * 255.).round() as u32;
        (self.dim_color & 0xFFFFFF00) | a.min(0xFF)
    }
}

/// `..dark()` spelled for const fn (struct-update syntax is not const).
const fn dark_const() -> Theme {
    Theme::dark()
}

/// One addressable solid-color field of [`Theme`] — the registry the
/// Appearance editor and `theme.toml` both speak. Labels, file keys
/// and accessors come from here and nowhere else, so the picker grid,
/// the save format and the mockup cannot drift apart. The derived
/// fields (accent → pin_border tint, `adapt_accent` → toolbar_selected
/// tint) live in `load::apply`'s override logic, not here.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ColorKey {
    DimColor,
    Accent,
    PinBorder,
    ChipBg,
    HintText,
    BtnText,
    BtnHoverBg,
    ToolbarBg,
    ToolbarText,
    ToolbarBorder,
    ToolbarHover,
    ToolbarSelected,
    SwatchBorder,
}

impl ColorKey {
    pub const ALL: [Self; 13] = [
        Self::DimColor,
        Self::Accent,
        Self::PinBorder,
        Self::ChipBg,
        Self::HintText,
        Self::BtnText,
        Self::BtnHoverBg,
        Self::ToolbarBg,
        Self::ToolbarText,
        Self::ToolbarBorder,
        Self::ToolbarHover,
        Self::ToolbarSelected,
        Self::SwatchBorder,
    ];

    /// Human label (settings copy lives with the registry it names).
    pub fn label(self) -> &'static str {
        match self {
            Self::DimColor => "Dim",
            Self::Accent => "Accent",
            Self::PinBorder => "Pin border",
            Self::ChipBg => "Chip background",
            Self::HintText => "Hint text",
            Self::BtnText => "Button text",
            Self::BtnHoverBg => "Button hover",
            Self::ToolbarBg => "Toolbar background",
            Self::ToolbarText => "Toolbar text",
            Self::ToolbarBorder => "Toolbar border",
            Self::ToolbarHover => "Toolbar hover",
            Self::ToolbarSelected => "Toolbar selected",
            Self::SwatchBorder => "Swatch border",
        }
    }

    /// The `theme.toml` key (stable — files outlive binaries).
    pub fn key(self) -> &'static str {
        match self {
            Self::DimColor => "dim_color",
            Self::Accent => "accent",
            Self::PinBorder => "pin_border",
            Self::ChipBg => "chip_bg",
            Self::HintText => "hint_text",
            Self::BtnText => "btn_text",
            Self::BtnHoverBg => "btn_hover_bg",
            Self::ToolbarBg => "toolbar_bg",
            Self::ToolbarText => "toolbar_text",
            Self::ToolbarBorder => "toolbar_border",
            Self::ToolbarHover => "toolbar_hover",
            Self::ToolbarSelected => "toolbar_selected",
            Self::SwatchBorder => "swatch_border",
        }
    }

    pub fn get(self, theme: &Theme) -> u32 {
        match self {
            Self::DimColor => theme.dim_color,
            Self::Accent => theme.accent,
            Self::PinBorder => theme.pin_border,
            Self::ChipBg => theme.chip_bg,
            Self::HintText => theme.hint_text,
            Self::BtnText => theme.btn_text,
            Self::BtnHoverBg => theme.btn_hover_bg,
            Self::ToolbarBg => theme.toolbar_bg,
            Self::ToolbarText => theme.toolbar_text,
            Self::ToolbarBorder => theme.toolbar_border,
            Self::ToolbarHover => theme.toolbar_hover,
            Self::ToolbarSelected => theme.toolbar_selected,
            Self::SwatchBorder => theme.swatch_border,
        }
    }

    pub fn set(self, theme: &mut Theme, color: u32) {
        match self {
            Self::DimColor => theme.dim_color = color,
            Self::Accent => theme.accent = color,
            Self::PinBorder => theme.pin_border = color,
            Self::ChipBg => theme.chip_bg = color,
            Self::HintText => theme.hint_text = color,
            Self::BtnText => theme.btn_text = color,
            Self::BtnHoverBg => theme.btn_hover_bg = color,
            Self::ToolbarBg => theme.toolbar_bg = color,
            Self::ToolbarText => theme.toolbar_text = color,
            Self::ToolbarBorder => theme.toolbar_border = color,
            Self::ToolbarHover => theme.toolbar_hover = color,
            Self::ToolbarSelected => theme.toolbar_selected = color,
            Self::SwatchBorder => theme.swatch_border = color,
        }
    }
}

#[derive(Clone, Copy)]
struct Palettes {
    fixed_or_dark: Theme,
    light: Option<Theme>,
}
/// Replaceable on purpose: the settings window's Appearance editor
/// writes the working theme here on every change, live-previewing the
/// next capture's palette in its own chrome. (The overlay process is
/// short-lived and never edits; it installs once at startup.)
static ACTIVE: RwLock<Palettes> = RwLock::new(Palettes {
    fixed_or_dark: Theme::dark(),
    light: None,
});
static SYSTEM_DARK: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

impl Palettes {
    fn current(&self, dark: bool) -> Theme {
        if dark {
            self.fixed_or_dark
        } else {
            self.light.unwrap_or(self.fixed_or_dark)
        }
    }
}

/// The active palette. By value: the theme became replaceable (see
/// [`set`]) and a reference into the lock cannot outlive the read —
/// every field is `Copy`, so handing the struct out is free.
pub fn c() -> Theme {
    ACTIVE
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .current(SYSTEM_DARK.load(std::sync::atomic::Ordering::Relaxed))
}

/// True when the system appearance currently selects the dark palette
/// (the `auto` base follows it; the Appearance mockup shows whichever
/// palette this picks, exactly like a capture would).
pub fn system_dark() -> bool {
    SYSTEM_DARK.load(std::sync::atomic::Ordering::Relaxed)
}

pub fn set(theme: Theme, light: Option<Theme>) {
    *ACTIVE
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Palettes {
        fixed_or_dark: theme,
        light,
    };
}

/// Called by GPUI on initial window creation and appearance changes.
pub fn follow_system(appearance: gpui_kit::WindowAppearance) {
    SYSTEM_DARK.store(
        matches!(
            appearance,
            gpui_kit::WindowAppearance::Dark | gpui_kit::WindowAppearance::VibrantDark
        ),
        std::sync::atomic::Ordering::Relaxed,
    );
}

fn contrast(a: u32, b: u32) -> f32 {
    let luminance = |color: u32| {
        let rgb = color.to_be_bytes();
        let linear = |i: usize| {
            let v = rgb[i] as f32 / 255.;
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        linear(0) * 0.2126 + linear(1) * 0.7152 + linear(2) * 0.0722
    };
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_accents_keep_all_control_text_readable() {
        for mut theme in [Theme::dark(), Theme::light(), Theme::high_contrast()] {
            for accent in [0xFFFFFFFF, 0x000000FF, 0xFF6A00FF, 0x00FF00FF, 0x202028FF] {
                theme.accent = accent;
                theme.adapt_accent();
                assert!(contrast(theme.selected_text(), theme.toolbar_selected) >= 4.5);
                assert!(contrast(theme.accent_text(), theme.accent) >= 4.5);
                assert!(contrast(theme.toolbar_text, theme.toolbar_bg) >= 4.5);
                assert!(contrast(theme.toolbar_text, theme.toolbar_hover) >= 4.5);
            }
        }
    }

    #[test]
    fn appearance_changes_switch_whole_palettes_but_fixed_theme_stays() {
        let auto = Palettes {
            fixed_or_dark: Theme::dark(),
            light: Some(Theme::light()),
        };
        assert_eq!(auto.current(true).toolbar_bg, Theme::dark().toolbar_bg);
        assert_eq!(
            auto.current(false).toolbar_text,
            Theme::light().toolbar_text
        );
        let fixed = Palettes {
            fixed_or_dark: Theme::dark(),
            light: None,
        };
        assert_eq!(fixed.current(false).toolbar_bg, Theme::dark().toolbar_bg);
    }

    /// dark().dim() must equal the original hard-coded DIM constant.
    #[test]
    fn dark_dim_matches_legacy_constant() {
        assert_eq!(Theme::dark().dim(), 0x0000008C);
    }

    #[test]
    fn dim_opacity_clamps() {
        let mut t = Theme::dark();
        t.dim_opacity = -1.;
        assert_eq!(t.dim() & 0xFF, 0);
        t.dim_opacity = 2.;
        assert_eq!(t.dim() & 0xFF, 0xFF);
        t.dim_opacity = 0.;
        assert_eq!(t.dim(), 0x00000000);
    }
}
