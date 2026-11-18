//! `[annotation]` settings block: initial tool sizes, styles, swatch indices,
//! and editing defaults.

use std::path::Path;

use serde::Deserialize;

use super::PALETTE;
use crate::annotation::text::TextWrap;
use crate::annotation::{ArrowStyle, HighlightBlend, NumberFormat};

pub(crate) const ARROW_STYLES: [ArrowStyle; 7] = [
    ArrowStyle::Open,
    ArrowStyle::Filled,
    ArrowStyle::Triangle,
    ArrowStyle::Outline,
    ArrowStyle::DoubleOpen,
    ArrowStyle::DoubleTriangle,
    ArrowStyle::Dimension,
];

pub(crate) const fn arrow_style_id(style: ArrowStyle) -> &'static str {
    match style {
        ArrowStyle::Open => "open",
        ArrowStyle::DoubleOpen => "double_open",
        ArrowStyle::Outline => "outline",
        ArrowStyle::Filled => "filled",
        ArrowStyle::Dimension => "dimension",
        ArrowStyle::Triangle => "triangle",
        ArrowStyle::DoubleTriangle => "double_triangle",
    }
}

pub(crate) fn arrow_style_from_id(id: &str) -> Option<ArrowStyle> {
    ARROW_STYLES
        .iter()
        .copied()
        .find(|s| arrow_style_id(*s) == id.trim())
}

pub(crate) fn arrow_style_label(style: ArrowStyle) -> std::borrow::Cow<'static, str> {
    match style {
        ArrowStyle::Open => crate::i18n::t!("toolbar.arrow_open"),
        ArrowStyle::DoubleOpen => crate::i18n::t!("toolbar.arrow_double_open"),
        ArrowStyle::Outline => crate::i18n::t!("toolbar.arrow_outline"),
        ArrowStyle::Filled => crate::i18n::t!("toolbar.arrow_filled"),
        ArrowStyle::Dimension => crate::i18n::t!("toolbar.arrow_dimension"),
        ArrowStyle::Triangle => crate::i18n::t!("toolbar.arrow_triangle"),
        ArrowStyle::DoubleTriangle => crate::i18n::t!("toolbar.arrow_double_triangle"),
    }
}

pub(crate) const HIGHLIGHT_BLENDS: [HighlightBlend; 2] =
    [HighlightBlend::Multiply, HighlightBlend::Translucent];

pub(crate) const fn highlight_blend_id(blend: HighlightBlend) -> &'static str {
    match blend {
        HighlightBlend::Multiply => "multiply",
        HighlightBlend::Translucent => "translucent",
    }
}

pub(crate) fn highlight_blend_from_id(id: &str) -> Option<HighlightBlend> {
    HIGHLIGHT_BLENDS
        .iter()
        .copied()
        .find(|b| highlight_blend_id(*b) == id.trim())
}

pub(crate) fn highlight_blend_label(blend: HighlightBlend) -> std::borrow::Cow<'static, str> {
    match blend {
        HighlightBlend::Multiply => crate::i18n::t!("toolbar.highlighter_multiply"),
        HighlightBlend::Translucent => crate::i18n::t!("toolbar.highlighter_translucent"),
    }
}

pub(crate) const NUMBER_FORMATS: [NumberFormat; 3] = [
    NumberFormat::Decimal,
    NumberFormat::Alphabetic,
    NumberFormat::Roman,
];

pub(crate) const fn number_format_id(format: NumberFormat) -> &'static str {
    match format {
        NumberFormat::Decimal => "decimal",
        NumberFormat::Alphabetic => "alphabetic",
        NumberFormat::Roman => "roman",
    }
}

pub(crate) fn number_format_from_id(id: &str) -> Option<NumberFormat> {
    NUMBER_FORMATS
        .iter()
        .copied()
        .find(|f| number_format_id(*f) == id.trim())
}

pub(crate) fn number_format_label(format: NumberFormat) -> std::borrow::Cow<'static, str> {
    match format {
        NumberFormat::Decimal => crate::i18n::t!("settings.annotation.number_format.decimal"),
        NumberFormat::Alphabetic => crate::i18n::t!("settings.annotation.number_format.alphabetic"),
        NumberFormat::Roman => crate::i18n::t!("settings.annotation.number_format.roman"),
    }
}

