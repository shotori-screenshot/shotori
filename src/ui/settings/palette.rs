//! Project the resolved capture palette onto semantic editor colors.
//! Contrast adjustments keep custom colors readable on their active surfaces.
use gpui_kit::component::{ActiveTheme, Theme, ThemeColor, ThemeMode};
use gpui_kit::{App, BoxShadow, Hsla, Rgba, px, rgba};

use crate::ui::theme;

/// Layered 3-tier tonal surfaces, hairlines, and elevation tokens for
/// `SettingsWindow`, derived from the active palette without changing
/// component base tokens.
pub(super) struct SettingsSurfaces {
    pub(super) sidebar_bg: Hsla,
    pub(super) sidebar_text: Hsla,
    pub(super) canvas_bg: Hsla,
    pub(super) card_bg: Hsla,
    pub(super) selected_card_bg: Hsla,
    pub(super) well_bg: Hsla,
    pub(super) hairline: Hsla,
    pub(super) divider: Hsla,
    pub(super) pill_bg: Hsla,
    pub(super) pill_hover: Hsla,
    pub(super) card_shadow: Vec<BoxShadow>,
    pub(super) selected_shadow: Vec<BoxShadow>,
}

pub(super) fn surfaces(cx: &App) -> SettingsSurfaces {
    let t = cx.theme();
    let dark = t.is_dark();
    let white = color(0xFFFF_FFFF);
    let black = color(0x0000_00FF);

    let canvas_bg = if dark {
        if relative_luminance(t.background) > 0.012 {
            t.background.blend(black.opacity(0.30))
        } else {
            color(0x0909_0CFF)
        }
    } else if relative_luminance(t.background) > 0.88 {
        t.background.blend(black.opacity(0.052))
    } else {
        t.background.blend(black.opacity(0.038))
    };

    let card_bg = if dark {
        t.background.blend(white.opacity(0.052))
    } else {
        t.group_box.blend(white.opacity(0.90))
    };

    // Floating navigation island sits cleanly elevated above `canvas_bg`
    // rather than sunken into a dark side strip.
    let sidebar_bg = if dark {
        t.background.blend(white.opacity(0.026))
    } else {
        card_bg.blend(canvas_bg.opacity(0.28))
    };

    let sidebar_text = if contrast(t.foreground, sidebar_bg) >= 6.0 {
        t.foreground
    } else {
        readable_color(t.foreground, sidebar_bg, 6.0)
    };

    let well_bg = if dark {
        card_bg.blend(black.opacity(0.24))
    } else {
        card_bg.blend(black.opacity(0.042))
    };

    let hairline = t.border.opacity(if dark { 0.42 } else { 0.55 });
    let divider = t.border.opacity(if dark { 0.24 } else { 0.34 });
    let pill_bg = if dark {
        t.background.blend(white.opacity(0.06))
    } else {
        card_bg
    };
    let pill_hover = if dark {
        t.secondary_hover.blend(white.opacity(0.06))
    } else {
        t.secondary_hover
    };

    let card_shadow = if dark {
        vec![
            BoxShadow::new(px(0.), px(1.), rgba(0x0000_0028).into()).blur_radius(px(2.)),
            BoxShadow::new(px(0.), px(4.), rgba(0x0000_0030).into())
                .blur_radius(px(7.))
                .spread_radius(px(-1.)),
        ]
    } else {
        vec![
            BoxShadow::new(px(0.), px(1.), rgba(0x0000_000A).into()).blur_radius(px(1.5)),
            BoxShadow::new(px(0.), px(4.), rgba(0x0000_0010).into())
                .blur_radius(px(7.))
                .spread_radius(px(-1.)),
        ]
    };

    let selected_card_bg = card_bg.blend(t.primary.opacity(if dark { 0.14 } else { 0.10 }));
    let selected_shadow = vec![
        BoxShadow::new(
            px(0.),
            px(2.),
            t.primary.opacity(if dark { 0.22 } else { 0.16 }),
        )
        .blur_radius(px(6.)),
    ];

    SettingsSurfaces {
        sidebar_bg,
        sidebar_text,
        canvas_bg,
        card_bg,
        selected_card_bg,
        well_bg,
        hairline,
        divider,
        pill_bg,
        pill_hover,
        card_shadow,
        selected_shadow,
    }
}

pub(super) fn apply(cx: &mut App, palette: &theme::Theme) {
    let mode = if palette.is_dark() {
        ThemeMode::Dark
    } else {
        ThemeMode::Light
    };
    if Theme::global(cx).mode != mode {
        Theme::change(mode, None, cx);
    }
    let colors = editor_colors(palette);
    Theme::update(cx, |theme| {
        theme.colors = colors;
        theme.tokens = colors.into();
        theme.radius = px(10.);
        theme.radius_lg = px(14.);
    });
    Theme::sync_base(cx);
}

fn color(value: u32) -> Hsla {
    rgba(value).into()
}

