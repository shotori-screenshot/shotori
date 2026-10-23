//! Project the resolved capture palette onto semantic editor colors.
//! Contrast adjustments keep custom colors readable on their active surfaces.
use gpui_kit::component::{Theme, ThemeColor, ThemeMode};
use gpui_kit::{App, Hsla, Rgba, rgba};

use crate::ui::theme;

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
    Theme::update(cx, |theme| theme.colors = colors);
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
    let black = color(0x000000FF);
    let on_primary = if contrast(primary, white) > contrast(primary, black) {
        white
    } else {
        black
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
    c.switch = readable_color(background.blend(foreground.opacity(0.45)), on_primary, 3.);
    c.switch_thumb = on_primary;
    c.slider_bar = primary;
    c.slider_thumb = on_primary;
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

fn contrast(a: Hsla, b: Hsla) -> f32 {
    let luminance = |color: Hsla| {
        let color: Rgba = color.into();
        let linear = |channel: f32| {
            if channel <= 0.04045 {
                channel / 12.92
            } else {
                ((channel + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * linear(color.r) + 0.7152 * linear(color.g) + 0.0722 * linear(color.b)
    };
    let a = luminance(a);
    let b = luminance(b);
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
                assert!(contrast(c.switch_thumb, c.primary) >= 4.5);
                assert!(contrast(c.switch_thumb, c.switch) >= 3.);
                assert!(contrast(c.slider_thumb, c.slider_bar) >= 4.5);
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
}