/// `[annotation]` configuration block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AnnotationSettings {
    pub text_wrap: TextWrap,
    pub renumber_on_delete: bool,
    /// Initial swatch index (`0..PALETTE`, default `0` = Red) for strokes,
    /// shapes, badges, and text.
    pub default_color_index: usize,
    /// Initial swatch index (`0..PALETTE`, default `2` = Yellow) for highlighter.
    pub highlighter_color_index: usize,
    /// Initial width (`1..=20`, default `3`) for shapes, lines, arrows, and pencil.
    pub stroke_width: u8,
    /// Initial highlighter brush width (`8..=60`, default `20`).
    pub highlighter_width: u8,
    pub highlighter_blend: HighlightBlend,
    /// Initial mosaic / blur strength (`4..=48`, default `16`).
    pub mosaic_strength: u8,
    /// Initial eraser brush diameter (`8..=96`, default `32`).
    pub eraser_size: u8,
    /// Initial text font size (`12..=72`, default `24`).
    pub text_size: u8,
    /// Initial sequence badge diameter (`16..=64`, default `32`).
    pub number_size: u8,
    /// Fill sequence badges by default (`true` = solid circle, `false` = outline ring).
    pub number_fill: bool,
    pub number_format: NumberFormat,
    pub arrow_style: ArrowStyle,
    /// Smooth freehand pencil strokes (`true` by default).
    pub smooth_pencil: bool,
    /// Initial spotlight backdrop opacity percentage (`0..=100`, default `55`).
    pub spotlight_opacity: u8,
    /// Default spotlight cutout shape (`false` = rectangle, `true` = ellipse).
    pub spotlight_ellipse: bool,
    /// Initial magnifier lens zoom (`1..=8`, default `2`).
    pub magnifier_zoom: u8,
    /// Default magnifier lens shape (`false` = rectangle, `true` = circle/ellipse).
    pub magnifier_ellipse: bool,
    /// Default drop shadow on magnifier lenses.
    pub magnifier_shadow: bool,
    /// Default rectangle corner radius (`0..=48`, default `0`).
    pub geometry_radius: u8,
    /// Default filled geometry mode for rectangles and ellipses.
    pub geometry_fill: bool,
    /// Default bold text annotations.
    pub text_bold: bool,
    /// Default high-contrast white outline around text annotations.
    pub text_outline: bool,
}