fn editor_colors(palette: &theme::Theme) -> ThemeColor {
    let dark = palette.is_dark();
    let mut c = if dark {
        *ThemeColor::dark()
    } else {
        *ThemeColor::light()
    };
    let background = color(palette.toolbar_bg).alpha(1.);
    let foreground = readable_color(color(palette.toolbar_text), background, 7.);
    let sidebar = color(palette.chip_bg).alpha(1.);
    let surface = background.blend(if dark {
        foreground.opacity(0.035)
    } else {
        color(0xFFFFFFFF).opacity(0.65)
    });
    let text_surface = if contrast(foreground, surface) < contrast(foreground, background) {
        surface
    } else {
        background
    };
    let muted = readable_color(color(palette.hint_text), text_surface, 4.5);
    let border = color(palette.toolbar_border).alpha(1.);
    let hover = color(palette.toolbar_hover).alpha(1.);
    let primary = readable_color(color(palette.accent), background, 4.5);
    let white = color(0xFFFFFFFF);
    let on_primary = if relative_luminance(primary) > 0.38 {
        color(0x1818_20FF)
    } else {
        color(0xF7F8_FCFF)
    };
    let selected = background.blend(primary.opacity(0.16));
    c.background = background;
    c.foreground = foreground;
    c.border = border;
    c.sidebar = sidebar;
    c.sidebar_foreground = readable_color(foreground, sidebar, 4.5);
    c.sidebar_border = border;
    c.sidebar_accent = selected;
    c.sidebar_accent_foreground = foreground;
    c.title_bar = sidebar;
    c.title_bar_border = border;
    c.status_bar = sidebar;
    c.status_bar_border = border;
    c.popover = surface;
    c.popover_foreground = foreground;
    c.group_box = surface;
    c.group_box_foreground = foreground;
    c.muted = sidebar;
    c.muted_foreground = muted;
    c.accent = selected;
    c.accent_foreground = foreground;
    c.primary = primary;
    c.primary_hover = primary.blend(foreground.opacity(0.08));
    c.primary_active = primary.blend(foreground.opacity(0.16));
    c.primary_foreground = on_primary;
    c.secondary = sidebar;
    c.secondary_foreground = foreground;
    c.secondary_hover = hover;
    c.secondary_active = selected;
    c.button = surface;
    c.button_foreground = foreground;
    c.button_hover = hover;
    c.button_active = selected;
    c.input = border;
    c.ring = primary;
    c.caret = foreground;
    c.selection = selected;
    c.link = primary;
    c.link_hover = primary;
    c.link_active = primary;
    c.list = surface;
    c.list_hover = hover;
    c.list_active = selected;
    c.list_active_border = primary;
    c.list_head = sidebar;
    c.tab = sidebar;
    c.tab_bar = sidebar;
    c.tab_bar_segmented = sidebar;
    c.tab_active = surface;
    c.tab_active_foreground = foreground;
    c.tab_foreground = muted;
    c.switch = if dark {
        surface.blend(foreground.opacity(0.15))
    } else {
        background.blend(foreground.opacity(0.14))
    };
    c.switch_thumb = if dark {
        surface.blend(foreground.opacity(0.76))
    } else {
        background.blend(white.opacity(0.92))
    };
    c.slider_bar = surface.blend(primary.opacity(if dark { 0.78 } else { 0.84 }));
    c.slider_thumb = if dark {
        surface.blend(foreground.opacity(0.78))
    } else {
        background.blend(white.opacity(0.94))
    };
    c.scrollbar = background.opacity(0.);
    c.scrollbar_thumb = foreground.opacity(0.3);
    c.scrollbar_thumb_hover = foreground.opacity(0.5);
    c.drop_target = selected;
    c.drag_border = primary;
    c
}

fn readable_color(mut candidate: Hsla, background: Hsla, min_contrast: f32) -> Hsla {
    let white = color(0xFFFFFFFF);
    let black = color(0x000000FF);
    let target = if contrast(white, background) > contrast(black, background) {
        white
    } else {
        black
    };
    for _ in 0..16 {
        if contrast(candidate, background) >= min_contrast {
            return candidate;
        }
        candidate = candidate.blend(target.opacity(0.12));
    }
    target
}

pub(super) fn is_light_color(color: Hsla) -> bool {
    relative_luminance(color) > 0.50
}

