//! # Shotori theme system
//!
//! Every visual constant of the overlay lives here, grouped into a
//! [`Theme`] struct instead of loose constants, so the look can be
//! swapped as a whole. Resolution order (see [`load`]):
//!
//! 1. a built-in base theme — `auto` (default, follows the system
//!    appearance) or one of the preinstalled palettes registered in
//!    [`load::BUILT_INS`] (the factory `dark`/`light`/`high_contrast`
//!    plus community palettes: Catppuccin, Dracula, Gruvbox, Nord,
//!    Rosé Pine, Tokyo Night, and the house `shotori` theme)
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
    /// (`toolbar_selected` is the exact `adapt_accent` derivation —
    /// every load path re-derives it, the literal just keeps direct
    /// `Theme::dark()` users in sync with them.)
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
            toolbar_selected: 0x3A2823FF,
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
            toolbar_selected: 0xFAE8DDFF,
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
            toolbar_selected: 0xFFEDE0FF,
            swatch_border: 0x00000059,
            ..dark_const()
        }
    }

    // ── Preinstalled community palettes ────────────────────────────────────
    // Mapping conventions (kept uniform across the set so a new palette
    // is mechanical): `toolbar_*` surfaces come from the source's
    // background/surface ramp (hover = one step lighter than bg,
    // border = two steps), `chip_bg` = the darkest step at the usual
    // 0xE6 alpha, text roles from the fg/subtext ramp, `accent` = the
    // palette's signature color, `pin_border` = accent at 0x99 (the
    // historical single-knob tint), `toolbar_selected` = accent mixed
    // 12% into toolbar_bg (the `adapt_accent` ratio), `swatch_border` =
    // the fg ramp at 0x38/0x26 alpha like the factory rows. Alphas are
    // baked in as literals — the theme.toml format cannot express them
    // (opaque hex only), which is why these are code palettes, not
    // embedded TOML files.

    /// The house theme: deep charcoal-blue surfaces, teal accent —
    /// shotori's own look, distinct from the historical orange default.
    pub fn shotori() -> Self {
        Self {
            dim_color: 0x0B0E14FF,
            dim_opacity: 0.55,
            accent: 0x2DD4BFFF,
            pin_border: 0x2DD4BF99,
            chip_bg: 0x10131AE6,
            hint_text: 0x9AA3B2FF,
            btn_text: 0xE8ECF1FF,
            btn_hover_bg: 0xFFFFFF1F,
            toolbar_bg: 0x15171FFF,
            toolbar_text: 0xE8ECF1FF,
            toolbar_border: 0x30343FFF,
            toolbar_hover: 0x1E212BFF,
            toolbar_selected: 0x172D32FF,
            swatch_border: 0xFFFFFF38,
        }
    }

    /// Catppuccin Frappé — the lightest of Catppuccin's dark flavors.
    pub fn catppuccin_frappe() -> Self {
        Self {
            dim_color: 0x232634FF,
            dim_opacity: 0.55,
            accent: 0xCA9EE6FF,
            pin_border: 0xCA9EE699,
            chip_bg: 0x292C3CE6,
            hint_text: 0xA5ADCEFF,
            btn_text: 0xC6D0F5FF,
            btn_hover_bg: 0x51576D4D,
            toolbar_bg: 0x303446FF,
            toolbar_text: 0xC6D0F5FF,
            toolbar_border: 0x51576DFF,
            toolbar_hover: 0x414559FF,
            toolbar_selected: 0x424059FF,
            swatch_border: 0x8993B259,
        }
    }

    /// Catppuccin Macchiato — Catppuccin's middle dark flavor.
    pub fn catppuccin_macchiato() -> Self {
        Self {
            dim_color: 0x181926FF,
            dim_opacity: 0.55,
            accent: 0xC6A0F6FF,
            pin_border: 0xC6A0F699,
            chip_bg: 0x1E2030E6,
            hint_text: 0xA5ADCBFF,
            btn_text: 0xCAD3F5FF,
            btn_hover_bg: 0x494D644D,
            toolbar_bg: 0x24273AFF,
            toolbar_text: 0xCAD3F5FF,
            toolbar_border: 0x494D64FF,
            toolbar_hover: 0x363A4FFF,
            toolbar_selected: 0x373550FF,
            swatch_border: 0x8087A259,
        }
    }

    /// Catppuccin Mocha — Catppuccin's darkest flavor.
    pub fn catppuccin_mocha() -> Self {
        Self {
            dim_color: 0x11111BFF,
            dim_opacity: 0.55,
            accent: 0xCBA6F7FF,
            pin_border: 0xCBA6F799,
            chip_bg: 0x181825E6,
            hint_text: 0xA6ADC8FF,
            btn_text: 0xCDD6F4FF,
            btn_hover_bg: 0x45475A4D,
            toolbar_bg: 0x1E1E2EFF,
            toolbar_text: 0xCDD6F4FF,
            toolbar_border: 0x45475AFF,
            toolbar_hover: 0x313244FF,
            toolbar_selected: 0x322E46FF,
            swatch_border: 0x9399B259,
        }
    }

    /// Catppuccin Latte — Catppuccin's light flavor (dim stays dark,
    /// like the factory light palette: dimming bright wallpapers).
    pub fn catppuccin_latte() -> Self {
        Self {
            dim_color: 0x000000FF,
            dim_opacity: 0.55,
            accent: 0x8839EFFF,
            pin_border: 0x8839EF99,
            chip_bg: 0xE6E9EFE6,
            hint_text: 0x6C6F85FF,
            btn_text: 0x4C4F69FF,
            btn_hover_bg: 0x00000014,
            toolbar_bg: 0xEFF1F5FF,
            toolbar_text: 0x4C4F69FF,
            toolbar_border: 0xBCC0CCFF,
            toolbar_hover: 0xE6E9EFFF,
            toolbar_selected: 0xE2DAF4FF,
            swatch_border: 0x00000026,
        }
    }

    /// Dracula — `toolbar_selected` and friends follow the standard
    /// accent-into-background mix; hint is the canonical comment color
    /// lightened just enough to clear 4.5:1 on the chip.
    pub fn dracula() -> Self {
        Self {
            dim_color: 0x21222CFF,
            dim_opacity: 0.55,
            accent: 0xBD93F9FF,
            pin_border: 0xBD93F999,
            chip_bg: 0x21222CE6,
            hint_text: 0x8B95CCFF,
            btn_text: 0xF8F8F2FF,
            btn_hover_bg: 0xF8F8F21F,
            toolbar_bg: 0x282A36FF,
            toolbar_text: 0xF8F8F2FF,
            toolbar_border: 0x44475AFF,
            toolbar_hover: 0x363948FF,
            toolbar_selected: 0x39364DFF,
            swatch_border: 0xF8F8F238,
        }
    }

    /// Gruvbox Dark — warm surfaces, the yellow as the accent (the
    /// factory orange default would read as "still shotori dark").
    pub fn gruvbox_dark() -> Self {
        Self {
            dim_color: 0x1D2021FF,
            dim_opacity: 0.55,
            accent: 0xFABD2FFF,
            pin_border: 0xFABD2F99,
            chip_bg: 0x1D2021E6,
            hint_text: 0xA89984FF,
            btn_text: 0xEBDBB2FF,
            btn_hover_bg: 0xFBF1C71A,
            toolbar_bg: 0x282828FF,
            toolbar_text: 0xEBDBB2FF,
            toolbar_border: 0x504945FF,
            toolbar_hover: 0x3C3836FF,
            toolbar_selected: 0x413928FF,
            swatch_border: 0xFBF1C738,
        }
    }

    /// Nord — frost accent over nord0 surfaces; the chip steps one
    /// shade darker than the toolbar for depth (both canonical nord0
    /// would leave the OCR dialog flat against the toolbar).
    pub fn nord() -> Self {
        Self {
            dim_color: 0x2E3440FF,
            dim_opacity: 0.55,
            accent: 0x88C0D0FF,
            pin_border: 0x88C0D099,
            chip_bg: 0x242933E6,
            hint_text: 0x81A1C1FF,
            btn_text: 0xE5E9F0FF,
            btn_hover_bg: 0xD8DEE921,
            toolbar_bg: 0x2E3440FF,
            toolbar_text: 0xE5E9F0FF,
            toolbar_border: 0x434C5EFF,
            toolbar_hover: 0x3B4252FF,
            toolbar_selected: 0x384451FF,
            swatch_border: 0xE5E9F038,
        }
    }

    /// Rosé Pine — love as the accent over the surface ramp.
    pub fn rose_pine() -> Self {
        Self {
            dim_color: 0x191724FF,
            dim_opacity: 0.55,
            accent: 0xEB6F92FF,
            pin_border: 0xEB6F9299,
            chip_bg: 0x191724E6,
            hint_text: 0x908CAAFF,
            btn_text: 0xE0DEF4FF,
            btn_hover_bg: 0xE0DEF41A,
            toolbar_bg: 0x1F1D2EFF,
            toolbar_text: 0xE0DEF4FF,
            toolbar_border: 0x403D52FF,
            toolbar_hover: 0x26233AFF,
            toolbar_selected: 0x37263AFF,
            swatch_border: 0xE0DEF438,
        }
    }

    /// Tokyo Night — the blue accent over its deep navy ramp.
    pub fn tokyo_night() -> Self {
        Self {
            dim_color: 0x16161EFF,
            dim_opacity: 0.55,
            accent: 0x7AA2F7FF,
            pin_border: 0x7AA2F799,
            chip_bg: 0x16161EE6,
            hint_text: 0x737AA2FF,
            btn_text: 0xC0CAF5FF,
            btn_hover_bg: 0xC0CAF521,
            toolbar_bg: 0x1A1B26FF,
            toolbar_text: 0xC0CAF5FF,
            toolbar_border: 0x3B4261FF,
            toolbar_hover: 0x292E42FF,
            toolbar_selected: 0x252B3FFF,
            swatch_border: 0xC0CAF538,
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

    /// The palette's inherent appearance: dark surfaces → dark. The
    /// settings shell picks the design-system mode from this, so a
    /// fixed light base (e.g. catppuccin-latte) gets light component
    /// chrome even on a dark system.
    pub fn is_dark(&self) -> bool {
        luminance(self.toolbar_bg) < 0.5
    }
}

/// `..dark()` spelled for const fn (struct-update syntax is not const).
const fn dark_const() -> Theme {
    Theme::dark()
}

/// One addressable solid-color field of [`Theme`] — the registry the
/// Appearance editor and `theme.toml` both speak. Labels, file keys
/// and accessors come from here and nowhere else, so the picker grid
/// and the save format cannot drift apart. The derived
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
/// (the `auto` base follows it; the Appearance editor resolves its
/// pickers through whichever palette this picks, like a capture would).
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

/// WCAG relative luminance of an 0xRRGGBB(A) color.
fn luminance(color: u32) -> f32 {
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
}

fn contrast(a: u32, b: u32) -> f32 {
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_appearance_drives_the_shell_mode() {
        // `is_dark` picks the design-system mode for the settings
        // shell (sync_shell_palette): every dark base must read dark,
        // every light base light — a fixed light base must not inherit
        // dark chrome from a dark system.
        let darks = [
            Theme::dark(),
            Theme::shotori(),
            Theme::catppuccin_frappe(),
            Theme::catppuccin_macchiato(),
            Theme::catppuccin_mocha(),
            Theme::dracula(),
            Theme::gruvbox_dark(),
            Theme::nord(),
            Theme::rose_pine(),
            Theme::tokyo_night(),
        ];
        let lights = [
            Theme::light(),
            Theme::high_contrast(),
            Theme::catppuccin_latte(),
        ];
        for theme in darks {
            assert!(theme.is_dark(), "{:08X}", theme.toolbar_bg);
        }
        for theme in lights {
            assert!(!theme.is_dark(), "{:08X}", theme.toolbar_bg);
        }
    }

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