impl Default for AnnotationSettings {
    fn default() -> Self {
        Self {
            text_wrap: TextWrap::default(),
            renumber_on_delete: false,
            default_color_index: 0,
            highlighter_color_index: 2,
            stroke_width: 3,
            highlighter_width: 20,
            highlighter_blend: HighlightBlend::Multiply,
            mosaic_strength: 16,
            eraser_size: 32,
            text_size: 24,
            number_size: 32,
            number_fill: true,
            number_format: NumberFormat::Decimal,
            arrow_style: ArrowStyle::Open,
            smooth_pencil: true,
            spotlight_opacity: 55,
            spotlight_ellipse: false,
            magnifier_zoom: 2,
            magnifier_ellipse: false,
            magnifier_shadow: false,
            geometry_radius: 0,
            geometry_fill: false,
            text_bold: false,
            text_outline: false,
        }
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub(super) struct AnnotationFile {
    text_wrap: Option<String>,
    renumber_on_delete: Option<bool>,
    default_color_index: Option<i64>,
    highlighter_color_index: Option<i64>,
    stroke_width: Option<i64>,
    highlighter_width: Option<i64>,
    highlighter_blend: Option<String>,
    mosaic_strength: Option<i64>,
    eraser_size: Option<i64>,
    text_size: Option<i64>,
    number_size: Option<i64>,
    number_fill: Option<bool>,
    number_format: Option<String>,
    arrow_style: Option<String>,
    smooth_pencil: Option<bool>,
    spotlight_opacity: Option<i64>,
    spotlight_ellipse: Option<bool>,
    magnifier_zoom: Option<i64>,
    magnifier_ellipse: Option<bool>,
    magnifier_shadow: Option<bool>,
    geometry_radius: Option<i64>,
    geometry_fill: Option<bool>,
    text_bold: Option<bool>,
    text_outline: Option<bool>,
}

fn bounded_u8(
    raw: Option<i64>,
    min: i64,
    max: i64,
    default: u8,
    key: &str,
    path: &Path,
    errors: &mut Vec<String>,
) -> u8 {
    match raw {
        None => default,
        Some(v) if (min..=max).contains(&v) => v as u8,
        Some(v) => {
            errors.push(format!(
                "{}: annotation.{key} = {v} must be in {min}..={max} (using {default})",
                path.display()
            ));
            default
        }
    }
}

fn bounded_swatch(
    raw: Option<i64>,
    default: usize,
    key: &str,
    path: &Path,
    errors: &mut Vec<String>,
) -> usize {
    let max = (PALETTE - 1) as i64;
    match raw {
        None => default,
        Some(v) if (0..=max).contains(&v) => v as usize,
        Some(v) => {
            errors.push(format!(
                "{}: annotation.{key} = {v} must be in 0..={max} (using {default})",
                path.display()
            ));
            default
        }
    }
}

impl AnnotationFile {
    pub(super) fn resolve(self, path: &Path, errors: &mut Vec<String>) -> AnnotationSettings {
        let d = AnnotationSettings::default();
        let text_wrap = self.text_wrap.as_deref().map_or(d.text_wrap, |id| {
            TextWrap::from_id(id).unwrap_or_else(|| {
                errors.push(format!(
                    "{}: unknown annotation.text_wrap {id:?} (using word)",
                    path.display()
                ));
                d.text_wrap
            })
        });
        let highlighter_blend =
            self.highlighter_blend
                .as_deref()
                .map_or(d.highlighter_blend, |id| {
                    highlight_blend_from_id(id).unwrap_or_else(|| {
                        errors.push(format!(
                            "{}: unknown annotation.highlighter_blend {id:?} (using multiply)",
                            path.display()
                        ));
                        d.highlighter_blend
                    })
                });
        let number_format = self.number_format.as_deref().map_or(d.number_format, |id| {
            number_format_from_id(id).unwrap_or_else(|| {
                errors.push(format!(
                    "{}: unknown annotation.number_format {id:?} (using decimal)",
                    path.display()
                ));
                d.number_format
            })
        });
        let arrow_style = self.arrow_style.as_deref().map_or(d.arrow_style, |id| {
            arrow_style_from_id(id).unwrap_or_else(|| {
                errors.push(format!(
                    "{}: unknown annotation.arrow_style {id:?} (using open)",
                    path.display()
                ));
                d.arrow_style
            })
        });

        AnnotationSettings {
            text_wrap,
            renumber_on_delete: self.renumber_on_delete.unwrap_or(d.renumber_on_delete),
            default_color_index: bounded_swatch(
                self.default_color_index,
                d.default_color_index,
                "default_color_index",
                path,
                errors,
            ),
            highlighter_color_index: bounded_swatch(
                self.highlighter_color_index,
                d.highlighter_color_index,
                "highlighter_color_index",
                path,
                errors,
            ),
            stroke_width: bounded_u8(
                self.stroke_width,
                1,
                20,
                d.stroke_width,
                "stroke_width",
                path,
                errors,
            ),
            highlighter_width: bounded_u8(
                self.highlighter_width,
                8,
                60,
                d.highlighter_width,
                "highlighter_width",
                path,
                errors,
            ),
            highlighter_blend,
            mosaic_strength: bounded_u8(
                self.mosaic_strength,
                4,
                48,
                d.mosaic_strength,
                "mosaic_strength",
                path,
                errors,
            ),
            eraser_size: bounded_u8(
                self.eraser_size,
                8,
                96,
                d.eraser_size,
                "eraser_size",
                path,
                errors,
            ),
            text_size: bounded_u8(
                self.text_size,
                12,
                72,
                d.text_size,
                "text_size",
                path,
                errors,
            ),
            number_size: bounded_u8(
                self.number_size,
                16,
                64,
                d.number_size,
                "number_size",
                path,
                errors,
            ),
            number_fill: self.number_fill.unwrap_or(d.number_fill),
            number_format,
            arrow_style,
            smooth_pencil: self.smooth_pencil.unwrap_or(d.smooth_pencil),
            spotlight_opacity: bounded_u8(
                self.spotlight_opacity,
                0,
                100,
                d.spotlight_opacity,
                "spotlight_opacity",
                path,
                errors,
            ),
            spotlight_ellipse: self.spotlight_ellipse.unwrap_or(d.spotlight_ellipse),
            magnifier_zoom: bounded_u8(
                self.magnifier_zoom,
                1,
                8,
                d.magnifier_zoom,
                "magnifier_zoom",
                path,
                errors,
            ),
            magnifier_ellipse: self.magnifier_ellipse.unwrap_or(d.magnifier_ellipse),
            magnifier_shadow: self.magnifier_shadow.unwrap_or(d.magnifier_shadow),
            geometry_radius: bounded_u8(
                self.geometry_radius,
                0,
                48,
                d.geometry_radius,
                "geometry_radius",
                path,
                errors,
            ),
            geometry_fill: self.geometry_fill.unwrap_or(d.geometry_fill),
            text_bold: self.text_bold.unwrap_or(d.text_bold),
            text_outline: self.text_outline.unwrap_or(d.text_outline),
        }
    }
}

impl AnnotationSettings {
    pub(super) fn write_toml(
        &self,
        root: &mut toml::map::Map<String, toml::Value>,
        path: &Path,
    ) -> Result<(), String> {
        let section = root
            .entry("annotation")
            .or_insert_with(|| toml::Value::Table(toml::map::Map::new()));
        let Some(t) = section.as_table_mut() else {
            return Err(format!(
                "{}: [annotation] is not a table — refusing to overwrite",
                path.display()
            ));
        };
        t.insert("text_wrap".into(), toml::Value::from(self.text_wrap.id()));
        t.insert(
            "renumber_on_delete".into(),
            toml::Value::from(self.renumber_on_delete),
        );
        t.insert(
            "default_color_index".into(),
            toml::Value::from(self.default_color_index as i64),
        );
        t.insert(
            "highlighter_color_index".into(),
            toml::Value::from(self.highlighter_color_index as i64),
        );
        t.insert(
            "stroke_width".into(),
            toml::Value::from(i64::from(self.stroke_width)),
        );
        t.insert(
            "highlighter_width".into(),
            toml::Value::from(i64::from(self.highlighter_width)),
        );
        t.insert(
            "highlighter_blend".into(),
            toml::Value::from(highlight_blend_id(self.highlighter_blend)),
        );
        t.insert(
            "mosaic_strength".into(),
            toml::Value::from(i64::from(self.mosaic_strength)),
        );
        t.insert(
            "eraser_size".into(),
            toml::Value::from(i64::from(self.eraser_size)),
        );
        t.insert(
            "text_size".into(),
            toml::Value::from(i64::from(self.text_size)),
        );
        t.insert(
            "number_size".into(),
            toml::Value::from(i64::from(self.number_size)),
        );
        t.insert("number_fill".into(), toml::Value::from(self.number_fill));
        t.insert(
            "number_format".into(),
            toml::Value::from(number_format_id(self.number_format)),
        );
        t.insert(
            "arrow_style".into(),
            toml::Value::from(arrow_style_id(self.arrow_style)),
        );
        t.insert(
            "smooth_pencil".into(),
            toml::Value::from(self.smooth_pencil),
        );
        t.insert(
            "spotlight_opacity".into(),
            toml::Value::from(i64::from(self.spotlight_opacity)),
        );
        t.insert(
            "spotlight_ellipse".into(),
            toml::Value::from(self.spotlight_ellipse),
        );
        t.insert(
            "magnifier_zoom".into(),
            toml::Value::from(i64::from(self.magnifier_zoom)),
        );
        t.insert(
            "magnifier_ellipse".into(),
            toml::Value::from(self.magnifier_ellipse),
        );
        t.insert(
            "magnifier_shadow".into(),
            toml::Value::from(self.magnifier_shadow),
        );
        t.insert(
            "geometry_radius".into(),
            toml::Value::from(i64::from(self.geometry_radius)),
        );
        t.insert(
            "geometry_fill".into(),
            toml::Value::from(self.geometry_fill),
        );
        t.insert("text_bold".into(), toml::Value::from(self.text_bold));
        t.insert("text_outline".into(), toml::Value::from(self.text_outline));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn annotation_enum_ids_round_trip() {
        for style in ARROW_STYLES {
            assert_eq!(arrow_style_from_id(arrow_style_id(style)), Some(style));
        }
        for blend in HIGHLIGHT_BLENDS {
            assert_eq!(
                highlight_blend_from_id(highlight_blend_id(blend)),
                Some(blend)
            );
        }
        for fmt in NUMBER_FORMATS {
            assert_eq!(number_format_from_id(number_format_id(fmt)), Some(fmt));
        }
    }
}