fn relative_luminance(color: Hsla) -> f32 {
    let color: Rgba = color.into();
    let linear = |channel: f32| {
        if channel <= 0.04045 {
            channel / 12.92
        } else {
            ((channel + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * linear(color.r) + 0.7152 * linear(color.g) + 0.0722 * linear(color.b)
}

fn contrast(a: Hsla, b: Hsla) -> f32 {
    let a = relative_luminance(a);
    let b = relative_luminance(b);
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

#[cfg(test)]
mod tests {
    use super::{apply, color, contrast, editor_colors};
    use crate::ui::theme::{Theme as CaptureTheme, load};
    use gpui_kit::TestAppContext;
    use gpui_kit::component::{Theme, ThemeMode};

    #[test]
    fn controls_and_labels_remain_legible_with_every_palette_and_custom_accents() {
        let palettes = load::BUILT_INS
            .iter()
            .map(|preset| load::built_in(preset.name).unwrap())
            .chain([CaptureTheme::light(), CaptureTheme::dark()]);
        for palette in palettes {
            for accent in [
                palette.accent,
                0xFFFFFFFF,
                0x000000FF,
                0xFFFF00FF,
                0x101010FF,
            ] {
                let c = editor_colors(&CaptureTheme { accent, ..palette });
                assert!(contrast(c.foreground, c.background) >= 7.);
                assert!(contrast(c.muted_foreground, c.background) >= 4.5);
                assert!(contrast(c.muted_foreground, c.group_box) >= 4.5);
                assert!(contrast(c.sidebar_foreground, c.sidebar) >= 4.5);
                assert!(contrast(c.sidebar_accent_foreground, c.sidebar_accent) >= 4.5);
                assert!(contrast(c.primary, c.background) >= 4.5);
                assert_ne!(c.switch_thumb, color(0x000000FF));
                assert_ne!(c.slider_thumb, color(0x000000FF));
                assert!(super::relative_luminance(c.switch_thumb) > 0.20);
                assert!(super::relative_luminance(c.slider_thumb) > 0.20);
                assert_ne!(c.switch_thumb, c.switch);
            }
        }
    }

    #[test]
    fn related_presets_keep_distinct_surfaces_with_the_same_accent() {
        let mut backgrounds = Vec::new();
        let mut sidebars = Vec::new();
        for mut palette in [
            CaptureTheme::catppuccin_frappe(),
            CaptureTheme::catppuccin_macchiato(),
            CaptureTheme::catppuccin_mocha(),
            CaptureTheme::dracula(),
        ] {
            palette.accent = 0xAA88DDFF;
            let c = editor_colors(&palette);
            assert!(!backgrounds.contains(&c.background));
            assert!(!sidebars.contains(&c.sidebar));
            backgrounds.push(c.background);
            sidebars.push(c.sidebar);
        }
    }

    #[gpui_kit::test]
    fn switching_presets_updates_component_and_base_themes_in_one_app(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::base::init(cx);
            gpui_kit::component::init(cx);
            for palette in [
                CaptureTheme::catppuccin_frappe(),
                CaptureTheme::catppuccin_macchiato(),
                CaptureTheme::catppuccin_mocha(),
                CaptureTheme::catppuccin_latte(),
                CaptureTheme::dark(),
            ] {
                apply(cx, &palette);
                let active = Theme::global(cx);
                assert_eq!(active.background, color(palette.toolbar_bg));
                assert_eq!(active.sidebar, color(palette.chip_bg).alpha(1.));
                assert_eq!(active.border, color(palette.toolbar_border));
                assert_eq!(active.tokens.background.color, active.background);
                assert_eq!(active.tokens.accent.color, active.accent);
                assert_eq!(active.tokens.primary.color, active.primary);
                assert_eq!(active.is_dark(), palette.is_dark());
                let base = gpui_kit::base::Theme::global(cx);
                assert_eq!(base.tokens.colors.background, active.background);
                assert_eq!(base.tokens.colors.foreground, active.foreground);
                assert_eq!(
                    active.mode,
                    if palette.is_dark() {
                        ThemeMode::Dark
                    } else {
                        ThemeMode::Light
                    }
                );
            }
        });
    }

    #[gpui_kit::test]
    fn settings_surfaces_preserve_three_tier_hierarchy_and_legibility(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::base::init(cx);
            gpui_kit::component::init(cx);
            let palettes = load::BUILT_INS
                .iter()
                .map(|preset| load::built_in(preset.name).unwrap())
                .chain([
                    CaptureTheme::light(),
                    CaptureTheme::dark(),
                    CaptureTheme::capsule_light(),
                    CaptureTheme::capsule_dark(),
                ]);
            for palette in palettes {
                apply(cx, &palette);
                let s = super::surfaces(cx);
                let active = Theme::global(cx);
                assert_ne!(s.sidebar_bg, s.canvas_bg);
                assert_ne!(s.canvas_bg, s.card_bg);
                assert!(
                    contrast(active.foreground, s.card_bg) >= 6.5,
                    "foreground contrast on card_bg too low: {}",
                    contrast(active.foreground, s.card_bg)
                );
                assert!(
                    contrast(active.muted_foreground, s.card_bg) >= 4.2,
                    "muted contrast on card_bg too low: {}",
                    contrast(active.muted_foreground, s.card_bg)
                );
                assert!(
                    contrast(s.sidebar_text, s.sidebar_bg) >= 4.5,
                    "sidebar text contrast too low: {}",
                    contrast(s.sidebar_text, s.sidebar_bg)
                );
            }
        });
    }
}
